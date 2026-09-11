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
    let environment = crate::environment::Environment::collect();
    context_for_environment(&environment)
}

pub fn context_for_environment(environment: &crate::environment::Environment) -> Value {
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
        "compositor_candidates": environment.compositor_candidates.iter().map(|candidate| json!({
            "id": candidate.id,
            "applicable": candidate.applicable,
            "ipc": candidate.ipc,
            "score": candidate.score,
            "selected": environment.selected_compositor == Some(candidate.id),
        })).collect::<Vec<_>>(),
        "extension_adapters": environment.extension_adapters.iter().map(|adapter| {
            let applicable = crate::extension_adapters::applicable_with_context(
                adapter,
                &environment.compositor_context,
            );
            json!({
                "id": adapter.id,
                "display": adapter.display,
                "manifest": adapter.manifest_path,
                "tier": adapter.tier,
                "applicable": applicable,
                "selected": environment.selected_extension.as_deref() == Some(adapter.id.as_str()),
            })
        }).collect::<Vec<_>>(),
    })
}

/// Schema v2 path entries. Schema v1 serializes `LayerResult` directly and
/// remains schema-compatible, preserving its fields and complete evidence;
/// only v2 gains the optional `binding` object.
/// Replaying a report without one never invents typed evidence.
pub fn path(layers: &[LayerResult]) -> Vec<Value> {
    layers
        .iter()
        .map(|layer| {
            let (status, propagation) = (layer.status(), layer.propagation());
            let mut entry = json!({
                "layer": layer.layer,
                "status": status,
                "propagation": propagation,
                "summary": layer.summary,
                "evidence": layer.all_details().map(|detail| json!({
                    "kind": "detail",
                    "text": detail,
                })).collect::<Vec<_>>(),
            });
            if let Some(binding) = &layer.binding {
                entry["binding"] = json!(binding);
            }
            entry
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
    fn v2_context_exposes_scored_compositor_candidates() {
        let environment = crate::environment::Environment::collect();
        let value = context_for_environment(&environment);
        assert!(value["compositor_candidates"].is_array());
        if let Some(selected) = environment.selected_compositor {
            assert!(
                value["compositor_candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|candidate| candidate["id"] == selected && candidate["selected"] == true)
            );
        }
    }

    #[test]
    fn evidence_preserves_layer_details() {
        let layer = LayerResult::new(
            "test",
            crate::layers::LayerId::Diagnostic,
            crate::layers::Outcome::Consumed,
            "handled",
            vec!["source: fixture".into()],
        );
        let value = path(&[layer]);
        assert_eq!(value[0]["evidence"][0]["kind"], "detail");
        assert_eq!(value[0]["evidence"][0]["text"], "source: fixture");
    }

    #[test]
    fn path_omits_binding_without_evidence_and_carries_it_with() {
        let plain = LayerResult::new(
            "test",
            crate::layers::LayerId::Diagnostic,
            crate::layers::Outcome::Consumed,
            "handled",
            vec!["source: fixture".into()],
        );
        let value = path(&[plain]);
        assert!(value[0].get("binding").is_none());
        let evidenced = LayerResult::new(
            "Hyprland",
            crate::layers::LayerId::Compositor,
            crate::layers::Outcome::HandledUncertain,
            "matching binding; runtime effect unknown",
            vec!["opaque runtime hook".into()],
        )
        .with_binding(crate::layers::BindingEvidence {
            dispatcher: Some("__lua".into()),
            action: Some("__lua 285".into()),
            description: Some("Herdr".into()),
            submap: Some("default".into()),
            scope: crate::layers::BindingScope::Universal,
            source: None,
            has_universal_match: true,
            uncertainty: None,
        });
        let value = path(&[evidenced]);
        assert_eq!(value[0]["binding"]["dispatcher"], "__lua");
        assert_eq!(value[0]["binding"]["scope"], "Universal");
    }
}
