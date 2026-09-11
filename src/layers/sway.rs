use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command;
use crate::key::KeyCombo;
use crate::layers::LayerResult;

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

/// Return Sway bindings as inventory records for the global listing.
pub fn binding_inventory() -> Result<Vec<super::BindingRecord>, String> {
    let value =
        binding_inventory_json().map_err(|error| format!("{}{}", META.error_prefix, error))?;
    Ok(super::collect_ipc_json_bindings(&value, "Sway"))
}

fn binding_inventory_json() -> Result<serde_json::Value, String> {
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
    super::tiling::focused_pid_in_tree(&tree)
        .ok_or_else(|| "Sway focused tree node does not expose a valid PID".into())
}

const META: super::tiling::TilingMeta = super::tiling::TilingMeta {
    layer: "Sway",
    error_prefix: "Sway: ",
    unavailable_summary: "could not inspect effective Sway bindings",
    no_match_source: "swaymsg -t get_bindings -r",
    match_source: "source: swaymsg -t get_bindings -r",
    possible_note: "Sway binding may accept additional modifiers because exact matching is not enabled",
    show_release: true,
};

impl Sway {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let loaded =
            run_swaymsg().map(|bindings| bindings.iter().map(to_ipc_binding).collect::<Vec<_>>());
        super::tiling::inspect(&META, loaded, key)
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

fn to_ipc_binding(binding: &SwayBinding) -> super::tiling::IpcBinding {
    let mut keys = super::tiling::string_values(&binding.keysym);
    keys.extend(super::tiling::string_values(&binding.symbols));
    super::tiling::IpcBinding {
        command: binding.command.clone(),
        keys,
        input_code: binding.input_code,
        keycodes: binding.keycodes.clone(),
        mask: super::json_u32(&binding.event_state_mask),
        release: binding.release,
        exact: binding.exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_sway_keysym_and_symbol_fields() {
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
        let ipc = to_ipc_binding(&binding);
        assert_eq!(ipc.keys, vec!["Return", "KP_Enter"]);
        assert_eq!(ipc.mask, Some(64));
        assert_eq!(ipc.input_code, Some(36));
        assert_eq!(ipc.keycodes, vec![36]);
    }
}
