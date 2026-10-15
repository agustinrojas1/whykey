use crate::recording::PathInputs;
use std::path::PathBuf;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{EvidenceTier, LayerId, LayerResult, Outcome};

pub fn inspect(key: &KeyCombo) -> LayerResult {
    // `zellij setup --dump-config` prints the defaults built into the
    // installed binary, not the running session's state.
    let (source, content, complete, tier) = match config_path() {
        Some(path) => match crate::recording::read_to_string(&path) {
            Ok(content) if config_clears_defaults(&content) => (
                format!("config: {} (clear-defaults)", path.display()),
                content,
                false,
                EvidenceTier::ConfigFile,
            ),
            Ok(content) => match dump_default_config() {
                Some(defaults) => (
                    format!("source: zellij defaults + config: {}", path.display()),
                    format!("{defaults}\n{content}"),
                    true,
                    EvidenceTier::ConfigFile,
                ),
                None => (
                    format!("config: {}", path.display()),
                    content,
                    false,
                    EvidenceTier::ConfigFile,
                ),
            },
            Err(error) => {
                return unavailable(format!("failed to read {}: {error}", path.display()));
            }
        },
        None => match dump_default_config() {
            Some(content) => (
                "source: zellij setup --dump-config (built-in defaults)".into(),
                content,
                true,
                EvidenceTier::Default,
            ),
            None => {
                return unavailable(
                    "Zellij is active but no config.kdl or dumpable default config was found"
                        .into(),
                );
            }
        },
    };
    let (mode, mode_source, mode_known) = resolve_mode(
        crate::recording::env_var("WHYKEY_ZELLIJ_MODE")
            .ok()
            .as_deref(),
        &content,
    );
    let mut details = vec![source, format!("mode: {mode} ({mode_source})")];
    if let Some(binding) = find_binding(&content, key, &mode) {
        details.push(format!("binding: {}", binding));
        let continues = binding.starts_with("WriteChars") || binding.starts_with("Write ");
        if !mode_known {
            // Zellij does not report its current mode, and a key bound in the
            // assumed mode is often free in another (Ctrl g toggles Locked).
            details.push(format!(
                "Zellij does not report its current mode; this binding applies only while it is in {mode} mode"
            ));
            return LayerResult::new(
                "Zellij",
                LayerId::Multiplexer,
                Outcome::HandledUncertain,
                if continues {
                    "binding writes input to the pane if Zellij is in the assumed mode"
                } else {
                    "binding consumes the key if Zellij is in the assumed mode"
                },
                details,
            )
            .with_tier(tier);
        }
        return LayerResult::new(
            "Zellij",
            LayerId::Multiplexer,
            if continues {
                Outcome::HandledAndPassed
            } else {
                Outcome::Consumed
            },
            if continues {
                "binding writes input to the pane"
            } else {
                "binding consumes the key"
            },
            details,
        )
        .with_tier(tier);
    }
    details.push("no matching binding in the selected Zellij config/defaults".into());
    if !complete {
        details.push(
            "Zellij defaults or unsupported KDL bindings may still handle this key; forwarding is conditional".into(),
        );
    }
    LayerResult::new(
        "Zellij",
        LayerId::Multiplexer,
        unbound_outcome(complete),
        if complete {
            "no matching Zellij binding; forwarded to the pane"
        } else {
            "no parsed Zellij binding; forwarding cannot be proven"
        },
        details,
    )
    .with_tier(tier)
}

/// The mode the key is matched in, where it came from, and whether it is
/// known. `WHYKEY_ZELLIJ_MODE` names it; otherwise it is assumed to be the
/// config's `default_mode` (the mode Zellij starts and rests in), or
/// `normal`, Zellij's own default.
fn resolve_mode(env_mode: Option<&str>, content: &str) -> (String, &'static str, bool) {
    if let Some(mode) = env_mode.map(str::trim).filter(|mode| !mode.is_empty()) {
        return (mode.to_owned(), "from WHYKEY_ZELLIJ_MODE", true);
    }
    match default_mode(content) {
        Some(mode) => (mode, "assumed from the config's default_mode", false),
        None => ("normal".into(), "assumed: Zellij's default mode", false),
    }
}

/// A top-level `default_mode "locked"` setting, lowercased like the mode
/// blocks of `keybinds`.
fn default_mode(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.split("//").next()?.trim();
        let value = line.strip_prefix("default_mode")?;
        if !value.starts_with(char::is_whitespace) {
            return None;
        }
        let mode = value.trim().trim_matches('"').trim();
        (!mode.is_empty()).then(|| mode.to_ascii_lowercase())
    })
}

/// The verdict when no parsed binding matches. Complete defaults plus config
/// mean the key is forwarded. Otherwise the parse may have missed a binding,
/// but the key still continues unless one matches, as for a shell without its
/// init file (`UncertainContinues`, not `Unknown`, which would not say whether
/// the key continues at all).
fn unbound_outcome(complete: bool) -> Outcome {
    if complete {
        Outcome::Pass
    } else {
        Outcome::UncertainContinues
    }
}

fn unavailable(message: String) -> LayerResult {
    LayerResult::new(
        "Zellij",
        LayerId::Multiplexer,
        Outcome::Unavailable,
        "could not inspect Zellij key bindings",
        vec![message],
    )
}

fn config_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = crate::recording::env_var_os("ZELLIJ_CONFIG_FILE") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(directory) = crate::recording::env_var_os("ZELLIJ_CONFIG_DIR") {
        candidates.push(PathBuf::from(directory).join("config.kdl"));
    }
    if let Some(base) = crate::util::config_base() {
        candidates.push(base.join("zellij/config.kdl"));
    }
    candidates.push(PathBuf::from("/etc/zellij/config.kdl"));
    candidates.into_iter().find(|path| path.recorded_is_file())
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
    fn mode_is_known_only_from_the_environment_and_otherwise_assumes_default_mode() {
        let config = "default_mode \"locked\"\nkeybinds {\n}\n";
        assert_eq!(
            resolve_mode(Some("pane"), config),
            ("pane".into(), "from WHYKEY_ZELLIJ_MODE", true)
        );
        assert_eq!(
            resolve_mode(None, config),
            (
                "locked".into(),
                "assumed from the config's default_mode",
                false
            )
        );
        assert_eq!(
            resolve_mode(Some("  "), "keybinds {\n}\n"),
            ("normal".into(), "assumed: Zellij's default mode", false)
        );
        assert_eq!(default_mode("// default_mode \"locked\"\n"), None);
        assert_eq!(default_mode("default_modes \"x\"\n"), None);
    }

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
    fn unbound_key_continues_unless_the_binding_set_is_incomplete() {
        assert_eq!(unbound_outcome(true), Outcome::Pass);
        assert_eq!(unbound_outcome(false), Outcome::UncertainContinues);
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
