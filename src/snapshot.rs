//! Reproducible, offline-safe diagnostic snapshots.
//!
//! A snapshot records the inputs and structured result of one static
//! inspection. Replaying it only renders the stored report; it never sends a
//! key event, queries the desktop, or executes a dispatcher.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::key::KeyCombo;
use crate::layers::LayerResult;
use crate::{report, schema};

pub const SNAPSHOT_SCHEMA_VERSION: u8 = 1;
const SNAPSHOT_KIND: &str = "whykey.diagnostic_snapshot";

pub fn create(key: &KeyCombo, layers: &[LayerResult]) -> Value {
    let report = serde_json::from_str::<Value>(&report::render_json(key, layers, None, 2))
        .expect("whykey reports are valid JSON");
    let mut snapshot = serde_json::json!({
        "schema_version": SNAPSHOT_SCHEMA_VERSION,
        "kind": SNAPSHOT_KIND,
        "tool": {
            "name": "whykey",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "context": schema::context(),
        "request": {
            "kind": "key",
            "key": key,
            "key_display": key.to_string(),
        },
        "report": report,
        "redactions": [],
    });
    redact_private_data(&mut snapshot);
    snapshot
}

pub fn render(snapshot: &Value) -> String {
    serde_json::to_string_pretty(snapshot).expect("snapshot JSON is serializable") + "\n"
}

pub fn save(path: &Path, snapshot: &Value) -> std::io::Result<()> {
    fs::write(path, render(snapshot))
}

pub fn is_snapshot(value: &Value) -> bool {
    value.get("kind").and_then(Value::as_str) == Some(SNAPSHOT_KIND)
}

pub fn report(value: &Value) -> &Value {
    value
        .get("report")
        .filter(|report| report.is_object())
        .unwrap_or(value)
}

fn redact_private_data(snapshot: &mut Value) {
    let mut redactions = Vec::new();
    if let Some(shell) = snapshot
        .get_mut("context")
        .and_then(|context| context.get_mut("shell"))
    {
        if !shell.is_null() {
            *shell = Value::String("[redacted]".into());
            redactions.push("context.shell");
        }
    }
    if redact_paths(snapshot) {
        redactions.push("private paths in captured evidence");
    }
    snapshot["redactions"] = serde_json::json!(redactions);
}

fn redact_paths(value: &mut Value) -> bool {
    match value {
        Value::String(text) if text.contains("/home/") || text.contains("/run/user/") => {
            *text = "[redacted: private path]".into();
            true
        }
        Value::Array(items) => {
            let mut redacted = false;
            for item in items {
                redacted |= redact_paths(item);
            }
            redacted
        }
        Value::Object(items) => {
            let mut redacted = false;
            for item in items.values_mut() {
                redacted |= redact_paths(item);
            }
            redacted
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_versioned_snapshot_without_observation_injection() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let layers = [LayerResult {
            binding: None,
            layer: "Hyprland",
            id: crate::layers::LayerId::Compositor,
            outcome: crate::layers::Outcome::HandledUncertain,
            summary: "runtime effect unknown".into(),
            details: vec!["binding: __lua 285; Herdr".into()],
        }];

        let snapshot = create(&key, &layers);

        assert_eq!(snapshot["schema_version"], SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot["kind"], SNAPSHOT_KIND);
        assert_eq!(snapshot["request"]["key_display"], "CTRL + SUPER + RETURN");
        assert_eq!(snapshot["report"]["operation"], "inspect");
        assert!(snapshot["report"]["observation"].is_null());
        assert_eq!(report(&snapshot)["operation"], "inspect");
    }

    #[test]
    fn recognizes_only_the_snapshot_envelope_kind() {
        assert!(is_snapshot(&serde_json::json!({
            "kind": SNAPSHOT_KIND
        })));
        assert!(!is_snapshot(&serde_json::json!({
            "kind": "whykey.report"
        })));
    }

    #[test]
    fn redacts_shell_identity_and_private_paths() {
        let mut snapshot = serde_json::json!({
            "context": {"shell": "/bin/bash"},
            "report": {"path": [{"evidence": [{"text": "source: /home/alice/.config/hypr"}]}]}
        });

        redact_private_data(&mut snapshot);

        assert_eq!(snapshot["context"]["shell"], "[redacted]");
        assert_eq!(
            snapshot["report"]["path"][0]["evidence"][0]["text"],
            "[redacted: private path]"
        );
        assert!(snapshot["redactions"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn redacts_all_private_paths_in_a_collection() {
        let mut snapshot = serde_json::json!({
            "evidence": [
                "source: /home/alice/.config/hypr",
                {"path": "/run/user/1000/hyprland.sock"},
                "unrelated detail"
            ]
        });

        redact_private_data(&mut snapshot);

        assert_eq!(snapshot["evidence"][0], "[redacted: private path]");
        assert_eq!(snapshot["evidence"][1]["path"], "[redacted: private path]");
        assert_eq!(snapshot["evidence"][2], "unrelated detail");
    }
}
