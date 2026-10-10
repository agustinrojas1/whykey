use std::env;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::LayerResult;

use super::tiling::Family;

/// Read-only i3 compositor adapter.
pub struct I3;

/// i3 withholds the press of a `--release` binding from the focused client:
/// `grab_all_keys` (i3 src/bindings.c) installs the same passive
/// `xcb_grab_key(conn, 0, root, mods, keycode, XCB_GRAB_MODE_ASYNC,
/// XCB_GRAB_MODE_ASYNC)` for every keyboard binding regardless of
/// `bind->release`, so the X server delivers both the KeyPress and the
/// KeyRelease to i3 alone, and i3 never replays them (`handle_key_press` in
/// src/key_press.c only runs the matched binding). Lab-verified with i3 4.25
/// on Xvfb and xdotool: a focused X client logged only `Super_L` for a
/// `bindsym --release Mod4+r` while the command fired.
pub(super) const FAMILY: Family = Family {
    layer: "i3",
    source: "i3",
    msg: "i3-msg",
    msg_args: &[],
    config_raw_args: &["-r"],
    release_press_forwarded: Some(false),
    keysym_in_binding_state: true,
    config_candidates,
};

/// i3's documented config search order. i3 ≥ 4.20 reports the loaded path in
/// `included_configs`, so this only matters for older releases.
fn config_candidates() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let mut candidates = Vec::new();
    if let Some(config_home) = &config_home {
        candidates.push(config_home.join("i3/config"));
    }
    candidates.push(PathBuf::from("/etc/xdg/i3/config"));
    if let Some(home) = &home {
        candidates.push(home.join(".i3/config"));
    }
    candidates.push(PathBuf::from("/etc/i3/config"));
    candidates
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
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
    super::tiling::version_available(&FAMILY)
}

/// Return i3 bindings, parsed from the running config, for the global
/// listing.
pub fn binding_inventory() -> Result<Vec<super::BindingRecord>, String> {
    super::tiling::inventory(&FAMILY).map_err(|error| format!("i3: {error}"))
}

pub fn focused_pid() -> Result<u32, String> {
    super::tiling::focused_pid(&FAMILY)
}

impl I3 {
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
    fn session_fixture_is_a_get_config_reply_with_included_configs() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/sessions/i3.json")).unwrap();
        assert_eq!(fixture["id"], "i3");
        let (main, provided) = config_reply(&fixture["get_config"]).unwrap();
        let loaded = LoadedConfig {
            parsed: parse_config(&main, provided),
            active_mode: fixture["get_binding_state"]["name"]
                .as_str()
                .map(str::to_owned),
        };
        assert!(loaded.parsed.gaps.is_empty(), "{:?}", loaded.parsed.gaps);
        let result = inspect(&FAMILY, Ok(loaded), &"super+return".parse().unwrap());
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("exec i3-sensible-terminal"))
        );
        assert_eq!(fixture["focused_tree"]["pid"], 4343);
        assert!(fixture["ipc_failure"].as_str().unwrap().contains("i3-msg"));
    }

    #[test]
    fn searches_xdg_config_before_legacy_locations() {
        let candidates = config_candidates();
        assert_eq!(
            candidates.last().map(PathBuf::as_path),
            Some(std::path::Path::new("/etc/i3/config"))
        );
        assert!(candidates.contains(&PathBuf::from("/etc/xdg/i3/config")));
    }
}
