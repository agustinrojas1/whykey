//! Pure Hyprland binding analysis boundary.
//!
//! IPC, instance selection, and configuration acquisition stay in
//! [`super::hyprland`]. This module is the intentionally side-effect-free
//! hand-off used by that adapter once its snapshots have been collected.

use crate::key::KeyCombo;
use crate::layers::{LayerResult, PhysicalInput};

pub(crate) fn analyze(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[super::hyprland::LuaBindHint],
    config_ignore_mods: bool,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    super::hyprland::inspect_json_with_keycode(
        key,
        bindings_json,
        active_submap,
        keycodes,
        lua_hints,
        config_ignore_mods,
        physical_input,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzes_a_snapshot_without_ipc_or_config_access() {
        let key: KeyCombo = "super+return".parse().unwrap();
        let result = analyze(
            &key,
            r#"[{"modmask":64,"key":"Return","dispatcher":"spawn","arg":"foot","submap":""}]"#,
            "default",
            &[],
            &[],
            false,
            None,
        )
        .unwrap();
        assert!(result.summary.contains("active binding found"));
    }
}
