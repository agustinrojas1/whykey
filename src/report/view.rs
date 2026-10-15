//! The human report: verdict, route table, binding evidence, confidence and
//! next steps. Everything here is presentation. The typed conclusion comes
//! from `evaluate_conclusion` and is never changed by wording.

use super::*;
use crate::style::{Mark, TableRow};

/// Column the report's panel (header, table) is laid out in. Prose wraps at
/// the real terminal width; the table stays readable on wide terminals.
const PANEL_WIDTH: usize = 80;

struct Verdict {
    mark: Mark,
    headline: String,
    lines: Vec<String>,
}

/// Friendly short names for layers whose internal name is a program pair.
pub(super) fn short_name(layer: &str) -> String {
    match layer {
        "Bash / Readline" => "bash".into(),
        "Zsh / ZLE" => "zsh".into(),
        "Fish" => "fish".into(),
        other => other.to_owned(),
    }
}

/// The `whykey <combo>` spelling of a key, as the user would type it.
fn combo_arg(key: &KeyCombo) -> String {
    let mut parts: Vec<String> = style::human_modifiers(key.modmask())
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    parts.push(key.key().to_ascii_lowercase());
    let arg = parts.join("+");
    if key.is_alone() {
        format!("{arg} --alone")
    } else {
        arg
    }
}

fn find_layer<'a>(layers: &'a [LayerResult], name: &str) -> Option<&'a LayerResult> {
    layers.iter().find(|layer| layer.layer == name)
}

fn action_name(layer: &LayerResult) -> Option<String> {
    layer.binding.as_ref().and_then(|binding| {
        binding
            .description
            .clone()
            .or_else(|| binding.action.clone())
    })
}

fn names(layers: &[&str]) -> String {
    let short: Vec<String> = layers
        .iter()
        .map(|name| match *name {
            "TTY driver" => "the TTY driver".to_owned(),
            "Input method" => "the input method".to_owned(),
            other => short_name(other),
        })
        .collect();
    let refs: Vec<&str> = short.iter().map(String::as_str).collect();
    human_list(&refs, "and")
}

fn opaque_kind(binding: &BindingEvidence) -> &'static str {
    match binding.dispatcher.as_deref() {
        Some("__lua") => "Lua",
        _ => "plugin",
    }
}

fn static_verdict(key: &KeyCombo, layers: &[LayerResult], conclusion: &Conclusion) -> Verdict {
    let key_text = style::human_key(key);
    let verdict = |mark, headline: String, lines: Vec<String>| Verdict {
        mark,
        headline,
        lines,
    };
    match conclusion {
        Conclusion::Remapped {
            remapper,
            target,
            then,
        } => {
            let headline = format!("{remapper} remaps {key_text} to {target}.");
            match then {
                Some(then) => {
                    let inner = static_verdict(key, layers, then);
                    let mut lines = vec![format!("Then: {}", inner.headline)];
                    lines.extend(inner.lines);
                    verdict(Mark::Remap, headline, lines)
                }
                None => verdict(
                    Mark::Remap,
                    headline,
                    vec!["Whykey cannot name the key that results on this layout, so later layers were not evaluated for it.".into()],
                ),
            }
        }
        Conclusion::ConfiguredConsumer { layer, .. } => {
            let found = find_layer(layers, layer);
            if *layer == "TTY driver"
                && found.is_some_and(|candidate| {
                    candidate
                        .details
                        .iter()
                        .any(|detail| detail.contains("VSUSP"))
                })
            {
                return verdict(
                    Mark::Stop,
                    format!(
                        "The TTY driver turns {key_text} into a stop signal for the foreground job."
                    ),
                    vec![format!("Nothing after the TTY driver receives {key_text}.")],
                );
            }
            let who = short_name(layer);
            let headline = match found.and_then(action_name) {
                Some(action) if *layer == "TTY driver" => {
                    format!("The TTY driver handles {key_text} itself: {action}.")
                }
                Some(action) => format!("{who} takes {key_text} and runs {action}."),
                None => format!("{who} takes {key_text}."),
            };
            verdict(
                Mark::Stop,
                headline,
                vec![format!("Nothing after {who} receives {key_text}.")],
            )
        }
        Conclusion::DeadKeyComposition { layer, keysym, .. } => verdict(
            Mark::Remap,
            format!(
                "{key_text} is a dead key ({keysym}); {} composes it with the next key.",
                short_name(layer)
            ),
            vec!["Nothing is sent until the next key completes or cancels the composition.".into()],
        ),
        Conclusion::ConfiguredRedirect { layer, .. } => verdict(
            Mark::Remap,
            format!("{} sends {key_text} to another window.", short_name(layer)),
            vec!["The focused window does not receive it.".into()],
        ),
        Conclusion::SelectedApplication { layer, status } => {
            let who = short_name(layer);
            let headline = match status {
                ApplicationStatus::Unavailable => format!("Whykey could not inspect {who}."),
                ApplicationStatus::UnresolvedMode | ApplicationStatus::UnverifiedExecution => {
                    format!("{who} has a matching keymap, but whykey cannot verify it runs.")
                }
                ApplicationStatus::InteractiveGeneric(_) => {
                    format!("{who} is the selected application; its shortcuts are not verified.")
                }
                ApplicationStatus::NoMatchingKeymap => format!("{who} has no matching keymap."),
            };
            let mark = if matches!(status, ApplicationStatus::NoMatchingKeymap) {
                Mark::Pass
            } else {
                Mark::Unsure
            };
            verdict(mark, headline, Vec::new())
        }
        Conclusion::UnverifiedSession { layer } => verdict(
            Mark::Unsure,
            format!(
                "{} has a candidate binding, but whykey cannot verify the key reaches it.",
                short_name(layer)
            ),
            Vec::new(),
        ),
        Conclusion::UnverifiedDefault {
            layer,
            action,
            source,
        } => {
            let who = short_name(layer);
            let headline = match source {
                Some(file) => format!("Probably {who}: {file} binds {key_text} to {action}."),
                None => format!("Probably {who}: its default binds {key_text} to {action}."),
            };
            verdict(
                Mark::Unsure,
                headline,
                vec!["Unverified. Your shell configuration may override it.".into()],
            )
        }
        Conclusion::PossibleUnresolvedMatch {
            layer,
            consumes_if_matched,
            ..
        } => {
            let who = short_name(layer);
            let count = find_layer(layers, layer)
                .and_then(|found| found.binding.as_ref())
                .and_then(|binding| match binding.uncertainty {
                    Some(UncertaintyReason::UnresolvedBindingKey { candidates, .. }) => {
                        Some(candidates)
                    }
                    _ => None,
                });
            let headline = match count {
                Some(1) => format!("{who} has 1 binding whose key whykey could not read."),
                Some(count) => {
                    format!("{who} has {count} bindings whose keys whykey could not read.")
                }
                None => format!("{who} has bindings whose keys whykey could not read."),
            };
            let line = if *consumes_if_matched {
                format!(
                    "If one of them uses {key_text}, {who} takes it and nothing after receives it."
                )
            } else {
                format!(
                    "If one of them uses {key_text}, {who} handles it; whether it continues is unknown."
                )
            };
            verdict(Mark::Unsure, headline, vec![line])
        }
        Conclusion::HandledAndForwarded {
            layers: handled,
            uncertain,
            forwarding_unknown,
            ..
        } => {
            let who = names(handled);
            let opaque = handled
                .iter()
                .filter_map(|name| find_layer(layers, name))
                .filter_map(|found| found.binding.as_ref())
                .find(|binding| binding.is_opaque());
            if let Some(binding) = opaque {
                return verdict(
                    Mark::Unsure,
                    format!(
                        "{who} matches {key_text}, but its {} action is opaque to whykey.",
                        opaque_kind(binding)
                    ),
                    vec!["Whykey cannot tell whether it passes the key on.".into()],
                );
            }
            if *forwarding_unknown {
                verdict(
                    Mark::Unsure,
                    format!(
                        "{who} matches {key_text}, but whykey cannot tell whether it continues."
                    ),
                    Vec::new(),
                )
            } else if *uncertain {
                verdict(
                    Mark::Unsure,
                    format!("{who} handles {key_text} and passes it on."),
                    vec!["Some later layers could not be verified.".into()],
                )
            } else {
                verdict(
                    Mark::Pass,
                    format!("{who} handles {key_text} and passes it on."),
                    vec!["It continues to the next layer.".into()],
                )
            }
        }
        Conclusion::CouldNotInspect { layer } => verdict(
            Mark::Unsure,
            format!("Whykey could not inspect {}.", short_name(layer)),
            Vec::new(),
        ),
        Conclusion::ModifierAmbiguity { layer } => verdict(
            Mark::Unsure,
            format!(
                "{} has bindings on this key, but whykey cannot tell which modifiers they need.",
                short_name(layer)
            ),
            Vec::new(),
        ),
        Conclusion::IndeterminateForwarding { layer } => verdict(
            Mark::Unsure,
            format!(
                "{} matches {key_text}, but whykey cannot tell whether it continues.",
                short_name(layer)
            ),
            Vec::new(),
        ),
        Conclusion::NoBindingReported { unknown } => verdict(
            Mark::Unsure,
            format!(
                "Nothing claims {key_text} before {}, which whykey could not verify.",
                names(unknown)
            ),
            vec!["What those layers do with it is unknown.".into()],
        ),
        Conclusion::NoLayersInspected => {
            verdict(Mark::Unsure, "No layers were inspected.".into(), Vec::new())
        }
        Conclusion::UnhandledContinues => verdict(
            Mark::Pass,
            format!("Nothing claims {key_text}; it continues to the application."),
            vec!["No inspected layer has a binding for it.".into()],
        ),
        Conclusion::EndpointUnbound { layer, effect, .. } => {
            let mut effect = effect.trim_end_matches('.').to_owned();
            if let Some(first) = effect.get(..1) {
                effect.replace_range(..1, &first.to_ascii_uppercase());
            }
            verdict(
                Mark::Pass,
                format!("{} has no binding for {key_text}.", short_name(layer)),
                vec![format!("{effect}.")],
            )
        }
        Conclusion::SuppressedHandled { .. } => verdict(
            Mark::Unsure,
            "A matching compositor binding was captured for inspection.".into(),
            Vec::new(),
        ),
    }
}

fn verdict_for(
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> Verdict {
    let mut verdict = static_verdict(key, layers, conclusion);
    if observation.is_some() {
        // A capture changes what is known; its own wording leads the report.
        verdict.headline = diagnosis_text(key, layers, observation, conclusion);
        verdict.lines = vec![effect_text(key, layers, observation, conclusion)];
        if matches!(conclusion, Conclusion::SuppressedHandled { .. }) {
            verdict.mark = Mark::Unsure;
        }
    }
    if let Some(qualification) = qualification_text(key, layers, observation, conclusion) {
        verdict.lines.push(qualification);
    }
    verdict
}

/// Environment shown beside the combination: compositor, session type,
/// shell, and the terminal when it is known.
pub(super) fn join_context(parts: &[Option<String>]) -> String {
    parts
        .iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

fn header_context(layers: &[LayerResult]) -> String {
    let compositor = layers
        .iter()
        .find(|layer| layer.id == LayerId::Compositor)
        .map(|layer| layer.layer.to_owned());
    let session = crate::recording::env_var("XDG_SESSION_TYPE")
        .ok()
        .map(|value| match value.as_str() {
            "wayland" => "Wayland".to_owned(),
            "x11" => "X11".to_owned(),
            "tty" => "TTY".to_owned(),
            other => other.to_owned(),
        })
        .or_else(|| {
            crate::recording::env_var_os("WAYLAND_DISPLAY")
                .map(|_| "Wayland".to_owned())
                .or_else(|| crate::recording::env_var_os("DISPLAY").map(|_| "X11".to_owned()))
        });
    let shell = layers
        .iter()
        .find(|layer| layer.id == LayerId::Shell)
        .map(|layer| short_name(layer.layer))
        .or_else(|| {
            crate::recording::env_var("SHELL").ok().and_then(|path| {
                path.rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
        });
    let terminal = crate::layers::terminal_identity().map(str::to_owned);
    join_context(&[compositor, session, shell, terminal])
}

fn push_header(output: &mut String, headline: &str, context: &str, options: RenderOptions) {
    let panel = options.width.min(PANEL_WIDTH);
    let left = style::display_width(headline);
    let right = style::display_width(context);
    output.push_str(&options.bold(headline));
    if context.is_empty() {
        output.push('\n');
    } else if left + 2 + right <= panel {
        output.push_str(&" ".repeat(panel - left - right));
        output.push_str(&options.muted(context));
        output.push('\n');
    } else {
        output.push('\n');
        output.push_str(&options.muted(context));
        output.push('\n');
    }
}

/// How one layer's answer is described in a single short phrase.
fn short_reason(layer: &LayerResult) -> String {
    match layer.summary.trim() {
        "no active binding found" => "no binding".into(),
        "no tmux, GNU Screen, or Zellij session detected" => "none detected".into(),
        "input method was detected but is inactive; keys pass through unchanged" => {
            "detected, inactive; keys pass through".into()
        }
        other => concise_summary(other.trim_end_matches('.')),
    }
}

struct Row {
    name: String,
    mark: Mark,
    outcome: Option<Outcome>,
    from_remapper: bool,
    detail: String,
    trailing: String,
    notes: Vec<String>,
}

fn tier_word(tier: EvidenceTier) -> &'static str {
    match tier {
        EvidenceTier::Observed => "observed",
        EvidenceTier::LiveQuery => "live query",
        EvidenceTier::ConfigFile => "config file",
        EvidenceTier::Default => "default",
    }
}

fn layer_row(
    layer: &LayerResult,
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
) -> Row {
    let reason = layer
        .binding
        .as_ref()
        .and_then(|binding| binding.uncertainty);
    let unverified_default = reason == Some(UncertaintyReason::UnverifiedDefault);
    let mark = match (reason, layer.outcome) {
        (Some(UncertaintyReason::UnverifiedDefault), _)
        | (Some(UncertaintyReason::UnresolvedBindingKey { .. }), _) => Mark::Unsure,
        (_, Outcome::Consumed) if layer.dead_key.is_some() => Mark::Remap,
        (_, Outcome::Pass | Outcome::HandledAndPassed) => Mark::Pass,
        (_, Outcome::Consumed) => Mark::Stop,
        (_, Outcome::Redirected) => Mark::Remap,
        _ => Mark::Unsure,
    };
    let mut notes = Vec::new();
    let detail = if let Some(UncertaintyReason::UnresolvedBindingKey { candidates, .. }) = reason {
        format!(
            "{candidates} binding{} with unreported keys",
            if candidates == 1 { "" } else { "s" }
        )
    } else if let Some(keysym) = &layer.dead_key {
        format!("{keysym} starts composition; no bytes until the next key")
    } else if let Some(binding) = &layer.binding {
        let name = |binding: &BindingEvidence| {
            binding
                .description
                .clone()
                .or_else(|| binding.action.clone())
        };
        let first = name(binding).unwrap_or_else(|| concise_summary(&layer.summary));
        let also: Vec<String> = layer.also_fires.iter().filter_map(name).collect();
        if !also.is_empty() {
            notes.push(format!("also: {}", also.join("; ")));
        }
        first
    } else if observation.is_some_and(|value| {
        value.source.proves_compositor_receipt() && layer.id == LayerId::Compositor
    }) {
        "event received at the compositor".into()
    } else if layer.outcome == Outcome::Pass {
        short_reason(layer)
    } else {
        concise_summary(layer.summary.trim_end_matches('.'))
    };
    let trailing = if unverified_default {
        if layer
            .binding
            .as_ref()
            .is_some_and(|binding| binding.source.is_some())
        {
            "startup file, unverified".to_owned()
        } else {
            "default, unverified".to_owned()
        }
    } else {
        evidence_tier(layer, observation).map_or_else(String::new, |tier| tier_word(tier).into())
    };
    let _ = terminal_observed;
    let name = if layer.outcome == Outcome::Pass
        && matches!(layer.id, LayerId::Session | LayerId::Multiplexer)
        && layer.layer == "Session context"
    {
        "Multiplexer".to_owned()
    } else if layer.id == LayerId::Terminal && layer.summary.starts_with("not applicable") {
        "Terminal".to_owned()
    } else {
        short_name(layer.layer)
    };
    Row {
        name,
        mark,
        outcome: Some(layer.outcome),
        from_remapper: layer.id == LayerId::Remapper,
        detail,
        trailing,
        notes,
    }
}

/// The seven route positions in order. A position nothing answered for is
/// still listed, so the table always shows the whole chain.
fn route_rows(
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    terminal_observed: bool,
) -> Vec<Row> {
    struct Group {
        ids: &'static [LayerId],
        generic: &'static str,
        optional: bool,
    }
    const GROUPS: &[Group] = &[
        Group {
            ids: &[LayerId::Remapper],
            generic: "Remapper",
            optional: false,
        },
        Group {
            ids: &[LayerId::Compositor],
            generic: "Compositor",
            optional: false,
        },
        Group {
            ids: &[LayerId::Terminal],
            generic: "Terminal",
            optional: false,
        },
        Group {
            ids: &[LayerId::Tty],
            generic: "TTY driver",
            optional: false,
        },
        Group {
            ids: &[LayerId::Ime],
            generic: "Input method",
            optional: false,
        },
        Group {
            ids: &[LayerId::Multiplexer, LayerId::Session],
            generic: "Multiplexer",
            optional: false,
        },
        Group {
            ids: &[LayerId::Application],
            generic: "Application",
            optional: true,
        },
        Group {
            ids: &[LayerId::Shell],
            generic: "Shell",
            optional: false,
        },
        Group {
            ids: &[LayerId::Diagnostic, LayerId::Unknown],
            generic: "Other",
            optional: true,
        },
    ];
    let mut rows: Vec<Row> = Vec::new();
    for group in GROUPS {
        let members: Vec<&LayerResult> = layers
            .iter()
            .filter(|layer| group.ids.contains(&layer.id))
            // An application with nothing to say is verbose-only context.
            .filter(|layer| group.ids != [LayerId::Application] || layer.outcome != Outcome::Pass)
            .collect();
        if members.is_empty() {
            if group.optional {
                continue;
            }
            let remapper = group.ids == [LayerId::Remapper];
            rows.push(Row {
                name: group.generic.to_owned(),
                mark: if remapper { Mark::Pass } else { Mark::Off },
                outcome: None,
                from_remapper: false,
                detail: if remapper {
                    "none detected".into()
                } else {
                    "not inspected".into()
                },
                trailing: String::new(),
                notes: Vec::new(),
            });
            continue;
        }
        for layer in members {
            rows.push(layer_row(layer, observation, terminal_observed));
        }
    }
    // Everything after a layer that keeps (or redirects) the key is never
    // reached. A remapper rewrites the key instead; the route continues.
    let stop = rows.iter().position(|row| {
        !row.from_remapper && matches!(row.outcome, Some(Outcome::Consumed | Outcome::Redirected))
    });
    if let Some(stop) = stop {
        for row in rows.iter_mut().skip(stop + 1) {
            row.mark = Mark::Off;
            row.detail = "not reached".into();
            row.trailing.clear();
            row.notes.clear();
        }
    }
    rows
}

/// The layer whose answer decides the verdict: what stops or redirects the
/// key, else the first match, else an answer that is only a default.
fn deciding_layer(layers: &[LayerResult]) -> Option<&LayerResult> {
    layers
        .iter()
        .find(|layer| matches!(layer.outcome, Outcome::Consumed | Outcome::Redirected))
        .or_else(|| {
            layers
                .iter()
                .find(|layer| layer.status() == LayerStatus::Handled)
        })
        .or_else(|| {
            layers.iter().find(|layer| {
                matches!(
                    layer
                        .binding
                        .as_ref()
                        .and_then(|binding| binding.uncertainty),
                    Some(UncertaintyReason::UnverifiedDefault)
                        | Some(UncertaintyReason::UnresolvedBindingKey { .. })
                )
            })
        })
}

fn bound_in_lines(layer: &LayerResult) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(binding) = &layer.binding {
        if let Some(source) = &binding.source {
            lines.push(match source.line {
                Some(line) => format!("{}:{line}", source.file),
                None => source.file.clone(),
            });
        }
        match (&binding.dispatcher, &binding.action) {
            (Some(dispatcher), Some(action)) if binding.is_opaque() => {
                let command = if action.starts_with(dispatcher.as_str()) {
                    action.clone()
                } else {
                    format!("{dispatcher} {action}")
                };
                lines.push(format!(
                    "{command} ({} dispatcher; whykey does not execute it)",
                    opaque_kind(binding)
                ));
            }
            (Some(dispatcher), Some(action)) if action.starts_with(dispatcher.as_str()) => {
                lines.push(action.clone());
            }
            (Some(dispatcher), Some(action)) => lines.push(format!("{dispatcher} {action}")),
            (Some(dispatcher), None) => lines.push(dispatcher.clone()),
            _ => {}
        }
        match &binding.scope {
            BindingScope::Universal => lines.push("applies in every submap".into()),
            BindingScope::Submap(name) if name != "default" => {
                lines.push(format!("submap {name}"));
            }
            BindingScope::Submap(_) => {}
            BindingScope::Unknown => {}
        }
    }
    // Typed binding fields lead; a tagged detail (`sequence: ESC f`) only
    // fills in when the binding has nothing to say or names the sequence.
    if let Some(detail) = concise_detail_evidence(layer) {
        if lines.is_empty() || detail.starts_with("sequence:") {
            lines.push(detail);
        }
    }
    lines
}

fn confidence_text(
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    decider: Option<&LayerResult>,
) -> Option<String> {
    if let Some(observation) = observation {
        if observation.disposition != crate::listen::CaptureDisposition::NotDelivered {
            return Some(format!(
                "observed: whykey saw the key at {}. Layers after that point are predicted from configuration.",
                observation.source.label()
            ));
        }
    }
    let Some(layer) = decider else {
        let unavailable: Vec<&str> = layers
            .iter()
            .filter(|layer| layer.status() == LayerStatus::Unavailable)
            .map(|layer| layer.layer)
            .collect();
        if !unavailable.is_empty() {
            return Some(format!(
                "incomplete: {} could not be inspected, and no other layer reported a binding.",
                names(&unavailable)
            ));
        }
        let weakest = layers
            .iter()
            .filter_map(|layer| evidence_tier(layer, observation))
            .max_by_key(|tier| match tier {
                EvidenceTier::Observed => 0,
                EvidenceTier::LiveQuery => 1,
                EvidenceTier::ConfigFile => 2,
                EvidenceTier::Default => 3,
            })?;
        let caveat = match weakest {
            EvidenceTier::Default | EvidenceTier::ConfigFile => " Your configuration may differ.",
            _ => "",
        };
        return Some(format!(
            "{}: no layer reported a binding.{caveat}",
            tier_word(weakest)
        ));
    };
    let who = short_name(layer.layer);
    let unverified_default = layer
        .binding
        .as_ref()
        .and_then(|binding| binding.uncertainty)
        == Some(UncertaintyReason::UnverifiedDefault);
    let file = layer
        .binding
        .as_ref()
        .and_then(|binding| binding.source.as_ref())
        .map(|source| source.file.clone());
    if unverified_default {
        return Some(match file {
            Some(file) => format!(
                "startup file: read from {file}; the running {who} may have rebound the key."
            ),
            None => {
                format!("default: {who}'s built-in default; your configuration may override it.")
            }
        });
    }
    Some(match evidence_tier(layer, observation) {
        Some(EvidenceTier::Observed) => {
            format!("observed: {who} was seen passing the key.")
        }
        Some(EvidenceTier::LiveQuery) => {
            let scope = match layer.binding.as_ref().map(|binding| &binding.scope) {
                Some(BindingScope::Universal) => ", in every submap".to_owned(),
                Some(BindingScope::Submap(name)) => format!(", in submap {name}"),
                _ => String::new(),
            };
            format!(
                "live query: {who} answered from the running program{scope}. Not observed: run `whykey listen` to watch it."
            )
        }
        Some(EvidenceTier::ConfigFile) => match file {
            Some(file) => format!(
                "config file: read from {file}; whykey did not confirm the running {who} loaded it."
            ),
            None => format!(
                "config file: read from configuration; whykey did not confirm the running {who} loaded it."
            ),
        },
        Some(EvidenceTier::Default) => {
            format!("default: {who}'s built-in default; your configuration may override it.")
        }
        None if layer.status() == LayerStatus::Unavailable => {
            format!("none: whykey could not inspect {who}.")
        }
        None => return None,
    })
}

/// One executable command and why to run it.
struct Step {
    command: String,
    reason: String,
}

fn next_steps_for(
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    conclusion: &Conclusion,
) -> Vec<Step> {
    if observation.is_some_and(|value| value.source.proves_compositor_receipt()) {
        return vec![Step {
            command: "whykey listen --suppress".into(),
            reason: "inspect a binding without letting its action run".into(),
        }];
    }
    if let Conclusion::Remapped {
        then: Some(then), ..
    } = conclusion
    {
        return next_steps_for(key, layers, observation, then);
    }
    let mut steps = Vec::new();
    let observe_desktop = observation.is_none() && layers.iter().any(desktop_unsettled);
    if observe_desktop {
        steps.push(Step {
            command: "whykey listen --window".into(),
            reason: "press the shortcut in the probe window to see if it arrives".into(),
        });
    }
    if let Some(command) = layers
        .iter()
        .filter(|layer| {
            layer.id == LayerId::Shell
                && matches!(
                    evidence_tier(layer, observation),
                    Some(EvidenceTier::Default | EvidenceTier::ConfigFile)
                )
        })
        .find_map(|layer| shell_init_command(layer.layer))
    {
        steps.push(Step {
            command: command.to_owned(),
            reason: format!("then run whykey {} again", combo_arg(key)),
        });
    }
    if !observe_desktop
        && matches!(
            conclusion,
            Conclusion::ConfiguredConsumer {
                assumes_earlier_forward: true,
                layer,
            } if layers.iter().any(|candidate| candidate.layer == *layer)
        )
    {
        steps.push(Step {
            command: "whykey listen".into(),
            reason: "see whether the key reaches the layer that takes it".into(),
        });
    }
    if layers.iter().any(|layer| {
        layer.status() == LayerStatus::Unavailable
            && !(observe_desktop && layer.id == LayerId::Compositor)
    }) {
        steps.push(Step {
            command: "whykey doctor".into(),
            reason: "see which integration is unavailable".into(),
        });
    }
    steps.truncate(2);
    steps
}

fn push_prose(
    output: &mut String,
    text: &str,
    options: RenderOptions,
    first: &str,
    continuation: &str,
    paint: impl Fn(RenderOptions, String) -> String,
) {
    for line in style::wrap_hanging(text, options.width, first, continuation) {
        output.push_str(&paint(options, line));
        output.push('\n');
    }
}

pub(super) fn render_report(
    key: &KeyCombo,
    layers: &[LayerResult],
    observation: Option<&ObservedKey>,
    options: RenderOptions,
    include_header: bool,
    key_symbol: Option<&str>,
) -> String {
    let terminal_observed = observation.is_some_and(|value| value.source.confirms_terminal());
    let evaluated = evaluate_conclusion(layers, observation, terminal_observed);
    let verdict = verdict_for(key, layers, observation, &evaluated.conclusion);
    let unsure = verdict.mark == Mark::Unsure;
    let mut output = String::new();

    if include_header {
        let headline = match key_symbol {
            Some(symbol) => style::human_key(&KeyCombo::from_parts(
                key.modmask(),
                crate::key::normalize_key_str(symbol),
            )),
            None => style::human_key(key),
        };
        let context = if options.context {
            header_context(layers)
        } else {
            String::new()
        };
        push_header(&mut output, &headline, &context, options);
        output.push('\n');
    }

    let mark = options.mark(verdict.mark);
    let headline_lines =
        style::wrap_hanging(&verdict.headline, options.width.saturating_sub(2), "", "");
    for (index, line) in headline_lines.iter().enumerate() {
        if index == 0 {
            output.push_str(&format!("{mark} {}", options.bold(line)));
        } else {
            output.push_str(&format!("  {}", options.bold(line)));
        }
        output.push('\n');
    }
    for line in &verdict.lines {
        push_prose(&mut output, line, options, "  ", "  ", |options, text| {
            if unsure {
                options.uncertain(text)
            } else {
                text
            }
        });
    }
    if let Some(observation) = observation {
        let capture_mode = match observation.disposition {
            crate::listen::CaptureDisposition::Suppressed => "suppressed",
            crate::listen::CaptureDisposition::PassedThrough => "pass-through",
            crate::listen::CaptureDisposition::ObservedOnly => "observe-only",
            crate::listen::CaptureDisposition::NotDelivered => "key not delivered",
        };
        let observed_line = format!(
            "Observed via {} ({capture_mode}).",
            observation.source.label()
        );
        push_prose(&mut output, &observed_line, options, "  ", "  ", |o, t| {
            o.muted(t)
        });
    }
    output.push('\n');

    // Route: the whole chain, in order.
    output.push_str(&options.accent("Route"));
    output.push('\n');
    let rows = route_rows(layers, observation, terminal_observed);
    let table: Vec<TableRow> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| TableRow {
            cells: vec![
                (index + 1).to_string(),
                row.name.clone(),
                options.mark(row.mark),
            ],
            detail: row.detail.clone(),
            trailing: if row.trailing.is_empty() {
                String::new()
            } else {
                options.muted(&row.trailing)
            },
            notes: row.notes.clone(),
        })
        .collect();
    for line in style::layout_table(&table, options.width.min(PANEL_WIDTH), "  ") {
        output.push_str(&line);
        output.push('\n');
    }
    output.push('\n');

    let decider = deciding_layer(layers);
    if let Some(bound) = decider
        .map(bound_in_lines)
        .filter(|lines| !lines.is_empty())
    {
        output.push_str(&options.accent("Bound in"));
        output.push('\n');
        for line in bound {
            // Paths stay whole: no wrapping, one value per line.
            output.push_str("  ");
            output.push_str(&line);
            output.push('\n');
        }
        output.push('\n');
    }

    if let Some(text) = confidence_text(layers, observation, decider) {
        push_prose(
            &mut output,
            &text,
            options,
            "Confidence  ",
            "            ",
            |_, line| line,
        );
        output.push('\n');
    }

    let steps = next_steps_for(key, layers, observation, &evaluated.conclusion);
    if !steps.is_empty() {
        output.push_str(&options.accent(if unsure { "To confirm" } else { "Next" }));
        output.push('\n');
        // Commands are never wrapped; the reason takes the rest of the line
        // or moves under it when the terminal is narrow.
        for step in &steps {
            let used = 2 + style::display_width(&step.command) + 2;
            if used + style::display_width(&step.reason) <= options.width {
                output.push_str(&format!(
                    "  {}  {}\n",
                    step.command,
                    options.muted(&step.reason)
                ));
            } else {
                output.push_str(&format!("  {}\n", step.command));
                push_prose(
                    &mut output,
                    &step.reason,
                    options,
                    "    ",
                    "    ",
                    |o, t| o.muted(t),
                );
            }
        }
        output.push('\n');
    }

    if options.verbose {
        render_verbose_evidence(&mut output, key, layers, observation, options, key_symbol);
        if let Some(note) = options.context.then(compositor_candidates_note).flatten() {
            output.push_str(&options.muted(note));
        }
    } else if include_header {
        let more = if observation.is_some() {
            "whykey listen -v".to_owned()
        } else if unsure
            && !steps
                .iter()
                .any(|step| step.command.starts_with("whykey listen"))
        {
            format!("whykey {} -v  \u{b7}  whykey listen", combo_arg(key))
        } else {
            format!("whykey {} -v", combo_arg(key))
        };
        output.push_str(&format!("{}  {}\n", options.accent("More"), more));
    }
    options.fold(output)
}
