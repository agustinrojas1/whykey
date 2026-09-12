use crate::key::{KeyCombo, KeySequence};
use crate::layers::{
    BindingEvidence, LayerId, LayerResult, LayerStatus, Outcome, Propagation, UncertaintyReason,
    format_bytes,
};
use crate::listen::ObservedKey;
use crate::schema;
use crate::style::{self, RenderOptions};
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
    render_with_options(key, layers, RenderOptions::plain(verbose))
}

pub fn render_with_options(
    key: &KeyCombo,
    layers: &[LayerResult],
    options: RenderOptions,
) -> String {
    render_inner(key, layers, None, options, true)
}

pub fn render_observed(observed: &ObservedKey, layers: &[LayerResult], verbose: bool) -> String {
    render_observed_with_options(observed, layers, RenderOptions::plain(verbose))
}

pub fn render_observed_with_options(
    observed: &ObservedKey,
    layers: &[LayerResult],
    options: RenderOptions,
) -> String {
    render_inner(&observed.combo, layers, Some(observed), options, true)
}

/// Render an inspection where each step is analyzed independently.
pub fn render_sequence(
    sequence: &KeySequence,
    reports: &[Vec<LayerResult>],
    verbose: bool,
) -> String {
    render_sequence_with_options(sequence, reports, RenderOptions::plain(verbose))
}

pub fn render_sequence_with_options(
    sequence: &KeySequence,
    reports: &[Vec<LayerResult>],
    options: RenderOptions,
) -> String {
    let mut output = format!("{}\n\n", options.accent("Sequence · Inspect"));
    for (index, (key, layers)) in sequence.as_slice().iter().zip(reports).enumerate() {
        output.push_str(&format!(
            "{}\n\n",
            options.bold(format!(
                "Step {}/{} · {}",
                index + 1,
                sequence.len(),
                style::human_key(key)
            ))
        ));
        output.push_str(&render_inner(key, layers, None, options, false));
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
    observation: Option<&ObservedKey>,
    options: RenderOptions,
    include_header: bool,
) -> String {
    let observed = observation.is_some();
    let terminal_observed = observation.is_some_and(|value| value.source.confirms_terminal());
    let operation = if observed { "Listen" } else { "Inspect" };
    let human_key = style::human_key(key);
    let evaluated = evaluate_conclusion(layers, observation, terminal_observed);
    let mut output = String::new();

    if include_header {
        output.push_str(&options.bold(human_key));
        output.push_str(&options.accent(format!(" · {operation}")));
        output.push_str("\n\n");
    }

    let diagnosis = diagnosis_text(key, layers, observation, &evaluated.conclusion);
    for line in style::wrap_hanging(&diagnosis, options.width, "", "  ") {
        output.push_str(&options.bold(line));
        output.push('\n');
    }
    if let Some(qualification) = qualification_text(key, layers, observation, &evaluated.conclusion)
    {
        for line in style::wrap_hanging(&qualification, options.width, "", "  ") {
            output.push_str(&options.uncertain(line));
            output.push('\n');
        }
    }
    if let Some(observation) = observation {
        let capture_mode = match observation.disposition {
            crate::listen::CaptureDisposition::Suppressed => "suppressed",
            crate::listen::CaptureDisposition::PassedThrough => "pass-through",
            crate::listen::CaptureDisposition::ObservedOnly => "observe-only",
        };
        let observed_line = format!(
            "Observed via {} ({}).",
            observation.source.label(),
            capture_mode
        );
        for line in style::wrap_hanging(&observed_line, options.width, "", "  ") {
            output.push_str(&options.muted(line));
            output.push('\n');
        }
    }
    output.push('\n');

    output.push_str(&options.accent("Effect"));
    output.push('\n');
    let effect = effect_text(key, layers, observation, &evaluated.conclusion);
    for line in style::wrap_hanging(&effect, options.width, "  ", "  ") {
        output.push_str(&line);
        output.push('\n');
    }
    output.push('\n');

    let visible = relevant_layers(layers, observation);
    output.push_str(&options.accent(if observed {
        "Relevant layers (configuration and observation)"
    } else {
        "Relevant layers (configuration)"
    }));
    output.push('\n');
    if visible.is_empty() {
        output.push_str("  No matching handler was found in the inspected layers.\n");
    } else {
        for layer in visible {
            let line = layer_line(layer, observation, terminal_observed, options);
            for wrapped in style::wrap_hanging(&line, options.width, "  ", "      ") {
                output.push_str(&wrapped);
                output.push('\n');
            }
            if let Some(evidence) = concise_binding_evidence(layer) {
                for wrapped in style::wrap_hanging(&evidence, options.width, "      ", "      ") {
                    output.push_str(&wrapped);
                    output.push('\n');
                }
            } else if let Some(evidence) = concise_detail_evidence(layer) {
                for wrapped in style::wrap_hanging(&evidence, options.width, "      ", "      ") {
                    output.push_str(&wrapped);
                    output.push('\n');
                }
            }
        }
    }
    output.push('\n');

    if let Some(uncertainty) = uncertainty_text(key, layers, observation, &evaluated.conclusion) {
        output.push_str(&options.accent("What remains uncertain"));
        output.push('\n');
        for line in style::wrap_hanging(&uncertainty, options.width, "  ", "  ") {
            output.push_str(&options.uncertain(line));
            output.push('\n');
        }
        output.push('\n');
    }

    if let Some(next) = next_step(key, layers, observation, &evaluated.conclusion) {
        output.push_str(&options.accent("Next"));
        output.push('\n');
        for line in style::wrap_hanging(&next, options.width, "  ", "  ") {
            output.push_str(&line);
            output.push('\n');
        }
        output.push('\n');
    }

    if options.verbose {
        render_verbose_evidence(&mut output, key, layers, observation, options);
        if let Some(note) = compositor_candidates_note() {
            output.push_str(&options.muted(note));
        }
    } else {
        let hint = "Technical detail: use --verbose for the complete route and raw evidence.";
        for line in style::wrap_hanging(hint, options.width, "", "  ") {
            output.push_str(&options.muted(line));
            output.push('\n');
        }
    }
    output
}

fn diagnosis_text(
    _key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> String {
    if let Some(observation) = observation {
        if matches!(
            observation.disposition,
            crate::listen::CaptureDisposition::Suppressed
        ) {
            if let Some(layer) = layers.iter().find(|layer| layer.id == LayerId::Compositor) {
                if layer
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.is_opaque())
                {
                    let backend = observation.source.backend_name().unwrap_or("compositor");
                    return format!(
                        "{backend} has a matching binding; its runtime effect is unknown."
                    );
                }
            }
        }
        if observation.source.proves_compositor_receipt() {
            let backend = observation.source.backend_name().unwrap_or("compositor");
            if let Some(layer) = layers.iter().find(|layer| layer.id == LayerId::Compositor) {
                if layer
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.is_opaque())
                    || layer.propagation() == Propagation::Indeterminate
                {
                    return format!("{backend} has a matching binding; forwarding is unknown.");
                }
                if layer.status() == LayerStatus::Handled {
                    return format!(
                        "{backend} has a matching binding; its configured route is shown below."
                    );
                }
            }
            return format!("{backend} observed the key; no matching binding was confirmed.");
        }
    }

    match conclusion {
        Conclusion::ConfiguredConsumer { layer, .. } if *layer == "TTY driver" => {
            if layers.iter().any(|candidate| {
                candidate.id == LayerId::Tty
                    && candidate.outcome == Outcome::Consumed
                    && candidate
                        .details
                        .iter()
                        .any(|detail| detail.contains("VSUSP"))
            }) {
                "TTY is configured to suspend the foreground job.".into()
            } else {
                "TTY driver is configured to consume this control byte.".into()
            }
        }
        Conclusion::ConfiguredConsumer { layer, .. } => {
            format!("{layer} is configured to consume the shortcut.")
        }
        Conclusion::ConfiguredRedirect { layer, .. } => {
            format!("{layer} is configured to redirect the shortcut.")
        }
        Conclusion::SelectedApplication { layer, status } => match status {
            ApplicationStatus::Unavailable => format!("{layer} could not be inspected."),
            ApplicationStatus::UnresolvedMode | ApplicationStatus::UnverifiedExecution => {
                format!("{layer} has a matching keymap, but execution is unverified.")
            }
            ApplicationStatus::InteractiveGeneric(_) => {
                format!("{layer} is the selected application; shortcut handling is unverified.")
            }
            ApplicationStatus::NoMatchingKeymap => format!("{layer} has no matching keymap."),
        },
        Conclusion::UnverifiedSession { layer } => {
            format!("{layer} has a candidate binding, but delivery is unverified.")
        }
        Conclusion::HandledAndForwarded { layers, uncertain, .. } => {
            let names = layers.join(", ");
            if *uncertain {
                format!("{names} has a matching binding; forwarding is unknown.")
            } else {
                format!("{names} has a matching binding and is configured to forward it.")
            }
        }
        Conclusion::CouldNotInspect { layer } => format!("{layer} could not be inspected."),
        Conclusion::ModifierAmbiguity { layer } => {
            format!("{layer} has same-key bindings, but modifier matching is ambiguous.")
        }
        Conclusion::IndeterminateForwarding { layer } => {
            format!("{layer} has a matching key, but forwarding is unknown.")
        }
        Conclusion::ConditionalForwarding => {
            "No single handler can be confirmed; forwarding depends on an uncertain layer.".into()
        }
        Conclusion::NoLayersInspected => "No layers were inspected.".into(),
        Conclusion::UnhandledContinues => "No inspected layer matches; it can continue.".into(),
        Conclusion::SuppressedHandled { .. } => {
            "A matching compositor binding was captured for inspection; its runtime effect is configuration-only.".into()
        }
    }
}

fn qualification_text(
    _key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> Option<String> {
    if matches!(
        conclusion,
        Conclusion::ConfiguredConsumer {
            assumes_earlier_forward: true,
            ..
        } | Conclusion::ConfiguredRedirect {
            assumes_earlier_forward: true,
            ..
        }
    ) {
        return Some("This applies if earlier layers forward the key.".into());
    }
    if observation.is_some_and(|value| value.source.proves_compositor_receipt())
        && layers.iter().any(|layer| {
            layer.id == LayerId::Compositor
                && (layer.propagation() == Propagation::Indeterminate
                    || layer
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.is_opaque()))
        })
    {
        return Some(
            "Whykey cannot determine whether the Lua/plugin action passes the key onward.".into(),
        );
    }
    None
}

fn effect_text(
    _key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> String {
    if let Some(observation) = observation {
        match observation.disposition {
            crate::listen::CaptureDisposition::Suppressed => {
                return "Whykey temporarily intercepted the compositor event to inspect its configuration. That does not prove the configured action ran.".into();
            }
            crate::listen::CaptureDisposition::PassedThrough => {
                if observation.source.proves_compositor_receipt() {
                    let backend = observation
                        .source
                        .backend_name()
                        .unwrap_or("the compositor");
                    return format!(
                        "The key was observed at {backend}. Receipt is confirmed; downstream forwarding is a separate question."
                    );
                }
            }
            crate::listen::CaptureDisposition::ObservedOnly => {
                if observation.source.confirms_terminal() {
                    return "The event reached the terminal, so earlier forwarding is confirmed."
                        .into();
                }
                if observation.source.proves_compositor_receipt() {
                    let backend = observation
                        .source
                        .backend_name()
                        .unwrap_or("the compositor");
                    return format!(
                        "The key was observed at {backend}. This confirms receipt, not forwarding."
                    );
                }
                return format!(
                    "The physical key event was observed via {} before compositor processing. Later forwarding is not confirmed.",
                    observation.source.label()
                );
            }
        }
    }
    if layers.iter().any(|layer| {
        layer.id == LayerId::Tty
            && layer.outcome == Outcome::Consumed
            && layer.details.iter().any(|detail| detail.contains("VSUSP"))
    }) {
        return "Terminal signal handling sends a stop signal to the foreground job when this control byte arrives.".into();
    }
    match conclusion {
        Conclusion::ConfiguredConsumer { .. } => "The configured consumer stops this shortcut before later layers can handle it.".into(),
        Conclusion::ConfiguredRedirect { .. } => "The configured redirect sends the shortcut to another window.".into(),
        Conclusion::HandledAndForwarded { uncertain: true, .. } | Conclusion::ConditionalForwarding => "A configured layer may handle the shortcut, but later handling depends on uncertain forwarding.".into(),
        Conclusion::HandledAndForwarded { .. } => "The matching configuration passes the shortcut to the next layer.".into(),
        Conclusion::UnhandledContinues => "No matching configuration was found, so the shortcut can continue downstream.".into(),
        _ => "The report below separates observed events from configuration-based predictions.".into(),
    }
}

fn relevant_layers<'a>(
    layers: &'a [LayerResult],
    observation: Option<&ObservedKey>,
) -> Vec<&'a LayerResult> {
    let mut selected = Vec::new();
    for layer in layers {
        let important = !matches!(layer.outcome, Outcome::Pass)
            || layer.binding.is_some()
            || layer.id == LayerId::Terminal
            || observation.is_some_and(|observed| {
                observed.source.proves_compositor_receipt() && layer.id == LayerId::Compositor
            });
        if important {
            selected.push(layer);
        }
    }
    if selected.is_empty() {
        if let Some(layer) = layers.iter().find(|layer| layer.id == LayerId::Terminal) {
            selected.push(layer);
        } else if let Some(layer) = layers.first() {
            selected.push(layer);
        }
    }
    selected
}

fn layer_line(
    layer: &LayerResult,
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
    options: RenderOptions,
) -> String {
    let status = match layer.outcome {
        Outcome::Pass => "No matching binding",
        Outcome::HandledAndPassed if terminal_observed => "Observed receipt",
        Outcome::HandledAndPassed => "Matching binding",
        Outcome::Consumed => "Configured to stop",
        Outcome::Redirected => "Redirected",
        Outcome::HandledUncertain | Outcome::UncertainContinues | Outcome::Unknown => {
            "Forwarding unknown"
        }
        Outcome::Unavailable => "Unavailable",
        Outcome::UnadaptedTarget => "Not evaluated",
    };
    let status = match status {
        "Unavailable" => options.failure(status),
        "Forwarding unknown" => options.uncertain(status),
        "Observed receipt" => options.success(status),
        "No matching binding" | "Not evaluated" => options.muted(status),
        _ => status.to_owned(),
    };
    let explanation = if layer.id == LayerId::Tty && layer.outcome == Outcome::Consumed {
        "terminal signal handling".into()
    } else if let Some(binding) = &layer.binding {
        binding
            .description
            .clone()
            .or_else(|| binding.action.clone())
            .unwrap_or_else(|| concise_summary(&layer.summary))
    } else if observation.is_some_and(|value| {
        value.source.proves_compositor_receipt() && layer.id == LayerId::Compositor
    }) {
        "event received at the compositor".into()
    } else {
        concise_summary(&layer.summary)
    };
    format!("{} — {status}: {explanation}", layer.layer)
}

fn concise_binding_evidence(layer: &LayerResult) -> Option<String> {
    let binding = layer.binding.as_ref()?;
    let mut fields = Vec::new();
    if let Some(action) = &binding.action {
        fields.push(format!("action {action}"));
    }
    if let Some(source) = &binding.source {
        let location = source.line.map_or_else(
            || source.file.clone(),
            |line| format!("{}:{line}", source.file),
        );
        fields.push(format!("source {location}"));
    }
    (!fields.is_empty()).then(|| fields.join("; "))
}

/// Adapter details use a small set of explicit evidence labels. Select only
/// those tagged lines for the compact report; the complete detail collection
/// remains available in `--verbose` and JSON. This keeps presentation choices
/// separate from the typed conclusion instead of parsing prose to decide it.
fn concise_detail_evidence(layer: &LayerResult) -> Option<String> {
    for prefix in [
        "binding:",
        "action:",
        "sequence:",
        "source:",
        "static transformation:",
    ] {
        if let Some(detail) = layer
            .details
            .iter()
            .find(|detail| detail.starts_with(prefix))
        {
            return Some(detail.clone());
        }
    }
    None
}

fn concise_summary(summary: &str) -> String {
    summary
        .split(';')
        .next()
        .unwrap_or(summary)
        .trim()
        .to_owned()
}

fn uncertainty_text(
    _key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> Option<String> {
    if observation.is_some_and(|value| value.source.proves_compositor_receipt())
        && layers.iter().any(|layer| {
            layer.id == LayerId::Compositor
                && (layer.propagation() == Propagation::Indeterminate
                    || layer
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.is_opaque()))
        })
    {
        return None;
    }
    if matches!(
        conclusion,
        Conclusion::ConfiguredConsumer {
            assumes_earlier_forward: true,
            ..
        } | Conclusion::ConfiguredRedirect {
            assumes_earlier_forward: true,
            ..
        }
    ) {
        // The diagnosis qualification already states this condition directly;
        // do not repeat it in a second section.
        return None;
    }
    if layers
        .iter()
        .any(|layer| layer.status() == LayerStatus::Unavailable)
    {
        return Some("One or more integrations were unavailable, so downstream behavior remains conditional.".into());
    }
    if layers
        .iter()
        .any(|layer| layer.status() == LayerStatus::Indeterminate)
    {
        return Some(
            "One or more layers remain uncertain, so downstream behavior is conditional.".into(),
        );
    }
    if layers
        .iter()
        .any(|layer| layer.propagation() == Propagation::Indeterminate)
    {
        return Some("At least one layer could not establish whether it forwards the key.".into());
    }
    None
}

fn next_step(
    _key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> Option<String> {
    if observation.is_some_and(|value| value.source.proves_compositor_receipt()) {
        return Some("Use --suppress only when you need to inspect a compositor binding without letting its normal action run.".into());
    }
    if matches!(
        conclusion,
        Conclusion::ConfiguredConsumer {
            assumes_earlier_forward: true,
            ..
        }
    ) {
        return Some(
            "Run `whykey listen` to observe whether the shortcut reaches the configured consumer."
                .into(),
        );
    }
    if layers
        .iter()
        .any(|layer| layer.status() == LayerStatus::Unavailable)
    {
        return Some("Run `whykey doctor` to see which integration is unavailable.".into());
    }
    None
}

fn render_verbose_evidence(
    output: &mut String,
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    options: RenderOptions,
) {
    output.push_str(&options.accent("Technical evidence"));
    output.push('\n');
    if let Some(observation) = observation {
        push_wrapped(
            output,
            &format!("source: {}", observation.source.label()),
            options,
            "  ",
            "  ",
        );
        push_wrapped(
            output,
            &format!("disposition: {:?}", observation.disposition),
            options,
            "  ",
            "  ",
        );
        push_wrapped(
            output,
            &format!("observed key: {}", style::human_key(&observation.combo)),
            options,
            "  ",
            "  ",
        );
        push_wrapped(
            output,
            &format!("event: {}", observation.event_type.label()),
            options,
            "  ",
            "  ",
        );
        let raw_display = observation
            .raw_display
            .clone()
            .unwrap_or_else(|| format_bytes(&observation.raw));
        let raw_label = if observation.source.confirms_terminal() {
            "probe bytes"
        } else {
            "raw event"
        };
        push_wrapped(
            output,
            &format!("{raw_label}: {raw_display}"),
            options,
            "  ",
            "  ",
        );
        push_wrapped(
            output,
            &format!("encoding: {}", observation.encoding),
            options,
            "  ",
            "  ",
        );
        push_wrapped(
            output,
            &format!(
                "protocol flags: {}",
                observation
                    .protocol_flags
                    .map_or_else(|| "unavailable".into(), |flags| format!("0x{flags:x}"))
            ),
            options,
            "  ",
            "  ",
        );
        if let Some(keycode) = observation.physical_keycode {
            push_wrapped(
                output,
                &format!("physical keycode: {keycode:?}"),
                options,
                "  ",
                "  ",
            );
        }
        if let Some(text) = &observation.associated_text {
            push_wrapped(
                output,
                &format!("associated text: {text}"),
                options,
                "  ",
                "  ",
            );
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
            push_wrapped(
                output,
                &format!("modifiers pressed: {pressed}"),
                options,
                "  ",
                "  ",
            );
            push_wrapped(
                output,
                &format!("modifiers locked: {locked}"),
                options,
                "  ",
                "  ",
            );
            if let Some(latched) = &state.latched {
                let latched = if latched.is_empty() {
                    "none".into()
                } else {
                    latched.join(", ")
                };
                push_wrapped(
                    output,
                    &format!("modifiers latched: {latched}"),
                    options,
                    "  ",
                    "  ",
                );
            } else {
                push_wrapped(
                    output,
                    "modifiers latched: unavailable from compositor",
                    options,
                    "  ",
                    "  ",
                );
            }
            for device in &state.devices {
                push_wrapped(
                    output,
                    &format!(
                        "modifier device: {} ({}) pressed={} locked={}",
                        device.device,
                        device.path,
                        device.pressed.join(", "),
                        device.locked.join(", ")
                    ),
                    options,
                    "  ",
                    "  ",
                );
            }
        } else {
            push_wrapped(output, "modifiers: unavailable", options, "  ", "  ");
        }
        if let Some(alternate) = &observation.alternate_keys {
            push_wrapped(
                output,
                &format!("alternate keys: {}", alternate.join(", ")),
                options,
                "  ",
                "  ",
            );
        } else if let Some(alternate) = &observation.alternate_key {
            push_wrapped(
                output,
                &format!("alternate key: {alternate}"),
                options,
                "  ",
                "  ",
            );
        }
    }
    push_wrapped(
        output,
        &format!("requested key: {}", style::human_key(key)),
        options,
        "  ",
        "  ",
    );
    output.push_str(if observation.is_some() {
        "  route evidence:\n"
    } else {
        "  configuration route (predicted):\n"
    });
    for (index, layer) in layers.iter().enumerate() {
        push_wrapped(
            output,
            &format!(
                "{}. {} [{:?}; {:?}]",
                index + 1,
                layer.layer,
                layer.status(),
                layer.propagation()
            ),
            options,
            "    ",
            "       ",
        );
        push_wrapped(
            output,
            &format!("summary: {}", layer.summary),
            options,
            "       ",
            "       ",
        );
        for detail in layer.all_details() {
            push_wrapped(
                output,
                &format!("evidence: {detail}"),
                options,
                "       ",
                "       ",
            );
        }
        if let Some(binding) = &layer.binding {
            output.push_str("       binding:\n");
            if let Some(dispatcher) = &binding.dispatcher {
                push_wrapped(
                    output,
                    &format!("dispatcher: {dispatcher}"),
                    options,
                    "         ",
                    "         ",
                );
            }
            if let Some(action) = &binding.action {
                push_wrapped(
                    output,
                    &format!("action: {action}"),
                    options,
                    "         ",
                    "         ",
                );
            }
            if let Some(description) = &binding.description {
                push_wrapped(
                    output,
                    &format!("description: {description}"),
                    options,
                    "         ",
                    "         ",
                );
            }
            if let Some(submap) = &binding.submap {
                push_wrapped(
                    output,
                    &format!("submap: {submap}"),
                    options,
                    "         ",
                    "         ",
                );
            }
            push_wrapped(
                output,
                &format!("scope: {:?}", binding.scope),
                options,
                "         ",
                "         ",
            );
            if let Some(source) = &binding.source {
                push_wrapped(
                    output,
                    &format!(
                        "source: {}{}",
                        source.file,
                        source
                            .line
                            .map_or_else(String::new, |line| format!(":{line}"))
                    ),
                    options,
                    "         ",
                    "         ",
                );
            }
            if let Some(uncertainty) = binding.uncertainty {
                push_wrapped(
                    output,
                    &format!("uncertainty: {uncertainty:?}"),
                    options,
                    "         ",
                    "         ",
                );
            }
        }
    }
    output.push('\n');
}

fn push_wrapped(
    output: &mut String,
    text: &str,
    options: RenderOptions,
    first: &str,
    continuation: &str,
) {
    if [
        "source:",
        "config:",
        "binding:",
        "action:",
        "evidence: source:",
        "evidence: config:",
        "evidence: binding:",
        "evidence: action:",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
    {
        output.push_str(first);
        output.push_str(text);
        output.push('\n');
        return;
    }
    for line in style::wrap_hanging(text, options.width, first, continuation) {
        output.push_str(&line);
        output.push('\n');
    }
}

fn compositor_candidates_note() -> Option<String> {
    if !candidate_detection_possible() {
        return None;
    }
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

fn candidate_detection_possible() -> bool {
    [
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "I3SOCK",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
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
    if layers.is_empty() {
        return Conclusion::NoLayersInspected;
    }
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
    } else {
        Conclusion::UnhandledContinues
    }
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

        assert!(output.contains("Ctrl+Left · Inspect"));
        assert!(output.contains("No inspected layer matches; it can continue."));
        assert!(output.contains("No matching binding"));
        assert!(output.contains("--verbose"));

        let verbose = render(&key, std::slice::from_ref(&layer), true);
        assert!(verbose.contains("active submap: default"));
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

        assert!(output.contains("Readline is configured to consume the shortcut."));
        assert!(output.contains("Readline — Configured to stop"));
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

        assert!(output.contains("This applies if earlier layers forward the key."));
        assert!(output.contains("Bash / Readline is configured to consume"));
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

        assert!(output.contains("Hyprland is configured to redirect the shortcut."));
        assert!(output.contains("Hyprland — Redirected"));
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

        assert!(
            output.contains("Hyprland has a matching binding and is configured to forward it.")
        );
        assert!(output.contains("Ghostty — No matching binding"));
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

        assert!(output.contains("Ghostty has a matching binding; forwarding is unknown."));
        assert!(output.contains("What remains uncertain"));
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

        assert!(output.contains("Hyprland — Unavailable: inspection failed"));
        assert!(output.contains("Hyprland could not be inspected."));
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

        assert!(output.contains("Hyprland — Unavailable"));
        assert!(output.contains("One or more integrations were unavailable"));
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

        assert!(output.contains("Observed via terminal (observe-only)"));
        assert!(output.contains("earlier forwarding is confirmed"));
        assert!(output.contains("probe bytes: ESC [ 1 ; 5 D"));
        let normal = render_observed(&observed, &layers, false);
        assert!(normal.contains("earlier forwarding is confirmed"));
        assert!(!normal.contains("probe bytes"));
        assert!(output.contains("Readline is configured to consume the shortcut."));
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
        assert!(output.contains("Observed via evdev (Test Keyboard; /dev/input/event0)"));
        assert!(output.contains("physical key event was observed via evdev"));
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
        assert!(output.contains("Hyprland"));
        assert!(output.contains("encoding: Hyprland XKB key event"));
        let normal = render_observed(&observed, &layers, false);
        assert!(normal.contains("Observed via Hyprland (pass-through)"));
        assert!(!normal.contains("encoding: Hyprland XKB key event"));
        assert!(!normal.contains("modifiers latched"));
        assert!(output.contains("The key was observed at Hyprland"));
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
        assert!(output.contains("Hyprland"));
        assert!(output.contains("Observed via Hyprland (suppressed)"));
        assert!(output.contains("temporarily intercepted the compositor event"));
        // An opaque dispatcher never becomes "executed": the conclusion may
        // name the configured action but must qualify it as unexecuted.
        assert!(output.contains("Hyprland has a matching binding; its runtime effect is unknown."));
        assert!(output.contains("Whykey temporarily intercepted"));
        assert!(!output.contains("executed"));
        assert!(!output.contains("would handle and consume"));
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

        assert!(output.contains("Hyprland has a matching binding; its runtime effect is unknown."));
        assert!(output.contains("temporarily intercepted the compositor event"));
        assert!(!output.contains("executed"));
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
        assert!(output.contains("Hyprland has a matching binding; its runtime effect is unknown."));
        assert!(output.contains("temporarily intercepted the compositor event"));
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
        assert!(output.contains("Hyprland — Forwarding unknown: active binding found"));
        assert!(!output.contains("active submap: default"));
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
        assert!(normal.contains("Hyprland — Configured to stop: Herdr"));
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
            diagnosis_line(&normal),
            diagnosis_line(&verbose),
            "verbosity changes evidence depth, never the diagnosis"
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
        let assessment = if text.contains("conditional") {
            "conditional"
        } else if text.contains("confirmed") {
            "confirmed"
        } else {
            "configured"
        };
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
        assert!(output.contains("Input method — Forwarding unknown"));
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

    fn diagnosis_line(output: &str) -> Option<&str> {
        output.lines().skip(2).find(|line| !line.is_empty())
    }

    #[test]
    fn detail_wording_never_changes_the_diagnosis() {
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
            diagnosis_line(&render_observed(&observed, &plain, false)),
            diagnosis_line(&render_observed(&observed, &reworded, false)),
            "the diagnosis follows typed evidence, not detail wording"
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

        assert!(output.contains("Sequence · Inspect"));
        assert!(output.contains("Step 1/2 · Ctrl+X"));
        assert!(output.contains("Step 2/2 · Ctrl+S"));
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
        assert!(output.contains("Hyprland — Forwarding unknown: active binding found"));
        assert!(!output.contains("Result:"));
        assert!(output.contains("Shell input — Unavailable"));
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
        assert!(output.contains("tmux has a candidate binding, but delivery is unverified."));
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
        assert!(output.contains("Neovim has a matching keymap, but execution is unverified."));
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

    #[test]
    fn ctrl_z_leads_with_tty_suspend_diagnosis_and_keeps_exact_evidence_verbose() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layers = [
            LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unknown,
                "compositor forwarding is unknown",
                vec![],
            ),
            LayerResult::new(
                "TTY driver",
                LayerId::Tty,
                Outcome::Consumed,
                "interprets byte 0x1a as VSUSP",
                vec![
                    "special character: VSUSP = ^Z".into(),
                    "byte: 0x1a".into(),
                    "kernel sends SIGTSTP to the foreground process".into(),
                ],
            ),
        ];

        let normal = render_with_options(&key, &layers, RenderOptions::plain(false));
        assert!(normal.contains("TTY is configured to suspend the foreground job."));
        assert!(normal.contains("This applies if earlier layers forward the key."));
        assert!(normal.contains("TTY driver — Configured to stop: terminal signal handling"));
        assert!(!normal.contains("special character: VSUSP"));

        let verbose = render_with_options(&key, &layers, RenderOptions::plain(true));
        for evidence in ["VSUSP", "0x1a", "SIGTSTP"] {
            assert!(
                verbose.contains(evidence),
                "verbose output must retain {evidence}"
            );
        }
    }

    #[test]
    fn compositor_capture_with_opaque_binding_names_unknown_forwarding() {
        let key: KeyCombo = "super+k".parse().unwrap();
        let observed = ObservedKey {
            combo: key.clone(),
            raw: vec![],
            raw_display: Some("Hyprland XKB keycode=45 (press)".into()),
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
            disposition: crate::listen::CaptureDisposition::PassedThrough,
        };
        let layer = LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching binding; runtime effect unknown",
            vec![],
        )
        .with_binding(BindingEvidence {
            dispatcher: Some("__lua".into()),
            action: Some("__lua 42".into()),
            description: Some("Open launcher".into()),
            submap: Some("default".into()),
            scope: BindingScope::Submap("default".into()),
            source: Some(crate::layers::SourceLocation::new(
                "/tmp/hyprland.conf",
                Some(12),
            )),
            has_universal_match: false,
            uncertainty: Some(UncertaintyReason::OpaqueDispatcher),
        });

        let output = render_observed_with_options(&observed, &[layer], RenderOptions::plain(false));
        assert!(output.contains("Super+K · Listen"));
        assert!(output.contains("Hyprland has a matching binding; forwarding is unknown."));
        assert!(output.contains("The key was observed at Hyprland."));
        assert!(output.contains("Open launcher"));
        assert!(output.contains(
            "Whykey cannot determine whether the Lua/plugin action passes the key onward."
        ));
        assert!(!output.contains("executed"));
        assert!(!output.contains("blocked"));
    }

    #[test]
    fn narrow_and_color_modes_keep_text_readable_without_touching_json() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layer = LayerResult::new(
            "TTY driver",
            LayerId::Tty,
            Outcome::Consumed,
            "interprets byte 0x1a as VSUSP",
            vec!["special character: VSUSP = ^Z".into()],
        );
        let narrow =
            render_with_options(&key, &[layer], RenderOptions::plain(false).with_width(60));
        assert!(
            narrow
                .lines()
                .all(|line| crate::style::display_width(line) <= 60)
        );
        assert!(!narrow.contains("\x1b["));
        let colored = render_with_options(
            &key,
            &[],
            RenderOptions {
                color: crate::style::ColorChoice::Always,
                verbose: false,
                width: 80,
            },
        );
        assert!(colored.contains("\x1b[1m"));
        let json = render_json(&key, &[], None, 1);
        assert!(!json.contains("\x1b["));
    }
}
