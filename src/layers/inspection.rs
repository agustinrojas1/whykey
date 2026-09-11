use super::model::*;
use super::*;
use crate::command;
use crate::key::KeyCombo;

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

/// Inspect the default Hyprland -> Ghostty -> TTY -> session -> application
/// -> shell input chain.
///
/// A terminal adapter is the point where a logical key becomes terminal
/// bytes. Keeping that conversion here prevents the listener's Kitty probe
/// sequence from being mistaken for the input Bash would normally receive.
/// Inspect one key combination through the default chain.
pub fn inspect_default_chain(key: &KeyCombo) -> Vec<LayerResult> {
    inspect(&InspectRequest::new(key))
}

/// Inspect the chain while resolving the interactive application from a
/// caller-selected process rather than whykey's own ancestry.
pub fn inspect_default_chain_for_pid(key: &KeyCombo, target_pid: u32) -> Vec<LayerResult> {
    let mut request = InspectRequest::new(key);
    request.force_continue = true;
    request.application_pid = Some(target_pid);
    inspect(&request)
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
    let mut request = InspectRequest::new(key);
    request.force_continue = true;
    request.application_pid = Some(target_pid);
    request.application_source = source;
    inspect(&request)
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
    let mut request = InspectRequest::new(key);
    request.force_continue = true;
    request.protocol_flags = protocol_flags;
    request.observed_bytes = Some(observed_bytes);
    request.termios = termios;
    inspect(&request)
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
    let mut request = InspectRequest::new(key);
    request.physical_input = Some(PhysicalInput {
        device: Some(device.into()),
        keycode,
    });
    inspect_with_environment(&request, session)
}

pub fn inspect_default_chain_hyprland_with_session(
    key: &KeyCombo,
    keycode: crate::xkb::EvdevKeycode,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    let mut request = InspectRequest::new(key);
    request.physical_input = Some(PhysicalInput {
        device: None,
        keycode,
    });
    inspect_with_environment(&request, session)
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
    let mut request = InspectRequest::new(key);
    request.force_continue = true;
    request.protocol_flags = protocol_flags;
    request.observed_bytes = Some(observed_bytes);
    request.termios = termios;
    inspect_with_environment(&request, session)
}

/// One production inspection path: a typed request plus a fresh environment
/// snapshot. All public entry points above build a request and land here.
pub fn inspect(request: &InspectRequest<'_>) -> Vec<LayerResult> {
    // Single discovery pass per inspection: the environment snapshot runs
    // each adapter probe once and owns the selected compositor.
    let session = crate::environment::Environment::collect();
    inspect_with_environment(request, &session)
}

pub(crate) fn inspect_with_environment(
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

/// Continuation policy defines when a layer finding halts further inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationPolicy {
    /// Only genuine event interception (consumption or redirection) halts inspection.
    /// Unavailable or unhandled states do not block downstream terminal/TTY inspection.
    StopOnTerminalInterception,
    /// Any blocking outcome (consumed, redirected, or unavailable) halts inspection.
    StopOnBlockingResult,
}

impl ContinuationPolicy {
    pub fn should_stop(self, result: &LayerResult, force_continue: bool) -> bool {
        if force_continue {
            return false;
        }
        match self {
            Self::StopOnTerminalInterception => {
                matches!(result.outcome, Outcome::Consumed | Outcome::Redirected)
            }
            Self::StopOnBlockingResult => result.blocks_chain(),
        }
    }
}

/// Accumulates route results incrementally without reconstructing prefixes.
pub(crate) struct RouteAccumulator {
    results: Vec<LayerResult>,
    force_continue: bool,
}

impl RouteAccumulator {
    pub fn new(force_continue: bool) -> Self {
        Self {
            results: Vec::new(),
            force_continue,
        }
    }

    /// Appends a layer result according to its continuation policy.
    /// Returns true if inspection should continue, false if it halted.
    pub fn append(&mut self, result: LayerResult, policy: ContinuationPolicy) -> bool {
        let should_stop = policy.should_stop(&result, self.force_continue);
        self.results.push(result);
        !should_stop
    }

    /// Records static diagnostic deadline exhaustion if exceeded.
    /// Returns true if the budget was exceeded.
    pub fn check_deadline(&mut self) -> bool {
        if command::deadline_exceeded() {
            self.results.push(deadline_result());
            true
        } else {
            false
        }
    }

    pub fn finish(self) -> Vec<LayerResult> {
        self.results
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetEndpoint {
    InteractiveApplication,
    Shell,
}

pub(crate) fn select_target_endpoint(
    application_target: Option<&ApplicationTarget<'_>>,
    application_result: &LayerResult,
) -> TargetEndpoint {
    match application_target {
        Some(target) => {
            if application::is_shell_process(target.pid) {
                TargetEndpoint::Shell
            } else {
                TargetEndpoint::InteractiveApplication
            }
        }
        None => {
            if application_result.id == LayerId::Application
                && application_result.outcome != Outcome::Pass
            {
                TargetEndpoint::InteractiveApplication
            } else {
                TargetEndpoint::Shell
            }
        }
    }
}

fn inspect_compositor(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    session: &crate::environment::Environment,
) -> LayerResult {
    let extension = || {
        session
            .extension_adapters
            .iter()
            .find(|adapter| {
                crate::extension_adapters::applicable_with_context(
                    adapter,
                    &session.compositor_context,
                )
            })
            .map(|adapter| {
                crate::extension_adapters::inspect_with_report(
                    adapter,
                    key,
                    session.extension_inventory(adapter),
                )
            })
    };
    match session.selected_compositor {
        Some("compositor") | None => extension().unwrap_or_else(|| {
            if session.ssh && session.selected_compositor.is_none() {
                // Keep the explicit SSH explanation for remote terminals while
                // avoiding a fabricated compositor failure for ordinary consoles.
                compositor::remote_without_local_adapter()
            } else {
                compositor::inspect()
            }
        }),
        Some(id) => {
            let entry = crate::registry::DESKTOPS
                .iter()
                .find(|entry| entry.id == id)
                .unwrap_or_else(|| panic!("selected compositor '{id}' has no adapter descriptor"));
            if let Some(inspect) = entry.inspect_with_context {
                inspect(key, physical_input, session)
            } else {
                (entry.inspect)(key, physical_input)
            }
        }
    }
}

fn inspect_default_chain_inner_without_remapper(
    request: &InspectRequest<'_>,
    session: &crate::environment::Environment,
) -> Vec<LayerResult> {
    let key = request.key;
    let mut acc = RouteAccumulator::new(request.force_continue);

    if acc.check_deadline() {
        return acc.finish();
    }

    // 1. Compositor
    let compositor_result = inspect_compositor(key, request.physical_input.as_ref(), session);
    if !acc.append(
        compositor_result,
        ContinuationPolicy::StopOnTerminalInterception,
    ) {
        return acc.finish();
    }
    if acc.check_deadline() {
        return acc.finish();
    }

    // 2. Terminal emulator
    let mut emulator = terminal_dispatch::TerminalAnalysis::analyze(key, request.protocol_flags);
    let observed_fallback = request.observed_bytes.map(|bytes| TerminalInput {
        bytes: bytes.to_vec(),
        description: "captured bytes; normal encoding is unknown".into(),
        confidence: InputConfidence::Observed,
    });
    let input = emulator.input.clone().or(observed_fallback);
    let bytes_available = input.is_some();
    if request.protocol_flags.is_some() {
        if let Some(input) = &input {
            emulator
                .layer
                .details
                .retain(|detail| !detail.starts_with("byte:") && !detail.starts_with("sequence:"));
            emulator.layer.details.push(format!(
                "normal input for the existing keyboard protocol: {}",
                input.display_bytes()
            ));
        }
    }
    if !acc.append(
        emulator.layer,
        ContinuationPolicy::StopOnTerminalInterception,
    ) {
        return acc.finish();
    }
    if acc.check_deadline() {
        return acc.finish();
    }

    // 3. TTY driver
    let terminal = terminal::Terminal;
    let terminal_result = match (input.as_ref(), request.termios) {
        (Some(input), Some(termios)) => terminal.inspect_input_with_termios(input, termios),
        (Some(input), None) => terminal.inspect_input(input),
        (None, _) => terminal.inspect_unknown(),
    };
    if !acc.append(terminal_result, ContinuationPolicy::StopOnBlockingResult) {
        return acc.finish();
    }
    if acc.check_deadline() {
        return acc.finish();
    }

    // 4. Session / multiplexer
    let session_result = session::inspect_with_env(key, bytes_available, session);
    if !acc.append(session_result, ContinuationPolicy::StopOnBlockingResult) {
        return acc.finish();
    }
    if acc.check_deadline() {
        return acc.finish();
    }

    // 5. Application
    let application_target = request.application_pid.map(|pid| ApplicationTarget {
        pid,
        source: request.application_source,
    });
    let application_result = application_target.map_or_else(
        || application::inspect(key),
        |target| application::inspect_for_pid_with_source(key, target.pid, target.source),
    );
    let endpoint = select_target_endpoint(application_target.as_ref(), &application_result);
    if !acc.append(application_result, ContinuationPolicy::StopOnBlockingResult) {
        return acc.finish();
    }
    if acc.check_deadline() {
        return acc.finish();
    }

    // 6. Shell (if endpoint is shell)
    if endpoint == TargetEndpoint::Shell {
        let shell_result = match (input.as_ref(), application_target) {
            (Some(input), Some(target)) => shell::inspect_input_for_pid(input, target.pid),
            (Some(input), None) => shell::inspect_input(input),
            (None, _) => shell::inspect_unknown(),
        };
        acc.append(shell_result, ContinuationPolicy::StopOnBlockingResult);
        acc.check_deadline();
    }

    acc.finish()
}

pub(crate) fn deadline_result() -> LayerResult {
    LayerResult::unavailable(
        "Diagnostic budget",
        LayerId::Diagnostic,
        "static diagnostic deadline exceeded",
        vec![
            "later layers were not queried".into(),
            "increase WHYKEY_DIAGNOSTIC_TIMEOUT_MS for this session if needed".into(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_result(outcome: Outcome) -> LayerResult {
        LayerResult::new("Test", LayerId::Unknown, outcome, "test", Vec::new())
    }

    #[test]
    fn continuation_policy_terminal_interception() {
        let policy = ContinuationPolicy::StopOnTerminalInterception;
        // Stops only on Consumed or Redirected when force_continue is false
        assert!(policy.should_stop(&dummy_result(Outcome::Consumed), false));
        assert!(policy.should_stop(&dummy_result(Outcome::Redirected), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::Pass), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::HandledAndPassed), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::HandledUncertain), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::UncertainContinues), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::Unknown), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::Unavailable), false));

        // When force_continue is true, never stops
        assert!(!policy.should_stop(&dummy_result(Outcome::Consumed), true));
        assert!(!policy.should_stop(&dummy_result(Outcome::Redirected), true));
    }

    #[test]
    fn continuation_policy_blocking_result() {
        let policy = ContinuationPolicy::StopOnBlockingResult;
        // Stops on Consumed, Redirected, or Unavailable
        assert!(policy.should_stop(&dummy_result(Outcome::Consumed), false));
        assert!(policy.should_stop(&dummy_result(Outcome::Redirected), false));
        assert!(policy.should_stop(&dummy_result(Outcome::Unavailable), false));

        // Passes on other outcomes
        assert!(!policy.should_stop(&dummy_result(Outcome::Pass), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::HandledAndPassed), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::HandledUncertain), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::UncertainContinues), false));
        assert!(!policy.should_stop(&dummy_result(Outcome::Unknown), false));

        // When force_continue is true, never stops
        assert!(!policy.should_stop(&dummy_result(Outcome::Consumed), true));
        assert!(!policy.should_stop(&dummy_result(Outcome::Redirected), true));
        assert!(!policy.should_stop(&dummy_result(Outcome::Unavailable), true));
    }

    #[test]
    fn route_accumulator_appends_and_stops_cleanly() {
        let mut acc = RouteAccumulator::new(false);
        let continues = acc.append(
            dummy_result(Outcome::Pass),
            ContinuationPolicy::StopOnBlockingResult,
        );
        assert!(continues);
        assert_eq!(acc.results.len(), 1);

        let stops = acc.append(
            dummy_result(Outcome::Consumed),
            ContinuationPolicy::StopOnBlockingResult,
        );
        assert!(!stops);
        assert_eq!(acc.results.len(), 2);
    }

    #[test]
    fn target_endpoint_selection() {
        let app_handled = LayerResult::new(
            "Neovim",
            LayerId::Application,
            Outcome::HandledUncertain,
            "mode-dependent",
            Vec::new(),
        );
        let app_pass = dummy_result(Outcome::Pass);

        // Without explicit target, app match selects InteractiveApplication
        assert_eq!(
            select_target_endpoint(None, &app_handled),
            TargetEndpoint::InteractiveApplication
        );
        // Without explicit target, app pass selects Shell
        assert_eq!(
            select_target_endpoint(None, &app_pass),
            TargetEndpoint::Shell
        );

        // With explicit non-shell target, always selects InteractiveApplication
        let editor_target = ApplicationTarget {
            pid: 99999999,
            source: None,
        };
        assert_eq!(
            select_target_endpoint(Some(&editor_target), &app_pass),
            TargetEndpoint::InteractiveApplication
        );
    }
}
