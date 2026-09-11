//! Diagnostic report snapshots for replay and comparison.
//!
//! A snapshot stores one redacted schema-v2 report plus its request context.
//! Replaying it only renders the stored report; it never sends a key event,
//! queries the desktop, re-runs adapter analysis, or executes a dispatcher.
//! The stored adapter outputs are conclusions, not the raw IPC or
//! configuration inputs analysis would need to run again offline.
//! Inspect a snapshot before sharing it: redaction is fixed and mechanical,
//! not a review of your data.

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
    if redact_secrets(snapshot) {
        redactions.push("secret-bearing assignments and command arguments");
    }
    snapshot["redactions"] = serde_json::json!(redactions);
}

/// Keys whose values are secrets more often than not. The match is
/// case-insensitive and covers `KEY=value`, `KEY: value`, and `--key value`
/// spellings; the value runs to the next whitespace, quote, comma, or
/// semicolon boundary.
const SECRET_KEYS: &[&str] = &[
    "token", "password", "passwd", "secret", "api_key", "apikey", "api-key",
];

fn redact_secrets(value: &mut Value) -> bool {
    match value {
        Value::String(text) => redact_secret_string(text),
        Value::Array(items) => {
            let mut redacted = false;
            for item in items {
                redacted |= redact_secrets(item);
            }
            redacted
        }
        Value::Object(items) => {
            let mut redacted = false;
            for item in items.values_mut() {
                redacted |= redact_secrets(item);
            }
            redacted
        }
        _ => false,
    }
}

fn redact_secret_string(text: &mut String) -> bool {
    let mut redacted = false;
    for key in SECRET_KEYS {
        let mut search_from = 0;
        loop {
            let lower = text.to_ascii_lowercase();
            let Some(found) = lower[search_from..].find(key) else {
                break;
            };
            let start = search_from + found;
            let after_key = &text[start + key.len()..];
            // `--key value` spelling: dashes before the key, spaces after.
            let flag_form = start >= 2 && text[..start].ends_with("--");
            let separator = after_key
                .chars()
                .next()
                .filter(|next| *next == '=' || *next == ':' || (flag_form && next.is_whitespace()));
            let Some(separator) = separator else {
                search_from = start + key.len();
                continue;
            };
            let value_start = start + key.len() + separator.len_utf8();
            let rest = &text[value_start..];
            // Skip whitespace and quote characters around the value.
            let trimmed = rest.trim_start_matches([' ', '\t', '"', '\'']);
            let skipped = rest.len() - trimmed.len();
            let value_end = trimmed
                .find([' ', '\t', '"', '\'', ',', ';'])
                .unwrap_or(trimmed.len());
            if value_end == 0 {
                search_from = start + key.len();
                continue;
            }
            let absolute_start = value_start + skipped;
            text.replace_range(absolute_start..absolute_start + value_end, "[redacted]");
            redacted = true;
            search_from = absolute_start + "[redacted]".len();
        }
    }
    redacted
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
        let layers = [LayerResult::new(
            "Hyprland",
            crate::layers::LayerId::Compositor,
            crate::layers::Outcome::HandledUncertain,
            "runtime effect unknown",
            vec!["binding: __lua 285; Herdr".into()],
        )];

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

    #[test]
    fn redacts_every_private_path_and_the_shell_path() {
        let mut snapshot = serde_json::json!({
            "context": {"shell": "/bin/fish"},
            "report": {"path": [{"evidence": [
                {"text": "config: /home/alice/.config/hypr/bindings.lua"},
                {"text": "socket: /run/user/1000/hypr/alice/.socket2.sock"},
                {"text": "nested /home/alice/first and /home/alice/second"},
            ]}]}
        });

        redact_private_data(&mut snapshot);

        let texts: Vec<_> = snapshot["report"]["path"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["text"].as_str().unwrap().to_string())
            .collect();
        assert!(
            texts
                .iter()
                .all(|text| !text.contains("/home/") && !text.contains("/run/user/")),
            "every private path must go, got {texts:?}"
        );
        assert_eq!(snapshot["context"]["shell"], "[redacted]");
    }

    #[test]
    fn redacts_secret_assignments_but_keeps_binding_descriptions() {
        let mut snapshot = serde_json::json!({
            "report": {"path": [{"evidence": [
                {"text": "exec pass --token s3cr3t-value"},
                {"text": "env: API_KEY=abcdef12345"},
                {"text": "export PASSWORD=hunter2"},
                {"text": "description: Password Manager"},
                {"text": "binding: exec foot; Herdr"},
            ]}]}
        });

        redact_private_data(&mut snapshot);

        let texts: Vec<_> = snapshot["report"]["path"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["text"].as_str().unwrap().to_string())
            .collect();
        assert!(!texts.iter().any(|text| text.contains("s3cr3t-value")));
        assert!(!texts.iter().any(|text| text.contains("abcdef12345")));
        assert!(!texts.iter().any(|text| text.contains("hunter2")));
        assert!(
            texts.iter().any(|text| text.contains("Password Manager")),
            "ordinary descriptions survive, got {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("binding: exec foot; Herdr")),
            "binding actions survive, got {texts:?}"
        );
    }
}
