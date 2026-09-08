//! Single per-command environment snapshot.
//!
//! Adapters reuse this instead of rediscovering the desktop, terminal,
//! shell, and processes independently.

use crate::layers::{compositor, ghostty, terminal_app};
use crate::{ime, remapper};

/// Session identity and selected integration points, collected once.
#[derive(Debug, Clone)]
pub struct Environment {
    pub desktop: Option<String>,
    pub session_desktop: Option<String>,
    pub session_type: Option<String>,
    pub display_server: Option<String>,
    pub ssh: bool,
    pub tty_available: bool,
    pub shell: String,
    pub shell_name: String,
    pub terminal: Option<&'static str>,
    pub multiplexer: String,
    pub tmux: bool,
    pub screen: bool,
    pub zellij: bool,
    /// Generic desktop/session context captured with the rest of the
    /// environment. Keeping this here prevents schema and doctor output from
    /// re-reading the session variables after discovery.
    pub compositor_context: compositor::Context,
    /// Read-only pre-compositor and input-method observations captured once
    /// for the command or listener session.
    pub remappers: Vec<remapper::Detection>,
    pub ime: Vec<ime::Detection>,
    /// Selected compositor in inspect priority order, if any.
    pub selected_compositor: Option<&'static str>,
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
        let compositor_context = compositor::current_context();
        let desktops = crate::registry::DESKTOPS
            .iter()
            .map(|descriptor| {
                DesktopStatus::evaluate(descriptor.id, descriptor.applicable, descriptor.ipc)
            })
            .collect::<Vec<_>>();
        let selected_compositor = desktops
            .iter()
            .find(|status| status.applicable)
            .map(|status| status.id);

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
            desktop: std::env::var("XDG_CURRENT_DESKTOP").ok(),
            session_desktop: std::env::var("XDG_SESSION_DESKTOP").ok(),
            session_type: std::env::var("XDG_SESSION_TYPE").ok(),
            display_server: if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                Some("Wayland".to_owned())
            } else if std::env::var_os("DISPLAY").is_some() {
                Some("X11".to_owned())
            } else {
                None
            },
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
            compositor_context,
            remappers: remapper::detect(),
            ime: ime::detect(),
            selected_compositor,
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
