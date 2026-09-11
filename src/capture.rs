//! Capture backend contracts shared by native compositor integrations.

use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

use crate::listen::ObservedKey;
use crate::xkb::XkbKeycode;

/// Whether a capture backend should consume the captured shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePolicy {
    #[default]
    Suppress,
    PassThrough,
}

impl CapturePolicy {
    pub const fn suppresses(self) -> bool {
        matches!(self, Self::Suppress)
    }

    pub const fn is_pass_through(self) -> bool {
        matches!(self, Self::PassThrough)
    }
}

impl std::fmt::Display for CapturePolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Suppress => "suppress",
            Self::PassThrough => "pass-through",
        })
    }
}

/// Stable identifiers for native capture implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBackendId {
    Hyprland,
}

impl NativeBackendId {
    pub const fn display(self) -> &'static str {
        match self {
            Self::Hyprland => "Hyprland",
        }
    }
}

impl std::fmt::Display for NativeBackendId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display())
    }
}

/// Stable identifiers for every capture transport exposed to listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackendId {
    Native(NativeBackendId),
    Terminal,
    Evdev,
}

impl CaptureBackendId {
    pub const fn display(self) -> &'static str {
        match self {
            Self::Native(backend) => backend.display(),
            Self::Terminal => "Terminal",
            Self::Evdev => "Evdev",
        }
    }
}

impl std::fmt::Display for CaptureBackendId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display())
    }
}

/// Identity returned when a transport is armed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureToken {
    pub backend: CaptureBackendId,
    pub token: String,
    pub suppressing: bool,
}

impl CaptureToken {
    pub fn new(backend: CaptureBackendId, token: impl Into<String>, suppressing: bool) -> Self {
        Self {
            backend,
            token: token.into(),
            suppressing,
        }
    }
}

/// Result of waiting for all keys in a captured chord to be released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordReleaseStatus {
    Released,
    TimedOut,
}

/// Operations the listen loop needs from a native compositor capture backend.
pub trait CaptureBackend {
    fn id(&self) -> CaptureBackendId;
    fn display(&self) -> &'static str;
    fn arm(&mut self, policy: CapturePolicy) -> Result<(), String>;
    fn arm_token(&mut self, policy: CapturePolicy) -> Result<CaptureToken, String> {
        self.arm(policy)?;
        Ok(CaptureToken::new(
            self.id(),
            "legacy",
            self.is_suppressing(),
        ))
    }
    fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String>;
    fn next_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        self.next_observed_event(events_all)
    }
    fn wait_for_chord_release(
        &mut self,
        main: XkbKeycode,
        timeout: Duration,
    ) -> Result<ChordReleaseStatus, String>;
    fn is_suppressing(&self) -> bool;
    fn close(&mut self) -> io::Result<()>;

    /// Optional backend-specific warning shown before arming.
    fn pre_arm_warning(&self, _policy: CapturePolicy) -> Option<String> {
        None
    }
}

/// Transport operations kept separate from the backend's capture contract.
/// The listen loop uses these to poll an event source without making the
/// public backend trait know about Unix file descriptors.
pub trait NativeCaptureIo: CaptureBackend {
    fn socket_fd(&self) -> RawFd;
    fn read_incoming(&mut self) -> io::Result<usize>;
    fn renew_lease(&self) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_default_is_suppress() {
        assert_eq!(CapturePolicy::default(), CapturePolicy::Suppress);
        assert!(CapturePolicy::Suppress.suppresses());
        assert!(!CapturePolicy::Suppress.is_pass_through());
        assert!(CapturePolicy::PassThrough.is_pass_through());
        assert_eq!(CapturePolicy::Suppress.to_string(), "suppress");
    }

    #[test]
    fn backend_id_display_is_stable() {
        assert_eq!(NativeBackendId::Hyprland.to_string(), "Hyprland");
        assert_eq!(NativeBackendId::Hyprland.display(), "Hyprland");
        assert_eq!(
            CaptureBackendId::Native(NativeBackendId::Hyprland).display(),
            "Hyprland"
        );
        assert_eq!(CaptureBackendId::Terminal.to_string(), "Terminal");
        assert_eq!(CaptureBackendId::Evdev.to_string(), "Evdev");
    }
}
