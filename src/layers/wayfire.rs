use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;

/// Read-only Wayfire INI shortcut adapter.
pub struct Wayfire;

pub type BindingInventoryEntry = (KeyCombo, String, String);

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
                .any(|name| name.trim().eq_ignore_ascii_case("wayfire"))
        })
        || config_path().is_some_and(|path| path.is_file())
}

pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    Ok(load_bindings()?
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.section))
        .collect())
}

impl Wayfire {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult::new(
                    "Wayfire",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect Wayfire shortcuts",
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
                "Wayfire",
                LayerId::Compositor,
                Outcome::Pass,
                "no matching Wayfire binding found",
                vec![format!("source: {}", config_description())],
            );
        }
        let mut details = vec![format!("source: {}", config_description())];
        for binding in matches {
            details.push(format!("binding: {} [{}]", binding.action, binding.section));
        }
        details.push(
            "Wayfire plugin activation, reload state, and runtime precedence remain conditional"
                .into(),
        );
        LayerResult::new(
            "Wayfire",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "Wayfire config contains a matching binding",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    section: String,
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let path = config_path().ok_or_else(|| "Wayfire config was not found".to_owned())?;
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
    if let Some(path) = env::var_os("WAYFIRE_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = crate::util::config_base()?;
    Some(base.join("wayfire.ini"))
}

fn config_description() -> String {
    config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Wayfire config".into())
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut section = String::new();
    for raw_line in content.lines() {
        let line = raw_line.split_once('#').map_or(raw_line, |(line, _)| line);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_owned();
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("binding_") {
            continue;
        }
        let value = value.trim();
        let Some(combo) = parse_accelerator(value) else {
            continue;
        };
        let action = name.trim_start_matches("binding_");
        if action.is_empty() {
            continue;
        }
        bindings.push(Binding {
            combo,
            action: action.to_owned(),
            section: section.clone(),
        });
        if bindings.len() >= MAX_BINDINGS {
            break;
        }
    }
    bindings
}

fn parse_accelerator(value: &str) -> Option<KeyCombo> {
    if value.eq_ignore_ascii_case("none") || value.contains(['{', '}', '|']) {
        return None;
    }
    let mut modifiers = Vec::new();
    let mut key = None;
    for raw in value.split_whitespace() {
        let token = raw.trim_matches(['<', '>']);
        if token.is_empty() {
            continue;
        }
        if let Some(modifier) = parse_modifier(token) {
            modifiers.push(modifier);
        } else if key.is_none() {
            key = Some(token.to_owned());
        } else {
            return None;
        }
    }
    let key = key?;
    let input = if modifiers.is_empty() {
        key
    } else {
        format!("{}+{key}", modifiers.join("+"))
    };
    input.parse().ok()
}

fn parse_modifier(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "super" | "mod4" | "meta" | "win" => Some("super"),
        "ctrl" | "control" | "primary" => Some("ctrl"),
        "alt" | "mod1" => Some("alt"),
        "shift" => Some("shift"),
        "caps" | "capslock" => Some("caps"),
        "num" | "numlock" | "mod2" => Some("mod2"),
        "hyper" | "mod3" => Some("mod3"),
        "mod5" => Some("mod5"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wayfire_shortcut_values() {
        let bindings = parse_bindings(
            "[command]\nbinding_terminal = <super> <shift> t\ncommand_terminal = foot\n[shortcuts]\nbinding_next_output = <alt> Right\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+shift+t".parse().unwrap());
        assert_eq!(bindings[0].action, "terminal");
        assert_eq!(bindings[1].combo, "alt+right".parse().unwrap());
    }

    #[test]
    fn rejects_wayfire_dynamic_or_disabled_values() {
        assert!(parse_accelerator("none").is_none());
        assert!(parse_accelerator("<super> {key}").is_none());
        assert!(parse_accelerator("<super> <ctrl> t").is_some());
    }
}
