use serde::Serialize;

pub use crate::layers::BindingRecord;
/// Compatibility alias from the 1.x series: new code uses [`BindingRecord`].
pub type BindingEntry = BindingRecord;

use crate::schema;

const MAX_BINDINGS: usize = 8192;

/// Versioned output for `whykey bindings`.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema_version: u8,
    /// False means the listed sources are useful evidence but do not prove
    /// every runtime client, plugin, or firmware transformation.
    pub complete: bool,
    pub bindings: Vec<BindingRecord>,
    pub unavailable: Vec<String>,
    pub limitations: Vec<String>,
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

    for entry in crate::registry::DESKTOPS {
        let Some(collect) = entry.inventory.or(entry.bindings) else {
            continue;
        };
        if !inventory_adapter_applies(&environment, entry) {
            continue;
        }
        match collect() {
            Ok(mut records) => inventory.bindings.append(&mut records),
            Err(error) => inventory.unavailable.push(error),
        }
    }

    for adapter in &environment.extension_adapters {
        if !crate::extension_adapters::applicable_with_context(
            adapter,
            &environment.compositor_context,
        ) {
            continue;
        }
        let report = environment.extension_inventory(adapter);
        if let Some(error) = report.error {
            inventory.unavailable.push(error);
            continue;
        }
        inventory.bindings.extend(report.records);
        if report.malformed_entries > 0 {
            inventory.unavailable.push(format!(
                "{}: {} malformed binding entr{} skipped",
                adapter.display,
                report.malformed_entries,
                if report.malformed_entries == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }
    }

    if environment.desktop("compositor").applicable
        && !crate::registry::DESKTOPS.iter().any(|entry| {
            entry.inventory.or(entry.bindings).is_some()
                && inventory_adapter_applies(&environment, entry)
        })
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
        assert_eq!(crate::layers::combo_display(4 | 64, "c"), "CTRL+SUPER+C");
        assert_eq!(crate::layers::combo_display(0, "code:30"), "CODE:30");
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
                BindingRecord {
                    device: Some("AT Keyboard".into()),
                    submap: Some("default".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec foot".into(),
                    context: Some("default".into()),
                    certainty: "configured".into(),
                },
                BindingRecord {
                    device: Some("Other Keyboard".into()),
                    submap: Some("gaming".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec kitty".into(),
                    context: Some("gaming".into()),
                    certainty: "configured".into(),
                },
                BindingRecord {
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
