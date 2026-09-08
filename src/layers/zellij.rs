use std::env;
use std::fs;
use std::path::PathBuf;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

pub fn inspect(key: &KeyCombo) -> LayerResult {
    let (source, content, complete) = match config_path() {
        Some(path) => match fs::read_to_string(&path) {
            Ok(content) if config_clears_defaults(&content) => (
                format!("config: {} (clear-defaults)", path.display()),
                content,
                false,
            ),
            Ok(content) => match dump_default_config() {
                Some(defaults) => (
                    format!("source: zellij defaults + config: {}", path.display()),
                    format!("{defaults}\n{content}"),
                    true,
                ),
                None => (format!("config: {}", path.display()), content, false),
            },
            Err(error) => {
                return unavailable(format!("failed to read {}: {error}", path.display()));
            }
        },
        None => match dump_default_config() {
            Some(content) => ("source: zellij setup --dump-config".into(), content, true),
            None => {
                return unavailable(
                    "Zellij is active but no config.kdl or dumpable default config was found"
                        .into(),
                );
            }
        },
    };
    let (mode, mode_source) = env::var("WHYKEY_ZELLIJ_MODE")
        .ok()
        .filter(|mode| !mode.trim().is_empty())
        .map(|mode| (mode.trim().to_owned(), "from WHYKEY_ZELLIJ_MODE"))
        .unwrap_or_else(|| ("normal".into(), "assumed"));
    let mut details = vec![source, format!("mode: {mode} ({mode_source})")];
    if let Some(binding) = find_binding(&content, key, &mode) {
        details.push(format!("binding: {}", binding));
        let continues = binding.starts_with("WriteChars") || binding.starts_with("Write ");
        return LayerResult {
            layer: "Zellij",
            id: LayerId::Multiplexer,
            outcome: if continues {
                Outcome::HandledAndPassed
            } else {
                Outcome::Consumed
            },
            summary: if continues {
                "binding writes input to the pane"
            } else {
                "binding consumes the key"
            }
            .into(),
            details,
        };
    }
    details.push("no matching binding in the selected Zellij config/defaults".into());
    if !complete {
        details.push(
            "Zellij defaults or unsupported KDL bindings may still handle this key; forwarding is conditional".into(),
        );
    }
    LayerResult {
        layer: "Zellij",
        id: LayerId::Multiplexer,
        outcome: if complete {
            Outcome::Pass
        } else {
            Outcome::Unknown
        },
        summary: if complete {
            "no matching Zellij binding; forwarded to the pane"
        } else {
            "no parsed Zellij binding; forwarding cannot be proven"
        }
        .into(),
        details,
    }
}

fn unavailable(message: String) -> LayerResult {
    LayerResult {
        layer: "Zellij",
        id: LayerId::Multiplexer,
        outcome: Outcome::Unavailable,
        summary: "could not inspect Zellij key bindings".into(),
        details: vec![message],
    }
}

fn config_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("ZELLIJ_CONFIG_FILE") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(directory) = env::var_os("ZELLIJ_CONFIG_DIR") {
        candidates.push(PathBuf::from(directory).join("config.kdl"));
    }
    if let Some(base) = crate::util::config_base() {
        candidates.push(base.join("zellij/config.kdl"));
    }
    candidates.push(PathBuf::from("/etc/zellij/config.kdl"));
    candidates.into_iter().find(|path| path.is_file())
}

fn dump_default_config() -> Option<String> {
    let mut command = std::process::Command::new("zellij");
    command.args(["setup", "--dump-config"]);
    let output = command::output(&mut command).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn config_clears_defaults(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default();
        let compact = line
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        compact.contains("keybindsclear-defaults=true")
    })
}

fn find_binding(content: &str, key: &KeyCombo, mode: &str) -> Option<String> {
    let mut in_normal = false;
    let mut normal_depth = 0_i32;
    let mut ignored_depth = 0_i32;
    let mode_start = format!("{mode} {{");
    let mode_start_compact = format!("{mode}{{");
    let mut found = None;
    for raw_line in content.lines() {
        let line = raw_line.split('#').next()?.trim();
        if line.is_empty() {
            continue;
        }
        if ignored_depth > 0 {
            ignored_depth += line.matches('{').count() as i32;
            ignored_depth -= line.matches('}').count() as i32;
            continue;
        }
        if line.starts_with("shared_except ") && quoted_values(line).contains(&mode) {
            ignored_depth = brace_depth(line);
            continue;
        }
        if line.starts_with("shared_among ") && !quoted_values(line).contains(&mode) {
            ignored_depth = brace_depth(line);
            continue;
        }
        if line.starts_with(&mode_start)
            || line == mode_start_compact
            || line.starts_with("shared_except ")
            || line.starts_with("shared_among ")
        {
            in_normal = true;
            normal_depth = 1;
            if let Some((_, body)) = line.split_once('{') {
                let body = body.trim().trim_end_matches('}').trim();
                if line_has_binding(body, key) {
                    found = Some(body.to_owned());
                }
            }
            continue;
        }
        if in_normal {
            normal_depth += line.matches('{').count() as i32;
            normal_depth -= line.matches('}').count() as i32;
            if normal_depth <= 0 {
                in_normal = false;
                continue;
            }
            if line_has_binding(line, key) {
                found = Some(line.to_owned());
            }
        }
    }
    found
}

fn brace_depth(line: &str) -> i32 {
    let depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
    depth.max(1)
}

fn line_has_binding(line: &str, key: &KeyCombo) -> bool {
    line.starts_with("bind")
        && quoted_values(line)
            .iter()
            .any(|raw| parse_zellij_key(raw).as_ref() == Some(key))
}

fn quoted_values(line: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        values.push(&after_start[..end]);
        rest = &after_start[end + 1..];
    }
    values
}

fn parse_zellij_key(raw: &str) -> Option<KeyCombo> {
    let mut modifiers = 0;
    let mut key = None;
    for part in raw.split_whitespace() {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers |= 4,
            "alt" => modifiers |= 8,
            "shift" => modifiers |= 1,
            "super" | "meta" => modifiers |= 64,
            _ if key.is_none() => key = Some(lower),
            _ => return None,
        }
    }
    let key = key?;
    let key = match key.as_str() {
        "left" => "LEFT",
        "right" => "RIGHT",
        "up" => "UP",
        "down" => "DOWN",
        "enter" | "return" => "RETURN",
        "esc" | "escape" => "ESCAPE",
        "space" => "SPACE",
        "tab" => "TAB",
        value => value,
    };
    Some(KeyCombo::from_parts(modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zellij_key_names() {
        assert_eq!(
            parse_zellij_key("Ctrl Left").unwrap().to_string(),
            "CTRL + LEFT"
        );
        assert_eq!(
            parse_zellij_key("Alt Shift x").unwrap().to_string(),
            "ALT + SHIFT + X"
        );
    }

    #[test]
    fn finds_bindings_in_normal_mode() {
        let content = r#"keybinds {
            normal {
                bind "Ctrl Left" { MoveFocus "Left"; }
            }
        }"#;
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        assert!(find_binding(content, &key, "normal").is_some());
    }

    #[test]
    fn finds_bindings_in_shared_modes() {
        let content = r#"keybinds {
            shared_except "locked" {
                bind "Ctrl q" { Quit; }
            }
        }"#;
        let key: KeyCombo = "ctrl+q".parse().unwrap();
        assert!(find_binding(content, &key, "normal").is_some());
    }

    #[test]
    fn excludes_shared_bindings_for_another_mode() {
        let content = r#"keybinds {
            shared_except "locked" {
                bind "Ctrl q" { Quit; }
            }
        }"#;
        let key: KeyCombo = "ctrl+q".parse().unwrap();
        assert!(find_binding(content, &key, "locked").is_none());
    }

    #[test]
    fn selects_an_explicit_runtime_mode() {
        let content = r#"keybinds {
            normal { bind "Ctrl q" { WriteChars "normal"; } }
            pane { bind "Ctrl q" { CloseFocus; } }
        }"#;
        let key: KeyCombo = "ctrl+q".parse().unwrap();
        let binding = find_binding(content, &key, "pane").unwrap();
        assert!(binding.contains("CloseFocus"));
    }

    #[test]
    fn detects_when_zellij_defaults_are_cleared() {
        assert!(config_clears_defaults(
            "keybinds clear-defaults = true {\n normal { }\n}"
        ));
        assert!(!config_clears_defaults("keybinds {\n normal { }\n}"));
    }

    #[test]
    fn later_zellij_binding_overrides_dumped_default() {
        let content = r#"normal {
    bind "Ctrl g" { SwitchToMode "Locked"; }
}
normal {
    bind "Ctrl g" { WriteChars "g"; }
}"#;
        assert_eq!(
            find_binding(content, &"ctrl+g".parse().unwrap(), "normal").as_deref(),
            Some("bind \"Ctrl g\" { WriteChars \"g\"; }")
        );
    }
}
