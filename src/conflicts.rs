use std::collections::BTreeMap;

use serde::Serialize;

use crate::bindings;
use crate::schema;

#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub key: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submap: Option<String>,
    pub classification: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema_version: u8,
    pub conflicts: Vec<Conflict>,
    pub unavailable: Vec<String>,
    pub limitations: Vec<String>,
}

pub fn current() -> Report {
    let inventory = bindings::current();
    let mut groups: BTreeMap<(String, String, Option<String>), Vec<&bindings::BindingEntry>> =
        BTreeMap::new();
    for entry in &inventory.bindings {
        groups
            .entry((
                entry.source.clone(),
                entry.key.clone(),
                entry.context.clone(),
            ))
            .or_default()
            .push(entry);
    }

    let mut conflicts = Vec::new();
    let mut unknown_context_groups = 0;
    for ((source, key, context), entries) in groups {
        if context.is_none() {
            if entries
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1
            {
                unknown_context_groups += 1;
            }
            continue;
        }
        let mut actions = entries
            .iter()
            .map(|entry| entry.action.clone())
            .collect::<Vec<_>>();
        actions.sort();
        actions.dedup();
        if actions.len() < 2 {
            continue;
        }
        let runtime = entries
            .iter()
            .all(|entry| entry.certainty == "runtime effective binding");
        conflicts.push(Conflict {
            key,
            source,
            context,
            device: entries.first().and_then(|entry| entry.device.clone()),
            submap: entries.first().and_then(|entry| entry.submap.clone()),
            classification: if runtime {
                "possible; precedence/order is not exposed".into()
            } else {
                "possible; runtime activation or precedence is unknown".into()
            },
            actions,
        });
    }

    let mut limitations = inventory.limitations;
    if unknown_context_groups > 0 {
        limitations.push(format!(
            "{unknown_context_groups} duplicate group(s) were omitted because their active context was unavailable"
        ));
    }
    limitations.push(
        "identical actions and bindings in different applications are not global conflicts".into(),
    );

    Report {
        schema_version: 1,
        conflicts,
        unavailable: inventory.unavailable,
        limitations,
    }
}

pub fn filter(
    mut report: Report,
    key: Option<&str>,
    action: Option<&str>,
    source: Option<&str>,
    device: Option<&str>,
    submap: Option<&str>,
) -> Report {
    report.conflicts.retain(|conflict| {
        contains(&conflict.key, key)
            && conflict.actions.iter().any(|value| {
                action.is_none_or(|needle| {
                    value
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
            })
            && contains(&conflict.source, source)
            && contains_optional(conflict.device.as_deref(), device)
            && contains_optional(conflict.submap.as_deref(), submap)
    });
    report
}

fn contains(haystack: &str, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

fn contains_optional(field: Option<&str>, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        field.is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
    })
}

pub fn render_text(report: &Report) -> String {
    let mut output = String::from("whykey conflicts\n\n");
    if report.conflicts.is_empty() {
        output.push_str("No conflicts were found in the enumerated sources.\n");
    } else {
        for conflict in &report.conflicts {
            output.push_str(&format!(
                "- {} [{}] {}\n",
                conflict.key, conflict.classification, conflict.source
            ));
            if let Some(context) = &conflict.context {
                output.push_str(&format!("  context: {context}\n"));
            }
            if let Some(device) = &conflict.device {
                output.push_str(&format!("  device: {device}\n"));
            }
            if let Some(submap) = &conflict.submap {
                output.push_str(&format!("  submap: {submap}\n"));
            }
            for action in &conflict.actions {
                output.push_str(&format!("  action: {action}\n"));
            }
        }
    }
    for unavailable in &report.unavailable {
        output.push_str(&format!("! unavailable: {unavailable}\n"));
    }
    for limitation in &report.limitations {
        output.push_str(&format!("? limitation: {limitation}\n"));
    }
    output
}

pub fn render_json(report: &Report, schema_version: u8) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "conflicts",
            "context": schema::context(),
            "conflicts": report.conflicts,
            "unavailable": report.unavailable,
            "limitations": report.limitations,
        }))
        .expect("conflicts are serializable")
            + "\n";
    }
    serde_json::to_string_pretty(report).expect("conflict report is serializable") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_different_actions_in_the_same_context_are_conflicts() {
        let entries = vec![
            bindings::BindingEntry {
                device: None,
                submap: None,
                source: "test".into(),
                key: "CTRL+C".into(),
                action: "first".into(),
                context: Some("default".into()),
                certainty: "runtime effective binding".into(),
            },
            bindings::BindingEntry {
                device: None,
                submap: None,
                source: "test".into(),
                key: "CTRL+C".into(),
                action: "second".into(),
                context: Some("default".into()),
                certainty: "runtime effective binding".into(),
            },
        ];
        let mut groups = BTreeMap::new();
        for entry in &entries {
            groups
                .entry((
                    entry.source.clone(),
                    entry.key.clone(),
                    entry.context.clone(),
                ))
                .or_insert_with(Vec::new)
                .push(entry);
        }
        assert_eq!(groups.len(), 1);
        let actions = groups
            .values()
            .next()
            .unwrap()
            .iter()
            .map(|entry| entry.action.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actions, vec!["first", "second"]);
    }

    #[test]
    fn renders_versioned_json() {
        let report = Report {
            schema_version: 1,
            conflicts: Vec::new(),
            unavailable: Vec::new(),
            limitations: Vec::new(),
        };
        let value: serde_json::Value = serde_json::from_str(&render_json(&report, 1)).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(value["conflicts"].is_array());
    }

    fn conflict_fixture() -> Report {
        Report {
            schema_version: 1,
            conflicts: vec![
                Conflict {
                    key: "CTRL+X".into(),
                    source: "Hyprland".into(),
                    context: Some("default".into()),
                    device: Some("AT Keyboard".into()),
                    submap: Some("default".into()),
                    classification: "possible".into(),
                    actions: vec!["Open terminal".into(), "Open editor".into()],
                },
                Conflict {
                    key: "SUPER+X".into(),
                    source: "GNOME".into(),
                    context: Some("global".into()),
                    device: None,
                    submap: None,
                    classification: "possible".into(),
                    actions: vec!["Open overview".into(), "Open settings".into()],
                },
            ],
            unavailable: Vec::new(),
            limitations: Vec::new(),
        }
    }

    #[test]
    fn filters_conflicts_by_key_action_and_source() {
        let filtered = filter(
            conflict_fixture(),
            Some("ctrl+x"),
            Some("terminal"),
            Some("hypr"),
            None,
            None,
        );

        assert_eq!(filtered.conflicts.len(), 1);
        assert_eq!(filtered.conflicts[0].source, "Hyprland");
    }

    #[test]
    fn filters_conflicts_by_device_and_submap() {
        let filtered = filter(
            conflict_fixture(),
            None,
            None,
            None,
            Some("at keyboard"),
            None,
        );
        assert_eq!(filtered.conflicts.len(), 1);
        assert_eq!(filtered.conflicts[0].key, "CTRL+X");
        let filtered = filter(conflict_fixture(), None, None, None, None, Some("DEFAULT"));
        assert_eq!(filtered.conflicts.len(), 1);
        let combined = filter(
            conflict_fixture(),
            Some("ctrl+x"),
            Some("editor"),
            None,
            Some("at"),
            Some("default"),
        );
        assert_eq!(combined.conflicts.len(), 1);
        let missing = filter(
            conflict_fixture(),
            None,
            None,
            Some("gnome"),
            Some("kbd"),
            None,
        );
        assert!(missing.conflicts.is_empty());
        let text = render_text(&filtered);
        assert!(text.contains("CTRL+X"));
        let value: serde_json::Value = serde_json::from_str(&render_json(&filtered, 1)).unwrap();
        assert_eq!(value["conflicts"].as_array().unwrap().len(), 1);
    }
}
