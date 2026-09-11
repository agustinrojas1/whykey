use crate::key::{KeyCombo, KeySequence};
use crate::layers::{
    BindingEvidence, LayerId, LayerResult, LayerStatus, Outcome, Propagation, UncertaintyReason,
    format_bytes,
};
use crate::listen::ObservedKey;
use crate::schema;
use serde::Serialize;

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    key: &'a KeyCombo,
    key_display: String,
    confidence: &'static str,
    observed: Option<serde_json::Value>,
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
    // The value indirection exists only to add source_label alongside the
    // canonical source object without changing ObservedKey's public fields.
    observation: Option<serde_json::Value>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    binding: Option<&'a BindingEvidence>,
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
    let observed_v1 = observed.and_then(observation_value_v1).map(|mut val| {
        if let serde_json::Value::Object(map) = &mut val {
            map.remove("disposition");
        }
        val
    });
    serde_json::to_string_pretty(&JsonReport {
        schema_version: 1,
        key,
        key_display: key.to_string(),
        confidence: confidence_label(
            layers,
            observed.is_some_and(|observation| observation.source.confirms_terminal()),
        ),
        observed: observed_v1,
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
        observation: observed.and_then(observation_value_v2),
        path: layers
            .iter()
            .map(|layer| NdjsonPathEntry {
                layer: layer.layer,
                status: layer.status(),
                propagation: layer.propagation(),
                summary: &layer.summary,
                binding: layer.binding.as_ref(),
                evidence: layer
                    .all_details()
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
        "observation": observed.and_then(observation_value_v2),
        "path": schema::path(layers),
    }))
    .expect("whykey v2 report types are serializable")
        + "\n"
}

/// Schema v1 keeps the historical Hyprland source string. This shaping is
/// explicit because the in-memory native source has one canonical v2 form.
fn observation_value_v1(observed: &ObservedKey) -> Option<serde_json::Value> {
    let mut value = serde_json::to_value(observed).ok()?;
    if let crate::listen::CaptureSource::CompositorNative { backend } = &observed.source {
        if backend == "Hyprland" {
            if let Some(object) = value.as_object_mut() {
                object.insert("source".into(), serde_json::json!("Hyprland"));
            }
        }
    }
    Some(value)
}

/// Schema v2 carries the canonical native source object and adds the stable
/// human-readable label used by renderers and consumers.
fn observation_value_v2(observed: &ObservedKey) -> Option<serde_json::Value> {
    let mut value = serde_json::to_value(observed).ok()?;
    if let crate::listen::CaptureSource::CompositorNative { .. } = &observed.source {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "source_label".into(),
                serde_json::json!(observed.source.label()),
            );
        }
    }
    Some(value)
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
    output.push_str(&render_conclusion(
        key,
        layers,
        observation,
        terminal_observed,
    ));
    if verbose {
        if let Some(note) = compositor_candidates_note() {
            output.push_str(&note);
        }
    }
    if let Some(observation) = observation {
        // Normal reports name the capture source, key, and event. Raw bytes,
        // encoding internals, full modifier state, and alternate keys stay
        // behind `--verbose`; JSON always carries everything.
        output.push_str(&format!(
            "Capture\n  source: {}\n  observed key: {}\n  event: {}\n",
            observation.source.label(),
            observation.combo,
            observation.event_type.label(),
        ));
        // The conclusion already states suppression once; repeating it here
        // printed "captured and suppressed" twice.
        match observation.disposition {
            crate::listen::CaptureDisposition::Suppressed => {}
            crate::listen::CaptureDisposition::PassedThrough
            | crate::listen::CaptureDisposition::ObservedOnly => {
                let note = if observation.source.proves_compositor_receipt() {
                    format!(
                        "  compositor capture proves the key reached {}, but does not prove forwarding",
                        observation
                            .source
                            .backend_name()
                            .unwrap_or("the compositor")
                    )
                } else {
                    match &observation.source {
                        crate::listen::CaptureSource::Terminal => {
                            match crate::layers::terminal_identity() {
                                Some(name) => {
                                    format!("  downstream analysis uses {name}'s normal encoding when available")
                                }
                                None => "  downstream analysis uses the terminal's normal encoding when available"
                                    .to_owned(),
                            }
                        }
                        crate::listen::CaptureSource::Evdev { .. } => {
                            "  physical capture does not prove compositor or terminal forwarding".to_owned()
                        }
                        _ => unreachable!("capture source semantics are inconsistent"),
                    }
                };
                output.push_str(&note);
                output.push('\n');
            }
        }
        if verbose {
            let raw_display = observation
                .raw_display
                .clone()
                .unwrap_or_else(|| format_bytes(&observation.raw));
            let raw_label = if observation.source.confirms_terminal() {
                "probe bytes"
            } else {
                "raw event"
            };
            output.push_str(&format!(
                "  {raw_label}: {raw_display}\n  encoding: {}\n",
                observation.encoding,
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
                    let from = if observation.source.proves_compositor_receipt() {
                        "compositor"
                    } else {
                        match &observation.source {
                            crate::listen::CaptureSource::Terminal => "terminal",
                            crate::listen::CaptureSource::Evdev { .. } => "evdev",
                            _ => unreachable!("capture source semantics are inconsistent"),
                        }
                    };
                    output.push_str(&format!("  modifiers latched: unavailable from {from}\n"));
                }
            }
            if let Some(alternate) = &observation.alternate_key {
                output.push_str(&format!("  alternate key: {alternate}\n"));
            }
        }
        output.push('\n');
    }
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
        if verbose {
            for detail in &layer.verbose_details {
                output.push_str("    ");
                output.push_str(detail);
                output.push('\n');
            }
        }
        output.push('\n');
    }

    output
}

fn compositor_candidates_note() -> Option<String> {
    let environment = crate::environment::Environment::collect();
    let mut candidates = environment
        .compositor_candidates
        .iter()
        .map(|candidate| format!("{}[{}]", candidate.id, candidate.score))
        .collect::<Vec<_>>();
    candidates.extend(
        environment
            .extension_candidates
            .iter()
            .map(|candidate| format!("{}[{}]", candidate.id, candidate.score)),
    );
    if candidates.len() < 2 {
        return None;
    }
    let selected = environment
        .selected_compositor
        .map(str::to_owned)
        .or_else(|| environment.selected_extension.clone())
        .unwrap_or_else(|| "none".into());
    Some(format!(
        "Note: {} compositor candidates applicable ({}); selected {selected}. Use --instance/--terminal to choose explicitly.\n\n",
        candidates.len(),
        candidates.join(", ")
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conclusion {
    SuppressedHandled {
        handled: SuppressedHandledOutcome,
    },
    ConfiguredConsumer {
        layer: &'static str,
        assumes_earlier_forward: bool,
    },
    ConfiguredRedirect {
        layer: &'static str,
        assumes_earlier_forward: bool,
    },
    SelectedApplication {
        layer: &'static str,
        status: ApplicationStatus,
    },
    UnverifiedSession {
        layer: &'static str,
    },
    HandledAndForwarded {
        layers: Vec<&'static str>,
        uncertain: bool,
        uninspected_layer: Option<&'static str>,
    },
    CouldNotInspect {
        layer: &'static str,
    },
    ModifierAmbiguity {
        layer: &'static str,
    },
    IndeterminateForwarding {
        layer: &'static str,
    },
    ConditionalForwarding,
    NoLayersInspected,
    UnhandledContinues,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationStatus {
    Unavailable,
    UnresolvedMode,
    UnverifiedExecution,
    InteractiveGeneric(String),
    NoMatchingKeymap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuppressedHandledOutcome {
    IndeterminatePropagation {
        action: Option<String>,
    },
    Universal {
        layer: &'static str,
        action: Option<String>,
    },
    Opaque {
        layer: &'static str,
        action: Option<String>,
    },
    Standard {
        layer: &'static str,
        action: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturePreamble {
    Suppressed { universal_match: bool },
    TerminalObserved,
    CompositorObserved { backend: String },
    EvdevObserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedConclusion {
    pub preamble: Option<CapturePreamble>,
    pub conclusion: Conclusion,
}

/// Pure conclusion evaluation converting layer findings and capture context
/// into a typed conclusion before rendering.
pub fn evaluate_conclusion(
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
) -> EvaluatedConclusion {
    let mut preamble = None;

    if let Some(observation) = observation {
        match observation.disposition {
            crate::listen::CaptureDisposition::Suppressed => {
                let universal_match = layers.iter().any(|l| {
                    l.binding
                        .as_ref()
                        .is_some_and(BindingEvidence::includes_universal_match)
                });
                if let Some(layer) = layers.iter().find(|l| l.status() == LayerStatus::Handled) {
                    let evidence = layer.binding.as_ref();
                    let action = evidence.and_then(|binding| {
                        binding
                            .description
                            .clone()
                            .or_else(|| binding.action.clone())
                    });
                    let opaque = evidence.is_some_and(BindingEvidence::is_opaque);
                    let handled = if layer.propagation() == Propagation::Indeterminate {
                        SuppressedHandledOutcome::IndeterminatePropagation { action }
                    } else if universal_match {
                        SuppressedHandledOutcome::Universal {
                            layer: layer.layer,
                            action,
                        }
                    } else if opaque {
                        SuppressedHandledOutcome::Opaque {
                            layer: layer.layer,
                            action,
                        }
                    } else {
                        SuppressedHandledOutcome::Standard {
                            layer: layer.layer,
                            action,
                        }
                    };
                    return EvaluatedConclusion {
                        preamble: Some(CapturePreamble::Suppressed { universal_match }),
                        conclusion: Conclusion::SuppressedHandled { handled },
                    };
                } else {
                    preamble = Some(CapturePreamble::Suppressed { universal_match });
                }
            }
            crate::listen::CaptureDisposition::PassedThrough => {
                preamble = Some(CapturePreamble::CompositorObserved {
                    backend: observation
                        .source
                        .backend_name()
                        .unwrap_or("compositor")
                        .to_owned(),
                });
            }
            crate::listen::CaptureDisposition::ObservedOnly => {
                preamble = if observation.source.confirms_terminal() {
                    Some(CapturePreamble::TerminalObserved)
                } else if observation.source.proves_compositor_receipt() {
                    Some(CapturePreamble::CompositorObserved {
                        backend: observation
                            .source
                            .backend_name()
                            .unwrap_or("compositor")
                            .to_owned(),
                    })
                } else {
                    Some(CapturePreamble::EvdevObserved)
                };
            }
        }
    }

    let conclusion = evaluate_layer_conclusion(layers, terminal_observed);
    EvaluatedConclusion {
        preamble,
        conclusion,
    }
}

fn evaluate_layer_conclusion(layers: &[LayerResult], terminal_observed: bool) -> Conclusion {
    if let Some(layer) = layers
        .iter()
        .find(|l| l.propagation() == Propagation::Stops)
    {
        return Conclusion::ConfiguredConsumer {
            layer: layer.layer,
            assumes_earlier_forward: !terminal_observed && prior_uncertainty(layers),
        };
    }
    if let Some(layer) = layers
        .iter()
        .find(|l| l.propagation() == Propagation::Redirected)
    {
        return Conclusion::ConfiguredRedirect {
            layer: layer.layer,
            assumes_earlier_forward: !terminal_observed && prior_uncertainty(layers),
        };
    }

    let active_app = layers
        .last()
        .filter(|l| l.id == LayerId::Application && l.outcome != Outcome::Pass);
    if let Some(app) = active_app {
        let status = if app.outcome == Outcome::Unavailable {
            ApplicationStatus::Unavailable
        } else if let Some(binding) = &app.binding {
            if binding.uncertainty == Some(UncertaintyReason::UnresolvedMode) {
                ApplicationStatus::UnresolvedMode
            } else {
                ApplicationStatus::UnverifiedExecution
            }
        } else if app.outcome == Outcome::UnadaptedTarget {
            ApplicationStatus::InteractiveGeneric(app.summary.clone())
        } else {
            ApplicationStatus::NoMatchingKeymap
        };
        return Conclusion::SelectedApplication {
            layer: app.layer,
            status,
        };
    }

    let unverified_session = layers.iter().find(|l| {
        l.id == LayerId::Multiplexer
            && l.outcome == Outcome::HandledUncertain
            && l.binding.as_ref().and_then(|b| b.uncertainty)
                == Some(UncertaintyReason::UnverifiedTerminalBytes)
    });
    if let Some(session) = unverified_session {
        return Conclusion::UnverifiedSession {
            layer: session.layer,
        };
    }

    let handled_layers: Vec<_> = layers
        .iter()
        .filter(|layer| layer.status() == LayerStatus::Handled)
        .collect();
    if !handled_layers.is_empty() {
        let names = handled_layers.iter().map(|l| l.layer).collect();
        let uncertain = !terminal_observed && has_uncertain_layer(layers);
        let uninspected_layer = layers
            .iter()
            .find(|l| l.status() == LayerStatus::Unavailable)
            .map(|l| l.layer);
        return Conclusion::HandledAndForwarded {
            layers: names,
            uncertain,
            uninspected_layer,
        };
    }

    if layers
        .iter()
        .all(|l| l.status() == LayerStatus::Unavailable)
    {
        return Conclusion::CouldNotInspect {
            layer: layers.last().unwrap().layer,
        };
    }

    if let Some(layer) = layers.last().filter(|l| {
        l.status() == LayerStatus::Unavailable
            && (l.id == LayerId::Application || l.id == LayerId::Diagnostic)
    }) {
        return Conclusion::CouldNotInspect { layer: layer.layer };
    }

    if let Some(layer) = layers.iter().find(|l| {
        l.status() == LayerStatus::Indeterminate
            && l.binding.as_ref().and_then(|b| b.uncertainty)
                == Some(UncertaintyReason::ModifierAmbiguity)
    }) {
        return Conclusion::ModifierAmbiguity { layer: layer.layer };
    }
    if let Some(layer) = layers.iter().find(|l| {
        l.status() == LayerStatus::Indeterminate && l.propagation() == Propagation::Indeterminate
    }) {
        return Conclusion::IndeterminateForwarding { layer: layer.layer };
    }

    if !terminal_observed && has_uncertain_layer(layers) {
        Conclusion::ConditionalForwarding
    } else if layers.is_empty() {
        Conclusion::NoLayersInspected
    } else {
        Conclusion::UnhandledContinues
    }
}

/// The conclusion answers the user's question first: which layer finally
/// handles the key, or why no layer can claim it.
fn render_conclusion(
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
) -> String {
    let evaluated = evaluate_conclusion(layers, observation, terminal_observed);
    let mut output = String::from("Result:\n");
    if let Some(preamble) = &evaluated.preamble {
        match preamble {
            CapturePreamble::Suppressed {
                universal_match: true,
            } => {
                output.push_str(
                    "  Whykey captured this event, but matching universal Hyprland bindings bypass submap capture and may execute.\n",
                );
            }
            CapturePreamble::Suppressed {
                universal_match: false,
            } => {
                output.push_str("  Whykey captured and suppressed this event.\n");
            }
            CapturePreamble::CompositorObserved { backend } => {
                output.push_str(&format!(
                    "  The key event was captured by {backend}, but forwarding is not confirmed.\n"
                ));
            }
            CapturePreamble::TerminalObserved => {
                output.push_str(
                    "  The captured event reached this terminal, so earlier forwarding is confirmed.\n",
                );
            }
            CapturePreamble::EvdevObserved => {
                output.push_str(
                    "  The physical key event was captured before the compositor; forwarding is not confirmed.\n",
                );
            }
        }
    }

    match &evaluated.conclusion {
        Conclusion::SuppressedHandled { handled } => match handled {
            SuppressedHandledOutcome::IndeterminatePropagation { action } => {
                output.push_str("  A matching Hyprland binding was found, but its runtime effect and propagation could not be determined.\n");
                if let Some(action) = action {
                    output.push_str(&format!(
                            "  The configuration describes the action as {action}; Whykey did not execute the dispatcher.\n"
                        ));
                }
            }
            SuppressedHandledOutcome::Universal {
                layer,
                action: Some(action),
            } => {
                output.push_str(&format!(
                    "  The normal configuration indicates that {layer} may execute {action}.\n"
                ));
            }
            SuppressedHandledOutcome::Universal {
                layer,
                action: None,
            } => {
                output.push_str(&format!(
                        "  The normal configuration indicates that {layer} universal binding may handle {key}.\n"
                    ));
            }
            SuppressedHandledOutcome::Opaque {
                layer,
                action: Some(action),
            } => {
                output.push_str(&format!(
                        "  The normal configuration indicates that {layer} may execute {action}; Whykey did not execute the dispatcher.\n"
                    ));
            }
            SuppressedHandledOutcome::Opaque {
                layer,
                action: None,
            } => {
                output.push_str(&format!(
                        "  The normal configuration indicates that {layer} may handle {key}; Whykey did not execute the dispatcher.\n"
                    ));
            }
            SuppressedHandledOutcome::Standard {
                layer,
                action: Some(action),
            } => {
                output.push_str(&format!(
                    "  The normal configuration indicates that {layer} would run {action}.\n"
                ));
            }
            SuppressedHandledOutcome::Standard {
                layer,
                action: None,
            } => {
                output.push_str(&format!(
                        "  The normal configuration indicates that {layer} would handle and consume {key}.\n"
                    ));
            }
        },
        Conclusion::ConfiguredConsumer {
            layer,
            assumes_earlier_forward,
        } => {
            let qualifier = if *assumes_earlier_forward {
                "Assuming earlier uncertain layers forward it, "
            } else {
                ""
            };
            output.push_str(&format!(
                "  ✓ Configured handler: {layer}\n  {qualifier}{layer} is configured to consume {key}.\n  It should not reach a later layer under this configuration.\n\n"
            ));
        }
        Conclusion::ConfiguredRedirect {
            layer,
            assumes_earlier_forward,
        } => {
            let qualifier = if *assumes_earlier_forward {
                "Assuming earlier uncertain layers forward it, "
            } else {
                ""
            };
            output.push_str(&format!(
                "  ✓ Configured handler: {layer}\n  {qualifier}{layer} is configured to redirect {key} to another window.\n  It should not reach a later layer in this chain under this configuration.\n\n"
            ));
        }
        Conclusion::SelectedApplication { layer, status } => match status {
            ApplicationStatus::Unavailable => {
                output.push_str(&format!("  Could not inspect {layer}.\n\n"));
            }
            ApplicationStatus::UnresolvedMode => {
                output.push_str(&format!(
                        "  Selected target: {layer} has a matching keymap for {key}, but mode-dependent execution is uncertain.\n\n"
                    ));
            }
            ApplicationStatus::UnverifiedExecution => {
                output.push_str(&format!(
                    "  Selected target: {layer} matches {key}; execution is unverified.\n\n"
                ));
            }
            ApplicationStatus::InteractiveGeneric(summary) => {
                output.push_str(&format!(
                    "  Selected target: {summary}; application shortcut handling is unverified.\n\n"
                ));
            }
            ApplicationStatus::NoMatchingKeymap => {
                output.push_str(&format!(
                    "  Selected target: {layer}; no matching keymap was found for {key}.\n\n"
                ));
            }
        },
        Conclusion::UnverifiedSession { layer } => {
            output.push_str(&format!(
                "  {layer} has a candidate binding for {key} in the root table, but terminal byte delivery could not be verified.\n\n"
            ));
        }
        Conclusion::HandledAndForwarded {
            layers,
            uncertain,
            uninspected_layer,
        } => {
            let names = layers.join(", ");
            if *uncertain {
                output.push_str(&format!(
                    "  {names} has a matching binding for {key}; forwarding is uncertain. Other layers may also handle it.\n  Downstream layer results are conditional.\n"
                ));
            } else {
                output.push_str(&format!(
                    "  {names} has a matching binding for {key} and is configured to forward it.\n  It should continue to the next layer.\n"
                ));
            }
            if let Some(unavailable) = uninspected_layer {
                output.push_str(&format!("  Note: {unavailable} was not inspected.\n"));
            }
            output.push('\n');
        }
        Conclusion::CouldNotInspect { layer } => {
            output.push_str(&format!("  Could not inspect {layer}.\n\n"));
        }
        Conclusion::ModifierAmbiguity { layer } => {
            output.push_str(&format!(
                "  No exact binding for {key} was found in {layer}.\n  Some same-key bindings may ignore modifiers, so forwarding cannot be proven.\n\n"
            ));
        }
        Conclusion::IndeterminateForwarding { layer } => {
            output.push_str(&format!(
                "  Could not determine whether {layer} forwards {key}.\n\n"
            ));
        }
        Conclusion::ConditionalForwarding => {
            output.push_str(&format!(
                "  {key} may be forwarded, but one or more layers are uncertain.\n  Downstream layer results are conditional.\n\n"
            ));
        }
        Conclusion::NoLayersInspected => {
            output.push_str("  No layers were inspected.\n\n");
        }
        Conclusion::UnhandledContinues => {
            output.push_str(&format!(
                "  No inspected layer has a matching binding for {key}.\n  It should continue to the next layer.\n\n"
            ));
        }
    }
    output
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
    use crate::layers::{BindingScope, LayerId, Outcome};

    #[test]
    fn renders_a_forwarded_result() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "no active binding found",
            vec!["active submap: default".into()],
        );

        let output = render(&key, std::slice::from_ref(&layer), false);

        assert!(output.contains("Key: CTRL + LEFT"));
        assert!(output.contains("Assessment: configured"));
        assert!(output.contains("No inspected layer has a matching binding for CTRL + LEFT."));
        assert!(output.contains("It should continue to the next layer."));
        assert!(output.contains("--verbose"));

        let verbose = render(&key, std::slice::from_ref(&layer), true);
        assert!(verbose.contains("✓ no active binding found"));
    }

    #[test]
    fn uses_the_layer_that_stops_the_key() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::HandledAndPassed,
                "binding found",
                vec![],
            ),
            LayerResult::new(
                "Readline",
                LayerId::Shell,
                Outcome::Consumed,
                "binding found",
                vec![],
            ),
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Readline is configured to consume CTRL + Z."));
        assert!(!output.contains("Hyprland handles and consumes"));
    }

    #[test]
    fn qualifies_a_later_handler_after_uncertainty() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unknown,
                "forwarding uncertain",
                vec![],
            ),
            LayerResult::new(
                "Bash / Readline",
                LayerId::Shell,
                Outcome::Consumed,
                "bound to backward-word",
                vec![],
            ),
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains("Assuming earlier uncertain layers forward it"));
        assert!(output.contains("Assessment: conditional"));
    }

    #[test]
    fn explains_a_redirected_key() {
        let key: KeyCombo = "ctrl+p".parse().unwrap();
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Redirected,
            "active binding found; event is redirected to another window",
            vec!["binding: pass class:example".into()],
        );

        let output = render(&key, &[layer], false);

        assert!(output.contains("↗ active binding found"));
        assert!(output.contains("is configured to redirect"));
    }

    #[test]
    fn keeps_handlers_from_earlier_forwarding_layers() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::HandledAndPassed,
                "binding found",
                vec![],
            ),
            LayerResult::new(
                "Ghostty",
                LayerId::Terminal,
                Outcome::Pass,
                "no binding found",
                vec![],
            ),
        ];

        let output = render(&key, &layers, false);

        assert!(output.contains(
            "Hyprland has a matching binding for CTRL + LEFT and is configured to forward it."
        ));
        assert!(!output.contains("No inspected layer handles"));
    }

    #[test]
    fn keeps_confirmed_and_possible_handlers() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::UncertainContinues,
                "binding may run",
                vec![],
            ),
            LayerResult::new(
                "Ghostty",
                LayerId::Terminal,
                Outcome::HandledAndPassed,
                "binding found",
                vec![],
            ),
        ];

        let output = render(&key, &layers, false);

        assert!(
            output.contains(
                "Ghostty has a matching binding for CTRL + LEFT; forwarding is uncertain."
            )
        );
        assert!(output.contains("Other layers may also handle it."));
    }

    #[test]
    fn renders_unavailable_before_indeterminate_propagation() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Unavailable,
            "inspection failed",
            vec![],
        );

        let output = render(&key, &[layer], false);

        assert!(output.contains("! inspection failed"));
        assert!(output.contains("Could not inspect Hyprland."));
    }

    #[test]
    fn keeps_unavailable_context_visible_when_nothing_handles() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unavailable,
                "inspection failed",
                vec![],
            ),
            LayerResult::new(
                "Readline",
                LayerId::Shell,
                Outcome::Pass,
                "no binding",
                vec![],
            ),
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
            disposition: crate::listen::CaptureDisposition::ObservedOnly,
        };
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unknown,
                "forwarding uncertain",
                vec![],
            ),
            LayerResult::new(
                "Readline",
                LayerId::Shell,
                Outcome::Consumed,
                "bound to backward-word",
                vec![],
            ),
        ];

        let output = render_observed(&observed, &layers, true);

        assert!(output.contains("earlier forwarding is confirmed"));
        assert!(output.contains("probe bytes: ESC [ 1 ; 5 D"));
        let normal = render_observed(&observed, &layers, false);
        assert!(normal.contains("earlier forwarding is confirmed"));
        assert!(!normal.contains("probe bytes"));
        assert!(output.contains("Readline is configured to consume CTRL + LEFT."));
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
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(33)),
            encoding: "evdev key event before compositor processing".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: Some("physical keycode 33 (F)".into()),
            source: crate::listen::CaptureSource::Evdev {
                device: "Test Keyboard".into(),
                path: "/dev/input/event0".into(),
            },
            disposition: crate::listen::CaptureDisposition::ObservedOnly,
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "no active binding found",
            vec![],
        )];

        let output = render_observed(&observed, &layers, false);
        assert!(output.contains("source: evdev (Test Keyboard; /dev/input/event0)"));
        assert!(output.contains("physical key event was captured before the compositor"));
        assert!(!output.contains("earlier forwarding is confirmed"));
        assert!(!output.contains("probe bytes:"));
    }

    #[test]
    fn hyprland_report_does_not_claim_terminal_forwarding() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: Some("Hyprland XKB keycode=36 evdev=28 (press)".into()),
            modifier_state: Some(crate::listen::ModifierState {
                pressed: vec!["CTRL".into(), "SUPER".into()],
                locked: vec![],
                latched: None,
                devices: vec![],
            }),
            associated_text: None,
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(28)),
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: Some("physical keycode 28 (RETURN)".into()),
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::PassedThrough,
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )];

        let output = render_observed(&observed, &layers, true);
        assert!(output.contains("source: Hyprland"));
        assert!(output.contains("observed key: CTRL + SUPER + RETURN"));
        assert!(output.contains("encoding: Hyprland XKB key event"));
        let normal = render_observed(&observed, &layers, false);
        assert!(normal.contains("source: Hyprland"));
        assert!(!normal.contains("encoding: Hyprland XKB key event"));
        assert!(!normal.contains("modifiers latched"));
        assert!(
            output.contains(
                "The key event was captured by Hyprland, but forwarding is not confirmed."
            )
        );
        assert!(!output.contains("earlier forwarding is confirmed"));
        assert!(output.contains("modifiers latched: unavailable from compositor"));
    }

    fn opaque_herdr_evidence() -> BindingEvidence {
        BindingEvidence {
            dispatcher: Some("__lua".into()),
            action: Some("__lua 285".into()),
            description: Some("Herdr".into()),
            submap: Some("default".into()),
            scope: crate::layers::BindingScope::Submap("default".into()),
            source: None,
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::OpaqueDispatcher),
        }
    }

    #[test]
    fn hyprland_report_with_suppression() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: Some("Hyprland XKB keycode=36 evdev=28 (press)".into()),
            modifier_state: Some(crate::listen::ModifierState {
                pressed: vec!["CTRL".into(), "SUPER".into()],
                locked: vec![],
                latched: None,
                devices: vec![],
            }),
            associated_text: None,
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(28)),
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: Some("physical keycode 28 (RETURN)".into()),
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::Suppressed,
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )
        .with_binding(opaque_herdr_evidence())];

        let output = render_observed(&observed, &layers, false);
        assert!(output.contains("source: Hyprland"));
        assert!(output.contains("observed key: CTRL + SUPER + RETURN"));
        assert!(output.contains("Whykey captured and suppressed this event."));
        // An opaque dispatcher never becomes "executed": the conclusion may
        // name the configured action but must qualify it as unexecuted.
        assert!(
            output.contains("The normal configuration indicates that Hyprland may execute Herdr")
        );
        assert!(output.contains("Whykey did not execute the dispatcher"));
        assert!(!output.contains("would run Herdr"));
        assert!(!output.contains("would handle and consume"));
        assert!(!output.contains("forwarding is not confirmed"));
        assert!(!output.contains("earlier forwarding is confirmed"));
    }

    #[test]
    fn uncertain_suppression_never_claims_consumption_or_execution() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: Some("Hyprland XKB keycode=36 evdev=28 (press)".into()),
            modifier_state: None,
            associated_text: None,
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(28)),
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: None,
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::Suppressed,
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching binding; runtime effect unknown",
            vec!["opaque runtime hook with a Herdr label".into()],
        )
        .with_binding(opaque_herdr_evidence())];

        let output = render_observed(&observed, &layers, false);

        assert!(output.contains("runtime effect and propagation could not be determined"));
        assert!(output.contains("Whykey did not execute the dispatcher"));
        assert!(!output.contains("would run Herdr"));
        assert!(!output.contains("would handle and consume"));
    }

    #[test]
    fn hyprland_report_with_universal_binding_qualifies_suppression() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: Some("Hyprland XKB keycode=36 evdev=28 (press)".into()),
            modifier_state: Some(crate::listen::ModifierState {
                pressed: vec!["CTRL".into(), "SUPER".into()],
                locked: vec![],
                latched: None,
                devices: vec![],
            }),
            associated_text: None,
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(28)),
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: Some("physical keycode 28 (RETURN)".into()),
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::Suppressed,
        };
        let universal = BindingEvidence {
            scope: crate::layers::BindingScope::Submap("default".into()),
            has_universal_match: true,
            ..opaque_herdr_evidence()
        };
        // No "(all submaps)" marker: the conclusion must follow the
        // typed scope, never the detail wording.
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["opaque runtime hook with a Herdr label".into()],
        )
        .with_binding(universal)];

        let output = render_observed(&observed, &layers, false);
        assert!(output.contains("source: Hyprland"));
        assert!(output.contains(
            "Whykey captured this event, but matching universal Hyprland bindings bypass submap capture and may execute."
        ));
        assert!(
            output.contains("The normal configuration indicates that Hyprland may execute Herdr.")
        );
    }

    #[test]
    fn schema_v1_json_keeps_normal_and_verbose_evidence() {
        let key: KeyCombo = "ctrl+x".parse().unwrap();
        let layers = [LayerResult::new(
            "test",
            LayerId::Diagnostic,
            Outcome::Pass,
            "not handled",
            vec!["normal evidence".into()],
        )
        .with_verbose_details(vec!["verbose evidence".into()])];
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&key, &layers, None, 1)).unwrap();
        assert_eq!(
            value["layers"][0]["details"],
            serde_json::json!(["normal evidence", "verbose evidence"])
        );
    }

    #[test]
    fn layer_summary_is_separated_from_its_details() {
        let key: KeyCombo = "ctrl+x".parse().unwrap();
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "active binding found; forwarding cannot be determined",
            vec!["active submap: default".into()],
        )];
        let output = render(&key, &layers, false);
        assert!(output.contains(
            "  ? active binding found; forwarding cannot be determined\n    active submap: default\n"
        ));
    }

    #[test]
    fn normal_report_fits_one_screen_while_verbose_keeps_diagnostics() {
        let observed = suppressed_observed();
        let observed = ObservedKey {
            modifier_state: Some(crate::listen::ModifierState {
                pressed: vec!["CTRL".into(), "SUPER".into()],
                locked: vec![],
                latched: None,
                devices: vec![],
            }),
            alternate_key: Some("physical keycode 28 (RETURN)".into()),
            ..observed
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )
        .with_verbose_details(vec![
            "main keyboard: at-translated-set-2-keyboard".into(),
            "active XKB layout group index: 0".into(),
            "2 matching binding(s) exist in inactive submap(s); current submap is default".into(),
        ])
        .with_binding(opaque_herdr_evidence())];
        let normal = render_observed(&observed, &layers, false);
        assert!(
            normal.lines().count() < 24,
            "normal report must fit roughly one screen, got {} lines:\n{normal}",
            normal.lines().count()
        );
        for diagnostic in [
            "main keyboard",
            "group index",
            "inactive submap",
            "raw event",
            "modifiers pressed",
        ] {
            assert!(
                !normal.contains(diagnostic),
                "normal report must not carry {diagnostic:?}"
            );
        }
        assert!(normal.contains("may execute Herdr"));
        let verbose = render_observed(&observed, &layers, true);
        for diagnostic in [
            "main keyboard",
            "group index",
            "inactive submap",
            "raw event",
            "modifiers pressed",
        ] {
            assert!(
                verbose.contains(diagnostic),
                "--verbose must retain {diagnostic:?}"
            );
        }
        assert_eq!(
            conclusion_lines(&normal),
            conclusion_lines(&verbose),
            "verbosity changes evidence depth, never the conclusion"
        );
    }

    #[test]
    fn text_and_json_share_the_conclusion() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = suppressed_observed();
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )
        .with_verbose_details(vec!["main keyboard: test".into()])
        .with_binding(opaque_herdr_evidence())];
        let text = render_observed(&observed, &layers, false);
        let assessment = text
            .lines()
            .find_map(|line| line.strip_prefix("Assessment: "))
            .expect("text report names the assessment");
        for version in [1, 2] {
            let json: serde_json::Value =
                serde_json::from_str(&render_listen_json(&key, &layers, Some(&observed), version))
                    .unwrap();
            let confidence = if version == 1 {
                json["confidence"].as_str().unwrap()
            } else {
                json["assessment"]["confidence"].as_str().unwrap()
            };
            assert_eq!(
                assessment, confidence,
                "v{version} JSON must agree with text"
            );
        }
        let v2: serde_json::Value =
            serde_json::from_str(&render_listen_json(&key, &layers, Some(&observed), 2)).unwrap();
        assert!(
            v2["path"][0]["evidence"]
                .as_array()
                .is_some_and(|evidence| evidence
                    .iter()
                    .any(|item| item["text"] == "main keyboard: test")),
            "JSON keeps verbose evidence even in normal mode"
        );
    }

    #[test]
    fn inactive_ime_stays_context_not_a_traversed_handler() {
        let observed = suppressed_observed();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Pass,
                "no active binding found",
                vec![],
            ),
            LayerResult::new(
                "Input method",
                LayerId::Ime,
                Outcome::UncertainContinues,
                "input method was detected but inactive or closed; application-side text transformation remains unobserved",
                vec!["runtime state: inactive".into()],
            ),
        ];
        let output = render_observed(&observed, &layers, false);
        assert!(!output.contains("Final handler: Input method"));
        assert!(output.contains("inactive or closed"));
    }
    fn suppressed_observed() -> ObservedKey {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        ObservedKey {
            combo: key,
            raw: Vec::new(),
            raw_display: Some("Hyprland XKB keycode=36 evdev=28 (press)".into()),
            modifier_state: None,
            associated_text: None,
            physical_keycode: Some(crate::xkb::EvdevKeycode::from(28)),
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: None,
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::Suppressed,
        }
    }

    fn conclusion_lines(output: &str) -> Vec<&str> {
        let mut lines = Vec::new();
        let mut in_result = false;
        for line in output.lines() {
            if line == "Result:" {
                in_result = true;
                continue;
            }
            if in_result {
                if line.is_empty() || !line.starts_with(' ') {
                    break;
                }
                lines.push(line);
            }
        }
        lines
    }

    #[test]
    fn detail_wording_never_changes_the_conclusion() {
        let observed = suppressed_observed();
        let plain = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )
        .with_binding(opaque_herdr_evidence())];
        let reworded = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["a runtime hook labeled Herdr (all submaps)".into()],
        )
        .with_binding(opaque_herdr_evidence())];
        assert_eq!(
            conclusion_lines(&render_observed(&observed, &plain, false)),
            conclusion_lines(&render_observed(&observed, &reworded, false)),
            "the conclusion follows typed evidence, not detail wording"
        );
    }

    #[test]
    fn schema_v2_carries_binding_evidence_while_v1_omits_it() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Consumed,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )
        .with_binding(opaque_herdr_evidence())];
        let v1: serde_json::Value =
            serde_json::from_str(&render_json(&key, &layers, None, 1)).unwrap();
        assert!(v1["layers"][0].get("binding").is_none());
        let v2: serde_json::Value =
            serde_json::from_str(&render_json(&key, &layers, None, 2)).unwrap();
        let binding = &v2["path"][0]["binding"];
        assert_eq!(binding["dispatcher"], "__lua");
        assert_eq!(binding["scope"]["Submap"], "default");
        assert_eq!(binding["description"], "Herdr");
        assert_eq!(binding["submap"], "default");
        assert_eq!(binding["action"], "__lua 285");
    }

    #[test]
    fn schema_v2_json_includes_disposition_while_v1_omits_it() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: Vec::new(),
            raw_display: None,
            modifier_state: None,
            associated_text: None,
            physical_keycode: None,
            encoding: "Hyprland XKB key event".into(),
            protocol_flags: None,
            event_type: crate::listen::KeyEventType::Press,
            alternate_keys: None,
            alternate_key: None,
            source: crate::listen::CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition: crate::listen::CaptureDisposition::Suppressed,
        };
        let layers = [LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "active binding found",
            vec!["binding: __lua 285; Herdr".into()],
        )];

        let v1_json = render_listen_json(&key, &layers, Some(&observed), 1);
        let v1_val: serde_json::Value = serde_json::from_str(&v1_json).unwrap();
        assert_eq!(v1_val["schema_version"], 1);
        assert!(v1_val["observed"].is_object());
        assert!(
            v1_val["observed"].get("disposition").is_none(),
            "schema-v1 must omit disposition"
        );

        let v2_json = render_listen_json(&key, &layers, Some(&observed), 2);
        let v2_val: serde_json::Value = serde_json::from_str(&v2_json).unwrap();
        assert_eq!(v2_val["schema_version"], 2);
        assert_eq!(v2_val["observation"]["disposition"], "suppressed");
    }

    #[test]
    fn schema_v2_names_native_backend_and_keeps_source_label() {
        let key: KeyCombo = "ctrl+super+return".parse().unwrap();
        let mut observed = suppressed_observed();
        observed.source = crate::listen::CaptureSource::CompositorNative {
            backend: "Hyprland".into(),
        };
        let value: serde_json::Value =
            serde_json::from_str(&render_listen_json(&key, &[], Some(&observed), 2)).unwrap();
        assert_eq!(value["observation"]["source"]["kind"], "compositor-native");
        assert_eq!(value["observation"]["source"]["backend"], "Hyprland");
        assert_eq!(value["observation"]["source_label"], "Hyprland");
        let legacy: serde_json::Value =
            serde_json::from_str(&render_listen_json(&key, &[], Some(&observed), 1)).unwrap();
        assert_eq!(legacy["observed"]["source"], "Hyprland");
    }

    #[test]
    fn renders_each_sequence_step_in_text() {
        let sequence: KeySequence = "ctrl+x ctrl+s".parse().unwrap();
        let reports = vec![
            vec![LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Pass,
                "no active binding found",
                vec![],
            )],
            vec![LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Consumed,
                "binding found",
                vec![],
            )],
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

    #[test]
    fn upstream_match_is_not_masked_by_trailing_unavailable_layer() {
        let key: KeyCombo = "ctrl+alt+delete".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::HandledUncertain,
                "active binding found; forwarding cannot be determined",
                vec!["binding: __lua 11; Close all windows".into()],
            ),
            LayerResult::new(
                "Shell input",
                LayerId::Shell,
                Outcome::Unavailable,
                "shell 'unknown shell' is not inspected",
                vec![],
            ),
        ];
        let output = render(&key, &layers, false);
        assert!(output.contains(
            "Hyprland has a matching binding for CTRL + ALT + DELETE; forwarding is uncertain."
        ));
        assert!(!output.contains("Result:\n  Could not inspect Shell input."));
        assert!(output.contains("Note: Shell input was not inspected."));
    }

    #[test]
    fn candidate_multiplexer_binding_reports_unverified_delivery() {
        let key: KeyCombo = "alt+return".parse().unwrap();
        let layers = [LayerResult::new(
            "tmux",
            LayerId::Multiplexer,
            Outcome::HandledUncertain,
            "candidate binding matches in tmux root table; delivery is unverified",
            vec!["binding: bind-key -T root M-Enter split-window -v".into()],
        )
        .with_binding(BindingEvidence {
            dispatcher: None,
            action: Some("split-window -v".into()),
            description: None,
            submap: None,
            scope: BindingScope::Unknown,
            source: None,
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::UnverifiedTerminalBytes),
        })];
        let output = render(&key, &layers, false);
        assert!(output.contains(
            "tmux has a candidate binding for ALT + RETURN in the root table, but terminal byte delivery could not be verified."
        ));
        assert!(!output.contains("No inspected layer handles"));
    }

    #[test]
    fn active_application_target_reports_application_uncertainty_without_shell() {
        let key: KeyCombo = "ctrl+w".parse().unwrap();
        let layers = [LayerResult::new(
            "Neovim",
            LayerId::Application,
            Outcome::HandledUncertain,
            "runtime mapping found; Neovim may consume the key",
            vec!["runtime mapping: mode n: <C-W>".into()],
        )
        .with_binding(BindingEvidence {
            dispatcher: None,
            action: Some("<C-W>".into()),
            description: None,
            submap: None,
            scope: BindingScope::Unknown,
            source: None,
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::UnresolvedMode),
        })];
        let output = render(&key, &layers, false);
        assert!(output.contains(
            "Selected target: Neovim has a matching keymap for CTRL + W, but mode-dependent execution is uncertain."
        ));
        assert!(!output.contains("Bash"));
        assert!(!output.contains("Readline"));
    }

    #[test]
    fn evaluate_conclusion_precedence() {
        let consumer = LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::Consumed,
            "copy",
            vec![],
        );
        let app = LayerResult::new(
            "Neovim",
            LayerId::Application,
            Outcome::HandledUncertain,
            "mode",
            vec![],
        );

        // Consumer before app -> ConfiguredConsumer takes precedence
        let eval = evaluate_conclusion(&[consumer.clone(), app.clone()], None, false);
        assert_eq!(
            eval.conclusion,
            Conclusion::ConfiguredConsumer {
                layer: "Ghostty",
                assumes_earlier_forward: false,
            }
        );

        // App without consumer -> SelectedApplication
        let eval = evaluate_conclusion(&[app], None, false);
        assert!(matches!(
            eval.conclusion,
            Conclusion::SelectedApplication {
                layer: "Neovim",
                status: ApplicationStatus::NoMatchingKeymap,
            }
        ));
    }

    #[test]
    fn evaluate_conclusion_typed_modifier_ambiguity() {
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::UncertainContinues,
            "arbitrary reworded summary text",
            vec![],
        )
        .with_binding(BindingEvidence {
            dispatcher: None,
            action: None,
            description: None,
            submap: None,
            scope: BindingScope::Unknown,
            source: None,
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::ModifierAmbiguity),
        });
        let eval = evaluate_conclusion(&[layer], None, false);
        assert_eq!(
            eval.conclusion,
            Conclusion::ModifierAmbiguity { layer: "Hyprland" }
        );
    }

    #[test]
    fn evaluate_conclusion_summary_wording_independence() {
        // Changing summary text must not change the conclusion when typed uncertainty is present
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::UncertainContinues,
            "something completely custom and unrelated to bindings",
            vec![],
        )
        .with_binding(BindingEvidence {
            dispatcher: None,
            action: None,
            description: None,
            submap: None,
            scope: BindingScope::Unknown,
            source: None,
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::ModifierAmbiguity),
        });
        let eval = evaluate_conclusion(&[layer], None, false);
        assert_eq!(
            eval.conclusion,
            Conclusion::ModifierAmbiguity { layer: "Hyprland" }
        );
    }

    #[test]
    fn evaluate_conclusion_display_label_independence() {
        // Generic unadapted target with custom display label
        let custom_app = LayerResult::new(
            "Custom Process Display Name",
            LayerId::Application,
            Outcome::UnadaptedTarget,
            "custom process summary",
            vec![],
        );
        let eval = evaluate_conclusion(&[custom_app], None, false);
        assert_eq!(
            eval.conclusion,
            Conclusion::SelectedApplication {
                layer: "Custom Process Display Name",
                status: ApplicationStatus::InteractiveGeneric("custom process summary".into()),
            }
        );

        // Profiled editor with custom display label (no matching keymap)
        let profiled_app = LayerResult::new(
            "My Customized Neovim",
            LayerId::Application,
            Outcome::Unknown,
            "no matching mapping",
            vec![],
        );
        let eval2 = evaluate_conclusion(&[profiled_app], None, false);
        assert_eq!(
            eval2.conclusion,
            Conclusion::SelectedApplication {
                layer: "My Customized Neovim",
                status: ApplicationStatus::NoMatchingKeymap,
            }
        );
    }
}
