use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command;
use crate::key::KeyCombo;
use crate::layers::LayerResult;

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
    let value =
        binding_inventory_json().map_err(|error| format!("{}{}", META.error_prefix, error))?;
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
    super::tiling::focused_pid_in_tree(&tree)
        .ok_or_else(|| "i3 focused tree node does not expose a valid PID".into())
}

const META: super::tiling::TilingMeta = super::tiling::TilingMeta {
    layer: "i3",
    error_prefix: "i3: ",
    unavailable_summary: "could not inspect effective i3 bindings",
    no_match_source: "source: i3-msg -t get_bindings",
    match_source: "source: i3-msg -t get_bindings",
    possible_note: "i3 binding may accept additional modifiers because exact matching is not enabled",
    show_release: false,
};

impl I3 {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let loaded =
            run_i3msg().map(|bindings| bindings.iter().map(to_ipc_binding).collect::<Vec<_>>());
        super::tiling::inspect(&META, loaded, key)
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

fn to_ipc_binding(binding: &I3Binding) -> super::tiling::IpcBinding {
    super::tiling::IpcBinding {
        command: binding.command.clone(),
        keys: super::tiling::string_values(&binding.symbol),
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
    fn converts_i3_symbol_field() {
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
        let ipc = to_ipc_binding(&binding);
        assert_eq!(ipc.keys, vec!["c"]);
        assert_eq!(ipc.mask, Some(4));
        assert_eq!(ipc.input_code, Some(46));
        assert_eq!(ipc.keycodes, vec![46]);
    }
}
