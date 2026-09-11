use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;

/// Read-only River init-script adapter.
pub struct River;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let bindings = load_bindings().map_err(|error| format!("River: {error}"))?;
    Ok(bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "River",
                binding.combo.compact_display(),
                binding.action,
                "configured; runtime activation conditional",
            )
            .with_context(format!("riverctl map mode: {}", binding.mode))
        })
        .collect())
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("river"))
        })
        || config_path().is_some_and(|path| path.is_file())
}

pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

impl River {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult::new(
                    "River",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect River init bindings",
                    vec![error],
                );
            }
        };
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return LayerResult::new(
                "River",
                LayerId::Compositor,
                Outcome::Pass,
                "no matching River map command found",
                vec![format!("source: {}", config_description())],
            );
        }
        let mut details = vec![format!("source: {}", config_description())];
        for binding in matches {
            details.push(format!(
                "binding: {} (mode: {})",
                binding.action, binding.mode
            ));
        }
        details.push(
            "River init reload state, mode, and runtime map precedence remain conditional".into(),
        );
        LayerResult::new(
            "River",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "River init contains a matching map command",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    mode: String,
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let path = config_path().ok_or_else(|| "River init script was not found".to_owned())?;
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_CONFIG_BYTES}-byte config limit",
            path.display()
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    Ok(parse_bindings(&content))
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("RIVER_INIT") {
        return Some(PathBuf::from(path));
    }
    let base = crate::util::config_base()?;
    Some(base.join("river/init"))
}

fn config_description() -> String {
    config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "River init".into())
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.split_once('#').map_or(raw_line, |(line, _)| line);
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        let Some(map_index) = tokens.iter().position(|token| *token == "map") else {
            continue;
        };
        if tokens.len() <= map_index + 3 || tokens[map_index.saturating_sub(1)] != "riverctl" {
            continue;
        }
        let mode = tokens[map_index + 1];
        let Some((combo, key_index)) = parse_map_key(&tokens[map_index + 2..]) else {
            continue;
        };
        let action_index = map_index + 2 + key_index + 1;
        let action = tokens[action_index..].join(" ");
        if action.is_empty() {
            continue;
        }
        bindings.push(Binding {
            combo,
            action,
            mode: mode.to_owned(),
        });
        if bindings.len() >= MAX_BINDINGS {
            break;
        }
    }
    bindings
}

fn parse_map_key(tokens: &[&str]) -> Option<(KeyCombo, usize)> {
    if tokens.is_empty() {
        return None;
    }
    // River accepts both `Super+Shift Q` and `Super+Shift+Q` forms. In the
    // former, modifiers are one token and the key is the next token.
    if tokens[0].contains('+') {
        let pieces = tokens[0].split('+').collect::<Vec<_>>();
        if pieces.len() >= 2 {
            let inline_key = pieces.last().copied().unwrap_or_default();
            if !is_modifier(inline_key) {
                let input = pieces.join("+");
                if let Some(combo) = parse_accelerator(&input) {
                    return Some((combo, 0));
                }
            } else if let Some(key) = tokens.get(1) {
                let input = format!("{}+{key}", tokens[0]);
                if let Some(combo) = parse_accelerator(&input) {
                    return Some((combo, 1));
                }
            }
        }
    }
    let mut modifiers = Vec::new();
    let mut key_index = 0;
    while key_index < tokens.len() && is_modifier(tokens[key_index]) {
        modifiers.push(tokens[key_index]);
        key_index += 1;
    }
    let key = *tokens.get(key_index)?;
    let input = if modifiers.is_empty() {
        key.to_owned()
    } else {
        format!("{}+{key}", modifiers.join("+"))
    };
    Some((parse_accelerator(&input)?, key_index))
}

fn is_modifier(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "super" | "mod4" | "shift" | "control" | "ctrl" | "alt" | "mod1" | "mod2" | "mod3" | "mod5"
    )
}

fn parse_accelerator(value: &str) -> Option<KeyCombo> {
    if value.contains(['{', '}', '"', '|']) {
        return None;
    }
    let mut parts = value.split('+').collect::<Vec<_>>();
    let key = parts.pop()?.trim();
    if key.is_empty() {
        return None;
    }
    let mut modifiers = Vec::new();
    for modifier in parts {
        modifiers.push(match modifier.to_ascii_lowercase().as_str() {
            "super" | "mod4" | "meta" => "super",
            "shift" => "shift",
            "control" | "ctrl" => "ctrl",
            "alt" | "mod1" => "alt",
            "mod2" | "num" => "mod2",
            "mod3" | "hyper" => "mod3",
            "mod5" => "mod5",
            _ => return None,
        });
    }
    let input = if modifiers.is_empty() {
        key.to_owned()
    } else {
        format!("{}+{key}", modifiers.join("+"))
    };
    input.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_river_map_forms_and_preserves_modes() {
        let bindings = parse_bindings(
            "riverctl map normal Super+Shift Q close\nriverctl map normal Super+Return spawn foot\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+shift+q".parse().unwrap());
        assert_eq!(bindings[0].mode, "normal");
        assert_eq!(bindings[1].action, "spawn foot");
    }

    #[test]
    fn rejects_dynamic_river_keys() {
        assert!(parse_accelerator("Super+{key}").is_none());
        assert!(parse_accelerator("Super+Q").is_some());
    }
}
