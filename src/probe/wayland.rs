//! The Wayland probe window: one xdg toplevel with a solid shm buffer and a
//! wl_keyboard, built on wayland-client's pure-Rust backend (no libwayland),
//! so it works wherever whykey runs.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::fs::FileExt as _;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::backend::WaylandError;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use super::{ProbeEvent, ProbeKind, ProbeWindow};
use crate::xkb::XkbKeycode;

pub const PROBE_APP_ID: &str = "whykey-probe";

const MAX_KEYMAP_BYTES: usize = 16 << 20;
const MAX_GLOBALS: usize = 1024;
const ROUNDTRIP_TIMEOUT: Duration = Duration::from_secs(5);
const BUFFER_WIDTH: i32 = 240;
const BUFFER_HEIGHT: i32 = 160;
/// Opaque slate; the probe draws no text.
const BUFFER_PIXEL: u32 = 0xff2e_3440;

const COMPOSITOR_VERSION: u32 = 4;
const SHM_VERSION: u32 = 1;
const XDG_WM_BASE_VERSION: u32 = 2;
/// wl_keyboard inherits the seat version. Below 10 the compositor never sends
/// the `repeated` key state, so every press is a real press.
const SEAT_VERSION: u32 = 7;

/// Whether the environment names a Wayland display.
pub fn display_configured() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

/// `$WAYLAND_DISPLAY` when absolute, else `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY`.
pub(crate) fn socket_path() -> io::Result<PathBuf> {
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "WAYLAND_DISPLAY is not set"))?;
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Ok(display);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is not set, so the Wayland socket cannot be located",
            )
        })?;
    Ok(PathBuf::from(runtime).join(display))
}

fn protocol_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn wayland_error(error: WaylandError) -> io::Error {
    match error {
        WaylandError::Io(error) => error,
        WaylandError::Protocol(error) => io::Error::other(format!(
            "Wayland protocol error on {}@{} (code {}): {}",
            error.object_interface, error.object_id, error.code, error.message
        )),
    }
}

fn dispatch_error(error: DispatchError) -> io::Error {
    match error {
        DispatchError::Backend(error) => wayland_error(error),
        error => protocol_error(format!("malformed Wayland message: {error}")),
    }
}

/// A memfd holding one solid ARGB8888 frame.
fn create_shm_file(width: i32, height: i32) -> io::Result<OwnedFd> {
    // SAFETY: the name is a valid NUL-terminated string.
    let raw = unsafe { libc::memfd_create(c"whykey-probe".as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: memfd_create returned a new descriptor we exclusively own.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let pixels = (width * height) as usize;
    let mut frame = Vec::with_capacity(pixels * 4);
    for _ in 0..pixels {
        frame.extend_from_slice(&BUFFER_PIXEL.to_le_bytes());
    }
    let mut file = File::from(fd);
    file.write_all(&frame)?;
    Ok(OwnedFd::from(file))
}

/// Copy the keymap text out of the compositor's descriptor.
fn read_keymap(fd: OwnedFd, size: u32) -> io::Result<String> {
    let size = size as usize;
    if size == 0 || size > MAX_KEYMAP_BYTES {
        return Err(protocol_error(format!(
            "Wayland keymap size {size} is outside the accepted range"
        )));
    }
    let file = File::from(fd);
    if file.metadata()?.len() < size as u64 {
        return Err(protocol_error(
            "Wayland keymap file is shorter than announced",
        ));
    }
    // Positioned reads leave the descriptor's offset, which the compositor
    // may share with other clients, untouched.
    let mut bytes = vec![0; size];
    file.read_exact_at(&mut bytes, 0)?;
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(size);
    bytes.truncate(end);
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()))
}

/// The active keymap plus its virtual-modifier to X11 mapping.
struct Keymap {
    text: String,
    /// Real-modifier mask for each virtual modifier, in index order (the
    /// first virtual modifier has index 8).
    virtual_modifiers: Vec<u32>,
}

impl Keymap {
    fn new(text: String) -> Self {
        let virtual_modifiers = virtual_modifier_masks(&text);
        Self {
            text,
            virtual_modifiers,
        }
    }

    fn keysym(&self, keycode: u32, group: usize) -> Option<String> {
        let keycode = XkbKeycode::new(keycode);
        let symbol = crate::xkb::preferred_symbol_for_xkb_keycode(&self.text, keycode, group, 0)
            .or_else(|| {
                (group != 0)
                    .then(|| {
                        crate::xkb::preferred_symbol_for_xkb_keycode(&self.text, keycode, 0, 0)
                    })
                    .flatten()
            })
            .filter(|symbol| symbol != "NoSymbol")?;
        Some(canonical_keysym(symbol))
    }
}

/// Keymaps may spell keysyms numerically (wtype's generated keymaps use
/// `[ 0x61 ]`); report the X keysym name either way.
fn canonical_keysym(symbol: String) -> String {
    match symbol
        .strip_prefix("0x")
        .or_else(|| symbol.strip_prefix("0X"))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    {
        Some(value) => super::x11::keysym_name(value),
        None => symbol,
    }
}

/// X11-style mask for a wl_keyboard modifier mask.
///
/// libxkbcommon always assigns the eight real modifiers the first indices in
/// their fixed order (Shift, Lock, Control, Mod1..Mod5), which is exactly the
/// X11 bit layout. Virtual modifiers follow in declaration order; when the
/// keymap spells out their mapping (`virtual_modifiers Super=Mod4`), a set
/// virtual bit is translated to its real modifiers.
fn x11_modmask(mask: u32, virtual_modifiers: &[u32]) -> u32 {
    let mut result = mask & 0xff;
    for (index, real) in virtual_modifiers.iter().enumerate() {
        let bit = 8 + index;
        if bit < 32 && mask & (1 << bit) != 0 {
            result |= real;
        }
    }
    result
}

/// Virtual modifiers in first-declaration order with their explicit real
/// mapping (0 when the keymap does not state one).
fn virtual_modifier_masks(keymap: &str) -> Vec<u32> {
    let mut names: Vec<(String, u32)> = Vec::new();
    for statement in keymap.split(';') {
        // A declaration opens its section's body: keep only the text after
        // the last brace (`xkb_types "x" {\n\tvirtual_modifiers ...`).
        let statement = statement
            .rsplit(['{', '}'])
            .next()
            .unwrap_or(statement)
            .trim();
        let Some(rest) = statement.strip_prefix("virtual_modifiers") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        for declaration in rest.split(',') {
            let (name, value) = match declaration.split_once('=') {
                Some((name, value)) => (name.trim(), real_modifier_mask(value.trim())),
                None => (declaration.trim(), 0),
            };
            if name.is_empty() || name.len() > 64 {
                continue;
            }
            if let Some((_, mask)) = names.iter_mut().find(|(known, _)| known == name) {
                *mask |= value;
            } else if names.len() < 24 {
                names.push((name.to_owned(), value));
            }
        }
    }
    names.into_iter().map(|(_, mask)| mask).collect()
}

/// Parse a real-modifier expression: a number or `Mod4+Shift`-style names.
fn real_modifier_mask(value: &str) -> u32 {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u32::from_str_radix(hex, 16).map_or(0, |mask| mask & 0xff);
    }
    if let Ok(mask) = value.parse::<u32>() {
        return mask & 0xff;
    }
    value
        .split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "shift" => 1,
            "lock" => 2,
            "control" => 4,
            "mod1" => 8,
            "mod2" => 16,
            "mod3" => 32,
            "mod4" => 64,
            "mod5" => 128,
            "all" => 0xff,
            _ => 0,
        })
        .fold(0, |mask, bit| mask | bit)
}

#[derive(Debug)]
struct Global {
    name: u32,
    interface: String,
    version: u32,
}

/// Everything the event handlers update.
#[derive(Default)]
struct State {
    globals: Vec<Global>,
    sync_done: bool,
    surface: Option<wl_surface::WlSurface>,
    buffer: Option<wl_buffer::WlBuffer>,
    attached: bool,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    focused: bool,
    closed: bool,
    keymap: Option<Keymap>,
    modmask: u32,
    group: usize,
    events: VecDeque<ProbeEvent>,
    /// A failure inside an event handler, reported after dispatching.
    error: Option<io::Error>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if state.globals.len() < MAX_GLOBALS {
                state.globals.push(Global {
                    name,
                    interface,
                    version,
                });
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.sync_done = true;
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg_surface.ack_configure(serial);
        let Some(surface) = &state.surface else {
            return;
        };
        if !state.attached {
            surface.attach(state.buffer.as_ref(), 0, 0);
            surface.damage(0, 0, BUFFER_WIDTH, BUFFER_HEIGHT);
            state.attached = true;
        }
        surface.commit();
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.closed = true;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        let has_keyboard = capabilities.contains(wl_seat::Capability::Keyboard);
        match (has_keyboard, &state.keyboard) {
            (true, None) => state.keyboard = Some(seat.get_keyboard(queue, ())),
            (false, Some(keyboard)) => {
                // The compositor made the keyboard inert; a later keyboard
                // capability needs a fresh wl_keyboard.
                if keyboard.version() >= 3 {
                    keyboard.release();
                }
                state.keyboard = None;
                state.focused = false;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.keyboard.as_ref() != Some(keyboard) {
            return;
        }
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                state.keymap = None;
                if format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    match read_keymap(fd, size) {
                        Ok(text) => state.keymap = Some(Keymap::new(text)),
                        Err(error) => state.error = Some(error),
                    }
                }
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                if state.surface.as_ref() == Some(&surface) {
                    state.focused = true;
                }
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                if state.surface.as_ref() == Some(&surface) {
                    state.focused = false;
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let kind = match key_state {
                    WEnum::Value(wl_keyboard::KeyState::Released) => ProbeKind::Release,
                    WEnum::Value(wl_keyboard::KeyState::Pressed) => ProbeKind::Press,
                    _ => return,
                };
                let Some(keycode) = key.checked_add(8) else {
                    return;
                };
                let keysym = state
                    .keymap
                    .as_ref()
                    .and_then(|keymap| keymap.keysym(keycode, state.group));
                state.events.push_back(ProbeEvent {
                    keycode,
                    keysym,
                    modmask: state.modmask,
                    kind,
                });
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let virtual_modifiers = state
                    .keymap
                    .as_ref()
                    .map_or(&[][..], |keymap| &keymap.virtual_modifiers[..]);
                state.modmask = x11_modmask(
                    mods_depressed | mods_latched | mods_locked,
                    virtual_modifiers,
                );
                state.group = group as usize;
                state.events.push_back(ProbeEvent {
                    keycode: 0,
                    keysym: None,
                    modmask: state.modmask,
                    kind: ProbeKind::Modifiers,
                });
            }
            _ => {}
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_surface::WlSurface);

/// A mapped xdg toplevel that reports keyboard events delivered to it.
pub struct WaylandWindow {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

/// Connect, create the toplevel titled `title`, and wait for the initial
/// configure round trip. Keyboard focus arrives later, as the compositor
/// decides; poll [`ProbeWindow::focused`].
pub fn open_wayland_window(title: &str) -> io::Result<WaylandWindow> {
    let path = socket_path()?;
    let socket = UnixStream::connect(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "cannot connect to Wayland socket {}: {error}",
                path.display()
            ),
        )
    })?;
    WaylandWindow::open(socket, title)
}

impl WaylandWindow {
    fn open(socket: UnixStream, title: &str) -> io::Result<Self> {
        let conn = Connection::from_socket(socket).map_err(io::Error::other)?;
        let queue = conn.new_event_queue();
        let handle = queue.handle();
        let registry = conn.display().get_registry(&handle, ());
        let mut window = Self {
            conn,
            queue,
            state: State::default(),
        };
        window.roundtrip()?;

        let compositor: wl_compositor::WlCompositor =
            window.bind(&registry, &handle, COMPOSITOR_VERSION)?;
        let shm: wl_shm::WlShm = window.bind(&registry, &handle, SHM_VERSION)?;
        let wm_base: xdg_wm_base::XdgWmBase =
            window.bind(&registry, &handle, XDG_WM_BASE_VERSION)?;
        let _seat: wl_seat::WlSeat = window.bind(&registry, &handle, SEAT_VERSION)?;

        let stride = BUFFER_WIDTH * 4;
        let fd = create_shm_file(BUFFER_WIDTH, BUFFER_HEIGHT)?;
        let pool = shm.create_pool(fd.as_fd(), stride * BUFFER_HEIGHT, &handle, ());
        let buffer = pool.create_buffer(
            0,
            BUFFER_WIDTH,
            BUFFER_HEIGHT,
            stride,
            wl_shm::Format::Argb8888,
            &handle,
            (),
        );
        // The buffer keeps the pool's memory alive.
        pool.destroy();

        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        toplevel.set_title(title.to_owned());
        toplevel.set_app_id(PROBE_APP_ID.to_owned());
        // The initial commit without a buffer asks for the first configure.
        surface.commit();
        window.state.surface = Some(surface);
        window.state.buffer = Some(buffer);
        window.roundtrip()?;
        Ok(window)
    }

    fn bind<I>(
        &self,
        registry: &wl_registry::WlRegistry,
        handle: &QueueHandle<State>,
        max_version: u32,
    ) -> io::Result<I>
    where
        I: Proxy + 'static,
        State: Dispatch<I, ()>,
    {
        let interface = I::interface().name;
        let global = self
            .state
            .globals
            .iter()
            .find(|global| global.interface == interface)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("the Wayland compositor does not offer {interface}"),
                )
            })?;
        let version = global.version.clamp(1, max_version);
        Ok(registry.bind(global.name, version, handle, ()))
    }

    /// Send wl_display.sync and dispatch until its callback fires.
    fn roundtrip(&mut self) -> io::Result<()> {
        self.state.sync_done = false;
        self.conn.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + ROUNDTRIP_TIMEOUT;
        while !self.state.sync_done {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the Wayland compositor did not answer a sync request",
                ));
            }
            self.read_and_dispatch(deadline)?;
        }
        Ok(())
    }

    /// Flush requests, read once (waiting until `deadline`), and dispatch
    /// every queued event.
    fn read_and_dispatch(&mut self, deadline: Instant) -> io::Result<()> {
        match self.conn.flush() {
            Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            result => result.map_err(wayland_error)?,
        }
        if let Some(guard) = self.queue.prepare_read() {
            if super::wait_readable(guard.connection_fd().as_raw_fd(), deadline)? {
                match guard.read() {
                    Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                    result => {
                        result.map_err(wayland_error)?;
                    }
                }
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(dispatch_error)?;
        match self.state.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ProbeWindow for WaylandWindow {
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<ProbeEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.state.events.pop_front() {
                return Ok(Some(event));
            }
            if self.state.closed {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "the probe window was closed",
                ));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.read_and_dispatch(deadline)?;
        }
    }

    fn focused(&self) -> bool {
        self.state.focused
    }

    fn protocol(&self) -> &'static str {
        "Wayland"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use wayland_protocols::xdg::shell::server::{
        xdg_surface as server_xdg_surface, xdg_toplevel as server_xdg_toplevel,
        xdg_wm_base as server_xdg_wm_base,
    };
    use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
    use wayland_server::protocol::{
        wl_buffer as server_wl_buffer, wl_compositor as server_wl_compositor,
        wl_keyboard as server_wl_keyboard, wl_seat as server_wl_seat, wl_shm as server_wl_shm,
        wl_shm_pool as server_wl_shm_pool, wl_surface as server_wl_surface,
    };
    use wayland_server::{Client, DataInit, Display, DisplayHandle, GlobalDispatch, New, Resource};

    use super::*;

    /// The shape libxkbcommon serializes: one statement per line.
    const TEST_KEYMAP: &str = "xkb_keymap {\n\
        xkb_keycodes \"t\" {\n\
        \tminimum = 8;\n\
        \tmaximum = 255;\n\
        \t<AB02> = 53;\n\
        };\n\
        \n\
        xkb_types \"t\" {\n\
        \tvirtual_modifiers NumLock,Super=Mod4;\n\
        };\n\
        \n\
        xkb_symbols \"t\" {\n\
        \tkey <AB02> {\t[ x, X ] };\n\
        };\n\
        \n\
        };\n";

    #[test]
    fn maps_real_and_declared_virtual_modifiers_to_x11_bits() {
        let virtual_modifiers = virtual_modifier_masks(
            "virtual_modifiers NumLock,Super=Mod4;\n virtual_modifiers NumLock,Alt=0x8;",
        );
        assert_eq!(virtual_modifiers, vec![0, 64, 8]);
        assert_eq!(x11_modmask(0x41, &virtual_modifiers), 0x41);
        assert_eq!(x11_modmask(1 << 9, &virtual_modifiers), 64);
        assert_eq!(x11_modmask(1 << 10, &virtual_modifiers), 8);
        // A virtual modifier without a stated mapping adds nothing.
        assert_eq!(x11_modmask(1 << 8, &virtual_modifiers), 0);
        assert_eq!(x11_modmask(u32::MAX, &[]), 0xff);
    }

    #[test]
    fn numeric_keysyms_are_named() {
        assert_eq!(canonical_keysym("0x61".into()), "a");
        assert_eq!(canonical_keysym("0xff0d".into()), "Return");
        assert_eq!(canonical_keysym("Return".into()), "Return");
    }

    fn memfd_with(text: &str) -> OwnedFd {
        // SAFETY: the name is a valid NUL-terminated string.
        let raw = unsafe { libc::memfd_create(c"test-keymap".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(raw >= 0);
        // SAFETY: memfd_create returned a new descriptor we exclusively own.
        let mut file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
        file.write_all(text.as_bytes()).unwrap();
        file.write_all(&[0]).unwrap();
        OwnedFd::from(file)
    }

    #[test]
    fn keymap_reading_rejects_bad_sizes() {
        let text = read_keymap(memfd_with("abc"), 4).unwrap();
        assert_eq!(text, "abc");
        assert!(read_keymap(memfd_with("abc"), 64).is_err());
        assert!(read_keymap(memfd_with("abc"), 0).is_err());
    }

    #[derive(Debug, Default)]
    struct ServerLog {
        title: String,
        app_id: String,
        acked: Option<u32>,
        attached: bool,
        pong: Option<u32>,
    }

    /// A scripted compositor: configures the toplevel, pings, and on
    /// get_keyboard sends a keymap, focus, Super+x (or a protocol error).
    struct Compositor {
        fail_keyboard: bool,
        log: ServerLog,
        surface: Option<server_wl_surface::WlSurface>,
        xdg_surface: Option<server_xdg_surface::XdgSurface>,
        configured: bool,
    }

    struct ClientState(Arc<AtomicBool>);

    impl ClientData for ClientState {
        fn disconnected(&self, _: ClientId, _: DisconnectReason) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Advertised globals; `$body` runs on the freshly bound resource.
    macro_rules! server_global {
        ($interface:ty, |$resource:ident| $body:expr) => {
            impl GlobalDispatch<$interface, ()> for Compositor {
                fn bind(
                    _: &mut Self,
                    _: &DisplayHandle,
                    _: &Client,
                    resource: New<$interface>,
                    _: &(),
                    data_init: &mut DataInit<'_, Self>,
                ) {
                    let $resource = data_init.init(resource, ());
                    $body
                }
            }
        };
    }

    server_global!(server_wl_compositor::WlCompositor, |_compositor| {});
    server_global!(server_wl_shm::WlShm, |_shm| {});
    server_global!(server_xdg_wm_base::XdgWmBase, |wm_base| wm_base.ping(41));
    server_global!(server_wl_seat::WlSeat, |seat| seat
        .capabilities(server_wl_seat::Capability::Keyboard));

    macro_rules! server_dispatch {
        ($interface:ty, |$state:ident, $request:ident, $data_init:ident| $body:expr) => {
            impl wayland_server::Dispatch<$interface, ()> for Compositor {
                fn request(
                    $state: &mut Self,
                    _: &Client,
                    _: &$interface,
                    $request: <$interface as Resource>::Request,
                    _: &(),
                    _: &DisplayHandle,
                    $data_init: &mut DataInit<'_, Self>,
                ) {
                    $body
                }
            }
        };
    }

    server_dispatch!(
        server_wl_compositor::WlCompositor,
        |state, request, init| {
            if let server_wl_compositor::Request::CreateSurface { id } = request {
                state.surface = Some(init.init(id, ()));
            }
        }
    );
    server_dispatch!(server_wl_surface::WlSurface, |state, request, _init| {
        match request {
            server_wl_surface::Request::Attach { .. } => state.log.attached = true,
            server_wl_surface::Request::Commit if !state.configured => {
                state.configured = true;
                if let Some(xdg_surface) = &state.xdg_surface {
                    xdg_surface.configure(7);
                }
            }
            _ => {}
        }
    });
    server_dispatch!(server_wl_shm::WlShm, |_state, request, init| {
        if let server_wl_shm::Request::CreatePool { id, .. } = request {
            init.init(id, ());
        }
    });
    server_dispatch!(server_wl_shm_pool::WlShmPool, |_state, request, init| {
        if let server_wl_shm_pool::Request::CreateBuffer { id, .. } = request {
            init.init(id, ());
        }
    });
    server_dispatch!(server_wl_buffer::WlBuffer, |_state, _request, _init| {});
    server_dispatch!(server_xdg_wm_base::XdgWmBase, |state, request, init| {
        match request {
            server_xdg_wm_base::Request::GetXdgSurface { id, .. } => {
                state.xdg_surface = Some(init.init(id, ()));
            }
            server_xdg_wm_base::Request::Pong { serial } => state.log.pong = Some(serial),
            _ => {}
        }
    });
    server_dispatch!(server_xdg_surface::XdgSurface, |state, request, init| {
        match request {
            server_xdg_surface::Request::GetToplevel { id } => {
                init.init(id, ());
            }
            server_xdg_surface::Request::AckConfigure { serial } => {
                state.log.acked = Some(serial);
            }
            _ => {}
        }
    });
    server_dispatch!(server_xdg_toplevel::XdgToplevel, |state, request, _init| {
        match request {
            server_xdg_toplevel::Request::SetTitle { title } => state.log.title = title,
            server_xdg_toplevel::Request::SetAppId { app_id } => state.log.app_id = app_id,
            _ => {}
        }
    });
    server_dispatch!(server_wl_keyboard::WlKeyboard, |_state, _request, _init| {});
    server_dispatch!(server_wl_seat::WlSeat, |state, request, init| {
        let server_wl_seat::Request::GetKeyboard { id } = request else {
            return;
        };
        let keyboard = init.init(id, ());
        if state.fail_keyboard {
            keyboard.post_error(3_u32, "boom");
            return;
        }
        let keymap = memfd_with(TEST_KEYMAP);
        keyboard.keymap(
            server_wl_keyboard::KeymapFormat::XkbV1,
            keymap.as_fd(),
            TEST_KEYMAP.len() as u32 + 1,
        );
        let surface = state.surface.as_ref().unwrap();
        keyboard.enter(1, surface, Vec::new());
        keyboard.modifiers(2, 1 << 9, 0, 0, 0);
        keyboard.key(3, 0, 45, server_wl_keyboard::KeyState::Pressed);
        keyboard.modifiers(4, 0, 0, 0, 0);
    });

    fn fake_compositor(socket: UnixStream, fail_keyboard: bool) -> ServerLog {
        let mut display = Display::<Compositor>::new().unwrap();
        let handle = display.handle();
        handle.create_global::<Compositor, server_wl_compositor::WlCompositor, ()>(6, ());
        handle.create_global::<Compositor, server_wl_shm::WlShm, ()>(1, ());
        handle.create_global::<Compositor, server_xdg_wm_base::XdgWmBase, ()>(6, ());
        handle.create_global::<Compositor, server_wl_seat::WlSeat, ()>(9, ());
        let disconnected = Arc::new(AtomicBool::new(false));
        display
            .handle()
            .insert_client(socket, Arc::new(ClientState(disconnected.clone())))
            .unwrap();
        let mut compositor = Compositor {
            fail_keyboard,
            log: ServerLog::default(),
            surface: None,
            xdg_surface: None,
            configured: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !disconnected.load(Ordering::SeqCst) && Instant::now() < deadline {
            let fd = display.backend().poll_fd().as_raw_fd();
            super::super::wait_readable(fd, Instant::now() + Duration::from_millis(100)).unwrap();
            display.dispatch_clients(&mut compositor).unwrap();
            let _ = display.flush_clients();
        }
        compositor.log
    }

    fn collect_events(window: &mut WaylandWindow, wanted: usize) -> io::Result<Vec<ProbeEvent>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.len() < wanted && Instant::now() < deadline {
            if let Some(event) = window.next_event(Duration::from_millis(100))? {
                events.push(event);
            }
        }
        Ok(events)
    }

    #[test]
    fn scripted_compositor_session_delivers_focus_keymap_and_keys() {
        let (client, server) = UnixStream::pair().unwrap();
        let compositor = thread::spawn(move || fake_compositor(server, false));
        let mut window = WaylandWindow::open(client, "probe").unwrap();
        let events = collect_events(&mut window, 3).unwrap();
        assert!(window.focused());
        assert_eq!(
            events,
            vec![
                ProbeEvent {
                    keycode: 0,
                    keysym: None,
                    modmask: 64,
                    kind: ProbeKind::Modifiers,
                },
                ProbeEvent {
                    keycode: 53,
                    keysym: Some("x".into()),
                    modmask: 64,
                    kind: ProbeKind::Press,
                },
                ProbeEvent {
                    keycode: 0,
                    keysym: None,
                    modmask: 0,
                    kind: ProbeKind::Modifiers,
                },
            ]
        );
        drop(window);
        let log = compositor.join().unwrap();
        assert_eq!(log.title, "probe");
        assert_eq!(log.app_id, PROBE_APP_ID);
        assert_eq!(log.acked, Some(7));
        assert!(log.attached);
        assert_eq!(log.pong, Some(41));
    }

    #[test]
    fn protocol_errors_surface_with_the_compositor_message() {
        let (client, server) = UnixStream::pair().unwrap();
        let compositor = thread::spawn(move || fake_compositor(server, true));
        let error = WaylandWindow::open(client, "probe")
            .and_then(|mut window| collect_events(&mut window, 1).map(|_| ()))
            .unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        compositor.join().unwrap();
    }
}
