//! Probe-window capture: whykey opens its own focused toplevel and reports
//! what the compositor/desktop actually delivers to a client.
//!
//! A key press that arrives proves every layer above the window (remappers,
//! compositor, desktop shortcut daemons, input methods) passed it on.
//! Modifier changes that arrive while the key never does prove something
//! above the window consumed that key.

use super::output::run_observed_loop;
use super::terminal::SignalGuard;
use super::*;
use crate::capture::ChordReleaseStatus;
use crate::probe::{ProbeEvent, ProbeKind, ProbeWindow};

const PROBE_TITLE: &str = "whykey: press a shortcut here (Esc exits)";
/// Matches the other backends' signal/deadline polling cadence.
const POLL_INTERVAL: Duration = Duration::from_millis(SIGNAL_POLL_MS as u64);
const FOCUS_NOTICE_AFTER: Duration = Duration::from_secs(2);
/// Caps Lock and Num Lock describe toggled state, not a held chord.
const LOCK_BITS: u32 = 2 | 16;

pub(super) fn run_window(options: Options) -> Result<(), ListenError> {
    // Install first: the guard must outlive the capture session it protects.
    let signals = SignalGuard::install()?;
    let snapshot = ListenSession::capture();
    let mut backend = WindowBackend::new();
    backend
        .arm(CapturePolicy::PassThrough)
        .map_err(ListenError::Setup)?;
    run_observed_loop(options, backend, snapshot, None, &signals)
}

/// What one probe event means for the chord being observed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observation {
    /// A key press reached the window; `modifier` marks modifier keys.
    Press {
        event: ProbeEvent,
        modifier: bool,
    },
    Release(ProbeEvent),
    /// Modifiers were held and released, and no other key arrived meanwhile.
    NotDelivered {
        modmask: u32,
    },
}

/// Follows held modifiers across events to notice a chord whose key never
/// arrived.
#[derive(Debug, Default)]
struct ChordTracker {
    /// Union of chord modifiers held since they were last all released.
    held: u32,
    /// Whether a non-modifier key arrived while `held` was accumulating.
    key_seen: bool,
}

impl ChordTracker {
    fn observe(&mut self, event: ProbeEvent) -> Option<Observation> {
        match event.kind {
            ProbeKind::Press => {
                let modifier = is_modifier_key(&event);
                if !modifier {
                    self.key_seen = true;
                }
                Some(Observation::Press { event, modifier })
            }
            ProbeKind::Release => Some(Observation::Release(event)),
            ProbeKind::Modifiers => {
                let active = event.modmask & !LOCK_BITS;
                if active != 0 {
                    if self.held == 0 {
                        self.key_seen = false;
                    }
                    self.held |= active;
                    return None;
                }
                let held = std::mem::take(&mut self.held);
                let key_seen = std::mem::take(&mut self.key_seen);
                (held != 0 && !key_seen).then_some(Observation::NotDelivered { modmask: held })
            }
        }
    }
}

fn is_modifier_key(event: &ProbeEvent) -> bool {
    match event.keysym.as_deref() {
        Some(keysym) => is_modifier_keysym(keysym),
        None => crate::xkb::XkbKeycode::new(event.keycode)
            .to_evdev()
            .is_some_and(|code| evdev_modifier(code.get()).is_some()),
    }
}

fn is_modifier_keysym(keysym: &str) -> bool {
    let sided = keysym
        .strip_suffix("_L")
        .or_else(|| keysym.strip_suffix("_R"));
    if let Some(base) = sided {
        if matches!(
            base,
            "Shift" | "Control" | "Alt" | "Meta" | "Super" | "Hyper"
        ) {
            return true;
        }
    }
    keysym.starts_with("ISO_Level")
        || keysym.starts_with("ISO_Group")
        || keysym.ends_with("_Group")
        || matches!(
            keysym,
            "Caps_Lock" | "Shift_Lock" | "Num_Lock" | "Mode_switch" | "ISO_Lock"
        )
}

fn modifier_names(modmask: u32) -> Vec<String> {
    [
        (4, "CTRL"),
        (8, "ALT"),
        (1, "SHIFT"),
        (64, "SUPER"),
        (32, "HYPER"),
        (128, "ALTGR"),
    ]
    .into_iter()
    .filter(|(bit, _)| modmask & bit != 0)
    .map(|(_, name)| name.to_owned())
    .collect()
}

fn lock_names(modmask: u32) -> Vec<String> {
    [(2, "CAPSLOCK"), (16, "NUMLOCK")]
        .into_iter()
        .filter(|(bit, _)| modmask & bit != 0)
        .map(|(_, name)| name.to_owned())
        .collect()
}

fn delivered_key(event: &ProbeEvent, event_type: KeyEventType) -> ObservedKey {
    let evdev_code = crate::xkb::XkbKeycode::new(event.keycode).to_evdev();
    let key = event
        .keysym
        .clone()
        .or_else(|| {
            evdev_code
                .and_then(|code| evdev_key_name(code.get()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("CODE:{}", event.keycode));
    let chord = event.modmask & !LOCK_BITS;
    ObservedKey {
        disposition: CaptureDisposition::ObservedOnly,
        combo: KeyCombo::from_parts(chord, &key),
        raw: Vec::new(),
        raw_display: Some(format!(
            "probe window XKB keycode={} keysym={} ({})",
            event.keycode,
            event.keysym.as_deref().unwrap_or("unresolved"),
            event_type.label()
        )),
        modifier_state: Some(ModifierState {
            pressed: modifier_names(chord),
            locked: lock_names(event.modmask),
            latched: None,
            devices: Vec::new(),
        }),
        associated_text: None,
        physical_keycode: evdev_code,
        encoding: "probe window key event".into(),
        protocol_flags: None,
        event_type,
        alternate_keys: None,
        alternate_key: None,
        source: CaptureSource::ProbeWindow,
    }
}

fn withheld_key(modmask: u32) -> ObservedKey {
    let names = modifier_names(modmask);
    ObservedKey {
        disposition: CaptureDisposition::NotDelivered,
        combo: KeyCombo::from_parts(modmask, ""),
        raw: Vec::new(),
        raw_display: Some(format!(
            "probe window saw {} held and released; no other key arrived",
            names.join("+")
        )),
        modifier_state: Some(ModifierState {
            pressed: names,
            locked: Vec::new(),
            latched: None,
            devices: Vec::new(),
        }),
        associated_text: None,
        physical_keycode: None,
        encoding: "probe window modifier events".into(),
        protocol_flags: None,
        event_type: KeyEventType::Press,
        alternate_keys: None,
        alternate_key: None,
        source: CaptureSource::ProbeWindow,
    }
}

/// Capture backend over a focused probe window.
pub struct WindowBackend {
    window: Option<Box<dyn ProbeWindow>>,
    /// Chord state persists across captures: the window stays mapped, so a
    /// chord may span the end of one report and the start of the next.
    tracker: ChordTracker,
    opened_at: Instant,
    /// Set once the window first gains focus or the user was told to focus
    /// it. Later focus loss (for example a keyboard device going away) is
    /// not worth a notice.
    focus_settled: bool,
}

impl WindowBackend {
    pub fn new() -> Self {
        Self {
            window: None,
            tracker: ChordTracker::default(),
            opened_at: Instant::now(),
            focus_settled: false,
        }
    }
}

impl Default for WindowBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for WindowBackend {
    fn id(&self) -> CaptureBackendId {
        CaptureBackendId::Window
    }

    fn display(&self) -> &'static str {
        CaptureBackendId::Window.display()
    }

    fn arm(&mut self, _policy: CapturePolicy) -> Result<(), String> {
        if self.window.is_none() {
            let window = crate::probe::open_window(PROBE_TITLE)
                .map_err(|error| format!("could not open the probe window: {error}"))?;
            self.window = Some(window);
            self.tracker = ChordTracker::default();
            self.opened_at = Instant::now();
            self.focus_settled = false;
        }
        Ok(())
    }

    fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        let window = self
            .window
            .as_mut()
            .ok_or_else(|| "probe window capture is closed".to_owned())?;
        if !self.focus_settled {
            if window.focused() {
                self.focus_settled = true;
            } else if self.opened_at.elapsed() >= FOCUS_NOTICE_AFTER {
                eprintln!(
                    "whykey listen: the probe window does not have keyboard focus; click it or switch to it."
                );
                self.focus_settled = true;
            }
        }
        let Some(event) = window
            .next_event(POLL_INTERVAL)
            .map_err(|error| format!("probe window: {error}"))?
        else {
            return Ok(None);
        };
        Ok(match self.tracker.observe(event) {
            Some(Observation::Press { event, modifier }) if !modifier || events_all => {
                Some(delivered_key(&event, KeyEventType::Press))
            }
            Some(Observation::Release(event)) if events_all => {
                Some(delivered_key(&event, KeyEventType::Release))
            }
            Some(Observation::NotDelivered { modmask }) => Some(withheld_key(modmask)),
            _ => None,
        })
    }

    fn wait_for_chord_release(
        &mut self,
        _main: crate::xkb::XkbKeycode,
        _timeout: Duration,
    ) -> Result<ChordReleaseStatus, String> {
        Ok(ChordReleaseStatus::Released)
    }

    fn is_suppressing(&self) -> bool {
        false
    }

    /// Ends one capture. The window itself stays mapped so repeated captures
    /// keep focus; it closes when the backend is dropped.
    fn close(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(keysym: &str, keycode: u32, modmask: u32) -> ProbeEvent {
        ProbeEvent {
            keycode,
            keysym: Some(keysym.into()),
            modmask,
            kind: ProbeKind::Press,
        }
    }

    fn modifiers(modmask: u32) -> ProbeEvent {
        ProbeEvent {
            keycode: 0,
            keysym: None,
            modmask,
            kind: ProbeKind::Modifiers,
        }
    }

    #[test]
    fn modifiers_without_a_key_are_not_delivered() {
        let mut tracker = ChordTracker::default();
        assert!(tracker.observe(modifiers(64)).is_none());
        assert_eq!(
            tracker.observe(modifiers(0)),
            Some(Observation::NotDelivered { modmask: 64 })
        );
        // The chord is finished; another release reports nothing.
        assert!(tracker.observe(modifiers(0)).is_none());
    }

    #[test]
    fn modifier_key_presses_do_not_count_as_the_chord_key() {
        let mut tracker = ChordTracker::default();
        let observed = tracker.observe(press("Super_L", 133, 0));
        assert!(matches!(
            observed,
            Some(Observation::Press { modifier: true, .. })
        ));
        assert!(tracker.observe(modifiers(64)).is_none());
        assert!(tracker.observe(modifiers(64 | 4)).is_none());
        assert_eq!(
            tracker.observe(modifiers(0)),
            Some(Observation::NotDelivered { modmask: 64 | 4 })
        );
    }

    #[test]
    fn delivered_chord_key_suppresses_the_not_delivered_verdict() {
        let mut tracker = ChordTracker::default();
        tracker.observe(modifiers(64));
        let observed = tracker.observe(press("x", 53, 64));
        let Some(Observation::Press {
            event,
            modifier: false,
        }) = observed
        else {
            panic!("expected a key press, got {observed:?}");
        };
        let key = delivered_key(&event, KeyEventType::Press);
        assert_eq!(key.combo, "super+x".parse().unwrap());
        assert_eq!(key.source, CaptureSource::ProbeWindow);
        assert!(tracker.observe(modifiers(0)).is_none());
    }

    #[test]
    fn lock_state_is_not_a_chord() {
        let mut tracker = ChordTracker::default();
        assert!(tracker.observe(modifiers(2 | 16)).is_none());
        assert!(tracker.observe(modifiers(0)).is_none());
        let observed = tracker.observe(press("a", 38, 2 | 16));
        let Some(Observation::Press { event, .. }) = observed else {
            panic!("expected a key press");
        };
        let key = delivered_key(&event, KeyEventType::Press);
        assert_eq!(key.combo, "a".parse().unwrap());
        assert_eq!(
            key.modifier_state.unwrap().locked,
            vec!["CAPSLOCK".to_owned(), "NUMLOCK".to_owned()]
        );
    }

    #[test]
    fn withheld_observation_names_the_modifiers() {
        let key = withheld_key(64);
        assert_eq!(key.disposition, CaptureDisposition::NotDelivered);
        assert_eq!(key.combo.modmask(), 64);
        assert!(key.combo.key().is_empty());
        assert_eq!(key.modifier_state.unwrap().pressed, vec!["SUPER"]);
    }

    #[test]
    fn probe_window_source_round_trips() {
        let encoded = serde_json::to_value(CaptureSource::ProbeWindow).unwrap();
        assert_eq!(encoded, serde_json::json!({ "kind": "probe-window" }));
        assert_eq!(
            serde_json::from_value::<CaptureSource>(encoded).unwrap(),
            CaptureSource::ProbeWindow
        );
        assert_eq!(CaptureSource::ProbeWindow.label(), "probe window");
    }

    #[test]
    fn not_delivered_reports_state_the_verdict() {
        let observed = withheld_key(64);
        let human = report::render_observed_with_options(
            &observed,
            &[],
            crate::style::RenderOptions {
                color: crate::style::ColorChoice::Never,
                verbose: false,
                width: 400,
            },
        );
        assert!(
            human.contains(
                "A key pressed with Super did not reach the window: the compositor/desktop or an input method consumed it."
            ),
            "{human}"
        );
        let json: serde_json::Value = serde_json::from_str(&report::render_listen_json(
            &observed.combo,
            &[],
            Some(&observed),
            1,
        ))
        .unwrap();
        assert_eq!(json["observed"]["disposition"], "not_delivered");
        assert_eq!(json["key_display"], "SUPER + <key not delivered>");
        assert_eq!(json["confidence"], "confirmed");
    }

    #[test]
    fn delivered_key_confirms_forwarding_above_the_window() {
        let event = press("Return", 36, 64);
        let observed = delivered_key(&event, KeyEventType::Press);
        let mut results = vec![LayerResult::new(
            "Sway",
            LayerId::Compositor,
            Outcome::Consumed,
            "bindsym $mod+Return",
            Vec::new(),
        )];
        super::super::output::confirm_window_delivery(&mut results);
        assert_eq!(results[0].outcome, Outcome::HandledAndPassed);
        assert!(
            results[0]
                .summary
                .ends_with("observed key reached the probe window")
        );
        assert_eq!(observed.combo, "super+return".parse().unwrap());
    }

    #[test]
    fn classifies_modifier_keysyms() {
        for keysym in [
            "Shift_L",
            "Control_R",
            "Super_L",
            "Alt_R",
            "ISO_Level3_Shift",
            "Caps_Lock",
            "Num_Lock",
        ] {
            assert!(is_modifier_keysym(keysym), "{keysym}");
        }
        for keysym in ["a", "Return", "L", "F1", "Escape"] {
            assert!(!is_modifier_keysym(keysym), "{keysym}");
        }
    }
}
