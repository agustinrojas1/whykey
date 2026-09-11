//! Capture one key event from a native compositor, Linux evdev, or the
//! controlling terminal, and explain it.
//!
//! When native compositor capture is available, the default listener uses a
//! temporary runtime hook to suppress bound compositor actions during
//! inspection (or passes them through when configured). Pass-through-only
//! native backends do not change compositor state. If native capture is
//! unavailable, it falls back to terminal capture
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

use crate::capture::{CaptureBackend, CaptureBackendId, NativeCaptureIo};
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};
use crate::report;

const SEQUENCE_TIMEOUT_MS: i32 = 12;
/// Briefly wait for a legacy Escape sequence to continue. Kitty mode encodes
/// Escape unambiguously, so normal interactive cancellation is immediate.
const LEGACY_ESCAPE_SEQUENCE_TIMEOUT_MS: i32 = 30;
const SIGNAL_POLL_MS: i32 = 100;
const MAX_TERMINAL_SEQUENCE_BYTES: usize = 4096;
/// Ask for every currently specified Kitty keyboard enhancement. The original
/// flags are kept separately in `TerminalSession::protocol_flags` so the
/// downstream byte prediction still describes the terminal before capture.
const KITTY_CAPTURE_FLAGS: u32 = 1 | 2 | 4 | 8 | 16;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
pub use crate::capture::CapturePolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDisposition {
    Suppressed,
    PassedThrough,
    ObservedOnly,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub capture_policy: CapturePolicy,
    pub explicit_suppress: bool,
    pub dry_run: bool,
    pub explain_capture: bool,
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
    pub physical_keycode: Option<crate::xkb::EvdevKeycode>,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CaptureSource {
    Terminal,
    CompositorNative { backend: String },
    Evdev { device: String, path: String },
}

impl Serialize for CaptureSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Terminal => serializer.serialize_str("Terminal"),
            Self::CompositorNative { backend } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("kind", "compositor-native")?;
                map.serialize_entry("backend", backend)?;
                map.end()
            }
            Self::Evdev { device, path } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    "Evdev",
                    &serde_json::json!({ "device": device, "path": path }),
                )?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CaptureSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(source) => match source.as_str() {
                "Terminal" => Ok(Self::Terminal),
                "Hyprland" => Ok(Self::CompositorNative {
                    backend: "Hyprland".into(),
                }),
                other => Err(serde::de::Error::custom(format!(
                    "unknown capture source {other}"
                ))),
            },
            serde_json::Value::Object(mut object) => {
                if let Some(native) = object.remove("CompositorNative") {
                    let backend = native
                        .get("backend")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| serde::de::Error::custom("native source lacks backend"))?;
                    return Ok(Self::CompositorNative {
                        backend: backend.to_owned(),
                    });
                }
                if object.get("kind").and_then(serde_json::Value::as_str)
                    == Some("compositor-native")
                {
                    let backend = object
                        .get("backend")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| serde::de::Error::custom("native source lacks backend"))?;
                    return Ok(Self::CompositorNative {
                        backend: backend.to_owned(),
                    });
                }
                if let Some(evdev) = object.remove("Evdev") {
                    return serde_json::from_value::<EvdevSource>(evdev)
                        .map(|value| Self::Evdev {
                            device: value.device,
                            path: value.path,
                        })
                        .map_err(serde::de::Error::custom);
                }
                Err(serde::de::Error::custom("unknown capture source object"))
            }
            _ => Err(serde::de::Error::custom(
                "capture source must be a string or object",
            )),
        }
    }
}

#[derive(Deserialize)]
struct EvdevSource {
    device: String,
    path: String,
}

impl CaptureSource {
    pub fn label(&self) -> String {
        match self {
            Self::Terminal => "terminal".into(),
            Self::CompositorNative { backend: _ } => {
                self.backend_name().unwrap_or("compositor").to_owned()
            }
            Self::Evdev { device, path } => format!("evdev ({device}; {path})"),
        }
    }

    pub fn backend_name(&self) -> Option<&str> {
        match self {
            Self::CompositorNative { backend } => Some(backend),
            Self::Terminal | Self::Evdev { .. } => None,
        }
    }

    pub fn confirms_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }

    pub fn proves_compositor_receipt(&self) -> bool {
        matches!(self, Self::CompositorNative { .. })
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
    if options.dry_run || options.explain_capture {
        let discovery = ListenSession::capture();
        if options.explain_capture {
            print_capture_explanation(&discovery.environment, options.capture_policy);
        }
        if options.dry_run {
            print_capture_dry_run(&discovery.environment, &options);
            return Ok(());
        }
    }
    if options.evdev || options.device.is_some() {
        return run_evdev(options);
    }
    if options.terminal {
        return run_terminal(options);
    }
    let discovery = ListenSession::capture();
    #[cfg(target_os = "linux")]
    match select_native_backend(&discovery.environment) {
        Ok(session) => run_native(options, session, discovery),
        Err(error) => {
            if options.events_all {
                return Err(ListenError::Message(format!(
                    "Native compositor capture unavailable: {error}; --events all requires native compositor or evdev capture"
                )));
            }
            if options.capture_policy == CapturePolicy::Suppress
                && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
            {
                let backend = error.backend_display();
                eprintln!(
                    "whykey listen: could not suppress native compositor shortcuts ({backend})"
                );
                eprintln!("No key was captured and no shortcut was executed.");
                eprintln!("Use --pass-through to capture without suppression.");
                return Err(ListenError::Setup(format!(
                    "native compositor capture unavailable: {error}"
                )));
            }
            eprintln!("Native compositor capture is unavailable: {error}");
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
        eprintln!("Native compositor capture is unavailable: only supported on Linux");
        eprintln!("Using terminal capture; shortcuts consumed by the compositor will not appear.");
        run_terminal(options)
    }
}

#[cfg(target_os = "linux")]
include!("output.rs");
include!("evdev.rs");
include!("terminal.rs");
include!("protocol.rs");
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
        let fixture = include_str!("../../tests/fixtures/remappers/compose-e-acute.txt");
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

    #[test]
    fn kitty_negotiation_matrix_rejects_malformed_or_contradictory_responses() {
        let cases = [
            ("empty", b"".as_slice(), None),
            ("valid zero", b"\x1b[?0u".as_slice(), Some(0)),
            ("valid flags", b"\x1b[?31u".as_slice(), Some(31)),
            ("truncated escape", b"\x1b[?31".as_slice(), None),
            ("wrong final byte", b"\x1b[?31x".as_slice(), None),
            ("missing question marker", b"\x1b[31u".as_slice(), None),
            ("contradictory CSI", b"\x1b[1;5D".as_slice(), None),
            ("negative flags", b"\x1b[?-1u".as_slice(), None),
            ("overflow flags", b"\x1b[?4294967296u".as_slice(), None),
            ("non-UTF8 flags", b"\x1b[?\xffu".as_slice(), None),
        ];
        for (label, response, expected) in cases {
            assert_eq!(parse_protocol_response(response), expected, "{label}");
        }
    }

    #[test]
    fn kitty_event_matrix_preserves_lifecycle_for_press_repeat_release() {
        let cases = [
            (b"\x1b[97;1:1u".as_slice(), Some(KeyEventType::Press)),
            (b"\x1b[97;1:2u".as_slice(), Some(KeyEventType::Repeat)),
            (b"\x1b[97;1:3u".as_slice(), Some(KeyEventType::Release)),
            (b"\x1b[97;1:4u".as_slice(), None),
            (b"\x1b[97;1:bogus u".as_slice(), None),
        ];
        for (bytes, expected) in cases {
            assert_eq!(kitty_event_type(bytes), expected);
        }
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
        let letter = include_str!("../../tests/fixtures/evdev/keyboard-letter.hex");
        let space = include_str!("../../tests/fixtures/evdev/keyboard-space.hex");
        let non_keyboard = include_str!("../../tests/fixtures/evdev/non-keyboard.hex");
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
        assert_eq!(session.key_name(crate::xkb::EvdevKeycode::from(16)), "b");
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
        let native = CaptureSource::CompositorNative {
            backend: "Hyprland".into(),
        };
        assert_eq!(native.label(), "Hyprland");
        assert_eq!(native.backend_name(), Some("Hyprland"));
        assert!(native.proves_compositor_receipt());
        let encoded = serde_json::to_value(&native).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"kind": "compositor-native", "backend": "Hyprland"})
        );
        assert_eq!(
            serde_json::from_value::<CaptureSource>(encoded).unwrap(),
            CaptureSource::CompositorNative {
                backend: "Hyprland".into()
            }
        );
    }

    #[test]
    fn terminal_backend_is_never_suppressing() {
        let backend = TerminalBackend {
            session: None,
            pending: VecDeque::new(),
        };
        assert_eq!(backend.id(), CaptureBackendId::Terminal);
        assert!(!backend.is_suppressing());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_backend_reports_physical_backend_identity() {
        let backend = EvdevBackend {
            session: None,
            requested: None,
        };
        assert_eq!(backend.id(), CaptureBackendId::Evdev);
        assert!(!backend.is_suppressing());
    }

    #[test]
    fn native_source_reader_accepts_structured_shape() {
        let source = serde_json::from_value::<CaptureSource>(serde_json::json!({
            "kind": "compositor-native",
            "backend": "Sway"
        }))
        .unwrap();
        assert_eq!(
            source,
            CaptureSource::CompositorNative {
                backend: "Sway".into()
            }
        );
    }

    #[test]
    fn non_hyprland_native_round_trips_through_canonical_shape() {
        let source = CaptureSource::CompositorNative {
            backend: "Sway".into(),
        };
        let encoded = serde_json::to_value(&source).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"kind": "compositor-native", "backend": "Sway"})
        );
        assert_eq!(
            serde_json::from_value::<CaptureSource>(encoded).unwrap(),
            source
        );
    }

    #[test]
    fn capture_fixtures_use_accepted_source_shapes() {
        let fixtures = [
            (
                include_str!("../../tests/fixtures/differential/capture-terminal-stub.json"),
                CaptureSource::Terminal,
            ),
            (
                include_str!("../../tests/fixtures/differential/capture-hyprland-stub.json"),
                CaptureSource::CompositorNative {
                    backend: "Hyprland".into(),
                },
            ),
        ];
        for (fixture, expected_source) in fixtures {
            let document: serde_json::Value = serde_json::from_str(fixture).unwrap();
            let source: CaptureSource =
                serde_json::from_value(document["observation"]["source"].clone()).unwrap();
            assert_eq!(source, expected_source);
            assert!(
                crate::replay::render_text(std::slice::from_ref(&document))
                    .contains("whykey replay")
            );
            assert!(crate::diff::compare(&document, &document).is_empty());
        }
    }

    #[test]
    fn hyprland_events_all_includes_modifiers_and_releases() {
        use crate::hyprland_capture::{HyprlandKeyEvent, decode_test_event};

        // LeftCtrl (evdev 29 -> XKB 37) press
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(37),
            event_type: KeyEventType::Press,
            modifier_mask: 4,
        };
        let observed = decode_test_event(&event, None, 0, false, false);
        assert_eq!(observed.combo.key(), "LEFTCTRL");
        assert_eq!(observed.event_type, KeyEventType::Press);

        // Release event (state = 0)
        let rel_event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(36),
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
        assert!(opt_repeat.count.is_none_or(|c| captured_1 < c));
        let captured_2 = 2;
        assert!(opt_repeat.count.is_some_and(|c| captured_2 >= c));
    }
}
