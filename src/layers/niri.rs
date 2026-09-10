use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;

/// Read-only Niri `config.kdl` binding adapter.
pub struct Niri;

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
                .any(|name| name.trim().eq_ignore_ascii_case("niri"))
        })
        || env::var_os("NIRI_SOCKET").is_some()
        || config_path().is_some_and(|path| path.is_file())
}

/// Niri does not expose a stable global binding inventory command across
/// releases; a readable config is therefore the available source.
pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    Ok(load_bindings()?
        .into_iter()
        .map(|binding| (binding.combo, binding.action))
        .collect())
}

impl Niri {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult {
                    verbose_details: Vec::new(),
                    binding: None,
                    layer: "Niri",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect Niri key bindings".into(),
                    details: vec![error],
                };
            }
        };
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Niri",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no matching Niri binding found in config.kdl".into(),
                details: vec![format!("source: {}", config_description())],
            };
        }
        let mut details = vec![format!("source: {}", config_description())];
        for binding in matches {
            details.push(format!("binding: {}", binding.action));
        }
        details.push(
            "Niri runtime reload state, inhibitor state, and active mode remain conditional".into(),
        );
        LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Niri",
            id: LayerId::Compositor,
            outcome: Outcome::HandledUncertain,
            summary: "Niri config contains a matching binding".into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let path = config_path().ok_or_else(|| "Niri config.kdl was not found".to_owned())?;
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
    if let Some(path) = env::var_os("NIRI_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = crate::util::config_base()?;
    Some(base.join("niri/config.kdl"))
}

fn config_description() -> String {
    config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Niri config.kdl".into())
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut in_binds = false;
    let mut depth = 0_i32;
    let mut pending: Option<(KeyCombo, String)> = None;
    for raw_line in content.lines() {
        let line = raw_line.split_once("//").map_or(raw_line, |(line, _)| line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_binds {
            if trimmed.starts_with("binds") && trimmed.contains('{') {
                in_binds = true;
                depth = brace_delta(trimmed);
            }
            continue;
        }

        if let Some((combo, action)) = pending.as_mut() {
            action.push(' ');
            action.push_str(trimmed);
            if trimmed.contains('}') {
                let action = action.trim_end_matches('}').trim().trim_matches(';').trim();
                if !action.is_empty() {
                    bindings.push(Binding {
                        combo: combo.clone(),
                        action: action.to_owned(),
                    });
                }
                pending = None;
            }
        } else if let Some(open) = trimmed.find('{') {
            let prefix = trimmed[..open].trim();
            let Some(raw_key) = prefix.split_whitespace().next() else {
                depth += brace_delta(trimmed);
                continue;
            };
            if let Some(combo) = parse_accelerator(raw_key) {
                let mut action = trimmed[open + 1..].trim().to_owned();
                if action.contains('}') {
                    action = action
                        .trim_end_matches('}')
                        .trim()
                        .trim_matches(';')
                        .trim()
                        .to_owned();
                    if !action.is_empty() {
                        bindings.push(Binding { combo, action });
                    }
                } else {
                    pending = Some((combo, action));
                }
            }
        }
        depth += brace_delta(trimmed);
        if depth <= 0 && pending.is_none() {
            in_binds = false;
        }
        if bindings.len() >= MAX_BINDINGS {
            break;
        }
    }
    bindings
}

fn brace_delta(value: &str) -> i32 {
    value.chars().fold(0, |delta, character| match character {
        '{' => delta + 1,
        '}' => delta - 1,
        _ => delta,
    })
}

fn parse_accelerator(value: &str) -> Option<KeyCombo> {
    if value.contains(['{', '}', '"']) {
        return None;
    }
    let mut parts = value.split('+').collect::<Vec<_>>();
    let key = parts.pop()?.trim();
    if key.is_empty() || parts.is_empty() && key.eq_ignore_ascii_case("Mod") {
        return None;
    }
    let mut modifiers = Vec::new();
    for modifier in parts {
        modifiers.push(match modifier.to_ascii_lowercase().as_str() {
            "mod" | "super" | "meta" | "mod4" => "super",
            "ctrl" | "control" => "ctrl",
            "alt" => "alt",
            "shift" => "shift",
            "capslock" | "caps" => "caps",
            "numlock" | "num" => "num",
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
    fn parses_single_and_multiline_niri_bindings() {
        let bindings = parse_bindings(
            "binds {\n  Mod+Return { spawn \"foot\"; }\n  Mod+Shift+Q {\n    close-window\n  }\n}\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert!(bindings[0].action.contains("spawn"));
        assert_eq!(bindings[1].combo, "super+shift+q".parse().unwrap());
    }

    #[test]
    fn rejects_dynamic_or_modifier_only_accelerators() {
        assert!(parse_accelerator("Mod+{key}").is_none());
        assert!(parse_accelerator("Mod").is_none());
        assert!(parse_accelerator("Mod+Ctrl+Q").is_some());
    }
}
