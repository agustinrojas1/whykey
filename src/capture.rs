//! Capture backend contracts shared by native compositor integrations.

use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

use crate::listen::ObservedKey;
use crate::xkb::XkbKeycode;

/// Whether a native backend should consume the captured shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePolicy {
    #[default]
    Suppress,
    PassThrough,
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

/// Result of waiting for all keys in a captured chord to be released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordReleaseStatus {
    Released,
    TimedOut,
}

/// Operations the listen loop needs from a native compositor capture backend.
pub trait CaptureBackend {
    fn id(&self) -> NativeBackendId;
    fn display(&self) -> &'static str;
    fn arm(&mut self, policy: CapturePolicy) -> Result<(), String>;
    fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String>;
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
    }

    #[test]
    fn backend_id_display_is_stable() {
        assert_eq!(NativeBackendId::Hyprland.to_string(), "Hyprland");
        assert_eq!(NativeBackendId::Hyprland.display(), "Hyprland");
    }
}
