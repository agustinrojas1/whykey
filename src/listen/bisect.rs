//! Bisecting capture: watch more than one point on the key route for the same
//! key press, so a key that dies somewhere answers "where" instead of timing
//! out.
//!
//! The earliest point that is available leads (evdev, then the compositor
//! hook); the terminal is read alongside it, and the probe window stays the
//! target when it is the chosen backend. Every point is read-only. A key the
//! leading point saw and a later point never did is evidence, not an error:
//! it was taken between the two, and the static route names who could have.

use super::protocol::{read_event, remaining_millis, wait_for_input};
use super::terminal::{InputReader, SignalGuard};
use super::*;
use crate::layers::{LayerId, LayerResult, Outcome, route_context_of, route_order};

/// How long the compositor leg waits for a key that evdev already saw.
pub(super) const COMPOSITOR_GRACE: Duration = Duration::from_millis(300);
/// How long the terminal leg waits for a key the compositor already saw. An
/// input method round trip sits inside this window.
pub(super) const TERMINAL_GRACE: Duration = Duration::from_millis(400);
/// How long an evdev press stays relevant to a probe window that only
/// reported modifiers.
pub(super) const CHORD_MEMORY: Duration = Duration::from_secs(5);
/// Time to finish reading one escape sequence that arrived in two pieces.
const SEQUENCE_PATIENCE: Duration = Duration::from_millis(40);
/// Presses older than this can no longer belong to the key being judged.
const PRESS_MEMORY: Duration = Duration::from_secs(2);

/// What the terminal made of the key presses that arrived while another
/// capture point led.
#[derive(Debug, Default)]
pub(super) struct TerminalWatch {
    pending: VecDeque<u8>,
    presses: VecDeque<Instant>,
}

impl TerminalWatch {
    /// Read whatever the terminal has sent so far. Modifier presses and key
    /// releases are not the key being judged and are dropped.
    pub(super) fn drain(&mut self, tty: &mut File) {
        loop {
            if self.pending.is_empty() && !wait_for_input(tty.as_raw_fd(), 0).unwrap_or(false) {
                break;
            }
            let mut reader = InputReader::with_pending(tty, self.pending.drain(..).collect());
            let event = read_event(&mut reader, Some(Instant::now() + SEQUENCE_PATIENCE));
            self.pending = reader.pending;
            match event {
                Ok(ReadEvent::Key(observed)) => {
                    if observed.event_type == KeyEventType::Press
                        && !output::is_terminal_modifier_key(&observed)
                    {
                        self.presses.push_back(Instant::now());
                    }
                }
                Ok(ReadEvent::Idle) | Err(_) => break,
            }
        }
        while self
            .presses
            .front()
            .is_some_and(|at| at.elapsed() > PRESS_MEMORY)
        {
            self.presses.pop_front();
        }
    }

    /// Whether a key press reached the terminal at or after `since`.
    pub(super) fn pressed_since(&self, since: Instant) -> bool {
        self.presses.iter().any(|at| *at >= since)
    }

    pub(super) fn forget(&mut self) {
        self.presses.clear();
        self.pending.clear();
    }

    /// Wait up to `grace` after `since` for a press to arrive. `None` when a
    /// signal interrupted the wait: nothing can be concluded then.
    pub(super) fn await_press(
        &mut self,
        tty: &mut File,
        since: Instant,
        grace: Duration,
        signals: &SignalGuard,
    ) -> Option<bool> {
        let deadline = since + grace;
        loop {
            self.drain(tty);
            if self.pressed_since(
                since
                    .checked_sub(Duration::from_millis(100))
                    .unwrap_or(since),
            ) {
                return Some(true);
            }
            if signals.received() {
                return None;
            }
            let remaining = remaining_millis(deadline);
            if remaining <= 0 {
                return Some(false);
            }
            let _ = wait_for_input(tty.as_raw_fd(), remaining.min(25));
        }
    }
}

/// A key press read from the physical keyboard while a later capture point
/// was the one being asked for it.
#[cfg(target_os = "linux")]
pub(super) struct EvdevWitness {
    session: Option<EvdevSession>,
    presses: VecDeque<(Instant, ObservedKey)>,
}

#[cfg(target_os = "linux")]
impl EvdevWitness {
    /// The witness is optional by design: without read access to the input
    /// devices it stays empty and every verdict is judged without it.
    pub(super) fn open() -> Self {
        let session = EvdevSession::open(None).ok().map(|mut session| {
            session.include_modifiers = false;
            session
        });
        Self {
            session,
            presses: VecDeque::new(),
        }
    }

    /// A witness that never sees anything, for runs that do not bisect.
    pub(super) fn unavailable() -> Self {
        Self {
            session: None,
            presses: VecDeque::new(),
        }
    }

    /// The descriptors a poll loop should wake on.
    pub(super) fn poll_fds(&self) -> Vec<libc::c_int> {
        self.session.as_ref().map_or_else(Vec::new, |session| {
            session
                .devices
                .iter()
                .map(|device| device.file.as_raw_fd())
                .collect()
        })
    }

    /// Read pending physical key events without blocking.
    pub(super) fn drain(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        for _ in 0..64 {
            match session.read_key_event(0) {
                Ok(Some(observed)) => {
                    if observed.event_type == KeyEventType::Press {
                        self.presses.push_back((Instant::now(), observed));
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        while self
            .presses
            .front()
            .is_some_and(|(at, _)| at.elapsed() > CHORD_MEMORY)
        {
            self.presses.pop_front();
        }
    }

    /// The oldest press at or after `since`, which no later point confirmed.
    pub(super) fn press_since(&self, since: Instant) -> Option<&(Instant, ObservedKey)> {
        self.presses.iter().find(|(at, _)| *at >= since)
    }

    pub(super) fn forget(&mut self) {
        self.presses.clear();
    }
}

/// Whether the terminal whykey runs in is the focused window. `None` when the
/// compositor cannot say: a key that never arrived is then not judged.
pub(super) fn terminal_has_focus(environment: &crate::environment::Environment) -> Option<bool> {
    let target = crate::focus::resolve_with_environment(environment).ok()?;
    Some(crate::focus::shares_terminal_with_whykey(target.pid))
}

/// No physical keyboard witness exists off Linux.
#[cfg(not(target_os = "linux"))]
pub(super) struct EvdevWitness;

#[cfg(not(target_os = "linux"))]
impl EvdevWitness {
    pub(super) fn open() -> Self {
        Self
    }
    pub(super) fn unavailable() -> Self {
        Self
    }
    pub(super) fn poll_fds(&self) -> Vec<libc::c_int> {
        Vec::new()
    }
    pub(super) fn drain(&mut self) {}
    pub(super) fn press_since(&self, _since: Instant) -> Option<&(Instant, ObservedKey)> {
        None
    }
    pub(super) fn forget(&mut self) {}
}

/// The program that stands for a watch point in reports.
pub(super) fn point_name(results: &[LayerResult], point: WatchPoint, compositor: &str) -> String {
    let layer_name = |id: LayerId| {
        results
            .iter()
            .find(|layer| layer.id == id)
            .map(|layer| layer.layer)
            .filter(|name| !name.starts_with("Unknown"))
    };
    match point {
        WatchPoint::Compositor => compositor.to_owned(),
        WatchPoint::Terminal => layer_name(LayerId::Terminal)
            .unwrap_or("the terminal")
            .to_owned(),
        WatchPoint::ProbeWindow => "the probe window".to_owned(),
    }
}

/// The route stages between the capture point that saw the key and `point`.
fn span_ids(results: &[LayerResult], seen_at: &CaptureSource, point: WatchPoint) -> Vec<LayerId> {
    let order = route_order(route_context_of(results));
    let index = |id: LayerId| order.iter().position(|stage| *stage == id);
    let start = if matches!(seen_at, CaptureSource::CompositorNative { .. }) {
        index(LayerId::Compositor).map_or(0, |at| at + 1)
    } else {
        0
    };
    let end = match point {
        // The compositor hook sits after the remapper; the span includes it.
        WatchPoint::Compositor => index(LayerId::Compositor).map_or(order.len(), |at| at + 1),
        // The probe window is a client: it sits where the terminal would.
        WatchPoint::ProbeWindow => index(LayerId::Terminal).unwrap_or(order.len()),
        // whykey reads the terminal's tty: everything before the TTY driver
        // (or the multiplexer's own tty) has had its chance to take the key.
        WatchPoint::Terminal => index(LayerId::Tty).unwrap_or(order.len()),
    };
    order[start.min(end)..end].to_vec()
}

/// A layer worth naming when the key vanished on this span. Stages the user
/// has nothing running for (no multiplexer, no remapper) are not candidates.
fn is_candidate(layer: &LayerResult, point: WatchPoint) -> bool {
    match layer.id {
        LayerId::Terminal => point == WatchPoint::Terminal,
        LayerId::Compositor | LayerId::Ime | LayerId::Remapper => true,
        LayerId::Multiplexer | LayerId::Session => layer.outcome != Outcome::Pass,
        LayerId::Tty | LayerId::Application | LayerId::Shell | LayerId::Diagnostic => false,
        LayerId::Unknown => false,
    }
}

/// The key arrived at `point`: every layer on the way forwarded it, whatever
/// its configuration says.
pub(super) fn apply_reached(
    results: &mut [LayerResult],
    observed: &mut ObservedKey,
    point: WatchPoint,
    name: String,
) {
    for id in span_ids(results, &observed.source, point) {
        for layer in results.iter_mut().filter(|layer| layer.id == id) {
            if !is_candidate(layer, point) && layer.id != LayerId::Terminal {
                continue;
            }
            layer.tier = Some(crate::layers::EvidenceTier::Observed);
            match layer.outcome {
                Outcome::Consumed | Outcome::Redirected | Outcome::HandledUncertain => {
                    layer.outcome = Outcome::HandledAndPassed;
                    layer
                        .summary
                        .push_str("; the key was observed arriving at ");
                    layer.summary.push_str(&name);
                }
                Outcome::Unknown | Outcome::UncertainContinues | Outcome::Unavailable => {
                    layer
                        .summary
                        .push_str("; the key was observed arriving at ");
                    layer.summary.push_str(&name);
                }
                _ => {}
            }
        }
    }
    observed.onward = Some(Onward {
        point,
        name,
        reached: true,
        waited_ms: None,
        candidates: Vec::new(),
    });
}

/// The layer the static route already says stops the key on the way to
/// `point`. The layer that saw the key counts too: a compositor reports a key
/// before it dispatches its own binding.
fn static_consumer<'a>(
    results: &'a mut [LayerResult],
    seen_at: &CaptureSource,
    point: WatchPoint,
) -> Option<&'a mut LayerResult> {
    let mut span = span_ids(results, seen_at, point);
    if matches!(seen_at, CaptureSource::CompositorNative { .. }) {
        span.push(LayerId::Compositor);
    }
    results.iter_mut().find(|layer| {
        span.contains(&layer.id) && matches!(layer.outcome, Outcome::Consumed | Outcome::Redirected)
    })
}

/// Judge a key that never arrived at a terminal. The verdict only means
/// something when this terminal was the focused window, so `has_focus` is
/// asked. A route that already has a consumer for the key (a Hyprland binding)
/// is not second-guessed when focus moved away: a bound action usually takes
/// focus with it, and that silence needs no note. Returns false when the
/// caller should say nothing was judged.
pub(super) fn judge_lost(
    results: &mut Vec<LayerResult>,
    observed: &mut ObservedKey,
    point: WatchPoint,
    name: String,
    waited: Duration,
    has_focus: impl FnOnce() -> Option<bool>,
) -> bool {
    let explained = static_consumer(results, &observed.source, point).is_some();
    if point != WatchPoint::Terminal || has_focus() == Some(true) {
        apply_lost(results, observed, point, name, waited);
        return true;
    }
    explained
}

/// The key was seen at `observed.source` and never arrived at `point`.
///
/// When the static route already has a layer that consumes the key on that
/// span, the observation confirms it and nothing else is blamed. Otherwise
/// the layers of the span become the candidates, and the route stops there:
/// nothing after them received the key.
pub(super) fn apply_lost(
    results: &mut Vec<LayerResult>,
    observed: &mut ObservedKey,
    point: WatchPoint,
    name: String,
    waited: Duration,
) {
    let span = span_ids(results, &observed.source, point);
    let in_span = |layer: &LayerResult| {
        span.contains(&layer.id) && (is_candidate(layer, point) || layer.id == LayerId::Terminal)
    };
    let waited_ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
    if let Some(consumer) = static_consumer(results, &observed.source, point) {
        consumer.tier = Some(crate::layers::EvidenceTier::Observed);
        consumer
            .details
            .push(format!("the key never arrived at {name}, as expected"));
        observed.onward = Some(Onward {
            point,
            name,
            reached: false,
            waited_ms: Some(waited_ms),
            candidates: Vec::new(),
        });
        return;
    }

    let mut candidates: Vec<String> = Vec::new();
    // A layer with a matching binding is the likeliest taker: name it first.
    let mut ranked: Vec<&LayerResult> = results.iter().filter(|layer| in_span(layer)).collect();
    ranked.sort_by_key(|layer| u8::from(layer.binding.is_none()));
    for layer in ranked {
        if !candidates.iter().any(|known| known == layer.layer) {
            candidates.push(layer.layer.to_owned());
        }
    }
    for layer in results.iter_mut().filter(|layer| in_span(layer)) {
        layer.outcome = match layer.outcome {
            Outcome::Pass | Outcome::UncertainContinues => Outcome::Unknown,
            Outcome::HandledAndPassed => Outcome::HandledUncertain,
            other => other,
        };
        layer.details.push(format!(
            "the key never arrived at {name}: this layer may have taken it"
        ));
    }
    // Layers after the span never received the key.
    if let Some(last) = results.iter().rposition(in_span) {
        results.truncate(last + 1);
    }
    observed.onward = Some(Onward {
        point,
        name,
        reached: false,
        waited_ms: Some(waited_ms),
        candidates,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unfocused_terminal_is_never_blamed_and_a_bound_key_needs_no_note() {
        let mut results = route();
        let mut observed = seen_by_hyprland();
        let judged = judge_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
            || Some(false),
        );
        assert!(!judged);
        assert!(observed.onward.is_none());
        assert_eq!(results.len(), 5);

        results[0].outcome = Outcome::Consumed;
        let judged = judge_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
            || Some(false),
        );
        assert!(judged, "a bound key needs no note when focus moved away");
        assert!(observed.onward.is_none(), "but nothing is claimed either");
    }

    fn layer(id: LayerId, name: &'static str, outcome: Outcome) -> LayerResult {
        LayerResult::new(name, id, outcome, format!("{name} summary"), Vec::new())
    }

    fn seen_by_hyprland() -> ObservedKey {
        ObservedKey {
            disposition: CaptureDisposition::PassedThrough,
            onward: None,
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

    fn route() -> Vec<LayerResult> {
        vec![
            layer(LayerId::Compositor, "Hyprland", Outcome::Pass),
            layer(LayerId::Ime, "Fcitx5", Outcome::HandledUncertain),
            layer(LayerId::Terminal, "Ghostty", Outcome::Pass),
            layer(LayerId::Tty, "TTY driver", Outcome::Pass),
            layer(LayerId::Shell, "bash", Outcome::Pass),
        ]
    }

    #[test]
    fn a_key_the_terminal_never_got_names_the_layers_between() {
        let mut results = route();
        let mut observed = seen_by_hyprland();
        apply_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
        );
        let onward = observed.onward.expect("verdict recorded");
        assert!(onward.lost());
        assert_eq!(onward.waited_ms, Some(400));
        // The layer with a binding is named first; the compositor that saw
        // the key is not a suspect.
        assert_eq!(onward.candidates, ["Fcitx5", "Ghostty"]);
        let names: Vec<_> = results.iter().map(|layer| layer.layer).collect();
        assert_eq!(names, ["Hyprland", "Fcitx5", "Ghostty"]);
    }

    #[test]
    fn a_static_consumer_on_the_span_is_confirmed_not_second_guessed() {
        let mut results = route();
        results[1].outcome = Outcome::Consumed;
        let mut observed = seen_by_hyprland();
        apply_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
        );
        let onward = observed.onward.unwrap();
        assert!(onward.lost());
        assert!(onward.candidates.is_empty());
        assert_eq!(results.len(), 5, "the route is left as the static one");
        assert_eq!(results[1].tier, Some(crate::layers::EvidenceTier::Observed));
    }

    #[test]
    fn a_compositor_binding_that_took_the_key_explains_the_silent_terminal() {
        let mut results = route();
        results[0].outcome = Outcome::Consumed;
        let mut observed = seen_by_hyprland();
        apply_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
        );
        let onward = observed.onward.unwrap();
        assert!(onward.lost());
        assert!(onward.candidates.is_empty(), "nobody else is blamed");
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn a_key_that_arrived_confirms_the_layers_it_passed() {
        let mut results = route();
        results[1].outcome = Outcome::Consumed;
        let mut observed = seen_by_hyprland();
        apply_reached(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
        );
        assert!(observed.onward.as_ref().unwrap().reached);
        assert_eq!(results[1].outcome, Outcome::HandledAndPassed);
        assert_eq!(results[1].tier, Some(crate::layers::EvidenceTier::Observed));
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn a_key_evdev_saw_and_the_compositor_did_not_blames_remapper_or_compositor() {
        let mut results = vec![
            layer(LayerId::Remapper, "keyd", Outcome::Unknown),
            layer(LayerId::Compositor, "Hyprland", Outcome::Pass),
            layer(LayerId::Terminal, "Ghostty", Outcome::Pass),
        ];
        let mut observed = seen_by_hyprland();
        observed.source = CaptureSource::Evdev {
            device: "kbd".into(),
            path: "/dev/input/event3".into(),
        };
        apply_lost(
            &mut results,
            &mut observed,
            WatchPoint::Compositor,
            "Hyprland".into(),
            COMPOSITOR_GRACE,
        );
        let onward = observed.onward.unwrap();
        assert_eq!(onward.candidates, ["keyd", "Hyprland"]);
        assert_eq!(results.len(), 2, "the terminal never got the key either");
    }

    #[test]
    fn stages_with_nothing_running_are_not_suspects() {
        let mut results = route();
        results.insert(
            3,
            layer(LayerId::Multiplexer, "Session context", Outcome::Pass),
        );
        let mut observed = seen_by_hyprland();
        apply_lost(
            &mut results,
            &mut observed,
            WatchPoint::Terminal,
            "Ghostty".into(),
            TERMINAL_GRACE,
        );
        assert_eq!(observed.onward.unwrap().candidates, ["Fcitx5", "Ghostty"]);
    }

    #[test]
    fn point_names_fall_back_for_an_unidentified_terminal() {
        let results = vec![layer(
            LayerId::Terminal,
            "Unknown terminal",
            Outcome::Unknown,
        )];
        assert_eq!(
            point_name(&results, WatchPoint::Terminal, "Hyprland"),
            "the terminal"
        );
        assert_eq!(
            point_name(&results, WatchPoint::Compositor, "Hyprland"),
            "Hyprland"
        );
    }
}
