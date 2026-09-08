//! Output-schema negotiation shared by the CLI renderers.
//!
//! Schema v1 remains the default for backwards compatibility. The CLI
//! selects v2 per command with `--schema-version 2`; the version travels
//! through command options and request context, never global mutable state.

use std::env;

use serde_json::{Value, json};

use crate::layers::LayerResult;

/// Default schema unless a command option selects otherwise.
pub const DEFAULT_VERSION: u8 = 1;

pub fn context() -> Value {
    json!({
        "desktop": env::var("XDG_CURRENT_DESKTOP").ok(),
        "session_desktop": env::var("XDG_SESSION_DESKTOP").ok(),
        "session_type": env::var("XDG_SESSION_TYPE").ok(),
        "display_server": if env::var_os("WAYLAND_DISPLAY").is_some() {
            Some("Wayland")
        } else if env::var_os("DISPLAY").is_some() {
            Some("X11")
        } else {
            None
        },
        "terminal": crate::layers::terminal_identity(),
        "shell": env::var("SHELL").ok(),
        "ssh": env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
    })
}

pub fn path(layers: &[LayerResult]) -> Vec<Value> {
    layers
        .iter()
        .map(|layer| {
            let (status, propagation) = (layer.status(), layer.propagation());
            json!({
                "layer": layer.layer,
                "status": status,
                "propagation": propagation,
                "summary": layer.summary,
                "evidence": layer.details.iter().map(|detail| json!({
                    "kind": "detail",
                    "text": detail,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_legacy_schema() {
        assert_eq!(DEFAULT_VERSION, 1);
    }

    #[test]
    fn evidence_preserves_layer_details() {
        let layer = LayerResult {
            layer: "test",
            id: crate::layers::LayerId::Diagnostic,
            outcome: crate::layers::Outcome::Consumed,
            summary: "handled".into(),
            details: vec!["source: fixture".into()],
        };
        let value = path(&[layer]);
        assert_eq!(value[0]["evidence"][0]["kind"], "detail");
        assert_eq!(value[0]["evidence"][0]["text"], "source: fixture");
    }
}
