//! Read-only runtime probes for desktop shortcut services.
//!
//! These probes query a running session service only. They never register a
//! shortcut, start or D-Bus-activate a daemon, evaluate shell code, or
//! mutate compositor state.

use std::process::Command;

use serde::Serialize;

use crate::command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Unavailable,
    MalformedResponse,
    ApiDoesNotExpose,
}

impl Availability {
    pub fn available(self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::MalformedResponse => "malformed-response",
            Self::ApiDoesNotExpose => "api-does-not-expose",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReaderStatus {
    pub id: &'static str,
    pub service: &'static str,
    pub interface: &'static str,
    pub availability: Availability,
    pub evidence: String,
}

const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_OBJECT: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

pub fn current() -> Vec<ReaderStatus> {
    vec![portal(), kglobalaccel(), gnome_gsettings()]
}

/// xdg-desktop-portal GlobalShortcuts lets an application register its own
/// shortcuts; it has no call that lists other applications' bindings.
fn portal() -> ReaderStatus {
    let (availability, evidence) = match name_has_owner(PORTAL_SERVICE) {
        Ok(true) => match introspect(PORTAL_SERVICE, PORTAL_OBJECT) {
            Ok(output) => parse_introspection(&output, PORTAL_INTERFACE),
            Err(error) => (
                Availability::Unavailable,
                format!("live D-Bus unavailable: {error}"),
            ),
        },
        Ok(false) => (
            Availability::Unavailable,
            format!("{PORTAL_SERVICE} is not running; whykey does not D-Bus-activate it"),
        ),
        Err(error) => (
            Availability::Unavailable,
            format!("live D-Bus unavailable: {error}"),
        ),
    };
    ReaderStatus {
        id: "portal-globalshortcuts",
        service: PORTAL_SERVICE,
        interface: PORTAL_INTERFACE,
        availability,
        evidence,
    }
}

fn kglobalaccel() -> ReaderStatus {
    let (availability, evidence) = match crate::layers::kde::runtime_reachable() {
        Ok(components) => (
            Availability::Available,
            format!(
                "kglobalaccel answered allComponents ({components} components); shortcuts are matched with globalShortcutsByKey and isActive"
            ),
        ),
        Err(error) => (
            Availability::Unavailable,
            format!("kglobalaccel D-Bus unavailable: {error}"),
        ),
    };
    ReaderStatus {
        id: "kglobalaccel-runtime",
        service: "org.kde.kglobalaccel",
        interface: "org.kde.KGlobalAccel",
        availability,
        evidence,
    }
}

/// mutter, GNOME Shell and gnome-settings-daemon rebind from GSettings as
/// soon as a value changes, so the live GSettings values are the runtime
/// keybindings.
fn gnome_gsettings() -> ReaderStatus {
    let (availability, evidence) = match crate::layers::gnome::runtime_reachable() {
        Ok(()) => (
            Availability::Available,
            "gsettings list-recursively reads the session's keybinding schemas".to_owned(),
        ),
        Err(error) => (Availability::Unavailable, error),
    };
    ReaderStatus {
        id: "gnome-gsettings",
        service: "GSettings (dconf)",
        interface: "org.gnome.desktop.wm.keybindings",
        availability,
        evidence,
    }
}

/// `org.freedesktop.DBus.NameHasOwner`: asking the bus never activates the
/// service, unlike calling the service itself.
fn name_has_owner(service: &str) -> Result<bool, String> {
    let mut command = Command::new("gdbus");
    command.args([
        "call",
        "--session",
        "--dest",
        "org.freedesktop.DBus",
        "--object-path",
        "/org/freedesktop/DBus",
        "--method",
        "org.freedesktop.DBus.NameHasOwner",
        service,
    ]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "(true,)" => Ok(true),
        "(false,)" => Ok(false),
        other => Err(format!("unexpected NameHasOwner reply: {other}")),
    }
}

fn introspect(service: &str, object: &str) -> Result<String, String> {
    let mut command = Command::new("gdbus");
    command.args([
        "introspect",
        "--session",
        "--dest",
        service,
        "--object-path",
        object,
    ]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gdbus returned invalid UTF-8: {error}"))
}

fn parse_introspection(output: &str, interface: &str) -> (Availability, String) {
    if output.trim().is_empty() {
        return (
            Availability::MalformedResponse,
            "live introspection returned an empty response".into(),
        );
    }
    if !output.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("interface ") && line.contains(interface)
    }) {
        return (
            Availability::MalformedResponse,
            format!("live introspection did not expose {interface}"),
        );
    }
    (
        Availability::ApiDoesNotExpose,
        format!(
            "{interface} is reachable, but it exposes registration/session APIs rather than a read-only binding inventory"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_is_explicitly_not_an_inventory_api() {
        let (availability, evidence) = parse_introspection(
            include_str!("../tests/fixtures/runtime_readers/portal-globalshortcuts.txt"),
            PORTAL_INTERFACE,
        );
        assert_eq!(availability, Availability::ApiDoesNotExpose);
        assert!(evidence.contains(PORTAL_INTERFACE));
    }

    #[test]
    fn distinguishes_malformed_runtime_responses() {
        let (availability, _) = parse_introspection(
            include_str!("../tests/fixtures/runtime_readers/malformed.txt"),
            PORTAL_INTERFACE,
        );
        assert_eq!(availability, Availability::MalformedResponse);
        assert_eq!(
            parse_introspection("", PORTAL_INTERFACE).0,
            Availability::MalformedResponse
        );
    }

    #[test]
    fn readers_have_stable_ids() {
        let ids = current().iter().map(|reader| reader.id).collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "portal-globalshortcuts",
                "kglobalaccel-runtime",
                "gnome-gsettings"
            ]
        );
    }
}
