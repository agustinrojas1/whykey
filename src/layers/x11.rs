use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only X11 `xbindkeys` configuration adapter.
///
/// A readable `.xbindkeysrc` is static evidence only: it does not prove that
/// the daemon loaded it, or that another X11 client consumes the key first.
pub struct X11;

pub type BindingInventoryEntry = (KeyCombo, String);

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    x11_session() || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    let path = config_path().ok_or_else(|| {
        "xbindkeys configuration was not found; runtime X11 bindings remain unknown".to_owned()
    })?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok(parse_bindings(&content)
        .into_iter()
        .map(|binding| (binding.combo, binding.command))
        .collect())
}

impl X11 {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(path) = config_path() else {
            return LayerResult {
                binding: None,
                layer: "X11 xbindkeys",
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: "xbindkeys configuration is unavailable".into(),
                details: vec![
                    "set XBINDKEYSRC or provide ~/.xbindkeysrc for static X11 shortcut evidence"
                        .into(),
                ],
            };
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult {
                    binding: None,
                    layer: "X11 xbindkeys",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not read xbindkeys configuration".into(),
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
                "no matching literal xbindkeys binding found; other X11 clients remain unknown"
                    .into(),
            );
            return LayerResult {
                binding: None,
                layer: "X11 xbindkeys",
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: "X11 shortcut state is conditional".into(),
                details,
            };
        }
        for binding in &matches {
            details.push(format!("binding: {}", binding.command));
        }
        LayerResult {
binding: None,
            layer: "X11 xbindkeys",
            id: LayerId::Compositor,
            outcome: Outcome::HandledUncertain,
            summary: "matching xbindkeys shortcut configured; daemon activation and X11 precedence are conditional".into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    command: String,
}

fn x11_session() -> bool {
    env::var("XDG_SESSION_TYPE")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("x11"))
        || (env::var_os("DISPLAY").is_some() && env::var_os("WAYLAND_DISPLAY").is_none())
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("XBINDKEYSRC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = env::var_os("HOME").map(|home| PathBuf::from(home).join(".xbindkeysrc"))?;
    path.is_file().then_some(path)
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let command_line = lines[index].trim();
        index += 1;
        if command_line.is_empty() || command_line.starts_with('#') {
            continue;
        }
        let Some(command) = parse_command(command_line) else {
            continue;
        };
        while index < lines.len() && lines[index].trim().is_empty() {
            index += 1;
        }
        let Some(raw_key) = lines.get(index).map(|line| line.trim()) else {
            break;
        };
        index += 1;
        if raw_key.is_empty() || raw_key.starts_with('#') {
            continue;
        }
        let normalized = raw_key.split_whitespace().collect::<String>();
        if normalized.contains(['{', '}', '|']) || normalized.contains("Release+") {
            continue;
        }
        let Ok(combo) = normalized.parse::<KeyCombo>() else {
            continue;
        };
        bindings.push(Binding { combo, command });
    }
    bindings
}

fn parse_command(line: &str) -> Option<String> {
    let line = line.strip_prefix('"')?;
    let end = line.find('"')?;
    let command = &line[..end];
    (!command.trim().is_empty()).then(|| command.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_xbindkeys_pairs() {
        let bindings = parse_bindings(
            "\"alacritty\"\n  Control+Alt + t\n\n# comment\n\"notify-send hi\"\n  Super+c\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "ctrl+alt+t".parse().unwrap());
        assert_eq!(bindings[1].combo, "super+c".parse().unwrap());
    }

    #[test]
    fn rejects_dynamic_and_release_bindings() {
        assert!(parse_bindings("\"run\"\n  Control+{a,b}\n").is_empty());
        assert!(parse_bindings("\"run\"\n  Release+Control+c\n").is_empty());
    }
}
