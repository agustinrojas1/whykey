//! What a bisecting capture adds to the report: the key was seen at one
//! capture point and a later one did, or did not, get it. Wording only; the
//! facts (`ObservedKey::onward`) are recorded by `listen`.

use super::*;
use crate::listen::{CaptureSource, Onward};

/// The headline and supporting lines for a key taken between two capture
/// points. `None` unless a later point never received the key and the static
/// route names no consumer of its own to blame.
pub(super) struct LostText {
    pub(super) headline: String,
    pub(super) lines: Vec<String>,
}

fn onward(observation: &ObservedKey) -> Option<&Onward> {
    observation.onward.as_ref()
}

/// Who saw the key, as the subject of a sentence and as a place.
fn seen_by(source: &CaptureSource) -> (String, String) {
    match source {
        CaptureSource::Evdev { .. } => (
            "The keyboard produced the key".into(),
            "the keyboard".into(),
        ),
        other => {
            let name = other
                .backend_name()
                .map_or_else(|| other.label(), str::to_owned);
            (format!("{name} saw the key"), name)
        }
    }
}

pub(super) fn lost_text(key: &KeyCombo, observation: &ObservedKey) -> Option<LostText> {
    let onward =
        onward(observation).filter(|onward| onward.lost() && !onward.candidates.is_empty())?;
    let (subject, from) = seen_by(&observation.source);
    let waited = onward
        .waited_ms
        .map_or_else(String::new, |ms| format!(" within {ms} ms"));
    let mut lines = vec![format!(
        "{subject} and {} never received it{waited}.",
        onward.name
    )];
    lines.push(format!(
        "Candidates on that span: {}. The route below shows what each is configured to do with this key.",
        onward.candidates.join(", ")
    ));
    Some(LostText {
        headline: format!(
            "{} was taken between {from} and {}.",
            style::human_key(key),
            onward.name
        ),
        lines,
    })
}

/// The headline for a key that was seen upstream and then arrived at the
/// terminal: nothing between the two took it. The layers after the terminal
/// are still predicted, so the verdict mark stays the static one.
pub(super) fn reached_text(key: &KeyCombo, observation: &ObservedKey) -> Option<LostText> {
    let onward = onward(observation).filter(|onward| onward.reached)?;
    let (_, from) = seen_by(&observation.source);
    Some(LostText {
        headline: format!(
            "{} reached {}; nothing between {from} and {} took it.",
            style::human_key(key),
            onward.name,
            onward.name
        ),
        lines: vec![
            "Layers after the terminal are predicted from configuration, not observed.".into(),
        ],
    })
}

/// A probe window that received only modifiers while the physical keyboard
/// produced a key with them: the diagnosis and effect that replace the
/// "consumed or a lone modifier tap" ambiguity.
pub(super) fn withheld_text(observation: &ObservedKey) -> Option<(String, String)> {
    let onward = onward(observation)?;
    let key = onward.seen_key.as_deref()?;
    let mut effect = "The physical keyboard produced the key, so this is a consumed key, not a lone modifier tap.".to_owned();
    if !onward.candidates.is_empty() {
        effect.push_str(&format!(
            " Candidates above the window: {}.",
            onward.candidates.join(", ")
        ));
    }
    Some((
        format!("{key} was pressed and never reached {}.", onward.name),
        effect,
    ))
}

/// A line for the observation when a later point was watched and the report
/// is not the "taken between" verdict.
pub(super) fn also_line(observation: &ObservedKey) -> Option<String> {
    let onward = onward(observation)?;
    if onward.reached {
        return None;
    }
    onward.candidates.is_empty().then(|| {
        let waited = onward
            .waited_ms
            .map_or_else(String::new, |ms| format!(" (waited {ms} ms)"));
        format!(
            "It never reached {}{waited}, as the route below expects.",
            onward.name
        )
    })
}

/// The confidence sentence for a capture that watched two points.
pub(super) fn confidence(observation: &ObservedKey) -> Option<String> {
    let onward = onward(observation)?;
    let (_, from) = seen_by(&observation.source);
    Some(if onward.reached {
        format!(
            "observed: whykey saw the key at {from} and again at {}. Layers after that point are predicted from configuration.",
            onward.name
        )
    } else if onward.candidates.is_empty() {
        format!(
            "observed: whykey saw the key at {from} and watched {} for it: it never arrived, which matches the layer shown as taking it.",
            onward.name
        )
    } else {
        format!(
            "observed: whykey saw the key at {from} and watched {} for it: it never arrived. Which candidate took it is not observed.",
            onward.name
        )
    })
}

/// True when the report is the "taken between" verdict, which has no
/// inspection to suggest.
pub(super) fn is_lost(observation: Option<&ObservedKey>) -> bool {
    observation.is_some_and(|value| {
        value
            .onward
            .as_ref()
            .is_some_and(|onward| onward.lost() && !onward.candidates.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listen::{CaptureDisposition, KeyEventType, WatchPoint};

    fn layer(id: LayerId, name: &'static str, outcome: Outcome, summary: &str) -> LayerResult {
        if outcome == Outcome::Pass {
            return LayerResult::pass(
                name,
                id,
                summary,
                Vec::new(),
                crate::layers::Checked::fixture(),
            );
        }
        LayerResult::new(name, id, outcome, summary, Vec::new())
    }

    fn observed(onward: Onward) -> ObservedKey {
        ObservedKey {
            disposition: CaptureDisposition::PassedThrough,
            onward: Some(onward),
            combo: "ctrl+alt+p".parse().unwrap(),
            raw: Vec::new(),
            raw_display: None,
            modifier_state: None,
            associated_text: None,
            physical_keycode: None,
            encoding: "fixture".into(),
            protocol_flags: None,
            event_type: KeyEventType::Press,
            alternate_keys: None,
            alternate_key: None,
            source: CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
        }
    }

    fn lost_onward(candidates: &[&str]) -> Onward {
        Onward {
            point: WatchPoint::Terminal,
            name: "Ghostty".into(),
            reached: false,
            waited_ms: Some(400),
            candidates: candidates.iter().map(|name| (*name).to_owned()).collect(),
            seen_key: None,
        }
    }

    /// The route `listen` leaves after a key died between Hyprland and Ghostty.
    fn lost_route() -> Vec<LayerResult> {
        vec![
            layer(
                LayerId::Compositor,
                "Hyprland",
                Outcome::Pass,
                "no active binding found",
            ),
            layer(
                LayerId::Ime,
                "Fcitx5",
                Outcome::HandledUncertain,
                "Fcitx5 hotkey TogglePreedit toggles embedded preedit",
            ),
            layer(
                LayerId::Terminal,
                "Ghostty",
                Outcome::Unknown,
                "no Ghostty binding",
            ),
        ]
    }

    #[test]
    fn a_key_taken_between_the_compositor_and_the_terminal_names_the_span() {
        let observation = observed(lost_onward(&["Fcitx5", "Ghostty"]));
        let output = render_observed(&observation, &lost_route(), false);
        assert!(output.contains("Ctrl+Alt+P was taken between Hyprland and Ghostty."));
        assert!(output.contains("Candidates on that span: Fcitx5, Ghostty."));
        assert!(!output.contains("whykey listen --suppress"));
        crate::style::assert_golden("a_key_taken_between_compositor_and_terminal", &output);
    }

    #[test]
    fn a_route_that_already_names_the_consumer_is_left_alone() {
        let observation = observed(lost_onward(&[]));
        let output = render_observed(&observation, &lost_route(), false);
        let output = output.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!output.contains("was taken between"));
        assert!(output.contains("It never reached Ghostty (waited 400 ms)"));
        assert!(output.contains("which matches the layer shown as taking it"));
    }

    #[test]
    fn a_key_that_reached_the_terminal_is_confirmed_through_it() {
        let observation = observed(Onward {
            point: WatchPoint::Terminal,
            name: "Ghostty".into(),
            reached: true,
            waited_ms: None,
            candidates: Vec::new(),
            seen_key: None,
        });
        let layers = [
            layer(LayerId::Compositor, "Hyprland", Outcome::Pass, "no binding"),
            layer(
                LayerId::Terminal,
                "Ghostty",
                Outcome::Pass,
                "no Ghostty binding",
            ),
        ];
        let output = render_observed(&observation, &layers, false);
        assert!(output.contains("reached Ghostty; nothing between Hyprland and Ghostty took it."));
        assert!(output.contains("again at Ghostty"));
        let json = render_listen_json(&observation.combo, &layers, Some(&observation), 2);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["assessment"]["terminal_reached"], true);
        assert_eq!(value["observation"]["onward"]["point"], "terminal");
        assert_eq!(value["observation"]["onward"]["reached"], true);
    }

    #[test]
    fn a_lost_key_is_recorded_in_json_with_its_candidates() {
        let observation = observed(lost_onward(&["Fcitx5", "Ghostty"]));
        let json = render_listen_json(&observation.combo, &lost_route(), Some(&observation), 2);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let onward = &value["observation"]["onward"];
        assert_eq!(onward["reached"], false);
        assert_eq!(onward["waited_ms"], 400);
        assert_eq!(
            onward["candidates"],
            serde_json::json!(["Fcitx5", "Ghostty"])
        );
        assert_eq!(value["assessment"]["terminal_reached"], false);
    }

    #[test]
    fn a_capture_without_a_second_point_has_no_onward_field() {
        let mut observation = observed(lost_onward(&[]));
        observation.onward = None;
        let json = render_listen_json(&observation.combo, &lost_route(), Some(&observation), 2);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["observation"].get("onward").is_none());
    }

    #[test]
    fn a_probe_window_that_only_got_modifiers_is_confirmed_by_the_keyboard() {
        let mut observation = observed(Onward {
            point: WatchPoint::ProbeWindow,
            name: "the probe window".into(),
            reached: false,
            waited_ms: None,
            candidates: vec!["Hyprland".into()],
            seen_key: Some("Super+Return".into()),
        });
        observation.disposition = CaptureDisposition::NotDelivered;
        let output = render_observed(&observation, &[], false);
        let flat = output.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("Super+Return was pressed and never reached the probe window."));
        assert!(flat.contains("not a lone modifier tap"));
        assert!(flat.contains("Candidates above the window: Hyprland."));
        // Without the keyboard the report keeps its honest ambiguity.
        observation.onward = None;
        let plain = render_observed(&observation, &[], false);
        assert!(plain.contains("or"), "{plain}");
        assert!(!plain.contains("not a lone modifier tap"));
    }
}
