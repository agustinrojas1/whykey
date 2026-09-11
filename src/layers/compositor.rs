use std::env;

use crate::layers::{LayerId, LayerResult, Outcome};

/// Environment-only context for a Linux display session that does not have a
/// dedicated compositor adapter yet. It deliberately does not claim that a
/// shortcut is forwarded: desktop-level keybindings are outside the generic
/// environment APIs available to whykey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub desktop: Option<String>,
    pub session_type: Option<String>,
    pub display_server: Option<String>,
}

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    if ["HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK"]
        .into_iter()
        .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return false;
    }
    let context = current_context();
    if context
        .desktop
        .as_deref()
        .is_some_and(has_dedicated_adapter_name)
    {
        return false;
    }
    context.desktop.is_some() || context.session_type.is_some() || context.display_server.is_some()
}

pub fn current_context() -> Context {
    context_from(
        env::var("XDG_CURRENT_DESKTOP").ok(),
        env::var("XDG_SESSION_DESKTOP").ok(),
        env::var("XDG_SESSION_TYPE").ok(),
        env::var("WAYLAND_DISPLAY").ok(),
        env::var("DISPLAY").ok(),
    )
}

pub fn inspect() -> LayerResult {
    let context = current_context();
    let desktop = context.desktop.as_deref().unwrap_or("unknown desktop");
    let session = context
        .session_type
        .as_deref()
        .or(context.display_server.as_deref())
        .unwrap_or("unknown display session");
    let mut details = vec![
        "desktop/compositor keybindings are not exposed through a generic Linux IPC".into(),
        "use whykey listen for terminal-side evidence; a desktop shortcut may consume the key before it arrives".into(),
    ];
    if let Some(display_server) = context.display_server.as_deref() {
        details.push(format!("display server: {display_server}"));
    }
    LayerResult::new(
        "Desktop compositor",
        LayerId::Compositor,
        Outcome::UncertainContinues,
        format!("{desktop} ({session}) detected; no compositor adapter is available"),
        details,
    )
}

/// Result for a console or otherwise display-less session. Keeping this
/// separate from `inspect` avoids manufacturing a Hyprland IPC failure when
/// there is no desktop compositor to query.
pub fn not_detected() -> LayerResult {
    LayerResult::new(
        "Desktop compositor",
        LayerId::Compositor,
        Outcome::Pass,
        "no desktop compositor session detected",
        vec!["the diagnostic starts at the terminal/TTY path for this session".into()],
    )
}

fn context_from(
    current_desktop: Option<String>,
    session_desktop: Option<String>,
    session_type: Option<String>,
    wayland_display: Option<String>,
    x_display: Option<String>,
) -> Context {
    let desktop = first_nonempty(current_desktop).or_else(|| first_nonempty(session_desktop));
    let session_type = first_nonempty(session_type);
    let display_server = if wayland_display
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        Some("Wayland".into())
    } else if x_display.as_deref().is_some_and(|value| !value.is_empty()) {
        Some("X11".into())
    } else {
        None
    };
    Context {
        desktop,
        session_type,
        display_server,
    }
}

fn first_nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn has_dedicated_adapter_name(desktop: &str) -> bool {
    desktop.split(':').any(|name| {
        matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "hyprland"
                | "sway"
                | "i3"
                | "gnome"
                | "kde"
                | "kde plasma"
                | "plasma"
                | "xfce"
                | "xfce4"
                | "cinnamon"
                | "x-cinnamon"
                | "mate"
                | "bspwm"
                | "openbox"
                | "niri"
                | "river"
                | "wayfire"
                | "labwc"
                | "awesome"
                | "awesomewm"
                | "qtile"
                | "xmonad"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{LayerStatus, Propagation};

    #[test]
    fn prefers_current_desktop_and_identifies_wayland() {
        let context = context_from(
            Some("KDE".into()),
            Some("kde".into()),
            Some("wayland".into()),
            Some("wayland-0".into()),
            None,
        );
        assert_eq!(context.desktop.as_deref(), Some("KDE"));
        assert_eq!(context.session_type.as_deref(), Some("wayland"));
        assert_eq!(context.display_server.as_deref(), Some("Wayland"));
    }

    #[test]
    fn falls_back_to_session_desktop_and_x11() {
        let context = context_from(
            Some(" ".into()),
            Some("GNOME".into()),
            None,
            None,
            Some(":0".into()),
        );
        assert_eq!(context.desktop.as_deref(), Some("GNOME"));
        assert_eq!(context.display_server.as_deref(), Some("X11"));
    }

    #[test]
    fn represents_a_console_without_a_fake_ipc_failure() {
        let result = not_detected();
        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(result.summary.contains("no desktop compositor"));
    }
}
