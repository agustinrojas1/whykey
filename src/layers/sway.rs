use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, Propagation};

/// Read-only Sway compositor adapter.
pub struct Sway;

#[derive(Debug, Deserialize, Serialize)]
struct SwayBinding {
    #[serde(default)]
    command: String,
    #[serde(default)]
    keysym: serde_json::Value,
    #[serde(default)]
    symbols: serde_json::Value,
    #[serde(default)]
    input_code: Option<u32>,
    #[serde(default)]
    keycodes: Vec<u32>,
    #[serde(default)]
    event_state_mask: serde_json::Value,
    #[serde(default)]
    release: bool,
    #[serde(default)]
    exact: bool,
}

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    env::var_os("SWAYSOCK").is_some_and(|value| !value.is_empty())
        || env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .into_iter()
            .chain(env::var("XDG_SESSION_DESKTOP").ok())
            .any(|desktop| {
                desktop
                    .split(':')
                    .any(|name| name.eq_ignore_ascii_case("sway"))
            })
}

pub fn ipc_available() -> bool {
    run_sway_version().is_ok()
}

/// Return Sway's effective binding payload for the global inventory command.
pub fn binding_inventory_json() -> Result<serde_json::Value, String> {
    let output = run_swaymsg()?;
    serde_json::to_value(output)
        .map_err(|error| format!("could not serialize Sway bindings: {error}"))
}

pub fn focused_pid() -> Result<u32, String> {
    let mut command = Command::new("swaymsg");
    command.args(["-t", "get_tree", "-r"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("swaymsg exited with {}", output.status)
        } else {
            format!("swaymsg: {message}")
        });
    }
    let tree: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("swaymsg returned invalid tree data: {error}"))?;
    focused_pid_in_tree(&tree)
        .ok_or_else(|| "Sway focused tree node does not expose a valid PID".into())
}

fn focused_pid_in_tree(value: &serde_json::Value) -> Option<u32> {
    if value.get("focused").and_then(serde_json::Value::as_bool) == Some(true) {
        if let Some(pid) = value
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            return Some(pid);
        }
    }
    value
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find_map(focused_pid_in_tree)
        .or_else(|| {
            value
                .get("floating_nodes")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .find_map(focused_pid_in_tree)
        })
}

impl Sway {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match run_swaymsg() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult {
                    verbose_details: Vec::new(),
                    binding: None,
                    layer: "Sway",
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect effective Sway bindings".into(),
                    details: vec![error],
                };
            }
        };

        let mut exact_matches = Vec::new();
        let mut possible_matches = Vec::new();
        for binding in &bindings {
            if binding.release {
                continue;
            }
            let key_matches = binding_matches_key(binding, key);
            if !key_matches {
                continue;
            }
            match binding_modifier_mask(binding) {
                Some(mask) if mask == key.modmask() => {
                    exact_matches.push(binding);
                }
                Some(_) if !binding.exact => {
                    possible_matches.push(binding);
                }
                None => {
                    possible_matches.push(binding);
                }
                Some(_) => {}
            }
        }

        if exact_matches.is_empty() && possible_matches.is_empty() {
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Sway",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no active binding found".into(),
                details: vec!["swaymsg -t get_bindings -r".into()],
            };
        }

        let mut details = vec!["source: swaymsg -t get_bindings -r".into()];
        for binding in exact_matches.iter().chain(possible_matches.iter()) {
            details.push(format!(
                "binding: {}{}",
                binding.command,
                if binding.release { " (release)" } else { "" }
            ));
        }
        if !possible_matches.is_empty() {
            details.push("Sway binding may accept additional modifiers because exact matching is not enabled".into());
        }

        let outcome = if exact_matches.is_empty() {
            if possible_matches.iter().all(|binding| {
                command_propagation(&binding.command) == Some(Propagation::Continues)
            }) {
                Outcome::UncertainContinues
            } else {
                Outcome::Unknown
            }
        } else {
            match exact_matches
                .iter()
                .find_map(|binding| command_propagation(&binding.command))
            {
                Some(Propagation::Continues) => Outcome::HandledAndPassed,
                Some(Propagation::Stops) => Outcome::Consumed,
                Some(Propagation::Redirected) => Outcome::Redirected,
                Some(Propagation::Indeterminate) | None => Outcome::HandledUncertain,
            }
        };
        LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Sway",
            id: LayerId::Compositor,
            outcome,
            summary: if exact_matches.is_empty() {
                "possible binding found; modifier matching is conditional".into()
            } else {
                "active binding found".into()
            },
            details,
        }
    }
}

fn run_swaymsg() -> Result<Vec<SwayBinding>, String> {
    let mut command = Command::new("swaymsg");
    command.args(["-t", "get_bindings", "-r"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("swaymsg exited with {}", output.status)
        } else {
            format!("swaymsg: {message}")
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("swaymsg returned invalid binding data: {error}"))
}

fn run_sway_version() -> Result<(), String> {
    let mut command = Command::new("swaymsg");
    command.args(["-t", "get_version"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            format!("swaymsg exited with {}", output.status)
        } else {
            format!("swaymsg: {message}")
        })
    }
}

fn binding_keys(binding: &SwayBinding) -> Vec<String> {
    let mut keys = string_values(&binding.keysym);
    keys.extend(string_values(&binding.symbols));
    keys
}

fn string_values(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) => vec![value.clone()],
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn binding_modifier_mask(binding: &SwayBinding) -> Option<u32> {
    json_u32(&binding.event_state_mask)
}

fn binding_matches_key(binding: &SwayBinding, key: &KeyCombo) -> bool {
    if let Some(query_code) = key
        .key()
        .strip_prefix("CODE:")
        .and_then(|value| value.parse::<u32>().ok())
    {
        return binding.input_code == Some(query_code) || binding.keycodes.contains(&query_code);
    }
    binding_keys(binding).iter().any(|candidate| {
        candidate
            .parse::<KeyCombo>()
            .map(|combo| combo.key().eq_ignore_ascii_case(key.key()))
            .unwrap_or_else(|_| candidate.eq_ignore_ascii_case(key.key()))
    })
}

fn json_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn command_propagation(command: &str) -> Option<Propagation> {
    let dispatcher = command.split_whitespace().next()?.to_ascii_lowercase();
    Some(match dispatcher.as_str() {
        "nop" => Propagation::Continues,
        "pass" => Propagation::Redirected,
        _ => Propagation::Stops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sway_keys_from_string_and_array_fields() {
        let binding: SwayBinding = serde_json::from_str(
            r#"{
                "command": "exec test",
                "keysym": "Return",
                "symbols": ["KP_Enter"],
                "input_code": 36,
                "keycodes": [36],
                "event_state_mask": 64
            }"#,
        )
        .unwrap();
        assert_eq!(binding_keys(&binding), vec!["Return", "KP_Enter"]);
        assert_eq!(binding_modifier_mask(&binding), Some(64));
        let code: KeyCombo = "code:36".parse().unwrap();
        assert!(binding_matches_key(&binding, &code));
    }

    #[test]
    fn classifies_sway_dispatchers() {
        assert_eq!(command_propagation("nop"), Some(Propagation::Continues));
        assert_eq!(command_propagation("pass"), Some(Propagation::Redirected));
        assert_eq!(command_propagation("exec foo"), Some(Propagation::Stops));
        assert_eq!(
            command_propagation(""),
            None,
            "an empty dispatcher leaves propagation conditional instead of guessing"
        );
    }

    #[test]
    fn finds_a_focused_pid_in_nested_sway_nodes() {
        let tree = serde_json::json!({
            "nodes": [{
                "focused": false,
                "nodes": [{"focused": true, "pid": 4242}]
            }]
        });
        assert_eq!(focused_pid_in_tree(&tree), Some(4242));
    }
}
