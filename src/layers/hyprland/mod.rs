use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, LayerId, LayerResult, LayerStatus, Outcome,
    PhysicalInput, Propagation, SourceLocation,
};
use crate::xkb;

pub struct Hyprland;

#[derive(Debug, Clone)]
pub(crate) struct Probe {
    binds: Result<String, String>,
    submap: Result<String, String>,
    devices: Option<String>,
}

impl Probe {
    pub(crate) fn ipc_available(&self) -> bool {
        self.binds.is_ok() && self.submap.is_ok()
    }
}

thread_local! {
    static INSTANCE_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run an inspection with an explicitly selected Hyprland instance. The
/// override is thread-local and restored immediately, so a CLI query cannot
/// mutate the user's session environment or affect later work in the process.
pub fn with_instance_override<T>(instance: &str, operation: impl FnOnce() -> T) -> T {
    let previous = INSTANCE_OVERRIDE.with(|value| value.replace(Some(instance.to_owned())));
    let result = operation();
    INSTANCE_OVERRIDE.with(|value| value.replace(previous));
    result
}

fn instance_override() -> Option<String> {
    INSTANCE_OVERRIDE.with(|value| value.borrow().clone())
}

#[derive(Debug, Deserialize)]
struct HyprlandInstance {
    instance: String,
    #[serde(default)]
    wl_socket: String,
}

#[derive(Debug, Deserialize)]
struct Binding {
    modmask: u32,
    key: String,
    #[serde(default)]
    keycode: crate::xkb::XkbKeycode,
    #[serde(default)]
    catch_all: bool,
    #[serde(default)]
    non_consuming: bool,
    #[serde(default)]
    auto_consuming: bool,
    #[serde(default)]
    release: bool,
    #[serde(default, rename = "longPress")]
    long_press: bool,
    #[serde(default)]
    submap: String,
    #[serde(default, deserialize_with = "deserialize_boolish")]
    submap_universal: bool,
    #[serde(default)]
    ignore_mods: serde_json::Value,
    #[serde(default)]
    locked: serde_json::Value,
    #[serde(default)]
    repeat: serde_json::Value,
    #[serde(default)]
    transparent: serde_json::Value,
    #[serde(default)]
    dont_inhibit: serde_json::Value,
    #[serde(default)]
    allow_input_capture: serde_json::Value,
    /// Older Hyprland versions expose this as a string; newer per-device
    /// bindings may expose the structured `{ inclusive, list }` value. Keep
    /// it as JSON so either shape remains inspectable instead of invalidating
    /// the whole `hyprctl binds -j` response.
    #[serde(default)]
    device: serde_json::Value,
    #[serde(default)]
    description: String,
    dispatcher: String,
    #[serde(default)]
    arg: String,
}

impl Hyprland {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        self.inspect_with_input(key, None)
    }

    pub fn inspect_with_input(
        &self,
        key: &KeyCombo,
        physical_input: Option<&PhysicalInput>,
    ) -> LayerResult {
        self.inspect_with_probe(key, physical_input, None)
    }

    pub(crate) fn inspect_with_probe(
        &self,
        key: &KeyCombo,
        physical_input: Option<&PhysicalInput>,
        probe: Option<&Probe>,
    ) -> LayerResult {
        if remote_session_without_compositor() {
            return LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Pass,
                "not applicable in this remote session",
                vec![
                    "SSH session detected without a local Hyprland IPC signature".into(),
                    "the remote terminal/session layers are inspected instead".into(),
                ],
            );
        }
        let result = match probe {
            Some(probe) => inspect_system_with_probe(key, physical_input, probe),
            None => inspect_system(key, physical_input),
        };
        match result {
            Ok(result) => result,
            Err(message) => ipc_uncertain_result(message),
        }
    }
}

fn ipc_uncertain_result(message: String) -> LayerResult {
    let mut details = vec![message];
    if let Some(config) = hyprland_config_paths()
        .into_iter()
        .find(|path| path.is_file())
    {
        details.push(format!(
            "static Hyprland configuration found at {}; active IPC state could not be verified",
            config.display()
        ));
    }
    details.push(
        "the active submap, runtime overrides, and input-inhibitor state remain unknown".into(),
    );
    LayerResult::new(
        "Hyprland",
        LayerId::Compositor,
        Outcome::UncertainContinues,
        "Hyprland IPC is unavailable; effective binding state is unknown",
        details,
    )
}

pub(crate) fn remote_session_without_compositor() -> bool {
    remote_session_without_compositor_state(
        env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
        instance_override().is_some() || has_hyprland_signature(),
    )
}

fn remote_session_without_compositor_state(ssh: bool, compositor_signature: bool) -> bool {
    ssh && !compositor_signature
}

include!("ipc.rs");
include!("config.rs");
include!("keyboard.rs");
include!("matching.rs");
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_ssh_without_a_compositor_signature_as_not_applicable() {
        assert!(remote_session_without_compositor_state(true, false));
        assert!(!remote_session_without_compositor_state(true, true));
        assert!(!remote_session_without_compositor_state(false, false));
    }

    #[test]
    fn keeps_ipc_failure_conditional_and_allows_downstream_inspection() {
        let result = ipc_uncertain_result("hyprctl: stale socket".into());

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(result.summary.contains("IPC is unavailable"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("stale socket"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("active submap"))
        );
    }

    #[test]
    fn selects_hyprland_instance_for_current_wayland_socket() {
        let instances: Vec<HyprlandInstance> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hyprland/instances.json"
        ))
        .unwrap();
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-1")),
            Some("second".into())
        );
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-9")),
            None
        );
    }

    #[test]
    fn selects_the_only_hyprland_instance_without_wayland_identity() {
        let instances = vec![HyprlandInstance {
            instance: "only".into(),
            wl_socket: "wayland-0".into(),
        }];
        assert_eq!(
            select_hyprland_instance(&instances, None),
            Some("only".into())
        );
    }

    #[test]
    fn leaves_ambiguous_instances_unselected_without_socket_identity() {
        let instances: Vec<HyprlandInstance> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hyprland/instances-ambiguous.json"
        ))
        .unwrap();
        assert_eq!(select_hyprland_instance(&instances, None), None);
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-9")),
            None
        );
    }

    #[test]
    fn leaves_duplicate_socket_matches_unselected() {
        let instances = vec![
            HyprlandInstance {
                instance: "first".into(),
                wl_socket: "wayland-0".into(),
            },
            HyprlandInstance {
                instance: "second".into(),
                wl_socket: "wayland-0".into(),
            },
        ];
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-0")),
            None
        );
    }

    #[test]
    fn keeps_valid_instances_when_one_inventory_entry_is_malformed() {
        let payload =
            include_bytes!("../../../tests/fixtures/hyprland/instances-version-matrix.json");
        let instances = parse_hyprland_instances(payload).unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-2")),
            Some("second".into())
        );
    }

    const BINDINGS: &str =
        include_str!("../../../tests/fixtures/hyprland/binds-representative.json");

    #[test]
    fn preserves_a_universal_match_when_a_regular_binding_is_primary() {
        let combo: KeyCombo = "ctrl+x".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "X",
                "submap": "default",
                "submap_universal": false,
                "dispatcher": "exec",
                "arg": "regular"
            },
            {
                "modmask": 4,
                "key": "X",
                "submap": "default",
                "submap_universal": true,
                "dispatcher": "exec",
                "arg": "universal"
            }
        ]"#;
        let result = inspect_json(&combo, bindings, "default").unwrap();
        let evidence = result.binding.expect("a primary binding is expected");
        assert_eq!(evidence.scope, BindingScope::Submap("default".into()));
        assert!(evidence.has_universal_match);
    }

    #[test]
    fn sanitized_fixture_matrix_covers_legacy_structured_and_malformed_shapes() {
        let matrix: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hyprland/version-matrix.json"
        ))
        .unwrap();
        assert_eq!(matrix["schema_version"], 1);
        let entries = matrix["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 4);

        for entry in entries {
            let binds = entry["binds"].as_str().unwrap();
            let devices = entry["devices"].as_str().unwrap();
            let binds_json = match binds {
                "binds-representative.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-representative.json")
                }
                "binds-structured-device.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-structured-device.json")
                }
                "binds-malformed-entry.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-malformed-entry.json")
                }
                "binds-invalid-object.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-invalid-object.json")
                }
                other => panic!("unexpected binds fixture {other}"),
            };
            let devices_json = match devices {
                "devices-keyboards.json" => {
                    include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json")
                }
                "devices-layout-groups.json" => {
                    include_str!("../../../tests/fixtures/hyprland/devices-layout-groups.json")
                }
                other => panic!("unexpected devices fixture {other}"),
            };
            serde_json::from_str::<serde_json::Value>(binds_json).unwrap();
            serde_json::from_str::<serde_json::Value>(devices_json).unwrap();
        }
    }

    #[test]
    fn reports_a_binding_in_the_default_submap() {
        let combo: KeyCombo = "super+c".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.details[1].contains("__lua 61"));
        assert!(result.details[1].contains("Universal copy"));
    }

    #[test]
    fn ignores_a_binding_in_an_inactive_submap() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(
            result
                .verbose_details
                .iter()
                .any(|detail| detail.contains("inactive submap binding")),
            "inactive submaps stay behind --verbose"
        );
        assert!(
            result
                .details
                .iter()
                .all(|detail| !detail.contains("inactive submap binding"))
        );
    }

    #[test]
    fn reports_that_a_non_consuming_binding_is_forwarded() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "resize").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn reports_auto_consuming_as_indeterminate() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.details[1].contains("all submaps"));
    }

    #[test]
    fn warns_when_a_same_key_binding_may_ignore_modifiers() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.summary.contains("no exact binding"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("no exact binding for CTRL + C"))
        );
    }

    #[test]
    fn honors_explicit_ignore_mods() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "C",
            "ignore_mods": true,
            "dispatcher": "exec",
            "arg": "notify-send copy"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
        assert!(result.details[1].contains("modifier-insensitive binding"));
    }

    #[test]
    fn reports_inhibitor_safe_binding_flags() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "Z",
            "locked": true,
            "allow_input_capture": true,
            "dispatcher": "exec",
            "arg": "notify-send suspend"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("input inhibitor"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("input capture"))
        );
    }

    #[test]
    fn matches_numeric_keycode_combinations() {
        let combo: KeyCombo = "ctrl+code:30".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 30,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn keeps_device_specific_bindings_conditional() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": "at-translated-set-2-keyboard",
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("device-specific"))
        );
    }

    #[test]
    fn accepts_structured_device_scope_from_newer_hyprland_json() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings =
            include_str!("../../../tests/fixtures/hyprland/binds-structured-device.json");

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("device-specific"))
        );
    }

    #[test]
    fn physical_capture_resolves_device_specific_binding() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": "example-keyboard",
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("Example-Keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(46),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("captured evdev device"))
        );
    }

    #[test]
    fn matched_binding_populates_typed_evidence() {
        let combo: KeyCombo = "ctrl+super+return".parse().unwrap();
        let bindings = r#"[{
            "modmask": 68,
            "key": "Return",
            "dispatcher": "__lua",
            "arg": "285",
            "description": "Herdr",
            "submap": "default",
            "submap_universal": true
        }]"#;
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], false, None).unwrap();
        let evidence = result.binding.expect("a match must carry evidence");
        assert_eq!(evidence.dispatcher.as_deref(), Some("__lua"));
        assert_eq!(evidence.action.as_deref(), Some("__lua 285"));
        assert_eq!(evidence.description.as_deref(), Some("Herdr"));
        assert_eq!(evidence.submap.as_deref(), Some("default"));
        assert_eq!(evidence.scope, BindingScope::Universal);
        assert!(evidence.is_universal());
        assert!(evidence.is_opaque());
    }

    #[test]
    fn evidence_links_a_config_hint_only_on_exact_description() {
        let combo: KeyCombo = "ctrl+super+return".parse().unwrap();
        let bindings = r#"[{
            "modmask": 68,
            "key": "Return",
            "dispatcher": "__lua",
            "arg": "285",
            "description": "Herdr",
            "submap": "default"
        }]"#;
        let hints = vec![LuaBindHint {
            combo: combo.clone(),
            description: Some("Herdr".into()),
            action: None,
            ignore_mods: None,
            source: PathBuf::from("/home/user/.config/hypr/bindings.lua"),
            line_number: 42,
        }];
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &hints, false, None)
                .unwrap();
        let evidence = result.binding.expect("a match must carry evidence");
        assert_eq!(evidence.scope, BindingScope::Submap("default".into()));
        let source = evidence.source.expect("an exact hint must link");
        assert_eq!(source.file, "/home/user/.config/hypr/bindings.lua");
        assert_eq!(source.line, Some(42));
        let unrelated =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], false, None).unwrap();
        assert!(
            unrelated
                .binding
                .as_ref()
                .is_some_and(|evidence| evidence.source.is_none()),
            "no hint must mean no file guess"
        );
    }

    #[test]
    fn physical_capture_keeps_unproven_device_mismatch_conditional() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": {"inclusive": true, "list": ["other-keyboard"]},
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("example-keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(46),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("could not be matched by name"))
        );
    }

    #[test]
    fn physical_evdev_code_matches_xkb_code_offset() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn physical_evdev_code_does_not_match_raw_xkb_number() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 105,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn explicit_code_query_is_not_broadened_by_physical_capture() {
        let combo: KeyCombo = "ctrl+code:30".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
    }

    #[test]
    fn keeps_valid_bindings_when_one_effective_entry_has_an_unexpected_shape() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = include_str!("../../../tests/fixtures/hyprland/binds-malformed-entry.json");

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("could not parse 1"))
        );
    }

    #[test]
    fn summarizes_active_keyboard_keymaps() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json");
        assert_eq!(
            summarize_keyboards(devices).as_deref(),
            Some(
                "main keyboard: at-translated-set-2-keyboard, keymap English (US), main; 1 additional keyboard(s): rk68-consumer, keymap English (US)",
            )
        );
    }

    #[test]
    fn pass_dispatcher_redirects_without_non_consuming() {
        let combo: KeyCombo = "ctrl+p".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "P",
            "dispatcher": "pass",
            "arg": "class:example"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Redirected);
    }

    #[test]
    fn release_binding_consumes_the_press_until_release() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "dispatcher": "exec",
            "arg": "notify-send released"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn non_consuming_release_binding_passes_the_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "non_consuming": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn auto_consuming_release_binding_passes_the_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "auto_consuming": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn release_pass_binding_is_redirected_on_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "dispatcher": "pass",
            "arg": "class:example"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Redirected);
    }

    #[test]
    fn long_press_binding_passes_the_original_press() {
        let combo: KeyCombo = "ctrl+l".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "L",
            "longPress": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn release_long_press_binding_still_suppresses_the_press() {
        let combo: KeyCombo = "ctrl+l".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "L",
            "release": true,
            "longPress": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn submap_binding_stops_later_bindings() {
        let combo: KeyCombo = "ctrl+m".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "M",
                "dispatcher": "submap",
                "arg": "resize"
            },
            {
                "modmask": 4,
                "key": "M",
                "dispatcher": "pass",
                "arg": "class:example"
            }
        ]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn possible_ignore_mods_bindings_are_combined_with_exact_matches() {
        let combo: KeyCombo = "ctrl+x".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "X",
                "non_consuming": true,
                "dispatcher": "exec"
            },
            {
                "modmask": 64,
                "key": "X",
                "dispatcher": "exec"
            }
        ]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("possible modifier-insensitive"))
        );
    }

    #[test]
    fn parses_xkb_keycodes_and_symbols() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <LEFT> = 113;
    <FK01> = 67;
};
xkb_symbols "pc" {
    key <LEFT> { [ Left ] };
    key <FK01> {
        type = "CTRL+ALT",
        symbols[1] = [ F1, F1 ]
    };
};
};
"#;
        let mapping = parse_xkb_symbol_keycodes(keymap);
        assert_eq!(mapping.get("Left"), Some(&vec![113]));
        assert_eq!(
            parse_xkb_symbol_fragment("symbols[1] = [ F1, F1 ]"),
            Some(vec!["F1".into(), "F1".into()])
        );
        assert_eq!(
            parse_xkb_symbol_key_name("key <FK01> {"),
            Some("FK01".into())
        );
        assert_eq!(mapping.get("F1"), Some(&vec![67]));
    }

    #[test]
    fn selects_only_symbols_from_the_active_xkb_layout_group() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, Q ]
        symbols[2] = [ adiaeresis, Adiaeresis ]
    };
};
};
"#;

        let first = parse_xkb_symbol_keycodes_for_group(keymap, 0);
        assert_eq!(first.get("q"), Some(&vec![24]));
        assert_eq!(first.get("Q"), Some(&vec![24]));
        let second = parse_xkb_symbol_keycodes_for_group(keymap, 1);
        assert_eq!(second.get("adiaeresis"), Some(&vec![24]));
        assert_eq!(second.get("Adiaeresis"), Some(&vec![24]));
        let missing = parse_xkb_symbol_keycodes_for_group(keymap, 3);
        assert!(missing.is_empty());
    }

    #[test]
    fn reads_the_main_keyboard_active_layout_group() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-layout-groups.json");
        assert_eq!(main_keyboard_active_layout_index(devices), Some(2));
    }

    #[test]
    fn does_not_compile_a_guessed_us_layout_when_devices_omit_layout() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json");
        assert!(compile_main_xkb_keymap(devices).is_none());
    }

    #[test]
    fn matches_symbolic_query_against_xkb_keycode_binding() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[crate::xkb::XkbKeycode::from(113)],
            &[],
            false,
            None,
        )
        .unwrap();
        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn parses_ignore_mods_from_declarative_bind_syntax() {
        let variables = HashMap::from([(String::from("mainMod"), String::from("SUPER"))]);
        let hint = parse_config_bind_hint("bind[i] = $mainMod, Z, exec, test", &variables)
            .expect("bind hint");
        assert_eq!(hint.key, "Z");
        assert!(hint.ignore_mods);
    }

    #[test]
    fn skips_unresolved_declarative_bind_variables() {
        assert!(
            parse_config_bind_hint("bind[i] = $unknown, Z, exec, test", &HashMap::new()).is_none()
        );
    }

    #[test]
    fn resolves_relative_config_includes() {
        let variables = HashMap::new();
        let path = resolve_config_path(
            "parts/binds.conf",
            Path::new("/tmp/hyprland.conf"),
            &variables,
        );
        assert_eq!(path, PathBuf::from("/tmp/parts/binds.conf"));
    }

    #[test]
    fn follows_literal_includes_and_variables_for_ignore_mods() {
        let base = std::env::temp_dir().join(format!("whykey-hypr-config-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let root = base.join("hyprland.conf");
        let child = base.join("bindings.conf");
        fs::write(&root, "$mainMod = SUPER\nsource = bindings.conf\n").unwrap();
        fs::write(&child, "bind[i] = $mainMod, Z, exec, test\n").unwrap();

        let mut visited = HashSet::new();
        let mut variables = HashMap::new();
        let mut hints = Vec::new();
        collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
        assert!(hints.iter().any(|hint| hint.key == "Z" && hint.ignore_mods));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn uses_config_hint_to_resolve_same_key_modifiers() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "Z",
            "dispatcher": "exec"
        }]"#;
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], true, None).unwrap();
        assert_eq!(result.status(), LayerStatus::Handled);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("ignore_mods"))
        );
    }

    #[test]
    fn parses_literal_omarchy_lua_binding() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + C", "Copy", "wl-copy")"#,
            Path::new("/tmp/bindings.lua"),
            7,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.to_string(), "SUPER + C");
        assert_eq!(hint.description.as_deref(), Some("Copy"));
        assert_eq!(hint.action.as_deref(), Some("wl-copy"));
        assert_eq!(hint.ignore_mods, Some(false));
        assert_eq!(hint.line_number, 7);
    }

    #[test]
    fn parses_lua_ignore_mods_option() {
        let hint = parse_lua_bind_hint(
            r#"hl.bind("Z", hl.dsp.exec_cmd("suspend"), { ignore_mods = true })"#,
            Path::new("/tmp/bindings.lua"),
            3,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.key(), "Z");
        assert_eq!(hint.ignore_mods, Some(true));
    }

    #[test]
    fn leaves_dynamic_lua_options_unknown() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + LEFT", "Move", "movefocus l", options)"#,
            Path::new("/tmp/bindings.lua"),
            5,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.ignore_mods, None);
    }

    #[test]
    fn parses_multiline_lua_function_binding() {
        let hints = parse_lua_bind_hints(
            r#"
o.bind("SUPER + CTRL + Z", "Zoom in", function()
  local zoom = hl.get_config("cursor.zoom_factor") or 1
  hl.config({ cursor = { zoom_factor = zoom + 1 } })
end)
"#,
            Path::new("/tmp/utilities.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "CTRL + SUPER + Z");
        assert_eq!(hints[0].description.as_deref(), Some("Zoom in"));
        assert_eq!(hints[0].ignore_mods, Some(false));
        assert_eq!(hints[0].line_number, 2);
    }

    #[test]
    fn ignores_bind_text_inside_lua_strings_and_long_comments() {
        let hints = parse_lua_bind_hints(
            r##"
local quoted = "o.bind(\"SUPER + A\", \"fake\", \"fake\")"
-- o.bind("SUPER + B", "fake", "fake")
--[=[
o.bind("SUPER + C", "fake", "fake")
]=]
local long_string = [=[hl.bind("SUPER + D", "fake")]=]
o.bind("SUPER + E", "Real", "exec")
"##,
            Path::new("/tmp/bindings.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "SUPER + E");
        assert_eq!(hints[0].description.as_deref(), Some("Real"));
    }

    #[test]
    fn resumes_code_after_a_closed_long_comment() {
        let hints = parse_lua_bind_hints(
            "--[[ ignored ]] o.bind(\"SUPER + F\", \"Find\", \"search\")",
            Path::new("/tmp/bindings.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "SUPER + F");
    }

    #[test]
    fn does_not_reduce_an_unrecognized_object_method_to_a_global_bind() {
        assert!(
            parse_lua_bind_hints(
                "custom.bind(\"SUPER + G\", \"Unknown\", \"exec\")",
                Path::new("/tmp/bindings.lua"),
            )
            .is_empty()
        );
    }

    #[test]
    fn parses_representative_consecutive_omarchy_bindings() {
        let hints = parse_lua_bind_hints(
            include_str!("../../../tests/fixtures/hyprland/omarchy-bindings.lua"),
            Path::new("tests/fixtures/hyprland/omarchy-bindings.lua"),
        );
        assert_eq!(hints.len(), 5);
        assert!(hints.iter().all(|hint| hint.ignore_mods == Some(false)));
    }

    #[test]
    fn lua_default_options_rule_out_a_different_modifier_match() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "LEFT",
            "dispatcher": "movefocus"
        }]"#;
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + LEFT", "Move", "movefocus l")"#,
            Path::new("/tmp/tiling.lua"),
            1,
        )
        .expect("Lua binding hint");
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[hint], false, None)
                .unwrap();
        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn accepts_optional_space_before_lua_call_parenthesis() {
        let hint = parse_lua_bind_hint(
            r#"o.bind ( "CTRL + F", "Find", "search" )"#,
            Path::new("/tmp/bindings.lua"),
            4,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.to_string(), "CTRL + F");
    }

    #[test]
    fn ignores_dynamic_lua_binding_expressions() {
        assert!(
            parse_lua_bind_hint(
                "o.bind(modifier .. \" + C\", \"Copy\", command)",
                Path::new("/tmp/bindings.lua"),
                1,
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_literal_prefix_concatenated_with_dynamic_lua_expression() {
        assert!(
            parse_lua_bind_hint(
                "o.bind(\"SUPER + C\" .. suffix, \"Copy\", command)",
                Path::new("/tmp/bindings.lua"),
                1,
            )
            .is_none()
        );
    }

    #[test]
    fn accepts_escaped_characters_only_inside_a_complete_lua_literal() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("CTRL + F", "Find\nHere", "search")"#,
            Path::new("/tmp/bindings.lua"),
            1,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.description.as_deref(), Some("Find\nHere"));
        assert!(lua_literal_argument(r#""CTRL + F"   "#).is_some());
        assert!(lua_literal_argument(r#""CTRL + F" .. suffix"#).is_none());
    }

    #[test]
    fn rejects_invalid_json() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let error = inspect_json(&combo, "not json", "default").unwrap_err();

        assert!(error.starts_with("hyprctl returned invalid binding data:"));
    }

    #[test]
    fn rejects_a_valid_json_object_instead_of_a_binding_array() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let error = inspect_json(
            &combo,
            include_str!("../../../tests/fixtures/hyprland/binds-invalid-object.json"),
            "default",
        )
        .unwrap_err();

        assert!(error.contains("expected an array"));
    }
}
