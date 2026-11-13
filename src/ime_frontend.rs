//! Which frontend connects the session's input method to applications,
//! and so where in the route a key meets it.
//!
//! With a Wayland input-method-v2 frontend the compositor hands each key to
//! the input method (a keyboard grab) before the focused client sees it.
//! With X11 XIM or a GTK/Qt input-method module the client, here the
//! terminal, forwards keys to the input method. The two orders differ, so
//! the route places the input method accordingly. Everything here is
//! read-only: a process list and two files under `/proc` per daemon.

use crate::ime::Detection;
use crate::layers::ImePlacement;
use crate::util::ProcessSnapshot;

/// Fcitx5 loads this addon only while it serves Wayland clients through
/// input-method-v2.
const FCITX5_WAYLAND_ADDON: &str = "libwaylandim.so";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeFrontend {
    /// No input method was detected, so there is nothing to place.
    None,
    /// A Wayland input-method-v2 frontend grabs the keyboard.
    WaylandInputMethodV2 { evidence: String },
    /// Applications reach the input method through XIM or a toolkit input
    /// method module, so the terminal forwards keys to it.
    ClientModule { evidence: String },
    /// The frontend could not be determined; the reason says why.
    Undetermined { reason: String },
}

impl ImeFrontend {
    /// Where the input method sits in the route. An undetermined frontend
    /// keeps the historical order (after the terminal).
    pub fn placement(&self) -> ImePlacement {
        match self {
            Self::WaylandInputMethodV2 { .. } => ImePlacement::BeforeTerminal,
            Self::None | Self::ClientModule { .. } | Self::Undetermined { .. } => {
                ImePlacement::AfterTerminal
            }
        }
    }

    /// The detail line for the input-method row, naming what the placement
    /// rests on. `None` when there is no input method to describe.
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::WaylandInputMethodV2 { evidence } => Some(format!(
                "frontend: Wayland input-method-v2 ({evidence}); the compositor hands keys to the input method before the terminal"
            )),
            Self::ClientModule { evidence } => Some(format!(
                "frontend: XIM or a toolkit input-method module ({evidence}); the terminal forwards keys to the input method"
            )),
            Self::Undetermined { reason } => Some(format!(
                "frontend: undetermined ({reason}); the input method is placed after the terminal, which may not be where the key meets it"
            )),
        }
    }
}

pub fn detect(snapshot: &ProcessSnapshot, detections: &[Detection]) -> ImeFrontend {
    let frontend = detect_daemons(snapshot, detections);
    // Read only when it can change the answer, so a session without an
    // input-method-v2 frontend asks nothing more of its environment.
    if matches!(frontend, ImeFrontend::WaylandInputMethodV2 { .. }) {
        let window_id = crate::recording::env_var("WINDOWID").ok();
        return apply_terminal_client(frontend, window_id.as_deref());
    }
    frontend
}

/// `window_id` is `$WINDOWID` of the terminal whykey runs in. Terminals set it
/// only when they are X11 clients (xterm, urxvt, st, VTE and kitty on X11,
/// ...): on a Wayland session that is XWayland. Such a client never enables a
/// text-input, so the input-method-v2 keyboard grab is never armed for it and
/// it reaches the input method through XIM or a toolkit module instead.
fn apply_terminal_client(frontend: ImeFrontend, window_id: Option<&str>) -> ImeFrontend {
    match (&frontend, window_id.filter(|id| !id.is_empty())) {
        (ImeFrontend::WaylandInputMethodV2 { evidence }, Some(id)) => ImeFrontend::ClientModule {
            evidence: format!(
                "the terminal is an X11 (XWayland) client (WINDOWID={id}), which reaches the input method through XIM or a toolkit module, not input-method-v2; the daemon serves Wayland clients: {evidence}"
            ),
        },
        _ => frontend,
    }
}

fn detect_daemons(snapshot: &ProcessSnapshot, detections: &[Detection]) -> ImeFrontend {
    if detections.is_empty() {
        return ImeFrontend::None;
    }
    // Several daemons at once: any one grabbing the keyboard sees the key
    // before the terminal does.
    let mut frontends: Vec<ImeFrontend> = detections
        .iter()
        .map(|detection| detect_one(snapshot, detection))
        .collect();
    if let Some(index) = frontends
        .iter()
        .position(|frontend| matches!(frontend, ImeFrontend::WaylandInputMethodV2 { .. }))
    {
        return frontends.swap_remove(index);
    }
    if let Some(index) = frontends
        .iter()
        .position(|frontend| matches!(frontend, ImeFrontend::ClientModule { .. }))
    {
        return frontends.swap_remove(index);
    }
    frontends.swap_remove(0)
}

fn detect_one(snapshot: &ProcessSnapshot, detection: &Detection) -> ImeFrontend {
    match detection.engine.as_str() {
        "fcitx5" => fcitx5_frontend(snapshot),
        // IBus reaches applications through its toolkit modules and XIM;
        // its Wayland support is the same modules talking to text-input.
        "ibus" => ImeFrontend::ClientModule {
            evidence: "IBus serves applications through its input-method modules".into(),
        },
        engine => ImeFrontend::Undetermined {
            reason: format!("no frontend check for {engine}"),
        },
    }
}

fn fcitx5_frontend(snapshot: &ProcessSnapshot) -> ImeFrontend {
    let pids = snapshot.matching_pids(&["fcitx5"]);
    if pids.is_empty() {
        return ImeFrontend::Undetermined {
            reason: "the fcitx5 process was not found, only its environment variables".into(),
        };
    }
    let mut answers = Vec::new();
    for pid in pids {
        let maps = crate::recording::read(format!("/proc/{pid}/maps"));
        let environ = crate::recording::read(format!("/proc/{pid}/environ"));
        if let (Ok(maps), Ok(environ)) = (maps, environ) {
            answers.push(classify_fcitx5(pid, &maps, &environ));
        }
    }
    // One daemon serving a Wayland display is enough: it grabs the keyboard.
    answers
        .iter()
        .position(|answer| matches!(answer, ImeFrontend::WaylandInputMethodV2 { .. }))
        .map(|index| answers.swap_remove(index))
        .or_else(|| answers.pop())
        .unwrap_or_else(|| ImeFrontend::Undetermined {
            reason: "the fcitx5 process memory map or environment is not readable".into(),
        })
}

/// Fcitx5 loads its Wayland addon even where no Wayland display exists, so
/// the addon alone proves nothing: the daemon must also have been started
/// with a Wayland display to connect to.
fn classify_fcitx5(pid: u32, maps: &[u8], environ: &[u8]) -> ImeFrontend {
    let addon = contains(maps, FCITX5_WAYLAND_ADDON.as_bytes());
    let display = environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"WAYLAND_DISPLAY="))
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned());
    match (addon, display) {
        (true, Some(display)) => ImeFrontend::WaylandInputMethodV2 {
            evidence: format!(
                "fcitx5 pid {pid} has {FCITX5_WAYLAND_ADDON} loaded and was started with WAYLAND_DISPLAY={display}"
            ),
        },
        (true, None) => ImeFrontend::ClientModule {
            evidence: format!(
                "fcitx5 pid {pid} was started without WAYLAND_DISPLAY, so it serves no Wayland display"
            ),
        },
        (false, _) => ImeFrontend::ClientModule {
            evidence: format!("fcitx5 pid {pid} has no Wayland input-method addon loaded"),
        },
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_follows_the_frontend() {
        let wayland = ImeFrontend::WaylandInputMethodV2 {
            evidence: "x".into(),
        };
        assert_eq!(wayland.placement(), ImePlacement::BeforeTerminal);
        for frontend in [
            ImeFrontend::None,
            ImeFrontend::ClientModule {
                evidence: "x".into(),
            },
            ImeFrontend::Undetermined { reason: "x".into() },
        ] {
            assert_eq!(frontend.placement(), ImePlacement::AfterTerminal);
        }
    }

    #[test]
    fn undetermined_frontend_says_which_order_was_assumed() {
        let detail = ImeFrontend::Undetermined {
            reason: "the fcitx5 process was not found".into(),
        }
        .detail()
        .unwrap();
        assert!(detail.contains("undetermined"), "{detail}");
        assert!(detail.contains("after the terminal"), "{detail}");
    }

    #[test]
    fn no_input_method_has_no_frontend_detail() {
        assert_eq!(ImeFrontend::None.detail(), None);
        assert_eq!(detect(&ProcessSnapshot::collect(), &[]), ImeFrontend::None);
    }

    #[test]
    fn fcitx5_needs_the_addon_and_a_wayland_display_to_grab_the_keyboard() {
        let with_addon = b"7f00-7f01 r-xp 0 08:01 12 /usr/lib/fcitx5/libwaylandim.so\n";
        let without_addon = b"7f00-7f01 r-xp 0 08:01 12 /usr/lib/fcitx5/libxim.so\n";
        let wayland = b"HOME=/root\0WAYLAND_DISPLAY=wayland-1\0";
        let x11 = b"HOME=/root\0DISPLAY=:0\0WAYLAND_DISPLAY=\0";
        assert_eq!(
            classify_fcitx5(7, with_addon, wayland).placement(),
            ImePlacement::BeforeTerminal
        );
        // The addon is loaded even on an X11-only session.
        let x11_only = classify_fcitx5(7, with_addon, x11);
        assert!(
            matches!(x11_only, ImeFrontend::ClientModule { .. }),
            "{x11_only:?}"
        );
        let no_addon = classify_fcitx5(7, without_addon, wayland);
        assert!(
            matches!(no_addon, ImeFrontend::ClientModule { .. }),
            "{no_addon:?}"
        );
    }

    #[test]
    fn an_x11_terminal_reaches_the_input_method_through_the_client_not_the_grab() {
        // fcitx5 serves Wayland clients through input-method-v2, but an X11
        // (XWayland) terminal, which exports WINDOWID, is not one of them.
        let maps = b"7f00-7f01 r-xp 0 08:01 12 /usr/lib/fcitx5/libwaylandim.so\n";
        let wayland = classify_fcitx5(7, maps, b"WAYLAND_DISPLAY=wayland-1\0");
        assert_eq!(wayland.placement(), ImePlacement::BeforeTerminal);
        for none in [None, Some("")] {
            assert_eq!(
                apply_terminal_client(wayland.clone(), none),
                wayland,
                "{none:?}"
            );
        }
        let x11 = apply_terminal_client(wayland, Some("62914563"));
        assert_eq!(x11.placement(), ImePlacement::AfterTerminal);
        assert!(x11.detail().unwrap().contains("XWayland"), "{x11:?}");
        // A client-module frontend is unchanged.
        let module = ImeFrontend::ClientModule {
            evidence: "x".into(),
        };
        assert_eq!(apply_terminal_client(module.clone(), Some("1")), module);
    }

    #[test]
    fn needle_search_matches_inside_a_memory_map() {
        let maps = b"7f00-7f01 r-xp 0 08:01 12 /usr/lib/fcitx5/libwaylandim.so\n";
        assert!(contains(maps, FCITX5_WAYLAND_ADDON.as_bytes()));
        assert!(!contains(b"libxim.so", FCITX5_WAYLAND_ADDON.as_bytes()));
    }
}
