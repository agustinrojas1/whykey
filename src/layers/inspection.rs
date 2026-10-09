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
    inspect_step(request, session, &mut StepDelivery::default())
}

/// Inspect every step of a key sequence against one environment snapshot.
/// Stateful layers receive the earlier steps they actually saw: tmux/Screen
/// get the keys that reached the multiplexer (so `ctrl+b ctrl+left` resolves
/// in the prefix table) and the shell gets the input of every step that
/// reached it (so `ctrl+x ctrl+e` resolves as one Readline sequence).
/// `target` selects the application process (`--pid`/`--focused`).
pub fn inspect_sequence(
    keys: &[KeyCombo],
    target: Option<(u32, Option<&str>)>,
) -> Vec<Vec<LayerResult>> {
    let session = crate::environment::Environment::collect();
    let mut prefix = SequencePrefix::default();
    keys.iter()
        .enumerate()
        .map(|(index, key)| {
            let mut request = InspectRequest::new(key);
            if let Some((pid, source)) = target {
                request.force_continue = true;
                request.application_pid = Some(pid);
                request.application_source = source;
            }
            request.sequence_prefix = (index > 0).then_some(&prefix);
            let mut delivery = StepDelivery::default();
            let results = inspect_step(&request, &session, &mut delivery);
            delivery.record(&mut prefix);
            results
        })
        .collect()
}

/// What one sequence step delivered to the stateful layers. A layer counts
/// as reached only when no earlier layer consumed or redirected the key.
#[derive(Debug, Default)]
struct StepDelivery {
    multiplexer_key: Option<KeyCombo>,
    /// `Some(None)`: the shell received the key, but its bytes are unknown.
    shell_input: Option<Option<TerminalInput>>,
}

impl StepDelivery {
    fn record(self, prefix: &mut SequencePrefix) {
        prefix.multiplexer_keys.extend(self.multiplexer_key);
        match self.shell_input {
            Some(Some(input)) => prefix.shell_inputs.push(input),
            Some(None) => prefix.shell_input_unknown = true,
            None => {}
        }
    }
}

fn inspect_step(
    request: &InspectRequest<'_>,
    session: &crate::environment::Environment,
    delivery: &mut StepDelivery,
) -> Vec<LayerResult> {
    let generations_before = session.reload_generations();
    let remapper_result = (!command::deadline_exceeded()).then(|| {
        crate::remapper::inspect_with_detections(
            request.physical_input.as_ref(),
            &session.remappers,
        )
    });
    let mut results = route_after_remapper(remapper_result, request, |request| {
        inspect_default_chain_inner_without_remapper(request, session, delivery)
    });
    let generations_after = session.reload_generations();
    mark_stale_generation_evidence(&mut results, &generations_before, &generations_after);
    results
}

/// Apply the pre-compositor remapper finding to the rest of the route. A
/// consuming remapper stops the chain; a redirecting remapper sends the
/// remapped key downstream, or makes downstream findings conditional when
/// its target is not a key combination.
fn route_after_remapper(
    remapper: Option<LayerResult>,
    request: &InspectRequest<'_>,
    downstream: impl FnOnce(&InspectRequest<'_>) -> Vec<LayerResult>,
) -> Vec<LayerResult> {
    let Some(mut remapper) = remapper.filter(|result| result.outcome != Outcome::Pass) else {
        return downstream(request);
    };
    if remapper.outcome == Outcome::Redirected {
        let target = remapper
            .binding
            .as_ref()
            .and_then(|binding| binding.action.as_deref())
            .and_then(|action| KeyCombo::parse_query(action).ok());
        return match target {
            Some(target) => {
                remapper.details.push(format!(
                    "downstream layers inspect the remapped key {target}"
                ));
                let redirected = InspectRequest {
                    key: &target,
                    // The captured keycode belongs to the original physical key.
                    physical_input: None,
                    ..request.clone()
                };
                let mut results = vec![remapper];
                results.extend(downstream(&redirected));
                results
            }
            None => {
                remapper.details.push(
                    "remap target is not a key combination; downstream findings for the original key are conditional"
                        .into(),
                );
                let mut results = vec![remapper];
                results.extend(downstream(request).into_iter().map(conditional_on_remap));
                results
            }
        };
    }
    let stop = ContinuationPolicy::StopOnTerminalInterception
        .should_stop(&remapper, request.force_continue);
    let mut results = vec![remapper];
    if !stop {
        results.extend(downstream(request));
    }
    results
}

/// Downgrade a finding for a key an upstream remapper redirected to an
/// unknown target: the original key may never arrive.
fn conditional_on_remap(mut result: LayerResult) -> LayerResult {
    result.outcome = match result.outcome {
        Outcome::Pass => Outcome::UncertainContinues,
        Outcome::HandledAndPassed | Outcome::Consumed | Outcome::Redirected => {
            Outcome::HandledUncertain
        }
        other => other,
    };
    result
        .details
        .push("conditional: an upstream remapper redirects this key".into());
    result
}

fn mark_stale_generation_evidence(
    results: &mut [LayerResult],
    before: &[(&'static str, Option<String>)],
    after: &[(&'static str, Option<String>)],
) {
    let changed = before
        .iter()
        .zip(after)
        .filter(
            |((before_id, before_generation), (after_id, after_generation))| {
                before_id == after_id && before_generation != after_generation
            },
        )
        .map(|(entry, _)| entry.0)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return;
    }
    let detail = format!(
        "binding evidence may be stale: adapter configuration generation changed ({})",
        changed.join(", ")
    );
    if let Some(compositor) = results
        .iter_mut()
        .find(|result| result.id == LayerId::Compositor)
    {
        compositor.details.push(detail);
    } else if let Some(first) = results.first_mut() {
        first.details.push(detail);
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    #[test]
    fn generation_bump_marks_evidence_stale() {
        let mut results = vec![LayerResult::new(
            "Compositor",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "binding found",
            vec![],
        )];
        mark_stale_generation_evidence(
            &mut results,
            &[("test", Some("1".into()))],
            &[("test", Some("2".into()))],
        );
        assert!(results[0].details[0].contains("may be stale"));
    }

    #[test]
    fn unchanged_generation_keeps_results_unchanged() {
        let mut results = vec![LayerResult::new(
            "Compositor",
            LayerId::Compositor,
            Outcome::Pass,
            "no binding",
            vec![],
        )];
        mark_stale_generation_evidence(
            &mut results,
            &[("test", Some("1".into()))],
            &[("test", Some("1".into()))],
        );
        assert!(results[0].details.is_empty());
    }
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

    /// True once a recorded layer consumed or redirected the key, so later
    /// layers are analyzed (with `force_continue`) but never received it.
    pub fn intercepted(&self) -> bool {
        self.results
            .iter()
            .any(|result| matches!(result.outcome, Outcome::Consumed | Outcome::Redirected))
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
    // Only the selected (highest-scored applicable) manifest runs its
    // bindings command; inapplicable manifests are never executed.
    let extension = || {
        let selected = session.selected_extension.as_deref()?;
        session
            .extension_adapters
            .iter()
            .find(|adapter| adapter.id == selected)
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
    delivery: &mut StepDelivery,
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
    let terminal_continues = acc.append(
        emulator.layer,
        ContinuationPolicy::StopOnTerminalInterception,
    );
    // The input method is a client of the terminal: it sees every key the
    // compositor delivers to the terminal, including keys the terminal then
    // consumes, so its finding is kept even when the route stops here. Any
    // detected IME is reported, even when inactive, so its evidence stays
    // visible in verbose and JSON output.
    if !session.ime.is_empty()
        && !acc.append(
            crate::ime::inspect_with_detections(&session.ime),
            ContinuationPolicy::StopOnTerminalInterception,
        )
    {
        return acc.finish();
    }
    if !terminal_continues || acc.check_deadline() {
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
    let preceding = request
        .sequence_prefix
        .map_or(&[][..], |prefix| prefix.multiplexer_keys.as_slice());
    if !acc.intercepted() {
        delivery.multiplexer_key = Some(key.clone());
    }
    let session_result = session::inspect_with_env(preceding, key, bytes_available, session);
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
        if !acc.intercepted() {
            delivery.shell_input = Some(input.clone());
        }
        let shell_result = inspect_shell(
            request.sequence_prefix,
            input.as_ref(),
            application_target.map(|target| target.pid),
        );
        acc.append(shell_result, ContinuationPolicy::StopOnBlockingResult);
        acc.check_deadline();
    }

    acc.finish()
}

/// Shell editors interpret multi-key sequences, so a later sequence step is
/// evaluated together with the input of the earlier steps the shell received.
fn inspect_shell(
    sequence_prefix: Option<&SequencePrefix>,
    input: Option<&TerminalInput>,
    target_pid: Option<u32>,
) -> LayerResult {
    let earlier = sequence_prefix
        .filter(|prefix| !prefix.shell_inputs.is_empty() || prefix.shell_input_unknown);
    match (earlier, input) {
        (Some(prefix), Some(input)) if !prefix.shell_input_unknown => {
            let mut inputs = prefix.shell_inputs.clone();
            inputs.push(input.clone());
            shell::inspect_sequence(&inputs, target_pid)
        }
        (Some(_), _) | (None, None) => shell::inspect_unknown(),
        (None, Some(input)) => match target_pid {
            Some(pid) => shell::inspect_input_for_pid(input, pid),
            None => shell::inspect_input(input),
        },
    }
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

    fn remapper_result(outcome: Outcome, target: Option<&str>) -> LayerResult {
        let result = LayerResult::new(
            "Input remapper",
            LayerId::Remapper,
            outcome,
            "remapper",
            Vec::new(),
        );
        match target {
            Some(target) => {
                result.with_binding(BindingEvidence::action(target, BindingScope::Unknown, None))
            }
            None => result,
        }
    }

    /// Downstream stand-in that reports which key it was asked about.
    fn echo_downstream(request: &InspectRequest<'_>) -> Vec<LayerResult> {
        vec![LayerResult::new(
            "Compositor",
            LayerId::Compositor,
            Outcome::Pass,
            request.key.to_string(),
            Vec::new(),
        )]
    }

    #[test]
    fn consuming_remapper_stops_the_chain() {
        let key: KeyCombo = "f12".parse().unwrap();
        let request = InspectRequest::new(&key);
        let results = route_after_remapper(
            Some(remapper_result(Outcome::Consumed, None)),
            &request,
            |_| panic!("downstream layers must not see a consumed key"),
        );
        let ids: Vec<_> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, vec![LayerId::Remapper]);
    }

    #[test]
    fn redirecting_remapper_retargets_downstream_layers() {
        let key: KeyCombo = "f12".parse().unwrap();
        let request = InspectRequest::new(&key);
        let results = route_after_remapper(
            Some(remapper_result(Outcome::Redirected, Some("escape"))),
            &request,
            echo_downstream,
        );
        assert_eq!(results[0].id, LayerId::Remapper);
        let expected: KeyCombo = "escape".parse().unwrap();
        assert_eq!(results[1].summary, expected.to_string());
        assert_eq!(results[1].outcome, Outcome::Pass);
    }

    #[test]
    fn redirect_to_unparsed_target_makes_downstream_conditional() {
        let key: KeyCombo = "f12".parse().unwrap();
        let request = InspectRequest::new(&key);
        let results = route_after_remapper(
            Some(remapper_result(Outcome::Redirected, Some("layer(nav)"))),
            &request,
            echo_downstream,
        );
        assert_eq!(results[1].summary, key.to_string());
        assert_eq!(results[1].outcome, Outcome::UncertainContinues);
    }

    #[test]
    fn passing_remapper_is_omitted() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let request = InspectRequest::new(&key);
        let results = route_after_remapper(
            Some(remapper_result(Outcome::Pass, None)),
            &request,
            echo_downstream,
        );
        let ids: Vec<_> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, vec![LayerId::Compositor]);
    }
}
