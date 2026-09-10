use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";

/// Read-only GNOME global shortcut adapter.
pub struct Gnome;

#[derive(Debug, Clone, PartialEq, Eq)]
struct GnomeBinding {
    combo: KeyCombo,
    name: String,
    command: Option<String>,
    source: String,
}

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
                .any(|name| name.trim().eq_ignore_ascii_case("gnome"))
        })
        || env::var_os("GNOME_DESKTOP_SESSION_ID").is_some()
}

pub fn ipc_available() -> bool {
    run_gsettings_list().is_ok()
}

/// Return GNOME bindings as normalized values for the global inventory.
pub fn binding_inventory() -> Result<Vec<(KeyCombo, String, Option<String>)>, String> {
    load_bindings().map(|bindings| {
        bindings
            .into_iter()
            .map(|binding| (binding.combo, binding.name, binding.command))
            .collect()
    })
}

impl Gnome {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match load_bindings() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult {
                    verbose_details: Vec::new(),
                    binding: None,
                    layer: "GNOME",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect GNOME global shortcuts".into(),
                    details: vec![error],
                };
            }
        };

        let matches = bindings
            .iter()
            .filter(|binding| &binding.combo == key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "GNOME",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no active GNOME global shortcut found".into(),
                details: vec![format!("source: gsettings {MEDIA_KEYS_SCHEMA}")],
            };
        }

        let mut details = vec![format!("source: gsettings {MEDIA_KEYS_SCHEMA}")];
        for binding in matches {
            details.push(format!(
                "binding: {}{}{}",
                binding.name,
                binding
                    .command
                    .as_deref()
                    .map(|command| format!(" — command: {command}"))
                    .unwrap_or_default(),
                if binding.source.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", binding.source)
                }
            ));
        }
        LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "GNOME",
            id: LayerId::Compositor,
            outcome: Outcome::Consumed,
            summary: "GNOME global shortcut consumes the key".into(),
            details,
        }
    }
}

fn load_bindings() -> Result<Vec<GnomeBinding>, String> {
    let output = run_gsettings_list()?;
    let mut bindings = Vec::new();
    let mut custom_paths = Vec::new();
    for line in output.lines() {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if schema != MEDIA_KEYS_SCHEMA {
            continue;
        }
        if name == "custom-keybindings" {
            custom_paths.extend(
                quoted_values(value)
                    .into_iter()
                    .filter(|path| path.starts_with('/')),
            );
            continue;
        }
        for raw_binding in quoted_values(value) {
            if let Some(combo) = parse_gnome_binding(&raw_binding) {
                bindings.push(GnomeBinding {
                    combo,
                    name: name.to_owned(),
                    command: None,
                    source: "media-keys".into(),
                });
            }
        }
    }

    for path in custom_paths {
        let schema = format!("{MEDIA_KEYS_SCHEMA}.custom-keybinding:{path}");
        let Some(raw_binding) = run_gsettings_get(&schema, "binding")? else {
            continue;
        };
        let Some(combo) = quoted_values(&raw_binding)
            .into_iter()
            .find_map(|value| parse_gnome_binding(&value))
        else {
            continue;
        };
        let name = run_gsettings_get(&schema, "name")?
            .and_then(|value| quoted_values(&value).into_iter().next())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "custom shortcut".into());
        let command = run_gsettings_get(&schema, "command")?
            .and_then(|value| quoted_values(&value).into_iter().next())
            .filter(|value| !value.is_empty());
        bindings.push(GnomeBinding {
            combo,
            name,
            command,
            source: "custom-keybinding".into(),
        });
    }
    Ok(bindings)
}

fn run_gsettings_list() -> Result<String, String> {
    let mut command = Command::new("gsettings");
    command.args(["list-recursively", MEDIA_KEYS_SCHEMA]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(command_error("gsettings", output.status, &output.stderr));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gsettings returned invalid UTF-8: {error}"))
}

fn run_gsettings_get(schema: &str, key: &str) -> Result<Option<String>, String> {
    let mut command = Command::new("gsettings");
    command.args(["get", schema, key]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    String::from_utf8(output.stdout)
        .map(Some)
        .map_err(|error| format!("gsettings returned invalid UTF-8: {error}"))
}

fn command_error(program: &str, status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    if message.is_empty() {
        format!("{program} exited with {status}")
    } else {
        format!("{program}: {message}")
    }
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

fn parse_gnome_binding(value: &str) -> Option<KeyCombo> {
    let mut remainder = value.trim();
    let mut modifiers = Vec::new();
    while remainder.starts_with('<') {
        let end = remainder.find('>')?;
        let modifier = &remainder[1..end];
        modifiers.push(match modifier.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "primary" => "ctrl",
            "alt" | "mod1" => "alt",
            "shift" => "shift",
            "super" | "win" | "mod4" => "super",
            "hyper" | "mod3" => "hyper",
            "mod2" | "num" => "mod2",
            "mod5" => "mod5",
            _ => return None,
        });
        remainder = remainder[end + 1..].trim();
    }
    if remainder.is_empty() {
        return None;
    }
    let combo = if modifiers.is_empty() {
        remainder.to_owned()
    } else {
        format!("{}+{remainder}", modifiers.join("+"))
    };
    combo.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnome_modifier_notation() {
        assert_eq!(
            parse_gnome_binding("<Primary><Alt>Left").unwrap(),
            "ctrl+alt+left".parse().unwrap()
        );
        assert_eq!(
            parse_gnome_binding("<Super>Return").unwrap(),
            "super+return".parse().unwrap()
        );
        assert_eq!(
            parse_gnome_binding("<BogusMod>Left"),
            None,
            "an unknown modifier never becomes a false positive binding"
        );
    }
    #[test]
    fn extracts_gsettings_strings() {
        assert_eq!(
            quoted_values("['<Super>l', '<Control><Alt>t']"),
            vec!["<Super>l", "<Control><Alt>t"]
        );
        assert!(quoted_values("@as []").is_empty());
    }
}
