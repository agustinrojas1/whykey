//! Shared, read-only doctor cause taxonomy.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    Available,
    NotInstalled,
    PermissionMissing,
    IpcUnreachable,
    ApiDoesNotExpose,
}

impl Cause {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NotInstalled => "not-installed",
            Self::PermissionMissing => "permission-missing",
            Self::IpcUnreachable => "ipc-unreachable",
            Self::ApiDoesNotExpose => "api-does-not-expose",
        }
    }

    pub const fn next_check(self) -> &'static str {
        match self {
            Self::Available => "no further check is needed",
            Self::NotInstalled => "install the integration command or compositor package",
            Self::PermissionMissing => {
                "check device/socket permissions without escalating automatically"
            }
            Self::IpcUnreachable => "check the compositor socket and session environment",
            Self::ApiDoesNotExpose => "use a static configuration source or a supported extension",
        }
    }
}

pub fn ipc(applicable: bool, reachable: bool) -> Cause {
    match (applicable, reachable) {
        (false, _) => Cause::NotInstalled,
        (true, true) => Cause::Available,
        (true, false) => Cause::IpcUnreachable,
    }
}

/// What a read-only look at `/dev/input` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvdevScan {
    /// `/dev/input` is missing: this kernel or container exposes no evdev.
    NoInputDirectory,
    /// `/dev/input` exists but cannot be listed.
    InputDirectoryDenied,
    /// `/dev/input` lists no keyboard event devices.
    NoKeyboardDevices,
    /// Keyboard event devices exist; `readable` is whether one can be opened.
    KeyboardDevices { readable: bool },
}

impl EvdevScan {
    /// Classify `input_dir` once the keyboard device scan is known. The
    /// directory is only listed when no keyboard device was found, to tell an
    /// empty directory from a missing or unreadable one.
    pub fn classify(input_dir: &std::path::Path, keyboard_devices: bool, readable: bool) -> Self {
        if keyboard_devices {
            return Self::KeyboardDevices { readable };
        }
        match std::fs::read_dir(input_dir) {
            Ok(_) => Self::NoKeyboardDevices,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Self::InputDirectoryDenied
            }
            Err(_) => Self::NoInputDirectory,
        }
    }

    pub const fn cause(self) -> Cause {
        match self {
            Self::KeyboardDevices { readable: true } => Cause::Available,
            Self::KeyboardDevices { readable: false } | Self::InputDirectoryDenied => {
                Cause::PermissionMissing
            }
            Self::NoInputDirectory => Cause::NotInstalled,
            // The input subsystem is present but exposes no keyboard to read.
            Self::NoKeyboardDevices => Cause::ApiDoesNotExpose,
        }
    }

    /// What was found, in words precise enough to act on.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::KeyboardDevices { readable: true } => {
                "a keyboard event device can be opened read-only"
            }
            Self::KeyboardDevices { readable: false } => {
                "keyboard event devices exist under /dev/input, but none can be opened read-only"
            }
            Self::InputDirectoryDenied => "/dev/input exists but cannot be listed",
            Self::NoInputDirectory => "/dev/input does not exist, so no evdev devices are exposed",
            Self::NoKeyboardDevices => "/dev/input lists no keyboard event devices",
        }
    }

    /// The step that would make `--evdev` work, or `None` when it already can.
    /// For unreadable keyboards, [`evdev_acl_command`] names the exact nodes.
    pub const fn next_step(self) -> Option<&'static str> {
        match self {
            Self::KeyboardDevices { readable: true } => None,
            Self::KeyboardDevices { readable: false } => Some(
                "grant your user temporary read access to the keyboard event devices with `sudo setfacl -m u:$USER:r /dev/input/eventN`; joining the input group instead grants permanent access to every input device",
            ),
            Self::InputDirectoryDenied => {
                Some("grant read access to /dev/input and its event devices before using --evdev")
            }
            Self::NoInputDirectory | Self::NoKeyboardDevices => Some(
                "no keyboard is visible to evdev here; use native compositor or terminal capture instead of --evdev",
            ),
        }
    }
}

/// A least-privilege, temporary grant for the keyboards whykey found but
/// cannot open: a read ACL for the current user on exactly those nodes.
/// Device nodes live on devtmpfs, so the ACL disappears when the device is
/// replugged or the system reboots. `None` when no keyboard is unreadable.
pub fn evdev_acl_command(devices: &[crate::listen::EvdevDeviceInfo]) -> Option<String> {
    let nodes = devices
        .iter()
        .filter(|device| device.keyboard == Some(true) && !device.readable)
        .map(|device| format!("'{}'", device.path.replace('\'', r"'\''")))
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return None;
    }
    let nodes = nodes.join(" ");
    Some(format!(
        "for temporary read access to the detected keyboards run `sudo setfacl -m u:\"$USER\":r {nodes}`; the grant is lost when the device is replugged or the system reboots (undo it sooner with `sudo setfacl -x u:\"$USER\" {nodes}`)"
    ))
}

pub const fn api_unavailable() -> Cause {
    Cause::ApiDoesNotExpose
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cause_taxonomy_has_actionable_next_checks() {
        for cause in [
            Cause::NotInstalled,
            Cause::PermissionMissing,
            Cause::IpcUnreachable,
            Cause::ApiDoesNotExpose,
        ] {
            assert!(!cause.label().is_empty());
            assert!(!cause.next_check().is_empty());
        }
    }

    #[test]
    fn maps_each_integration_state_deterministically() {
        assert_eq!(ipc(false, false), Cause::NotInstalled);
        assert_eq!(ipc(true, false), Cause::IpcUnreachable);
        assert_eq!(ipc(true, true), Cause::Available);
        assert_eq!(
            EvdevScan::KeyboardDevices { readable: false }.cause(),
            Cause::PermissionMissing
        );
        assert_eq!(api_unavailable(), Cause::ApiDoesNotExpose);
    }

    #[test]
    fn missing_keyboards_are_not_reported_as_missing_permissions_or_software() {
        let base = std::env::temp_dir().join(format!("whykey-evdev-scan-{}", std::process::id()));
        let input = base.join("input");
        std::fs::create_dir_all(&input).unwrap();

        let empty = EvdevScan::classify(&input, false, false);
        assert_eq!(empty, EvdevScan::NoKeyboardDevices);
        assert_eq!(empty.cause(), Cause::ApiDoesNotExpose);
        assert!(!empty.next_step().unwrap().contains("grant"));

        let missing = EvdevScan::classify(&base.join("absent"), false, false);
        assert_eq!(missing, EvdevScan::NoInputDirectory);
        assert_eq!(missing.cause(), Cause::NotInstalled);

        assert_eq!(
            EvdevScan::classify(&input, true, false).cause(),
            Cause::PermissionMissing
        );
        assert_eq!(
            EvdevScan::classify(&input, true, true).cause(),
            Cause::Available
        );
        assert_eq!(EvdevScan::classify(&input, true, true).next_step(), None);
        assert_eq!(
            EvdevScan::InputDirectoryDenied.cause(),
            Cause::PermissionMissing
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn acl_command_names_only_unreadable_keyboards_and_is_temporary() {
        let device = |path: &str, keyboard, readable| crate::listen::EvdevDeviceInfo {
            path: path.into(),
            name: "kbd".into(),
            keyboard,
            readable,
            error: None,
        };
        assert_eq!(
            evdev_acl_command(&[device("/dev/input/event3", Some(true), true)]),
            None
        );
        let command = evdev_acl_command(&[
            device("/dev/input/event3", Some(true), false),
            device("/dev/input/event4", None, false),
            device("/dev/input/event5", Some(true), true),
            device("/dev/input/it's", Some(true), false),
        ])
        .unwrap();
        assert!(
            command.contains(
                "`sudo setfacl -m u:\"$USER\":r '/dev/input/event3' '/dev/input/it'\\''s'`"
            )
        );
        assert!(!command.contains("event4") && !command.contains("event5"));
        assert!(command.contains("replugged") && command.contains("reboots"));
    }
}
