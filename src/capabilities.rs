use std::env;

use serde::Serialize;

use crate::listen;
use crate::schema;

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    /// Owned so dynamically discovered extension capabilities do not require
    /// leaking a string into the process-wide static lifetime.
    pub id: String,
    pub area: &'static str,
    pub implemented: bool,
    pub availability: &'static str,
    pub evidence: String,
}

pub fn current() -> Vec<Capability> {
    // One snapshot per command: desktop discovery runs once here instead
    // of once per adapter call site.
    let environment = crate::environment::Environment::collect();
    let ssh = environment.ssh;
    let tty = environment.tty_available;
    let generic_compositor = environment.desktop("compositor").applicable;
    let evdev_devices = listen::evdev_devices();
    let remappers = environment.remappers.clone();
    let ime = environment.ime.clone();
    let ime_runtime_engine = ime
        .iter()
        .any(|detection| detection.active_engine.is_some());
    let terminal = environment.terminal;
    let shell = env::var("SHELL").ok();
    let shell_name = environment.shell_name.as_str();
    let shell_snapshot = match shell_name {
        "bash" => env::var_os("WHYKEY_READLINE_BINDINGS").is_some(),
        "zsh" => env::var_os("WHYKEY_ZLE_BINDINGS").is_some(),
        "fish" => env::var_os("WHYKEY_FISH_BINDINGS").is_some(),
        _ => false,
    };
    let native_compositor = native_capture_available_for_doctor();
    let native_backend = crate::capture::NativeBackendId::Hyprland.display();
    let native_sway = crate::sway_capture::probe_available();
    // Focus availability comes from the registry: adapters that expose a
    // focused-window PID operation, evaluated on the same Environment
    // snapshot as every other capability entry.
    let focused_available = crate::registry::DESKTOPS.iter().any(|entry| {
        entry.focused_pid.or(entry.focus).is_some() && {
            let status = environment.desktop(entry.id);
            status.applicable && status.ipc
        }
    });
    let focus_ipc_unavailable = crate::registry::DESKTOPS.iter().any(|entry| {
        entry.focused_pid.or(entry.focus).is_some()
            && environment.desktop(entry.id).applicable
            && !focused_available
    });
    let focus_without_pid_api = crate::registry::DESKTOPS.iter().any(|entry| {
        entry.focused_pid.or(entry.focus).is_none() && environment.desktop(entry.id).applicable
    });

    let mut capabilities = vec![
        implemented(
            "cli.inspect.combination",
            "cli",
            "whykey <combination> is available",
        ),
        implemented(
            "cli.inspect.sequence",
            "cli",
            "whykey inspect analyzes up to 64 independent steps",
        ),
        implemented(
            "cli.inspect.pid",
            "cli",
            "whykey inspect --pid selects process ancestry",
        ),
        available(
            "cli.inspect.focused",
            "cli",
            focused_available,
            if focused_available {
                "focused-window PID discovery is available through a compositor tree API"
            } else if focus_ipc_unavailable {
                "a supported compositor was detected, but its focused-window IPC is unavailable"
            } else if focus_without_pid_api || generic_compositor {
                "the detected desktop has no supported focused-window PID API"
            } else {
                "no supported compositor session detected"
            },
        ),
        implemented(
            "cli.bindings",
            "cli",
            "whykey bindings enumerates effective bindings from detected compositor adapters and reports explicit coverage limits",
        ),
        implemented(
            "cli.conflicts",
            "cli",
            "whykey conflicts groups duplicate actions only within the same enumerated source and context, preserving unknown precedence",
        ),
        implemented(
            "cli.replay",
            "cli",
            "whykey replay re-renders saved schema-v1 and schema-v2 reports without injecting input",
        ),
        implemented(
            "cli.snapshot",
            "cli",
            "whykey snapshot saves a versioned static diagnostic report without capturing or injecting input",
        ),
        implemented(
            "cli.diff",
            "cli",
            "whykey diff compares saved snapshots or reports without querying the desktop",
        ),
        implemented(
            "cli.output.schema-v2",
            "cli",
            "--schema-version 2 (or --json-v2) emits structured context and evidence while v1 remains the default",
        ),
        implemented(
            "cli.extensions",
            "cli",
            "whykey extension runs one explicitly selected executable through the versioned stdin/stdout protocol with bounded execution",
        ),
        implemented(
            "cli.extensions-compositor",
            "cli",
            "user-owned compositor manifests provide bounded read-only binding and focused-window queries",
        ),
        available(
            "capture.terminal",
            "capture",
            tty,
            if tty {
                "controlling /dev/tty is readable"
            } else {
                "/dev/tty is unavailable in this process"
            },
        ),
        available(
            "capture.native-compositor",
            "capture",
            native_compositor,
            if native_compositor {
                format!(
                    "Native compositor capture ({native_backend}): compositor IPC connection accepted"
                )
            } else {
                format!(
                    "Native compositor capture ({native_backend}): compositor IPC connection unavailable"
                )
            },
        ),
        available(
            "capture.native-sway",
            "capture",
            native_sway,
            if native_sway {
                "Native compositor capture (Sway): IPC connection accepted; observation is pass-through only"
            } else {
                "Native compositor capture (Sway): SWAYSOCK is unavailable or unreachable"
            },
        ),
        implemented(
            "capture.ndjson",
            "capture",
            "whykey listen --ndjson emits one compact schema-v2 record per captured event",
        ),
        implemented(
            "capture.export",
            "capture",
            "whykey listen --output writes replayable JSON or NDJSON records without mixing diagnostics into the export",
        ),
        available(
            "capture.evdev",
            "capture",
            listen::evdev_available(),
            if evdev_devices.is_empty() {
                "no readable keyboard event device was detected"
            } else {
                "keyboard event devices were enumerated read-only"
            },
        ),
        available(
            "remapper.pre-compositor",
            "input",
            !remappers.is_empty(),
            if remappers.is_empty() {
                "no keyd, kanata, kmonad, input-remapper, or xremap process/configuration was detected"
            } else {
                "a common remapper was detected; transformations remain conditional until observed"
            },
        ),
        available(
            "input.ime-context",
            "input",
            !ime.is_empty(),
            if ime.is_empty() {
                "no IBus or Fcitx5 session variables/processes were detected"
            } else {
                "IBus/Fcitx5 context detected from session variables or processes; active-engine queries are attempted read-only"
            },
        ),
        available(
            "input.ime-runtime-engine",
            "input",
            ime_runtime_engine,
            if ime_runtime_engine {
                "the active IBus/Fcitx5 engine was observed through a read-only API query"
            } else if ime.is_empty() {
                "no IBus or Fcitx5 context was detected"
            } else {
                "an input method was detected, but its active engine could not be queried"
            },
        ),
        implemented(
            "input.ime.fcitx5-safe-query",
            "safety",
            "Fcitx5 runtime inspection uses bounded read-only D-Bus calls and never launches fcitx5-remote",
        ),
    ];
    capabilities.extend(desktop_capability_entries(&environment));
    capabilities.extend(extension_capability_entries(&environment));
    capabilities.extend(vec![
        available(
            "compositor.generic",
            "compositor",
            generic_compositor,
            if generic_compositor {
                "generic desktop/session context detected; global bindings remain opaque"
            } else if ssh {
                "not applicable to this SSH session"
            } else {
                "no generic desktop/session context detected"
            },
        ),
        planned(
            "compositor.additional-desktops",
            "compositor",
            "additional desktop integrations beyond the supported compositor set remain planned; X11 xbindkeys is covered separately",
        ),
        available(
            "terminal.dedicated",
            "terminal",
            terminal.is_some(),
            terminal.map_or("no dedicated terminal identity detected".into(), |name| {
                format!("detected terminal adapter: {name}")
            }),
        ),
        available(
            "shell.runtime-snapshot",
            "shell",
            shell_snapshot,
            if shell_snapshot {
                format!("live {shell_name} binding snapshot supplied by shell-init")
            } else if shell.is_some() {
                format!(
                    "no live {shell_name} snapshot; static/default analysis remains conditional"
                )
            } else {
                "SHELL is not set".into()
            },
        ),
        available(
            "session.multiplexers",
            "session",
            env::var_os("TMUX").is_some()
                || env::var_os("STY").is_some()
                || env::var_os("ZELLIJ").is_some()
                || env::var_os("ZELLIJ_SESSION_NAME").is_some(),
            "tmux, GNU Screen, and Zellij adapters are available when their session markers are present",
        ),
        planned(
            "input.ime",
            "input",
            "active-engine context is available, but committed text, dead-key/Compose history, and application-side preedit state remain unobservable from a passive terminal diagnostic",
        ),
        implemented(
            "safety.read-only",
            "safety",
            "adapters do not edit configuration or execute investigated actions",
        ),
    ]);
    capabilities
}

#[cfg(target_os = "linux")]
pub fn native_capture_available_for_doctor() -> bool {
    crate::hyprland_capture::HyprlandCaptureSession::probe_available()
}

#[cfg(not(target_os = "linux"))]
pub fn native_capture_available_for_doctor() -> bool {
    false
}

/// Human-readable inventory. By default only applicable and available
/// entries are shown; `--all` restores the complete adapter inventory.
/// JSON always keeps full structured evidence.
pub fn render_text(capabilities: &[Capability], all: bool) -> String {
    let mut output = String::from("whykey capabilities\n\n");
    for capability in capabilities
        .iter()
        .filter(|capability| all || capability.availability == "available")
    {
        let marker = if !capability.implemented {
            "-"
        } else if capability.availability == "available" {
            "✓"
        } else {
            "!"
        };
        output.push_str(&format!(
            "{marker} {} [{}; {}]\n  {}\n",
            capability.id, capability.area, capability.availability, capability.evidence
        ));
    }
    output
}

pub fn render_json(capabilities: &[Capability], schema_version: u8) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "capabilities",
            "context": schema::context(),
            "capabilities": capabilities,
        }))
        .expect("capabilities are serializable")
            + "\n";
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "capabilities": capabilities,
    }))
    .expect("capabilities are serializable")
        + "\n"
}

/// Desktop capability entries generated from the registry metadata so the
/// capability list and the adapter registry cannot drift.
fn desktop_capability_entries(environment: &crate::environment::Environment) -> Vec<Capability> {
    crate::registry::DESKTOPS
        .iter()
        .filter_map(|entry| {
            let capability = entry.capability?;
            let status = environment.desktop(entry.id);
            let is_available = status.applicable && status.ipc;
            let evidence = capability.evidence.map_or_else(
                || {
                    crate::registry::compositor_evidence(
                        capability.name,
                        status.applicable,
                        status.ipc,
                    )
                },
                |evidence| evidence(status.applicable, status.ipc),
            );
            Some(available(
                capability.id,
                "compositor",
                is_available,
                evidence,
            ))
        })
        .collect()
}

fn extension_capability_entries(environment: &crate::environment::Environment) -> Vec<Capability> {
    environment
        .extension_adapters
        .iter()
        .map(|adapter| {
            let applicable = crate::extension_adapters::applicable_with_context(
                adapter,
                &environment.compositor_context,
            );
            let id = format!("compositor.extension.{}", adapter.id);
            available(
                id,
                "compositor",
                applicable,
                if applicable {
                    format!(
                        "{} manifest is applicable; bindings remain conditional until its command reports them",
                        adapter.display
                    )
                } else {
                    format!("{} manifest is installed but its environment and desktop hints do not match", adapter.display)
                },
            )
        })
        .collect()
}

fn implemented(
    id: impl Into<String>,
    area: &'static str,
    evidence: impl Into<String>,
) -> Capability {
    Capability {
        id: id.into(),
        area,
        implemented: true,
        availability: "available",
        evidence: evidence.into(),
    }
}

fn planned(id: impl Into<String>, area: &'static str, evidence: impl Into<String>) -> Capability {
    Capability {
        id: id.into(),
        area,
        implemented: false,
        availability: "planned",
        evidence: evidence.into(),
    }
}

fn available(
    id: impl Into<String>,
    area: &'static str,
    is_available: bool,
    evidence: impl Into<String>,
) -> Capability {
    Capability {
        id: id.into(),
        area,
        implemented: true,
        availability: if is_available {
            "available"
        } else {
            "unavailable"
        },
        evidence: evidence.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Deserialize;

    use super::*;

    const SUPPORT_MATRIX: &str = include_str!("../support-matrix.json");
    const GENERATED_SUPPORT_MATRIX: &str = include_str!("../SUPPORT_MATRIX.md");

    #[derive(Debug, Deserialize)]
    struct SupportMatrix {
        schema_version: u8,
        entries: Vec<SupportEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct SupportEntry {
        id: String,
        area: String,
        implemented: bool,
    }

    #[test]
    fn exposes_implemented_and_planned_capabilities_separately() {
        let capabilities = current();
        let sequence = capabilities
            .iter()
            .find(|capability| capability.id == "cli.inspect.sequence")
            .unwrap();
        assert!(sequence.implemented);
        assert_eq!(sequence.availability, "available");
        let fcitx_safety = capabilities
            .iter()
            .find(|capability| capability.id == "input.ime.fcitx5-safe-query")
            .unwrap();
        assert!(fcitx_safety.implemented);
        assert!(
            fcitx_safety
                .evidence
                .contains("never launches fcitx5-remote")
        );
        let ndjson = capabilities
            .iter()
            .find(|capability| capability.id == "capture.ndjson")
            .unwrap();
        assert_eq!(ndjson.availability, "available");
    }

    #[test]
    fn renders_versioned_json() {
        let output = render_json(&current(), 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert!(value["capabilities"].is_array());
    }

    #[test]
    fn capability_ids_are_owned_and_stable_across_calls() {
        let first = current()
            .into_iter()
            .map(|capability| capability.id)
            .collect::<Vec<_>>();
        let second = current()
            .into_iter()
            .map(|capability| capability.id)
            .collect::<Vec<_>>();

        assert_eq!(first, second);
        assert!(first.iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn support_matrix_is_the_registry_contract_and_has_generated_docs() {
        let matrix: SupportMatrix = serde_json::from_str(SUPPORT_MATRIX).unwrap();
        assert_eq!(matrix.schema_version, 1);

        let capabilities = current();
        assert_eq!(matrix.entries.len(), capabilities.len());
        let mut ids = HashSet::new();
        for (entry, capability) in matrix.entries.iter().zip(capabilities.iter()) {
            assert!(ids.insert(&entry.id), "duplicate matrix id {}", entry.id);
            assert_eq!(entry.id, capability.id);
            assert_eq!(entry.area, capability.area);
            assert_eq!(entry.implemented, capability.implemented);
            assert!(
                GENERATED_SUPPORT_MATRIX.contains(&format!("| `{}` |", entry.id)),
                "generated support matrix is missing {}",
                entry.id
            );
        }
    }
}
