//! X11 client (x11rb) for the probe window and passive-grab checks.
//!
//! It opens a probe window that reports the keys the X server delivers to
//! it, and asks the server whether another client already holds a passive
//! key grab. A passive grab is how every X11 window manager and hotkey daemon
//! (i3, Openbox, xbindkeys, sxhkd, KWin, Mutter, ...) claims a shortcut, and
//! the server refuses a second client's grab on the same combination with
//! `BadAccess`. That refusal is observed server state, not a prediction from
//! configuration files.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::fd::AsRawFd;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::x11_utils::X11Error;

use crate::key::KeyCombo;
use crate::probe::{ProbeEvent, ProbeKind, ProbeWindow};

/// How long the probe window waits for the window manager to map it.
const MAP_TIMEOUT: Duration = Duration::from_secs(2);

pub const SHIFT_MASK: u16 = 1;
pub const LOCK_MASK: u16 = 2;

/// The X server's virtual core keyboard (always device 3); Mutter grabs on it.
const XI_VIRTUAL_CORE_KEYBOARD: u16 = 3;
/// XIPassiveGrabDevice reports a combination another client holds with the
/// BadAccess error code as its status.
const XI_STATUS_ACCESS: u8 = 10;

const KEYSYM_NUM_LOCK: u32 = 0xff7f;

/// Whether another X client holds a passive grab on a key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabStatus {
    Free,
    GrabbedByOtherClient,
}

/// Result of probing one keycode + modifier mask on the root window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabCheck {
    pub keycode: u8,
    pub modmask: u16,
    /// Modifier masks (the requested mask combined with Lock/NumLock) another
    /// client grabbed through the core protocol.
    pub core_conflicts: Vec<u16>,
    /// Same for XInput2 passive grabs (Mutter/GNOME Shell); `None` when the
    /// server has no usable XInput2 extension.
    pub xi2_conflicts: Option<Vec<u16>>,
}

impl GrabCheck {
    pub fn status(&self) -> GrabStatus {
        let xi2 = self
            .xi2_conflicts
            .as_ref()
            .is_some_and(|conflicts| !conflicts.is_empty());
        if self.core_conflicts.is_empty() && !xi2 {
            GrabStatus::Free
        } else {
            GrabStatus::GrabbedByOtherClient
        }
    }
}

/// One keycode that produces a key combination's keysym, plus the modifiers
/// that keysym's level needs (Shift for a second-column keysym).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTarget {
    pub keycode: u8,
    pub level_modifiers: u16,
}

/// Whether some X client grabs `keycode` with `modmask` on the root window,
/// in any Lock/NumLock combination (window managers grab all of them).
pub fn grab_status(keycode: u32, modmask: u32) -> io::Result<GrabStatus> {
    let mut connection = Connection::connect_from_env()?;
    let keycode = checked_keycode(keycode)?;
    let modmask = checked_modmask(modmask)?;
    Ok(connection.check_grab(keycode, modmask)?.status())
}

/// Opens a mapped, focused probe toplevel on the `DISPLAY` X server.
pub fn open_x11_window(title: &str) -> io::Result<X11Window> {
    X11Window::open(title)
}

fn checked_keycode(keycode: u32) -> io::Result<u8> {
    u8::try_from(keycode)
        .ok()
        .filter(|keycode| *keycode >= 8)
        .ok_or_else(|| invalid_input(format!("X11 keycode {keycode} is outside 8..=255")))
}

fn checked_modmask(modmask: u32) -> io::Result<u16> {
    u16::try_from(modmask)
        .ok()
        .filter(|mask| *mask <= 0xff)
        .ok_or_else(|| {
            invalid_input(format!(
                "X11 modifier mask {modmask:#x} has non-modifier bits"
            ))
        })
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn connect_error(error: ConnectError) -> io::Error {
    match error {
        ConnectError::IoError(error) => error,
        ConnectError::DisplayParsingError(error) => invalid_input(format!("DISPLAY: {error}")),
        ConnectError::SetupFailed(xproto::SetupFailed { reason, .. })
        | ConnectError::SetupAuthenticate(xproto::SetupAuthenticate { reason, .. }) => {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "X server refused the connection: {}",
                    String::from_utf8_lossy(&reason)
                        .trim_end_matches('\0')
                        .trim()
                ),
            )
        }
        error => io::Error::other(format!("cannot connect to the X server: {error}")),
    }
}

fn connection_error(error: ConnectionError) -> io::Error {
    match error {
        ConnectionError::IoError(error) => error,
        error => io::Error::other(format!("X11 connection failed: {error}")),
    }
}

fn x11_error(error: &X11Error) -> io::Error {
    io::Error::other(format!(
        "X11 request {}.{} failed: {:?} ({})",
        error.major_opcode, error.minor_opcode, error.error_kind, error.error_code
    ))
}

fn reply_error(error: ReplyError) -> io::Error {
    match error {
        ReplyError::ConnectionError(error) => connection_error(error),
        ReplyError::X11Error(error) => x11_error(&error),
    }
}

fn id_error(error: ReplyOrIdError) -> io::Error {
    match error {
        ReplyOrIdError::IdsExhausted => io::Error::other("X11 resource IDs exhausted"),
        ReplyOrIdError::ConnectionError(error) => connection_error(error),
        ReplyOrIdError::X11Error(error) => x11_error(&error),
    }
}

/// Core keysym table: one row of `per_keycode` keysyms per keycode.
#[derive(Debug, Clone, Default)]
struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

impl Keymap {
    fn row(&self, keycode: u8) -> &[u32] {
        let Some(index) = keycode.checked_sub(self.min_keycode) else {
            return &[];
        };
        let start = usize::from(index) * self.per_keycode;
        self.keysyms
            .get(start..start + self.per_keycode)
            .unwrap_or(&[])
    }

    fn keycodes(&self) -> impl Iterator<Item = u8> + '_ {
        let count = self
            .keysyms
            .len()
            .checked_div(self.per_keycode)
            .unwrap_or(0);
        (0..count).filter_map(|index| {
            u8::try_from(index)
                .ok()
                .and_then(|index| self.min_keycode.checked_add(index))
        })
    }

    /// Every keycode whose first or second (Shift) column produces one of
    /// `keysyms`; group 1 only, like the core protocol's keysym lookup.
    fn targets(&self, keysyms: &[u32]) -> Vec<KeyTarget> {
        self.keycodes()
            .filter_map(|keycode| {
                let row = self.row(keycode);
                let level_modifiers = if row.first().is_some_and(|sym| keysyms.contains(sym)) {
                    0
                } else if row.get(1).is_some_and(|sym| keysyms.contains(sym)) {
                    SHIFT_MASK
                } else {
                    return None;
                };
                Some(KeyTarget {
                    keycode,
                    level_modifiers,
                })
            })
            .collect()
    }
}

/// Keycodes bound to each of the eight core modifiers.
#[derive(Debug, Clone, Default)]
struct ModifierMap {
    per_modifier: usize,
    keycodes: Vec<u8>,
}

impl ModifierMap {
    fn modifier_bit(&self, keycode: u8) -> Option<u16> {
        if keycode == 0 || self.per_modifier == 0 {
            return None;
        }
        let index = self.keycodes.iter().position(|code| *code == keycode)?;
        Some(1 << (index / self.per_modifier))
    }

    /// The modifier NumLock is bound to (usually Mod2).
    fn num_lock_mask(&self, keymap: &Keymap) -> Option<u16> {
        keymap
            .keycodes()
            .filter(|keycode| keymap.row(*keycode).contains(&KEYSYM_NUM_LOCK))
            .find_map(|keycode| self.modifier_bit(keycode))
    }
}

/// One connection to an X server.
pub struct Connection {
    conn: RustConnection,
    root: xproto::Window,
    white_pixel: u32,
    min_keycode: u8,
    max_keycode: u8,
    /// Whether the server speaks XInput 2, once asked.
    xinput: Option<bool>,
}

impl Connection {
    /// Connects to `$DISPLAY`, authenticating with the matching
    /// Xauthority entry (`$XAUTHORITY`, else `~/.Xauthority`).
    pub fn connect_from_env() -> io::Result<Self> {
        if std::env::var_os("DISPLAY").is_none_or(|value| value.is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "DISPLAY is not set",
            ));
        }
        let (conn, screen) = RustConnection::connect(None).map_err(connect_error)?;
        let setup = conn.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        if min_keycode < 8 || min_keycode > max_keycode {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("X server reports an invalid keycode range {min_keycode}..={max_keycode}"),
            ));
        }
        let root = setup
            .roots
            .get(screen)
            .ok_or_else(|| invalid_input(format!("X11 screen {screen} does not exist")))?;
        let (root, white_pixel) = (root.root, root.white_pixel);
        Ok(Self {
            conn,
            root,
            white_pixel,
            min_keycode,
            max_keycode,
            xinput: None,
        })
    }

    fn intern_atom(&self, name: &str) -> io::Result<xproto::Atom> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?
            .atom)
    }

    fn keyboard_mapping(&self) -> io::Result<Keymap> {
        let count = self.max_keycode - self.min_keycode + 1;
        let reply = self
            .conn
            .get_keyboard_mapping(self.min_keycode, count)
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        Ok(Keymap {
            min_keycode: self.min_keycode,
            per_keycode: usize::from(reply.keysyms_per_keycode),
            keysyms: reply.keysyms,
        })
    }

    fn modifier_mapping(&self) -> io::Result<ModifierMap> {
        let reply = self
            .conn
            .get_modifier_mapping()
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        Ok(ModifierMap {
            per_modifier: usize::from(reply.keycodes_per_modifier()),
            keycodes: reply.keycodes,
        })
    }

    /// Keycodes that produce `key`'s keysym in the server's current keymap.
    /// `CODE:N` names keycode N directly.
    pub fn key_targets(&self, key: &KeyCombo) -> io::Result<Vec<KeyTarget>> {
        if let Some(code) = key.key().strip_prefix("CODE:") {
            let keycode = code
                .parse::<u32>()
                .map_err(|_| invalid_input(format!("invalid keycode '{code}'")))
                .and_then(checked_keycode)?;
            return Ok(vec![KeyTarget {
                keycode,
                level_modifiers: 0,
            }]);
        }
        let keysyms = keysyms_for_key(key.key());
        if keysyms.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.keyboard_mapping()?.targets(&keysyms))
    }

    fn lock_variants(&self, modmask: u16) -> io::Result<Vec<u16>> {
        let keymap = self.keyboard_mapping()?;
        let num_lock = self.modifier_mapping()?.num_lock_mask(&keymap);
        let mut variants = vec![modmask, modmask | LOCK_MASK];
        if let Some(num_lock) = num_lock {
            variants.extend([modmask | num_lock, modmask | LOCK_MASK | num_lock]);
        }
        let mut unique = Vec::with_capacity(variants.len());
        for variant in variants {
            if !unique.contains(&variant) {
                unique.push(variant);
            }
        }
        Ok(unique)
    }

    /// Tries to grab `keycode`+`modmask` (with every Lock/NumLock variant)
    /// on the root window through the core protocol and XInput2. A grab the
    /// server refuses is held by another client; grabs that succeed are
    /// released immediately.
    pub fn check_grab(&mut self, keycode: u8, modmask: u16) -> io::Result<GrabCheck> {
        if keycode < self.min_keycode || keycode > self.max_keycode {
            return Err(invalid_input(format!(
                "X11 keycode {keycode} is outside the server range {}..={}",
                self.min_keycode, self.max_keycode
            )));
        }
        let variants = self.lock_variants(modmask)?;

        let mut grabs = Vec::with_capacity(variants.len());
        for &variant in &variants {
            let cookie = self
                .conn
                .grab_key(
                    false,
                    self.root,
                    xproto::ModMask::from(variant),
                    keycode,
                    xproto::GrabMode::ASYNC,
                    xproto::GrabMode::ASYNC,
                )
                .map_err(connection_error)?;
            grabs.push((variant, cookie));
        }
        let mut core_conflicts = Vec::new();
        for (variant, cookie) in grabs {
            match cookie.check() {
                Ok(()) => {}
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {
                    core_conflicts.push(variant);
                }
                Err(error) => return Err(reply_error(error)),
            }
        }
        let mut ungrabs = Vec::with_capacity(variants.len());
        for &variant in &variants {
            if !core_conflicts.contains(&variant) {
                ungrabs.push(
                    self.conn
                        .ungrab_key(keycode, self.root, xproto::ModMask::from(variant))
                        .map_err(connection_error)?,
                );
            }
        }
        for cookie in ungrabs {
            cookie.check().map_err(reply_error)?;
        }

        let xi2_conflicts = if self.xinput2_available()? {
            Some(self.check_xi2_grab(keycode, &variants)?)
        } else {
            None
        };
        Ok(GrabCheck {
            keycode,
            modmask,
            core_conflicts,
            xi2_conflicts,
        })
    }

    /// Whether the server supports XInput 2.0.
    fn xinput2_available(&mut self) -> io::Result<bool> {
        if let Some(available) = self.xinput {
            return Ok(available);
        }
        let present = self
            .conn
            .extension_information(xinput::X11_EXTENSION_NAME)
            .map_err(connection_error)?
            .is_some();
        let available = present
            && match self
                .conn
                .xinput_xi_query_version(2, 0)
                .map_err(connection_error)?
                .reply()
            {
                Ok(reply) => reply.major_version >= 2,
                Err(ReplyError::X11Error(_)) => false,
                Err(error) => return Err(reply_error(error)),
            };
        self.xinput = Some(available);
        Ok(available)
    }

    fn check_xi2_grab(&self, keycode: u8, variants: &[u16]) -> io::Result<Vec<u16>> {
        let modifiers: Vec<u32> = variants.iter().map(|&variant| u32::from(variant)).collect();
        let mask = [u32::from(
            xinput::XIEventMask::KEY_PRESS | xinput::XIEventMask::KEY_RELEASE,
        )];
        let reply = self
            .conn
            .xinput_xi_passive_grab_device(
                x11rb::CURRENT_TIME,
                self.root,
                x11rb::NONE,
                u32::from(keycode),
                XI_VIRTUAL_CORE_KEYBOARD,
                xinput::GrabType::KEYCODE,
                xinput::GrabMode22::ASYNC,
                xproto::GrabMode::ASYNC,
                xinput::GrabOwner::NO_OWNER,
                &mask,
                &modifiers,
            )
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        let mut conflicts = Vec::new();
        let mut held = modifiers;
        for failed in reply.modifiers {
            let Some(variant) = variants
                .iter()
                .copied()
                .find(|variant| u32::from(*variant) == failed.modifiers)
            else {
                continue;
            };
            held.retain(|held| *held != failed.modifiers);
            let status = u8::from(failed.status);
            if status == XI_STATUS_ACCESS {
                conflicts.push(variant);
            } else {
                return Err(io::Error::other(format!(
                    "XInput2 passive grab failed with status {status}"
                )));
            }
        }
        if !held.is_empty() {
            self.conn
                .xinput_xi_passive_ungrab_device(
                    self.root,
                    u32::from(keycode),
                    XI_VIRTUAL_CORE_KEYBOARD,
                    xinput::GrabType::KEYCODE,
                    &held,
                )
                .map_err(connection_error)?
                .check()
                .map_err(reply_error)?;
        }
        Ok(conflicts)
    }

    /// Next event before `deadline`, or `None`.
    fn next_event(&self, deadline: Instant) -> io::Result<Option<Event>> {
        self.conn.flush().map_err(connection_error)?;
        loop {
            if let Some(event) = self.conn.poll_for_event().map_err(connection_error)? {
                return Ok(Some(event));
            }
            if !super::wait_readable(self.conn.stream().as_raw_fd(), deadline)? {
                return Ok(None);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Probe window

/// A whykey-owned X11 toplevel that reports the keys it receives.
pub struct X11Window {
    connection: Connection,
    window: xproto::Window,
    focused: bool,
    keymap: Keymap,
    modifiers: ModifierMap,
    /// Events read while waiting for the window to be mapped.
    deferred: VecDeque<Event>,
    pending: VecDeque<ProbeEvent>,
    /// Modifier state after the last reported event.
    modmask: u16,
}

impl X11Window {
    fn open(title: &str) -> io::Result<Self> {
        let connection = Connection::connect_from_env()?;
        let net_wm_name = connection.intern_atom("_NET_WM_NAME")?;
        let utf8_string = connection.intern_atom("UTF8_STRING")?;
        let net_wm_pid = connection.intern_atom("_NET_WM_PID")?;
        let keymap = connection.keyboard_mapping()?;
        let modifiers = connection.modifier_mapping()?;

        let conn = &connection.conn;
        let window = conn.generate_id().map_err(id_error)?;
        let latin1 = title
            .chars()
            .map(|character| u8::try_from(u32::from(character)).unwrap_or(b'?'))
            .collect::<Vec<_>>();
        // WM_HINTS: flags = InputHint, input = True, the other seven fields 0.
        let hints = [1, 1, 0, 0, 0, 0, 0, 0, 0];
        let requests = [
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                connection.root,
                0,
                0,
                480,
                160,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &xproto::CreateWindowAux::new()
                    .background_pixel(connection.white_pixel)
                    .event_mask(
                        xproto::EventMask::KEY_PRESS
                            | xproto::EventMask::KEY_RELEASE
                            | xproto::EventMask::FOCUS_CHANGE
                            | xproto::EventMask::STRUCTURE_NOTIFY,
                    ),
            ),
            conn.change_property8(
                xproto::PropMode::REPLACE,
                window,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                &latin1,
            ),
            conn.change_property8(
                xproto::PropMode::REPLACE,
                window,
                net_wm_name,
                utf8_string,
                title.as_bytes(),
            ),
            conn.change_property8(
                xproto::PropMode::REPLACE,
                window,
                xproto::AtomEnum::WM_CLASS,
                xproto::AtomEnum::STRING,
                b"whykey-probe\0whykey\0",
            ),
            conn.change_property32(
                xproto::PropMode::REPLACE,
                window,
                net_wm_pid,
                xproto::AtomEnum::CARDINAL,
                &[std::process::id()],
            ),
            conn.change_property32(
                xproto::PropMode::REPLACE,
                window,
                xproto::AtomEnum::WM_HINTS,
                xproto::AtomEnum::WM_HINTS,
                &hints,
            ),
            conn.map_window(window),
        ];
        for request in requests {
            request
                .map_err(connection_error)?
                .check()
                .map_err(reply_error)?;
        }

        let mut probe = Self {
            connection,
            window,
            focused: false,
            keymap,
            modifiers,
            deferred: VecDeque::new(),
            pending: VecDeque::new(),
            modmask: 0,
        };
        // A window manager maps the window when it handles the MapRequest;
        // focus can only be set once the window is viewable.
        let deadline = Instant::now() + MAP_TIMEOUT;
        let mut mapped = false;
        while let Some(event) = probe.connection.next_event(deadline)? {
            let map_notify = matches!(&event, Event::MapNotify(notify) if notify.window == window);
            probe.deferred.push_back(event);
            if map_notify {
                mapped = true;
                break;
            }
        }
        if !mapped {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the X11 probe window was not mapped in time",
            ));
        }
        let conn = &probe.connection.conn;
        // A window manager may refuse focus changes; the focus state below
        // and later FocusIn/FocusOut events report what actually happened.
        match conn
            .set_input_focus(
                xproto::InputFocus::POINTER_ROOT,
                window,
                x11rb::CURRENT_TIME,
            )
            .map_err(connection_error)?
            .check()
        {
            Ok(()) | Err(ReplyError::X11Error(_)) => {}
            Err(error) => return Err(reply_error(error)),
        }
        let focus = conn
            .get_input_focus()
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        probe.focused = focus.focus == window;
        Ok(probe)
    }

    fn refresh_keymap(&mut self) -> io::Result<()> {
        self.keymap = self.connection.keyboard_mapping()?;
        self.modifiers = self.connection.modifier_mapping()?;
        Ok(())
    }

    fn handle(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::KeyPress(key) | Event::KeyRelease(key) if key.event == self.window => {
                let press = key.response_type & 0x7f == xproto::KEY_PRESS_EVENT;
                let keycode = key.detail;
                let state = u16::from(key.state) & 0xff;
                self.pending.push_back(ProbeEvent {
                    keycode: u32::from(keycode),
                    keysym: self
                        .keymap
                        .row(keycode)
                        .first()
                        .copied()
                        .filter(|keysym| *keysym != 0)
                        .map(keysym_name),
                    modmask: u32::from(state),
                    kind: if press {
                        ProbeKind::Press
                    } else {
                        ProbeKind::Release
                    },
                });
                let after = match self.modifiers.modifier_bit(keycode) {
                    Some(bit) => {
                        let after = if press { state | bit } else { state & !bit };
                        self.pending.push_back(ProbeEvent {
                            keycode: 0,
                            keysym: None,
                            modmask: u32::from(after),
                            kind: ProbeKind::Modifiers,
                        });
                        after
                    }
                    None => state,
                };
                self.modmask = after;
            }
            Event::FocusIn(focus) | Event::FocusOut(focus)
                if focus.event == self.window
                    && !matches!(
                        focus.detail,
                        xproto::NotifyDetail::INFERIOR | xproto::NotifyDetail::POINTER
                    ) =>
            {
                // NotifyInferior/NotifyPointer: focus did not really move
                // to/from this window.
                let focus_in = focus.response_type & 0x7f == xproto::FOCUS_IN_EVENT;
                self.focused = focus_in;
                if focus_in {
                    // Key events delivered elsewhere while focus was away
                    // (including a window manager's grab, which sends
                    // FocusOut/FocusIn with NotifyGrab/NotifyUngrab) may
                    // have changed the modifiers; report the real state.
                    let state = u16::from(
                        self.connection
                            .conn
                            .query_pointer(self.window)
                            .map_err(connection_error)?
                            .reply()
                            .map_err(reply_error)?
                            .mask,
                    ) & 0xff;
                    if state != self.modmask {
                        self.modmask = state;
                        self.pending.push_back(ProbeEvent {
                            keycode: 0,
                            keysym: None,
                            modmask: u32::from(state),
                            kind: ProbeKind::Modifiers,
                        });
                    }
                }
            }
            Event::MappingNotify(_) => self.refresh_keymap()?,
            _ => {}
        }
        Ok(())
    }
}

impl ProbeWindow for X11Window {
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<ProbeEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let event = match self.deferred.pop_front() {
                Some(event) => event,
                None => match self.connection.next_event(deadline)? {
                    Some(event) => event,
                    None => return Ok(None),
                },
            };
            self.handle(event)?;
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn protocol(&self) -> &'static str {
        "X11"
    }
}

// ---------------------------------------------------------------------------
// Keysym names

/// Function, keypad, modifier, and media keysyms without a Unicode mapping.
const NAMED_KEYSYMS: &[(u32, &str)] = &[
    (0xff08, "BackSpace"),
    (0xff09, "Tab"),
    (0xff0a, "Linefeed"),
    (0xff0b, "Clear"),
    (0xff0d, "Return"),
    (0xff13, "Pause"),
    (0xff14, "Scroll_Lock"),
    (0xff15, "Sys_Req"),
    (0xff1b, "Escape"),
    (0xff20, "Multi_key"),
    (0xff50, "Home"),
    (0xff51, "Left"),
    (0xff52, "Up"),
    (0xff53, "Right"),
    (0xff54, "Down"),
    (0xff55, "Prior"),
    (0xff56, "Next"),
    (0xff57, "End"),
    (0xff58, "Begin"),
    (0xff60, "Select"),
    (0xff61, "Print"),
    (0xff62, "Execute"),
    (0xff63, "Insert"),
    (0xff65, "Undo"),
    (0xff66, "Redo"),
    (0xff67, "Menu"),
    (0xff68, "Find"),
    (0xff69, "Cancel"),
    (0xff6a, "Help"),
    (0xff6b, "Break"),
    (0xff7e, "Mode_switch"),
    (0xff7f, "Num_Lock"),
    (0xff80, "KP_Space"),
    (0xff89, "KP_Tab"),
    (0xff8d, "KP_Enter"),
    (0xff91, "KP_F1"),
    (0xff92, "KP_F2"),
    (0xff93, "KP_F3"),
    (0xff94, "KP_F4"),
    (0xff95, "KP_Home"),
    (0xff96, "KP_Left"),
    (0xff97, "KP_Up"),
    (0xff98, "KP_Right"),
    (0xff99, "KP_Down"),
    (0xff9a, "KP_Prior"),
    (0xff9b, "KP_Next"),
    (0xff9c, "KP_End"),
    (0xff9d, "KP_Begin"),
    (0xff9e, "KP_Insert"),
    (0xff9f, "KP_Delete"),
    (0xffaa, "KP_Multiply"),
    (0xffab, "KP_Add"),
    (0xffac, "KP_Separator"),
    (0xffad, "KP_Subtract"),
    (0xffae, "KP_Decimal"),
    (0xffaf, "KP_Divide"),
    (0xffb0, "KP_0"),
    (0xffb1, "KP_1"),
    (0xffb2, "KP_2"),
    (0xffb3, "KP_3"),
    (0xffb4, "KP_4"),
    (0xffb5, "KP_5"),
    (0xffb6, "KP_6"),
    (0xffb7, "KP_7"),
    (0xffb8, "KP_8"),
    (0xffb9, "KP_9"),
    (0xffbd, "KP_Equal"),
    (0xffe1, "Shift_L"),
    (0xffe2, "Shift_R"),
    (0xffe3, "Control_L"),
    (0xffe4, "Control_R"),
    (0xffe5, "Caps_Lock"),
    (0xffe6, "Shift_Lock"),
    (0xffe7, "Meta_L"),
    (0xffe8, "Meta_R"),
    (0xffe9, "Alt_L"),
    (0xffea, "Alt_R"),
    (0xffeb, "Super_L"),
    (0xffec, "Super_R"),
    (0xffed, "Hyper_L"),
    (0xffee, "Hyper_R"),
    (0xffff, "Delete"),
    (0xfe03, "ISO_Level3_Shift"),
    (0xfe11, "ISO_Level5_Shift"),
    (0xfe20, "ISO_Left_Tab"),
    (0x1008ff02, "XF86MonBrightnessUp"),
    (0x1008ff03, "XF86MonBrightnessDown"),
    (0x1008ff04, "XF86KbdLightOnOff"),
    (0x1008ff05, "XF86KbdBrightnessUp"),
    (0x1008ff06, "XF86KbdBrightnessDown"),
    (0x1008ff10, "XF86Standby"),
    (0x1008ff11, "XF86AudioLowerVolume"),
    (0x1008ff12, "XF86AudioMute"),
    (0x1008ff13, "XF86AudioRaiseVolume"),
    (0x1008ff14, "XF86AudioPlay"),
    (0x1008ff15, "XF86AudioStop"),
    (0x1008ff16, "XF86AudioPrev"),
    (0x1008ff17, "XF86AudioNext"),
    (0x1008ff18, "XF86HomePage"),
    (0x1008ff19, "XF86Mail"),
    (0x1008ff1b, "XF86Search"),
    (0x1008ff1c, "XF86AudioRecord"),
    (0x1008ff1d, "XF86Calculator"),
    (0x1008ff26, "XF86Back"),
    (0x1008ff27, "XF86Forward"),
    (0x1008ff28, "XF86Stop"),
    (0x1008ff29, "XF86Refresh"),
    (0x1008ff2a, "XF86PowerOff"),
    (0x1008ff2b, "XF86WakeUp"),
    (0x1008ff2c, "XF86Eject"),
    (0x1008ff2d, "XF86ScreenSaver"),
    (0x1008ff2e, "XF86WWW"),
    (0x1008ff2f, "XF86Sleep"),
    (0x1008ff30, "XF86Favorites"),
    (0x1008ff31, "XF86AudioPause"),
    (0x1008ff32, "XF86AudioMedia"),
    (0x1008ff33, "XF86MyComputer"),
    (0x1008ff3e, "XF86AudioRewind"),
    (0x1008ff59, "XF86Display"),
    (0x1008ff5d, "XF86Explorer"),
    (0x1008ff81, "XF86Tools"),
    (0x1008ff8f, "XF86WebCam"),
    (0x1008ff93, "XF86Battery"),
    (0x1008ff94, "XF86Bluetooth"),
    (0x1008ff95, "XF86WLAN"),
    (0x1008ff97, "XF86AudioForward"),
    (0x1008ffa9, "XF86TouchpadToggle"),
    (0x1008ffb2, "XF86AudioMicMute"),
];

const KEYSYM_F1: u32 = 0xffbe;

/// Keysym names with a Unicode mapping (`src/xkb_keysyms.txt`).
static UNICODE_KEYSYMS: LazyLock<Vec<(&'static str, char)>> = LazyLock::new(|| {
    include_str!("../xkb_keysyms.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let (name, value) = line.split_once(' ')?;
            Some((name, char::from_u32(u32::from_str_radix(value, 16).ok()?)?))
        })
        .collect()
});

static UNICODE_NAMES: LazyLock<HashMap<char, &'static str>> = LazyLock::new(|| {
    let mut names = HashMap::new();
    for &(name, character) in UNICODE_KEYSYMS.iter() {
        names.entry(character).or_insert(name);
    }
    names
});

/// The keysym a Unicode character has in an X keymap: Latin-1 keysyms equal
/// their code point, everything else uses the 0x01000000 Unicode range.
fn char_keysym(character: char) -> u32 {
    let code = u32::from(character);
    if (0x20..=0x7e).contains(&code) || (0xa0..=0xff).contains(&code) {
        code
    } else {
        0x0100_0000 | code
    }
}

/// X keysym name for display (`a`, `Return`, `Super_L`, `U20AC`, `0x...`).
pub fn keysym_name(keysym: u32) -> String {
    if let Some((_, name)) = NAMED_KEYSYMS.iter().find(|(value, _)| *value == keysym) {
        return (*name).to_owned();
    }
    if (KEYSYM_F1..KEYSYM_F1 + 35).contains(&keysym) {
        return format!("F{}", keysym - KEYSYM_F1 + 1);
    }
    let code = match keysym {
        0x20..=0x7e | 0xa0..=0xff => Some(keysym),
        0x0100_0100..=0x0110_ffff => Some(keysym & 0x00ff_ffff),
        _ => None,
    };
    match code.and_then(char::from_u32) {
        Some(character) => UNICODE_NAMES.get(&character).map_or_else(
            || format!("U{:04X}", u32::from(character)),
            |name| (*name).to_owned(),
        ),
        None => format!("{keysym:#x}"),
    }
}

/// Every keysym whose whykey-normalized name equals the normalized `key`
/// (`T` → `t`/`T`, `RETURN` → `Return`/`KP_Enter`, `F5` → `F5`, ...).
fn keysyms_for_key(key: &str) -> Vec<u32> {
    fn push(keysyms: &mut Vec<u32>, keysym: u32) {
        if !keysyms.contains(&keysym) {
            keysyms.push(keysym);
        }
    }
    let mut keysyms = Vec::new();
    let mut characters = key.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        push(&mut keysyms, char_keysym(character));
        for lower in character.to_lowercase() {
            push(&mut keysyms, char_keysym(lower));
        }
        for upper in character.to_uppercase() {
            push(&mut keysyms, char_keysym(upper));
        }
    }
    for &(value, name) in NAMED_KEYSYMS {
        if crate::key::normalize_key_str(name) == key {
            push(&mut keysyms, value);
        }
    }
    if let Some(number) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=35).contains(&number) {
            push(&mut keysyms, KEYSYM_F1 + number - 1);
        }
    }
    if keysyms.is_empty() {
        for &(name, character) in UNICODE_KEYSYMS.iter() {
            if crate::key::normalize_key_str(name) == key {
                push(&mut keysyms, char_keysym(character));
            }
        }
    }
    keysyms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_keys_to_keysyms() {
        assert_eq!(keysyms_for_key("T"), vec![0x54, 0x74]);
        assert_eq!(keysyms_for_key("RETURN"), vec![0xff0d, 0xff8d]);
        assert_eq!(keysyms_for_key("PAGE_UP"), vec![0xff55, 0xff9a]);
        assert_eq!(keysyms_for_key("F12"), vec![0xffc9]);
        assert_eq!(keysyms_for_key("SUPER_L"), vec![0xffeb]);
        assert_eq!(keysyms_for_key("XF86AUDIOMUTE"), vec![0x1008ff12]);
        assert_eq!(keysyms_for_key("!"), vec![0x21]);
        assert_eq!(keysyms_for_key("€"), vec![0x0100_20ac]);
        assert!(keysyms_for_key("NOT_A_KEY").is_empty());
    }

    #[test]
    fn names_keysyms() {
        assert_eq!(keysym_name(0x74), "t");
        assert_eq!(keysym_name(0xff0d), "Return");
        assert_eq!(keysym_name(0xffeb), "Super_L");
        assert_eq!(keysym_name(0xffbe), "F1");
        assert_eq!(keysym_name(0x0100_20ac), "EuroSign");
        assert_eq!(keysym_name(0x0712_3456), "0x7123456");
    }

    #[test]
    fn keymap_targets_follow_shift_column() {
        let keymap = Keymap {
            min_keycode: 8,
            per_keycode: 2,
            // keycode 8: 1 / !, keycode 9: t / T
            keysyms: vec![0x31, 0x21, 0x74, 0x54],
        };
        assert_eq!(
            keymap.targets(&keysyms_for_key("T")),
            vec![KeyTarget {
                keycode: 9,
                level_modifiers: 0
            }]
        );
        assert_eq!(
            keymap.targets(&keysyms_for_key("!")),
            vec![KeyTarget {
                keycode: 8,
                level_modifiers: SHIFT_MASK
            }]
        );
    }

    #[test]
    fn modifier_map_finds_num_lock() {
        let keymap = Keymap {
            min_keycode: 8,
            per_keycode: 1,
            keysyms: vec![0xffe1, KEYSYM_NUM_LOCK],
        };
        let modifiers = ModifierMap {
            per_modifier: 1,
            keycodes: vec![8, 0, 0, 0, 9, 0, 0, 0],
        };
        assert_eq!(modifiers.modifier_bit(8), Some(SHIFT_MASK));
        assert_eq!(modifiers.num_lock_mask(&keymap), Some(16));
        assert_eq!(modifiers.modifier_bit(0), None);
    }

    #[test]
    fn grab_status_needs_a_conflict() {
        let mut check = GrabCheck {
            keycode: 28,
            modmask: 64,
            core_conflicts: Vec::new(),
            xi2_conflicts: None,
        };
        assert_eq!(check.status(), GrabStatus::Free);
        check.xi2_conflicts = Some(vec![64]);
        assert_eq!(check.status(), GrabStatus::GrabbedByOtherClient);
    }

    /// Lab-only: needs a live X server on `DISPLAY`. Run inside a lab
    /// container with `--ignored`.
    #[test]
    #[ignore = "needs an X server on DISPLAY (lab container)"]
    fn lab_second_client_grab_is_detected() {
        let holder = Connection::connect_from_env().unwrap();
        let mut prober = Connection::connect_from_env().unwrap();
        let key: KeyCombo = "super+F7".parse().unwrap();
        let target = prober.key_targets(&key).unwrap()[0];
        assert_eq!(
            prober.check_grab(target.keycode, 64).unwrap().status(),
            GrabStatus::Free
        );
        holder
            .conn
            .grab_key(
                false,
                holder.root,
                xproto::ModMask::M4,
                target.keycode,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
            )
            .unwrap()
            .check()
            .unwrap();
        let check = prober.check_grab(target.keycode, 64).unwrap();
        assert_eq!(check.core_conflicts, vec![64]);
        assert_eq!(check.status(), GrabStatus::GrabbedByOtherClient);
    }

    #[test]
    #[ignore = "needs an X server with XInput2 on DISPLAY (lab container)"]
    fn lab_second_client_xi2_grab_is_detected() {
        let mut holder = Connection::connect_from_env().unwrap();
        let mut prober = Connection::connect_from_env().unwrap();
        let key: KeyCombo = "ctrl+F8".parse().unwrap();
        let target = prober.key_targets(&key).unwrap()[0];
        assert!(holder.xinput2_available().unwrap(), "XInput2");
        let reply = holder
            .conn
            .xinput_xi_passive_grab_device(
                x11rb::CURRENT_TIME,
                holder.root,
                x11rb::NONE,
                u32::from(target.keycode),
                XI_VIRTUAL_CORE_KEYBOARD,
                xinput::GrabType::KEYCODE,
                xinput::GrabMode22::ASYNC,
                xproto::GrabMode::ASYNC,
                xinput::GrabOwner::NO_OWNER,
                &[u32::from(
                    xinput::XIEventMask::KEY_PRESS | xinput::XIEventMask::KEY_RELEASE,
                )],
                &[4],
            )
            .unwrap()
            .reply()
            .unwrap();
        assert!(reply.modifiers.is_empty());
        let check = prober.check_grab(target.keycode, 4).unwrap();
        assert!(check.core_conflicts.is_empty());
        assert_eq!(check.xi2_conflicts, Some(vec![4]));
    }

    /// Lab-only: run under Xvfb + i3 with `bindsym Mod4+t ...` and xdotool.
    #[test]
    #[ignore = "needs Xvfb, a window manager binding Mod4+t, and xdotool (lab container)"]
    fn lab_probe_window_receives_free_keys_only() {
        fn drain(window: &mut X11Window) -> Vec<ProbeEvent> {
            let mut events = Vec::new();
            while let Some(event) = window.next_event(Duration::from_millis(500)).unwrap() {
                events.push(event);
            }
            events
        }
        fn xdotool(keys: &str) {
            let status = std::process::Command::new("xdotool")
                .args(["key", keys])
                .status()
                .unwrap();
            assert!(status.success());
        }
        let mut window = open_x11_window("whykey probe").unwrap();
        drain(&mut window);
        assert!(window.focused(), "probe window should hold keyboard focus");

        xdotool("a");
        let events = drain(&mut window);
        eprintln!("a: {events:?}");
        assert!(events
            .iter()
            .any(|event| event.kind == ProbeKind::Press && event.keysym.as_deref() == Some("a")));

        xdotool("super+t");
        let events = drain(&mut window);
        eprintln!("super+t: {events:?}");
        // Openbox lets the `t` release through after its grab ends; only a
        // press proves delivery.
        assert!(!events
            .iter()
            .any(|event| event.kind == ProbeKind::Press && event.keysym.as_deref() == Some("t")));
        assert!(
            events
                .iter()
                .any(|event| event.kind == ProbeKind::Modifiers && event.modmask & 64 != 0)
        );
        // Even when the window manager's grab swallows the Super release,
        // the probe reports the modifiers returning to zero.
        let last = events
            .iter()
            .rev()
            .find(|event| event.kind == ProbeKind::Modifiers)
            .unwrap();
        assert_eq!(last.modmask, 0);
    }
}
