//! A minimal Wayland client: one xdg toplevel with a solid shm buffer and a
//! wl_keyboard. It speaks the wire protocol directly over the compositor's
//! Unix socket (no libwayland), so it works wherever whykey runs.
//!
//! Only the requests and events the probe needs are implemented. Every
//! incoming message is length-checked before its arguments are decoded;
//! unknown objects and opcodes are skipped by their declared size.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ProbeEvent, ProbeKind, ProbeWindow};
use crate::xkb::XkbKeycode;

pub const PROBE_APP_ID: &str = "whykey-probe";

const DISPLAY_ID: u32 = 1;
/// The wire header stores the message size in 16 bits.
const MAX_MESSAGE_BYTES: usize = u16::MAX as usize;
/// Incoming bytes kept while waiting for the rest of a message. A complete
/// message never exceeds [`MAX_MESSAGE_BYTES`], so this only bounds a
/// misbehaving peer.
const MAX_PENDING_BYTES: usize = 4 * MAX_MESSAGE_BYTES;
const MAX_PENDING_FDS: usize = 64;
const MAX_FDS_PER_READ: usize = 28;
const MAX_KEYMAP_BYTES: usize = 16 << 20;
const MAX_RELEASED_KEYBOARDS: usize = 16;
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

const WL_SEAT_CAPABILITY_KEYBOARD: u32 = 2;
const WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1: u32 = 1;
const WL_SHM_FORMAT_ARGB8888: u32 = 0;

// Request opcodes.
const WL_DISPLAY_SYNC: u16 = 0;
const WL_DISPLAY_GET_REGISTRY: u16 = 1;
const WL_REGISTRY_BIND: u16 = 0;
const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
const WL_SHM_CREATE_POOL: u16 = 0;
const WL_SHM_POOL_CREATE_BUFFER: u16 = 0;
const WL_SHM_POOL_DESTROY: u16 = 1;
const WL_SURFACE_ATTACH: u16 = 1;
const WL_SURFACE_DAMAGE: u16 = 2;
const WL_SURFACE_COMMIT: u16 = 6;
const XDG_WM_BASE_GET_XDG_SURFACE: u16 = 2;
const XDG_WM_BASE_PONG: u16 = 3;
const XDG_SURFACE_GET_TOPLEVEL: u16 = 1;
const XDG_SURFACE_ACK_CONFIGURE: u16 = 4;
const XDG_TOPLEVEL_SET_TITLE: u16 = 2;
const XDG_TOPLEVEL_SET_APP_ID: u16 = 3;
const WL_SEAT_GET_KEYBOARD: u16 = 1;
const WL_KEYBOARD_RELEASE: u16 = 0;

// Event opcodes.
const WL_DISPLAY_ERROR: u16 = 0;
const WL_REGISTRY_GLOBAL: u16 = 0;
const WL_CALLBACK_DONE: u16 = 0;
const XDG_WM_BASE_PING: u16 = 0;
const XDG_SURFACE_CONFIGURE: u16 = 0;
const XDG_TOPLEVEL_CLOSE: u16 = 1;
const WL_SEAT_CAPABILITIES: u16 = 0;
const WL_KEYBOARD_KEYMAP: u16 = 0;
const WL_KEYBOARD_ENTER: u16 = 1;
const WL_KEYBOARD_LEAVE: u16 = 2;
const WL_KEYBOARD_KEY: u16 = 3;
const WL_KEYBOARD_MODIFIERS: u16 = 4;

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

/// Request arguments in wire encoding (host-endian 32-bit words).
#[derive(Default)]
struct Args(Vec<u8>);

impl Args {
    fn uint(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_ne_bytes());
        self
    }

    fn int(self, value: i32) -> Self {
        self.uint(value as u32)
    }

    fn string(mut self, value: &str) -> Self {
        // Length includes the terminating NUL; content pads to 32 bits.
        // Interior NULs would truncate the string on the server, so drop them.
        let bytes: Vec<u8> = value.bytes().filter(|byte| *byte != 0).collect();
        let length = bytes.len() + 1;
        self.0.extend_from_slice(&(length as u32).to_ne_bytes());
        self.0.extend_from_slice(&bytes);
        self.0.push(0);
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
        self
    }
}

/// One complete incoming message.
#[derive(Debug, PartialEq, Eq)]
struct Message {
    object: u32,
    opcode: u16,
    args: Vec<u8>,
}

/// Bounds-checked decoder for one message's arguments.
struct ArgReader<'a> {
    data: &'a [u8],
}

impl<'a> ArgReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if length > self.data.len() {
            return Err(protocol_error(
                "Wayland message is shorter than its arguments",
            ));
        }
        let (head, tail) = self.data.split_at(length);
        self.data = tail;
        Ok(head)
    }

    fn uint(&mut self) -> io::Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn array(&mut self) -> io::Result<&'a [u8]> {
        let length = self.uint()? as usize;
        let padded = length
            .checked_add(3)
            .map(|value| value & !3)
            .ok_or_else(|| protocol_error("Wayland array length overflows"))?;
        let bytes = self.take(padded)?;
        Ok(&bytes[..length])
    }

    fn string(&mut self) -> io::Result<String> {
        let bytes = self.array()?;
        let text = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        Ok(String::from_utf8_lossy(text).into_owned())
    }
}

/// The socket plus incoming bytes and file descriptors not yet consumed.
struct Connection {
    socket: UnixStream,
    input: Vec<u8>,
    fds: VecDeque<OwnedFd>,
    next_id: u32,
}

impl Connection {
    fn connect() -> io::Result<Self> {
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
        Ok(Self::from_socket(socket))
    }

    fn from_socket(socket: UnixStream) -> Self {
        Self {
            socket,
            input: Vec::new(),
            fds: VecDeque::new(),
            next_id: DISPLAY_ID + 1,
        }
    }

    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, object: u32, opcode: u16, args: Args) -> io::Result<()> {
        self.send_with_fd(object, opcode, args, None)
    }

    fn send_with_fd(
        &mut self,
        object: u32,
        opcode: u16,
        args: Args,
        fd: Option<RawFd>,
    ) -> io::Result<()> {
        let size = 8 + args.0.len();
        if size > MAX_MESSAGE_BYTES {
            return Err(protocol_error(
                "Wayland request exceeds the wire size limit",
            ));
        }
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(&object.to_ne_bytes());
        bytes.extend_from_slice(&(((size as u32) << 16) | u32::from(opcode)).to_ne_bytes());
        bytes.extend_from_slice(&args.0);
        send_message(self.socket.as_raw_fd(), &bytes, fd)
    }

    /// Wait up to `timeout` for input and append it. `Ok(false)` on timeout.
    fn read(&mut self, timeout: Duration) -> io::Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: `poll` points to one initialized pollfd for the call.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        if ready == 0 {
            return Ok(false);
        }
        let (bytes, fds) = receive_message(self.socket.as_raw_fd())?;
        if bytes.is_empty() && fds.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the Wayland compositor closed the connection",
            ));
        }
        if self.input.len() + bytes.len() > MAX_PENDING_BYTES {
            return Err(protocol_error(
                "Wayland input exceeds the buffered-bytes limit",
            ));
        }
        if self.fds.len() + fds.len() > MAX_PENDING_FDS {
            return Err(protocol_error(
                "Wayland peer sent too many file descriptors",
            ));
        }
        self.input.extend_from_slice(&bytes);
        self.fds.extend(fds);
        Ok(true)
    }

    fn pop_message(&mut self) -> io::Result<Option<Message>> {
        pop_message(&mut self.input)
    }
}

fn pop_message(input: &mut Vec<u8>) -> io::Result<Option<Message>> {
    if input.len() < 8 {
        return Ok(None);
    }
    let object = u32::from_ne_bytes([input[0], input[1], input[2], input[3]]);
    let word = u32::from_ne_bytes([input[4], input[5], input[6], input[7]]);
    let size = (word >> 16) as usize;
    let opcode = (word & 0xffff) as u16;
    if size < 8 || size % 4 != 0 {
        return Err(protocol_error(format!(
            "malformed Wayland message header (object {object}, size {size})"
        )));
    }
    if input.len() < size {
        return Ok(None);
    }
    let args = input[8..size].to_vec();
    input.drain(..size);
    Ok(Some(Message {
        object,
        opcode,
        args,
    }))
}

const fn cmsg_space(fds: usize) -> usize {
    // SAFETY: CMSG_SPACE is a pure size computation.
    unsafe { libc::CMSG_SPACE((fds * std::mem::size_of::<RawFd>()) as libc::c_uint) as usize }
}

/// Write all of `bytes`, attaching `fd` (SCM_RIGHTS) to the first chunk.
fn send_message(socket: RawFd, bytes: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    let mut offset = 0;
    let mut pending_fd = fd;
    let mut control = [0_u64; cmsg_space(1).div_ceil(8)];
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let mut iov = libc::iovec {
            iov_base: remaining.as_ptr() as *mut libc::c_void,
            iov_len: remaining.len(),
        };
        // SAFETY: msghdr is plain data; zero is a valid empty header.
        let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
        header.msg_iov = &mut iov;
        header.msg_iovlen = 1;
        if let Some(fd) = pending_fd {
            control.fill(0);
            header.msg_control = control.as_mut_ptr().cast();
            header.msg_controllen = cmsg_space(1) as _;
            // SAFETY: the control buffer is aligned and sized for one fd, so
            // the first header and its data are in bounds.
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&header);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as _;
                std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
            }
        }
        // SAFETY: header references live buffers for the duration of the call.
        let sent = unsafe { libc::sendmsg(socket, &header, libc::MSG_NOSIGNAL) };
        if sent < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        pending_fd = None;
        offset += sent as usize;
    }
    Ok(())
}

/// One non-blocking recvmsg: bytes plus any SCM_RIGHTS descriptors.
fn receive_message(socket: RawFd) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    const CONTROL_BYTES: usize = cmsg_space(MAX_FDS_PER_READ);
    let mut data = vec![0_u8; 4096];
    let mut control = [0_u64; CONTROL_BYTES.div_ceil(8)];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    // SAFETY: msghdr is plain data; zero is a valid empty header.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = CONTROL_BYTES as _;
    let received = loop {
        // SAFETY: header references live buffers for the duration of the call.
        let received = unsafe {
            libc::recvmsg(
                socket,
                &mut header,
                libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC,
            )
        };
        if received >= 0 {
            break received as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        // WouldBlock (spurious readiness) is passed up as "no data, no EOF".
        return Err(error);
    };
    let mut fds = Vec::new();
    // SAFETY: the kernel filled `header.msg_control` with well-formed
    // headers up to msg_controllen; CMSG_NXTHDR stays within that range.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&header);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let payload = ((*cmsg).cmsg_len as usize)
                    .saturating_sub(libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                let first = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for index in 0..payload {
                    let fd = std::ptr::read_unaligned(first.add(index));
                    if fd >= 0 {
                        fds.push(OwnedFd::from_raw_fd(fd));
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&header, cmsg);
        }
    }
    if header.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(protocol_error(
            "Wayland file descriptors were truncated in transit",
        ));
    }
    data.truncate(received);
    Ok((data, fds))
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
fn read_keymap(fd: &OwnedFd, size: u32) -> io::Result<String> {
    let size = size as usize;
    if size == 0 || size > MAX_KEYMAP_BYTES {
        return Err(protocol_error(format!(
            "Wayland keymap size {size} is outside the accepted range"
        )));
    }
    // SAFETY: stat is plain data; fstat fills it for a valid descriptor.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is a live descriptor and `stat` is writable.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // Mapping past the end of the file would fault on access.
    if (stat.st_size as u64) < size as u64 {
        return Err(protocol_error(
            "Wayland keymap file is shorter than announced",
        ));
    }
    // SAFETY: a fresh private read-only mapping of `size` bytes, which the
    // file is known to contain.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            fd.as_raw_fd(),
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the mapping is `size` readable bytes until munmap below.
    let bytes = unsafe { std::slice::from_raw_parts(mapping.cast::<u8>(), size) };
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(size);
    let text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    // SAFETY: unmapping exactly the region mapped above; `bytes` is dead.
    unsafe {
        libc::munmap(mapping, size);
    }
    Ok(text)
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

#[derive(Debug, Clone)]
struct Global {
    name: u32,
    interface: String,
    version: u32,
}

/// A mapped xdg toplevel that reports keyboard events delivered to it.
pub struct WaylandWindow {
    conn: Connection,
    registry: u32,
    globals: Vec<Global>,
    pending_callback: Option<u32>,
    callback_done: bool,
    wm_base: u32,
    seat: u32,
    seat_version: u32,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
    buffer: u32,
    attached: bool,
    keyboard: Option<u32>,
    focused: bool,
    closed: bool,
    keymap: Option<Keymap>,
    modmask: u32,
    group: usize,
    events: VecDeque<ProbeEvent>,
    /// Keyboards we released; a keymap already in flight for one of them
    /// still carries a descriptor that must be consumed.
    released_keyboards: Vec<u32>,
}

/// Connect, create the toplevel titled `title`, and wait for the initial
/// configure round trip. Keyboard focus arrives later, as the compositor
/// decides; poll [`ProbeWindow::focused`].
pub fn open_wayland_window(title: &str) -> io::Result<WaylandWindow> {
    WaylandWindow::open(Connection::connect()?, title)
}

impl WaylandWindow {
    fn open(mut conn: Connection, title: &str) -> io::Result<Self> {
        let registry = conn.new_id();
        conn.send(
            DISPLAY_ID,
            WL_DISPLAY_GET_REGISTRY,
            Args::default().uint(registry),
        )?;
        let mut window = Self {
            conn,
            registry,
            globals: Vec::new(),
            pending_callback: None,
            callback_done: false,
            wm_base: 0,
            seat: 0,
            seat_version: 0,
            surface: 0,
            xdg_surface: 0,
            toplevel: 0,
            buffer: 0,
            attached: false,
            keyboard: None,
            focused: false,
            closed: false,
            keymap: None,
            modmask: 0,
            group: 0,
            events: VecDeque::new(),
            released_keyboards: Vec::new(),
        };
        window.roundtrip()?;

        let compositor = window.bind("wl_compositor", COMPOSITOR_VERSION)?;
        let shm = window.bind("wl_shm", SHM_VERSION)?;
        window.wm_base = window.bind("xdg_wm_base", XDG_WM_BASE_VERSION)?;
        window.seat = window.bind("wl_seat", SEAT_VERSION)?;
        window.seat_version = window
            .global("wl_seat")
            .map_or(1, |global| global.version.min(SEAT_VERSION));

        window.buffer = window.create_buffer(shm)?;

        window.surface = window.conn.new_id();
        window.conn.send(
            compositor,
            WL_COMPOSITOR_CREATE_SURFACE,
            Args::default().uint(window.surface),
        )?;
        window.xdg_surface = window.conn.new_id();
        window.conn.send(
            window.wm_base,
            XDG_WM_BASE_GET_XDG_SURFACE,
            Args::default()
                .uint(window.xdg_surface)
                .uint(window.surface),
        )?;
        window.toplevel = window.conn.new_id();
        window.conn.send(
            window.xdg_surface,
            XDG_SURFACE_GET_TOPLEVEL,
            Args::default().uint(window.toplevel),
        )?;
        window.conn.send(
            window.toplevel,
            XDG_TOPLEVEL_SET_TITLE,
            Args::default().string(title),
        )?;
        window.conn.send(
            window.toplevel,
            XDG_TOPLEVEL_SET_APP_ID,
            Args::default().string(PROBE_APP_ID),
        )?;
        // The initial commit without a buffer asks for the first configure.
        window
            .conn
            .send(window.surface, WL_SURFACE_COMMIT, Args::default())?;
        window.roundtrip()?;
        Ok(window)
    }

    fn global(&self, interface: &str) -> Option<&Global> {
        self.globals
            .iter()
            .find(|global| global.interface == interface)
    }

    fn bind(&mut self, interface: &str, max_version: u32) -> io::Result<u32> {
        let global = self.global(interface).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the Wayland compositor does not offer {interface}"),
            )
        })?;
        let version = global.version.clamp(1, max_version);
        let id = self.conn.new_id();
        self.conn.send(
            self.registry,
            WL_REGISTRY_BIND,
            Args::default()
                .uint(global.name)
                .string(interface)
                .uint(version)
                .uint(id),
        )?;
        Ok(id)
    }

    fn create_buffer(&mut self, shm: u32) -> io::Result<u32> {
        let stride = BUFFER_WIDTH * 4;
        let size = stride * BUFFER_HEIGHT;
        let fd = create_shm_file(BUFFER_WIDTH, BUFFER_HEIGHT)?;
        let pool = self.conn.new_id();
        self.conn.send_with_fd(
            shm,
            WL_SHM_CREATE_POOL,
            Args::default().uint(pool).int(size),
            Some(fd.as_raw_fd()),
        )?;
        drop(fd);
        let buffer = self.conn.new_id();
        self.conn.send(
            pool,
            WL_SHM_POOL_CREATE_BUFFER,
            Args::default()
                .uint(buffer)
                .int(0)
                .int(BUFFER_WIDTH)
                .int(BUFFER_HEIGHT)
                .int(stride)
                .uint(WL_SHM_FORMAT_ARGB8888),
        )?;
        // The buffer keeps the pool's memory alive.
        self.conn.send(pool, WL_SHM_POOL_DESTROY, Args::default())?;
        Ok(buffer)
    }

    /// Send wl_display.sync and dispatch until its callback fires.
    fn roundtrip(&mut self) -> io::Result<()> {
        let callback = self.conn.new_id();
        self.conn
            .send(DISPLAY_ID, WL_DISPLAY_SYNC, Args::default().uint(callback))?;
        self.pending_callback = Some(callback);
        self.callback_done = false;
        let deadline = Instant::now() + ROUNDTRIP_TIMEOUT;
        while !self.callback_done {
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the Wayland compositor did not answer a sync request",
                ));
            }
            self.read_and_dispatch(deadline - now)?;
        }
        self.pending_callback = None;
        Ok(())
    }

    /// Read once (waiting up to `timeout`) and dispatch complete messages.
    fn read_and_dispatch(&mut self, timeout: Duration) -> io::Result<()> {
        match self.conn.read(timeout) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        while let Some(message) = self.conn.pop_message()? {
            self.dispatch(message)?;
        }
        Ok(())
    }

    fn dispatch(&mut self, message: Message) -> io::Result<()> {
        let mut args = ArgReader::new(&message.args);
        let object = message.object;
        let opcode = message.opcode;
        if object == DISPLAY_ID {
            if opcode == WL_DISPLAY_ERROR {
                let failed = args.uint()?;
                let code = args.uint()?;
                let text = args.string()?;
                return Err(io::Error::other(format!(
                    "Wayland protocol error on object {failed} (code {code}): {text}"
                )));
            }
            // delete_id: ids are never reused, so nothing to recycle.
            return Ok(());
        }
        if object == self.registry {
            if opcode == WL_REGISTRY_GLOBAL {
                let name = args.uint()?;
                let interface = args.string()?;
                let version = args.uint()?;
                if self.globals.len() < 1024 {
                    self.globals.push(Global {
                        name,
                        interface,
                        version,
                    });
                }
            }
            return Ok(());
        }
        if Some(object) == self.pending_callback {
            if opcode == WL_CALLBACK_DONE {
                self.callback_done = true;
            }
            return Ok(());
        }
        if object == self.wm_base {
            if opcode == XDG_WM_BASE_PING {
                let serial = args.uint()?;
                self.conn
                    .send(self.wm_base, XDG_WM_BASE_PONG, Args::default().uint(serial))?;
            }
            return Ok(());
        }
        if object == self.xdg_surface {
            if opcode == XDG_SURFACE_CONFIGURE {
                let serial = args.uint()?;
                self.configure(serial)?;
            }
            return Ok(());
        }
        if object == self.toplevel {
            if opcode == XDG_TOPLEVEL_CLOSE {
                self.closed = true;
            }
            return Ok(());
        }
        if object == self.seat {
            if opcode == WL_SEAT_CAPABILITIES {
                let capabilities = args.uint()?;
                self.seat_capabilities(capabilities)?;
            }
            return Ok(());
        }
        if Some(object) == self.keyboard {
            return self.keyboard_event(opcode, &mut args);
        }
        if opcode == WL_KEYBOARD_KEYMAP && self.released_keyboards.contains(&object) {
            // A keymap sent to a keyboard we already released still carries
            // a descriptor; consume it so later descriptors stay aligned.
            self.conn.fds.pop_front();
        }
        Ok(())
    }

    fn configure(&mut self, serial: u32) -> io::Result<()> {
        self.conn.send(
            self.xdg_surface,
            XDG_SURFACE_ACK_CONFIGURE,
            Args::default().uint(serial),
        )?;
        if !self.attached {
            self.conn.send(
                self.surface,
                WL_SURFACE_ATTACH,
                Args::default().uint(self.buffer).int(0).int(0),
            )?;
            self.conn.send(
                self.surface,
                WL_SURFACE_DAMAGE,
                Args::default()
                    .int(0)
                    .int(0)
                    .int(BUFFER_WIDTH)
                    .int(BUFFER_HEIGHT),
            )?;
            self.attached = true;
        }
        self.conn
            .send(self.surface, WL_SURFACE_COMMIT, Args::default())
    }

    fn seat_capabilities(&mut self, capabilities: u32) -> io::Result<()> {
        let has_keyboard = capabilities & WL_SEAT_CAPABILITY_KEYBOARD != 0;
        match (has_keyboard, self.keyboard) {
            (true, None) => {
                let keyboard = self.conn.new_id();
                self.conn.send(
                    self.seat,
                    WL_SEAT_GET_KEYBOARD,
                    Args::default().uint(keyboard),
                )?;
                self.keyboard = Some(keyboard);
            }
            (false, Some(keyboard)) => {
                // The compositor made the keyboard inert; a later keyboard
                // capability needs a fresh wl_keyboard.
                if self.seat_version >= 3 {
                    self.conn
                        .send(keyboard, WL_KEYBOARD_RELEASE, Args::default())?;
                }
                if self.released_keyboards.len() >= MAX_RELEASED_KEYBOARDS {
                    self.released_keyboards.remove(0);
                }
                self.released_keyboards.push(keyboard);
                self.keyboard = None;
                self.focused = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn keyboard_event(&mut self, opcode: u16, args: &mut ArgReader<'_>) -> io::Result<()> {
        match opcode {
            WL_KEYBOARD_KEYMAP => {
                let format = args.uint()?;
                let size = args.uint()?;
                let fd = self.conn.fds.pop_front().ok_or_else(|| {
                    protocol_error("Wayland keymap event arrived without a file descriptor")
                })?;
                self.keymap = if format == WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 {
                    Some(Keymap::new(read_keymap(&fd, size)?))
                } else {
                    None
                };
            }
            WL_KEYBOARD_ENTER => {
                let _serial = args.uint()?;
                let surface = args.uint()?;
                let _keys = args.array()?;
                if surface == self.surface {
                    self.focused = true;
                }
            }
            WL_KEYBOARD_LEAVE => {
                let _serial = args.uint()?;
                let surface = args.uint()?;
                if surface == self.surface {
                    self.focused = false;
                }
            }
            WL_KEYBOARD_KEY => {
                let _serial = args.uint()?;
                let _time = args.uint()?;
                let key = args.uint()?;
                let state = args.uint()?;
                let kind = match state {
                    0 => ProbeKind::Release,
                    1 => ProbeKind::Press,
                    _ => return Ok(()),
                };
                let Some(keycode) = key.checked_add(8) else {
                    return Ok(());
                };
                let keysym = self
                    .keymap
                    .as_ref()
                    .and_then(|keymap| keymap.keysym(keycode, self.group));
                self.events.push_back(ProbeEvent {
                    keycode,
                    keysym,
                    modmask: self.modmask,
                    kind,
                });
            }
            WL_KEYBOARD_MODIFIERS => {
                let _serial = args.uint()?;
                let depressed = args.uint()?;
                let latched = args.uint()?;
                let locked = args.uint()?;
                let group = args.uint()?;
                let virtual_modifiers = self
                    .keymap
                    .as_ref()
                    .map_or(&[][..], |keymap| &keymap.virtual_modifiers[..]);
                self.modmask = x11_modmask(depressed | latched | locked, virtual_modifiers);
                self.group = group as usize;
                self.events.push_back(ProbeEvent {
                    keycode: 0,
                    keysym: None,
                    modmask: self.modmask,
                    kind: ProbeKind::Modifiers,
                });
            }
            _ => {}
        }
        Ok(())
    }
}

impl ProbeWindow for WaylandWindow {
    fn next_event(&mut self, timeout: Duration) -> io::Result<Option<ProbeEvent>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if self.closed {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "the probe window was closed",
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.read_and_dispatch(deadline - now)?;
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::thread;

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
    fn encodes_strings_with_terminator_and_padding() {
        let args = Args::default().string("abc").uint(7);
        let mut expected = 4_u32.to_ne_bytes().to_vec();
        expected.extend_from_slice(b"abc\0");
        expected.extend_from_slice(&7_u32.to_ne_bytes());
        assert_eq!(args.0, expected);

        let padded = Args::default().string("abcd");
        assert_eq!(padded.0.len(), 4 + 8);
    }

    #[test]
    fn argument_reader_rejects_truncated_data() {
        assert!(ArgReader::new(&[1, 0]).uint().is_err());
        let mut oversized = 64_u32.to_ne_bytes().to_vec();
        oversized.extend_from_slice(b"abcd");
        assert!(ArgReader::new(&oversized).array().is_err());
        let huge = u32::MAX.to_ne_bytes();
        assert!(ArgReader::new(&huge).array().is_err());
        let empty = 0_u32.to_ne_bytes();
        assert_eq!(ArgReader::new(&empty).string().unwrap(), "");
    }

    fn header(object: u32, size: u32, opcode: u32) -> Vec<u8> {
        let mut bytes = object.to_ne_bytes().to_vec();
        bytes.extend_from_slice(&((size << 16) | opcode).to_ne_bytes());
        bytes
    }

    #[test]
    fn message_framing_waits_for_complete_messages_and_rejects_bad_sizes() {
        let mut partial = header(3, 12, 1);
        assert_eq!(pop_message(&mut partial).unwrap(), None);
        assert_eq!(partial.len(), 8);
        partial.extend_from_slice(&9_u32.to_ne_bytes());
        let message = pop_message(&mut partial).unwrap().unwrap();
        assert_eq!(
            message,
            Message {
                object: 3,
                opcode: 1,
                args: 9_u32.to_ne_bytes().to_vec(),
            }
        );
        assert!(partial.is_empty());

        assert!(pop_message(&mut header(3, 4, 0)).is_err());
        assert!(pop_message(&mut header(3, 10, 0)).is_err());
    }

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

    #[derive(Debug, Default)]
    struct ServerLog {
        title: String,
        app_id: String,
        acked: Option<u32>,
        attached: bool,
        pong: Option<u32>,
    }

    /// A scripted compositor: advertises globals, configures the toplevel,
    /// and on get_keyboard sends a keymap (via SCM_RIGHTS), focus, Super+x.
    fn fake_compositor(socket: UnixStream, fail_keyboard: bool) -> ServerLog {
        let mut conn = Connection::from_socket(socket);
        let mut log = ServerLog::default();
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut registry = 0;
        let mut surface = 0;
        let mut xdg_surface = 0;
        let mut toplevel = 0;
        let mut configured = false;
        loop {
            match conn.read(Duration::from_secs(5)) {
                Ok(true) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                _ => return log,
            }
            while let Ok(Some(message)) = conn.pop_message() {
                let mut args = ArgReader::new(&message.args);
                let id = |name: &str| ids.get(name).copied();
                let object = Some(message.object);
                match message.opcode {
                    WL_DISPLAY_GET_REGISTRY if message.object == DISPLAY_ID => {
                        registry = args.uint().unwrap();
                        for (name, interface, version) in [
                            (1, "wl_compositor", 6),
                            (2, "wl_shm", 1),
                            (3, "xdg_wm_base", 6),
                            (4, "wl_seat", 9),
                        ] {
                            conn.send(
                                registry,
                                WL_REGISTRY_GLOBAL,
                                Args::default().uint(name).string(interface).uint(version),
                            )
                            .unwrap();
                        }
                    }
                    WL_DISPLAY_SYNC if message.object == DISPLAY_ID => {
                        let callback = args.uint().unwrap();
                        conn.send(callback, WL_CALLBACK_DONE, Args::default().uint(0))
                            .unwrap();
                    }
                    WL_REGISTRY_BIND if message.object == registry => {
                        let _name = args.uint().unwrap();
                        let interface = args.string().unwrap();
                        let _version = args.uint().unwrap();
                        let new_id = args.uint().unwrap();
                        if interface == "xdg_wm_base" {
                            conn.send(new_id, XDG_WM_BASE_PING, Args::default().uint(41))
                                .unwrap();
                        }
                        if interface == "wl_seat" {
                            conn.send(
                                new_id,
                                WL_SEAT_CAPABILITIES,
                                Args::default().uint(WL_SEAT_CAPABILITY_KEYBOARD),
                            )
                            .unwrap();
                        }
                        ids.insert(interface, new_id);
                    }
                    WL_COMPOSITOR_CREATE_SURFACE if object == id("wl_compositor") => {
                        surface = args.uint().unwrap();
                    }
                    XDG_WM_BASE_GET_XDG_SURFACE if object == id("xdg_wm_base") => {
                        xdg_surface = args.uint().unwrap();
                    }
                    XDG_WM_BASE_PONG if object == id("xdg_wm_base") => {
                        log.pong = Some(args.uint().unwrap());
                    }
                    XDG_SURFACE_GET_TOPLEVEL if message.object == xdg_surface => {
                        toplevel = args.uint().unwrap();
                    }
                    XDG_SURFACE_ACK_CONFIGURE if message.object == xdg_surface => {
                        log.acked = Some(args.uint().unwrap());
                    }
                    XDG_TOPLEVEL_SET_TITLE if message.object == toplevel => {
                        log.title = args.string().unwrap();
                    }
                    XDG_TOPLEVEL_SET_APP_ID if message.object == toplevel => {
                        log.app_id = args.string().unwrap();
                    }
                    WL_SURFACE_ATTACH if message.object == surface => log.attached = true,
                    WL_SURFACE_COMMIT if message.object == surface && !configured => {
                        configured = true;
                        conn.send(xdg_surface, XDG_SURFACE_CONFIGURE, Args::default().uint(7))
                            .unwrap();
                    }
                    WL_SEAT_GET_KEYBOARD if object == id("wl_seat") => {
                        let keyboard = args.uint().unwrap();
                        if fail_keyboard {
                            conn.send(
                                DISPLAY_ID,
                                WL_DISPLAY_ERROR,
                                Args::default().uint(keyboard).uint(3).string("boom"),
                            )
                            .unwrap();
                            continue;
                        }
                        let keymap = memfd_with(TEST_KEYMAP);
                        conn.send_with_fd(
                            keyboard,
                            WL_KEYBOARD_KEYMAP,
                            Args::default()
                                .uint(WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1)
                                .uint(TEST_KEYMAP.len() as u32 + 1),
                            Some(keymap.as_raw_fd()),
                        )
                        .unwrap();
                        let sends = [
                            (
                                WL_KEYBOARD_ENTER,
                                Args::default().uint(1).uint(surface).uint(0),
                            ),
                            (
                                WL_KEYBOARD_MODIFIERS,
                                Args::default().uint(2).uint(1 << 9).uint(0).uint(0).uint(0),
                            ),
                            (
                                WL_KEYBOARD_KEY,
                                Args::default().uint(3).uint(0).uint(45).uint(1),
                            ),
                            (
                                WL_KEYBOARD_MODIFIERS,
                                Args::default().uint(4).uint(0).uint(0).uint(0).uint(0),
                            ),
                        ];
                        for (opcode, args) in sends {
                            conn.send(keyboard, opcode, args).unwrap();
                        }
                    }
                    _ => {}
                }
            }
        }
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
        let mut window = WaylandWindow::open(Connection::from_socket(client), "probe").unwrap();
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
        let error = WaylandWindow::open(Connection::from_socket(client), "probe")
            .and_then(|mut window| collect_events(&mut window, 1).map(|_| ()))
            .unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        compositor.join().unwrap();
    }
}
