use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only Xfce keyboard-shortcut adapter.
pub struct Xfce;

pub type BindingInventoryEntry = (KeyCombo, String, String);

const CHANNEL: &str = "xfce4-keyboard-shortcuts";

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
                .any(|name| matches!(name.trim().to_ascii_lowercase().as_str(), "xfce" | "xfce4"))
        })
        || env::var_os("XFCE_DESKTOP_SESSION").is_some()
}

pub fn ipc_available() -> bool {
    run_query().is_ok()
}

/// Return Xfce's current channel values as normalized inventory entries.
pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    Ok(load_bindings()?
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.context))
        .collect())
}

impl Xfce {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult {
                    binding: None,
                    layer: "Xfce",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect Xfce keyboard shortcuts".into(),
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
                binding: None,
                layer: "Xfce",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no active Xfce keyboard shortcut found".into(),
                details: vec![format!("source: xfconf-query -c {CHANNEL} -l -v")],
            };
        }

        let mut details = vec![format!("source: xfconf-query -c {CHANNEL} -l -v")];
        for binding in matches {
            details.push(format!("binding: {} ({})", binding.action, binding.context));
        }
        LayerResult {
            binding: None,
            layer: "Xfce",
            id: LayerId::Compositor,
            outcome: Outcome::Consumed,
            summary: "Xfce global shortcut consumes the key".into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    context: String,
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let output = run_query()?;
    Ok(parse_bindings(&output))
}

fn run_query() -> Result<String, String> {
    let mut query = Command::new("xfconf-query");
    query.args(["-c", CHANNEL, "-l", "-v"]);
    let output = command::output(&mut query).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("xfconf-query exited with {}", output.status)
        } else {
            format!("xfconf-query: {message}")
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("xfconf-query returned invalid UTF-8: {error}"))
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (property, action) = line
            .split_once(':')
            .or_else(|| line.split_once(char::is_whitespace))
            .map(|(property, action)| (property.trim(), action.trim()))
            .unwrap_or((line, ""));
        if action.is_empty()
            || !(property.starts_with("/commands/custom/")
                || property.starts_with("/commands/default/"))
        {
            continue;
        }
        let Some(raw_accelerator) = property.rsplit('/').next() else {
            continue;
        };
        let Some(combo) = parse_accelerator(raw_accelerator) else {
            continue;
        };
        let context = property
            .strip_prefix("/commands/")
            .and_then(|value| value.split('/').next())
            .unwrap_or("unknown")
            .to_owned();
        bindings.push(Binding {
            combo,
            action: action.to_owned(),
            context,
        });
    }
    bindings
}

fn parse_accelerator(value: &str) -> Option<KeyCombo> {
    let mut remainder = value.trim();
    let mut modifiers = Vec::new();
    while remainder.starts_with('<') {
        let end = remainder.find('>')?;
        let modifier = &remainder[1..end];
        modifiers.push(match modifier.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "primary" | "ctl" => "ctrl",
            "alt" | "mod1" => "alt",
            "shift" => "shift",
            "super" | "meta" | "win" | "mod4" => "super",
            "hyper" | "mod3" => "mod3",
            "mod2" | "num" | "numlock" => "mod2",
            "mod5" => "mod5",
            _ => return None,
        });
        remainder = remainder[end + 1..].trim();
    }
    if !modifiers.is_empty() {
        if remainder.is_empty() {
            return None;
        }
        return format!("{}+{remainder}", modifiers.join("+")).parse().ok();
    }
    remainder.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xfce_channel_values() {
        let bindings = parse_bindings(
            "/commands/custom/<Super>r: xfrun4\n/commands/default/<Alt>F2 xfce4-appfinder\n/other/foo: ignored\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+r".parse().unwrap());
        assert_eq!(bindings[0].context, "custom");
        assert_eq!(bindings[1].combo, "alt+f2".parse().unwrap());
    }

    #[test]
    fn rejects_dynamic_or_modifier_only_properties() {
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(parse_accelerator("<Super>").is_none());
        assert!(parse_accelerator("<Primary><Alt>t").is_some());
    }
}
