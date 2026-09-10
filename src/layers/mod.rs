pub mod application;
pub mod cinnamon;
pub mod compositor;
pub mod ghostty;
pub mod gnome;
pub mod hyprland;
pub mod i3;
pub mod kde;
pub mod labwc;
pub mod mate;
pub mod niri;
pub mod openbox;
pub mod programmable;
pub mod readline;
pub mod river;
pub mod screen;
pub mod session;
pub mod shell;
pub mod sway;
pub mod sxhkd;
pub mod terminal;
pub mod terminal_app;
pub mod terminal_dispatch;
pub mod tmux;
pub mod wayfire;
pub mod x11;
pub mod xfce;
pub mod zellij;

use crate::command;
use crate::key::KeyCombo;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Clone, Copy)]
struct ApplicationTarget<'a> {
    pid: u32,
    source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalInput {
    pub bytes: Vec<u8>,
    pub description: String,
    pub confidence: InputConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InputConfidence {
    Configured,
    Predicted,
    Observed,
}

impl TerminalInput {
    pub fn predicted(bytes: Vec<u8>, description: impl Into<String>) -> Self {
        Self {
            bytes,
            description: description.into(),
            confidence: InputConfidence::Predicted,
        }
    }

    pub fn configured(bytes: Vec<u8>, description: impl Into<String>) -> Self {
        Self {
            bytes,
            description: description.into(),
            confidence: InputConfidence::Configured,
        }
    }

    pub fn confidence_label(&self) -> &'static str {
        match self.confidence {
            InputConfidence::Configured => "configured",
            InputConfidence::Predicted => "predicted",
            InputConfidence::Observed => "observed",
        }
    }

    pub fn display_bytes(&self) -> String {
        format_bytes(&self.bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerId {
    Remapper,
    Compositor,
    Terminal,
    Tty,
    Ime,
    Multiplexer,
    Application,
    Shell,
    Session,
    Diagnostic,
    Unknown,
}

/// Single internal route outcome. Serializers map it back to the existing
/// v1 (`status`) and (`propagation`) pair so automation keeps working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No binding was found; the event keeps flowing.
    Pass,
    /// A binding was found and the event keeps flowing.
    HandledAndPassed,
    /// A binding was found and the event is consumed.
    Consumed,
    /// The event is redirected to another window.
    Redirected,
    /// A binding was found, but consumption cannot be proven.
    HandledUncertain,
    /// Handling state is unknown, but the event most likely keeps flowing.
    UncertainContinues,
    /// Handling state could not be determined.
    Unknown,
    /// The layer could not be inspected.
    Unavailable,
}

impl Outcome {
    pub fn from_parts(status: &LayerStatus, propagation: &Propagation) -> Self {
        match (status, propagation) {
            (LayerStatus::NotHandled, Propagation::Continues) => Self::Pass,
            (LayerStatus::Handled, Propagation::Continues) => Self::HandledAndPassed,
            (LayerStatus::Handled, Propagation::Stops) => Self::Consumed,
            (_, Propagation::Redirected) => Self::Redirected,
            (LayerStatus::Unavailable, _) => Self::Unavailable,
            (LayerStatus::Handled, Propagation::Indeterminate) => Self::HandledUncertain,
            (LayerStatus::Indeterminate, Propagation::Continues) => Self::UncertainContinues,
            _ => Self::Unknown,
        }
    }

    pub fn as_parts(self) -> (LayerStatus, Propagation) {
        match self {
            Self::Pass => (LayerStatus::NotHandled, Propagation::Continues),
            Self::HandledAndPassed => (LayerStatus::Handled, Propagation::Continues),
            Self::Consumed => (LayerStatus::Handled, Propagation::Stops),
            Self::Redirected => (LayerStatus::Handled, Propagation::Redirected),
            Self::HandledUncertain => (LayerStatus::Handled, Propagation::Indeterminate),
            Self::UncertainContinues => (LayerStatus::Indeterminate, Propagation::Continues),
            Self::Unknown => (LayerStatus::Indeterminate, Propagation::Indeterminate),
            Self::Unavailable => (LayerStatus::Unavailable, Propagation::Indeterminate),
        }
    }
}

/// Explicit typed route order: remapper, compositor, terminal, IME, TTY,
/// multiplexer/session, application, shell.
pub const ROUTE_ORDER: &[LayerId] = &[
    LayerId::Remapper,
    LayerId::Compositor,
    LayerId::Terminal,
    LayerId::Ime,
    LayerId::Tty,
    LayerId::Multiplexer,
    LayerId::Session,
    LayerId::Application,
    LayerId::Shell,
];

/// One inspection request replacing positional arguments.
#[derive(Debug, Clone, Default)]
pub struct InspectRequest<'a> {
    pub key: Option<&'a KeyCombo>,
    pub force_continue: bool,
    pub protocol_flags: Option<u32>,
    pub observed_bytes: Option<&'a [u8]>,
    pub physical_input: Option<PhysicalInput>,
    pub termios: Option<&'a libc::termios>,
    pub application_pid: Option<u32>,
    pub application_source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LayerStatus {
    NotHandled,
    Handled,
    Indeterminate,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Propagation {
    Continues,
    Stops,
    Redirected,
    Indeterminate,
}

/// One layer finding. Route state is typed: `id` names the emitting layer
/// and `outcome` is the single control value. The legacy `status` and
/// `propagation` pair exists only as derived serialization output for
/// schema v1/v2 compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerResult {
    pub layer: &'static str,
    pub id: LayerId,
    pub outcome: Outcome,
    pub summary: String,
    pub details: Vec<String>,
}

impl Serialize for LayerResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (status, propagation) = self.outcome.as_parts();
        let mut state = serializer.serialize_struct("LayerResult", 5)?;
        state.serialize_field("layer", &self.layer)?;
        state.serialize_field("status", &status)?;
        state.serialize_field("propagation", &propagation)?;
        state.serialize_field("summary", &self.summary)?;
        state.serialize_field("details", &self.details)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalInput {
    pub device: Option<String>,
    pub keycode: crate::xkb::EvdevKeycode,
}

impl LayerResult {
    /// Legacy schema v1 status, derived from the typed outcome.
    pub fn status(&self) -> LayerStatus {
        self.outcome.as_parts().0
    }
    /// Legacy schema v1 propagation, derived from the typed outcome.
    pub fn propagation(&self) -> Propagation {
        self.outcome.as_parts().1
    }
    pub fn continues(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::Pass | Outcome::HandledAndPassed | Outcome::UncertainContinues
        )
    }
    pub fn blocks_chain(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::Consumed | Outcome::Redirected | Outcome::Unavailable
        )
    }
}

/// Unified terminal identity used by text reports, doctor, capabilities,
/// and schema context. Ghostty is a terminal variant, not a separate path.
pub fn terminal_identity() -> Option<&'static str> {
    if let Some(name) = terminal_app::detected_name() {
        return Some(name);
    }
    if ghostty::is_active() {
        return Some("Ghostty");
    }
    None
}

pub(crate) fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "ESC".to_owned(),
            0x20..=0x7e => char::from(*byte).to_string(),
            value => format!("0x{value:02x}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Inspect the default Hyprland -> Ghostty -> TTY -> session -> application
/// -> shell input chain.
///
/// A terminal adapter is the point where a logical key becomes terminal
/// bytes. Keeping that conversion here prevents the listener's Kitty probe
/// sequence from being mistaken for the input Bash would normally receive.
pub fn inspect_default_chain(key: &KeyCombo) -> Vec<LayerResult> {
    inspect_default_chain_inner(key, false, None, None, None, None, None)
}

/// Inspect the chain while resolving the interactive application from a
/// caller-selected process rather than whykey's own ancestry.
pub fn inspect_default_chain_for_pid(key: &KeyCombo, target_pid: u32) -> Vec<LayerResult> {
    inspect_default_chain_for_pid_with_source(key, target_pid, None)
}

pub fn inspect_default_chain_for_pid_with_source(
    key: &KeyCombo,
    target_pid: u32,
    source: Option<&str>,
) -> Vec<LayerResult> {
    // Keep downstream evidence visible even when an unrelated upstream layer
    // (for example, a missing controlling TTY in a CI process) is unavailable.
    // The report retains that uncertainty instead of hiding the selected
    // application's ancestry.
    inspect_default_chain_inner(
        key,
        true,
        None,
        None,
        None,
        None,
        Some(ApplicationTarget {
            pid: target_pid,
            source,
        }),
    )
}

/// Inspect one static chain with a shared wall-clock budget.
pub fn inspect_default_chain_with_deadline(
    key: &KeyCombo,
    timeout: std::time::Duration,
) -> Vec<LayerResult> {
    command::with_deadline(timeout, || inspect_default_chain(key))
}

pub fn inspect_default_chain_observed(
    key: &KeyCombo,
    observed_bytes: &[u8],
    protocol_flags: Option<u32>,
) -> Vec<LayerResult> {
    inspect_default_chain_observed_with_termios(key, observed_bytes, protocol_flags, None)
}

/// Inspect a captured terminal event while using the termios state that was
/// saved before raw capture. This lets repeated capture keep the terminal in
/// probe mode without making the TTY explanation describe the probe's raw
/// settings.
pub fn inspect_default_chain_observed_with_termios(
    key: &KeyCombo,
    observed_bytes: &[u8],
    protocol_flags: Option<u32>,
    termios: Option<&libc::termios>,
) -> Vec<LayerResult> {
    inspect_default_chain_inner(
        key,
        true,
        protocol_flags,
        Some(observed_bytes),
        None,
        termios,
        None,
    )
}

pub fn inspect_default_chain_evdev(
    key: &KeyCombo,
    device: impl Into<String>,
    keycode: crate::xkb::EvdevKeycode,
) -> Vec<LayerResult> {
    let session = crate::environment::Environment::collect();
    inspect_default_chain_evdev_with_session(key, device, keycode, &session)
}

/// Evdev inspection reusing one listener-session snapshot instead of
/// rediscovering the desktop per event.
pub fn inspect_default_chain_evdev_with_session(
    key: &KeyCombo,
    device: impl Into<String>,
    keycode: crate::xkb::EvdevKeycode,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    inspect_with_request_and_session(
        &InspectRequest {
            key: Some(key),
            force_continue: false,
            protocol_flags: None,
            observed_bytes: None,
            physical_input: Some(PhysicalInput {
                device: Some(device.into()),
                keycode,
            }),
            termios: None,
            application_pid: None,
            application_source: None,
        },
        session,
    )
}

pub fn inspect_default_chain_hyprland_with_session(
    key: &KeyCombo,
    keycode: crate::xkb::EvdevKeycode,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    inspect_with_request_and_session(
        &InspectRequest {
            key: Some(key),
            force_continue: false,
            protocol_flags: None,
            observed_bytes: None,
            physical_input: Some(PhysicalInput {
                device: None,
                keycode,
            }),
            termios: None,
            application_pid: None,
            application_source: None,
        },
        session,
    )
}

/// Captured-terminal inspection reusing one listener-session snapshot. The
/// saved termios state still describes the terminal before raw capture, so
/// repeated capture keeps probe mode without re-running session discovery.
pub fn inspect_default_chain_observed_with_session(
    key: &KeyCombo,
    observed_bytes: &[u8],
    protocol_flags: Option<u32>,
    termios: Option<&libc::termios>,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    inspect_with_request_and_session(
        &InspectRequest {
            key: Some(key),
            force_continue: true,
            protocol_flags,
            observed_bytes: Some(observed_bytes),
            physical_input: None,
            termios,
            application_pid: None,
            application_source: None,
        },
        session,
    )
}

fn inspect_default_chain_inner(
    key: &KeyCombo,
    force_continue: bool,
    protocol_flags: Option<u32>,
    observed_bytes: Option<&[u8]>,
    physical_input: Option<PhysicalInput>,
    termios: Option<&libc::termios>,
    application_target: Option<ApplicationTarget<'_>>,
) -> Vec<LayerResult> {
    let request = InspectRequest {
        key: Some(key),
        force_continue,
        protocol_flags,
        observed_bytes,
        physical_input,
        termios,
        application_pid: application_target.map(|target| target.pid),
        application_source: application_target.and_then(|target| target.source),
    };
    // Single discovery pass per inspection: the environment snapshot runs
    // each adapter probe once and owns the selected compositor.
    let session = crate::environment::Environment::collect();
    inspect_with_request_and_session(&request, &session)
}
fn inspect_with_request_and_session(
    request: &InspectRequest<'_>,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    let remapper_result = (!command::deadline_exceeded()).then(|| {
        crate::remapper::inspect_with_detections(
            request.physical_input.as_ref(),
            &session.remappers,
        )
    });
    let ime_result =
        (!command::deadline_exceeded()).then(|| crate::ime::inspect_with_detections(&session.ime));
    let mut results = inspect_default_chain_inner_without_remapper(request, session);
    if remapper_result
        .as_ref()
        .is_some_and(|result| result.outcome != Outcome::Pass)
    {
        results.insert(0, remapper_result.expect("remapper result is present"));
    }
    if ime_result
        .as_ref()
        .is_some_and(|result| result.outcome != Outcome::Pass)
    {
        if let Some(insert_at) = results.iter().position(|result| result.id == LayerId::Tty) {
            results.insert(insert_at, ime_result.expect("IME result is present"));
        }
    }
    results
}

fn inspect_default_chain_inner_without_remapper(
    request: &InspectRequest<'_>,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    let key = request.key.expect("inspect request holds a key");
    let force_continue = request.force_continue;
    let protocol_flags = request.protocol_flags;
    let observed_bytes = request.observed_bytes;
    let physical_input = request.physical_input.clone();
    let termios = request.termios;
    let application_target = request.application_pid.map(|pid| ApplicationTarget {
        pid,
        source: request.application_source,
    });
    if command::deadline_exceeded() {
        return vec![deadline_result()];
    }
    // The caller owns session discovery: reuse the snapshot instead of
    // running every adapter probe again for this inspection. The registry
    // owns the inspect operation for the selected desktop adapter.
    let compositor_result = match session.selected_compositor {
        Some(id) => {
            let entry = crate::registry::DESKTOPS
                .iter()
                .find(|entry| entry.id == id)
                .unwrap_or_else(|| panic!("selected compositor '{id}' is a registry adapter"));
            (entry.inspect)(key, physical_input.as_ref())
        }
        _ if hyprland::remote_session_without_compositor() => {
            // Keep the explicit SSH explanation for remote terminals while
            // avoiding a fabricated Hyprland failure for ordinary consoles.
            hyprland::Hyprland.inspect_with_input(key, physical_input.as_ref())
        }
        _ => compositor::not_detected(),
    };
    if command::deadline_exceeded() {
        return vec![compositor_result, deadline_result()];
    }
    // One terminal path: route evidence and normal PTY bytes come from a
    // single dispatcher analysis with one config load per inspection.
    let terminal = terminal::Terminal;

    if matches!(
        compositor_result.outcome,
        Outcome::Consumed | Outcome::Redirected
    ) && !force_continue
    {
        return vec![compositor_result];
    }

    let mut emulator = terminal_dispatch::TerminalAnalysis::analyze(key, protocol_flags);
    if command::deadline_exceeded() {
        let mut results = vec![compositor_result, emulator.layer];
        results.push(deadline_result());
        return results;
    }
    if matches!(
        emulator.layer.outcome,
        Outcome::Consumed | Outcome::Redirected
    ) && !force_continue
    {
        return vec![compositor_result, emulator.layer];
    }

    let observed_fallback = observed_bytes.map(|bytes| TerminalInput {
        bytes: bytes.to_vec(),
        description: "captured bytes; normal encoding is unknown".into(),
        confidence: InputConfidence::Observed,
    });
    let Some(input) = emulator.input.clone().or(observed_fallback) else {
        return vec![compositor_result, emulator.layer];
    };
    if protocol_flags.is_some() {
        emulator
            .layer
            .details
            .retain(|detail| !detail.starts_with("byte:") && !detail.starts_with("sequence:"));
        emulator.layer.details.push(format!(
            "normal input for the existing keyboard protocol: {}",
            input.display_bytes()
        ));
    }

    let terminal_result = termios
        .map(|termios| terminal.inspect_input_with_termios(&input, termios))
        .unwrap_or_else(|| terminal.inspect_input(&input));
    if command::deadline_exceeded() {
        let mut results = vec![compositor_result, emulator.layer];
        results.extend([terminal_result, deadline_result()]);
        return results;
    }
    if terminal_result.blocks_chain() && !force_continue {
        let mut results = vec![compositor_result, emulator.layer];
        results.push(terminal_result);
        return results;
    }

    let session_result = session::inspect(key);
    if command::deadline_exceeded() {
        let mut results = vec![compositor_result, emulator.layer];
        results.extend([terminal_result, session_result, deadline_result()]);
        return results;
    }
    if session_result.blocks_chain() && !force_continue {
        let mut results = vec![compositor_result, emulator.layer];
        results.extend([terminal_result, session_result]);
        return results;
    }

    let application_result = application_target.map_or_else(
        || application::inspect(key),
        |target| application::inspect_for_pid_with_source(key, target.pid, target.source),
    );
    if command::deadline_exceeded() {
        let mut results = vec![compositor_result, emulator.layer];
        results.extend([
            terminal_result,
            session_result,
            application_result,
            deadline_result(),
        ]);
        return results;
    }
    if application_result.blocks_chain() && !force_continue {
        let mut results = vec![compositor_result, emulator.layer];
        results.extend([terminal_result, session_result, application_result]);
        return results;
    }

    let shell_result = application_target.map_or_else(
        || shell::inspect_input(&input),
        |target| shell::inspect_input_for_pid(&input, target.pid),
    );
    let mut results = vec![compositor_result, emulator.layer];
    results.extend([
        terminal_result,
        session_result,
        application_result,
        shell_result,
    ]);
    if command::deadline_exceeded() {
        results.push(deadline_result());
    }
    results
}

fn deadline_result() -> LayerResult {
    LayerResult {
        layer: "Diagnostic budget",
        id: LayerId::Diagnostic,
        outcome: Outcome::Unavailable,
        summary: "static diagnostic deadline exceeded".into(),
        details: vec![
            "later layers were not queried".into(),
            "increase WHYKEY_DIAGNOSTIC_TIMEOUT_MS for this session if needed".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_consistently() {
        assert_eq!(format_bytes(&[0x1b, b'[', b'1', 0x1a]), "ESC [ 1 0x1a");
    }

    #[test]
    fn keeps_session_after_the_terminal() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers = inspect_default_chain(&key);
        let names: Vec<_> = layers.iter().map(|layer| layer.layer).collect();
        if let Some(session_index) = names.iter().position(|name| *name == "Session context") {
            assert!(names[..session_index].contains(&"TTY driver"));
        }
    }

    #[test]
    fn reports_an_exhausted_static_diagnostic_budget() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers =
            command::with_deadline(std::time::Duration::ZERO, || inspect_default_chain(&key));

        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, "Diagnostic budget");
        assert_eq!(layers[0].status(), LayerStatus::Unavailable);
    }

    #[test]
    fn outcome_round_trips_through_serialized_fields() {
        for outcome in [
            Outcome::Pass,
            Outcome::HandledAndPassed,
            Outcome::Consumed,
            Outcome::Redirected,
            Outcome::HandledUncertain,
            Outcome::UncertainContinues,
            Outcome::Unknown,
            Outcome::Unavailable,
        ] {
            let (status, propagation) = outcome.as_parts();
            assert_eq!(Outcome::from_parts(&status, &propagation), outcome);
        }
    }

    #[test]
    fn layer_ids_cover_typed_route_order() {
        assert!(ROUTE_ORDER.contains(&LayerId::Tty));
        assert!(ROUTE_ORDER.contains(&LayerId::Ime));
        let tty = ROUTE_ORDER
            .iter()
            .position(|id| *id == LayerId::Tty)
            .unwrap();
        let ime = ROUTE_ORDER
            .iter()
            .position(|id| *id == LayerId::Ime)
            .unwrap();
        assert!(ime < tty, "IME must precede the TTY stage");
        for id in [
            LayerId::Remapper,
            LayerId::Compositor,
            LayerId::Terminal,
            LayerId::Multiplexer,
            LayerId::Session,
            LayerId::Application,
            LayerId::Shell,
        ] {
            assert!(
                ROUTE_ORDER.contains(&id),
                "route order must name {id:?} explicitly"
            );
        }
    }

    #[test]
    fn emitted_layers_never_fall_back_to_an_unknown_id() {
        for key in ["ctrl+f", "ctrl+z", "super+c", "super+return", "ctrl+left"] {
            let key: KeyCombo = key.parse().unwrap();
            let layers = inspect_default_chain(&key);
            assert!(!layers.is_empty());
            for layer in &layers {
                assert_ne!(
                    layer.id,
                    LayerId::Unknown,
                    "layer {:?} must carry an explicit identity",
                    layer.layer
                );
            }
        }
    }

    #[test]
    fn ime_result_is_placed_before_the_tty_stage() {
        let tty = LayerResult {
            layer: "TTY driver",
            id: LayerId::Tty,
            outcome: Outcome::Pass,
            summary: "tty".into(),
            details: Vec::new(),
        };
        let ime = LayerResult {
            layer: "IME",
            id: LayerId::Ime,
            outcome: Outcome::HandledAndPassed,
            summary: "ime".into(),
            details: Vec::new(),
        };
        let mut results = vec![tty];
        let insert_at = results
            .iter()
            .position(|result| result.id == LayerId::Tty)
            .unwrap_or(results.len());
        results.insert(insert_at, ime);
        let ids: Vec<LayerId> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, vec![LayerId::Ime, LayerId::Tty]);
    }

    #[test]
    fn unified_terminal_identity_matches_schema_context() {
        let identity = terminal_identity();
        let context = crate::schema::context();
        assert_eq!(
            context.get("terminal").and_then(|value| value.as_str()),
            identity,
            "schema context and text reports must name the same adapter"
        );
    }
}
