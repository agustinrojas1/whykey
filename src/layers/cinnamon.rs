use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only Cinnamon global keyboard-shortcut adapter.
pub struct Cinnamon;

const SCHEMA_PREFIX: &str = "org.cinnamon.desktop.keybindings";

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop.split(':').any(|name| {
                matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "cinnamon" | "x-cinnamon"
                )
            })
        })
        || env::var_os("CINNAMON_VERSION").is_some()
}

pub fn ipc_available() -> bool {
    run_gsettings_list().is_ok()
}

/// Return Cinnamon's live GSettings shortcut values as normalized entries.
pub fn binding_inventory() -> Result<Vec<(KeyCombo, String, Option<String>)>, String> {
    Ok(load_bindings()?
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.command))
        .collect())
}

impl Cinnamon {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult {
                    verbose_details: Vec::new(),
                    binding: None,
                    layer: "Cinnamon",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect Cinnamon global shortcuts".into(),
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
                layer: "Cinnamon",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no active Cinnamon global shortcut found".into(),
                details: vec![format!(
                    "source: gsettings list-recursively {SCHEMA_PREFIX}"
                )],
            };
        }

        let mut details = vec![format!(
            "source: gsettings list-recursively {SCHEMA_PREFIX}"
        )];
        for binding in matches {
            details.push(format!(
                "binding: {}{}",
                binding.action,
                binding
                    .command
                    .as_deref()
                    .map(|command| format!(" — command: {command}"))
                    .unwrap_or_default()
            ));
        }
        LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Cinnamon",
            id: LayerId::Compositor,
            outcome: Outcome::Consumed,
            summary: "Cinnamon global shortcut consumes the key".into(),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    command: Option<String>,
}

fn load_bindings() -> Result<Vec<Binding>, String> {
    let output = run_gsettings_list()?;
    Ok(parse_bindings(&output))
}

fn run_gsettings_list() -> Result<String, String> {
    let mut gsettings = Command::new("gsettings");
    gsettings.args(["list-recursively", SCHEMA_PREFIX]);
    let output = command::output(&mut gsettings).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("gsettings exited with {}", output.status)
        } else {
            format!("gsettings: {message}")
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gsettings returned invalid UTF-8: {error}"))
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if !schema.starts_with(SCHEMA_PREFIX) {
            continue;
        }
        let values = quoted_values(value);
        for raw_binding in values {
            let Some(combo) = parse_accelerator(&raw_binding) else {
                continue;
            };
            bindings.push(Binding {
                combo,
                action: format!("{schema} {name}"),
                command: None,
            });
        }
    }
    bindings
}

fn quoted_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.trim().chars() {
        if let Some(active_quote) = quote {
            if escaped {
                current.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                values.push(std::mem::take(&mut current));
                quote = None;
            } else {
                current.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        }
    }
    if values.is_empty() {
        let value = value.trim();
        if !value.is_empty() && !value.starts_with('@') && value != "[]" {
            values.push(value.trim_matches(['[', ']', ' ', '\n']).to_owned());
        }
    }
    values
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
    fn parses_cinnamon_gsettings_rows() {
        let bindings = parse_bindings(
            "org.cinnamon.desktop.keybindings.wm switch-to-workspace-1 ['<Super>1']\norg.cinnamon.desktop.keybindings.media-keys terminal ['<Primary><Alt>t']\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+1".parse().unwrap());
        assert!(bindings[0].action.contains("switch-to-workspace-1"));
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    #[test]
    fn ignores_empty_and_unknown_accelerators() {
        assert!(parse_accelerator("<Super>").is_none());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(quoted_values("[]").is_empty());
    }
}
