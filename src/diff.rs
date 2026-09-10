//! Comparison of saved Whykey reports and diagnostic snapshots.

use serde::Serialize;
use serde_json::Value;

use crate::snapshot;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Change {
    pub path: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

pub fn compare(before: &Value, after: &Value) -> Vec<Change> {
    let before_report = snapshot::report(before);
    let after_report = snapshot::report(after);
    let mut changes = Vec::new();

    compare_value(
        &mut changes,
        "context",
        context(before, before_report),
        context(after, after_report),
    );
    compare_value(
        &mut changes,
        "input",
        request_key(before_report),
        request_key(after_report),
    );
    compare_route(&mut changes, "path", before_report, after_report);
    compare_steps(&mut changes, before_report, after_report);
    changes
}

pub fn render_text(changes: &[Change]) -> String {
    let mut output = String::from("whykey diff\n\n");
    if changes.is_empty() {
        output.push_str("No differences were found in the compared context or route.\n");
        return output;
    }
    for change in changes {
        output.push_str(&format!("- {}\n", change.path));
        output.push_str(&format!(
            "  before: {}\n",
            display_value(change.before.as_ref())
        ));
        output.push_str(&format!(
            "  after:  {}\n",
            display_value(change.after.as_ref())
        ));
    }
    output
}

pub fn render_json(changes: &[Change], schema_version: u8) -> String {
    let document = if schema_version == 2 {
        serde_json::json!({
            "schema_version": 2,
            "operation": "diff",
            "changes": changes,
        })
    } else {
        serde_json::json!({
            "schema_version": 1,
            "operation": "diff",
            "changes": changes,
        })
    };
    serde_json::to_string_pretty(&document).expect("diff changes are serializable") + "\n"
}

fn request_key(report: &Value) -> Option<&Value> {
    report
        .get("key_display")
        .or_else(|| {
            report
                .get("input")
                .and_then(|input| input.get("key_display"))
        })
        .or_else(|| report.get("sequence_display"))
        .or_else(|| {
            report
                .get("input")
                .and_then(|input| input.get("sequence_display"))
        })
}

fn context<'a>(document: &'a Value, report: &'a Value) -> Option<&'a Value> {
    document.get("context").or_else(|| report.get("context"))
}

fn compare_steps(changes: &mut Vec<Change>, before: &Value, after: &Value) {
    let before_steps = before.get("steps").and_then(Value::as_array);
    let after_steps = after.get("steps").and_then(Value::as_array);
    let length = before_steps
        .map_or(0, Vec::len)
        .max(after_steps.map_or(0, Vec::len));
    for index in 0..length {
        let before_step = before_steps.and_then(|steps| steps.get(index));
        let after_step = after_steps.and_then(|steps| steps.get(index));
        compare_value(
            changes,
            &format!("steps[{index}].input"),
            before_step.and_then(request_key),
            after_step.and_then(request_key),
        );
        compare_route(
            changes,
            &format!("steps[{index}].path"),
            before_step.unwrap_or(&Value::Null),
            after_step.unwrap_or(&Value::Null),
        );
    }
}

fn compare_route(changes: &mut Vec<Change>, prefix: &str, before: &Value, after: &Value) {
    let before_layers = before
        .get("path")
        .or_else(|| before.get("layers"))
        .and_then(Value::as_array);
    let after_layers = after
        .get("path")
        .or_else(|| after.get("layers"))
        .and_then(Value::as_array);
    let length = before_layers
        .map_or(0, Vec::len)
        .max(after_layers.map_or(0, Vec::len));
    for index in 0..length {
        let before_layer = before_layers.and_then(|layers| layers.get(index));
        let after_layer = after_layers.and_then(|layers| layers.get(index));
        let name = before_layer
            .or(after_layer)
            .and_then(|layer| layer.get("layer"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        compare_value(
            changes,
            &format!("{prefix}[{index}].{name}"),
            before_layer,
            after_layer,
        );
    }
}

fn compare_value(
    changes: &mut Vec<Change>,
    path: &str,
    before: Option<&Value>,
    after: Option<&Value>,
) {
    if before != after {
        changes.push(Change {
            path: path.to_owned(),
            before: before.cloned(),
            after: after.cloned(),
        });
    }
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string(value).expect("JSON value is serializable"),
        None => "<absent>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(summary: &str) -> Value {
        serde_json::json!({
            "schema_version": 2,
            "operation": "inspect",
            "context": {"session_type": "wayland"},
            "input": {"key_display": "CTRL + X"},
            "path": [{
                "layer": "Hyprland",
                "status": "Handled",
                "propagation": "Stops",
                "summary": summary,
                "evidence": []
            }]
        })
    }

    #[test]
    fn compares_snapshot_reports_by_context_request_and_layer() {
        let before = serde_json::json!({
            "schema_version": 1,
            "kind": "whykey.diagnostic_snapshot",
            "context": {"session_type": "wayland"},
            "report": report("old")
        });
        let after = serde_json::json!({
            "schema_version": 1,
            "kind": "whykey.diagnostic_snapshot",
            "context": {"session_type": "x11"},
            "report": {
                "schema_version": 2,
                "operation": "inspect",
                "context": {"session_type": "x11"},
                "input": {"key_display": "CTRL + Z"},
                "path": [{
                    "layer": "Terminal",
                    "status": "NotHandled",
                    "propagation": "Continues",
                    "summary": "new",
                    "evidence": []
                }]
            }
        });

        let changes = compare(&before, &after);

        assert!(changes.iter().any(|change| change.path == "context"));
        assert!(changes.iter().any(|change| change.path == "input"));
        assert!(
            changes
                .iter()
                .any(|change| change.path == "path[0].Hyprland")
        );
        assert!(changes.iter().any(|change| {
            change.path == "path[0].Hyprland"
                && change
                    .after
                    .as_ref()
                    .is_some_and(|value| value["layer"] == "Terminal")
        }));
    }

    #[test]
    fn identical_reports_have_no_changes() {
        let value = report("same");
        assert!(compare(&value, &value).is_empty());
        assert!(render_text(&[]).contains("No differences"));
        assert!(render_json(&[], 1).contains("\"changes\": []"));
        assert!(render_json(&[], 2).contains("\"schema_version\": 2"));
    }

    #[test]
    fn compares_sequence_steps_and_repeated_layers_by_position() {
        let before = serde_json::json!({
            "schema_version": 2,
            "operation": "inspect",
            "input": {"kind": "sequence", "sequence_display": "CTRL + X CTRL + S"},
            "steps": [{
                "input": {"key_display": "CTRL + X"},
                "path": [{"layer": "Hyprland", "summary": "first"}, {"layer": "Hyprland", "summary": "second"}]
            }]
        });
        let after = serde_json::json!({
            "schema_version": 2,
            "operation": "inspect",
            "input": {"kind": "sequence", "sequence_display": "CTRL + Z CTRL + S"},
            "steps": [{
                "input": {"key_display": "CTRL + Z"},
                "path": [{"layer": "Hyprland", "summary": "first"}, {"layer": "Hyprland", "summary": "changed"}]
            }]
        });

        let changes = compare(&before, &after);

        assert!(changes.iter().any(|change| change.path == "input"));
        assert!(changes.iter().any(|change| change.path == "steps[0].input"));
        assert!(
            changes
                .iter()
                .any(|change| change.path == "steps[0].path[1].Hyprland")
        );
    }
}
