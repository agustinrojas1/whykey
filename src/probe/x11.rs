//! Minimal X11 wire-protocol client (no Xlib/xcb dependency).
//!
//! It speaks just enough of the core protocol (plus XInput2 passive grabs)
//! to open a probe window that reports the keys the X server delivers to it,
//! and to ask the server whether another client already holds a passive key
//! grab. A passive grab is how every X11 window manager and hotkey daemon
//! (i3, Openbox, xbindkeys, sxhkd, KWin, Mutter, ...) claims a shortcut, and
//! the server refuses a second client's grab on the same combination with
//! `BadAccess`. That refusal is observed server state, not a prediction from
//! configuration files.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::os::fd::{AsRawFd, RawFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::key::KeyCombo;
use crate::probe::{ProbeEvent, ProbeKind, ProbeWindow};

/// Bound on every request/reply exchange and on connection setup.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Largest reply/setup payload accepted from the server.
const MAX_PACKET: usize = 4 << 20;
/// Largest Xauthority file read.
const MAX_XAUTHORITY: u64 = 1 << 20;
/// Events buffered while waiting for a reply.
const MAX_QUEUED_EVENTS: usize = 4096;

pub const SHIFT_MASK: u16 = 1;
pub const LOCK_MASK: u16 = 2;

const OP_CREATE_WINDOW: u8 = 1;
const OP_MAP_WINDOW: u8 = 8;
const OP_INTERN_ATOM: u8 = 16;
const OP_CHANGE_PROPERTY: u8 = 18;
const OP_GRAB_KEY: u8 = 33;
const OP_UNGRAB_KEY: u8 = 34;
const OP_QUERY_POINTER: u8 = 38;
const OP_SET_INPUT_FOCUS: u8 = 42;
const OP_GET_INPUT_FOCUS: u8 = 43;
const OP_QUERY_EXTENSION: u8 = 98;
const OP_GET_KEYBOARD_MAPPING: u8 = 101;
const OP_GET_MODIFIER_MAPPING: u8 = 119;

const XI_QUERY_VERSION: u8 = 47;
const XI_PASSIVE_GRAB_DEVICE: u8 = 54;
const XI_PASSIVE_UNGRAB_DEVICE: u8 = 55;
/// The X server's virtual core keyboard (always device 3); Mutter grabs on it.
const XI_VIRTUAL_CORE_KEYBOARD: u16 = 3;
const XI_GRABTYPE_KEYCODE: u8 = 1;
const XI_GRAB_MODE_ASYNC: u8 = 1;

const ERROR_ACCESS: u8 = 10;
const ERROR_NAMES: [&str; 18] = [
    "Success",
    "BadRequest",
    "BadValue",
    "BadWindow",
    "BadPixmap",
    "BadAtom",
    "BadCursor",
    "BadFont",
    "BadMatch",
    "BadDrawable",
    "BadAccess",
    "BadAlloc",
    "BadColormap",
    "BadGContext",
    "BadIDChoice",
    "BadName",
    "BadLength",
    "BadImplementation",
];

const EVENT_KEY_PRESS: u8 = 2;
const EVENT_KEY_RELEASE: u8 = 3;
const EVENT_FOCUS_IN: u8 = 9;
const EVENT_FOCUS_OUT: u8 = 10;
const EVENT_MAP_NOTIFY: u8 = 19;
const EVENT_MAPPING_NOTIFY: u8 = 34;
const EVENT_GENERIC: u8 = 35;

const MASK_KEY_PRESS: u32 = 1 << 0;
const MASK_KEY_RELEASE: u32 = 1 << 1;
const MASK_STRUCTURE_NOTIFY: u32 = 1 << 17;
const MASK_FOCUS_CHANGE: u32 = 1 << 21;

const ATOM_CARDINAL: u32 = 6;
const ATOM_STRING: u32 = 31;
const ATOM_WM_HINTS: u32 = 35;
const ATOM_WM_NAME: u32 = 39;
const ATOM_WM_CLASS: u32 = 67;

const FAMILY_INTERNET: u16 = 0;
const FAMILY_INTERNET6: u16 = 6;
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;
const MIT_MAGIC_COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";

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

fn malformed(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("malformed X11 {what} from the server"),
    )
}

fn le16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn pad4(length: usize) -> usize {
    (4 - length % 4) % 4
}

// ---------------------------------------------------------------------------
// DISPLAY and Xauthority

#[derive(Debug, Clone, PartialEq, Eq)]
enum Endpoint {
    /// Local server: `/tmp/.X11-unix/X<n>`, then the abstract socket.
    Local,
    /// Explicit Unix socket path (`/path/to/socket:N`).
    Path(String),
    Tcp(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisplayName {
    endpoint: Endpoint,
    display: u32,
    screen: usize,
}

fn parse_display(value: &str) -> Option<DisplayName> {
    let value = value.trim();
    let (protocol, rest) = match value.split_once('/') {
        Some((protocol, rest))
            if matches!(protocol, "unix" | "local" | "tcp" | "inet" | "inet6") =>
        {
            (Some(protocol), rest)
        }
        _ => (None, value),
    };
    let (host, number) = rest.rsplit_once(':')?;
    let (display, screen) = match number.split_once('.') {
        Some((display, screen)) => (display, screen.parse().ok()?),
        None => (number, 0),
    };
    let display = display.parse().ok()?;
    let endpoint = match protocol {
        Some("unix" | "local") => Endpoint::Local,
        Some(_) if host.is_empty() => return None,
        Some(_) => Endpoint::Tcp(host.trim_matches(['[', ']']).to_owned()),
        None if host.is_empty() || host == "unix" => Endpoint::Local,
        None if host.starts_with('/') => Endpoint::Path(host.to_owned()),
        None => Endpoint::Tcp(host.trim_matches(['[', ']']).to_owned()),
    };
    Some(DisplayName {
        endpoint,
        display,
        screen,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XauthEntry {
    family: u16,
    address: Vec<u8>,
    number: Vec<u8>,
    name: Vec<u8>,
    data: Vec<u8>,
}

fn parse_xauthority(bytes: &[u8]) -> Vec<XauthEntry> {
    fn counted<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(*offset)?,
            *bytes.get(*offset + 1)?,
        ]));
        let start = *offset + 2;
        let field = bytes.get(start..start + length)?;
        *offset = start + length;
        Some(field)
    }
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(family) = bytes
            .get(offset..offset + 2)
            .map(|family| u16::from_be_bytes([family[0], family[1]]))
        else {
            break;
        };
        offset += 2;
        let mut field = || counted(bytes, &mut offset).map(<[u8]>::to_vec);
        let (Some(address), Some(number), Some(name), Some(data)) =
            (field(), field(), field(), field())
        else {
            break;
        };
        entries.push(XauthEntry {
            family,
            address,
            number,
            name,
            data,
        });
    }
    entries
}

/// The server address as Xauthority families describe it.
enum AuthAddress {
    Local(Vec<u8>),
    Ip(IpAddr),
}

fn select_cookie(
    entries: &[XauthEntry],
    address: &AuthAddress,
    display: u32,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let number = display.to_string();
    entries
        .iter()
        .find(|entry| {
            let address_matches = match (entry.family, address) {
                (FAMILY_WILD, _) => true,
                (FAMILY_LOCAL, AuthAddress::Local(hostname)) => entry.address == *hostname,
                (FAMILY_INTERNET, AuthAddress::Ip(IpAddr::V4(ip))) => entry.address == ip.octets(),
                (FAMILY_INTERNET6, AuthAddress::Ip(IpAddr::V6(ip))) => entry.address == ip.octets(),
                _ => false,
            };
            address_matches
                && (entry.number.is_empty() || entry.number == number.as_bytes())
                && entry.name == MIT_MAGIC_COOKIE
        })
        .map(|entry| (entry.name.clone(), entry.data.clone()))
}

fn xauthority_path() -> Option<PathBuf> {
    match env::var_os("XAUTHORITY") {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")),
    }
}

fn read_xauthority() -> Vec<XauthEntry> {
    let Some(path) = xauthority_path() else {
        return Vec::new();
    };
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file.take(MAX_XAUTHORITY).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    parse_xauthority(&bytes)
}

fn hostname() -> Vec<u8> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its full length; gethostname writes at
    // most that many bytes.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if status != 0 {
        return Vec::new();
    }
    let length = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    buffer[..length].to_vec()
}

// ---------------------------------------------------------------------------
// Transport

enum Transport {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Transport {
    fn connect(display: &DisplayName) -> io::Result<(Self, AuthAddress)> {
        let local = || AuthAddress::Local(hostname());
        match &display.endpoint {
            Endpoint::Local => {
                let path = format!("/tmp/.X11-unix/X{}", display.display);
                let stream = UnixStream::connect(&path).or_else(|path_error| {
                    SocketAddr::from_abstract_name(path.as_bytes())
                        .and_then(|address| UnixStream::connect_addr(&address))
                        .map_err(|_| {
                            io::Error::new(path_error.kind(), format!("{path}: {path_error}"))
                        })
                })?;
                Ok((Self::Unix(stream), local()))
            }
            Endpoint::Path(path) => {
                let stream = UnixStream::connect(path)
                    .map_err(|error| io::Error::new(error.kind(), format!("{path}: {error}")))?;
                Ok((Self::Unix(stream), local()))
            }
            Endpoint::Tcp(host) => {
                let port = u16::try_from(6000 + display.display).map_err(|_| {
                    invalid_input(format!("X11 display {} is too large", display.display))
                })?;
                let mut last_error = None;
                for address in (host.as_str(), port).to_socket_addrs()? {
                    match TcpStream::connect_timeout(&address, IO_TIMEOUT) {
                        Ok(stream) => {
                            let ip = address.ip();
                            let auth = if ip.is_loopback() {
                                local()
                            } else {
                                AuthAddress::Ip(ip)
                            };
                            return Ok((Self::Tcp(stream), auth));
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                Err(last_error.unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"))
                }))
            }
        }
    }

    fn set_write_timeout(&self, timeout: Duration) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_write_timeout(Some(timeout)),
            Self::Tcp(stream) => stream.set_write_timeout(Some(timeout)),
        }
    }
}

impl AsRawFd for Transport {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Unix(stream) => stream.as_raw_fd(),
            Self::Tcp(stream) => stream.as_raw_fd(),
        }
    }
}

impl Read for Transport {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.read(buffer),
            Self::Tcp(stream) => stream.read(buffer),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.write(buffer),
            Self::Tcp(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

/// Waits until `fd` is readable or `deadline` passes (`Ok(false)`).
fn wait_readable(fd: RawFd, deadline: Instant) -> io::Result<bool> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let millis = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` points to one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready > 0 {
            return Ok(true);
        }
    }
}

// ---------------------------------------------------------------------------
// Requests and packets

struct Request(Vec<u8>);

impl Request {
    fn new(opcode: u8, data: u8) -> Self {
        Self(vec![opcode, data, 0, 0])
    }

    fn u8(&mut self, value: u8) -> &mut Self {
        self.0.push(value);
        self
    }

    fn u16(&mut self, value: u16) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn u32(&mut self, value: u32) -> &mut Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn padded(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self.0.resize(self.0.len() + pad4(bytes.len()), 0);
        self
    }

    fn finish(&mut self) -> io::Result<Vec<u8>> {
        let padding = pad4(self.0.len());
        self.0.resize(self.0.len() + padding, 0);
        let units = u16::try_from(self.0.len() / 4)
            .map_err(|_| invalid_input("X11 request is too long".into()))?;
        self.0[2..4].copy_from_slice(&units.to_le_bytes());
        Ok(std::mem::take(&mut self.0))
    }
}

/// An X protocol error the server reported for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct XError {
    code: u8,
    sequence: u16,
    major: u8,
    minor: u16,
}

impl XError {
    fn into_io(self) -> io::Error {
        let name = ERROR_NAMES
            .get(usize::from(self.code))
            .copied()
            .unwrap_or("extension error");
        io::Error::other(format!(
            "X11 request {}.{} failed: {name} ({})",
            self.major, self.minor, self.code
        ))
    }
}

enum Packet {
    Reply { sequence: u16, bytes: Vec<u8> },
    Error(XError),
    Event([u8; 32]),
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
    transport: Transport,
    sequence: u16,
    root: u32,
    white_pixel: u32,
    id_base: u32,
    id_mask: u32,
    id_next: u32,
    min_keycode: u8,
    max_keycode: u8,
    events: VecDeque<[u8; 32]>,
    errors: Vec<XError>,
    xinput: Option<Option<u8>>,
}

impl Connection {
    /// Connects to `$DISPLAY`, authenticating with the matching
    /// MIT-MAGIC-COOKIE-1 from `$XAUTHORITY`/`~/.Xauthority` when present.
    pub fn connect_from_env() -> io::Result<Self> {
        let value = env::var("DISPLAY")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "DISPLAY is not set"))?;
        let display = parse_display(&value).ok_or_else(|| {
            invalid_input(format!("DISPLAY '{value}' is not a valid X11 display"))
        })?;
        let (transport, address) = Transport::connect(&display)?;
        let cookie = select_cookie(&read_xauthority(), &address, display.display);
        Self::setup(transport, cookie, display.screen)
    }

    fn setup(
        transport: Transport,
        cookie: Option<(Vec<u8>, Vec<u8>)>,
        screen: usize,
    ) -> io::Result<Self> {
        transport.set_write_timeout(IO_TIMEOUT)?;
        let mut connection = Self {
            transport,
            sequence: 0,
            root: 0,
            white_pixel: 0,
            id_base: 0,
            id_mask: 0,
            id_next: 0,
            min_keycode: 0,
            max_keycode: 0,
            events: VecDeque::new(),
            errors: Vec::new(),
            xinput: None,
        };
        let (name, data) = cookie.unwrap_or_default();
        let mut request = vec![b'l', 0];
        request.extend_from_slice(&11u16.to_le_bytes());
        request.extend_from_slice(&0u16.to_le_bytes());
        request.extend_from_slice(&(name.len() as u16).to_le_bytes());
        request.extend_from_slice(&(data.len() as u16).to_le_bytes());
        request.extend_from_slice(&[0, 0]);
        request.extend_from_slice(&name);
        request.resize(request.len() + pad4(name.len()), 0);
        request.extend_from_slice(&data);
        request.resize(request.len() + pad4(data.len()), 0);
        connection.transport.write_all(&request)?;

        let deadline = Instant::now() + IO_TIMEOUT;
        let mut header = [0u8; 8];
        connection.read_exact_until(&mut header, deadline)?;
        let extra = usize::from(u16::from_le_bytes([header[6], header[7]])) * 4;
        let mut body = vec![0u8; extra];
        connection.read_exact_until(&mut body, deadline)?;
        match header[0] {
            1 => {}
            status => {
                let reason_length = if status == 0 {
                    usize::from(header[1])
                } else {
                    body.len()
                };
                let reason = String::from_utf8_lossy(&body[..reason_length.min(body.len())]);
                let reason = reason.trim_end_matches('\0').trim();
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("X server refused the connection: {reason}"),
                ));
            }
        }
        connection.parse_setup(&body, screen)?;
        Ok(connection)
    }

    fn parse_setup(&mut self, body: &[u8], screen: usize) -> io::Result<()> {
        let setup = || malformed("connection setup");
        self.id_base = le32(body, 4).ok_or_else(setup)?;
        self.id_mask = le32(body, 8).ok_or_else(setup)?;
        let vendor_length = usize::from(le16(body, 16).ok_or_else(setup)?);
        let screens = usize::from(*body.get(20).ok_or_else(setup)?);
        let formats = usize::from(*body.get(21).ok_or_else(setup)?);
        self.min_keycode = *body.get(26).ok_or_else(setup)?;
        self.max_keycode = *body.get(27).ok_or_else(setup)?;
        if self.id_mask == 0 || self.min_keycode > self.max_keycode {
            return Err(setup());
        }
        if screen >= screens {
            return Err(invalid_input(format!(
                "X11 screen {screen} does not exist (the server has {screens})"
            )));
        }
        let mut offset = 32 + vendor_length + pad4(vendor_length) + 8 * formats;
        for index in 0..=screen {
            let depths = usize::from(*body.get(offset + 39).ok_or_else(setup)?);
            if index == screen {
                self.root = le32(body, offset).ok_or_else(setup)?;
                self.white_pixel = le32(body, offset + 8).ok_or_else(setup)?;
            }
            offset += 40;
            for _ in 0..depths {
                let visuals = usize::from(le16(body, offset + 2).ok_or_else(setup)?);
                offset += 8 + 24 * visuals;
            }
        }
        Ok(())
    }

    fn generate_id(&mut self) -> io::Result<u32> {
        let step = self.id_mask & self.id_mask.wrapping_neg();
        let id = self.id_next;
        if id & !self.id_mask != 0 || (id != 0 && id & self.id_mask == 0) {
            return Err(io::Error::other("X11 resource IDs exhausted"));
        }
        self.id_next = self.id_next.wrapping_add(step);
        Ok(self.id_base | id)
    }

    /// Reads exactly `buffer.len()` bytes before `deadline`.
    fn read_exact_until(&mut self, buffer: &mut [u8], deadline: Instant) -> io::Result<()> {
        if !self.read_until(buffer, deadline, false)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "X server did not answer in time",
            ));
        }
        Ok(())
    }

    /// Fills `buffer`; `Ok(false)` when idle until `deadline` and nothing was
    /// read (only allowed when `idle_ok`).
    fn read_until(
        &mut self,
        buffer: &mut [u8],
        deadline: Instant,
        idle_ok: bool,
    ) -> io::Result<bool> {
        let mut filled = 0;
        while filled < buffer.len() {
            if !wait_readable(self.transport.as_raw_fd(), deadline)? {
                if filled == 0 && idle_ok {
                    return Ok(false);
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "X server did not answer in time",
                ));
            }
            match self.transport.read(&mut buffer[filled..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "X server closed the connection",
                    ));
                }
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }

    fn read_packet(&mut self, deadline: Instant, idle_ok: bool) -> io::Result<Option<Packet>> {
        let mut header = [0u8; 32];
        if !self.read_until(&mut header, deadline, idle_ok)? {
            return Ok(None);
        }
        let sequence = u16::from_le_bytes([header[2], header[3]]);
        let kind = header[0] & 0x7f;
        let extra = if kind == 1 || kind == EVENT_GENERIC {
            let units = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            usize::try_from(units)
                .ok()
                .and_then(|units| units.checked_mul(4))
                .filter(|extra| *extra <= MAX_PACKET)
                .ok_or_else(|| malformed("reply length"))?
        } else {
            0
        };
        let mut bytes = header.to_vec();
        if extra > 0 {
            bytes.resize(32 + extra, 0);
            let deadline = deadline.max(Instant::now() + IO_TIMEOUT);
            self.read_exact_until(&mut bytes[32..], deadline)?;
        }
        Ok(Some(match header[0] {
            0 => Packet::Error(XError {
                code: header[1],
                sequence,
                minor: u16::from_le_bytes([header[8], header[9]]),
                major: header[10],
            }),
            1 => Packet::Reply { sequence, bytes },
            _ => Packet::Event(header),
        }))
    }

    fn queue_event(&mut self, event: [u8; 32]) {
        if self.events.len() >= MAX_QUEUED_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn send(&mut self, mut request: Request) -> io::Result<u16> {
        let bytes = request.finish()?;
        self.transport.write_all(&bytes)?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(self.sequence)
    }

    /// Waits for the reply to request `sequence`, queueing events and
    /// remembering errors for other requests.
    fn reply(&mut self, sequence: u16) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match self.read_packet(deadline, false)? {
                Some(Packet::Reply {
                    sequence: got,
                    bytes,
                }) if got == sequence => return Ok(bytes),
                Some(Packet::Reply { .. }) | None => {}
                Some(Packet::Error(error)) if error.sequence == sequence => {
                    return Err(error.into_io());
                }
                Some(Packet::Error(error)) => self.errors.push(error),
                Some(Packet::Event(event)) => self.queue_event(event),
            }
        }
    }

    fn request_reply(&mut self, request: Request) -> io::Result<Vec<u8>> {
        let sequence = self.send(request)?;
        self.reply(sequence)
    }

    /// Round trip: every earlier request is processed when this returns the
    /// errors they produced.
    fn sync(&mut self) -> io::Result<Vec<XError>> {
        let sequence = self.send(Request::new(OP_GET_INPUT_FOCUS, 0))?;
        self.reply(sequence)?;
        Ok(std::mem::take(&mut self.errors))
    }

    fn intern_atom(&mut self, name: &str) -> io::Result<u32> {
        let mut request = Request::new(OP_INTERN_ATOM, 0);
        request
            .u16(
                u16::try_from(name.len())
                    .map_err(|_| invalid_input("atom name too long".into()))?,
            )
            .u16(0)
            .padded(name.as_bytes());
        let reply = self.request_reply(request)?;
        le32(&reply, 8).ok_or_else(|| malformed("InternAtom reply"))
    }

    fn keyboard_mapping(&mut self) -> io::Result<Keymap> {
        let count = self.max_keycode - self.min_keycode + 1;
        let mut request = Request::new(OP_GET_KEYBOARD_MAPPING, 0);
        request.u8(self.min_keycode).u8(count).u16(0);
        let reply = self.request_reply(request)?;
        let per_keycode = usize::from(reply[1]);
        let total = per_keycode * usize::from(count);
        let keysyms = (0..total)
            .map(|index| le32(&reply, 32 + index * 4))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| malformed("GetKeyboardMapping reply"))?;
        Ok(Keymap {
            min_keycode: self.min_keycode,
            per_keycode,
            keysyms,
        })
    }

    fn modifier_mapping(&mut self) -> io::Result<ModifierMap> {
        let reply = self.request_reply(Request::new(OP_GET_MODIFIER_MAPPING, 0))?;
        let per_modifier = usize::from(reply[1]);
        let keycodes = reply
            .get(32..32 + 8 * per_modifier)
            .ok_or_else(|| malformed("GetModifierMapping reply"))?
            .to_vec();
        Ok(ModifierMap {
            per_modifier,
            keycodes,
        })
    }

    /// Keycodes that produce `key`'s keysym in the server's current keymap.
    /// `CODE:N` names keycode N directly.
    pub fn key_targets(&mut self, key: &KeyCombo) -> io::Result<Vec<KeyTarget>> {
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

    fn lock_variants(&mut self, modmask: u16) -> io::Result<Vec<u16>> {
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
        self.sync()?;

        let mut sequences = Vec::with_capacity(variants.len());
        for &variant in &variants {
            let mut request = Request::new(OP_GRAB_KEY, 0);
            request
                .u32(self.root)
                .u16(variant)
                .u8(keycode)
                .u8(1)
                .u8(1)
                .u8(0)
                .u16(0);
            sequences.push((self.send(request)?, variant));
        }
        let mut core_conflicts = Vec::new();
        for error in self.sync()? {
            let Some(&(_, variant)) = sequences
                .iter()
                .find(|(sequence, _)| *sequence == error.sequence)
            else {
                continue;
            };
            if error.code != ERROR_ACCESS {
                return Err(error.into_io());
            }
            core_conflicts.push(variant);
        }
        for &variant in &variants {
            if core_conflicts.contains(&variant) {
                continue;
            }
            let mut request = Request::new(OP_UNGRAB_KEY, keycode);
            request.u32(self.root).u16(variant).u16(0);
            self.send(request)?;
        }
        if let Some(error) = self.sync()?.into_iter().next() {
            return Err(error.into_io());
        }

        let xi2_conflicts = match self.xinput_opcode()? {
            Some(opcode) => Some(self.check_xi2_grab(opcode, keycode, &variants)?),
            None => None,
        };
        Ok(GrabCheck {
            keycode,
            modmask,
            core_conflicts,
            xi2_conflicts,
        })
    }

    /// The XInput extension's major opcode when the server supports XI 2.0.
    fn xinput_opcode(&mut self) -> io::Result<Option<u8>> {
        if let Some(opcode) = self.xinput {
            return Ok(opcode);
        }
        let name = b"XInputExtension";
        let mut request = Request::new(OP_QUERY_EXTENSION, 0);
        request.u16(name.len() as u16).u16(0).padded(name);
        let reply = self.request_reply(request)?;
        let opcode = match (reply.get(8), reply.get(9)) {
            (Some(1), Some(&opcode)) => {
                let mut request = Request::new(opcode, XI_QUERY_VERSION);
                request.u16(2).u16(0);
                match self.request_reply(request) {
                    Ok(reply) if le16(&reply, 8).is_some_and(|major| major >= 2) => Some(opcode),
                    Ok(_) => None,
                    Err(error) if error.kind() == io::ErrorKind::Other => None,
                    Err(error) => return Err(error),
                }
            }
            _ => None,
        };
        self.xinput = Some(opcode);
        Ok(opcode)
    }

    fn check_xi2_grab(
        &mut self,
        opcode: u8,
        keycode: u8,
        variants: &[u16],
    ) -> io::Result<Vec<u16>> {
        let count = u16::try_from(variants.len())
            .map_err(|_| invalid_input("too many modifier variants".into()))?;
        let mut request = Request::new(opcode, XI_PASSIVE_GRAB_DEVICE);
        request
            .u32(0) // time: CurrentTime
            .u32(self.root)
            .u32(0) // cursor: None
            .u32(u32::from(keycode))
            .u16(XI_VIRTUAL_CORE_KEYBOARD)
            .u16(count)
            .u16(1) // mask_len
            .u8(XI_GRABTYPE_KEYCODE)
            .u8(XI_GRAB_MODE_ASYNC)
            .u8(XI_GRAB_MODE_ASYNC)
            .u8(0) // owner_events
            .u16(0)
            .u32((1 << 2) | (1 << 3)); // XI_KeyPress | XI_KeyRelease
        for &variant in variants {
            request.u32(u32::from(variant));
        }
        let reply = self.request_reply(request)?;
        let failed =
            usize::from(le16(&reply, 8).ok_or_else(|| malformed("XIPassiveGrabDevice reply"))?);
        let mut conflicts = Vec::new();
        let mut held = variants.to_vec();
        for index in 0..failed {
            let offset = 32 + index * 8;
            let modifiers =
                le32(&reply, offset).ok_or_else(|| malformed("XIPassiveGrabDevice reply"))?;
            let status = *reply
                .get(offset + 4)
                .ok_or_else(|| malformed("XIPassiveGrabDevice reply"))?;
            let Some(variant) = variants
                .iter()
                .copied()
                .find(|variant| u32::from(*variant) == modifiers)
            else {
                continue;
            };
            held.retain(|held| *held != variant);
            if status == ERROR_ACCESS {
                conflicts.push(variant);
            } else {
                return Err(io::Error::other(format!(
                    "XInput2 passive grab failed with status {status}"
                )));
            }
        }
        if !held.is_empty() {
            let mut request = Request::new(opcode, XI_PASSIVE_UNGRAB_DEVICE);
            request
                .u32(self.root)
                .u32(u32::from(keycode))
                .u16(XI_VIRTUAL_CORE_KEYBOARD)
                .u16(held.len() as u16)
                .u8(XI_GRABTYPE_KEYCODE)
                .u8(0)
                .u16(0);
            for variant in held {
                request.u32(u32::from(variant));
            }
            self.send(request)?;
            if let Some(error) = self.sync()?.into_iter().next() {
                return Err(error.into_io());
            }
        }
        Ok(conflicts)
    }

    /// Next event before `deadline`, or `None`.
    fn next_event(&mut self, deadline: Instant) -> io::Result<Option<[u8; 32]>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        loop {
            match self.read_packet(deadline, true)? {
                None => return Ok(None),
                Some(Packet::Event(event)) => return Ok(Some(event)),
                Some(Packet::Error(error)) => self.errors.push(error),
                Some(Packet::Reply { .. }) => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Probe window

/// A whykey-owned X11 toplevel that reports the keys it receives.
pub struct X11Window {
    connection: Connection,
    window: u32,
    focused: bool,
    keymap: Keymap,
    modifiers: ModifierMap,
    pending: VecDeque<ProbeEvent>,
    /// Modifier state after the last reported event.
    modmask: u16,
}

impl X11Window {
    fn open(title: &str) -> io::Result<Self> {
        let mut connection = Connection::connect_from_env()?;
        let net_wm_name = connection.intern_atom("_NET_WM_NAME")?;
        let utf8_string = connection.intern_atom("UTF8_STRING")?;
        let net_wm_pid = connection.intern_atom("_NET_WM_PID")?;
        let keymap = connection.keyboard_mapping()?;
        let modifiers = connection.modifier_mapping()?;

        let window = connection.generate_id()?;
        let mut request = Request::new(OP_CREATE_WINDOW, 0);
        request
            .u32(window)
            .u32(connection.root)
            .u16(0)
            .u16(0)
            .u16(480)
            .u16(160)
            .u16(0)
            .u16(1) // InputOutput
            .u32(0) // visual: CopyFromParent
            .u32(0x2 | 0x800) // background-pixel | event-mask
            .u32(connection.white_pixel)
            .u32(MASK_KEY_PRESS | MASK_KEY_RELEASE | MASK_FOCUS_CHANGE | MASK_STRUCTURE_NOTIFY);
        connection.send(request)?;

        let latin1 = title
            .chars()
            .map(|character| u8::try_from(u32::from(character)).unwrap_or(b'?'))
            .collect::<Vec<_>>();
        let pid = std::process::id().to_le_bytes();
        let mut hints = Vec::with_capacity(36);
        hints.extend_from_slice(&1u32.to_le_bytes()); // flags: InputHint
        hints.extend_from_slice(&1u32.to_le_bytes()); // input: True
        hints.resize(36, 0);
        let properties: [(u32, u32, u8, &[u8]); 5] = [
            (ATOM_WM_NAME, ATOM_STRING, 8, &latin1),
            (net_wm_name, utf8_string, 8, title.as_bytes()),
            (ATOM_WM_CLASS, ATOM_STRING, 8, b"whykey-probe\0whykey\0"),
            (net_wm_pid, ATOM_CARDINAL, 32, &pid),
            (ATOM_WM_HINTS, ATOM_WM_HINTS, 32, &hints),
        ];
        for (property, kind, format, data) in properties {
            let units = data.len() / usize::from(format / 8);
            let mut request = Request::new(OP_CHANGE_PROPERTY, 0);
            request
                .u32(window)
                .u32(property)
                .u32(kind)
                .u8(format)
                .u8(0)
                .u16(0)
                .u32(units as u32)
                .padded(data);
            connection.send(request)?;
        }
        let mut request = Request::new(OP_MAP_WINDOW, 0);
        request.u32(window);
        connection.send(request)?;
        if let Some(error) = connection.sync()?.into_iter().next() {
            return Err(error.into_io());
        }

        let mut probe = Self {
            connection,
            window,
            focused: false,
            keymap,
            modifiers,
            pending: VecDeque::new(),
            modmask: 0,
        };
        // A window manager maps the window when it handles the MapRequest;
        // focus can only be set once the window is viewable.
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut deferred = Vec::new();
        let mut mapped = false;
        while let Some(event) = probe.connection.next_event(deadline)? {
            deferred.push(event);
            if event[0] & 0x7f == EVENT_MAP_NOTIFY && le32(&event, 8) == Some(window) {
                mapped = true;
                break;
            }
        }
        for event in deferred.into_iter().rev() {
            probe.connection.events.push_front(event);
        }
        if !mapped {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the X11 probe window was not mapped in time",
            ));
        }
        let mut request = Request::new(OP_SET_INPUT_FOCUS, 1); // revert-to PointerRoot
        request.u32(window).u32(0);
        probe.connection.send(request)?;
        // A window manager may refuse focus changes; the focus state below
        // and later FocusIn/FocusOut events report what actually happened.
        probe.connection.sync()?;
        let reply = probe
            .connection
            .request_reply(Request::new(OP_GET_INPUT_FOCUS, 0))?;
        probe.focused = le32(&reply, 8) == Some(window);
        Ok(probe)
    }

    fn refresh_keymap(&mut self) -> io::Result<()> {
        self.keymap = self.connection.keyboard_mapping()?;
        self.modifiers = self.connection.modifier_mapping()?;
        Ok(())
    }

    fn handle(&mut self, event: [u8; 32]) -> io::Result<()> {
        match event[0] & 0x7f {
            kind @ (EVENT_KEY_PRESS | EVENT_KEY_RELEASE) => {
                if le32(&event, 12) != Some(self.window) {
                    return Ok(());
                }
                let keycode = event[1];
                let state = le16(&event, 28).unwrap_or(0) & 0xff;
                let press = kind == EVENT_KEY_PRESS;
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
            EVENT_FOCUS_IN | EVENT_FOCUS_OUT => {
                // detail: NotifyInferior 2, NotifyPointer 5 (focus did not
                // really move to/from this window).
                if le32(&event, 4) != Some(self.window) || matches!(event[1], 2 | 5) {
                    return Ok(());
                }
                let focus_in = event[0] & 0x7f == EVENT_FOCUS_IN;
                self.focused = focus_in;
                if focus_in {
                    // Key events delivered elsewhere while focus was away
                    // (including a window manager's grab, which sends
                    // FocusOut/FocusIn with NotifyGrab/NotifyUngrab) may
                    // have changed the modifiers; report the real state.
                    let mut request = Request::new(OP_QUERY_POINTER, 0);
                    request.u32(self.window);
                    let reply = self.connection.request_reply(request)?;
                    let state =
                        le16(&reply, 24).ok_or_else(|| malformed("QueryPointer reply"))? & 0xff;
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
            EVENT_MAPPING_NOTIFY => self.refresh_keymap()?,
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
            match self.connection.next_event(deadline)? {
                Some(event) => self.handle(event)?,
                None => return Ok(None),
            }
        }
    }

    fn focused(&self) -> bool {
        self.focused
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
    fn parses_display_forms() {
        let local = parse_display(":5").unwrap();
        assert_eq!(local.endpoint, Endpoint::Local);
        assert_eq!((local.display, local.screen), (5, 0));
        assert_eq!(parse_display("unix:0.1").unwrap().screen, 1);
        assert_eq!(
            parse_display("localhost:10.0").unwrap().endpoint,
            Endpoint::Tcp("localhost".into())
        );
        assert_eq!(
            parse_display("tcp/host:2").unwrap().endpoint,
            Endpoint::Tcp("host".into())
        );
        assert_eq!(
            parse_display("/tmp/launch-x/org.xquartz:0")
                .unwrap()
                .endpoint,
            Endpoint::Path("/tmp/launch-x/org.xquartz".into())
        );
        assert!(parse_display("").is_none());
        assert!(parse_display(":x").is_none());
        assert!(parse_display("wayland-1").is_none());
    }

    fn xauth_entry(family: u16, address: &[u8], number: &[u8], data: &[u8]) -> Vec<u8> {
        let mut bytes = family.to_be_bytes().to_vec();
        for field in [address, number, MIT_MAGIC_COOKIE, data] {
            bytes.extend_from_slice(&(field.len() as u16).to_be_bytes());
            bytes.extend_from_slice(field);
        }
        bytes
    }

    #[test]
    fn selects_cookie_for_host_and_display() {
        let mut file = xauth_entry(FAMILY_LOCAL, b"other", b"5", b"wrong-host");
        file.extend(xauth_entry(FAMILY_LOCAL, b"lab", b"4", b"wrong-display"));
        file.extend(xauth_entry(FAMILY_LOCAL, b"lab", b"5", b"right"));
        file.extend([0, 1]); // truncated trailing entry is ignored
        let entries = parse_xauthority(&file);
        assert_eq!(entries.len(), 3);
        let local = AuthAddress::Local(b"lab".to_vec());
        let (name, data) = select_cookie(&entries, &local, 5).unwrap();
        assert_eq!(
            (name.as_slice(), data.as_slice()),
            (MIT_MAGIC_COOKIE, &b"right"[..])
        );
        assert!(select_cookie(&entries, &local, 7).is_none());

        let wild = parse_xauthority(&xauth_entry(FAMILY_WILD, b"", b"", b"any"));
        assert!(select_cookie(&wild, &local, 9).is_some());
        let ip = AuthAddress::Ip("10.0.0.2".parse().unwrap());
        let inet = parse_xauthority(&xauth_entry(FAMILY_INTERNET, &[10, 0, 0, 2], b"0", b"c"));
        assert!(select_cookie(&inet, &ip, 0).is_some());
        assert!(select_cookie(&inet, &local, 0).is_none());
    }

    #[test]
    fn encodes_requests_with_padding_and_length() {
        let mut request = Request::new(OP_INTERN_ATOM, 0);
        request.u16(5).u16(0).padded(b"ABCDE");
        let bytes = request.finish().unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(le16(&bytes, 2), Some(4));
        assert_eq!(&bytes[8..13], b"ABCDE");
        assert_eq!(&bytes[13..], &[0, 0, 0]);
    }

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
        let mut holder = Connection::connect_from_env().unwrap();
        let mut prober = Connection::connect_from_env().unwrap();
        let key: KeyCombo = "super+F7".parse().unwrap();
        let target = prober.key_targets(&key).unwrap()[0];
        assert_eq!(
            prober.check_grab(target.keycode, 64).unwrap().status(),
            GrabStatus::Free
        );
        let mut request = Request::new(OP_GRAB_KEY, 0);
        request
            .u32(holder.root)
            .u16(64)
            .u8(target.keycode)
            .u8(1)
            .u8(1)
            .u8(0)
            .u16(0);
        holder.send(request).unwrap();
        assert!(holder.sync().unwrap().is_empty());
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
        let opcode = holder.xinput_opcode().unwrap().expect("XInput2");
        let mut request = Request::new(opcode, XI_PASSIVE_GRAB_DEVICE);
        request
            .u32(0)
            .u32(holder.root)
            .u32(0)
            .u32(u32::from(target.keycode))
            .u16(XI_VIRTUAL_CORE_KEYBOARD)
            .u16(1)
            .u16(1)
            .u8(XI_GRABTYPE_KEYCODE)
            .u8(XI_GRAB_MODE_ASYNC)
            .u8(XI_GRAB_MODE_ASYNC)
            .u8(0)
            .u16(0)
            .u32((1 << 2) | (1 << 3))
            .u32(4);
        let reply = holder.request_reply(request).unwrap();
        assert_eq!(le16(&reply, 8), Some(0));
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
