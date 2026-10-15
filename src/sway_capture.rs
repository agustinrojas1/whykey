//! Read-only Sway IPC capture.
//!
//! Sway exposes binding notifications, but unlike Hyprland it does not
//! expose a suppression hook through IPC. The backend therefore supports
//! pass-through observation only and fails closed when suppression is asked
//! for. This keeps `listen` honest while still making Sway a real registry
//! backend rather than a special case.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::capture::{
    CaptureBackend, CaptureBackendId, CapturePolicy, CaptureToken, ChordReleaseStatus,
    NativeBackendId, NativeCaptureIo, NativeCaptureTransport,
};
use crate::key::KeyCombo;
use crate::listen::{CaptureDisposition, CaptureSource, KeyEventType, ObservedKey};
use crate::xkb::XkbKeycode;

const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const IPC_SUBSCRIBE: u32 = 2;
const IPC_EVENT_BINDING: u32 = 0x8000_0005;

pub fn capture_connect() -> Result<Box<dyn NativeCaptureIo>, String> {
    Ok(Box::new(SwayCaptureSession::connect()?))
}

pub fn probe_available() -> bool {
    crate::recording::env_var_os("SWAYSOCK")
        .filter(|value| !value.is_empty())
        .and_then(|path| UnixStream::connect(path).ok())
        .is_some()
}

pub struct SwayCaptureSession {
    stream: UnixStream,
    buffer: Vec<u8>,
    policy: CapturePolicy,
    closed: bool,
}

impl SwayCaptureSession {
    fn connect() -> Result<Self, String> {
        let path = crate::recording::env_var_os("SWAYSOCK")
            .ok_or_else(|| "SWAYSOCK is not set".to_owned())?;
        let stream = UnixStream::connect(path).map_err(|error| format!("Sway IPC: {error}"))?;
        stream
            .set_nonblocking(true)
            .map_err(|error| format!("Sway IPC nonblocking setup failed: {error}"))?;
        let mut session = Self {
            stream,
            buffer: Vec::new(),
            policy: CapturePolicy::PassThrough,
            closed: false,
        };
        session.send_subscribe()?;
        Ok(session)
    }

    fn send_subscribe(&mut self) -> Result<(), String> {
        let payload = br#"["binding"]"#;
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.extend_from_slice(IPC_MAGIC);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&IPC_SUBSCRIBE.to_le_bytes());
        frame.extend_from_slice(payload);
        self.stream
            .write_all(&frame)
            .map_err(|error| format!("Sway IPC subscribe failed: {error}"))
    }

    fn parse_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        loop {
            if self.buffer.len() < 14 {
                return Ok(None);
            }
            if &self.buffer[..6] != IPC_MAGIC {
                return Err("Sway IPC returned an invalid frame header".into());
            }
            let length = u32::from_le_bytes(self.buffer[6..10].try_into().unwrap()) as usize;
            let kind = u32::from_le_bytes(self.buffer[10..14].try_into().unwrap());
            if length > 1_000_000 {
                return Err("Sway IPC frame exceeds the 1 MiB safety limit".into());
            }
            if self.buffer.len() < 14 + length {
                return Ok(None);
            }
            let payload = self.buffer[14..14 + length].to_vec();
            self.buffer.drain(..14 + length);
            if kind != IPC_EVENT_BINDING {
                continue;
            }
            let value: serde_json::Value = serde_json::from_slice(&payload)
                .map_err(|error| format!("Sway binding event was invalid JSON: {error}"))?;
            if let Some(observed) = observed_binding(&value, events_all, release_flag_of_config)? {
                return Ok(Some(observed));
            }
        }
    }
}

/// Whether the running config binds this combo with `--release`; see
/// [`crate::layers::tiling::release_flag`].
fn release_flag_of_config(combo: &KeyCombo) -> Option<bool> {
    crate::layers::sway::release_flag(combo)
}

/// Decode one Sway `binding` event. Sway runs a binding's command when the
/// binding matches: a `--release` binding runs on release and a normal one on
/// press, and the event payload carries no flag for that, so `release_flag`
/// asks the running config. `Ok(None)` skips an event that is not reported (a
/// bare modifier when `events_all` is off).
fn observed_binding(
    value: &serde_json::Value,
    events_all: bool,
    release_flag: impl Fn(&KeyCombo) -> Option<bool>,
) -> Result<Option<ObservedKey>, String> {
    let binding = value.get("binding").unwrap_or(value);
    // `input_code` is the XKB keycode of a `bindcode` binding, and 0 for a
    // `bindsym` binding, which names its key by symbol instead.
    let input_code = binding
        .get("input_code")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let symbol = binding
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            binding
                .get("symbols")
                .and_then(serde_json::Value::as_array)
                .and_then(|values| values.first())
                .and_then(serde_json::Value::as_str)
        });
    let key = match symbol {
        Some(symbol) => symbol.to_owned(),
        // A `bindcode` event has `symbol: null`; name the key by its keycode
        // the way Sway's own bindcode matching does (`CODE:n`).
        None if input_code != 0 => format!("CODE:{input_code}"),
        None => return Err("Sway binding event omitted a key symbol".to_owned()),
    };
    let mask = binding
        .get("event_state_mask")
        .and_then(crate::layers::tiling::event_state_mask)
        .unwrap_or(0);
    let combo = KeyCombo::from_parts(mask, &key);
    if !events_all && combo.modmask() == 0 && combo.key().is_empty() {
        return Ok(None);
    }
    let event_type = if release_flag(&combo) == Some(true) {
        KeyEventType::Release
    } else {
        KeyEventType::Press
    };
    let physical_keycode = if input_code == 0 {
        None
    } else {
        u32::try_from(input_code)
            .ok()
            .and_then(|code| XkbKeycode::new(code).to_evdev())
    };
    let action = binding
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Sway binding")
        .to_owned();
    Ok(Some(ObservedKey {
        disposition: CaptureDisposition::PassedThrough,
        combo,
        raw: Vec::new(),
        raw_display: Some(format!("Sway binding event: {action}")),
        modifier_state: None,
        associated_text: None,
        physical_keycode,
        encoding: "Sway IPC binding event".into(),
        protocol_flags: None,
        event_type,
        alternate_keys: None,
        alternate_key: Some(action),
        source: CaptureSource::CompositorNative {
            backend: NativeBackendId::Sway.display().into(),
        },
    }))
}

impl CaptureBackend for SwayCaptureSession {
    fn id(&self) -> CaptureBackendId {
        CaptureBackendId::Native(NativeBackendId::Sway)
    }

    fn display(&self) -> &'static str {
        NativeBackendId::Sway.display()
    }

    fn arm(&mut self, policy: CapturePolicy) -> Result<(), String> {
        if policy == CapturePolicy::Suppress {
            return Err(
                "Sway IPC exposes binding notifications but no suppression hook; use --no-suppress"
                    .into(),
            );
        }
        self.policy = policy;
        Ok(())
    }

    fn arm_token(&mut self, policy: CapturePolicy) -> Result<CaptureToken, String> {
        self.arm(policy)?;
        Ok(CaptureToken::new(self.id(), "sway-binding-events", false))
    }

    fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        self.parse_event(events_all)
    }

    fn wait_for_chord_release(
        &mut self,
        _main: XkbKeycode,
        _timeout: Duration,
    ) -> Result<ChordReleaseStatus, String> {
        Ok(ChordReleaseStatus::Released)
    }

    fn is_suppressing(&self) -> bool {
        false
    }

    fn close(&mut self) -> io::Result<()> {
        self.closed = true;
        self.stream.shutdown(std::net::Shutdown::Both)
    }
}

impl NativeCaptureTransport for SwayCaptureSession {
    fn poll_fd(&self) -> Option<RawFd> {
        Some(self.stream.as_raw_fd())
    }

    fn read_incoming(&mut self) -> io::Result<usize> {
        let mut chunk = [0_u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "socket closed",
            )),
            Ok(size) => {
                self.buffer.extend_from_slice(&chunk[..size]);
                Ok(size)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(error) => Err(error),
        }
    }
}

impl NativeCaptureIo for SwayCaptureSession {
    fn transport(&mut self) -> &mut dyn NativeCaptureTransport {
        self
    }
}

impl Drop for SwayCaptureSession {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_suppression_without_claiming_a_lease() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut session = SwayCaptureSession {
            stream,
            buffer: Vec::new(),
            policy: CapturePolicy::PassThrough,
            closed: false,
        };
        let error = session.arm(CapturePolicy::Suppress).unwrap_err();
        assert!(error.contains("no suppression hook"));
        assert!(!session.is_suppressing());
    }

    #[test]
    fn parses_binding_event_into_native_source() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut session = SwayCaptureSession {
            stream,
            buffer: Vec::new(),
            policy: CapturePolicy::PassThrough,
            closed: false,
        };
        let payload =
            br#"{"binding":{"symbol":"Return","event_state_mask":["Mod4"],"command":"exec foot"}}"#;
        session.buffer.extend_from_slice(IPC_MAGIC);
        session
            .buffer
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        session
            .buffer
            .extend_from_slice(&IPC_EVENT_BINDING.to_le_bytes());
        session.buffer.extend_from_slice(payload);
        let observed = session.next_event(false).unwrap().unwrap();
        assert_eq!(observed.combo.compact_display(), "SUPER+RETURN");
        assert_eq!(
            observed.source,
            CaptureSource::CompositorNative {
                backend: "Sway".into()
            }
        );
        assert_eq!(observed.disposition, CaptureDisposition::PassedThrough);
    }

    fn binding_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = IPC_MAGIC.to_vec();
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&IPC_EVENT_BINDING.to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn binding_event_keeps_modifier_names_from_the_ipc_array() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut session = SwayCaptureSession {
            stream,
            buffer: Vec::new(),
            policy: CapturePolicy::PassThrough,
            closed: false,
        };
        session.buffer.extend(binding_frame(
            br#"{"change":"run","binding":{"command":"move left","event_state_mask":["Shift","Mod4","Group2"],"input_code":0,"symbol":"h","input_type":"keyboard"}}"#,
        ));
        session.buffer.extend(binding_frame(
            br#"{"change":"run","binding":{"command":"kill","event_state_mask":["Ctrl","Mod1","Lock"],"symbol":"q"}}"#,
        ));
        let first = session.next_event(false).unwrap().unwrap();
        assert_eq!(first.combo.compact_display(), "SHIFT+SUPER+H");
        let second = session.next_event(false).unwrap().unwrap();
        assert_eq!(second.combo.compact_display(), "CTRL+ALT+CAPS+Q");
    }

    fn decode(payload: &str, release: Option<bool>) -> Result<Option<ObservedKey>, String> {
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        observed_binding(&value, false, |_| release)
    }

    #[test]
    fn bindcode_event_without_a_symbol_is_named_by_its_keycode() {
        // Sway sends `symbol: null` and `symbols: []` for `bindcode Mod4+36`.
        // Before, this error aborted the whole listen session.
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut session = SwayCaptureSession {
            stream,
            buffer: Vec::new(),
            policy: CapturePolicy::PassThrough,
            closed: false,
        };
        session.buffer.extend(binding_frame(
            br#"{"change":"run","binding":{"command":"exec touch /tmp/code","event_state_mask":["Mod4"],"input_codes":[36],"input_code":36,"symbols":[],"symbol":null,"input_type":"keyboard"}}"#,
        ));
        let observed = session.next_event(false).unwrap().unwrap();
        assert_eq!(observed.combo.key(), "CODE:36");
        assert_eq!(observed.combo.modmask(), 64);
    }

    #[test]
    fn input_code_is_an_xkb_keycode_converted_to_evdev() {
        // Sway's input_code for `bindcode 36` is the XKB keycode 36, which is
        // evdev 28 (KEY_ENTER) plus 8.
        let observed = decode(
            r#"{"binding":{"command":"exec foot","event_state_mask":["Mod4"],"input_code":36,"symbol":"Return"}}"#,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            observed.physical_keycode,
            Some(crate::xkb::EvdevKeycode::new(28))
        );
    }

    #[test]
    fn symbol_binding_has_no_physical_keycode() {
        // `bindsym` events carry input_code 0: no keycode was bound, so none is
        // reported (before, 0 was reported as evdev keycode 0).
        let observed = decode(
            r#"{"binding":{"command":"exec foot","event_state_mask":["Mod4"],"input_code":0,"symbol":"t"}}"#,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(observed.physical_keycode, None);
    }

    #[test]
    fn release_binding_event_is_labelled_as_a_release() {
        // Sway runs a `--release` binding on release, and its event does not say
        // so; the running config supplies the flag.
        let payload = r#"{"binding":{"command":"exec touch /tmp/rel","event_state_mask":["Mod4"],"input_code":0,"symbol":"u"}}"#;
        let release = decode(payload, Some(true)).unwrap().unwrap();
        assert_eq!(release.event_type, KeyEventType::Release);
        let press = decode(payload, Some(false)).unwrap().unwrap();
        assert_eq!(press.event_type, KeyEventType::Press);
        let unknown = decode(payload, None).unwrap().unwrap();
        assert_eq!(unknown.event_type, KeyEventType::Press);
    }
}
