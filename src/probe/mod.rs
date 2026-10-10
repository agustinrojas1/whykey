//! Probe windows: whykey's own minimal Wayland/X11 clients.
//!
//! A probe window is an ordinary toplevel that asks for keyboard focus and
//! reports exactly what the display server delivers to it. A key that
//! arrives proves the compositor/desktop passed it to a focused client;
//! modifier changes that arrive without the key prove something above the
//! window consumed it. This holds on every compositor because it relies only
//! on the core client protocol, not on compositor-specific IPC.

use std::io;
use std::time::Duration;

pub mod wayland;
pub mod x11;

/// What kind of input event a probe window received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Press,
    Release,
    /// The modifier state changed; `keycode` is 0 and `keysym` is `None`.
    Modifiers,
}

/// One input event delivered to a probe window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeEvent {
    /// XKB keycode (evdev + 8).
    pub keycode: u32,
    /// Keysym name resolved via the client keymap (level 1, unshifted).
    pub keysym: Option<String>,
    /// X11-style bits: Shift 1, Lock 2, Control 4, Mod1 8, Mod2 16, Mod3 32,
    /// Mod4 64, Mod5 128.
    pub modmask: u32,
    pub kind: ProbeKind,
}

pub trait ProbeWindow {
    /// Next input event delivered to this window, or None on timeout.
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<ProbeEvent>>;
    /// Whether the window currently has keyboard focus.
    fn focused(&self) -> bool;
}

/// The display protocol [`open_window`] would try first, judged from the
/// environment without connecting: "Wayland" when the Wayland socket exists,
/// "X11" when DISPLAY is set, else `None`.
pub fn available_display() -> Option<&'static str> {
    if wayland::socket_path().is_ok_and(|path| path.exists()) {
        Some("Wayland")
    } else if std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty()) {
        Some("X11")
    } else {
        None
    }
}

/// Opens a probe window: Wayland when WAYLAND_DISPLAY is usable, else X11 via
/// DISPLAY.
pub fn open_window(title: &str) -> io::Result<Box<dyn ProbeWindow>> {
    let wayland_error = if wayland::display_configured() {
        match wayland::open_wayland_window(title) {
            Ok(window) => return Ok(Box::new(window)),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    let x11_configured = std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty());
    if !x11_configured {
        return Err(wayland_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no display server: neither WAYLAND_DISPLAY nor DISPLAY is set",
            )
        }));
    }
    match x11::open_x11_window(title) {
        Ok(window) => Ok(Box::new(window)),
        Err(x11_error) => Err(match wayland_error {
            Some(wayland_error) => io::Error::new(
                x11_error.kind(),
                format!("Wayland: {wayland_error}; X11: {x11_error}"),
            ),
            None => x11_error,
        }),
    }
}
