use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

/// Read-only MATE global keyboard-shortcut adapter.
pub struct Mate;

const SCHEMAS: &[&str] = &[
    "org.mate.Marco.global-keybindings",
    "org.mate.SettingsDaemon.plugins.media-keys",
];

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
                .any(|name| name.trim().eq_ignore_ascii_case("mate"))
        })
        || env::var_os("MATE_DESKTOP_SESSION_ID").is_some()
}

pub fn ipc_available() -> bool {
    SCHEMAS
        .iter()
        .any(|schema| run_gsettings_list(schema).is_ok())
}

/// Return MATE's live GSettings shortcut values as normalized entries.
pub fn binding_inventory() -> Result<Vec<(KeyCombo, String, Option<String>)>, String> {
    let (bindings, errors) = load_bindings();
    if bindings.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(bindings
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.command))
        .collect())
}

impl Mate {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let (bindings, errors) = load_bindings();
        if bindings.is_empty() && !errors.is_empty() {
            return LayerResult::new(
                "MATE",
                LayerId::Compositor,
                Outcome::Unavailable,
                "could not inspect MATE global shortcuts",
                errors,
            );
        }
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            let mut details = SCHEMAS
                .iter()
                .map(|schema| format!("source: gsettings list-recursively {schema}"))
                .collect::<Vec<_>>();
            details.extend(errors);
            return LayerResult::new(
                "MATE",
                LayerId::Compositor,
                Outcome::Pass,
                "no active MATE global shortcut found",
                details,
            );
        }

        let mut details = SCHEMAS
            .iter()
            .map(|schema| format!("source: gsettings list-recursively {schema}"))
            .collect::<Vec<_>>();
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
        details.extend(errors);
        LayerResult::new(
            "MATE",
            LayerId::Compositor,
            Outcome::Consumed,
            "MATE global shortcut consumes the key",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    command: Option<String>,
}

fn load_bindings() -> (Vec<Binding>, Vec<String>) {
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for schema in SCHEMAS {
        match run_gsettings_list(schema) {
            Ok(output) => bindings.extend(parse_bindings(schema, &output)),
            Err(error) => errors.push(error),
        }
    }
    (bindings, errors)
}

fn run_gsettings_list(schema: &str) -> Result<String, String> {
    let mut gsettings = Command::new("gsettings");
    gsettings.args(["list-recursively", schema]);
    let output = command::output(&mut gsettings).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("gsettings {schema} exited with {}", output.status)
        } else {
            format!("gsettings {schema}: {message}")
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gsettings {schema} returned invalid UTF-8: {error}"))
}

fn parse_bindings(schema: &str, content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(row_schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if row_schema != schema {
            continue;
        }
        for raw_binding in quoted_values(value) {
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
    fn parses_marco_and_media_key_rows() {
        let marco = parse_bindings(
            SCHEMAS[0],
            "org.mate.Marco.global-keybindings run-command-1 '<Alt>F2'\n",
        );
        assert_eq!(marco.len(), 1);
        assert_eq!(marco[0].combo, "alt+f2".parse().unwrap());
        let media = parse_bindings(
            SCHEMAS[1],
            "org.mate.SettingsDaemon.plugins.media-keys screensaver ['<Super>l']\n",
        );
        assert_eq!(media[0].combo, "super+l".parse().unwrap());
    }

    #[test]
    fn ignores_disabled_and_unknown_values() {
        assert!(quoted_values("[]").is_empty());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(parse_accelerator("<Alt>").is_none());
    }
}
