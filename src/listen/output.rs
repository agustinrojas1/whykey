fn print_capture_dry_run(environment: &crate::environment::Environment, options: &Options) {
    let forced = if options.evdev || options.device.is_some() {
        Some("Evdev")
    } else if options.terminal {
        Some("Terminal")
    } else {
        None
    };
    if let Some(backend) = forced {
        println!("capture backend: {backend} (dry-run; not armed)");
        return;
    }
    match select_native_backend(environment) {
        Ok(session) => println!(
            "capture backend: {} (dry-run; not armed; policy={})",
            session.display(),
            options.capture_policy
        ),
        Err(error) => {
            println!("capture backend: terminal fallback (dry-run; native unavailable: {error})")
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn print_capture_dry_run(_environment: &crate::environment::Environment, options: &Options) {
    let backend = if options.evdev || options.device.is_some() {
        "Evdev"
    } else {
        "Terminal"
    };
    println!("capture backend: {backend} (dry-run; not armed)");
}

fn print_capture_explanation(environment: &crate::environment::Environment, policy: CapturePolicy) {
    println!("capture detection (policy={policy})");
    for status in &environment.desktops {
        let capture = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == status.id)
            .is_some_and(|entry| entry.capture.is_some());
        println!(
            "- {}: applicable={} ipc={} capture_factory={}",
            status.id, status.applicable, status.ipc, capture
        );
    }
    println!("selected compositor: {:?}", environment.selected_compositor);
    println!(
        "compositor candidates: {:?}",
        environment.compositor_candidates
    );
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct SelectError {
    attempted: Vec<(String, String)>,
}

#[cfg(target_os = "linux")]
impl fmt::Display for SelectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.attempted.is_empty() {
            return formatter.write_str("no registered native compositor backend");
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
    fn backend_display(&self) -> &str {
        self.attempted
            .first()
            .map_or("native compositor", |(backend, _)| backend.as_str())
    }
}

#[cfg(target_os = "linux")]
fn select_native_backend(
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
fn run_native(
    options: Options,
    mut capture_session: Box<dyn NativeCaptureIo>,
    snapshot: ListenSession,
) -> Result<(), ListenError> {
    let signals = SignalGuard::install()?;
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
    let tty_fd = terminal.tty.as_raw_fd();
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
            eprintln!("Use --pass-through to capture without suppression.");
            return Err(ListenError::Setup(format!(
                "could not suppress {} shortcuts",
                capture_session.display()
            )));
        } else {
            return Err(ListenError::Setup(format!(
                "{} capture failed to arm: {err}",
                capture_session.display()
            )));
        }
    }

    if options.json {
        eprintln!("whykey listen: press a key combination (Esc or Ctrl+C exits)");
    } else {
        println!("whykey listen");
        if options.capture_policy == CapturePolicy::Suppress {
            println!(
                "{} shortcuts are temporarily suppressed.",
                capture_session.display()
            );
        }
        println!("Press a key combination. Press Esc or Ctrl+C to exit.");
        if options.repeat {
            println!("Repeat mode is on.");
        }
        println!();
    }

    if options.json {
        eprintln!("Waiting for input...");
    } else {
        println!("Waiting for input...");
    }
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

            if capture_session.is_suppressing() && last_renewed.elapsed() >= Duration::from_secs(2)
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
            // Remove the hook before running desktop discovery and rendering.
            // This prevents inspection output, signal handling, or queued key
            // events from running while the capture submap is still active.
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
            report::render_observed(&observed, &results, options.verbose)
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
        if options.capture_policy == CapturePolicy::Suppress {
            capture_session
                .arm_token(CapturePolicy::Suppress)
                .map_err(ListenError::Message)?;
        }
        if !options.json {
            println!();
        }
    }

    let _ = flush_input(tty_fd);
    capture_session.close().map_err(ListenError::Io)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_native(_options: Options) -> Result<(), ListenError> {
    Err(ListenError::Setup(
        "native compositor capture is only available on Linux".into(),
    ))
}

fn run_terminal(options: Options) -> Result<(), ListenError> {
    let snapshot = ListenSession::capture();
    let mut backend = TerminalBackend::new();
    backend
        .arm(CapturePolicy::PassThrough)
        .map_err(ListenError::Setup)?;
    let original_termios = backend
        .original_termios()
        .ok_or_else(|| ListenError::Setup("terminal capture failed to arm".into()))?;
    run_observed_loop(options, backend, snapshot, Some(original_termios))
}

fn run_observed_loop<B: CaptureBackend>(
    options: Options,
    mut backend: B,
    snapshot: ListenSession,
    terminal_termios: Option<libc::termios>,
) -> Result<(), ListenError> {
    let signals = SignalGuard::install()?;
    let mut export = open_export(options.output.as_deref())?;
    let mut capture_stdout = io::BufWriter::new(io::stdout());
    if options.json {
        eprintln!(
            "whykey listen{}: press a key combination (Esc or Ctrl+C exits)",
            if backend.id() == CaptureBackendId::Evdev {
                " --evdev"
            } else {
                ""
            }
        );
    } else {
        println!(
            "whykey listen{}",
            if backend.id() == CaptureBackendId::Evdev {
                " --evdev"
            } else {
                ""
            }
        );
        println!("Press a key combination. Press Esc or Ctrl+C to exit.");
        if backend.id() == CaptureBackendId::Evdev {
            println!("Read-only capture; the input device is not grabbed.");
        }
        if options.repeat {
            println!("Repeat mode is on.");
        }
        println!();
    }
    if options.json {
        eprintln!("Waiting for input...");
    } else {
        println!("Waiting for input...");
    }
    let _ = io::stdout().flush();
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut captured_events = 0_usize;
    loop {
        if signals.received() {
            let _ = backend.close();
            return Err(ListenError::Message(
                "interrupted; terminal settings restored".into(),
            ));
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            let _ = backend.close();
            return Err(ListenError::Message("capture timed out".into()));
        }
        let Some(observed) = backend
            .next_event(options.events_all)
            .map_err(ListenError::Message)?
        else {
            continue;
        };
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
        let results = snapshot.inspect(&observed, terminal_termios.as_ref());
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
            report::render_observed(&observed, &results, options.verbose)
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
            println!();
        }
        if options.json {
            eprintln!("Waiting for input...");
        } else {
            println!("Waiting for input...");
        }
    }
}

fn is_cancel_key(observed: &ObservedKey) -> bool {
    (observed.combo.key() == "ESCAPE" && observed.combo.modmask() == 0)
        || (observed.combo.key() == "C" && observed.combo.modmask() & 4 != 0)
}

fn is_terminal_modifier_key(observed: &ObservedKey) -> bool {
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

fn inspect_with_session(
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
