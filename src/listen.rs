//! Capture one key event from Hyprland compositor, Linux evdev, or the
//! controlling terminal, and explain it.
//!
//! When running inside a compatible Hyprland session, the default listener
//! uses a temporary runtime Lua hook and capture submap to suppress bound
//! compositor actions during inspection (or pass them through when configured).
//! If Hyprland capture is unavailable, it falls back to terminal capture
//! (`--terminal` explicitly forces terminal capture). The optional evdev backend
//! observes physical input events before compositor processing.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};
use crate::report;

const SEQUENCE_TIMEOUT_MS: i32 = 12;
/// Briefly wait for a legacy Escape sequence to continue. Kitty mode encodes
/// Escape unambiguously, so normal interactive cancellation is immediate.
const LEGACY_ESCAPE_SEQUENCE_TIMEOUT_MS: i32 = 30;
const SIGNAL_POLL_MS: i32 = 100;
const WAITING_HINT_AFTER: Duration = Duration::from_secs(5);
const MAX_TERMINAL_SEQUENCE_BYTES: usize = 4096;
/// Ask for every currently specified Kitty keyboard enhancement. The original
/// flags are kept separately in `TerminalSession::protocol_flags` so the
/// downstream byte prediction still describes the terminal before capture.
const KITTY_CAPTURE_FLAGS: u32 = 1 | 2 | 4 | 8 | 16;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
pub use crate::hyprland_capture::HyprlandCapturePolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDisposition {
    Suppressed,
    PassedThrough,
    ObservedOnly,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub capture_policy: HyprlandCapturePolicy,
    pub repeat: bool,
    pub json: bool,
    /// Emit one compact schema-v2 JSON record per captured event. This is a
    /// stream-only format, so it is accepted by `whykey listen` rather than
    /// static inspection commands.
    pub ndjson: bool,
    pub terminal: bool,
    pub evdev: bool,
    pub device: Option<PathBuf>,
    /// Stop waiting after this amount of wall-clock time. The deadline is
    /// shared by all reports in a repeated capture.
    pub timeout: Option<Duration>,
    /// Maximum number of reports. `None` keeps the old one-shot/repeat
    /// behaviour, while `Some(n)` always stops after n captured events.
    pub count: Option<usize>,
    /// Include modifier-only and release events in physical capture.
    pub events_all: bool,
    /// Write captured JSON reports to a file instead of stdout. This is
    /// intentionally explicit so a capture can be replayed without shell
    /// redirection or accidental mixing with interface diagnostics.
    pub output: Option<PathBuf>,
    /// Show every route layer instead of only matching, consuming,
    /// unavailable, or uncertain layers.
    pub verbose: bool,
    /// Structured output version for JSON renders, from `--schema-version`.
    pub schema_version: u8,
}
/// One normal-mode environment snapshot for a listener session. Capture mode
/// is deliberately limited to reading one key; inspection and rendering run
/// after the terminal has been restored.
struct ListenSession {
    environment: crate::environment::Environment,
}

impl ListenSession {
    fn capture() -> Self {
        Self {
            environment: crate::environment::Environment::collect(),
        }
    }

    /// Inspect one event after terminal capture has finished.
    fn inspect(
        &self,
        observed: &ObservedKey,
        terminal_termios: Option<&libc::termios>,
    ) -> Vec<LayerResult> {
        inspect_with_session(observed, terminal_termios, self)
    }
}

/// Whether at least one Linux input event device can be opened read-only.
/// This is a capability check for `doctor`; it never grabs a device.
#[cfg(target_os = "linux")]
pub fn evdev_available() -> bool {
    EvdevSession::open(None).is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn evdev_available() -> bool {
    false
}

#[derive(Debug, Clone, Serialize)]
pub struct EvdevDeviceInfo {
    pub path: String,
    pub name: String,
    pub keyboard: Option<bool>,
    pub readable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Describe candidate keyboard event devices without grabbing or changing
/// any input device. This is used by `doctor` to make permission failures
/// actionable and to show the path accepted by `listen --device`.
#[cfg(target_os = "linux")]
pub fn evdev_devices() -> Vec<EvdevDeviceInfo> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .take(64)
        .filter_map(|path| {
            let keyboard = evdev_keyboard_capability(&path);
            if keyboard == Some(false) {
                return None;
            }
            let path_display = path.display().to_string();
            let name = evdev_device_name(&path);
            match File::open(&path) {
                Ok(_) => Some(EvdevDeviceInfo {
                    path: path_display,
                    name,
                    keyboard,
                    readable: true,
                    error: None,
                }),
                Err(error) => Some(EvdevDeviceInfo {
                    path: path_display,
                    name,
                    keyboard,
                    readable: false,
                    error: Some(error.to_string()),
                }),
            }
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub fn evdev_devices() -> Vec<EvdevDeviceInfo> {
    Vec::new()
}

#[derive(Debug)]
pub enum ListenError {
    Io(io::Error),
    Message(String),
    Setup(String),
}

impl fmt::Display for ListenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Message(message) | Self::Setup(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ListenError {}

impl From<io::Error> for ListenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReadEvent {
    Key(Box<ObservedKey>),
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservedKey {
    pub disposition: CaptureDisposition,
    pub combo: KeyCombo,
    pub raw: Vec<u8>,
    #[serde(default)]
    pub raw_display: Option<String>,
    /// Modifier state recovered from evdev at the observation point. The
    /// aggregate mask in `combo` remains for compatibility; this field keeps
    /// left/right pressed keys and lock state distinguishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_state: Option<ModifierState>,
    /// Text code points associated with a Kitty key event, when the terminal
    /// negotiated the associated-text enhancement. This is intentionally
    /// separate from `combo`: a layout/IME can produce text that has no
    /// recoverable physical key (Kitty uses key code zero for that case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associated_text: Option<String>,
    /// Linux evdev key code when the event was captured before the compositor.
    #[serde(default)]
    pub physical_keycode: Option<u16>,
    pub encoding: String,
    pub protocol_flags: Option<u32>,
    pub event_type: KeyEventType,
    /// All alternate key codes reported by Kitty, in wire order. The
    /// singular `alternate_key` field remains for schema compatibility and
    /// contains the first entry when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_keys: Option<Vec<String>>,
    pub alternate_key: Option<String>,
    pub source: CaptureSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModifierState {
    /// Physical modifier keys currently held across all observed devices.
    pub pressed: Vec<String>,
    /// Lock modifiers currently reported by the device LEDs.
    pub locked: Vec<String>,
    /// Evdev does not expose XKB-style latched state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latched: Option<Vec<String>>,
    /// Per-device state retained so two keyboards cannot be mistaken for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<DeviceModifierState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceModifierState {
    pub device: String,
    pub path: String,
    pub pressed: Vec<String>,
    pub locked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum CaptureSource {
    Terminal,
    Hyprland,
    Evdev { device: String, path: String },
}

impl CaptureSource {
    pub fn label(&self) -> String {
        match self {
            Self::Terminal => "terminal".into(),
            Self::Hyprland => "Hyprland".into(),
            Self::Evdev { device, path } => format!("evdev ({device}; {path})"),
        }
    }

    pub fn confirms_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

impl KeyEventType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Repeat => "repeat",
            Self::Release => "release",
        }
    }
}
pub fn run(options: Options) -> Result<(), ListenError> {
    if options.evdev || options.device.is_some() {
        return run_evdev(options);
    }
    if options.terminal {
        return run_terminal(options);
    }
    #[cfg(target_os = "linux")]
    match crate::hyprland_capture::HyprlandCaptureSession::connect() {
        Ok(session) => run_hyprland(options, session),
        Err(error) => {
            if options.events_all {
                return Err(ListenError::Message(format!(
                    "Hyprland capture unavailable: {error}; --events all requires Hyprland or evdev capture"
                )));
            }
            if options.capture_policy == HyprlandCapturePolicy::Suppress
                && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
            {
                eprintln!("whykey listen: could not suppress Hyprland shortcuts");
                eprintln!("No key was captured and no shortcut was executed.");
                eprintln!("Use --pass-through to capture without suppression.");
                return Err(ListenError::Setup(format!(
                    "Hyprland capture unavailable: {error}"
                )));
            }
            eprintln!("Global Hyprland capture is unavailable: {error}");
            eprintln!(
                "Using terminal capture; shortcuts consumed by the compositor will not appear."
            );
            run_terminal(options)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if options.events_all {
            return Err(ListenError::Message(
                "--events all requires evdev capture on non-Linux platforms".into(),
            ));
        }
        eprintln!("Global Hyprland capture is unavailable: only supported on Linux");
        eprintln!("Using terminal capture; shortcuts consumed by the compositor will not appear.");
        run_terminal(options)
    }
}

#[cfg(target_os = "linux")]
fn run_hyprland(
    options: Options,
    mut capture_session: crate::hyprland_capture::HyprlandCaptureSession,
) -> Result<(), ListenError> {
    let signals = SignalGuard::install()?;
    let mut export = open_export(options.output.as_deref())?;
    let snapshot = ListenSession::capture();

    if options.capture_policy == HyprlandCapturePolicy::Suppress
        && crate::hyprland_capture::has_universal_bindings()
    {
        eprintln!("warning: universal Hyprland bindings remain active during capture");
    }

    let mut capture_stdout = io::BufWriter::new(io::stdout());
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut captured_events = 0_usize;

    // Keep one terminal raw-mode guard for the complete Hyprland listening session
    let mut terminal = TerminalSession::open()?;
    let original_termios = terminal.original;
    let tty_fd = terminal.tty.as_raw_fd();
    let socket_fd = capture_session.socket_fd();
    let _ = flush_input(tty_fd);

    if let Err(err) = capture_session.arm(options.capture_policy) {
        let _ = flush_input(tty_fd);
        if options.capture_policy == HyprlandCapturePolicy::Suppress {
            eprintln!("whykey listen: could not suppress Hyprland shortcuts");
            eprintln!("No key was captured and no shortcut was executed.");
            eprintln!("Use --pass-through to capture without suppression.");
            return Err(ListenError::Setup(
                "could not suppress Hyprland shortcuts".into(),
            ));
        } else {
            return Err(ListenError::Setup(format!(
                "Hyprland capture failed to arm: {err}"
            )));
        }
    }

    if options.json {
        eprintln!("whykey listen: press a key combination (Esc or Ctrl+C exits)");
    } else {
        println!("whykey listen");
        if options.capture_policy == HyprlandCapturePolicy::Suppress {
            println!("Hyprland shortcuts are temporarily suppressed.");
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
                capture_session.renew_lease().map_err(|e| {
                    ListenError::Message(format!("failed to renew capture lease: {e}"))
                })?;
                last_renewed = Instant::now();
            }
            if let Some(observed) = capture_session
                .next_observed_event(options.events_all)
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
                return Err(ListenError::Message(
                    "Hyprland socket disconnected while listening".into(),
                ));
            }
            if pollfds[0].revents & libc::POLLIN != 0 {
                if let Err(err) = capture_session.read_incoming() {
                    let _ = flush_input(tty_fd);
                    capture_session.close().map_err(ListenError::Io)?;
                    return Err(ListenError::Io(err));
                }
            }
        };

        if capture_session.is_suppressing() {
            let main_code = observed
                .physical_keycode
                .map_or(0, |code| (code + 8) as u32);
            if let Err(err) =
                capture_session.wait_for_chord_release(main_code, Duration::from_millis(1000))
            {
                let _ = flush_input(tty_fd);
                capture_session.close().map_err(ListenError::Io)?;
                return Err(ListenError::Message(err));
            }
            let keep_listening = options.repeat
                || options
                    .count
                    .is_some_and(|count| count > 1 && captured_events + 1 < count);
            if !keep_listening {
                let _ = capture_session.restore_submap();
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
        if !options.json {
            println!();
        }
    }

    let _ = flush_input(tty_fd);
    capture_session.close().map_err(ListenError::Io)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_hyprland(_options: Options) -> Result<(), ListenError> {
    Err(ListenError::Setup(
        "Hyprland capture is only available on Linux".into(),
    ))
}

fn run_terminal(options: Options) -> Result<(), ListenError> {
    let signals = SignalGuard::install()?;
    let mut export = open_export(options.output.as_deref())?;
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    // Discovery can be slow and must not happen while the user's terminal is
    // in raw mode. Capture mode is entered only after the prompt is visible.
    let session = ListenSession::capture();
    let mut capture_stdout = io::BufWriter::new(io::stdout());

    if options.json {
        eprintln!("whykey listen: press a key combination (Esc or Ctrl+C exits)");
    } else {
        println!("whykey listen");
        println!("Press a key combination. Press Esc or Ctrl+C to exit.");
        if options.repeat {
            println!("Repeat mode is on.");
        }
        println!();
    }
    let mut captured_events = 0_usize;

    loop {
        let Some((observed, original_termios)) =
            capture_terminal_key(deadline, &signals, options.json)?
        else {
            if options.json {
                eprintln!("Stopped.");
            } else {
                println!("\nStopped.");
            }
            return Ok(());
        };

        let results = session.inspect(&observed, Some(&original_termios));
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
        if !options.json {
            println!();
        }
    }
}

/// Capture one intentional terminal key, restoring termios and the keyboard
/// protocol before returning it for analysis.
fn capture_terminal_key(
    deadline: Option<Instant>,
    signals: &SignalGuard,
    json: bool,
) -> Result<Option<(ObservedKey, libc::termios)>, ListenError> {
    let mut terminal = TerminalSession::open()?;
    let original_termios = terminal.original;
    let mut reader = InputReader::with_pending(&mut terminal.tty, Vec::new());
    let waiting_since = Instant::now();
    let mut waiting_hint_shown = false;

    if json {
        eprintln!("Waiting for input...");
    } else {
        println!("Waiting for input...");
    }
    io::stdout().flush()?;

    loop {
        if signals.received() {
            return Err(ListenError::Message(
                "interrupted; terminal settings restored".into(),
            ));
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            return Err(ListenError::Message("capture timed out".into()));
        }
        match read_event(&mut reader, deadline)? {
            ReadEvent::Idle => {
                if !waiting_hint_shown && waiting_since.elapsed() >= WAITING_HINT_AFTER {
                    eprintln!(
                        "No event arrived. A shortcut may have been consumed before the terminal; inspect it by name with `whykey <combination>`."
                    );
                    waiting_hint_shown = true;
                }
            }
            ReadEvent::Key(mut observed) => {
                observed.protocol_flags = terminal.protocol_flags;
                if is_cancel_key(&observed) {
                    return Ok(None);
                }
                if observed.event_type != KeyEventType::Press || is_terminal_modifier_key(&observed)
                {
                    continue;
                }
                return Ok(Some((*observed, original_termios)));
            }
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
        (CaptureSource::Hyprland, Some(keycode)) => {
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

#[cfg(not(target_os = "linux"))]
fn run_evdev(_options: Options) -> Result<(), ListenError> {
    Err(ListenError::Message(
        "evdev capture is only available on Linux".into(),
    ))
}

#[cfg(target_os = "linux")]
fn run_evdev(options: Options) -> Result<(), ListenError> {
    let signals = SignalGuard::install()?;
    let mut export = open_export(options.output.as_deref())?;
    let mut session = EvdevSession::open(options.device.as_deref())?;
    // One snapshot per session: repeated physical reports reuse it instead
    // of rediscovering the desktop per event. Device state (modifiers,
    // SYN_DROPPED resync, hotplug, removal) still updates per read.
    let snapshot = ListenSession::capture();
    let mut capture_stdout = io::BufWriter::new(io::stdout());
    session.include_modifiers = options.events_all;
    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut captured_events = 0_usize;
    if options.json {
        eprintln!(
            "whykey listen --evdev: press a key combination (Esc or Ctrl+C exits; read-only, no grab)"
        );
    } else {
        println!("whykey listen --evdev");
        println!("Press a key combination. Press Esc or Ctrl+C to exit.");
        println!("Read-only capture; the input device is not grabbed.");
        if options.repeat {
            println!("Repeat mode is on.");
        }
        println!();
    }

    loop {
        if signals.received() {
            return Err(ListenError::Message("interrupted".into()));
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            return Err(ListenError::Message("capture timed out".into()));
        }
        let timeout_ms = deadline
            .map(remaining_millis)
            .map_or(100, |remaining| remaining.min(100));
        let Some(observed) = session.read_key_event(timeout_ms)? else {
            continue;
        };
        if observed.event_type == KeyEventType::Press && is_cancel_key(&observed) {
            if options.json {
                eprintln!("Stopped.");
            } else {
                println!("\nStopped.");
            }
            return Ok(());
        }

        let results = snapshot.inspect(&observed, None);
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
        if !options.json {
            println!();
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

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct LinuxInputEvent {
    _time: libc::timeval,
    event_type: u16,
    code: u16,
    value: i32,
}

#[cfg(target_os = "linux")]
struct EvdevDevice {
    path: PathBuf,
    name: String,
    file: File,
    pressed_modifiers: Vec<u16>,
    locked_modifiers: u32,
    /// Set after `SYN_DROPPED`; all events through the following
    /// `SYN_REPORT` are discarded before querying the kernel's current state.
    resync_required: bool,
}

#[cfg(target_os = "linux")]
struct EvdevSession {
    devices: Vec<EvdevDevice>,
    requested: Option<PathBuf>,
    last_hotplug_scan: Instant,
    include_modifiers: bool,
    /// Optional compositor-independent XKB map compiled from explicit RMLVO
    /// environment variables. Without one, retain the Linux key-name
    /// fallback and report the translation as conditional.
    xkb_keymap: Option<String>,
    xkb_group: usize,
}

#[cfg(target_os = "linux")]
impl EvdevSession {
    fn open(requested: Option<&Path>) -> Result<Self, ListenError> {
        let paths = if let Some(path) = requested {
            vec![path.to_owned()]
        } else {
            let mut paths = std::fs::read_dir("/dev/input")
                .map_err(|error| {
                    ListenError::Message(format!(
                        "cannot enumerate /dev/input: {error}; evdev capture may require the input group"
                    ))
                })?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("event"))
                })
                .collect::<Vec<_>>();
            paths.sort();
            paths
        };

        let mut devices = Vec::new();
        let mut failures = Vec::new();
        for path in paths.into_iter().take(64) {
            let keyboard_capability = evdev_keyboard_capability(&path);
            if keyboard_capability == Some(false) {
                if requested.is_some() {
                    failures.push(format!(
                        "{}: device does not advertise alphanumeric keyboard keys",
                        path.display()
                    ));
                }
                continue;
            }
            match File::open(&path) {
                Ok(file) => {
                    if let Err(error) = set_nonblocking(&file) {
                        failures.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                    let name = evdev_device_name(&path);
                    let pressed_modifiers = evdev_pressed_modifiers_from_kernel(&file);
                    let locked_modifiers = evdev_locked_modifiers_from_kernel(&file);
                    devices.push(EvdevDevice {
                        path,
                        name,
                        file,
                        pressed_modifiers,
                        locked_modifiers,
                        resync_required: false,
                    });
                }
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        if devices.is_empty() {
            let suffix = failures
                .first()
                .map(|failure| format!(" ({failure})"))
                .unwrap_or_default();
            return Err(ListenError::Message(format!(
                "no readable keyboard event device found{suffix}; add your user to the input group or pass --device /dev/input/eventN"
            )));
        }
        Ok(Self {
            devices,
            requested: requested.map(Path::to_owned),
            last_hotplug_scan: Instant::now(),
            include_modifiers: false,
            xkb_keymap: crate::xkb::compile_keymap_from_environment(),
            xkb_group: crate::xkb::group_from_environment(),
        })
    }

    fn key_name(&self, code: u16) -> String {
        self.xkb_keymap
            .as_deref()
            .and_then(|keymap| {
                crate::xkb::preferred_symbol_for_evdev_keycode(
                    keymap,
                    code,
                    self.xkb_group,
                    self.xkb_level(),
                )
            })
            .unwrap_or_else(|| {
                evdev_key_name(code)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("CODE:{code}"))
            })
    }

    fn xkb_level(&self) -> usize {
        let modifiers = self.current_modifiers();
        let shift = modifiers & 1 != 0;
        let caps = modifiers & 2 != 0;
        let altgr = self
            .devices
            .iter()
            .any(|device| device.pressed_modifiers.contains(&100));
        if altgr {
            usize::from(shift) + 2
        } else {
            usize::from(shift ^ caps)
        }
    }

    fn encoding_label(&self) -> &'static str {
        if self.xkb_keymap.is_some() {
            "evdev key event translated with explicit XKB RMLVO before compositor processing"
        } else {
            "evdev key event before compositor processing"
        }
    }

    fn read_key_event(&mut self, timeout_ms: i32) -> io::Result<Option<ObservedKey>> {
        self.maybe_add_hotplugged_devices();
        let mut pollfds = self
            .devices
            .iter()
            .map(|device| libc::pollfd {
                fd: device.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        // SAFETY: pollfds points to valid descriptors owned by self for the
        // duration of this call.
        let result = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                // SignalGuard records the signal; let the outer loop observe
                // it and return the normal interruption result instead of
                // exposing a low-level EINTR failure.
                return Ok(None);
            }
            return Err(error);
        }
        if result == 0 {
            return Ok(None);
        }
        let mut disconnected = Vec::new();
        let mut observed = None;
        for (index, pollfd) in pollfds.iter().enumerate() {
            if pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                disconnected.push(index);
                continue;
            }
            if pollfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let event = match read_linux_input_event(&mut self.devices[index].file) {
                Ok(event) => event,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => continue,
                Err(error) if matches!(error.raw_os_error(), Some(libc::EIO | libc::ENODEV)) => {
                    disconnected.push(index);
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    disconnected.push(index);
                    continue;
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", self.devices[index].path.display()),
                    ));
                }
            };
            if event.event_type == EV_SYN && event.code == SYN_DROPPED {
                self.devices[index].resync_required = true;
                continue;
            }
            if self.devices[index].resync_required {
                // The kernel requires userspace to ignore all events up to
                // and including the next SYN_REPORT after SYN_DROPPED. Only
                // then is EVIOCGKEY/EVIOCGLED a coherent snapshot again.
                if event.event_type == EV_SYN && event.code == SYN_REPORT {
                    self.refresh_device_modifiers(index);
                    self.devices[index].resync_required = false;
                }
                continue;
            }
            if event.event_type != EV_KEY {
                continue;
            }
            let event_type = match event.value {
                0 => KeyEventType::Release,
                1 => KeyEventType::Press,
                2 => KeyEventType::Repeat,
                _ => continue,
            };
            let modifier = evdev_modifier(event.code);
            if let Some(mask) = modifier {
                let lock_key = matches!(event.code, 58 | 69);
                if lock_key && event_type == KeyEventType::Press {
                    self.devices[index].locked_modifiers ^= mask;
                } else if !lock_key && event_type == KeyEventType::Release {
                    self.devices[index]
                        .pressed_modifiers
                        .retain(|code| *code != event.code);
                } else if !lock_key && !self.devices[index].pressed_modifiers.contains(&event.code)
                {
                    self.devices[index].pressed_modifiers.push(event.code);
                }
                if self.include_modifiers {
                    let key_name = self.key_name(event.code);
                    let device = self.devices[index].name.clone();
                    let path = self.devices[index].path.display().to_string();
                    let modifiers = self.current_modifiers();
                    observed = Some(ObservedKey {
                        combo: KeyCombo::from_parts(modifiers, key_name.clone()),
                        raw: Vec::new(),
                        raw_display: Some(format!(
                            "EV_KEY code={} value={} ({})",
                            event.code,
                            event.value,
                            event_type.label()
                        )),
                        modifier_state: Some(self.modifier_state()),
                        associated_text: None,
                        encoding: self.encoding_label().into(),
                        protocol_flags: None,
                        event_type,
                        alternate_keys: None,
                        alternate_key: Some(format!(
                            "physical modifier keycode {} ({key_name})",
                            event.code
                        )),
                        physical_keycode: Some(event.code),
                        source: CaptureSource::Evdev { device, path },
                        disposition: CaptureDisposition::ObservedOnly,
                    });
                    break;
                } else {
                    continue;
                }
            }
            if !self.include_modifiers && event_type != KeyEventType::Press {
                continue;
            }
            let modifiers_before = self.current_modifiers();
            let key_name = self.key_name(event.code);
            let combo = KeyCombo::from_parts(modifiers_before, key_name.clone());
            let device = self.devices[index].name.clone();
            let path = self.devices[index].path.display().to_string();
            observed = Some(ObservedKey {
                combo,
                raw: Vec::new(),
                raw_display: Some(format!(
                    "EV_KEY code={} value={} ({})",
                    event.code,
                    event.value,
                    event_type.label()
                )),
                modifier_state: Some(self.modifier_state()),
                associated_text: None,
                encoding: self.encoding_label().into(),
                protocol_flags: None,
                event_type,
                alternate_keys: None,
                alternate_key: Some(format!("physical keycode {} ({key_name})", event.code)),
                physical_keycode: Some(event.code),
                source: CaptureSource::Evdev { device, path },
                disposition: CaptureDisposition::ObservedOnly,
            });
            break;
        }
        for index in disconnected.into_iter().rev() {
            self.devices.remove(index);
        }
        if self.devices.is_empty() {
            self.maybe_add_hotplugged_devices();
        }
        if self.devices.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "all evdev keyboard devices were disconnected",
            ));
        }
        Ok(observed)
    }

    fn maybe_add_hotplugged_devices(&mut self) {
        if self.requested.is_some() || self.last_hotplug_scan.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_hotplug_scan = Instant::now();
        let Ok(entries) = std::fs::read_dir("/dev/input") else {
            return;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths.into_iter().take(64) {
            if self.devices.iter().any(|device| device.path == path)
                || evdev_keyboard_capability(&path) == Some(false)
            {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            if set_nonblocking(&file).is_err() {
                continue;
            }
            let name = evdev_device_name(&path);
            self.devices.push(EvdevDevice {
                path,
                name,
                pressed_modifiers: evdev_pressed_modifiers_from_kernel(&file),
                locked_modifiers: evdev_locked_modifiers_from_kernel(&file),
                file,
                resync_required: false,
            });
        }
    }

    fn refresh_device_modifiers(&mut self, index: usize) {
        let device = &mut self.devices[index];
        device.pressed_modifiers = evdev_pressed_modifiers_from_kernel(&device.file);
        device.locked_modifiers = evdev_locked_modifiers_from_kernel(&device.file);
    }

    fn current_modifiers(&self) -> u32 {
        self.devices.iter().fold(0, |current, device| {
            let pressed = device
                .pressed_modifiers
                .iter()
                .filter_map(|code| evdev_modifier(*code))
                .fold(0, |current, modifier| current | modifier);
            current | pressed | device.locked_modifiers
        })
    }

    fn modifier_state(&self) -> ModifierState {
        let devices = self
            .devices
            .iter()
            .map(|device| DeviceModifierState {
                device: device.name.clone(),
                path: device.path.display().to_string(),
                pressed: modifier_key_names(device.pressed_modifiers.iter().copied()),
                locked: evdev_lock_names(device.locked_modifiers),
            })
            .collect::<Vec<_>>();

        let mut pressed = devices
            .iter()
            .flat_map(|device| device.pressed.iter().cloned())
            .collect::<Vec<_>>();
        pressed.sort();
        pressed.dedup();

        let mut locked = devices
            .iter()
            .flat_map(|device| device.locked.iter().cloned())
            .collect::<Vec<_>>();
        locked.sort();
        locked.dedup();

        ModifierState {
            pressed,
            locked,
            latched: None,
            devices,
        }
    }
}

#[cfg(target_os = "linux")]
fn read_linux_input_event(file: &mut File) -> io::Result<LinuxInputEvent> {
    let mut bytes = [0_u8; std::mem::size_of::<LinuxInputEvent>()];
    file.read_exact(&mut bytes)?;
    // SAFETY: input_event is a C-compatible, Copy struct and bytes contains
    // exactly one kernel input_event record. read_unaligned handles any
    // alignment supplied by the byte array.
    Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<LinuxInputEvent>()) })
}

#[cfg(target_os = "linux")]
fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: the descriptor is owned by `file`; fcntl does not retain the
    // pointer and only changes the descriptor flags.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn evdev_device_name(path: &Path) -> String {
    let fallback = path.display().to_string();
    let Some(event_name) = path.file_name().and_then(|value| value.to_str()) else {
        return fallback;
    };
    std::fs::read_to_string(format!("/sys/class/input/{event_name}/device/name"))
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
}

#[cfg(target_os = "linux")]
fn evdev_keyboard_capability(path: &Path) -> Option<bool> {
    let event_name = path.file_name()?.to_str()?;
    let contents = std::fs::read_to_string(format!(
        "/sys/class/input/{event_name}/device/capabilities/key"
    ))
    .ok()?;
    Some(parse_evdev_keyboard_capability(&contents))
}

fn parse_evdev_keyboard_capability(contents: &str) -> bool {
    let words = contents
        .split_whitespace()
        .filter_map(|word| u64::from_str_radix(word, 16).ok())
        .collect::<Vec<_>>();
    let has_key = |code: usize| {
        words
            .get(code / 64)
            .is_some_and(|word| word & (1_u64 << (code % 64)) != 0)
    };
    has_key(30) || has_key(44) || has_key(57)
}

#[cfg(target_os = "linux")]
const EV_KEY: u16 = 0x01;

#[cfg(target_os = "linux")]
const EV_SYN: u16 = 0x00;

#[cfg(target_os = "linux")]
const SYN_DROPPED: u16 = 0x03;

#[cfg(target_os = "linux")]
const SYN_REPORT: u16 = 0x00;

#[cfg(target_os = "linux")]
fn evdev_pressed_modifiers_from_kernel(file: &File) -> Vec<u16> {
    // EVIOCGKEY(KEY_MAX + 1) returns the current pressed-key bitmap. The
    // ioctl constants are kept local so the crate does not need a generated
    // linux/input.h binding just to recover modifier state.
    const KEY_MAX: usize = 0x2ff;
    const BITMAP_BYTES: usize = (KEY_MAX + 8) / 8;
    const IOC_READ: u64 = 2;
    const IOC_TYPE: u64 = b'E' as u64;
    const IOC_NR: u64 = 0x18;
    let request = (IOC_READ << 30) | ((BITMAP_BYTES as u64) << 16) | (IOC_TYPE << 8) | IOC_NR;
    let mut bitmap = [0_u8; BITMAP_BYTES];
    // SAFETY: bitmap is writable storage of the exact size encoded in the
    // request and the descriptor is owned by the caller for this operation.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::c_ulong,
            bitmap.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Vec::new();
    }
    [29_u16, 97, 42, 54, 56, 100, 125, 126, 58, 69]
        .into_iter()
        .filter(|code| bitmap[usize::from(*code) / 8] & (1 << (code % 8)) != 0)
        .filter(|code| !matches!(code, 58 | 69))
        .collect()
}

#[cfg(target_os = "linux")]
fn evdev_locked_modifiers_from_kernel(file: &File) -> u32 {
    // EVIOCGLED(LED_MAX + 1) exposes the kernel's current NumLock and
    // CapsLock LEDs. Unlike EVIOCGKEY, this is the persistent lock state, not
    // merely whether the lock key is physically held right now.
    const LED_MAX: usize = 0x0f;
    const BITMAP_BYTES: usize = (LED_MAX + 8) / 8;
    const IOC_READ: u64 = 2;
    const IOC_TYPE: u64 = b'E' as u64;
    const IOC_NR: u64 = 0x19;
    let request = (IOC_READ << 30) | ((BITMAP_BYTES as u64) << 16) | (IOC_TYPE << 8) | IOC_NR;
    let mut bitmap = [0_u8; BITMAP_BYTES];
    // SAFETY: bitmap is writable storage of the exact size encoded in the
    // request and the descriptor is owned by the caller for this operation.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::c_ulong,
            bitmap.as_mut_ptr(),
        )
    };
    if result < 0 {
        return 0;
    }
    let mut modifiers = 0;
    if bitmap[0] & (1 << 0) != 0 {
        modifiers |= 16;
    }
    if bitmap[0] & (1 << 1) != 0 {
        modifiers |= 2;
    }
    modifiers
}

#[cfg(target_os = "linux")]
pub(crate) fn evdev_modifier(code: u16) -> Option<u32> {
    Some(match code {
        29 | 97 => 4,    // KEY_LEFTCTRL / KEY_RIGHTCTRL
        42 | 54 => 1,    // KEY_LEFTSHIFT / KEY_RIGHTSHIFT
        56 | 100 => 8,   // KEY_LEFTALT / KEY_RIGHTALT
        125 | 126 => 64, // KEY_LEFTMETA / KEY_RIGHTMETA
        58 => 2,         // KEY_CAPSLOCK
        69 => 16,        // KEY_NUMLOCK
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn modifier_key_names(codes: impl Iterator<Item = u16>) -> Vec<String> {
    let mut names = codes
        .filter_map(evdev_key_name)
        .filter(|name| evdev_modifier_name(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

#[cfg(target_os = "linux")]
fn evdev_modifier_name(name: &str) -> bool {
    matches!(
        name,
        "LEFTCTRL"
            | "RIGHTCTRL"
            | "LEFTSHIFT"
            | "RIGHTSHIFT"
            | "LEFTALT"
            | "RIGHTALT"
            | "LEFTMETA"
            | "RIGHTMETA"
    )
}

#[cfg(target_os = "linux")]
fn evdev_lock_names(mask: u32) -> Vec<String> {
    let mut names = Vec::new();
    if mask & 2 != 0 {
        names.push("CAPSLOCK".into());
    }
    if mask & 16 != 0 {
        names.push("NUMLOCK".into());
    }
    names
}

#[cfg(target_os = "linux")]
pub(crate) fn evdev_key_name(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "ESCAPE",
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "-",
        13 => "=",
        14 => "BACKSPACE",
        15 => "TAB",
        16 => "Q",
        17 => "W",
        18 => "E",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        26 => "[",
        27 => "]",
        28 => "RETURN",
        29 => "LEFTCTRL",
        30 => "A",
        31 => "S",
        32 => "D",
        33 => "F",
        34 => "G",
        35 => "H",
        36 => "J",
        37 => "K",
        38 => "L",
        39 => ";",
        40 => "'",
        41 => "`",
        42 => "LEFTSHIFT",
        43 => "\\",
        44 => "Z",
        45 => "X",
        46 => "C",
        47 => "V",
        48 => "B",
        49 => "N",
        50 => "M",
        51 => ",",
        52 => ".",
        53 => "/",
        54 => "RIGHTSHIFT",
        55 => "KP_MULTIPLY",
        56 => "LEFTALT",
        57 => "SPACE",
        58 => "CAPSLOCK",
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
        69 => "NUMLOCK",
        70 => "SCROLLLOCK",
        71 => "KP_7",
        72 => "KP_8",
        73 => "KP_9",
        74 => "KP_SUBTRACT",
        75 => "KP_4",
        76 => "KP_5",
        77 => "KP_6",
        78 => "KP_ADD",
        79 => "KP_1",
        80 => "KP_2",
        81 => "KP_3",
        82 => "KP_0",
        83 => "KP_DECIMAL",
        87 => "F11",
        88 => "F12",
        96 => "KP_ENTER",
        97 => "RIGHTCTRL",
        98 => "KP_DIVIDE",
        100 => "RIGHTALT",
        102 => "HOME",
        103 => "UP",
        104 => "PAGE_UP",
        105 => "LEFT",
        106 => "RIGHT",
        107 => "END",
        108 => "DOWN",
        109 => "PAGE_DOWN",
        110 => "INSERT",
        111 => "DELETE",
        113 => "VOLUME_MUTE",
        114 => "VOLUME_DOWN",
        115 => "VOLUME_UP",
        119 => "PAUSE",
        125 => "LEFTMETA",
        126 => "RIGHTMETA",
        _ => return None,
    })
}

struct TerminalSession {
    tty: File,
    original: libc::termios,
    protocol_enabled: bool,
    protocol_flags: Option<u32>,
}

struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

impl SignalGuard {
    fn install() -> io::Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = signal_handler as *const () as usize;
        action.sa_flags = 0;
        // SAFETY: `action` is valid writable storage and sigemptyset receives
        // a pointer to its initialized signal mask.
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut previous = Vec::new();
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            let mut old_action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            // SAFETY: both action pointers refer to valid sigaction structs;
            // the signal numbers are the four constants listed above.
            if unsafe { libc::sigaction(signal, &action, &mut old_action) } != 0 {
                for (old_signal, old_action) in previous.drain(..).rev() {
                    // SAFETY: these handlers were returned by sigaction for
                    // the corresponding signal and are restored verbatim.
                    unsafe { libc::sigaction(old_signal, &old_action, std::ptr::null_mut()) };
                }
                return Err(io::Error::last_os_error());
            }
            previous.push((signal, old_action));
        }
        Ok(Self { previous })
    }

    fn received(&self) -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}

extern "C" fn signal_handler(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, action) in self.previous.drain(..).rev() {
            // SAFETY: each action was returned by sigaction and belongs to
            // the matching signal number.
            unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
        }
    }
}

struct InputReader<'a> {
    tty: &'a mut File,
    pending: VecDeque<u8>,
}

impl<'a> InputReader<'a> {
    fn with_pending(tty: &'a mut File, pending: Vec<u8>) -> Self {
        Self {
            tty,
            pending: pending.into(),
        }
    }

    fn read_byte(&mut self) -> io::Result<u8> {
        if let Some(byte) = self.pending.pop_front() {
            return Ok(byte);
        }
        let mut buffer = [0_u8; 256];
        let count = self.tty.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            ));
        }
        self.pending.extend(&buffer[..count]);
        self.pending
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "terminal input closed"))
    }

    fn has_input(&self, timeout_ms: i32, deadline: Option<Instant>) -> io::Result<bool> {
        if !self.pending.is_empty() {
            return Ok(true);
        }
        let timeout_ms = deadline
            .map(|deadline| timeout_ms.min(remaining_millis(deadline)))
            .unwrap_or(timeout_ms);
        if timeout_ms < 0 {
            return Ok(false);
        }
        wait_for_input(self.tty.as_raw_fd(), timeout_ms)
    }

    fn read_raw_event(&mut self, deadline: Option<Instant>) -> io::Result<Vec<u8>> {
        if !self.has_input(SIGNAL_POLL_MS, deadline)? {
            return Ok(Vec::new());
        }
        let first = self.read_byte()?;
        if first != 0x1b {
            let width = utf8_width(first);
            let mut bytes = vec![first];
            for _ in 1..width {
                if !self.has_input(SEQUENCE_TIMEOUT_MS, deadline)? {
                    break;
                }
                bytes.push(self.read_byte()?);
            }
            return Ok(bytes);
        }
        if !self.has_input(LEGACY_ESCAPE_SEQUENCE_TIMEOUT_MS, deadline)? {
            return Ok(vec![first]);
        }

        let second = self.read_byte()?;
        if second == 0x1b {
            // Treat a second literal Escape as the next event, not an
            // Alt-prefixed Escape. This preserves immediate Escape
            // cancellation in legacy terminals as well as Kitty terminals.
            self.pending.push_front(second);
            return Ok(vec![first]);
        }

        let mut bytes = vec![first, second];
        while bytes.len() < MAX_TERMINAL_SEQUENCE_BYTES
            && self.has_input(SEQUENCE_TIMEOUT_MS, deadline)?
        {
            bytes.push(self.read_byte()?);
            if is_complete_escape_sequence(&bytes) {
                break;
            }
        }
        Ok(bytes)
    }
}

impl TerminalSession {
    fn open() -> io::Result<Self> {
        let mut tty = File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|error| {
                if matches!(error.raw_os_error(), Some(libc::ENXIO) | Some(libc::ENOTTY)) {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "no controlling terminal; run `whykey listen` inside a terminal",
                    )
                } else {
                    error
                }
            })?;
        let fd = tty.as_raw_fd();
        let original = get_termios(fd)?;
        let mut raw = original;

        // Keep ISIG enabled so legacy terminals can still deliver Ctrl+C as
        // SIGINT. Kitty terminals encode it as a key, handled above.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::IEXTEN);
        raw.c_iflag &= !(libc::IXON | libc::IXOFF | libc::ICRNL | libc::INLCR | libc::IGNCR);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        set_termios(fd, &raw)?;
        // Bit 1 disambiguates escape/control sequences. Bit 8 asks for all
        // keys as sequences, which preserves Shift and lock modifiers.
        let (protocol_flags, _) = match query_keyboard_protocol(&mut tty) {
            Ok(result) => result,
            Err(error) => {
                let _ = set_termios(fd, &original);
                return Err(error);
            }
        };
        let request = format!("\x1b[>{KITTY_CAPTURE_FLAGS}u");
        if let Err(error) = tty.write_all(request.as_bytes()) {
            let _ = set_termios(fd, &original);
            return Err(error);
        }
        tty.flush()?;
        // Input typed before capture is ready, including the Enter release
        // that launched the command and protocol-query leftovers, must never
        // be mistaken for the requested shortcut.
        flush_input(fd)?;
        Ok(Self {
            tty,
            original,
            protocol_enabled: true,
            protocol_flags,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.protocol_enabled {
            if let Err(error) = self.tty.write_all(b"\x1b[<u") {
                first_error = Some(error);
            }
            self.protocol_enabled = false;
        }
        if let Err(error) = set_termios(self.tty.as_raw_fd(), &self.original) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn get_termios(fd: i32) -> io::Result<libc::termios> {
    let mut termios = std::mem::MaybeUninit::uninit();
    // SAFETY: `termios` points to writable storage and `fd` comes from an open file.
    let result = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr initialized the structure on success.
    Ok(unsafe { termios.assume_init() })
}

fn set_termios(fd: i32, termios: &libc::termios) -> io::Result<()> {
    // SAFETY: `termios` points to a valid structure and `fd` comes from an open file.
    let result = unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn flush_input(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is the controlling terminal owned by this session.
    if unsafe { libc::tcflush(fd, libc::TCIFLUSH) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn query_keyboard_protocol(tty: &mut File) -> io::Result<(Option<u32>, Vec<u8>)> {
    tty.write_all(b"\x1b[?u")?;
    tty.flush()?;
    if !wait_for_input(tty.as_raw_fd(), 30)? {
        return Ok((None, Vec::new()));
    }

    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    tty.read_exact(&mut byte)?;
    response.push(byte[0]);
    if response[0] != 0x1b {
        return Ok((None, response));
    }
    if !wait_for_input(tty.as_raw_fd(), 12)? {
        return Ok((None, response));
    }
    tty.read_exact(&mut byte)?;
    response.push(byte[0]);
    if response[1] != b'[' {
        return Ok((None, response));
    }
    while response.len() < 32 && wait_for_input(tty.as_raw_fd(), 12)? {
        tty.read_exact(&mut byte)?;
        response.push(byte[0]);
        if byte[0] == b'u' {
            break;
        }
    }

    if let Some(value) = parse_protocol_response(&response) {
        return Ok((Some(value), Vec::new()));
    }
    Ok((None, response))
}

fn parse_protocol_response(response: &[u8]) -> Option<u32> {
    if !response.starts_with(b"\x1b[?") || response.last() != Some(&b'u') {
        return None;
    }
    std::str::from_utf8(&response[3..response.len() - 1])
        .ok()?
        .parse()
        .ok()
}

fn read_event(reader: &mut InputReader<'_>, deadline: Option<Instant>) -> io::Result<ReadEvent> {
    let bytes = reader.read_raw_event(deadline)?;
    if bytes.is_empty() {
        return Ok(ReadEvent::Idle);
    }
    Ok(ReadEvent::Key(Box::new(decode_bytes(bytes)?)))
}

fn remaining_millis(deadline: Instant) -> i32 {
    deadline
        .checked_duration_since(Instant::now())
        .map(|remaining| remaining.as_millis().min(i32::MAX as u128) as i32)
        .unwrap_or(0)
}

fn wait_for_input(fd: i32, timeout_ms: i32) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one valid descriptor entry for the duration of the call.
    let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            // SignalGuard records the signal in an atomic flag. Returning an
            // idle poll lets the capture loop observe that flag and produce
            // its normal restoration diagnostic instead of leaking EINTR.
            Ok(false)
        } else {
            Err(error)
        }
    } else if result == 0 {
        Ok(false)
    } else if pollfd.revents & libc::POLLIN != 0 {
        Ok(true)
    } else if pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "controlling terminal closed while waiting for input",
        ))
    } else {
        Ok(false)
    }
}

fn is_complete_escape_sequence(bytes: &[u8]) -> bool {
    let Some(last) = bytes.last().copied() else {
        return false;
    };
    bytes.len() > 2 && (last.is_ascii_alphabetic() || last == b'~')
}

fn decode_bytes(bytes: Vec<u8>) -> Result<ObservedKey, io::Error> {
    let decoded = decode_key(&bytes);
    // Metadata from a Kitty-looking byte sequence is evidence only when the
    // sequence itself decoded successfully. Otherwise malformed input must
    // remain a raw observation, not acquire an apparently valid alternate
    // key or associated text field.
    let associated_text = decoded
        .as_ref()
        .and_then(|_| associated_text_from_bytes(&bytes));
    let alternate_keys = decoded.as_ref().and_then(|_| alternate_keys(&bytes));
    let alternate_key = alternate_keys
        .as_ref()
        .and_then(|keys| keys.first().cloned());
    let (combo, encoding, event_type) = decoded.unwrap_or_else(|| {
        let raw = bytes
            .iter()
            .fold(String::with_capacity(bytes.len() * 2), |mut raw, byte| {
                let _ = write!(raw, "{byte:02x}");
                raw
            });
        (
            KeyCombo::from_parts(0, format!("RAW:{raw}")),
            "unrecognized terminal bytes; physical/layout origin is unknown".into(),
            // Keep a valid Kitty lifecycle marker even when its key code is
            // not one Whykey names yet. The listener must never turn an
            // unknown release into a fake press and report it as a shortcut.
            kitty_event_type(&bytes).unwrap_or(KeyEventType::Press),
        )
    });
    Ok(ObservedKey {
        combo,
        raw: bytes,
        raw_display: None,
        modifier_state: None,
        associated_text,
        physical_keycode: None,
        encoding,
        protocol_flags: None,
        event_type,
        alternate_keys,
        alternate_key,
        source: CaptureSource::Terminal,
        disposition: CaptureDisposition::ObservedOnly,
    })
}

fn kitty_event_type(bytes: &[u8]) -> Option<KeyEventType> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let modifier_and_event = body.split(';').nth(1)?;
    match modifier_and_event.split(':').nth(1)? {
        "1" => Some(KeyEventType::Press),
        "2" => Some(KeyEventType::Repeat),
        "3" => Some(KeyEventType::Release),
        _ => None,
    }
}

fn alternate_keys(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let field = body.split(';').next()?;
    kitty_alternate_names(field)
}

fn kitty_alternate_names(field: &str) -> Option<Vec<String>> {
    let mut values = Vec::new();
    for value in field.split(':').skip(1).filter(|value| !value.is_empty()) {
        let codepoint = value.parse::<u32>().ok()?;
        values.push(kitty_key_name(codepoint)?);
    }
    Some(values)
}

fn associated_text_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let text_field = body.split(';').nth(2)?;
    if text_field.is_empty() {
        return None;
    }
    let mut text = String::new();
    for value in text_field.split(':') {
        let codepoint = value.parse::<u32>().ok()?;
        let character = char::from_u32(codepoint)?;
        if character.is_control() {
            return None;
        }
        text.push(character);
    }
    (!text.is_empty()).then_some(text)
}

fn decode_key(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    if bytes.first() == Some(&0x1b) {
        return decode_escape(bytes);
    }
    if bytes.len() > 1 {
        return decode_utf8_text(bytes);
    }

    let (modifiers, key): (u32, String) = match bytes.first().copied()? {
        0x00 => (4, "SPACE".into()),
        b'\r' => (0, "RETURN".into()),
        b'\n' => (4, "J".into()),
        b'\t' => (0, "TAB".into()),
        0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a => (4, char::from(b'A' + bytes[0] - 1).to_string()),
        0x7f => (0, "BACKSPACE".into()),
        value if value.is_ascii_graphic() || value == b' ' => (0, char::from(value).to_string()),
        _ => return None,
    };
    let encoding = match bytes[0] {
        b'\t' => "legacy byte input; could also be CTRL + I",
        b'\r' => "legacy byte input; could also be CTRL + M",
        b'A'..=b'Z' => "legacy text byte; Shift/CapsLock cannot be distinguished",
        _ => "legacy byte input",
    };
    Some((
        KeyCombo::from_parts(modifiers, key),
        encoding.into(),
        KeyEventType::Press,
    ))
}

fn decode_utf8_text(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() || character.is_control() {
        return None;
    }
    let key = character.to_uppercase().collect::<String>();
    Some((
        KeyCombo::from_parts(0, key),
        "UTF-8 text input; Compose/layout origin cannot be distinguished".into(),
        KeyEventType::Press,
    ))
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

fn decode_escape(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    if bytes.len() == 1 {
        return Some((
            KeyCombo::from_parts(0, "ESCAPE"),
            "legacy Escape key".into(),
            KeyEventType::Press,
        ));
    }
    if bytes[1] == b'[' {
        return decode_csi(&bytes[2..]);
    }
    if bytes[1] == b'O' {
        return decode_ss3(&bytes[2..]);
    }
    if bytes.len() > 2 {
        let (combo, _, event_type) = decode_utf8_text(&bytes[1..])?;
        return Some((
            KeyCombo::from_parts(combo.modmask() | 8, combo.key()),
            "Alt-prefixed UTF-8 input; layout/Compose origin is ambiguous".into(),
            event_type,
        ));
    }
    if bytes.len() == 2 {
        let (modifiers, key) = decode_simple_byte(bytes[1])?;
        return Some((
            KeyCombo::from_parts(modifiers | 8, key),
            "legacy Alt-prefixed input".into(),
            KeyEventType::Press,
        ));
    }
    None
}

fn decode_csi(body: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let final_byte = *body.last()?;
    let params = std::str::from_utf8(&body[..body.len() - 1]).ok()?;
    if final_byte == b'u' {
        return decode_kitty(params);
    }

    let key = match final_byte {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'P' => "F1",
        b'Q' => "F2",
        b'S' => "F4",
        b'Z' if params.is_empty() => "TAB",
        b'~' => csi_tilde_key(params.split(';').next()?)?,
        _ => return None,
    };
    let modifiers = if final_byte == b'Z' {
        1
    } else {
        params
            .split(';')
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
            .map(xterm_modifiers)
            .unwrap_or(0)
    };
    Some((
        KeyCombo::from_parts(modifiers, key),
        format!("CSI sequence, final byte {}", char::from(final_byte)),
        KeyEventType::Press,
    ))
}

fn csi_tilde_key(number: &str) -> Option<&'static str> {
    Some(match number {
        "1" | "7" => "HOME",
        "2" => "INSERT",
        "3" => "DELETE",
        "4" | "8" => "END",
        "5" => "PAGE_UP",
        "6" => "PAGE_DOWN",
        "11" => "F1",
        "12" => "F2",
        "13" => "F3",
        "14" => "F4",
        "15" => "F5",
        "17" => "F6",
        "18" => "F7",
        "19" => "F8",
        "20" => "F9",
        "21" => "F10",
        "23" => "F11",
        "24" => "F12",
        "25" => "F13",
        "26" => "F14",
        "28" => "F15",
        "29" => "F16",
        "31" => "F17",
        "32" => "F18",
        "33" => "F19",
        "34" => "F20",
        _ => return None,
    })
}

fn decode_ss3(body: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let key = match *body.last()? {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'P' => "F1",
        b'Q' => "F2",
        b'R' => "F3",
        b'S' => "F4",
        _ => return None,
    };
    Some((
        KeyCombo::from_parts(0, key),
        "SS3 sequence".into(),
        KeyEventType::Press,
    ))
}

fn decode_kitty(params: &str) -> Option<(KeyCombo, String, KeyEventType)> {
    let mut fields = params.split(';');
    let key_field = fields.next()?;
    let codepoint = key_field.split(':').next()?.parse::<u32>().ok()?;
    kitty_alternate_names(key_field)?;
    let modifier_and_event = fields.next().unwrap_or("1");
    let mut modifier_fields = modifier_and_event.split(':');
    let modifier_field = modifier_fields
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("1")
        .parse::<u32>()
        .ok()?;
    let event_type = match modifier_fields.next().unwrap_or("1") {
        "1" => KeyEventType::Press,
        "2" => KeyEventType::Repeat,
        "3" => KeyEventType::Release,
        // Kitty reserves the event-type field. Treat an unknown value as an
        // unrecognized sequence instead of silently reporting a press.
        _ => return None,
    };
    // A second colon in the modifier/event field is not part of the Kitty
    // grammar. Reject it rather than accepting a malformed event as valid.
    if modifier_fields.next().is_some() {
        return None;
    }
    // The protocol has at most three semicolon-separated fields: key,
    // modifiers/event, and associated text. Extra fields are malformed.
    let _associated_text = fields.next();
    if fields.next().is_some() {
        return None;
    }
    let key = kitty_key_name(codepoint)?;
    Some((
        KeyCombo::from_parts(xterm_modifiers(modifier_field), key),
        "Kitty keyboard protocol".into(),
        event_type,
    ))
}

fn kitty_key_name(codepoint: u32) -> Option<String> {
    Some(match codepoint {
        9 => "TAB".into(),
        13 => "RETURN".into(),
        27 => "ESCAPE".into(),
        32 => "SPACE".into(),
        57358 => "CAPSLOCK".into(),
        57359 => "SCROLLLOCK".into(),
        57360 => "NUMLOCK".into(),
        57361 => "PRINTSCREEN".into(),
        57362 => "PAUSE".into(),
        57363 => "MENU".into(),
        57376..=57398 => format!("F{}", codepoint - 57363),
        57399..=57408 => format!("KP_{}", codepoint - 57399),
        57409 => "KP_DECIMAL".into(),
        57410 => "KP_DIVIDE".into(),
        57411 => "KP_MULTIPLY".into(),
        57412 => "KP_SUBTRACT".into(),
        57413 => "KP_ADD".into(),
        57414 => "KP_ENTER".into(),
        57415 => "KP_EQUAL".into(),
        57416 => "KP_SEPARATOR".into(),
        57417 => "KP_LEFT".into(),
        57418 => "KP_RIGHT".into(),
        57419 => "KP_UP".into(),
        57420 => "KP_DOWN".into(),
        57421 => "KP_PAGE_UP".into(),
        57422 => "KP_PAGE_DOWN".into(),
        57423 => "KP_HOME".into(),
        57424 => "KP_END".into(),
        57425 => "KP_INSERT".into(),
        57426 => "KP_DELETE".into(),
        57427 => "KP_BEGIN".into(),
        57428 => "MEDIA_PLAY".into(),
        57429 => "MEDIA_PAUSE".into(),
        57430 => "MEDIA_PLAY_PAUSE".into(),
        57431 => "MEDIA_REVERSE".into(),
        57432 => "MEDIA_STOP".into(),
        57433 => "MEDIA_FAST_FORWARD".into(),
        57434 => "MEDIA_REWIND".into(),
        57435 => "MEDIA_TRACK_NEXT".into(),
        57436 => "MEDIA_TRACK_PREVIOUS".into(),
        57437 => "MEDIA_RECORD".into(),
        57438 => "LOWER_VOLUME".into(),
        57439 => "RAISE_VOLUME".into(),
        57440 => "MUTE_VOLUME".into(),
        57441 => "LEFT_SHIFT".into(),
        57442 => "LEFT_CONTROL".into(),
        57443 => "LEFT_ALT".into(),
        57444 => "LEFT_SUPER".into(),
        57445 => "LEFT_HYPER".into(),
        57446 => "LEFT_META".into(),
        57447 => "RIGHT_SHIFT".into(),
        57448 => "RIGHT_CONTROL".into(),
        57449 => "RIGHT_ALT".into(),
        57450 => "RIGHT_SUPER".into(),
        57451 => "RIGHT_HYPER".into(),
        57452 => "RIGHT_META".into(),
        57453 => "ISO_LEVEL3_SHIFT".into(),
        57454 => "ISO_LEVEL5_SHIFT".into(),
        0 => "TEXT".into(),
        value => {
            let character = char::from_u32(value)?;
            if character.is_control() {
                return None;
            }
            character.to_uppercase().collect()
        }
    })
}

fn decode_simple_byte(byte: u8) -> Option<(u32, String)> {
    if (0x01..=0x1a).contains(&byte) {
        return Some((4, char::from(b'A' + byte - 1).to_string()));
    }
    if byte.is_ascii_alphabetic() {
        return Some((0, char::from(byte).to_ascii_uppercase().to_string()));
    }
    match byte {
        b'\r' => Some((0, "RETURN".into())),
        b'\t' => Some((0, "TAB".into())),
        b' ' => Some((0, "SPACE".into())),
        _ => None,
    }
}

fn xterm_modifiers(value: u32) -> u32 {
    let value = value.saturating_sub(1);
    let mut modifiers = 0;
    if value & 1 != 0 {
        modifiers |= 1;
    }
    if value & 2 != 0 {
        modifiers |= 8;
    }
    if value & 4 != 0 {
        modifiers |= 4;
    }
    if value & 8 != 0 {
        modifiers |= 64;
    }
    if value & 16 != 0 {
        modifiers |= 32;
    }
    if value & 32 != 0 {
        modifiers |= 128;
    }
    if value & 64 != 0 {
        modifiers |= 2;
    }
    if value & 128 != 0 {
        modifiers |= 16;
    }
    modifiers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::format_bytes;

    #[test]
    fn decodes_ctrl_z() {
        let event = decode_bytes(vec![0x1a]).unwrap();
        assert_eq!(event.combo.to_string(), "CTRL + Z");
        assert_eq!(event.raw, vec![0x1a]);
    }

    #[test]
    fn decodes_ctrl_left_csi() {
        let event = decode_bytes(b"\x1b[1;5D".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "CTRL + LEFT");
    }

    #[test]
    fn decodes_alt_x() {
        let event = decode_bytes(b"\x1bx".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "ALT + X");
    }

    #[test]
    fn decodes_kitty_ctrl_left() {
        let event = decode_bytes(b"\x1b[97;5u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "CTRL + A");
    }

    #[test]
    fn decodes_kitty_super_c() {
        let event = decode_bytes(b"\x1b[99;9u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "SUPER + C");
    }

    #[test]
    fn decodes_ss3_left() {
        let event = decode_bytes(b"\x1bOD".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "LEFT");
    }

    #[test]
    fn decodes_legacy_function_key_csi_variants() {
        assert_eq!(
            decode_bytes(b"\x1b[15~".to_vec()).unwrap().combo.key(),
            "F5"
        );
        assert_eq!(
            decode_bytes(b"\x1b[24~".to_vec()).unwrap().combo.key(),
            "F12"
        );
        assert_eq!(
            decode_bytes(b"\x1b[1;5~".to_vec()).unwrap().combo.key(),
            "HOME"
        );
        assert_eq!(
            decode_bytes(b"\x1b[15;5~".to_vec())
                .unwrap()
                .combo
                .to_string(),
            "CTRL + F5"
        );
        assert_eq!(
            decode_bytes(b"\x1b[1;5P".to_vec())
                .unwrap()
                .combo
                .to_string(),
            "CTRL + F1"
        );
    }

    #[test]
    fn decodes_kitty_f13() {
        let event = decode_bytes(b"\x1b[57376;1u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "F13");
    }

    #[test]
    fn decodes_kitty_f35() {
        let event = decode_bytes(b"\x1b[57398;1u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "F35");
    }

    #[test]
    fn decodes_kitty_unicode() {
        let event = decode_bytes("\x1b[233;1u".as_bytes().to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "É");
    }

    #[test]
    fn decodes_utf8_text_without_dropping_continuation_bytes() {
        let event = decode_bytes("é".as_bytes().to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "É");
        assert!(event.encoding.contains("UTF-8"));
        assert!(event.encoding.contains("Compose"));
    }

    #[test]
    fn compose_fixture_reproduces_committed_text_without_inventing_history() {
        let fixture = include_str!("../tests/fixtures/remappers/compose-e-acute.txt");
        assert!(fixture.contains("Compose"));
        assert!(fixture.contains("é"));

        let event = decode_bytes("é".as_bytes().to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "É");
        assert_eq!(
            event.encoding,
            "UTF-8 text input; Compose/layout origin cannot be distinguished"
        );
    }

    #[test]
    fn decodes_alt_prefixed_utf8_text() {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice("é".as_bytes());
        let event = decode_bytes(bytes).unwrap();
        assert_eq!(event.combo.to_string(), "ALT + É");
    }

    #[test]
    fn decodes_legacy_alt_control_bytes() {
        let event = decode_bytes(vec![0x1b, 0x18]).unwrap();
        assert_eq!(event.combo.to_string(), "CTRL + ALT + X");
    }

    #[test]
    fn preserves_unrecognized_bytes_as_a_raw_observation() {
        let event = decode_bytes(vec![0xc3, 0x28]).unwrap();
        assert_eq!(event.combo.key(), "RAW:C328");
        assert!(event.encoding.contains("unrecognized"));
    }

    #[test]
    fn detects_utf8_sequence_widths() {
        assert_eq!(utf8_width(b'a'), 1);
        assert_eq!(utf8_width(0xc3), 2);
        assert_eq!(utf8_width(0xe2), 3);
        assert_eq!(utf8_width(0xf0), 4);
    }

    #[test]
    fn decodes_kitty_repeat_and_release_events() {
        let repeat = decode_bytes(b"\x1b[102;1:2u".to_vec()).unwrap();
        let release = decode_bytes(b"\x1b[102;1:3u".to_vec()).unwrap();
        assert_eq!(repeat.event_type, KeyEventType::Repeat);
        assert_eq!(release.event_type, KeyEventType::Release);
    }

    #[test]
    fn preserves_lifecycle_for_an_unknown_kitty_key() {
        let event = decode_bytes(b"\x1b[127;1:3u".to_vec()).unwrap();
        assert_eq!(event.combo.key(), "RAW:1B5B3132373B313A3375");
        assert_eq!(event.event_type, KeyEventType::Release);
    }

    #[test]
    fn does_not_invent_a_press_for_unknown_kitty_event_types() {
        let event = decode_bytes(b"\x1b[102;1:9u".to_vec()).unwrap();
        assert!(event.combo.key().starts_with("RAW:"));
        assert_eq!(event.alternate_key, None);
        assert_eq!(event.alternate_keys, None);
        assert_eq!(event.associated_text, None);
    }

    #[test]
    fn rejects_extra_kitty_fields() {
        let event = decode_bytes(b"\x1b[102;1:1;65;66u".to_vec()).unwrap();
        assert!(event.combo.key().starts_with("RAW:"));
        assert_eq!(event.associated_text, None);
    }

    #[test]
    fn decodes_a_kitty_alternate_key() {
        let event = decode_bytes(b"\x1b[97:98;1u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "A");
        assert_eq!(event.alternate_key.as_deref(), Some("B"));
        assert_eq!(event.alternate_keys, Some(vec!["B".into()]));

        let base_only = decode_bytes(b"\x1b[97::99;1u".to_vec()).unwrap();
        assert_eq!(base_only.alternate_key.as_deref(), Some("C"));
        assert_eq!(base_only.alternate_keys, Some(vec!["C".into()]));
    }

    #[test]
    fn preserves_multiple_kitty_alternate_keys_in_wire_order() {
        let event = decode_bytes(b"\x1b[97:98:99;1u".to_vec()).unwrap();
        assert_eq!(event.alternate_key.as_deref(), Some("B"));
        assert_eq!(event.alternate_keys, Some(vec!["B".into(), "C".into()]));
    }

    #[test]
    fn rejects_an_unknown_kitty_alternate_key_without_partial_metadata() {
        let event = decode_bytes(b"\x1b[97:98:9999999;1u".to_vec()).unwrap();
        assert!(event.combo.key().starts_with("RAW:"));
        assert_eq!(event.alternate_key, None);
        assert_eq!(event.alternate_keys, None);
    }

    #[test]
    fn decodes_kitty_associated_text_without_confusing_it_with_alternate_key() {
        let event = decode_bytes(b"\x1b[97;2;65u".to_vec()).unwrap();
        assert_eq!(event.combo.to_string(), "SHIFT + A");
        assert_eq!(event.associated_text.as_deref(), Some("A"));
        assert_eq!(event.alternate_key, None);
    }

    #[test]
    fn decodes_kitty_text_only_events() {
        let event = decode_bytes(b"\x1b[0;;229:776u".to_vec()).unwrap();
        assert_eq!(event.combo.key(), "TEXT");
        assert_eq!(event.associated_text.as_deref(), Some("\u{e5}\u{308}"));
    }

    #[test]
    fn formats_control_bytes() {
        assert_eq!(format_bytes(&[0x1b, b'[', b'1', 0x1a]), "ESC [ 1 0x1a");
    }

    #[test]
    fn marks_legacy_ambiguities() {
        let tab = decode_bytes(vec![b'\t']).unwrap();
        assert!(tab.encoding.contains("CTRL + I"));

        let uppercase = decode_bytes(vec![b'A']).unwrap();
        assert!(uppercase.encoding.contains("Shift/CapsLock"));
    }

    #[test]
    fn parses_keyboard_protocol_flags() {
        assert_eq!(parse_protocol_response(b"\x1b[?0u"), Some(0));
        assert_eq!(parse_protocol_response(b"\x1b[?9u"), Some(9));
        assert_eq!(parse_protocol_response(b"\x1b[1;5D"), None);
        assert_eq!(KITTY_CAPTURE_FLAGS, 31);
    }

    #[cfg(unix)]
    #[test]
    fn queries_fragmented_keyboard_protocol_response() {
        use std::io::{Read as _, Write as _};
        use std::os::fd::{FromRawFd, RawFd};

        let mut descriptors = [0; 2];
        // SAFETY: descriptors points to two writable integers and ownership
        // is transferred to exactly one File for each socket endpoint below.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_STREAM,
                    0,
                    descriptors.as_mut_ptr(),
                )
            },
            0
        );
        let mut tty = unsafe { File::from_raw_fd(descriptors[0] as RawFd) };
        let mut peer = unsafe { File::from_raw_fd(descriptors[1] as RawFd) };
        let responder = std::thread::spawn(move || {
            let mut request = [0_u8; 4];
            peer.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"\x1b[?u");
            peer.write_all(b"\x1b[?").unwrap();
            std::thread::sleep(Duration::from_millis(2));
            peer.write_all(b"31u").unwrap();
        });

        let result = query_keyboard_protocol(&mut tty).unwrap();
        responder.join().unwrap();
        assert_eq!(result, (Some(31), Vec::new()));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_malformed_keyboard_protocol_response_for_input_reader() {
        use std::io::{Read as _, Write as _};
        use std::os::fd::{FromRawFd, RawFd};

        let mut descriptors = [0; 2];
        // SAFETY: descriptors points to two writable integers and ownership
        // is transferred to exactly one File for each socket endpoint below.
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_STREAM,
                    0,
                    descriptors.as_mut_ptr(),
                )
            },
            0
        );
        let mut tty = unsafe { File::from_raw_fd(descriptors[0] as RawFd) };
        let mut peer = unsafe { File::from_raw_fd(descriptors[1] as RawFd) };
        let responder = std::thread::spawn(move || {
            let mut request = [0_u8; 4];
            peer.read_exact(&mut request).unwrap();
            peer.write_all(b"\x1b[?x").unwrap();
            std::thread::sleep(Duration::from_millis(40));
        });

        let result = query_keyboard_protocol(&mut tty).unwrap();
        responder.join().unwrap();
        assert_eq!(result, (None, b"\x1b[?x".to_vec()));
    }

    #[test]
    fn decodes_extended_kitty_modifier_bits() {
        let modifiers = xterm_modifiers(1 + 64 + 128 + 16 + 32);
        assert_eq!(modifiers, 2 | 16 | 32 | 128);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn maps_common_evdev_keys_and_modifiers() {
        assert_eq!(evdev_key_name(30), Some("A"));
        assert_eq!(evdev_key_name(105), Some("LEFT"));
        assert_eq!(evdev_key_name(59), Some("F1"));
        assert_eq!(evdev_modifier(29), Some(4));
        assert_eq!(evdev_modifier(56), Some(8));
        assert_eq!(evdev_modifier(30), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn classifies_evdev_capability_fixtures_without_event_number_assumptions() {
        let letter = include_str!("../tests/fixtures/evdev/keyboard-letter.hex");
        let space = include_str!("../tests/fixtures/evdev/keyboard-space.hex");
        let non_keyboard = include_str!("../tests/fixtures/evdev/non-keyboard.hex");
        assert!(parse_evdev_keyboard_capability(letter));
        assert!(parse_evdev_keyboard_capability(space));
        assert!(!parse_evdev_keyboard_capability(non_keyboard));
        assert!(!parse_evdev_keyboard_capability("not-a-capability"));
    }

    #[test]
    fn overlong_decoder_input_is_bounded_and_deterministic() {
        let overlong = vec![0x1bu8; MAX_TERMINAL_SEQUENCE_BYTES + 16];
        let first = decode_bytes(overlong.clone()).unwrap();
        let second = decode_bytes(overlong).unwrap();
        assert_eq!(first.combo.to_string(), second.combo.to_string());
        assert_eq!(first.event_type, second.event_type);
    }

    #[test]
    fn truncated_and_valid_sequences_decode_deterministically() {
        let truncated = decode_bytes(b"\x1b[".to_vec()).unwrap();
        assert_eq!(
            decode_bytes(b"\x1b[".to_vec()).unwrap().combo.to_string(),
            truncated.combo.to_string()
        );
        let kitty = decode_bytes(b"\x1b[97;5u".to_vec()).unwrap();
        assert_eq!(kitty.combo.to_string(), "CTRL + A");
        assert_eq!(kitty.event_type, KeyEventType::Press);
    }

    #[test]
    fn listener_session_collects_one_stable_snapshot() {
        let first = ListenSession::capture();
        let second = ListenSession::capture();
        assert_eq!(
            first.environment.selected_compositor, second.environment.selected_compositor,
            "one snapshot per session must give a stable selection"
        );
    }
    #[test]
    fn terminal_cancel_and_modifier_policy_is_explicit() {
        fn observed(combo: &str, raw: Vec<u8>) -> ObservedKey {
            ObservedKey {
                combo: combo.parse().unwrap(),
                raw,
                raw_display: None,
                modifier_state: None,
                associated_text: None,
                physical_keycode: None,
                encoding: "synthetic test event".into(),
                protocol_flags: None,
                event_type: KeyEventType::Press,
                alternate_keys: None,
                alternate_key: None,
                source: CaptureSource::Terminal,
                disposition: CaptureDisposition::ObservedOnly,
            }
        }
        assert!(is_cancel_key(&observed("escape", vec![0x1b])));
        assert!(is_cancel_key(&observed("ctrl+c", vec![0x03])));
        assert!(is_terminal_modifier_key(&observed(
            "LEFT_CONTROL",
            vec![0x1b]
        )));
        assert!(!is_terminal_modifier_key(&observed("ctrl+z", vec![0x1a])));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_discards_events_until_syn_report_after_dropped_batch() {
        use std::io::Write as _;
        use std::os::fd::FromRawFd;

        let mut descriptors = [0; 2];
        // SAFETY: `descriptors` points to two writable integers for libc to
        // initialize; both ends are owned immediately after this call.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: each descriptor was returned by pipe and is transferred to
        // exactly one File owner here.
        let reader = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
        set_nonblocking(&reader).unwrap();

        let mut session = EvdevSession {
            devices: vec![EvdevDevice {
                path: PathBuf::from("/dev/input/event-test"),
                name: "test keyboard".into(),
                file: reader,
                pressed_modifiers: Vec::new(),
                locked_modifiers: 0,
                resync_required: false,
            }],
            requested: Some(PathBuf::from("/dev/input/event-test")),
            last_hotplug_scan: Instant::now(),
            include_modifiers: false,
            xkb_keymap: None,
            xkb_group: 0,
        };

        let write_event = |writer: &mut File, event_type: u16, code: u16, value: i32| {
            let event = LinuxInputEvent {
                _time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                event_type,
                code,
                value,
            };
            // SAFETY: `event` is a repr(C), Copy value and the byte slice is
            // used only for the duration of this write.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (&event as *const LinuxInputEvent).cast::<u8>(),
                    std::mem::size_of::<LinuxInputEvent>(),
                )
            };
            writer.write_all(bytes).unwrap();
        };

        // Establish modifier state before the kernel reports that events were
        // lost. The resync snapshot below must not leave this stale Ctrl held.
        write_event(&mut writer, EV_KEY, 29, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut writer, EV_SYN, SYN_DROPPED, 0);
        assert!(session.read_key_event(0).unwrap().is_none());
        // This key belongs to the dropped batch and must not leak into the
        // diagnostic as if it were a fresh physical event.
        write_event(&mut writer, EV_KEY, 30, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut writer, EV_SYN, SYN_REPORT, 0);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut writer, EV_KEY, 31, 1);
        let observed = session.read_key_event(0).unwrap().expect("fresh key");
        assert_eq!(observed.combo.key(), "S");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_keeps_modifier_state_isolated_between_devices() {
        use std::io::Write as _;
        use std::os::fd::FromRawFd;

        let mut first_descriptors = [0; 2];
        let mut second_descriptors = [0; 2];
        assert_eq!(unsafe { libc::pipe(first_descriptors.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(second_descriptors.as_mut_ptr()) }, 0);
        let first_reader = unsafe { File::from_raw_fd(first_descriptors[0]) };
        let mut first_writer = unsafe { File::from_raw_fd(first_descriptors[1]) };
        let second_reader = unsafe { File::from_raw_fd(second_descriptors[0]) };
        let mut second_writer = unsafe { File::from_raw_fd(second_descriptors[1]) };
        set_nonblocking(&first_reader).unwrap();
        set_nonblocking(&second_reader).unwrap();

        let mut session = EvdevSession {
            devices: vec![
                EvdevDevice {
                    path: PathBuf::from("/dev/input/event-first"),
                    name: "first keyboard".into(),
                    file: first_reader,
                    pressed_modifiers: Vec::new(),
                    locked_modifiers: 0,
                    resync_required: false,
                },
                EvdevDevice {
                    path: PathBuf::from("/dev/input/event-second"),
                    name: "second keyboard".into(),
                    file: second_reader,
                    pressed_modifiers: Vec::new(),
                    locked_modifiers: 0,
                    resync_required: false,
                },
            ],
            requested: Some(PathBuf::from("/dev/input/event-first")),
            last_hotplug_scan: Instant::now(),
            include_modifiers: false,
            xkb_keymap: None,
            xkb_group: 0,
        };

        let write_event = |writer: &mut File, code: u16, value: i32| {
            let event = LinuxInputEvent {
                _time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                event_type: EV_KEY,
                code,
                value,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (&event as *const LinuxInputEvent).cast::<u8>(),
                    std::mem::size_of::<LinuxInputEvent>(),
                )
            };
            writer.write_all(bytes).unwrap();
        };

        // Ctrl on either keyboard must apply to a key arriving on the other.
        write_event(&mut first_writer, 29, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 97, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 30, 1);
        let observed = session.read_key_event(0).unwrap().expect("Ctrl+A");
        assert_eq!(observed.combo.to_string(), "CTRL + A");
        assert_eq!(
            observed
                .modifier_state
                .as_ref()
                .map(|state| state.pressed.clone()),
            Some(vec!["LEFTCTRL".into(), "RIGHTCTRL".into()])
        );
        assert_eq!(
            observed
                .modifier_state
                .as_ref()
                .map(|state| state.locked.clone()),
            Some(Vec::new())
        );
        let state = observed.modifier_state.as_ref().unwrap();
        assert_eq!(state.devices.len(), 2);
        assert_eq!(state.devices[0].pressed, vec!["LEFTCTRL"]);
        assert_eq!(state.devices[1].pressed, vec!["RIGHTCTRL"]);
        let json = serde_json::to_value(&observed).unwrap();
        assert_eq!(
            json["modifier_state"]["devices"][0]["pressed"][0],
            "LEFTCTRL"
        );

        // Releasing one Ctrl must not clear the other keyboard's still-held
        // modifier, and releasing the final Ctrl must clear the aggregate.
        write_event(&mut first_writer, 29, 0);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 31, 1);
        let observed = session.read_key_event(0).unwrap().expect("Ctrl+S");
        assert_eq!(observed.combo.to_string(), "CTRL + S");
        write_event(&mut second_writer, 97, 0);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 32, 1);
        let observed = session.read_key_event(0).unwrap().expect("D");
        assert_eq!(observed.combo.to_string(), "D");
        assert_eq!(
            observed
                .modifier_state
                .as_ref()
                .map(|state| state.pressed.clone()),
            Some(Vec::new())
        );

        // Lock-key presses toggle the persistent state even though they do
        // not complete a normal (non-`--events all`) capture. The next key
        // sees CapsLock, and a second press clears it again.
        write_event(&mut first_writer, 58, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 30, 1);
        let observed = session.read_key_event(0).unwrap().expect("Caps+A");
        assert_eq!(observed.combo.to_string(), "CAPS + A");
        assert_eq!(
            observed
                .modifier_state
                .as_ref()
                .map(|state| state.locked.clone()),
            Some(vec!["CAPSLOCK".into()])
        );
        write_event(&mut first_writer, 58, 1);
        assert!(session.read_key_event(0).unwrap().is_none());
        write_event(&mut second_writer, 32, 1);
        let observed = session.read_key_event(0).unwrap().expect("D");
        assert_eq!(observed.combo.to_string(), "D");
        assert_eq!(
            observed
                .modifier_state
                .as_ref()
                .map(|state| state.locked.clone()),
            Some(Vec::new())
        );

        session.devices[0].locked_modifiers = 2;
        write_event(&mut second_writer, 33, 1);
        let observed = session.read_key_event(0).unwrap().expect("Caps+F");
        let state = observed.modifier_state.as_ref().unwrap();
        assert_eq!(state.locked, vec!["CAPSLOCK"]);
        assert_eq!(state.devices[0].locked, vec!["CAPSLOCK"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_uses_explicit_xkb_layout_before_linux_key_name_fallback() {
        let session = EvdevSession {
            devices: Vec::new(),
            requested: None,
            last_hotplug_scan: Instant::now(),
            include_modifiers: false,
            xkb_keymap: Some(
                r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ a ]
        symbols[2] = [ b ]
    };
};
};
"#
                .into(),
            ),
            xkb_group: 1,
        };
        assert_eq!(session.key_name(16), "b");
        assert!(session.encoding_label().contains("explicit XKB RMLVO"));
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_keeps_fragmented_utf8_together() {
        use std::io::Write as _;
        use std::os::fd::FromRawFd;

        let mut descriptors = [0; 2];
        // SAFETY: `descriptors` points to two writable integers for libc to
        // initialize; both ends are owned immediately after this call.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: each descriptor was returned by pipe and is transferred to
        // exactly one File owner here.
        let mut reader_file = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
        let writer_thread = std::thread::spawn(move || {
            writer.write_all(&[0xc3]).unwrap();
            std::thread::sleep(Duration::from_millis(2));
            writer.write_all(&[0xa9]).unwrap();
        });

        let mut reader = InputReader::with_pending(&mut reader_file, vec![]);
        let bytes = reader.read_raw_event(None).unwrap();
        writer_thread.join().unwrap();
        assert_eq!(bytes, "é".as_bytes());
        assert_eq!(decode_bytes(bytes).unwrap().combo.to_string(), "É");
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_preserves_consecutive_escape_events() {
        use std::io::Write as _;
        use std::os::fd::FromRawFd;

        let mut descriptors = [0; 2];
        // SAFETY: `descriptors` points to two writable integers for libc to
        // initialize; both ends are owned immediately after this call.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: each descriptor was returned by pipe and is transferred to
        // exactly one File owner here.
        let mut reader_file = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
        writer.write_all(b"\x1b[A\x1b[B").unwrap();
        drop(writer);

        let mut reader = InputReader::with_pending(&mut reader_file, vec![]);
        assert_eq!(reader.read_raw_event(None).unwrap(), b"\x1b[A");
        assert_eq!(reader.read_raw_event(None).unwrap(), b"\x1b[B");
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_keeps_two_literal_escapes_as_two_events() {
        use std::io::Write as _;
        use std::os::fd::FromRawFd;

        let mut descriptors = [0; 2];
        // SAFETY: `descriptors` points to two writable integers for libc to
        // initialize; both ends are owned immediately after this call.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: each descriptor is transferred to exactly one File owner.
        let mut reader_file = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
        writer.write_all(b"\x1b\x1b").unwrap();

        let mut reader = InputReader::with_pending(&mut reader_file, vec![]);
        assert_eq!(reader.read_raw_event(None).unwrap(), b"\x1b");
        assert_eq!(reader.read_raw_event(None).unwrap(), b"\x1b");
        drop(writer);
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_reports_a_closed_pty_instead_of_spinning() {
        use std::os::fd::{FromRawFd, RawFd};

        let mut descriptors = [0; 2];
        // SAFETY: `descriptors` points to two writable integers for libc to
        // initialize; both ends are owned immediately after this call.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        // SAFETY: the read descriptor is transferred to exactly one File.
        let mut reader_file = unsafe { File::from_raw_fd(descriptors[0] as RawFd) };
        // SAFETY: dropping this File closes the only write end, producing the
        // same hangup a terminal listener sees when its PTY disappears.
        drop(unsafe { File::from_raw_fd(descriptors[1] as RawFd) });

        let mut reader = InputReader::with_pending(&mut reader_file, vec![]);
        let error = reader
            .read_raw_event(None)
            .expect_err("closed input must not look idle forever");
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    }

    #[cfg(unix)]
    #[test]
    fn input_reader_caps_an_incomplete_escape_sequence() {
        let mut pending = vec![0x1b, 0x00];
        pending.extend(std::iter::repeat_n(0x00, MAX_TERMINAL_SEQUENCE_BYTES - 2));
        pending.push(b'a');
        let mut reader_file = File::open("/dev/null").unwrap();
        let mut reader = InputReader::with_pending(&mut reader_file, pending);

        let bytes = reader.read_raw_event(None).unwrap();
        assert_eq!(bytes.len(), MAX_TERMINAL_SEQUENCE_BYTES);
        assert!(bytes.iter().all(|byte| *byte == 0 || *byte == 0x1b));
        assert_eq!(reader.read_raw_event(None).unwrap(), b"a");
    }

    #[test]
    fn distinguishes_physical_capture_from_terminal_capture() {
        assert!(CaptureSource::Terminal.confirms_terminal());
        assert!(
            !CaptureSource::Evdev {
                device: "Keyboard".into(),
                path: "/dev/input/event0".into(),
            }
            .confirms_terminal()
        );
        assert!(!CaptureSource::Hyprland.confirms_terminal());
        assert_eq!(CaptureSource::Hyprland.label(), "Hyprland");
    }

    #[test]
    fn hyprland_events_all_includes_modifiers_and_releases() {
        use crate::hyprland_capture::{HyprlandKeyEvent, decode_test_event};

        // LeftCtrl (evdev 29 -> XKB 37) press
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: 37,
            event_type: KeyEventType::Press,
            modifier_mask: 4,
        };
        let observed = decode_test_event(&event, None, 0, false, false);
        assert_eq!(observed.combo.key(), "LEFTCTRL");
        assert_eq!(observed.event_type, KeyEventType::Press);

        // Release event (state = 0)
        let rel_event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: 36,
            event_type: KeyEventType::Release,
            modifier_mask: 0,
        };
        let observed_rel = decode_test_event(&rel_event, None, 0, false, false);
        assert_eq!(observed_rel.combo.key(), "RETURN");
        assert_eq!(observed_rel.event_type, KeyEventType::Release);
    }

    #[test]
    fn stopping_conditions_for_oneshot_and_repeat() {
        let opt_oneshot = Options {
            repeat: false,
            count: None,
            ..Default::default()
        };
        let keep = opt_oneshot.repeat || opt_oneshot.count.is_some_and(|c| c > 1);
        assert!(!keep, "one-shot must not keep listening");

        let opt_repeat = Options {
            repeat: true,
            count: Some(2),
            ..Default::default()
        };
        let keep_repeat = opt_repeat.repeat || opt_repeat.count.is_some_and(|c| c > 1);
        assert!(keep_repeat);
        let captured_1 = 1;
        assert!(!opt_repeat.count.is_some_and(|c| captured_1 >= c));
        let captured_2 = 2;
        assert!(opt_repeat.count.is_some_and(|c| captured_2 >= c));
    }
}
