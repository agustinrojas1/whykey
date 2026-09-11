struct TerminalSession {
    tty: File,
    original: libc::termios,
    protocol_enabled: bool,
    protocol_flags: Option<u32>,
}

/// Terminal transport adapter. The established terminal capture loop remains
/// the compatibility path, while this type exposes the same backend contract
/// for callers that want to select transports uniformly.
pub struct TerminalBackend {
    session: Option<TerminalSession>,
    pending: VecDeque<u8>,
}

impl TerminalBackend {
    pub fn new() -> Self {
        Self {
            session: None,
            pending: VecDeque::new(),
        }
    }

    pub fn original_termios(&self) -> Option<libc::termios> {
        self.session.as_ref().map(|session| session.original)
    }

    fn read_one(&mut self) -> Result<Option<ObservedKey>, String> {
        let Some(session) = self.session.as_mut() else {
            return Err("terminal capture is closed".into());
        };
        let mut reader =
            InputReader::with_pending(&mut session.tty, self.pending.drain(..).collect());
        let result = read_event(&mut reader, None).map_err(|error| error.to_string());
        self.pending = reader.pending;
        let protocol_flags = session.protocol_flags;
        match result? {
            ReadEvent::Idle => Ok(None),
            ReadEvent::Key(mut observed) => {
                observed.protocol_flags = protocol_flags;
                Ok(Some(*observed))
            }
        }
    }
}

impl Default for TerminalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for TerminalBackend {
    fn id(&self) -> CaptureBackendId {
        CaptureBackendId::Terminal
    }

    fn display(&self) -> &'static str {
        CaptureBackendId::Terminal.display()
    }

    fn arm(&mut self, _policy: CapturePolicy) -> Result<(), String> {
        if self.session.is_none() {
            self.session = Some(TerminalSession::open().map_err(|error| error.to_string())?);
        }
        Ok(())
    }

    fn next_observed_event(&mut self, _events_all: bool) -> Result<Option<ObservedKey>, String> {
        loop {
            let Some(observed) = self.read_one()? else {
                return Ok(None);
            };
            if observed.event_type != KeyEventType::Press || is_terminal_modifier_key(&observed) {
                continue;
            }
            return Ok(Some(observed));
        }
    }

    fn wait_for_chord_release(
        &mut self,
        _main: crate::xkb::XkbKeycode,
        _timeout: Duration,
    ) -> Result<crate::capture::ChordReleaseStatus, String> {
        Ok(crate::capture::ChordReleaseStatus::Released)
    }

    fn is_suppressing(&self) -> bool {
        false
    }

    fn close(&mut self) -> io::Result<()> {
        let result = self
            .session
            .as_mut()
            .map_or(Ok(()), TerminalSession::restore);
        self.session = None;
        result
    }
}

#[cfg(target_os = "linux")]
/// Read-only Linux input transport adapter.
pub struct EvdevBackend {
    session: Option<EvdevSession>,
    requested: Option<PathBuf>,
}

#[cfg(target_os = "linux")]
impl EvdevBackend {
    pub fn open(device: Option<&Path>) -> Result<Self, ListenError> {
        Ok(Self {
            session: Some(EvdevSession::open(device)?),
            requested: device.map(Path::to_owned),
        })
    }

    pub fn set_events_all(&mut self, events_all: bool) {
        if let Some(session) = self.session.as_mut() {
            session.include_modifiers = events_all;
        }
    }
}

#[cfg(target_os = "linux")]
impl CaptureBackend for EvdevBackend {
    fn id(&self) -> CaptureBackendId {
        CaptureBackendId::Evdev
    }

    fn display(&self) -> &'static str {
        CaptureBackendId::Evdev.display()
    }

    fn arm(&mut self, _policy: CapturePolicy) -> Result<(), String> {
        if self.session.is_none() {
            self.session = Some(
                EvdevSession::open(self.requested.as_deref()).map_err(|error| error.to_string())?,
            );
        }
        Ok(())
    }

    fn next_observed_event(&mut self, _events_all: bool) -> Result<Option<ObservedKey>, String> {
        self.session
            .as_mut()
            .ok_or_else(|| "evdev capture is closed".to_owned())?
            .read_key_event(100)
            .map_err(|error| error.to_string())
    }

    fn wait_for_chord_release(
        &mut self,
        _main: crate::xkb::XkbKeycode,
        _timeout: Duration,
    ) -> Result<crate::capture::ChordReleaseStatus, String> {
        Ok(crate::capture::ChordReleaseStatus::Released)
    }

    fn is_suppressing(&self) -> bool {
        false
    }

    fn close(&mut self) -> io::Result<()> {
        self.session = None;
        Ok(())
    }
}

struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

impl SignalGuard {
    fn install() -> io::Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = signal_handler as *const () as usize;
        action.sa_flags = 0;
        // SAFETY: `action` is valid writable storage and sigemptyset receives
        // a pointer to its initialized signal mask.
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut previous = Vec::new();
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            let mut old_action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            // SAFETY: both action pointers refer to valid sigaction structs;
            // the signal numbers are the four constants listed above.
            if unsafe { libc::sigaction(signal, &action, &mut old_action) } != 0 {
                for (old_signal, old_action) in previous.drain(..).rev() {
                    // SAFETY: these handlers were returned by sigaction for
                    // the corresponding signal and are restored verbatim.
                    unsafe { libc::sigaction(old_signal, &old_action, std::ptr::null_mut()) };
                }
                return Err(io::Error::last_os_error());
            }
            previous.push((signal, old_action));
        }
        Ok(Self { previous })
    }

    fn received(&self) -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}

extern "C" fn signal_handler(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, action) in self.previous.drain(..).rev() {
            // SAFETY: each action was returned by sigaction and belongs to
            // the matching signal number.
            unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
        }
    }
}

struct InputReader<'a> {
    tty: &'a mut File,
    pending: VecDeque<u8>,
}

impl<'a> InputReader<'a> {
    fn with_pending(tty: &'a mut File, pending: Vec<u8>) -> Self {
        Self {
            tty,
            pending: pending.into(),
        }
    }

    fn read_byte(&mut self) -> io::Result<u8> {
        if let Some(byte) = self.pending.pop_front() {
            return Ok(byte);
        }
        let mut buffer = [0_u8; 256];
        let count = self.tty.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            ));
        }
        self.pending.extend(&buffer[..count]);
        self.pending
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "terminal input closed"))
    }

    fn has_input(&self, timeout_ms: i32, deadline: Option<Instant>) -> io::Result<bool> {
        if !self.pending.is_empty() {
            return Ok(true);
        }
        let timeout_ms = deadline
            .map(|deadline| timeout_ms.min(remaining_millis(deadline)))
            .unwrap_or(timeout_ms);
        if timeout_ms < 0 {
            return Ok(false);
        }
        wait_for_input(self.tty.as_raw_fd(), timeout_ms)
    }

    fn read_raw_event(&mut self, deadline: Option<Instant>) -> io::Result<Vec<u8>> {
        if !self.has_input(SIGNAL_POLL_MS, deadline)? {
            return Ok(Vec::new());
        }
        let first = self.read_byte()?;
        if first != 0x1b {
            let width = utf8_width(first);
            let mut bytes = vec![first];
            for _ in 1..width {
                if !self.has_input(SEQUENCE_TIMEOUT_MS, deadline)? {
                    break;
                }
                bytes.push(self.read_byte()?);
            }
            return Ok(bytes);
        }
        if !self.has_input(LEGACY_ESCAPE_SEQUENCE_TIMEOUT_MS, deadline)? {
            return Ok(vec![first]);
        }

        let second = self.read_byte()?;
        if second == 0x1b {
            // Treat a second literal Escape as the next event, not an
            // Alt-prefixed Escape. This preserves immediate Escape
            // cancellation in legacy terminals as well as Kitty terminals.
            self.pending.push_front(second);
            return Ok(vec![first]);
        }

        let mut bytes = vec![first, second];
        while bytes.len() < MAX_TERMINAL_SEQUENCE_BYTES
            && self.has_input(SEQUENCE_TIMEOUT_MS, deadline)?
        {
            bytes.push(self.read_byte()?);
            if is_complete_escape_sequence(&bytes) {
                break;
            }
        }
        Ok(bytes)
    }
}

impl TerminalSession {
    fn open() -> io::Result<Self> {
        let mut tty = File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|error| {
                if matches!(error.raw_os_error(), Some(libc::ENXIO) | Some(libc::ENOTTY)) {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "no controlling terminal; run `whykey listen` inside a terminal",
                    )
                } else {
                    error
                }
            })?;
        let fd = tty.as_raw_fd();
        let original = get_termios(fd)?;
        let mut raw = original;

        // Keep ISIG enabled so legacy terminals can still deliver Ctrl+C as
        // SIGINT. Kitty terminals encode it as a key, handled above.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::IEXTEN);
        raw.c_iflag &= !(libc::IXON | libc::IXOFF | libc::ICRNL | libc::INLCR | libc::IGNCR);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        set_termios(fd, &raw)?;
        // Bit 1 disambiguates escape/control sequences. Bit 8 asks for all
        // keys as sequences, which preserves Shift and lock modifiers.
        let (protocol_flags, _) = match query_keyboard_protocol(&mut tty) {
            Ok(result) => result,
            Err(error) => {
                let _ = set_termios(fd, &original);
                return Err(error);
            }
        };
        let request = format!("\x1b[>{KITTY_CAPTURE_FLAGS}u");
        if let Err(error) = tty.write_all(request.as_bytes()) {
            let _ = set_termios(fd, &original);
            return Err(error);
        }
        tty.flush()?;
        // Input typed before capture is ready, including the Enter release
        // that launched the command and protocol-query leftovers, must never
        // be mistaken for the requested shortcut.
        flush_input(fd)?;
        Ok(Self {
            tty,
            original,
            protocol_enabled: true,
            protocol_flags,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.protocol_enabled {
            if let Err(error) = self.tty.write_all(b"\x1b[<u") {
                first_error = Some(error);
            }
            self.protocol_enabled = false;
        }
        if let Err(error) = set_termios(self.tty.as_raw_fd(), &self.original) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

