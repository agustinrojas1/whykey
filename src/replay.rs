use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::snapshot;

const MAX_REPLAY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum ReplayError {
    Read(std::io::Error),
    TooLarge(u64),
    Empty,
    Invalid { document: usize, message: String },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "could not read replay file: {error}"),
            Self::TooLarge(size) => write!(
                formatter,
                "replay file is {size} bytes; the maximum supported size is {MAX_REPLAY_BYTES}"
            ),
            Self::Empty => formatter.write_str("replay file does not contain a JSON report"),
            Self::Invalid { document, message } => {
                write!(formatter, "invalid replay document {document}: {message}")
            }
        }
    }
}

impl std::error::Error for ReplayError {}

pub fn load(path: &Path) -> Result<Vec<Value>, ReplayError> {
    let file = fs::File::open(path).map_err(ReplayError::Read)?;
    let metadata = file.metadata().map_err(ReplayError::Read)?;
    if metadata.len() > MAX_REPLAY_BYTES {
        return Err(ReplayError::TooLarge(metadata.len()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_REPLAY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(ReplayError::Read)?;
    if bytes.len() as u64 > MAX_REPLAY_BYTES {
        return Err(ReplayError::TooLarge(bytes.len() as u64));
    }
    let mut documents = Vec::new();
    for (index, result) in serde_json::Deserializer::from_slice(&bytes)
        .into_iter::<Value>()
        .enumerate()
    {
        let value = result.map_err(|error| ReplayError::Invalid {
            document: index + 1,
            message: error.to_string(),
        })?;
        validate(&value, index + 1)?;
        documents.push(value);
    }
    if documents.is_empty() {
        return Err(ReplayError::Empty);
    }
    Ok(documents)
}

pub fn render_text(documents: &[Value]) -> String {
    let mut output = String::from("whykey replay\nReplayed reports; no input was injected.\n\n");
    for (index, document) in documents.iter().enumerate() {
        let report = snapshot::report(document);
        if index > 0 {
            output.push('\n');
        }
        if let Some(steps) = report.get("steps").and_then(Value::as_array) {
            let sequence = report
                .get("sequence_display")
                .or_else(|| {
                    report
                        .get("input")
                        .and_then(|input| input.get("sequence_display"))
                })
                .and_then(Value::as_str)
                .unwrap_or("unknown sequence");
            output.push_str(&format!("Sequence: {sequence}\n"));
            for (step_index, step) in steps.iter().enumerate() {
                let key = step
                    .get("key_display")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown key");
                let confidence = step
                    .get("confidence")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                output.push_str(&format!(
                    "Step {}/{}: {key} (assessment: {confidence})\n",
                    step_index + 1,
                    steps.len()
                ));
                render_layers(&mut output, step.get("layers").or_else(|| step.get("path")));
            }
        } else {
            let key = report
                .get("key_display")
                .or_else(|| {
                    report
                        .get("input")
                        .and_then(|input| input.get("key_display"))
                })
                .and_then(Value::as_str)
                .unwrap_or("unknown key");
            let confidence = report
                .get("confidence")
                .or_else(|| {
                    report
                        .get("assessment")
                        .and_then(|assessment| assessment.get("confidence"))
                })
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            output.push_str(&format!("Key: {key}\nAssessment: {confidence}\n"));
            render_layers(
                &mut output,
                report.get("layers").or_else(|| report.get("path")),
            );
        }
    }
    output
}

pub fn render_json(documents: &[Value], schema_version: u8) -> String {
    documents
        .iter()
        .map(|document| {
            let document = if schema_version == 2 && !snapshot::is_snapshot(document) {
                v2_document(document)
            } else {
                document.clone()
            };
            serde_json::to_string_pretty(&document).expect("validated replay JSON is serializable")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn v2_document(document: &Value) -> Value {
    if document.get("schema_version").and_then(Value::as_u64) == Some(2) {
        return document.clone();
    }
    if let Some(steps) = document.get("steps").and_then(Value::as_array) {
        return serde_json::json!({
            "schema_version": 2,
            "operation": "inspect",
            "context": stored_context(document),
            "input": {
                "kind": "sequence",
                "sequence": document.get("sequence"),
                "sequence_display": document.get("sequence_display"),
            },
            "steps": steps.iter().map(|step| serde_json::json!({
                "index": step.get("index"),
                "key": step.get("key"),
                "key_display": step.get("key_display"),
                "assessment": {"confidence": step.get("confidence")},
                "path": step.get("layers"),
            })).collect::<Vec<_>>(),
        });
    }
    serde_json::json!({
        "schema_version": 2,
        "operation": "inspect",
        "context": stored_context(document),
        "input": {
            "kind": "key",
            "key": document.get("key"),
            "key_display": document.get("key_display"),
        },
        "assessment": {"confidence": document.get("confidence")},
        "observation": document.get("observed"),
        "path": document.get("layers"),
    })
}

fn stored_context(document: &Value) -> Value {
    document
        .get("context")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"recorded": false}))
}

fn validate(value: &Value, document: usize) -> Result<(), ReplayError> {
    let Some(object) = value.as_object() else {
        return Err(invalid(document, "the document must be a JSON object"));
    };
    let schema_version = object.get("schema_version").and_then(Value::as_u64);
    if !matches!(schema_version, Some(1 | 2)) {
        return Err(invalid(
            document,
            "only schema_version 1 or 2 reports can be replayed",
        ));
    }
    if snapshot::is_snapshot(value) {
        if schema_version != Some(u64::from(snapshot::SNAPSHOT_SCHEMA_VERSION)) {
            return Err(invalid(document, "unsupported diagnostic snapshot schema"));
        }
        let Some(report) = object.get("report").filter(|value| value.is_object()) else {
            return Err(invalid(document, "snapshot is missing its report object"));
        };
        if !object.get("tool").is_some_and(Value::is_object)
            || !object.get("context").is_some_and(Value::is_object)
            || !object.get("request").is_some_and(Value::is_object)
        {
            return Err(invalid(
                document,
                "snapshot requires tool, context, request, and report objects",
            ));
        }
        validate(report, document)?;
        return Ok(());
    }
    let static_report = if schema_version == Some(2) {
        object.get("input").is_some_and(Value::is_object)
            && object.get("path").is_some_and(Value::is_array)
    } else {
        object.get("key").is_some_and(Value::is_object)
            && object.get("layers").is_some_and(Value::is_array)
    };
    let sequence_report = object.get("steps").is_some_and(Value::is_array);
    if !static_report && !sequence_report {
        return Err(invalid(
            document,
            "expected a key/layers report or a sequence steps report",
        ));
    }
    Ok(())
}

fn invalid(document: usize, message: &str) -> ReplayError {
    ReplayError::Invalid {
        document,
        message: message.into(),
    }
}

fn render_layers(output: &mut String, layers: Option<&Value>) {
    let Some(layers) = layers.and_then(Value::as_array) else {
        output.push_str("Path: unavailable in stored report\n");
        return;
    };
    output.push_str("Path:\n");
    for (index, layer) in layers.iter().enumerate() {
        let name = layer
            .get("layer")
            .and_then(Value::as_str)
            .unwrap_or("unknown layer");
        let status = layer
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("Unknown");
        let propagation = layer
            .get("propagation")
            .and_then(Value::as_str)
            .unwrap_or("Unknown");
        let marker = match (status, propagation) {
            ("Unavailable", _) => "!",
            ("NotHandled", "Continues") => "✓",
            ("Handled", "Stops") => "■",
            ("Handled", "Redirected") => "↗",
            ("Handled", "Continues") => "→",
            _ => "?",
        };
        let summary = layer
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("no summary");
        output.push_str(&format!("{}. {name}\n  {marker} {summary}\n", index + 1));
        let details = layer
            .get("details")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| {
                layer
                    .get("evidence")
                    .and_then(Value::as_array)
                    .map(|evidence| {
                        evidence
                            .iter()
                            .filter_map(|item| item.get("text").cloned())
                            .collect()
                    })
            });
        if let Some(details) = details.as_ref() {
            for detail in details.iter().filter_map(Value::as_str) {
                output.push_str(&format!("    {detail}\n"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn static_report() -> Value {
        serde_json::json!({
            "schema_version": 1,
            "key": {"modifiers": 4, "key": "X"},
            "key_display": "CTRL + X",
            "confidence": "configured",
            "layers": [{
                "layer": "Readline",
                "status": "Handled",
                "propagation": "Stops",
                "summary": "bound to forward-char",
                "details": []
            }]
        })
    }

    #[test]
    fn loads_multiple_pretty_printed_documents() {
        let file = tempfile_path();
        let mut handle = fs::File::create(&file).unwrap();
        writeln!(
            handle,
            "{}",
            serde_json::to_string_pretty(&static_report()).unwrap()
        )
        .unwrap();
        writeln!(
            handle,
            "{}",
            serde_json::to_string_pretty(&static_report()).unwrap()
        )
        .unwrap();

        let documents = load(&file).unwrap();
        assert_eq!(documents.len(), 2);
        let _ = fs::remove_file(file);
    }

    #[test]
    fn rejects_unknown_schema_versions() {
        let file = tempfile_path();
        fs::write(&file, r#"{"schema_version":3}"#).unwrap();
        let error = load(&file).unwrap_err().to_string();
        assert!(error.contains("only schema_version 1 or 2"));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn accepts_v2_reports_and_renders_structured_evidence() {
        let file = tempfile_path();
        fs::write(
            &file,
            r#"{"schema_version":2,"operation":"inspect","input":{"key_display":"CTRL + X"},"assessment":{"confidence":"conditional"},"path":[{"layer":"TTY","status":"Indeterminate","propagation":"Continues","summary":"state unknown","evidence":[{"kind":"detail","text":"termios unavailable"}]}]}"#,
        )
        .unwrap();
        let documents = load(&file).unwrap();
        let output = render_text(&documents);
        assert!(output.contains("CTRL + X"));
        assert!(output.contains("termios unavailable"));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn v1_upgrade_does_not_invent_renderer_context() {
        let upgraded = v2_document(&static_report());

        assert_eq!(upgraded["context"], serde_json::json!({"recorded": false}));
    }

    #[test]
    fn renders_stored_layer_evidence_without_injecting_input() {
        let output = render_text(&[static_report()]);
        assert!(output.contains("no input was injected"));
        assert!(output.contains("Readline"));
        assert!(output.contains("bound to forward-char"));
    }

    #[test]
    fn replays_a_diagnostic_snapshot_envelope_without_losing_metadata() {
        let file = tempfile_path();
        let snapshot = serde_json::json!({
            "schema_version": 1,
            "kind": "whykey.diagnostic_snapshot",
            "tool": {"name": "whykey", "version": "1.0.0"},
            "context": {},
            "request": {"kind": "key", "key_display": "CTRL + X"},
            "report": static_report(),
        });
        fs::write(&file, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();

        let documents = load(&file).unwrap();
        let text = render_text(&documents);
        assert!(text.contains("CTRL + X"));
        assert!(text.contains("Readline"));
        let json: Value = serde_json::from_str(&render_json(&documents, 2)).unwrap();
        assert_eq!(json["kind"], "whykey.diagnostic_snapshot");
        assert_eq!(json["report"]["key_display"], "CTRL + X");
        let _ = fs::remove_file(file);
    }

    fn tempfile_path() -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "whykey-replay-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
