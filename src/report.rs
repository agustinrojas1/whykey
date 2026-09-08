use crate::key::{KeyCombo, KeySequence};
use crate::layers::{LayerResult, LayerStatus, Propagation, format_bytes};
use crate::listen::ObservedKey;
use crate::schema;
use serde::Serialize;

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    key: &'a KeyCombo,
    key_display: String,
    confidence: &'static str,
    observed: Option<&'a ObservedKey>,
    layers: &'a [LayerResult],
}

#[derive(Serialize)]
struct JsonSequenceStep<'a> {
    index: usize,
    key: &'a KeyCombo,
    key_display: String,
    confidence: &'static str,
    layers: &'a [LayerResult],
}

#[derive(Serialize)]
struct JsonSequenceReport<'a> {
    schema_version: u8,
    sequence: &'a KeySequence,
    sequence_display: String,
    steps: Vec<JsonSequenceStep<'a>>,
}

#[derive(Serialize)]
struct NdjsonReport<'a> {
    schema_version: u8,
    operation: &'static str,
    context: serde_json::Value,
    input: NdjsonInput<'a>,
    assessment: NdjsonAssessment,
    observation: Option<&'a ObservedKey>,
    path: Vec<NdjsonPathEntry<'a>>,
}

#[derive(Serialize)]
struct NdjsonInput<'a> {
    kind: &'static str,
    key: &'a KeyCombo,
    key_display: String,
}

#[derive(Serialize)]
struct NdjsonAssessment {
    confidence: &'static str,
    terminal_reached: bool,
}

#[derive(Serialize)]
struct NdjsonPathEntry<'a> {
    layer: &'static str,
    status: LayerStatus,
    propagation: Propagation,
    summary: &'a str,
    evidence: Vec<NdjsonEvidence<'a>>,
}

#[derive(Serialize)]
struct NdjsonEvidence<'a> {
    kind: &'static str,
    text: &'a str,
}

pub fn render_json(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: Option<&ObservedKey>,
    schema_version: u8,
) -> String {
    render_json_for_operation(key, layers, observed, schema_version, "inspect")
}

/// Render a captured key report. Schema v2 records retain their operation so
/// consumers can distinguish a live observation from static inspection.
pub fn render_listen_json(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: Option<&ObservedKey>,
    schema_version: u8,
) -> String {
    render_json_for_operation(key, layers, observed, schema_version, "listen")
}

fn render_json_for_operation(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: Option<&ObservedKey>,
    schema_version: u8,
    operation: &'static str,
) -> String {
    if schema_version == 2 {
        return render_json_v2(key, layers, observed, operation);
    }
    serde_json::to_string_pretty(&JsonReport {
        schema_version: 1,
        key,
        key_display: key.to_string(),
        confidence: confidence_label(
            layers,
            observed.is_some_and(|observation| observation.source.confirms_terminal()),
        ),
        observed,
        layers,
    })
    .expect("whykey report types are serializable")
        + "\n"
}

/// Render one compact schema-v2 record for a capture stream.
///
/// Each call returns exactly one JSON object followed by a newline, making the
/// result safe to append to an NDJSON stream without mixing interface text
/// into stdout.
pub fn render_ndjson(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: Option<&ObservedKey>,
) -> String {
    serde_json::to_string(&NdjsonReport {
        schema_version: 2,
        operation: "listen",
        context: schema::context(),
        input: NdjsonInput {
            kind: "key",
            key,
            key_display: key.to_string(),
        },
        assessment: NdjsonAssessment {
            confidence: confidence_label(
                layers,
                observed.is_some_and(|observation| observation.source.confirms_terminal()),
            ),
            terminal_reached: observed
                .is_some_and(|observation| observation.source.confirms_terminal()),
        },
        observation: observed,
        path: layers
            .iter()
            .map(|layer| NdjsonPathEntry {
                layer: layer.layer,
                status: layer.status(),
                propagation: layer.propagation(),
                summary: &layer.summary,
                evidence: layer
                    .details
                    .iter()
                    .map(|detail| NdjsonEvidence {
                        kind: "detail",
                        text: detail,
                    })
                    .collect(),
            })
            .collect(),
    })
    .expect("whykey NDJSON report types are serializable")
        + "\n"
}

fn render_json_v2(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: Option<&ObservedKey>,
    operation: &'static str,
) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 2,
        "operation": operation,
        "context": schema::context(),
        "input": {
            "kind": "key",
            "key": key,
            "key_display": key.to_string(),
        },
        "assessment": {
            "confidence": confidence_label(
                layers,
                observed.is_some_and(|observation| observation.source.confirms_terminal()),
            ),
            "terminal_reached": observed.is_some_and(|observation| observation.source.confirms_terminal()),
        },
        "observation": observed,
        "path": schema::path(layers),
    }))
    .expect("whykey v2 report types are serializable")
        + "\n"
}

pub fn render(key: &KeyCombo, layers: &[LayerResult], verbose: bool) -> String {
    render_inner(key, layers, false, None, verbose)
}

pub fn render_observed(observed: &ObservedKey, layers: &[LayerResult], verbose: bool) -> String {
    render_inner(&observed.combo, layers, true, Some(observed), verbose)
}

/// Render an inspection where each step is analyzed independently.
pub fn render_sequence(
    sequence: &KeySequence,
    reports: &[Vec<LayerResult>],
    verbose: bool,
) -> String {
    let mut output = format!("Sequence: {sequence}\n\n");
    for (index, (key, layers)) in sequence.as_slice().iter().zip(reports).enumerate() {
        output.push_str(&format!("Step {}/{}: {key}\n", index + 1, sequence.len()));
        output.push_str(&render(key, layers, verbose));
        if index + 1 < sequence.len() {
            output.push('\n');
        }
    }
    output
}

pub fn render_sequence_json(
    sequence: &KeySequence,
    reports: &[Vec<LayerResult>],
    schema_version: u8,
) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "inspect",
            "context": schema::context(),
            "input": {
                "kind": "sequence",
                "sequence": sequence,
                "sequence_display": sequence.to_string(),
            },
            "steps": sequence
                .as_slice()
                .iter()
                .zip(reports)
                .enumerate()
                .map(|(index, (key, layers))| serde_json::json!({
                    "index": index + 1,
                    "key": key,
                    "key_display": key.to_string(),
                    "assessment": {
                        "confidence": confidence_label(layers, false),
                    },
                    "path": schema::path(layers),
                }))
                .collect::<Vec<_>>(),
        }))
        .expect("whykey v2 sequence report types are serializable")
            + "\n";
    }
    let steps = sequence
        .as_slice()
        .iter()
        .zip(reports)
        .enumerate()
        .map(|(index, (key, layers))| JsonSequenceStep {
            index: index + 1,
            key,
            key_display: key.to_string(),
            confidence: confidence_label(layers, false),
            layers,
        })
        .collect();

    serde_json::to_string_pretty(&JsonSequenceReport {
        schema_version: 1,
        sequence,
        sequence_display: sequence.to_string(),
        steps,
    })
    .expect("whykey sequence reports are serializable")
        + "\n"
}

fn render_inner(
    key: &KeyCombo,
    layers: &[LayerResult],
    observed: bool,
    observation: Option<&ObservedKey>,
    verbose: bool,
) -> String {
    let mut output = String::new();
    let terminal_observed = observation.is_some_and(|value| value.source.confirms_terminal());
    output.push_str(&format!(
        "Key: {key}\nAssessment: {}\n\n",
        confidence_label(layers, observed && terminal_observed,)
    ));
    if let Some(observation) = observation {
        let raw_display = observation
            .raw_display
            .clone()
            .unwrap_or_else(|| format_raw_bytes(&observation.raw));
        let raw_label = if observation.source.confirms_terminal() {
            "probe bytes"
        } else {
            "raw event"
        };
        let encoding_note = if observation.source.confirms_terminal() {
            match crate::layers::terminal_identity() {
                Some(name) => {
                    format!("  downstream analysis uses {name}'s normal encoding when available")
                }
                None => "  downstream analysis uses the terminal's normal encoding when available"
                    .to_owned(),
            }
        } else {
            "  physical capture does not prove compositor or terminal forwarding".to_owned()
        };
        output.push_str(&format!(
            "Capture\n  source: {}\n  observed key: {}\n  event: {}\n  encoding: {}\n  {raw_label}: {}\n{}\n",
            observation.source.label(),
            observation.combo,
            observation.event_type.label(),
            observation.encoding,
            raw_display,
            encoding_note
        ));
        if let Some(text) = &observation.associated_text {
            output.push_str(&format!("  associated text: {text}\n"));
        }
        if let Some(state) = &observation.modifier_state {
            let pressed = if state.pressed.is_empty() {
                "none".into()
            } else {
                state.pressed.join(", ")
            };
            let locked = if state.locked.is_empty() {
                "none".into()
            } else {
                state.locked.join(", ")
            };
            output.push_str(&format!(
                "  modifiers pressed: {pressed}\n  modifiers locked: {locked}\n"
            ));
            if state.latched.is_none() {
                output.push_str("  modifiers latched: unavailable from evdev\n");
            }
        }
        if let Some(alternate) = &observation.alternate_key {
            output.push_str(&format!("  alternate key: {alternate}\n\n"));
        }
    }
    output.push_str(&render_conclusion(
        key,
        layers,
        observation,
        terminal_observed,
    ));
    output.push_str("Path\n");

    // Default reports show only matching, consuming, unavailable, or
    // uncertain layers so the route fits one screen. `--verbose` restores
    // the full evidence view; JSON always keeps full structured evidence.
    let visible: Vec<_> = layers
        .iter()
        .filter(|layer| {
            verbose
                || layer.status() != LayerStatus::NotHandled
                || layer.propagation() != Propagation::Continues
        })
        .collect();
    if visible.is_empty() {
        output.push_str(
            "  No inspected layer claims this key; use --verbose for the full route.\n\n",
        );
    }
    for (index, layer) in visible.iter().enumerate() {
        output.push_str(&format!("{}. {}\n", index + 1, layer.layer));
        output.push_str("  ");
        output.push_str(match (layer.status(), layer.propagation()) {
            (LayerStatus::Unavailable, _) => "! ",
            (LayerStatus::NotHandled, Propagation::Continues) => "✓ ",
            (LayerStatus::Handled, Propagation::Continues) => "→ ",
            (LayerStatus::Handled, Propagation::Stops) => "■ ",
            (LayerStatus::Handled, Propagation::Redirected) => "↗ ",
            (LayerStatus::Indeterminate, _) | (_, Propagation::Indeterminate) => "? ",
            _ => "? ",
        });
        output.push_str(&layer.summary);
        output.push('\n');

        for detail in &layer.details {
            output.push_str("    ");
            output.push_str(detail);
            output.push('\n');
        }
        output.push('\n');
    }

    output
}

/// The conclusion answers the user's question first: which layer finally
/// handles the key, or why no layer can claim it.
fn render_conclusion(
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
) -> String {
    let mut output = String::from("Result:\n");
    if let Some(observation) = observation {
        if observation.source.confirms_terminal() {
            output.push_str(
                "  The captured event reached this terminal, so earlier forwarding is confirmed.\n",
            );
        } else {
            output.push_str(
                "  The physical key event was captured before the compositor; forwarding is not confirmed.\n",
            );
        }
    }
    match layers.last() {
        Some(layer) if layer.status() == LayerStatus::Unavailable => {
            output.push_str(&format!("  Could not inspect {}.\n", layer.layer))
        }
        Some(layer) if layer.propagation() == Propagation::Stops => {
            let qualifier = (!terminal_observed && prior_uncertainty(layers))
                .then_some("Assuming earlier uncertain layers forward it, ");
            output.push_str(&format!(
                "  ✓ Final handler: {}\n  {}{} handles and consumes {key}.\n  It should not reach a later layer.\n",
                layer.layer,
                qualifier.unwrap_or_default(),
                layer.layer
            ));
        }
        Some(layer) if layer.propagation() == Propagation::Redirected => {
            let qualifier = (!terminal_observed && prior_uncertainty(layers))
                .then_some("Assuming earlier uncertain layers forward it, ");
            output.push_str(&format!(
                "  ✓ Final handler: {}\n  {}{} handles {key} and redirects it to another window.\n  It should not reach a later layer in this chain.\n",
                layer.layer,
                qualifier.unwrap_or_default(),
                layer.layer
            ));
        }
        Some(layer)
            if layer.status() == LayerStatus::Indeterminate
                && !layers
                    .iter()
                    .any(|candidate| candidate.status() == LayerStatus::Handled) =>
        {
            output.push_str(&format!(
                "  No exact binding for {key} was found in {}.\n  Some same-key bindings may ignore modifiers, so forwarding cannot be proven.\n",
                layer.layer
            ));
        }
        Some(layer) if layer.propagation() == Propagation::Indeterminate => {
            output.push_str(&format!(
                "  Could not determine whether {} forwards {key}.\n",
                layer.layer
            ))
        }
        Some(_) if !terminal_observed && has_uncertain_layer(layers) => {
            let handlers: Vec<_> = layers
                .iter()
                .filter(|layer| layer.status() == LayerStatus::Handled)
                .map(|layer| layer.layer)
                .collect();
            if handlers.is_empty() {
                output.push_str(&format!(
                    "  {key} may be forwarded, but one or more layers are uncertain.\n  Downstream layer results are conditional.\n"
                ));
            } else {
                output.push_str(&format!(
                    "  {} handled {key}, but forwarding is uncertain. Other layers may also handle it.\n  Downstream layer results are conditional.\n",
                    handlers.join(", ")
                ));
            }
        }
        Some(_)
            if layers
                .iter()
                .any(|layer| layer.status() == LayerStatus::Handled) =>
        {
            let handlers: Vec<_> = layers
                .iter()
                .filter(|layer| layer.status() == LayerStatus::Handled)
                .map(|layer| layer.layer)
                .collect();
            output.push_str(&format!(
                "  {} handled {key} and forwarded it.\n  It should continue to the next layer.\n",
                handlers.join(", ")
            ));
        }
        Some(_) => output.push_str(&format!(
            "  No inspected layer handles {key}.\n  It should continue to the next layer.\n"
        )),
        None => output.push_str("  No layers were inspected.\n"),
    }
    output.push('\n');
    output
}

fn format_raw_bytes(bytes: &[u8]) -> String {
    format_bytes(bytes)
}

fn prior_uncertainty(layers: &[LayerResult]) -> bool {
    layers.iter().rev().skip(1).any(|layer| {
        layer.status() == LayerStatus::Indeterminate
            || layer.status() == LayerStatus::Unavailable
            || layer.propagation() == Propagation::Indeterminate
    })
}

fn has_uncertain_layer(layers: &[LayerResult]) -> bool {
    layers.iter().any(|layer| {
        layer.status() == LayerStatus::Indeterminate
            || layer.status() == LayerStatus::Unavailable
            || layer.propagation() == Propagation::Indeterminate
    })
}

fn confidence_label(layers: &[LayerResult], observed: bool) -> &'static str {
    if has_uncertain_layer(layers) {
        "conditional"
    } else if observed {
        "confirmed"
    } else {
        "configured"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{LayerId, Outcome};

    #[test]
    fn renders_a_forwarded_result() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layer = LayerResult {
            layer: "Hyprland",
            id: LayerId::Compositor,
            outcome: Outcome::Pass,
            summary: "no active binding found".into(),
            details: vec!["active submap: default".into()],
        };

        let output = render(&key, std::slice::from_ref(&layer), false);

        assert!(output.contains("Key: CTRL + LEFT"));
        assert!(output.contains("Assessment: configured"));
        assert!(output.contains("No inspected layer handles CTRL + LEFT."));
        assert!(output.contains("It should continue to the next layer."));
        assert!(output.contains("--verbose"));

        let verbose = render(&key, std::slice::from_ref(&layer), true);
        assert!(verbose.contains("✓ no active binding found"));
    }

    #[test]
    fn uses_the_layer_that_stops_the_key() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::HandledAndPassed,
                summary: "binding found".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Readline",
                id: LayerId::Shell,
                outcome: Outcome::Consumed,
                summary: "binding found".into(),
                details: vec![],
            },
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Readline handles and consumes CTRL + Z."));
        assert!(!output.contains("Hyprland handles and consumes"));
    }

    #[test]
    fn qualifies_a_later_handler_after_uncertainty() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: "forwarding uncertain".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Consumed,
                summary: "bound to backward-word".into(),
                details: vec![],
            },
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Assuming earlier uncertain layers forward it"));
        assert!(output.contains("Assessment: conditional"));
    }

    #[test]
    fn explains_a_redirected_key() {
        let key: KeyCombo = "ctrl+p".parse().unwrap();
        let layer = LayerResult {
            layer: "Hyprland",
            id: LayerId::Compositor,
            outcome: Outcome::Redirected,
            summary: "active binding found; event is redirected to another window".into(),
            details: vec!["binding: pass class:example".into()],
        };

        let output = render(&key, &[layer], false);

        assert!(output.contains("↗ active binding found"));
        assert!(output.contains("redirects it to another window"));
    }

    #[test]
    fn keeps_handlers_from_earlier_forwarding_layers() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::HandledAndPassed,
                summary: "binding found".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::Pass,
                summary: "no binding found".into(),
                details: vec![],
            },
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Hyprland handled CTRL + LEFT and forwarded it."));
        assert!(!output.contains("No inspected layer handles"));
    }

    #[test]
    fn keeps_confirmed_and_possible_handlers() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::UncertainContinues,
                summary: "binding may run".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::HandledAndPassed,
                summary: "binding found".into(),
                details: vec![],
            },
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Ghostty handled CTRL + LEFT, but forwarding is uncertain."));
        assert!(output.contains("Other layers may also handle it."));
    }

    #[test]
    fn renders_unavailable_before_indeterminate_propagation() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layer = LayerResult {
            layer: "Hyprland",
            id: LayerId::Compositor,
            outcome: Outcome::Unavailable,
            summary: "inspection failed".into(),
            details: vec![],
        };

        let output = render(&key, &[layer], false);

        assert!(output.contains("! inspection failed"));
        assert!(output.contains("Could not inspect Hyprland."));
    }

    #[test]
    fn keeps_unavailable_context_visible_when_nothing_handles() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: "inspection failed".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Readline",
                id: LayerId::Shell,
                outcome: Outcome::Pass,
                summary: "no binding".into(),
                details: vec![],
            },
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("may be forwarded, but one or more layers are uncertain"));
    }

    #[test]
    fn renders_a_versioned_json_report() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let output = render_json(&key, &[], None, 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["key"]["key"], "F");
        assert_eq!(value["confidence"], "configured");
    }

    #[test]
    fn renders_one_compact_ndjson_listen_record() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let output = render_ndjson(&key, &[], None);

        assert!(output.ends_with('\n'));
        assert!(!output.trim_end().contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["operation"], "listen");
        assert_eq!(value["input"]["key_display"], "CTRL + F");
        assert!(value["context"].is_object());
    }

    #[test]
    fn observed_input_removes_the_forwarding_qualifier() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: b"\x1b[1;5D".to_vec(),
            raw_display: None,
            modifier_state: None,
            associated_text: None,
            physical_keycode: None,
            encoding: "CSI sequence".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: None,
            source: crate::listen::CaptureSource::Terminal,
        };
        let layers = [
            LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: "forwarding uncertain".into(),
                details: vec![],
            },
            LayerResult {
                layer: "Readline",
                id: LayerId::Shell,
                outcome: Outcome::Consumed,
                summary: "bound to backward-word".into(),
                details: vec![],
            },
        ];

        let output = render_observed(&observed, &layers, false);

        assert!(output.contains("earlier forwarding is confirmed"));
        assert!(output.contains("probe bytes: ESC [ 1 ; 5 D"));
        assert!(output.contains("Readline handles and consumes CTRL + LEFT."));
        assert!(!output.contains("Assuming earlier uncertain layers"));
    }

    #[test]
    fn evdev_report_does_not_claim_terminal_forwarding() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: Some("EV_KEY code=33 value=1 (press)".into()),
            modifier_state: None,
            associated_text: None,
            physical_keycode: Some(33),
            encoding: "evdev key event before compositor processing".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: Some("physical keycode 33 (F)".into()),
            source: crate::listen::CaptureSource::Evdev {
                device: "Test Keyboard".into(),
                path: "/dev/input/event0".into(),
            },
        };
        let layers = [LayerResult {
            layer: "Hyprland",
            id: LayerId::Compositor,
            outcome: Outcome::Pass,
            summary: "no active binding found".into(),
            details: vec![],
        }];

        let output = render_observed(&observed, &layers, false);
        assert!(output.contains("source: evdev (Test Keyboard; /dev/input/event0)"));
        assert!(output.contains("physical key event was captured before the compositor"));
        assert!(!output.contains("earlier forwarding is confirmed"));
        assert!(!output.contains("probe bytes:"));
    }

    #[test]
    fn renders_each_sequence_step_in_text() {
        let sequence: KeySequence = "ctrl+x ctrl+s".parse().unwrap();
        let reports = vec![
            vec![LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::Pass,
                summary: "no active binding found".into(),
                details: vec![],
            }],
            vec![LayerResult {
                layer: "Hyprland",
                id: LayerId::Compositor,
                outcome: Outcome::Consumed,
                summary: "binding found".into(),
                details: vec![],
            }],
        ];

        let output = render_sequence(&sequence, &reports, false);

        assert!(output.contains("Sequence: CTRL+X CTRL+S"));
        assert!(output.contains("Step 1/2: CTRL + X"));
        assert!(output.contains("Step 2/2: CTRL + S"));
    }

    #[test]
    fn renders_a_versioned_json_sequence_report() {
        let sequence: KeySequence = "ctrl+x ctrl+s".parse().unwrap();
        let reports = vec![Vec::new(), Vec::new()];

        let output = render_sequence_json(&sequence, &reports, 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["sequence_display"], "CTRL+X CTRL+S");
        assert_eq!(value["steps"].as_array().unwrap().len(), 2);
        assert_eq!(value["steps"][0]["key"]["key"], "X");
    }
}
