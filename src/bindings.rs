use serde::Serialize;

use crate::layers::{
    cinnamon, gnome, hyprland, i3, kde, labwc, mate, niri, openbox, programmable, river, sway,
    sxhkd, wayfire, x11, xfce,
};
use crate::schema;

const MAX_BINDINGS: usize = 8192;

/// One binding returned by a supported desktop source.
#[derive(Debug, Clone, Serialize)]
pub struct BindingEntry {
    pub source: String,
    pub key: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Typed device scope read from the adapter payload, never inferred
    /// from `context`. `None` means the adapter did not report one, which
    /// never matches an active `--device` filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Typed submap read from the adapter payload, never inferred from
    /// `context`. Same missing-metadata rule as `device`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submap: Option<String>,
    pub certainty: String,
}

/// Versioned output for `whykey bindings`.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema_version: u8,
    /// False means the listed sources are useful evidence but do not prove
    /// every runtime client, plugin, or firmware transformation.
    pub complete: bool,
    pub bindings: Vec<BindingEntry>,
    pub unavailable: Vec<String>,
    pub limitations: Vec<String>,
}

/// One collection pass target for a desktop adapter.
pub struct BindingSink<'a> {
    pub entries: &'a mut Vec<BindingEntry>,
    pub unavailable: &'a mut Vec<String>,
}

pub fn current() -> Inventory {
    // One discovery pass: applicability comes from the shared Environment
    // snapshot instead of re-running desktop detection per adapter.
    let environment = crate::environment::Environment::collect();
    let mut inventory = Inventory {
        schema_version: 1,
        complete: false,
        bindings: Vec::new(),
        unavailable: Vec::new(),
        limitations: vec![
            "the inventory covers only sources exposed by detected adapters".into(),
            "application, plugin, input-inhibitor, and firmware bindings may remain outside the listing".into(),
        ],
    };

    {
        let mut sink = BindingSink {
            entries: &mut inventory.bindings,
            unavailable: &mut inventory.unavailable,
        };
        for entry in crate::registry::DESKTOPS {
            let Some(collect) = entry.bindings else {
                continue;
            };
            if !inventory_adapter_applies(&environment, entry) {
                continue;
            }
            collect(&mut sink);
        }
    }

    if environment.desktop("compositor").applicable
        && !crate::registry::DESKTOPS
            .iter()
            .any(|entry| entry.bindings.is_some() && inventory_adapter_applies(&environment, entry))
    {
        inventory
            .limitations
            .push("the detected desktop has no dedicated binding inventory adapter".into());
    }

    inventory.bindings.sort_by(|left, right| {
        (
            &left.source,
            &left.key,
            &left.context,
            &left.device,
            &left.submap,
            &left.action,
        )
            .cmp(&(
                &right.source,
                &right.key,
                &right.context,
                &right.device,
                &right.submap,
                &right.action,
            ))
    });
    if inventory.bindings.len() > MAX_BINDINGS {
        inventory.bindings.truncate(MAX_BINDINGS);
        inventory.limitations.push(format!(
            "binding inventory was truncated to {MAX_BINDINGS} entries"
        ));
    }
    inventory
}

pub fn filter(
    mut inventory: Inventory,
    key: Option<&str>,
    action: Option<&str>,
    source: Option<&str>,
    device: Option<&str>,
    submap: Option<&str>,
) -> Inventory {
    inventory.bindings.retain(|binding| {
        contains(&binding.key, key)
            && contains(&binding.action, action)
            && contains(&binding.source, source)
            && contains_optional(binding.device.as_deref(), device)
            && contains_optional(binding.submap.as_deref(), submap)
    });
    inventory
}

fn contains(haystack: &str, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

/// Missing metadata never matches an active filter: filters combine with
/// AND logic, so an entry without a device cannot satisfy `--device`.
fn contains_optional(field: Option<&str>, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        field.is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
    })
}

/// Whether a bindings-capable adapter contributes to this session. The X11
/// and programmable X11 adapters only expose bindings when their
/// configuration is readable, mirroring their original applicability rules.
fn inventory_adapter_applies(
    environment: &crate::environment::Environment,
    entry: &crate::registry::AdapterDescriptor,
) -> bool {
    let status = environment.desktop(entry.id);
    if matches!(entry.id, "x11" | "programmable") {
        status.applicable && status.ipc
    } else {
        status.applicable
    }
}

pub(crate) fn hyprland_entries(sink: &mut BindingSink<'_>) {
    match hyprland::binding_inventory_json() {
        Ok(value) => collect_hyprland(&value, sink.entries),
        Err(error) => sink.unavailable.push(format!("Hyprland: {error}")),
    }
}

pub(crate) fn sway_entries(sink: &mut BindingSink<'_>) {
    match sway::binding_inventory_json() {
        Ok(value) => collect_sway_or_i3(&value, "Sway", sink.entries),
        Err(error) => sink.unavailable.push(format!("Sway: {error}")),
    }
}

pub(crate) fn i3_entries(sink: &mut BindingSink<'_>) {
    match i3::binding_inventory_json() {
        Ok(value) => collect_sway_or_i3(&value, "i3", sink.entries),
        Err(error) => sink.unavailable.push(format!("i3: {error}")),
    }
}

pub(crate) fn gnome_entries(sink: &mut BindingSink<'_>) {
    match gnome::binding_inventory() {
        Ok(bindings) => {
            for (key, name, command) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "GNOME".into(),
                    key: key.compact_display(),
                    action: command.map_or(name.clone(), |command| format!("{name}: {command}")),
                    context: Some("global media/custom shortcut".into()),
                    certainty: "configured".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("GNOME: {error}")),
    }
}

pub(crate) fn kde_entries(sink: &mut BindingSink<'_>) {
    match kde::binding_inventory() {
        Ok(bindings) => {
            for (key, group, action, description) in bindings {
                let action = description.map_or_else(
                    || action.clone(),
                    |description| format!("{action} ({description})"),
                );
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "KDE Plasma".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(group),
                    certainty: "configured; runtime registration conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("KDE Plasma: {error}")),
    }
}

pub(crate) fn xfce_entries(sink: &mut BindingSink<'_>) {
    match xfce::binding_inventory() {
        Ok(bindings) => {
            for (key, action, context) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "Xfce".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(context),
                    certainty: "runtime channel value".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("Xfce: {error}")),
    }
}

pub(crate) fn cinnamon_entries(sink: &mut BindingSink<'_>) {
    match cinnamon::binding_inventory() {
        Ok(bindings) => {
            for (key, action, command) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "Cinnamon".into(),
                    key: key.compact_display(),
                    action: command
                        .map_or(action.clone(), |command| format!("{action}: {command}")),
                    context: Some("global GSettings shortcut".into()),
                    certainty: "runtime GSettings value".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("Cinnamon: {error}")),
    }
}

pub(crate) fn mate_entries(sink: &mut BindingSink<'_>) {
    match mate::binding_inventory() {
        Ok(bindings) => {
            for (key, action, command) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "MATE".into(),
                    key: key.compact_display(),
                    action: command
                        .map_or(action.clone(), |command| format!("{action}: {command}")),
                    context: Some("global GSettings shortcut".into()),
                    certainty: "runtime GSettings value".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("MATE: {error}")),
    }
}

pub(crate) fn niri_entries(sink: &mut BindingSink<'_>) {
    match niri::binding_inventory() {
        Ok(bindings) => {
            for (key, action) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "Niri".into(),
                    key: key.compact_display(),
                    action,
                    context: Some("config.kdl binds block".into()),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("Niri: {error}")),
    }
}

pub(crate) fn river_entries(sink: &mut BindingSink<'_>) {
    match river::binding_inventory() {
        Ok(bindings) => {
            for (key, action, mode) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "River".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(format!("riverctl map mode: {mode}")),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("River: {error}")),
    }
}

pub(crate) fn wayfire_entries(sink: &mut BindingSink<'_>) {
    match wayfire::binding_inventory() {
        Ok(bindings) => {
            for (key, action, section) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "Wayfire".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(if section.is_empty() {
                        "wayfire binding".into()
                    } else {
                        format!("Wayfire [{section}]")
                    }),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("Wayfire: {error}")),
    }
}

pub(crate) fn labwc_entries(sink: &mut BindingSink<'_>) {
    match labwc::binding_inventory() {
        Ok(bindings) => {
            for (key, action, context) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "labwc".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(context),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("labwc: {error}")),
    }
}

pub(crate) fn sxhkd_entries(sink: &mut BindingSink<'_>) {
    match sxhkd::binding_inventory() {
        Ok(bindings) => {
            for (key, action) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "sxhkd".into(),
                    key: key.compact_display(),
                    action,
                    context: Some("sxhkdrc static binding".into()),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("sxhkd: {error}")),
    }
}

pub(crate) fn openbox_entries(sink: &mut BindingSink<'_>) {
    match openbox::binding_inventory() {
        Ok(bindings) => {
            for (key, action, context) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "Openbox".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(context),
                    certainty: "configured; runtime activation conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("Openbox: {error}")),
    }
}

pub(crate) fn x11_entries(sink: &mut BindingSink<'_>) {
    match x11::binding_inventory() {
        Ok(bindings) => {
            for (key, action) in bindings {
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source: "X11 xbindkeys".into(),
                    key: key.compact_display(),
                    action,
                    context: Some(".xbindkeysrc static binding".into()),
                    certainty: "configured; daemon/runtime precedence conditional".into(),
                });
            }
        }
        Err(error) => sink.unavailable.push(format!("X11 xbindkeys: {error}")),
    }
}

pub(crate) fn programmable_entries(sink: &mut BindingSink<'_>) {
    match programmable::binding_inventory() {
        Ok(bindings) => {
            for (key, action, context) in bindings {
                let source = programmable::detect()
                    .map_or_else(|| "Programmable X11 WM".into(), |wm| wm.name().into());
                sink.entries.push(BindingEntry {
                    device: None,
                    submap: None,
                    source,
                    key: key.compact_display(),
                    action,
                    context: Some(context),
                    certainty: "configured; executable runtime and precedence conditional".into(),
                });
            }
        }
        Err(error) => sink
            .unavailable
            .push(format!("programmable X11 WM: {error}")),
    }
}

fn collect_hyprland(value: &serde_json::Value, entries: &mut Vec<BindingEntry>) {
    let Some(bindings) = value.as_array() else {
        return;
    };
    for binding in bindings {
        let Some(key) = binding.get("key").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let mask = binding
            .get("modmask")
            .and_then(json_u32)
            .unwrap_or_default();
        let action = binding
            .get("dispatcher")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown dispatcher");
        let mut action = action.to_owned();
        if let Some(argument) = binding
            .get("arg")
            .and_then(serde_json::Value::as_str)
            .filter(|argument| !argument.is_empty())
        {
            action.push(' ');
            action.push_str(argument);
        }
        if let Some(description) = binding
            .get("description")
            .and_then(serde_json::Value::as_str)
            .filter(|description| !description.is_empty())
        {
            action.push_str("; ");
            action.push_str(description);
        }
        let submap = binding
            .get("submap")
            .and_then(serde_json::Value::as_str)
            .filter(|submap| !submap.is_empty() && *submap != "reset")
            .map_or_else(|| "default".into(), str::to_owned);
        // Typed filter fields come straight from the payload. `device` stays
        // absent for structured per-device scopes instead of guessing.
        let device = binding
            .get("device")
            .and_then(serde_json::Value::as_str)
            .filter(|device| !device.is_empty())
            .map(str::to_owned);
        entries.push(BindingEntry {
            device,
            submap: Some(submap.clone()),
            source: "Hyprland (hyprctl binds -j)".into(),
            key: combo_display(mask, key),
            action,
            context: Some(submap),
            certainty: "runtime effective binding".into(),
        });
    }
}

fn collect_sway_or_i3(value: &serde_json::Value, source: &str, entries: &mut Vec<BindingEntry>) {
    let Some(bindings) = value.as_array() else {
        return;
    };
    for binding in bindings {
        let mask = binding.get("event_state_mask").and_then(json_u32);
        let action = binding
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown command")
            .to_owned();
        let mut keys = Vec::new();
        for field in ["keysym", "symbols", "symbol"] {
            if let Some(value) = binding.get(field) {
                match value {
                    serde_json::Value::String(value) => keys.push(value.clone()),
                    serde_json::Value::Array(values) => keys.extend(
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned),
                    ),
                    _ => {}
                }
            }
        }
        if let Some(code) = binding.get("input_code").and_then(json_u32) {
            keys.push(format!("code:{code}"));
        }
        if let Some(values) = binding
            .get("keycodes")
            .and_then(serde_json::Value::as_array)
        {
            keys.extend(
                values
                    .iter()
                    .filter_map(json_u32)
                    .map(|code| format!("code:{code}")),
            );
        }
        if keys.is_empty() {
            continue;
        }
        let certainty = if mask.is_some() {
            "runtime effective binding"
        } else {
            "runtime binding; modifier mask unknown"
        };
        for key in keys {
            entries.push(BindingEntry {
                device: None,
                submap: None,
                source: source.into(),
                key: mask.map_or_else(
                    || key.to_ascii_uppercase(),
                    |mask| combo_display(mask, &key),
                ),
                action: action.clone(),
                context: None,
                certainty: certainty.into(),
            });
        }
    }
}

fn json_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn combo_display(mask: u32, key: &str) -> String {
    let mut prefix = String::new();
    for (bit, name) in [
        (1, "SHIFT"),
        (2, "CAPS"),
        (4, "CTRL"),
        (8, "ALT"),
        (16, "MOD2"),
        (32, "MOD3"),
        (64, "SUPER"),
        (128, "MOD5"),
    ] {
        if mask & bit != 0 {
            prefix.push_str(name);
            prefix.push('+');
        }
    }
    prefix.push_str(&key.to_ascii_uppercase());
    prefix
}

pub fn render_text(inventory: &Inventory) -> String {
    let mut output = String::from("whykey bindings\n\n");
    output.push_str(&format!(
        "Scope: {} source(s); complete: {}\n",
        inventory
            .bindings
            .iter()
            .map(|entry| entry.source.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        if inventory.complete { "yes" } else { "no" }
    ));
    for entry in &inventory.bindings {
        output.push_str(&format!(
            "- {}  {}  {} [{}]\n",
            entry.key, entry.action, entry.source, entry.certainty
        ));
        if let Some(context) = &entry.context {
            output.push_str(&format!("  context: {context}\n"));
        }
        if let Some(device) = &entry.device {
            output.push_str(&format!("  device: {device}\n"));
        }
        if let Some(submap) = &entry.submap {
            output.push_str(&format!("  submap: {submap}\n"));
        }
    }
    if inventory.bindings.is_empty() {
        output.push_str("No bindings were enumerated from the detected sources.\n");
    }
    for unavailable in &inventory.unavailable {
        output.push_str(&format!("! unavailable: {unavailable}\n"));
    }
    for limitation in &inventory.limitations {
        output.push_str(&format!("? limitation: {limitation}\n"));
    }
    output
}

pub fn render_json(inventory: &Inventory, schema_version: u8) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "bindings",
            "context": schema::context(),
            "complete": inventory.complete,
            "bindings": inventory.bindings,
            "unavailable": inventory.unavailable,
            "limitations": inventory.limitations,
        }))
        .expect("bindings are serializable")
            + "\n";
    }
    serde_json::to_string_pretty(inventory).expect("binding inventory is serializable") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_modifier_masks_without_changing_the_raw_key() {
        assert_eq!(combo_display(4 | 64, "c"), "CTRL+SUPER+C");
        assert_eq!(combo_display(0, "code:30"), "CODE:30");
    }

    #[test]
    fn reports_an_explicitly_incomplete_inventory() {
        let inventory = Inventory {
            schema_version: 1,
            complete: false,
            bindings: Vec::new(),
            unavailable: Vec::new(),
            limitations: vec!["runtime clients remain unknown".into()],
        };
        let value: serde_json::Value = serde_json::from_str(&render_json(&inventory, 1)).unwrap();
        assert_eq!(value["complete"], false);
        assert_eq!(value["schema_version"], 1);
    }

    fn filter_fixture() -> Inventory {
        Inventory {
            schema_version: 1,
            complete: false,
            bindings: vec![
                BindingEntry {
                    device: Some("AT Keyboard".into()),
                    submap: Some("default".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec foot".into(),
                    context: Some("default".into()),
                    certainty: "configured".into(),
                },
                BindingEntry {
                    device: Some("Other Keyboard".into()),
                    submap: Some("gaming".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec kitty".into(),
                    context: Some("gaming".into()),
                    certainty: "configured".into(),
                },
                BindingEntry {
                    device: None,
                    submap: None,
                    source: "GNOME".into(),
                    key: "SUPER+X".into(),
                    action: "Open overview".into(),
                    context: None,
                    certainty: "configured".into(),
                },
            ],
            unavailable: Vec::new(),
            limitations: Vec::new(),
        }
    }

    #[test]
    fn filters_bindings_by_case_insensitive_key_action_and_source() {
        let filtered = filter(
            filter_fixture(),
            Some("ctrl+x"),
            Some("foot"),
            Some("hypr"),
            None,
            None,
        );

        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].source, "Hyprland");
    }

    #[test]
    fn filters_bindings_by_device_only() {
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            None,
            Some("at KEYBOARD"),
            None,
        );
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec foot");
    }

    #[test]
    fn filters_bindings_by_submap_only() {
        let filtered = filter(filter_fixture(), None, None, None, None, Some("GAMING"));
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec kitty");
    }

    #[test]
    fn filters_bindings_by_combined_key_device_and_submap() {
        let filtered = filter(
            filter_fixture(),
            Some("ctrl+x"),
            None,
            None,
            Some("other"),
            Some("gaming"),
        );
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec kitty");
        let empty = filter(
            filter_fixture(),
            Some("ctrl+x"),
            None,
            None,
            Some("other"),
            Some("default"),
        );
        assert!(empty.bindings.is_empty());
    }

    #[test]
    fn missing_metadata_never_matches_an_active_filter() {
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            Some("gnome"),
            Some("keyboard"),
            None,
        );
        assert!(filtered.bindings.is_empty());
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            Some("gnome"),
            None,
            Some("default"),
        );
        assert!(filtered.bindings.is_empty());
    }

    #[test]
    fn text_and_json_return_the_same_filtered_entries() {
        let filtered = filter(filter_fixture(), Some("ctrl+x"), None, None, None, None);
        let text = render_text(&filtered);
        assert!(text.contains("exec foot"));
        assert!(text.contains("exec kitty"));
        assert!(text.contains("submap: default"));
        assert!(text.contains("submap: gaming"));
        for version in [1, 2] {
            let value: serde_json::Value =
                serde_json::from_str(&render_json(&filtered, version)).unwrap();
            let entries = value["bindings"].as_array().unwrap();
            assert_eq!(entries.len(), 2);
            assert!(entries.iter().any(|entry| entry["submap"] == "default"));
        }
        let device_only = filter(filter_fixture(), None, None, None, Some("other"), None);
        let text = render_text(&device_only);
        assert!(text.contains("exec kitty"));
        assert!(!text.contains("exec foot"));
        let value: serde_json::Value = serde_json::from_str(&render_json(&device_only, 1)).unwrap();
        assert_eq!(value["bindings"].as_array().unwrap().len(), 1);
    }
}
