//! Single per-command environment snapshot.
//!
//! Adapters reuse this instead of rediscovering the desktop, terminal,
//! shell, and processes independently.

use crate::layers::{compositor, ghostty, hyprland, terminal_app};
use crate::{ime, remapper};

/// Session identity and selected integration points, collected once.
#[derive(Debug, Clone)]
pub struct Environment {
    pub ssh: bool,
    pub tty_available: bool,
    pub shell: String,
    pub shell_name: String,
    pub terminal: Option<&'static str>,
    pub multiplexer: String,
    pub tmux: bool,
    pub screen: bool,
    pub zellij: bool,
    pub term_program: Option<String>,
    pub tmux_pane: Option<String>,
    pub screen_session: Option<String>,
    pub zellij_pane: Option<String>,
    pub zellij_session: Option<String>,
    /// Generic desktop/session context captured with the rest of the
    /// environment. Keeping this here prevents schema and doctor output from
    /// re-reading the session variables after discovery.
    pub compositor_context: compositor::Context,
    /// Read-only pre-compositor and input-method observations captured once
    /// for the command or listener session.
    pub remappers: Vec<remapper::Detection>,
    pub(crate) hyprland_probe: Option<hyprland::Probe>,
    pub ime: Vec<ime::Detection>,
    /// Selected compositor in inspect priority order, if any.
    pub selected_compositor: Option<&'static str>,
    /// Every applicable compositor candidate, scored from the same snapshot.
    pub compositor_candidates: Vec<CompositorCandidate>,
    /// Per-desktop (applicable, ipc) flags in registry priority order.
    pub desktops: Vec<DesktopStatus>,
}

/// One desktop adapter's discovery result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopStatus {
    pub id: &'static str,
    pub applicable: bool,
    pub ipc: bool,
}

/// Candidate compositor context used for deterministic backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositorCandidate {
    pub id: &'static str,
    pub applicable: bool,
    pub ipc: bool,
    pub score: u8,
}

fn explicit_environment_hint(id: &str) -> bool {
    match id {
        "hyprland" => {
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|value| !value.is_empty())
        }
        "sway" => std::env::var_os("SWAYSOCK").is_some_and(|value| !value.is_empty()),
        "i3" => std::env::var_os("I3SOCK").is_some_and(|value| !value.is_empty()),
        _ => false,
    }
}

fn desktop_hint(id: &str, display: &str, context: &compositor::Context) -> bool {
    context.desktop.as_deref().is_some_and(|desktop| {
        desktop
            .split(':')
            .any(|value| value.eq_ignore_ascii_case(id) || value.eq_ignore_ascii_case(display))
    })
}

impl DesktopStatus {
    pub fn evaluate(
        id: &'static str,
        applicable: impl FnOnce() -> bool,
        ipc: Option<impl FnOnce() -> bool>,
    ) -> Self {
        let is_applicable = applicable();
        let has_ipc = if is_applicable {
            ipc.map(|check| check()).unwrap_or(false)
        } else {
            false
        };
        Self {
            id,
            applicable: is_applicable,
            ipc: has_ipc,
        }
    }
}

impl Environment {
    /// Collect every discovery probe once.
    pub fn collect() -> Self {
        let snapshot = crate::util::ProcessSnapshot::collect();
        let hyprland_probe = hyprland::collect_probe();
        let compositor_context = compositor::current_context();
        let desktops = crate::registry::DESKTOPS
            .iter()
            .map(|descriptor| {
                if descriptor.id == "hyprland" {
                    DesktopStatus {
                        id: descriptor.id,
                        applicable: hyprland_probe.is_some(),
                        ipc: hyprland_probe
                            .as_ref()
                            .is_some_and(hyprland::Probe::ipc_available),
                    }
                } else {
                    DesktopStatus::evaluate(descriptor.id, descriptor.applicable, descriptor.ipc)
                }
            })
            .collect::<Vec<_>>();
        let mut compositor_candidates = crate::registry::DESKTOPS
            .iter()
            .zip(&desktops)
            .filter(|(_, status)| status.applicable)
            .map(|(descriptor, status)| CompositorCandidate {
                id: status.id,
                applicable: status.applicable,
                ipc: status.ipc,
                score: u8::from(status.ipc) * 3
                    + u8::from(explicit_environment_hint(status.id)) * 2
                    + u8::from(desktop_hint(
                        status.id,
                        descriptor.display,
                        &compositor_context,
                    )),
            })
            .collect::<Vec<_>>();
        compositor_candidates.sort_by(|left, right| right.score.cmp(&left.score));
        let selected_compositor = compositor_candidates.first().map(|candidate| candidate.id);

        let shell = std::env::var("SHELL").unwrap_or_default();
        let shell_name = shell
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let tmux = std::env::var_os("TMUX").is_some();
        let screen = std::env::var_os("STY").is_some();
        let zellij = std::env::var_os("ZELLIJ").is_some()
            || std::env::var_os("ZELLIJ_SESSION_NAME").is_some();
        let mut names = Vec::new();
        if tmux {
            names.push("tmux");
        }
        if screen {
            names.push("screen");
        }
        if zellij {
            names.push("zellij");
        }
        let multiplexer = if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(" + ")
        };

        Self {
            ssh: std::env::var_os("SSH_CONNECTION").is_some()
                || std::env::var_os("SSH_TTY").is_some(),
            tty_available: std::fs::File::open("/dev/tty").is_ok(),
            shell,
            shell_name,
            terminal: terminal_app::detected_name().or_else(|| {
                if ghostty::is_active() {
                    Some("Ghostty")
                } else {
                    None
                }
            }),
            multiplexer,
            tmux,
            screen,
            zellij,
            term_program: std::env::var("TERM_PROGRAM").ok(),
            tmux_pane: std::env::var("TMUX_PANE").ok(),
            screen_session: std::env::var("STY").ok(),
            zellij_pane: std::env::var("ZELLIJ_PANE_ID").ok(),
            zellij_session: std::env::var("ZELLIJ_SESSION_NAME").ok(),
            compositor_context,
            remappers: remapper::detect_with_snapshot(&snapshot),
            hyprland_probe,
            ime: ime::detect_with_snapshot(&snapshot),
            selected_compositor,
            compositor_candidates,
            desktops,
        }
    }

    pub fn desktop(&self, id: &str) -> DesktopStatus {
        self.desktops
            .iter()
            .find(|status| status.id == id)
            .copied()
            .unwrap_or(DesktopStatus {
                id: "unknown",
                applicable: false,
                ipc: false,
            })
    }

    /// Compositor detail for schema context, reusing the snapshot.
    pub fn compositor_context(&self) -> compositor::Context {
        self.compositor_context.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_repeatable() {
        let first = Environment::collect();
        let second = Environment::collect();
        assert_eq!(
            first.selected_compositor, second.selected_compositor,
            "one snapshot per command must give a stable selection"
        );
        assert_eq!(first.desktops.len(), second.desktops.len());
        assert_eq!(
            first.compositor_candidates, second.compositor_candidates,
            "candidate scoring must be stable within one environment snapshot"
        );
        assert_eq!(first.compositor_context, second.compositor_context);
        assert_eq!(first.remappers, second.remappers);
        assert_eq!(first.ime, second.ime);
    }

    #[test]
    fn inapplicable_adapter_never_checks_ipc() {
        let status = DesktopStatus::evaluate(
            "test",
            || false,
            Some(|| panic!("IPC checked when inapplicable")),
        );
        assert!(!status.applicable);
        assert!(!status.ipc);
    }

    #[test]
    fn applicable_adapter_checks_ipc() {
        let status = DesktopStatus::evaluate("test", || true, Some(|| true));
        assert!(status.applicable);
        assert!(status.ipc);
    }
}
