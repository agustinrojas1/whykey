//! One owner for adapter registration.
//!
//! Route dispatch, binding inventory, focused-window lookup, doctor,
//! capabilities, and generated support documentation all use the same
//! registry instead of scattered adapter lists. Adding a desktop adapter
//! requires its module and one entry in [`DESKTOPS`].

use crate::capture::NativeCaptureIo;
use crate::key::KeyCombo;
use crate::layers::{
    BindingRecord, LayerResult, PhysicalInput, cinnamon, compositor, gnome, hyprland, i3, kde,
    labwc, mate, niri, openbox, programmable, river, sway, sxhkd, wayfire, x11, xfce,
};

/// Read-only focused-window PID lookup with its evidence label.
pub type FocusLookup = (&'static str, fn() -> Result<u32, String>);

/// Metadata for one desktop capability entry.
#[derive(Clone, Copy)]
pub struct CapabilityMeta {
    pub id: &'static str,
    pub name: &'static str,
    /// Custom evidence text; `None` uses [`compositor_evidence`] with
    /// [`CapabilityMeta::name`].
    pub evidence: Option<fn(applicable: bool, ipc: bool) -> String>,
}

/// Metadata for one doctor check.
#[derive(Clone, Copy)]
pub struct DoctorMeta {
    /// Text check label.
    pub check: &'static str,
    /// Remediation hint for a failed check.
    pub hint: &'static str,
    /// Key used in the doctor JSON desktop section.
    pub json_key: &'static str,
}

/// Adapter binding inventory function: enumerate records directly, or return
/// an unavailable reason already prefixed with the adapter's source label.
pub type BindingInventory = fn() -> Result<Vec<BindingRecord>, String>;

/// Native capture transport factory. `None` means the adapter is currently
/// static-only and has no live capture transport.
pub type CaptureFactory = fn() -> Result<Box<dyn NativeCaptureIo>, String>;

/// Static adapter descriptor: one registry entry plus its adapter module.
pub struct AdapterDescriptor {
    /// Stable ID used by selection and tests.
    pub id: &'static str,
    /// Human display name.
    pub display: &'static str,
    pub applicable: fn() -> bool,
    pub ipc: Option<fn() -> bool>,
    /// Inspect one key combination through this adapter.
    pub inspect: fn(&KeyCombo, Option<&PhysicalInput>) -> LayerResult,
    /// Enumerate configured bindings into the inventory, when supported.
    pub bindings: Option<BindingInventory>,
    /// Read-only focused-window PID lookup, when supported.
    pub focus: Option<FocusLookup>,
    /// Capability entry metadata, when the adapter has its own entry.
    pub capability: Option<CapabilityMeta>,
    /// Doctor check metadata, when the adapter has its own check.
    pub doctor: Option<DoctorMeta>,
    /// Live native capture transport, when the adapter provides one.
    pub capture: Option<CaptureFactory>,
}

// Inspect wrappers: one uniform signature per adapter. Only Hyprland needs
// the physical input context; the others ignore it.
fn inspect_hyprland(key: &KeyCombo, input: Option<&PhysicalInput>) -> LayerResult {
    hyprland::Hyprland.inspect_with_input(key, input)
}
fn inspect_sway(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    sway::Sway.inspect(key)
}
fn inspect_i3(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    i3::I3.inspect(key)
}
fn inspect_kde(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    kde::Kde.inspect(key)
}
fn inspect_xfce(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    xfce::Xfce.inspect(key)
}
fn inspect_cinnamon(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    cinnamon::Cinnamon.inspect(key)
}
fn inspect_mate(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    mate::Mate.inspect(key)
}
fn inspect_niri(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    niri::Niri.inspect(key)
}
fn inspect_river(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    river::River.inspect(key)
}
fn inspect_wayfire(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    wayfire::Wayfire.inspect(key)
}
fn inspect_labwc(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    labwc::Labwc.inspect(key)
}
fn inspect_sxhkd(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    sxhkd::Sxhkd.inspect(key)
}
fn inspect_openbox(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    openbox::Openbox.inspect(key)
}
fn inspect_gnome(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    gnome::Gnome.inspect(key)
}
fn inspect_programmable(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    programmable::Programmable.inspect(key)
}
fn inspect_x11(key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    x11::X11.inspect(key)
}
fn inspect_generic_compositor(_key: &KeyCombo, _input: Option<&PhysicalInput>) -> LayerResult {
    compositor::inspect()
}

/// Standard evidence text for a desktop capability entry.
pub fn compositor_evidence(name: &str, applicable: bool, ipc: bool) -> String {
    match (applicable, ipc) {
        (true, true) => format!("{name} session detected and its read-only IPC is available"),
        (true, false) => format!("{name} session detected, but its read-only IPC is unavailable"),
        (false, _) => format!("{name} session not detected"),
    }
}

fn x11_evidence(applicable: bool, ipc: bool) -> String {
    if applicable {
        if ipc {
            "X11 session/configuration detected and .xbindkeysrc is readable".into()
        } else {
            "X11 session detected, but xbindkeys configuration is unavailable".into()
        }
    } else {
        "X11 session not detected".into()
    }
}

fn programmable_evidence(applicable: bool, ipc: bool) -> String {
    if applicable {
        let name = programmable::detect().map_or("programmable X11 WM", |wm| wm.name());
        if ipc {
            format!("{name} configuration is readable; literal bindings can be inspected")
        } else {
            format!("{name} detected, but its configuration is unavailable")
        }
    } else {
        "AwesomeWM, Qtile, and XMonad not detected".into()
    }
}

/// Desktop adapters in inspect priority order. Adding an adapter requires
/// one entry here plus its adapter module.
pub static DESKTOPS: &[AdapterDescriptor] = &[
    AdapterDescriptor {
        id: "hyprland",
        display: "Hyprland",
        applicable: hyprland::applicable,
        ipc: Some(hyprland::ipc_available),
        inspect: inspect_hyprland,
        bindings: Some(hyprland::binding_inventory),
        focus: Some(("Hyprland activewindow", hyprland::focused_pid)),
        capability: Some(CapabilityMeta {
            id: "compositor.hyprland",
            name: "Hyprland",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Hyprland IPC",
            hint: "run inside the Hyprland session",
            json_key: "hyprland",
        }),
        capture: Some(crate::hyprland_capture::capture_connect),
    },
    AdapterDescriptor {
        id: "sway",
        display: "Sway",
        applicable: sway::applicable,
        ipc: Some(sway::ipc_available),
        inspect: inspect_sway,
        bindings: Some(sway::binding_inventory),
        focus: Some(("Sway get_tree", sway::focused_pid)),
        capability: Some(CapabilityMeta {
            id: "compositor.sway",
            name: "Sway",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Sway IPC",
            hint: "run inside the Sway session",
            json_key: "sway",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "i3",
        display: "i3",
        applicable: i3::applicable,
        ipc: Some(i3::ipc_available),
        inspect: inspect_i3,
        bindings: Some(i3::binding_inventory),
        focus: Some(("i3 get_tree", i3::focused_pid)),
        capability: Some(CapabilityMeta {
            id: "compositor.i3",
            name: "i3",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "i3 IPC",
            hint: "run inside the i3 session",
            json_key: "i3",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "kde",
        display: "KDE",
        applicable: kde::applicable,
        ipc: Some(kde::ipc_available),
        inspect: inspect_kde,
        bindings: Some(kde::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.kde",
            name: "KDE Plasma",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "KDE global shortcuts",
            hint: "make kglobalshortcutsrc readable or enable the KDE session",
            json_key: "kde",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "xfce",
        display: "XFCE",
        applicable: xfce::applicable,
        ipc: Some(xfce::ipc_available),
        inspect: inspect_xfce,
        bindings: Some(xfce::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.xfce",
            name: "Xfce",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Xfce keyboard shortcuts",
            hint: "install xfconf-query or enable the Xfce session",
            json_key: "xfce",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "cinnamon",
        display: "Cinnamon",
        applicable: cinnamon::applicable,
        ipc: Some(cinnamon::ipc_available),
        inspect: inspect_cinnamon,
        bindings: Some(cinnamon::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.cinnamon",
            name: "Cinnamon",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Cinnamon shortcuts",
            hint: "install or enable gsettings for the Cinnamon session",
            json_key: "cinnamon",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "mate",
        display: "MATE",
        applicable: mate::applicable,
        ipc: Some(mate::ipc_available),
        inspect: inspect_mate,
        bindings: Some(mate::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.mate",
            name: "MATE",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "MATE shortcuts",
            hint: "install or enable gsettings for the MATE session",
            json_key: "mate",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "niri",
        display: "Niri",
        applicable: niri::applicable,
        ipc: Some(niri::ipc_available),
        inspect: inspect_niri,
        bindings: Some(niri::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.niri",
            name: "Niri",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Niri keybinds",
            hint: "set NIRI_CONFIG or provide ~/.config/niri/config.kdl",
            json_key: "niri",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "river",
        display: "River",
        applicable: river::applicable,
        ipc: Some(river::ipc_available),
        inspect: inspect_river,
        bindings: Some(river::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.river",
            name: "River",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "River bindings",
            hint: "set RIVER_INIT or provide ~/.config/river/init",
            json_key: "river",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "wayfire",
        display: "Wayfire",
        applicable: wayfire::applicable,
        ipc: Some(wayfire::ipc_available),
        inspect: inspect_wayfire,
        bindings: Some(wayfire::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.wayfire",
            name: "Wayfire",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Wayfire shortcuts",
            hint: "set WAYFIRE_CONFIG or provide ~/.config/wayfire.ini",
            json_key: "wayfire",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "labwc",
        display: "Labwc",
        applicable: labwc::applicable,
        ipc: Some(labwc::ipc_available),
        inspect: inspect_labwc,
        bindings: Some(labwc::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.labwc",
            name: "labwc",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "labwc keybinds",
            hint: "set LABWC_CONFIG or provide ~/.config/labwc/rc.xml",
            json_key: "labwc",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "sxhkd",
        display: "Sxhkd",
        applicable: sxhkd::applicable,
        ipc: Some(sxhkd::ipc_available),
        inspect: inspect_sxhkd,
        bindings: Some(sxhkd::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.bspwm-sxhkd",
            name: "bspwm/sxhkd",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "sxhkd bindings",
            hint: "set SXHKD_CONFIG or provide ~/.config/sxhkd/sxhkdrc",
            json_key: "bspwm_sxhkd",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "openbox",
        display: "Openbox",
        applicable: openbox::applicable,
        ipc: Some(openbox::ipc_available),
        inspect: inspect_openbox,
        bindings: Some(openbox::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.openbox",
            name: "Openbox",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "Openbox keybinds",
            hint: "set OPENBOX_CONFIG or provide ~/.config/openbox/rc.xml",
            json_key: "openbox",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "gnome",
        display: "GNOME",
        applicable: gnome::applicable,
        ipc: Some(gnome::ipc_available),
        inspect: inspect_gnome,
        bindings: Some(gnome::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.gnome",
            name: "GNOME",
            evidence: None,
        }),
        doctor: Some(DoctorMeta {
            check: "GNOME shortcuts",
            hint: "install or enable gsettings",
            json_key: "gnome",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "programmable",
        display: "Programmable",
        applicable: programmable::applicable,
        ipc: Some(programmable::ipc_available),
        inspect: inspect_programmable,
        bindings: Some(programmable::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.programmable-x11",
            name: "programmable X11 WM",
            evidence: Some(programmable_evidence),
        }),
        doctor: Some(DoctorMeta {
            check: "programmable X11 WM",
            hint: "set the corresponding *_CONFIG variable or provide its standard config",
            json_key: "programmable_x11",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "x11",
        display: "X11",
        applicable: x11::applicable,
        ipc: Some(x11::ipc_available),
        inspect: inspect_x11,
        bindings: Some(x11::binding_inventory),
        focus: None,
        capability: Some(CapabilityMeta {
            id: "compositor.x11.xbindkeys",
            name: "X11 xbindkeys",
            evidence: Some(x11_evidence),
        }),
        doctor: Some(DoctorMeta {
            check: "X11 xbindkeys",
            hint: "set XBINDKEYSRC or provide ~/.xbindkeysrc",
            json_key: "x11_xbindkeys",
        }),
        capture: None,
    },
    AdapterDescriptor {
        id: "compositor",
        display: "Compositor",
        applicable: compositor::applicable,
        ipc: None,
        inspect: inspect_generic_compositor,
        bindings: None,
        focus: None,
        capability: None,
        doctor: None,
        capture: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_one_entry_per_desktop_adapter() {
        assert!(
            DESKTOPS.len() >= 16,
            "registry must own every desktop adapter"
        );
        let mut seen = std::collections::HashSet::new();
        for entry in DESKTOPS {
            assert!(seen.insert(entry.id), "duplicate registry id {}", entry.id);
        }
    }

    #[test]
    fn registry_priority_matches_inspect_selection() {
        assert!(DESKTOPS.len() >= 3);
        assert_eq!(
            [DESKTOPS[0].id, DESKTOPS[1].id, DESKTOPS[2].id],
            ["hyprland", "sway", "i3"]
        );
        assert_eq!(DESKTOPS.last().map(|entry| entry.id), Some("compositor"));
        let environment = crate::environment::Environment::collect();
        let expected = DESKTOPS
            .iter()
            .find(|entry| environment.desktop(entry.id).applicable)
            .map(|entry| entry.id);
        assert_eq!(
            environment.selected_compositor,
            environment
                .compositor_candidates
                .first()
                .map(|candidate| candidate.id)
                .or(expected)
        );
    }

    #[test]
    fn registry_and_capabilities_cannot_disagree_on_existence() {
        let capabilities = crate::capabilities::current();
        for entry in DESKTOPS {
            if matches!(
                entry.id,
                "compositor" | "programmable" | "x11" | "sxhkd" | "openbox"
            ) {
                continue;
            }
            let needle = format!("compositor.{entry_id}", entry_id = entry.id);
            assert!(
                capabilities
                    .iter()
                    .any(|capability| capability.id == needle || capability.id.contains(entry.id)),
                "registry adapter {} must appear in capabilities",
                entry.id
            );
        }
    }

    #[test]
    fn support_matrix_cannot_drift_from_registry_capability_entries() {
        #[derive(serde::Deserialize)]
        struct Matrix {
            entries: Vec<Entry>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            id: String,
        }
        let matrix: Matrix = serde_json::from_str(include_str!("../support-matrix.json")).unwrap();
        for entry in DESKTOPS {
            let Some(capability) = entry.capability else {
                continue;
            };
            assert!(
                matrix.entries.iter().any(|e| e.id == capability.id),
                "support-matrix.json is missing the registry capability {}",
                capability.id
            );
        }
    }
}
