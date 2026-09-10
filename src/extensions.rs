//! Opt-in protocol for application-specific diagnostics.
//!
//! An extension is an executable selected explicitly by the user. It receives
//! one JSON request on stdin and returns one JSON response on stdout. The
//! protocol is deliberately small: extensions can contribute a single layer
//! of evidence without gaining permission to inject input or modify config.

use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerResult, LayerStatus, Outcome, Propagation};
use crate::schema;

pub const SCHEMA_VERSION: u8 = 1;
const MAX_REQUEST_BYTES: usize = 64 * 1024;

const SUPPORTED_CAPABILITIES: &[&str] = &[
    "list_bindings",
    "query_mode",
    "identify_action",
    "observe_receipt",
];

#[derive(Debug, Serialize)]
struct Request<'a> {
    schema_version: u8,
    operation: &'static str,
    key: &'a KeyCombo,
    key_display: String,
    capabilities_requested: &'static [&'static str],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Response {
    pub schema_version: u8,
    pub capabilities: Vec<String>,
    pub result: ExtensionResult,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionResult {
    pub status: String,
    pub propagation: String,
    pub summary: String,
    #[serde(default)]
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub schema_version: u8,
    pub extension: String,
    pub capabilities: Vec<String>,
    pub layer: LayerResult,
}

pub fn inspect(program: &str, key: &KeyCombo) -> Result<RunResult, String> {
    let request = Request {
        schema_version: SCHEMA_VERSION,
        operation: "inspect",
        key,
        key_display: key.to_string(),
        capabilities_requested: SUPPORTED_CAPABILITIES,
    };
    let request =
        serde_json::to_vec(&request).map_err(|error| format!("encode request: {error}"))?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "extension request exceeds the {MAX_REQUEST_BYTES}-byte limit"
        ));
    }

    let output = command::output_with_input(&mut Command::new(program), &request)
        .map_err(|error| format!("extension {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("extension exited with {}", output.status)
        } else {
            format!("extension exited with {}: {stderr}", output.status)
        });
    }
    let response: Response = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("extension returned invalid JSON: {error}"))?;
    validate(&response)?;
    let layer = to_layer(program, response.result);
    Ok(RunResult {
        schema_version: response.schema_version,
        extension: program.to_owned(),
        capabilities: response.capabilities,
        layer,
    })
}

pub fn render_text(result: &RunResult, key: &KeyCombo) -> String {
    let mut output = format!("Key: {key}\n\nExternal extension\n");
    output.push_str(&format!("  program: {}\n", result.extension));
    output.push_str(&format!(
        "  capabilities: {}\n",
        if result.capabilities.is_empty() {
            "none".into()
        } else {
            result.capabilities.join(", ")
        }
    ));
    output.push_str(&format!("  status: {:?}\n", result.layer.status()));
    output.push_str(&format!("  {}\n", result.layer.summary));
    for detail in &result.layer.details {
        output.push_str(&format!("    {detail}\n"));
    }
    output
}

pub fn render_json(result: &RunResult, key: &KeyCombo, schema_version: u8) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "extension",
            "context": schema::context(),
            "input": {"key": key, "key_display": key.to_string()},
            "extension": result,
        }))
        .expect("extension v2 result is serializable")
            + "\n";
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "key": key,
        "key_display": key.to_string(),
        "extension": result,
    }))
    .expect("extension result is serializable")
        + "\n"
}

fn validate(response: &Response) -> Result<(), String> {
    if response.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported extension schema version {}; expected {SCHEMA_VERSION}",
            response.schema_version
        ));
    }
    for capability in &response.capabilities {
        if !SUPPORTED_CAPABILITIES.contains(&capability.as_str()) {
            return Err(format!(
                "extension returned unknown capability '{capability}'"
            ));
        }
    }
    // One total external-process output limit (enforced by the command
    // runner on the extension's stdout) covers oversized responses; no
    // extension-specific capability, detail-count, or detail-size limits.
    if !matches!(
        response.result.status.as_str(),
        "handled" | "not_handled" | "indeterminate" | "unavailable"
    ) {
        return Err(format!(
            "extension returned unsupported status '{}'; expected handled, not_handled, indeterminate, or unavailable",
            response.result.status
        ));
    }
    if !matches!(
        response.result.propagation.as_str(),
        "continues" | "stops" | "redirected" | "indeterminate"
    ) {
        return Err(format!(
            "extension returned unsupported propagation '{}'; expected continues, stops, redirected, or indeterminate",
            response.result.propagation
        ));
    }
    Ok(())
}

fn to_layer(program: &str, result: ExtensionResult) -> LayerResult {
    let status = match result.status.as_str() {
        "handled" => LayerStatus::Handled,
        "not_handled" => LayerStatus::NotHandled,
        "unavailable" => LayerStatus::Unavailable,
        _ => LayerStatus::Indeterminate,
    };
    let propagation = match result.propagation.as_str() {
        "continues" => Propagation::Continues,
        "stops" => Propagation::Stops,
        "redirected" => Propagation::Redirected,
        _ => Propagation::Indeterminate,
    };
    let mut details = vec![format!("extension: {program}")];
    details.extend(result.details);
    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "External extension",
        id: crate::layers::LayerId::Diagnostic,
        outcome: Outcome::from_parts(&status, &propagation),
        summary: result.summary,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: &str, propagation: &str) -> Response {
        Response {
            schema_version: SCHEMA_VERSION,
            capabilities: vec!["identify_action".into()],
            result: ExtensionResult {
                status: status.into(),
                propagation: propagation.into(),
                summary: "test result".into(),
                details: vec!["evidence".into()],
            },
        }
    }

    #[test]
    fn validates_the_versioned_response_contract() {
        validate(&response("handled", "stops")).unwrap();
        assert!(validate(&response("unknown", "stops")).is_err());
        assert!(validate(&response("handled", "unknown")).is_err());
    }

    #[test]
    fn maps_an_extension_result_to_a_conditional_layer() {
        let layer = to_layer(
            "my-editor-whykey",
            response("indeterminate", "indeterminate").result,
        );
        assert_eq!(layer.layer, "External extension");
        assert_eq!(layer.status(), LayerStatus::Indeterminate);
        assert!(layer.details[0].contains("my-editor-whykey"));
    }
}
