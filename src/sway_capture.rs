//! Read-only Sway IPC capture.
//!
//! Sway exposes binding notifications, but unlike Hyprland it does not
//! expose a suppression hook through IPC. The backend therefore supports
//! pass-through observation only and fails closed when suppression is asked
//! for. This keeps `listen` honest while still making Sway a real registry
//! backend rather than a special case.

use std::env;
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
    env::var_os("SWAYSOCK")
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
        let path = env::var_os("SWAYSOCK").ok_or_else(|| "SWAYSOCK is not set".to_owned())?;
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
            let binding = value.get("binding").unwrap_or(&value);
            let symbol = binding
                .get("symbol")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    binding
                        .get("symbols")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|values| values.first())
                        .and_then(serde_json::Value::as_str)
                })
                .ok_or_else(|| "Sway binding event omitted a key symbol".to_owned())?;
            let mask = binding
                .get("event_state_mask")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let combo = KeyCombo::from_parts(mask, symbol);
            if !events_all && combo.modmask() == 0 && combo.key().is_empty() {
                continue;
            }
            let action = binding
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Sway binding")
                .to_owned();
            return Ok(Some(ObservedKey {
                disposition: CaptureDisposition::PassedThrough,
                combo,
                raw: Vec::new(),
                raw_display: Some(format!("Sway binding event: {action}")),
                modifier_state: None,
                associated_text: None,
                physical_keycode: binding
                    .get("input_code")
                    .and_then(serde_json::Value::as_u64)
                    .map(|code| crate::xkb::EvdevKeycode::new(code as u16)),
                encoding: "Sway IPC binding event".into(),
                protocol_flags: None,
                event_type: KeyEventType::Press,
                alternate_keys: None,
                alternate_key: Some(action),
                source: CaptureSource::CompositorNative {
                    backend: NativeBackendId::Sway.display().into(),
                },
            }));
        }
    }
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
            br#"{"binding":{"symbol":"Return","event_state_mask":64,"command":"exec foot"}}"#;
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
}
