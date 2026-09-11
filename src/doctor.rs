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

pub fn evdev(has_device: bool, readable: bool) -> Cause {
    match (has_device, readable) {
        (true, true) => Cause::Available,
        (true, false) => Cause::PermissionMissing,
        (false, _) => Cause::NotInstalled,
    }
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
        assert_eq!(evdev(true, false), Cause::PermissionMissing);
        assert_eq!(api_unavailable(), Cause::ApiDoesNotExpose);
    }
}
