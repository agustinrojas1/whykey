use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, Propagation};

/// Read-only i3 compositor adapter.
pub struct I3;

#[derive(Debug, Deserialize, Serialize)]
struct I3Binding {
    #[serde(default)]
    command: String,
    #[serde(default)]
    symbol: serde_json::Value,
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
    env::var_os("I3SOCK").is_some_and(|value| !value.is_empty())
        || env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .into_iter()
            .chain(env::var("XDG_SESSION_DESKTOP").ok())
            .any(|desktop| {
                desktop
                    .split(':')
                    .any(|name| name.eq_ignore_ascii_case("i3"))
            })
}

pub fn ipc_available() -> bool {
    run_i3_version().is_ok()
}

/// Return i3 bindings as inventory records for the global listing.
pub fn binding_inventory() -> Result<Vec<super::BindingRecord>, String> {
    let value = binding_inventory_json().map_err(|error| format!("i3: {error}"))?;
    Ok(super::collect_ipc_json_bindings(&value, "i3"))
}

fn binding_inventory_json() -> Result<serde_json::Value, String> {
    let output = run_i3msg()?;
    serde_json::to_value(output)
        .map_err(|error| format!("could not serialize i3 bindings: {error}"))
}

pub fn focused_pid() -> Result<u32, String> {
    let mut command = Command::new("i3-msg");
    command.args(["-t", "get_tree"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("i3-msg exited with {}", output.status)
        } else {
            format!("i3-msg: {message}")
        });
    }
    let tree: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("i3-msg returned invalid tree data: {error}"))?;
    focused_pid_in_tree(&tree)
        .ok_or_else(|| "i3 focused tree node does not expose a valid PID".into())
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

impl I3 {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match run_i3msg() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult::new(
                    "i3",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect effective i3 bindings",
                    vec![error],
                );
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
            match json_u32(&binding.event_state_mask) {
                Some(mask) if mask == key.modmask() => exact_matches.push(binding),
                Some(_) if !binding.exact => possible_matches.push(binding),
                None => possible_matches.push(binding),
                Some(_) => {}
            }
        }

        if exact_matches.is_empty() && possible_matches.is_empty() {
            return LayerResult::new(
                "i3",
                LayerId::Compositor,
                Outcome::Pass,
                "no active binding found",
                vec!["source: i3-msg -t get_bindings".into()],
            );
        }

        let mut details = vec!["source: i3-msg -t get_bindings".into()];
        for binding in exact_matches.iter().chain(possible_matches.iter()) {
            details.push(format!("binding: {}", binding.command));
        }
        if !possible_matches.is_empty() {
            details.push(
                "i3 binding may accept additional modifiers because exact matching is not enabled"
                    .into(),
            );
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
        LayerResult::new(
            "i3",
            LayerId::Compositor,
            outcome,
            if exact_matches.is_empty() {
                "possible binding found; modifier matching is conditional".to_owned()
            } else {
                "active binding found".to_owned()
            },
            details,
        )
    }
}

fn run_i3msg() -> Result<Vec<I3Binding>, String> {
    let mut command = Command::new("i3-msg");
    command.args(["-t", "get_bindings"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("i3-msg exited with {}", output.status)
        } else {
            format!("i3-msg: {message}")
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("i3-msg returned invalid binding data: {error}"))
}

fn run_i3_version() -> Result<(), String> {
    let mut command = Command::new("i3-msg");
    command.args(["-t", "get_version"]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            format!("i3-msg exited with {}", output.status)
        } else {
            format!("i3-msg: {message}")
        })
    }
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

fn binding_matches_key(binding: &I3Binding, key: &KeyCombo) -> bool {
    if let Some(query_code) = key
        .key()
        .strip_prefix("CODE:")
        .and_then(|value| value.parse::<u32>().ok())
    {
        return binding.input_code == Some(query_code) || binding.keycodes.contains(&query_code);
    }
    string_values(&binding.symbol).iter().any(|candidate| {
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
    fn reads_i3_symbol_and_modifier_mask() {
        let binding: I3Binding = serde_json::from_str(
            r#"{
                "command": "exec copy",
                "symbol": "c",
                "input_code": 46,
                "keycodes": [46],
                "event_state_mask": 4,
                "exact": true
            }"#,
        )
        .unwrap();
        assert_eq!(string_values(&binding.symbol), vec!["c"]);
        assert_eq!(binding.event_state_mask.as_u64(), Some(4));
        let code: KeyCombo = "code:46".parse().unwrap();
        assert!(binding_matches_key(&binding, &code));
    }

    #[test]
    fn classifies_i3_dispatchers() {
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
    fn finds_a_focused_pid_in_floating_i3_nodes() {
        let tree = serde_json::json!({
            "nodes": [],
            "floating_nodes": [{"focused": true, "pid": 31337}]
        });
        assert_eq!(focused_pid_in_tree(&tree), Some(31337));
    }
}
