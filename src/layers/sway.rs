use std::env;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::LayerResult;

use super::tiling::Family;

/// Read-only Sway compositor adapter.
pub struct Sway;

/// Sway delivers both the press and the release of a `--release` binding to
/// the focused client and runs the command on release (`handle_key_event` in
/// sway/input/keyboard.c only marks the event handled when the held binding
/// runs on release). Lab-verified with sway 1.11 headless, wtype and a
/// focused wev window: wev logged `p` pressed and released while
/// `bindsym --release Mod4+p` fired.
const FAMILY: Family = Family {
    layer: "Sway",
    source: "Sway",
    msg: "swaymsg",
    msg_args: &["-r"],
    config_raw_args: &[],
    release_press_forwarded: Some(true),
    config_candidates,
};

/// The search order of sway's `get_config_path` (sway/main.c), used to find
/// which file `GET_CONFIG` returned so relative includes resolve.
fn config_candidates() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let mut candidates = Vec::new();
    if let Some(home) = &home {
        candidates.push(home.join(".sway/config"));
    }
    if let Some(config_home) = &config_home {
        candidates.push(config_home.join("sway/config"));
    }
    if let Some(home) = &home {
        candidates.push(home.join(".i3/config"));
    }
    if let Some(config_home) = &config_home {
        candidates.push(config_home.join("i3/config"));
    }
    candidates.push(PathBuf::from("/etc/sway/config"));
    candidates.push(PathBuf::from("/etc/i3/config"));
    candidates
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
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
    super::tiling::version_available(&FAMILY)
}

/// Return Sway bindings, parsed from the running config, for the global
/// listing.
pub fn binding_inventory() -> Result<Vec<super::BindingRecord>, String> {
    super::tiling::inventory(&FAMILY).map_err(|error| format!("Sway: {error}"))
}

pub fn focused_pid() -> Result<u32, String> {
    super::tiling::focused_pid(&FAMILY)
}

impl Sway {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        super::tiling::inspect(&FAMILY, super::tiling::load(&FAMILY), key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::Outcome;
    use crate::layers::tiling::{LoadedConfig, config_reply, inspect, parse_config};

    #[test]
    fn session_fixture_is_a_get_config_reply_with_a_consuming_binding() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/sessions/sway.json")).unwrap();
        assert_eq!(fixture["id"], "sway");
        let (main, provided) = config_reply(&fixture["get_config"]).unwrap();
        let loaded = LoadedConfig {
            parsed: parse_config(&main, provided),
            active_mode: fixture["get_binding_state"]["name"]
                .as_str()
                .map(str::to_owned),
        };
        let result = inspect(&FAMILY, Ok(loaded), &"super+return".parse().unwrap());
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.details.iter().any(|line| line.contains("exec foot")));
        assert_eq!(fixture["focused_tree"]["pid"], 4242);
        assert!(fixture["ipc_failure"].as_str().unwrap().contains("swaymsg"));
    }

    #[test]
    fn searches_sway_config_locations_in_sway_order() {
        let candidates = config_candidates();
        assert_eq!(
            candidates.last().map(PathBuf::as_path),
            Some(std::path::Path::new("/etc/i3/config"))
        );
        assert!(candidates.contains(&PathBuf::from("/etc/sway/config")));
    }
}
