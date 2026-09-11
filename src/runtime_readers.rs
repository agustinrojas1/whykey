//! Read-only runtime capability probes for desktop shortcut APIs.
//!
//! These probes introspect a session service only. They never register a
//! shortcut, start a daemon, evaluate shell code, or mutate compositor state.

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

struct Spec {
    id: &'static str,
    service: &'static str,
    object: &'static str,
    interface: &'static str,
    api_is_inventory: bool,
}

const SPECS: &[Spec] = &[
    Spec {
        id: "portal-globalshortcuts",
        service: "org.freedesktop.portal.Desktop",
        object: "/org/freedesktop/portal/desktop",
        interface: "org.freedesktop.portal.GlobalShortcuts",
        api_is_inventory: false,
    },
    Spec {
        id: "kglobalaccel-runtime",
        service: "org.kde.kglobalaccel",
        object: "/kglobalaccel",
        interface: "org.kde.KGlobalAccel",
        api_is_inventory: true,
    },
    Spec {
        id: "gnome-shell-runtime",
        service: "org.gnome.Shell",
        object: "/org/gnome/Shell",
        interface: "org.gnome.Shell",
        api_is_inventory: false,
    },
];

pub fn current() -> Vec<ReaderStatus> {
    SPECS.iter().map(probe).collect()
}

pub fn status(id: &str) -> ReaderStatus {
    SPECS
        .iter()
        .find(|spec| spec.id == id)
        .map(probe)
        .unwrap_or_else(|| ReaderStatus {
            id: "unknown",
            service: "unknown",
            interface: "unknown",
            availability: Availability::Unavailable,
            evidence: "runtime reader is not registered".into(),
        })
}

fn probe(spec: &Spec) -> ReaderStatus {
    let (availability, evidence) = match introspect(spec) {
        Ok(output) => parse_introspection(&output, spec.interface, spec.api_is_inventory),
        Err(error) => (
            Availability::Unavailable,
            format!("live D-Bus unavailable: {error}"),
        ),
    };
    ReaderStatus {
        id: spec.id,
        service: spec.service,
        interface: spec.interface,
        availability,
        evidence,
    }
}

fn introspect(spec: &Spec) -> Result<String, String> {
    let mut command = Command::new("gdbus");
    command.args([
        "introspect",
        "--session",
        "--dest",
        spec.service,
        "--object-path",
        spec.object,
    ]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gdbus returned invalid UTF-8: {error}"))
}

fn parse_introspection(
    output: &str,
    interface: &str,
    api_is_inventory: bool,
) -> (Availability, String) {
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
    if api_is_inventory {
        (
            Availability::Available,
            format!(
                "live D-Bus introspection exposed {interface}; runtime registrations remain conditional"
            ),
        )
    } else {
        (
            Availability::ApiDoesNotExpose,
            format!(
                "{interface} is reachable, but it exposes registration/session APIs rather than a read-only binding inventory"
            ),
        )
    }
}

pub fn evidence_line(id: &str) -> String {
    let result = status(id);
    format!(
        "runtime {}: {} ({})",
        result.id,
        result.evidence,
        result.availability.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_kglobalaccel_fixture() {
        let (availability, evidence) = parse_introspection(
            "node /org/kde/KGlobalAccel {\n  interface org.kde.KGlobalAccel {\n    method allComponents();\n  };\n};",
            "org.kde.KGlobalAccel",
            true,
        );
        assert_eq!(availability, Availability::Available);
        assert!(evidence.contains("live D-Bus"));
    }

    #[test]
    fn distinguishes_absent_and_malformed_runtime_responses() {
        let (availability, _) = parse_introspection("node {}", "org.gnome.Shell", false);
        assert_eq!(availability, Availability::MalformedResponse);
        let status = ReaderStatus {
            id: "fixture",
            service: "fixture",
            interface: "fixture",
            availability: Availability::Unavailable,
            evidence: "service absent".into(),
        };
        assert!(!status.availability.available());
    }

    #[test]
    fn portal_and_shell_are_explicitly_not_inventory_apis() {
        for interface in ["org.freedesktop.portal.GlobalShortcuts", "org.gnome.Shell"] {
            let (availability, _) = parse_introspection(
                &format!("interface {interface} {{ method Register(); }}"),
                interface,
                false,
            );
            assert_eq!(availability, Availability::ApiDoesNotExpose);
        }
    }
}
