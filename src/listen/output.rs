use super::protocol::{flush_input, remaining_millis};
use super::terminal::{SignalGuard, TerminalBackend, TerminalSession};
use super::*;
use std::io::IsTerminal as _;

/// One stage of the default capture chain and why it is (not) used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChainStage {
    pub(super) name: &'static str,
    pub(super) verdict: String,
}

/// The default capture chain as `listen` would walk it: native compositor
/// capture, then the probe window, then the controlling terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapturePlan {
    pub(super) stages: Vec<ChainStage>,
    /// The backend the chain settles on, or why no backend can be used.
    pub(super) selected: Result<String, String>,
}

/// Decide the default chain from what each stage reported. `native` is the
/// native backend that accepted a connection, or why none did; `display` is
/// the display protocol a probe window would use, or why it is skipped.
pub(super) fn plan_default_chain(
    native: Result<&str, &str>,
    display: Result<&str, &str>,
    tty: bool,
    policy: CapturePolicy,
    events_all: bool,
) -> CapturePlan {
    let stage = |name, verdict: String| ChainStage { name, verdict };
    let native_error = match native {
        Ok(backend) => {
            return CapturePlan {
                stages: vec![
                    stage(
                        "native compositor",
                        format!("selected: {backend} IPC accepted the capture connection"),
                    ),
                    stage("probe window", "not needed".into()),
                    stage("terminal", "not needed".into()),
                ],
                selected: Ok(backend.to_owned()),
            };
        }
        Err(error) => error,
    };
    let mut stages = vec![stage(
        "native compositor",
        format!("unavailable: {native_error}"),
    )];
    if policy == CapturePolicy::Suppress {
        stages.push(stage(
            "probe window",
            "not tried: --suppress never falls back to capture that cannot suppress".into(),
        ));
        stages.push(stage(
            "terminal",
            "not tried: --suppress never falls back to capture that cannot suppress".into(),
        ));
        return CapturePlan {
            stages,
            selected: Err(format!(
                "--suppress requires native compositor capture: {native_error}"
            )),
        };
    }
    let window_skipped = match display {
        Err(reason) => reason,
        Ok(display) => {
            stages.push(stage(
                "probe window",
                format!(
                    "selected: opens a probe window ({display}); press the shortcut there (keys the desktop consumes never reach it)"
                ),
            ));
            stages.push(stage(
                "terminal",
                "used only if the probe window cannot be opened".into(),
            ));
            return CapturePlan {
                stages,
                selected: Ok(format!("Probe window via {display}")),
            };
        }
    };
    stages.push(stage(
        "probe window",
        format!("unavailable: {window_skipped}"),
    ));
    let selected = if events_all {
        stages.push(stage(
            "terminal",
            "not usable: --events all requires native compositor, probe window, or evdev capture"
                .into(),
        ));
        Err("--events all requires native compositor, probe window, or evdev capture".into())
    } else if tty {
        stages.push(stage(
            "terminal",
            "selected: reads the controlling terminal; shortcuts consumed above it will not appear"
                .into(),
        ));
        Ok(CaptureBackendId::Terminal.display().to_owned())
    } else {
        stages.push(stage(
            "terminal",
            "unavailable: no controlling terminal".into(),
        ));
        Err("no capture backend is usable".into())
    };
    CapturePlan { stages, selected }
}

/// A stage verdict as a state glyph and the words after its keyword.
pub(super) fn stage_state(verdict: &str) -> (crate::style::Check, &str) {
    use crate::style::Check;
    for (keyword, check) in [
        ("selected: ", Check::Ok),
        ("unavailable: ", Check::Fail),
        ("not usable: ", Check::Fail),
        ("not tried: ", Check::Skip),
    ] {
        if let Some(reason) = verdict.strip_prefix(keyword) {
            return (check, reason);
        }
    }
    (Check::Skip, verdict)
}

/// Print the capture chain for `--dry-run`/`--explain-capture`. Nothing is
/// armed; native capture is only probed by connecting to its IPC.
pub(super) fn print_capture_plan(environment: &crate::environment::Environment, options: &Options) {
    let render = crate::style::RenderOptions::cli(options.color, false);
    // JSON mode keeps the plain one-line-per-fact text on stderr.
    let write_line = |line: String| {
        if options.json {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    };
    let armed = if options.dry_run {
        "dry-run; not armed"
    } else {
        "arming now"
    };
    let forced = if options.evdev || options.device.is_some() {
        Some(("Evdev".to_owned(), "--evdev/--device"))
    } else if options.terminal {
        Some(("Terminal".to_owned(), "--terminal"))
    } else if options.window {
        Some((
            match crate::probe::planned_display() {
                Some(display) => format!("Probe window via {display}"),
                None => format!("Probe window ({})", super::NO_PROBE_DISPLAY),
            },
            "--window",
        ))
    } else {
        None
    };
    if let Some((backend, flag)) = forced {
        if options.json {
            write_line(format!("capture chain: skipped; {flag} forces one backend"));
            write_line(format!("capture backend: {backend} ({armed})"));
        } else {
            write_line(format!(
                "{}  skipped; {flag} forces one backend",
                render.heading("Capture chain")
            ));
            write_line(format!(
                "{}  {backend} ({armed})",
                render.heading("Backend")
            ));
        }
        return;
    }
    #[cfg(target_os = "linux")]
    let native = select_native_backend(environment)
        .map(|session| session.display().to_owned())
        .map_err(|error| error.to_string());
    #[cfg(not(target_os = "linux"))]
    let native: Result<String, String> = Err(super::NATIVE_UNSUPPORTED.to_owned());
    let plan = plan_default_chain(
        native.as_deref().map_err(String::as_str),
        super::probe_window_display(environment.ssh),
        environment.tty_available,
        options.capture_policy,
        options.events_all,
    );
    if options.json {
        write_line("capture chain (first usable backend wins):".to_owned());
        for (index, stage) in plan.stages.iter().enumerate() {
            write_line(format!(
                "  {}. {}: {}",
                index + 1,
                stage.name,
                stage.verdict
            ));
        }
        match plan.selected {
            Ok(backend) => write_line(format!(
                "capture backend: {backend} ({armed}; policy={})",
                options.capture_policy
            )),
            Err(reason) => write_line(format!("capture backend: none ({armed}; {reason})")),
        }
        return;
    }
    let name_width = plan
        .stages
        .iter()
        .map(|stage| stage.name.len())
        .max()
        .unwrap_or(0);
    let mut text = format!(
        "{}  {}\n",
        render.heading("Capture chain"),
        render.muted("first usable backend wins")
    );
    for (index, stage) in plan.stages.iter().enumerate() {
        let (check, reason) = stage_state(&stage.verdict);
        text.push_str(&format!(
            "  {}  {}  {:<name_width$}  {reason}\n",
            render.check(check),
            index + 1,
            stage.name,
        ));
    }
    match plan.selected {
        Ok(backend) => text.push_str(&format!(
            "{}  {backend} ({armed}; policy={})\n",
            render.heading("Backend"),
            options.capture_policy
        )),
        Err(reason) => text.push_str(&format!(
            "{}  none ({armed}; {reason})\n",
            render.heading("Backend")
        )),
    }
    print!("{}", render.fold(text));
}

pub(super) fn print_capture_explanation(
    environment: &crate::environment::Environment,
    policy: CapturePolicy,
    color: crate::style::ColorChoice,
    json: bool,
) {
    if json {
        eprintln!("capture detection (policy={policy})");
        for status in &environment.desktops {
            let capture = crate::registry::DESKTOPS
                .iter()
                .find(|entry| entry.id == status.id)
                .is_some_and(|entry| entry.capture.is_some());
            eprintln!(
                "- {}: applicable={} ipc={} capture_factory={}",
                status.id, status.applicable, status.ipc, capture
            );
        }
        eprintln!("selected compositor: {:?}", environment.selected_compositor);
        eprintln!(
            "compositor candidates: {:?}",
            environment.compositor_candidates
        );
        return;
    }
    use crate::style::Check;
    let render = crate::style::RenderOptions::cli(color, false);
    let width = environment
        .desktops
        .iter()
        .map(|status| status.id.len())
        .max()
        .unwrap_or(0);
    let mut text = format!(
        "{}  {}\n",
        render.heading("Capture detection"),
        render.muted(format!("policy={policy}"))
    );
    for status in &environment.desktops {
        let capture = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == status.id)
            .is_some_and(|entry| entry.capture.is_some());
        let (check, reason) = match (status.applicable, status.ipc) {
            (false, _) => (Check::Skip, "not applicable in this session"),
            (true, true) => (Check::Ok, "applicable, IPC reachable"),
            (true, false) => (Check::Fail, "applicable, IPC unreachable"),
        };
        text.push_str(&format!(
            "  {}  {:<width$}  {reason}{}\n",
            render.check(check),
            status.id,
            if capture { "; can capture keys" } else { "" },
        ));
    }
    let selected = environment.selected_compositor.unwrap_or("none");
    let candidates = environment
        .compositor_candidates
        .iter()
        .map(|candidate| {
            format!(
                "{} (score {}, IPC {})",
                candidate.id,
                candidate.score,
                if candidate.ipc {
                    "reachable"
                } else {
                    "unreachable"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    text.push_str(&format!(
        "{}  {selected}\n{}  {}\n",
        render.heading("Selected compositor"),
        render.heading("Candidates"),
        if candidates.is_empty() {
            "none"
        } else {
            &candidates
        }
    ));
    print!("{}", render.fold(text));
}

/// What the waiting message says the capture observes.
pub(super) fn waiting_mode(
    backend: &str,
    policy: CapturePolicy,
    physical_observation: bool,
) -> &'static str {
    if physical_observation {
        "read-only physical observation; normal shortcut actions can still run"
    } else if backend == CaptureBackendId::Window.display() {
        "probe window observation; press the shortcut while the whykey probe window has focus; keys the desktop consumes never reach it"
    } else if backend == CaptureBackendId::Terminal.display() {
        "terminal observation; whykey reads the captured key, so its normal action does not run"
    } else if policy == CapturePolicy::Suppress {
        "temporary suppression; keys pressed while capture is armed are held from applications, and bound compositor actions do not run; universal bindings may still run"
    } else if backend == crate::capture::NativeBackendId::Sway.display() {
        // Sway IPC sends an event only when a binding runs: other keys are
        // invisible to this backend, which would otherwise wait in silence.
        "pass-through observation; Sway reports only keys that run a Sway binding, so other keys (Esc included) produce no report: use --window to see them"
    } else {
        "pass-through observation; normal shortcut actions can still run"
    }
}

/// The keys the waiting message names as the exit. Esc reaches native Sway
/// capture only when it is bound, so Ctrl+C is the exit there.
pub(super) fn exit_keys(backend: &str) -> &'static str {
    if backend == crate::capture::NativeBackendId::Sway.display() {
        "Ctrl+C"
    } else {
        "Esc or Ctrl+C"
    }
}

/// The erasable "waiting" block on stderr: how many lines it takes and
/// whether stderr is a terminal that can erase them.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Waiting {
    lines: usize,
    erasable: bool,
}

impl Waiting {
    /// Remove the waiting block once a key arrived (or the capture ended).
    pub(super) fn clear(self) {
        if self.erasable && self.lines > 0 {
            eprint!("\x1b[{}A\r\x1b[J", self.lines);
            let _ = io::stderr().flush();
        }
    }
}

/// A short label for how the capture treats the key, for the waiting line.
fn waiting_label(backend: &str, policy: CapturePolicy, physical_observation: bool) -> &'static str {
    if physical_observation {
        "observe only"
    } else if backend == CaptureBackendId::Window.display() {
        "probe window"
    } else if backend == CaptureBackendId::Terminal.display() {
        "terminal"
    } else if policy == CapturePolicy::Suppress {
        "suppressing"
    } else {
        "pass-through"
    }
}

fn print_waiting_message(
    backend: &str,
    policy: CapturePolicy,
    repeat: bool,
    json: bool,
    color: crate::style::ColorChoice,
    physical_observation: bool,
) -> Waiting {
    let mode = waiting_mode(backend, policy, physical_observation);
    let exit_keys = exit_keys(backend);
    if json {
        eprintln!(
            "whykey listen: capturing via {backend} ({mode}); Waiting for input. {exit_keys} exits"
        );
        return Waiting::default();
    }
    let options = crate::style::RenderOptions::cli(color, false);
    let label = waiting_label(backend, policy, physical_observation);
    let repeat = if repeat { ", repeat" } else { "" };
    let mut lines = vec![format!(
        "Waiting for a key\u{2026} ({backend} capture, {label}{repeat}; {exit_keys} cancels)"
    )];
    // Caveats the short label cannot carry: they decide what will be seen.
    if matches!(label, "probe window" | "suppressing")
        || backend == crate::capture::NativeBackendId::Sway.display()
    {
        lines.push(format!("{mode}."));
    }
    let erasable = io::stderr().is_terminal();
    // Wrap before printing so the number of rows to erase is known.
    let mut count = 0;
    for (index, line) in lines.into_iter().enumerate() {
        let line = options.fold(line);
        for row in crate::style::wrap_hanging(&line, options.width, "", "  ") {
            if index == 0 {
                eprintln!("{}", options.heading(row));
            } else {
                eprintln!("{}", options.muted(row));
            }
            count += 1;
        }
    }
    Waiting {
        lines: count,
        erasable,
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(super) struct SelectError {
    attempted: Vec<(String, String)>,
}

#[cfg(target_os = "linux")]
impl fmt::Display for SelectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.attempted.is_empty() {
            let backends = crate::registry::DESKTOPS
                .iter()
                .filter(|entry| entry.capture.is_some())
                .map(|entry| entry.display)
                .collect::<Vec<_>>()
                .join(" or ");
            return write!(formatter, "no {backends} session detected");
        }
        let details = self
            .attempted
            .iter()
            .map(|(backend, reason)| format!("{backend}: {reason}"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(formatter, "tried {details}")
    }
}

#[cfg(target_os = "linux")]
impl SelectError {
    pub(super) fn backend_display(&self) -> &str {
        self.attempted
            .first()
            .map_or("native compositor", |(backend, _)| backend.as_str())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn select_native_backend(
    environment: &crate::environment::Environment,
) -> Result<Box<dyn NativeCaptureIo>, SelectError> {
    let mut attempted = Vec::new();
    for candidate in &environment.compositor_candidates {
        let Some(descriptor) = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == candidate.id)
        else {
            continue;
        };
        let Some(factory) = descriptor.capture else {
            continue;
        };
        match factory() {
            Ok(backend) => return Ok(backend),
            Err(error) => attempted.push((descriptor.display.to_owned(), error)),
        }
    }
    Err(SelectError { attempted })
}

#[cfg(target_os = "linux")]
pub(super) fn run_native(
    options: Options,
    mut capture_session: Box<dyn NativeCaptureIo>,
    snapshot: ListenSession,
    signals: &SignalGuard,
) -> Result<(), ListenError> {
    let mut export = open_export(options.output.as_deref())?;
    if let Some(warning) = capture_session.pre_arm_warning(options.capture_policy) {
        eprintln!("{warning}");
    }
    if options.explicit_suppress {
        eprintln!(
            "warning: --suppress is opt-in and may temporarily change compositor state; see `whykey doctor`"
        );
    }

    let mut capture_stdout = io::BufWriter::new(io::stdout());
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut captured_events = 0_usize;

    // Keep one terminal raw-mode guard for the complete native listening session.
    let mut terminal = TerminalSession::open()?;
    let original_termios = terminal.original;
    let mut tty_fd = terminal.tty.as_raw_fd();
    let socket_fd = capture_session
        .transport()
        .poll_fd()
        .ok_or_else(|| ListenError::Setup("native capture transport is not pollable".into()))?;
    let _ = flush_input(tty_fd);

    if let Err(err) = capture_session.arm_token(options.capture_policy) {
        let _ = flush_input(tty_fd);
        if options.capture_policy == CapturePolicy::Suppress {
            eprintln!(
                "whykey listen: could not suppress native compositor shortcuts ({})",
                capture_session.display()
            );
            eprintln!("No key was captured and no shortcut was executed.");
            eprintln!("Use --no-suppress to capture without suppression.");
            return Err(ListenError::Setup(format!(
                "could not suppress {} shortcuts: {err}",
                capture_session.display()
            )));
        } else {
            return Err(ListenError::Setup(format!(
                "{} capture failed to arm: {err}",
                capture_session.display()
            )));
        }
    }

    let waiting = print_waiting_message(
        capture_session.display(),
        options.capture_policy,
        options.repeat,
        options.json,
        options.color,
        false,
    );
    let _ = io::stdout().flush();

    let mut last_renewed = Instant::now();
    loop {
        let observed = loop {
            if signals.received() {
                let _ = flush_input(tty_fd);
                capture_session.close().map_err(ListenError::Io)?;
                return Err(ListenError::Message(
                    "interrupted; terminal settings restored".into(),
                ));
            }
            if deadline.is_some_and(|value| Instant::now() >= value) {
                let _ = flush_input(tty_fd);
                capture_session.close().map_err(ListenError::Io)?;
                return Err(ListenError::Message("capture timed out".into()));
            }
            if signals.take_stop_request() {
                // Job control (Ctrl+Z / SIGTSTP): remove the leased hook and
                // leave raw/kitty mode before stopping, then re-enter both.
                let rearm = capture_session.holds_lease();
                if rearm {
                    capture_session.close().map_err(ListenError::Io)?;
                }
                let _ = flush_input(tty_fd);
                drop(terminal);
                signals.stop_process()?;
                terminal = TerminalSession::open()?;
                tty_fd = terminal.tty.as_raw_fd();
                if rearm {
                    capture_session
                        .arm_token(options.capture_policy)
                        .map_err(ListenError::Setup)?;
                    last_renewed = Instant::now();
                }
                continue;
            }

            if capture_session.holds_lease()
                && last_renewed.elapsed() >= crate::capture::CAPTURE_LEASE_RENEW_INTERVAL
            {
                capture_session.transport().renew_lease().map_err(|e| {
                    ListenError::Message(format!("failed to renew capture lease: {e}"))
                })?;
                last_renewed = Instant::now();
            }
            if let Some(observed) = capture_session
                .next_event(options.events_all)
                .map_err(ListenError::Message)?
            {
                waiting.clear();
                if observed.event_type == KeyEventType::Press && is_cancel_key(&observed) {
                    if options.json {
                        eprintln!("Stopped.");
                    } else {
                        println!("\nStopped.");
                    }
                    return Ok(());
                }
                break observed;
            }

            let timeout_ms = deadline
                .map(remaining_millis)
                .map_or(100, |remaining| remaining.min(100));
            let mut pollfds = [
                libc::pollfd {
                    fd: socket_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: tty_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let result = unsafe { libc::poll(pollfds.as_mut_ptr(), 2, timeout_ms) };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                let _ = flush_input(tty_fd);
                return Err(ListenError::Io(error));
            }
            if pollfds[1].revents & libc::POLLIN != 0 {
                // Drain forwarded terminal bytes continuously
                let mut scratch = [0u8; 1024];
                let _ = terminal.tty.read(&mut scratch);
            }
            if pollfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                let _ = flush_input(tty_fd);
                capture_session.close().map_err(ListenError::Io)?;
                return Err(ListenError::Message(format!(
                    "{} socket disconnected while listening",
                    capture_session.display()
                )));
            }
            if pollfds[0].revents & libc::POLLIN != 0 {
                if let Err(err) = capture_session.transport().read_incoming() {
                    let _ = flush_input(tty_fd);
                    capture_session.close().map_err(ListenError::Io)?;
                    return Err(ListenError::Io(err));
                }
            }
        };

        let leased = capture_session.holds_lease();
        if capture_session.is_suppressing() {
            let Some(main_code) = observed.physical_keycode.and_then(|code| code.to_xkb()) else {
                let _ = flush_input(tty_fd);
                let _ = capture_session.close();
                return Err(ListenError::Message(format!(
                    "captured {} event has no convertible physical keycode",
                    capture_session.display()
                )));
            };
            match capture_session.wait_for_chord_release(main_code, Duration::from_millis(1000)) {
                Ok(crate::hyprland_capture::ChordReleaseStatus::Released) => {}
                Ok(crate::hyprland_capture::ChordReleaseStatus::TimedOut) => {
                    let _ = flush_input(tty_fd);
                    capture_session.close().map_err(ListenError::Io)?;
                    return Err(ListenError::Message(
                        "timed out waiting for confirmed captured chord release; no report was produced"
                            .into(),
                    ));
                }
                Err(error) => {
                    let _ = flush_input(tty_fd);
                    capture_session.close().map_err(ListenError::Io)?;
                    return Err(ListenError::Message(error));
                }
            }
        }
        if leased {
            // Remove the hook before running desktop discovery and rendering.
            // This prevents inspection output, signal handling, or queued key
            // events from running while the capture submap is still active,
            // and keeps a slow report from outliving the hook's lease.
            if let Err(error) = capture_session.close() {
                return Err(ListenError::Message(format!(
                    "could not restore {} state before reporting: {error}",
                    capture_session.display()
                )));
            }
        }

        let results = snapshot.inspect(&observed, Some(&original_termios));
        let rendered = if options.json {
            if options.ndjson {
                report::render_ndjson(&observed.combo, &results, Some(&observed))
            } else {
                report::render_listen_json(
                    &observed.combo,
                    &results,
                    Some(&observed),
                    options.schema_version,
                )
            }
        } else {
            report::render_observed_with_options(
                &observed,
                &results,
                crate::style::RenderOptions::cli(options.color, options.verbose),
            )
        };
        write_capture(&mut export, &mut capture_stdout, &rendered)?;

        // Flush any lingering bytes in TTY input queue before waiting for the next key
        let _ = flush_input(tty_fd);

        captured_events += 1;
        let count_reached = options.count.is_some_and(|count| captured_events >= count);
        let keep_listening = options.repeat || options.count.is_some_and(|count| count > 1);
        if !keep_listening || count_reached {
            break;
        }
        if leased {
            capture_session
                .arm_token(options.capture_policy)
                .map_err(ListenError::Message)?;
            last_renewed = Instant::now();
        }
        if !options.json {
            println!(
                "\n{}\n",
                crate::style::RenderOptions::cli(options.color, false)
                    .muted("--- next capture ---")
            );
        }
    }

    let _ = flush_input(tty_fd);
    capture_session.close().map_err(ListenError::Io)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn run_native(_options: Options) -> Result<(), ListenError> {
    Err(ListenError::Setup(
        "native compositor capture is only available on Linux".into(),
    ))
}

pub(super) fn run_terminal(options: Options) -> Result<(), ListenError> {
    // Install first: the guard must outlive the raw-mode session it protects.
    let signals = SignalGuard::install()?;
    let snapshot = ListenSession::capture();
    let mut backend = TerminalBackend::new();
    backend
        .arm(CapturePolicy::PassThrough)
        .map_err(ListenError::Setup)?;
    let original_termios = backend
        .original_termios()
        .ok_or_else(|| ListenError::Setup("terminal capture failed to arm".into()))?;
    run_observed_loop(options, backend, snapshot, Some(original_termios), &signals)
}

/// Answer a SIGTSTP request: release the capture (restoring the terminal),
/// stop the process, and re-enter capture once the job is continued.
pub(super) fn suspend_for_job_control<B: CaptureBackend + ?Sized>(
    backend: &mut B,
    stop: impl FnOnce() -> io::Result<()>,
) -> Result<(), ListenError> {
    backend.close()?;
    stop()?;
    backend
        .arm(CapturePolicy::PassThrough)
        .map_err(ListenError::Setup)
}

pub(super) fn run_observed_loop<B: CaptureBackend>(
    options: Options,
    mut backend: B,
    snapshot: ListenSession,
    terminal_termios: Option<libc::termios>,
    signals: &SignalGuard,
) -> Result<(), ListenError> {
    let mut export = open_export(options.output.as_deref())?;
    let mut capture_stdout = io::BufWriter::new(io::stdout());
    let mut waiting = print_waiting_message(
        backend.display(),
        CapturePolicy::PassThrough,
        options.repeat,
        options.json,
        options.color,
        backend.id() == CaptureBackendId::Evdev,
    );
    let _ = io::stdout().flush();
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut captured_events = 0_usize;
    loop {
        if signals.received() {
            let _ = backend.close();
            // Only a loop that changed termios has terminal settings to restore.
            let message = if terminal_termios.is_some() {
                "interrupted; terminal settings restored"
            } else {
                "interrupted"
            };
            return Err(ListenError::Message(message.into()));
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            let _ = backend.close();
            return Err(ListenError::Message("capture timed out".into()));
        }
        if signals.take_stop_request() {
            suspend_for_job_control(&mut backend, || signals.stop_process())?;
            continue;
        }
        let Some(observed) = backend
            .next_event(options.events_all)
            .map_err(ListenError::Message)?
        else {
            continue;
        };
        waiting.clear();
        if observed.event_type == KeyEventType::Press && is_cancel_key(&observed) {
            backend.close().map_err(ListenError::Io)?;
            if options.json {
                eprintln!("Stopped.");
            } else {
                println!("\nStopped.");
            }
            return Ok(());
        }
        backend.close().map_err(ListenError::Io)?;
        // A key that never arrived has nothing to route through the layers.
        let results = if observed.disposition == CaptureDisposition::NotDelivered {
            Vec::new()
        } else {
            snapshot.inspect(&observed, terminal_termios.as_ref())
        };
        let rendered = if options.json {
            if options.ndjson {
                report::render_ndjson(&observed.combo, &results, Some(&observed))
            } else {
                report::render_listen_json(
                    &observed.combo,
                    &results,
                    Some(&observed),
                    options.schema_version,
                )
            }
        } else {
            report::render_observed_with_options(
                &observed,
                &results,
                crate::style::RenderOptions::cli(options.color, options.verbose),
            )
        };
        write_capture(&mut export, &mut capture_stdout, &rendered)?;
        captured_events += 1;
        let count_reached = options.count.is_some_and(|count| captured_events >= count);
        let keep_listening = options.repeat || options.count.is_some_and(|count| count > 1);
        if !keep_listening || count_reached {
            return Ok(());
        }
        backend
            .arm(CapturePolicy::PassThrough)
            .map_err(ListenError::Setup)?;
        if !options.json {
            println!(
                "\n{}\n",
                crate::style::RenderOptions::cli(options.color, false)
                    .muted("--- next capture ---")
            );
        }
        waiting = print_waiting_message(
            backend.display(),
            CapturePolicy::PassThrough,
            options.repeat,
            options.json,
            options.color,
            backend.id() == CaptureBackendId::Evdev,
        );
    }
}

pub(super) fn is_cancel_key(observed: &ObservedKey) -> bool {
    (observed.combo.key() == "ESCAPE" && observed.combo.modmask() == 0)
        || (observed.combo.key() == "C" && observed.combo.modmask() & 4 != 0)
}

pub(super) fn is_terminal_modifier_key(observed: &ObservedKey) -> bool {
    matches!(
        observed.combo.key(),
        "LEFT_SHIFT"
            | "RIGHT_SHIFT"
            | "LEFT_CONTROL"
            | "RIGHT_CONTROL"
            | "LEFT_ALT"
            | "RIGHT_ALT"
            | "LEFT_SUPER"
            | "RIGHT_SUPER"
            | "LEFT_HYPER"
            | "RIGHT_HYPER"
            | "LEFT_META"
            | "RIGHT_META"
            | "ISO_LEVEL3_SHIFT"
            | "ISO_LEVEL5_SHIFT"
    )
}

pub(super) fn inspect_with_session(
    observed: &ObservedKey,
    terminal_termios: Option<&libc::termios>,
    session: &ListenSession,
) -> Vec<LayerResult> {
    let mut results = match (&observed.source, observed.physical_keycode) {
        (CaptureSource::Evdev { device, .. }, Some(keycode)) => {
            crate::layers::inspect_default_chain_evdev_with_session(
                &observed.combo,
                device.clone(),
                keycode,
                &session.environment,
            )
        }
        (source, Some(keycode)) if source.proves_compositor_receipt() => {
            crate::layers::inspect_default_chain_hyprland_with_session(
                &observed.combo,
                keycode,
                &session.environment,
            )
        }
        _ if observed.source.confirms_terminal() => {
            crate::layers::inspect_default_chain_observed_with_session(
                &observed.combo,
                &observed.raw,
                observed.protocol_flags,
                terminal_termios,
                &session.environment,
            )
        }
        _ => {
            // An evdev event proves only that the physical keyboard generated it;
            // it does not prove compositor or terminal forwarding. Use the normal
            // static chain without upgrading earlier layers to observed success.
            crate::layers::inspect_default_chain(&observed.combo)
        }
    };

    if observed.disposition == CaptureDisposition::Suppressed {
        note_suppressed_hold(&mut results);
    }

    if observed.source.proves_compositor_delivery() {
        confirm_window_delivery(&mut results);
        return results;
    }

    // Capture proves that the event reached the terminal. It does not prove
    // that the normal TTY, multiplexer, editor, or shell path would forward
    // it: capture temporarily bypasses some of those consumers.
    if !observed.source.confirms_terminal() {
        return results;
    }
    for result in &mut results {
        match result.id {
            LayerId::Compositor | LayerId::Terminal => {
                if matches!(result.outcome, Outcome::Consumed | Outcome::Redirected) {
                    // Capture proves the event reached the terminal, so a
                    // stopping or redirecting layer actually forwarded it.
                    result.outcome = Outcome::HandledAndPassed;
                    result
                        .summary
                        .push_str("; observed event reached the terminal");
                } else if matches!(
                    result.outcome,
                    Outcome::Unknown | Outcome::Unavailable | Outcome::HandledUncertain
                ) {
                    result
                        .summary
                        .push_str("; live capture reached the terminal");
                }
            }
            LayerId::Tty if result.outcome == Outcome::Consumed => {
                result
                    .summary
                    .push_str("; capture bypassed this normal TTY consumer");
                result.details.push(
                    "the probe reads the byte with ISIG/line processing temporarily disabled"
                        .into(),
                );
            }
            _ => {}
        }
    }

    if let Some(index) = results
        .iter()
        .position(|result| matches!(result.outcome, Outcome::Consumed | Outcome::Redirected))
    {
        results.truncate(index + 1);
    }

    results
}

/// `--suppress` holds every key pressed while capture is armed, not only the
/// bound ones: the hook's catch-all consumes whatever is pressed (the lab's
/// `hyprland-suppress-unbound` scenario). That hold is the capture's own
/// doing, so a Hyprland layer with no binding keeps its normal-route verdict
/// (the key continues once whykey exits) and only gains a detail saying the
/// application did not receive it this time.
pub(super) fn note_suppressed_hold(results: &mut [LayerResult]) {
    if let Some(result) = results.iter_mut().find(|result| {
        result.id == LayerId::Compositor
            && result.layer == "Hyprland"
            && result.outcome == Outcome::Pass
    }) {
        result.details.push(
            "--suppress held this key while capture was armed (Hyprland catch-all), so the application did not receive it this time; without whykey it continues as shown".into(),
        );
    }
}

/// The key reached whykey's focused probe window, so every layer that sits
/// above a focused client forwarded it, whatever its configuration says.
/// Layers below the focused client (terminal, shell, editor) describe the
/// usual focused application, not the probe, and keep their static result.
pub(super) fn confirm_window_delivery(results: &mut [LayerResult]) {
    for result in results {
        if !matches!(result.id, LayerId::Remapper | LayerId::Compositor) {
            continue;
        }
        if matches!(result.outcome, Outcome::Consumed | Outcome::Redirected) {
            result.outcome = Outcome::HandledAndPassed;
            result
                .summary
                .push_str("; observed key reached the probe window");
        } else if matches!(
            result.outcome,
            Outcome::Unknown | Outcome::Unavailable | Outcome::HandledUncertain
        ) {
            result
                .summary
                .push_str("; live capture reached the probe window");
        }
    }
}

fn open_export(path: Option<&Path>) -> Result<Option<File>, ListenError> {
    path.map(File::create).transpose().map_err(ListenError::Io)
}

fn write_capture<W: Write>(
    export: &mut Option<File>,
    stdout: &mut W,
    rendered: &str,
) -> Result<(), ListenError> {
    if let Some(file) = export {
        file.write_all(rendered.as_bytes())?;
        file.flush()?;
    } else {
        stdout.write_all(rendered.as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}
