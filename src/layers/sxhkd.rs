use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only sxhkd binding adapter for bspwm-style sessions.
pub struct Sxhkd;

pub type BindingInventoryEntry = (KeyCombo, String);

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("bspwm"))
        })
        || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    let path = config_path()
        .ok_or_else(|| "sxhkdrc was not found; runtime sxhkd state remains unknown".to_owned())?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok(parse_bindings(&content)
        .into_iter()
        .map(|binding| (binding.combo, binding.command))
        .collect())
}

impl Sxhkd {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(path) = config_path() else {
            return LayerResult {
                layer: "sxhkd",
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: "sxhkd configuration is unavailable".into(),
                details: vec!["set SXHKD_CONFIG or provide ~/.config/sxhkd/sxhkdrc".into()],
            };
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult {
                    layer: "sxhkd",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not read sxhkd configuration".into(),
                    details: vec![format!("config: {}: {error}", path.display())],
                };
            }
        };
        let matches = parse_bindings(&content)
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let mut details = vec![format!("config: {}", path.display())];
        if matches.is_empty() {
            details.push(
                "no matching static sxhkd binding found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult {
                layer: "sxhkd",
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: "sxhkd shortcut state is conditional".into(),
                details,
            };
        }
        for binding in &matches {
            details.push(format!("binding: {}", binding.command));
        }
        LayerResult {
            layer: "sxhkd",
            id: LayerId::Compositor,
            outcome: Outcome::HandledUncertain,
            summary: "matching sxhkd shortcut configured; runtime activation is conditional".into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    command: String,
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("SXHKD_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("sxhkd/sxhkdrc");
    path.is_file().then_some(path)
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut bindings = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw_key = lines[index].trim();
        index += 1;
        if raw_key.is_empty() || raw_key.starts_with('#') || raw_key.starts_with("^") {
            continue;
        }
        let normalized = raw_key.split_whitespace().collect::<String>();
        if !normalized.contains('+') || normalized.contains(['{', '}', '|']) {
            continue;
        }
        let Ok(combo) = normalized.parse::<KeyCombo>() else {
            continue;
        };
        let Some(command_index) = (index..lines.len()).find(|position| {
            let line = lines[*position];
            line.chars().next().is_some_and(char::is_whitespace)
                && !line.trim().is_empty()
                && !line.trim_start().starts_with('#')
        }) else {
            break;
        };
        bindings.push(Binding {
            combo,
            command: lines[command_index].trim().to_owned(),
        });
        index = command_index + 1;
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_sxhkd_key_command_pairs() {
        let bindings = parse_bindings(
            "# comment\nsuper + Return\n    alacritty\n\nctrl + alt + t\n    notify-send terminal\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert_eq!(bindings[0].command, "alacritty");
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    #[test]
    fn ignores_dynamic_sxhkd_chords() {
        assert!(parse_bindings("super + {a,b}\n  echo dynamic\n").is_empty());
    }
}
