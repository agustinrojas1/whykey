use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only Openbox XML keybind adapter.
pub struct Openbox;

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
                .any(|name| name.trim().eq_ignore_ascii_case("openbox"))
        })
        || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    let path = config_path().ok_or_else(|| {
        "Openbox rc.xml was not found; runtime keybind state remains unknown".to_owned()
    })?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok(parse_bindings(&content)
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.key_name))
        .collect())
}

impl Openbox {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(path) = config_path() else {
            return LayerResult {
                layer: "Openbox",
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: "Openbox configuration is unavailable".into(),
                details: vec!["set OPENBOX_CONFIG or provide ~/.config/openbox/rc.xml".into()],
            };
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult {
                    layer: "Openbox",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not read Openbox configuration".into(),
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
                "no matching static Openbox keybind found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult {
                layer: "Openbox",
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: "Openbox shortcut state is conditional".into(),
                details,
            };
        }
        for binding in &matches {
            details.push(format!("binding: {}", binding.action));
        }
        LayerResult {
            layer: "Openbox",
            id: LayerId::Compositor,
            outcome: Outcome::HandledUncertain,
            summary: "matching Openbox keybind configured; runtime activation is conditional"
                .into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) combo: KeyCombo,
    pub(crate) action: String,
    pub(crate) key_name: String,
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("OPENBOX_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("openbox/rc.xml");
    path.is_file().then_some(path)
}

pub(crate) fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut remainder = content;
    while let Some(start) = remainder.find("<keybind") {
        remainder = &remainder[start..];
        let Some(open_end) = remainder.find('>') else {
            break;
        };
        let opening = &remainder[..=open_end];
        let Some(raw_key) = attribute(opening, "key") else {
            remainder = &remainder[open_end + 1..];
            continue;
        };
        let Some(close) = remainder[open_end + 1..].find("</keybind>") else {
            break;
        };
        let block_end = open_end + 1 + close;
        let block = &remainder[open_end + 1..block_end];
        if let Some(combo) = parse_key(&raw_key) {
            for action in parse_actions(block) {
                bindings.push(Binding {
                    combo: combo.clone(),
                    action,
                    key_name: raw_key.clone(),
                });
            }
        }
        remainder = &remainder[block_end + "</keybind>".len()..];
    }
    bindings
}

fn parse_actions(block: &str) -> Vec<String> {
    let mut actions = Vec::new();
    let mut remainder = block;
    while let Some(start) = remainder.find("<action") {
        remainder = &remainder[start..];
        let Some(open_end) = remainder.find('>') else {
            break;
        };
        let opening = &remainder[..=open_end];
        let name = attribute(opening, "name").unwrap_or_else(|| "unknown".into());
        let Some(close) = remainder[open_end + 1..].find("</action>") else {
            actions.push(name);
            break;
        };
        let action_end = open_end + 1 + close;
        let body = &remainder[open_end + 1..action_end];
        let command = tag_text(body, "command");
        actions.push(command.map_or(name.clone(), |command| format!("{name}: {command}")));
        remainder = &remainder[action_end + "</action>".len()..];
    }
    actions
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=");
    let start = tag.find(&marker)? + marker.len();
    let quote = tag[start..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = tag[start + quote.len_utf8()..].find(quote)?;
    Some(tag[start + quote.len_utf8()..start + quote.len_utf8() + end].to_owned())
}

fn tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)?;
    let value = block[start..start + end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_key(value: &str) -> Option<KeyCombo> {
    let mut parts = value.split('-').map(str::trim).collect::<Vec<_>>();
    let key = parts.pop()?.to_owned();
    if key.is_empty() || key.contains(['{', '}', '|']) || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let mut modifiers = Vec::new();
    for modifier in parts {
        modifiers.push(match modifier.to_ascii_uppercase().as_str() {
            "A" => "alt",
            "C" => "ctrl",
            "S" => "shift",
            "W" => "super",
            _ => return None,
        });
    }
    if modifiers.is_empty() {
        key.parse().ok()
    } else {
        format!("{}+{key}", modifiers.join("+")).parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openbox_keybind_actions_and_commands() {
        let bindings = parse_bindings(
            r#"<keybind key="W-C-t">
  <action name="Execute"><command>alacritty</command></action>
</keybind>
<keybind key="A-F2"><action name="ShowMenu" /></keybind>"#,
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+ctrl+t".parse().unwrap());
        assert_eq!(bindings[0].action, "Execute: alacritty");
        assert_eq!(bindings[1].combo, "alt+f2".parse().unwrap());
        assert_eq!(bindings[1].action, "ShowMenu");
    }

    #[test]
    fn rejects_dynamic_or_malformed_key_attributes() {
        assert!(parse_key("W-{a,b}").is_none());
        assert!(parse_key("W-C-").is_none());
        assert!(attribute("<keybind key='W-c'>", "key").is_some());
    }
}
