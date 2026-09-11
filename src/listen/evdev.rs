#[cfg(not(target_os = "linux"))]
fn run_evdev(_options: Options) -> Result<(), ListenError> {
    Err(ListenError::Message(
        "evdev capture is only available on Linux".into(),
    ))
}

#[cfg(target_os = "linux")]
fn run_evdev(options: Options) -> Result<(), ListenError> {
    let snapshot = ListenSession::capture();
    let mut backend = EvdevBackend::open(options.device.as_deref())?;
    backend.set_events_all(options.events_all);
    backend
        .arm(CapturePolicy::PassThrough)
        .map_err(ListenError::Setup)?;
    run_observed_loop(options, backend, snapshot, None)
}

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct LinuxInputEvent {
    _time: libc::timeval,
    event_type: u16,
    code: u16,
    value: i32,
}

#[cfg(target_os = "linux")]
struct EvdevDevice {
    path: PathBuf,
    name: String,
    file: File,
    pressed_modifiers: Vec<u16>,
    locked_modifiers: u32,
    /// Set after `SYN_DROPPED`; all events through the following
    /// `SYN_REPORT` are discarded before querying the kernel's current state.
    resync_required: bool,
}

#[cfg(target_os = "linux")]
struct EvdevSession {
    devices: Vec<EvdevDevice>,
    requested: Option<PathBuf>,
    last_hotplug_scan: Instant,
    include_modifiers: bool,
    /// Optional compositor-independent XKB map compiled from explicit RMLVO
    /// environment variables. Without one, retain the Linux key-name
    /// fallback and report the translation as conditional.
    xkb_keymap: Option<String>,
    xkb_group: usize,
}

#[cfg(target_os = "linux")]
impl EvdevSession {
    fn open(requested: Option<&Path>) -> Result<Self, ListenError> {
        let paths = if let Some(path) = requested {
            vec![path.to_owned()]
        } else {
            let mut paths = std::fs::read_dir("/dev/input")
                .map_err(|error| {
                    ListenError::Message(format!(
                        "cannot enumerate /dev/input: {error}; evdev capture may require the input group"
                    ))
                })?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("event"))
                })
                .collect::<Vec<_>>();
            paths.sort();
            paths
        };

        let mut devices = Vec::new();
        let mut failures = Vec::new();
        for path in paths.into_iter().take(64) {
            let keyboard_capability = evdev_keyboard_capability(&path);
            if keyboard_capability == Some(false) {
                if requested.is_some() {
                    failures.push(format!(
                        "{}: device does not advertise alphanumeric keyboard keys",
                        path.display()
                    ));
                }
                continue;
            }
            match File::open(&path) {
                Ok(file) => {
                    if let Err(error) = set_nonblocking(&file) {
                        failures.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                    let name = evdev_device_name(&path);
                    let pressed_modifiers = evdev_pressed_modifiers_from_kernel(&file);
                    let locked_modifiers = evdev_locked_modifiers_from_kernel(&file);
                    devices.push(EvdevDevice {
                        path,
                        name,
                        file,
                        pressed_modifiers,
                        locked_modifiers,
                        resync_required: false,
                    });
                }
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        if devices.is_empty() {
            let suffix = failures
                .first()
                .map(|failure| format!(" ({failure})"))
                .unwrap_or_default();
            return Err(ListenError::Message(format!(
                "no readable keyboard event device found{suffix}; add your user to the input group or pass --device /dev/input/eventN"
            )));
        }
        Ok(Self {
            devices,
            requested: requested.map(Path::to_owned),
            last_hotplug_scan: Instant::now(),
            include_modifiers: false,
            xkb_keymap: crate::xkb::compile_keymap_from_environment(),
            xkb_group: crate::xkb::group_from_environment(),
        })
    }

    fn key_name(&self, code: crate::xkb::EvdevKeycode) -> String {
        self.xkb_keymap
            .as_deref()
            .and_then(|keymap| {
                crate::xkb::preferred_symbol_for_evdev_keycode(
                    keymap,
                    code,
                    self.xkb_group,
                    self.xkb_level(),
                )
            })
            .unwrap_or_else(|| {
                evdev_key_name(code.get())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("CODE:{code}"))
            })
    }

    fn xkb_level(&self) -> usize {
        let modifiers = self.current_modifiers();
        let shift = modifiers & 1 != 0;
        let caps = modifiers & 2 != 0;
        let altgr = self
            .devices
            .iter()
            .any(|device| device.pressed_modifiers.contains(&100));
        if altgr {
            usize::from(shift) + 2
        } else {
            usize::from(shift ^ caps)
        }
    }

    fn encoding_label(&self) -> &'static str {
        if self.xkb_keymap.is_some() {
            "evdev key event translated with explicit XKB RMLVO before compositor processing"
        } else {
            "evdev key event before compositor processing"
        }
    }

    fn read_key_event(&mut self, timeout_ms: i32) -> io::Result<Option<ObservedKey>> {
        self.maybe_add_hotplugged_devices();
        let mut pollfds = self
            .devices
            .iter()
            .map(|device| libc::pollfd {
                fd: device.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        // SAFETY: pollfds points to valid descriptors owned by self for the
        // duration of this call.
        let result = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                // SignalGuard records the signal; let the outer loop observe
                // it and return the normal interruption result instead of
                // exposing a low-level EINTR failure.
                return Ok(None);
            }
            return Err(error);
        }
        if result == 0 {
            return Ok(None);
        }
        let mut disconnected = Vec::new();
        let mut observed = None;
        for (index, pollfd) in pollfds.iter().enumerate() {
            if pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                disconnected.push(index);
                continue;
            }
            if pollfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let event = match read_linux_input_event(&mut self.devices[index].file) {
                Ok(event) => event,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => continue,
                Err(error) if matches!(error.raw_os_error(), Some(libc::EIO | libc::ENODEV)) => {
                    disconnected.push(index);
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    disconnected.push(index);
                    continue;
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", self.devices[index].path.display()),
                    ));
                }
            };
            if event.event_type == EV_SYN && event.code == SYN_DROPPED {
                self.devices[index].resync_required = true;
                continue;
            }
            if self.devices[index].resync_required {
                // The kernel requires userspace to ignore all events up to
                // and including the next SYN_REPORT after SYN_DROPPED. Only
                // then is EVIOCGKEY/EVIOCGLED a coherent snapshot again.
                if event.event_type == EV_SYN && event.code == SYN_REPORT {
                    self.refresh_device_modifiers(index);
                    self.devices[index].resync_required = false;
                }
                continue;
            }
            if event.event_type != EV_KEY {
                continue;
            }
            let event_type = match event.value {
                0 => KeyEventType::Release,
                1 => KeyEventType::Press,
                2 => KeyEventType::Repeat,
                _ => continue,
            };
            // Kernel input enters the typed keycode system here: the raw u16
            // becomes an EvdevKeycode once, and only `to_xkb` crosses to XKB.
            let evdev_code = crate::xkb::EvdevKeycode::from(event.code);
            let modifier = evdev_modifier(event.code);
            if let Some(mask) = modifier {
                let lock_key = matches!(event.code, 58 | 69);
                if lock_key && event_type == KeyEventType::Press {
                    self.devices[index].locked_modifiers ^= mask;
                } else if !lock_key && event_type == KeyEventType::Release {
                    self.devices[index]
                        .pressed_modifiers
                        .retain(|code| *code != event.code);
                } else if !lock_key && !self.devices[index].pressed_modifiers.contains(&event.code)
                {
                    self.devices[index].pressed_modifiers.push(event.code);
                }
                if self.include_modifiers {
                    let key_name = self.key_name(evdev_code);
                    let device = self.devices[index].name.clone();
                    let path = self.devices[index].path.display().to_string();
                    let modifiers = self.current_modifiers();
                    observed = Some(ObservedKey {
                        combo: KeyCombo::from_parts(modifiers, key_name.clone()),
                        raw: Vec::new(),
                        raw_display: Some(format!(
                            "EV_KEY code={} value={} ({})",
                            event.code,
                            event.value,
                            event_type.label()
                        )),
                        modifier_state: Some(self.modifier_state()),
                        associated_text: None,
                        encoding: self.encoding_label().into(),
                        protocol_flags: None,
                        event_type,
                        alternate_keys: None,
                        alternate_key: Some(format!(
                            "physical modifier keycode {} ({key_name})",
                            event.code
                        )),
                        physical_keycode: Some(evdev_code),
                        source: CaptureSource::Evdev { device, path },
                        disposition: CaptureDisposition::ObservedOnly,
                    });
                    break;
                } else {
                    continue;
                }
            }
            if !self.include_modifiers && event_type != KeyEventType::Press {
                continue;
            }
            let modifiers_before = self.current_modifiers();
            let key_name = self.key_name(evdev_code);
            let combo = KeyCombo::from_parts(modifiers_before, key_name.clone());
            let device = self.devices[index].name.clone();
            let path = self.devices[index].path.display().to_string();
            observed = Some(ObservedKey {
                combo,
                raw: Vec::new(),
                raw_display: Some(format!(
                    "EV_KEY code={} value={} ({})",
                    event.code,
                    event.value,
                    event_type.label()
                )),
                modifier_state: Some(self.modifier_state()),
                associated_text: None,
                encoding: self.encoding_label().into(),
                protocol_flags: None,
                event_type,
                alternate_keys: None,
                alternate_key: Some(format!("physical keycode {} ({key_name})", event.code)),
                physical_keycode: Some(evdev_code),
                source: CaptureSource::Evdev { device, path },
                disposition: CaptureDisposition::ObservedOnly,
            });
            break;
        }
        for index in disconnected.into_iter().rev() {
            self.devices.remove(index);
        }
        if self.devices.is_empty() {
            self.maybe_add_hotplugged_devices();
        }
        if self.devices.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "all evdev keyboard devices were disconnected",
            ));
        }
        Ok(observed)
    }

    fn maybe_add_hotplugged_devices(&mut self) {
        if self.requested.is_some() || self.last_hotplug_scan.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_hotplug_scan = Instant::now();
        let Ok(entries) = std::fs::read_dir("/dev/input") else {
            return;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths.into_iter().take(64) {
            if self.devices.iter().any(|device| device.path == path)
                || evdev_keyboard_capability(&path) == Some(false)
            {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            if set_nonblocking(&file).is_err() {
                continue;
            }
            let name = evdev_device_name(&path);
            self.devices.push(EvdevDevice {
                path,
                name,
                pressed_modifiers: evdev_pressed_modifiers_from_kernel(&file),
                locked_modifiers: evdev_locked_modifiers_from_kernel(&file),
                file,
                resync_required: false,
            });
        }
    }

    fn refresh_device_modifiers(&mut self, index: usize) {
        let device = &mut self.devices[index];
        device.pressed_modifiers = evdev_pressed_modifiers_from_kernel(&device.file);
        device.locked_modifiers = evdev_locked_modifiers_from_kernel(&device.file);
    }

    fn current_modifiers(&self) -> u32 {
        self.devices.iter().fold(0, |current, device| {
            let pressed = device
                .pressed_modifiers
                .iter()
                .filter_map(|code| evdev_modifier(*code))
                .fold(0, |current, modifier| current | modifier);
            current | pressed | device.locked_modifiers
        })
    }

    fn modifier_state(&self) -> ModifierState {
        let devices = self
            .devices
            .iter()
            .map(|device| DeviceModifierState {
                device: device.name.clone(),
                path: device.path.display().to_string(),
                pressed: modifier_key_names(device.pressed_modifiers.iter().copied()),
                locked: evdev_lock_names(device.locked_modifiers),
            })
            .collect::<Vec<_>>();

        let mut pressed = devices
            .iter()
            .flat_map(|device| device.pressed.iter().cloned())
            .collect::<Vec<_>>();
        pressed.sort();
        pressed.dedup();

        let mut locked = devices
            .iter()
            .flat_map(|device| device.locked.iter().cloned())
            .collect::<Vec<_>>();
        locked.sort();
        locked.dedup();

        ModifierState {
            pressed,
            locked,
            latched: None,
            devices,
        }
    }
}

#[cfg(target_os = "linux")]
fn read_linux_input_event(file: &mut File) -> io::Result<LinuxInputEvent> {
    let mut bytes = [0_u8; std::mem::size_of::<LinuxInputEvent>()];
    file.read_exact(&mut bytes)?;
    // SAFETY: input_event is a C-compatible, Copy struct and bytes contains
    // exactly one kernel input_event record. read_unaligned handles any
    // alignment supplied by the byte array.
    Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<LinuxInputEvent>()) })
}

#[cfg(target_os = "linux")]
fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: the descriptor is owned by `file`; fcntl does not retain the
    // pointer and only changes the descriptor flags.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn evdev_device_name(path: &Path) -> String {
    let fallback = path.display().to_string();
    let Some(event_name) = path.file_name().and_then(|value| value.to_str()) else {
        return fallback;
    };
    std::fs::read_to_string(format!("/sys/class/input/{event_name}/device/name"))
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
}

#[cfg(target_os = "linux")]
fn evdev_keyboard_capability(path: &Path) -> Option<bool> {
    let event_name = path.file_name()?.to_str()?;
    let contents = std::fs::read_to_string(format!(
        "/sys/class/input/{event_name}/device/capabilities/key"
    ))
    .ok()?;
    Some(parse_evdev_keyboard_capability(&contents))
}

fn parse_evdev_keyboard_capability(contents: &str) -> bool {
    let words = contents
        .split_whitespace()
        .filter_map(|word| u64::from_str_radix(word, 16).ok())
        .collect::<Vec<_>>();
    let has_key = |code: usize| {
        words
            .get(code / 64)
            .is_some_and(|word| word & (1_u64 << (code % 64)) != 0)
    };
    has_key(30) || has_key(44) || has_key(57)
}

#[cfg(target_os = "linux")]
const EV_KEY: u16 = 0x01;

#[cfg(target_os = "linux")]
const EV_SYN: u16 = 0x00;

#[cfg(target_os = "linux")]
const SYN_DROPPED: u16 = 0x03;

#[cfg(target_os = "linux")]
const SYN_REPORT: u16 = 0x00;

#[cfg(target_os = "linux")]
fn evdev_pressed_modifiers_from_kernel(file: &File) -> Vec<u16> {
    // EVIOCGKEY(KEY_MAX + 1) returns the current pressed-key bitmap. The
    // ioctl constants are kept local so the crate does not need a generated
    // linux/input.h binding just to recover modifier state.
    const KEY_MAX: usize = 0x2ff;
    const BITMAP_BYTES: usize = (KEY_MAX + 8) / 8;
    const IOC_READ: u64 = 2;
    const IOC_TYPE: u64 = b'E' as u64;
    const IOC_NR: u64 = 0x18;
    let request = (IOC_READ << 30) | ((BITMAP_BYTES as u64) << 16) | (IOC_TYPE << 8) | IOC_NR;
    let mut bitmap = [0_u8; BITMAP_BYTES];
    // SAFETY: bitmap is writable storage of the exact size encoded in the
    // request and the descriptor is owned by the caller for this operation.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::c_ulong,
            bitmap.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Vec::new();
    }
    [29_u16, 97, 42, 54, 56, 100, 125, 126, 58, 69]
        .into_iter()
        .filter(|code| bitmap[usize::from(*code) / 8] & (1 << (code % 8)) != 0)
        .filter(|code| !matches!(code, 58 | 69))
        .collect()
}

#[cfg(target_os = "linux")]
fn evdev_locked_modifiers_from_kernel(file: &File) -> u32 {
    // EVIOCGLED(LED_MAX + 1) exposes the kernel's current NumLock and
    // CapsLock LEDs. Unlike EVIOCGKEY, this is the persistent lock state, not
    // merely whether the lock key is physically held right now.
    const LED_MAX: usize = 0x0f;
    const BITMAP_BYTES: usize = (LED_MAX + 8) / 8;
    const IOC_READ: u64 = 2;
    const IOC_TYPE: u64 = b'E' as u64;
    const IOC_NR: u64 = 0x19;
    let request = (IOC_READ << 30) | ((BITMAP_BYTES as u64) << 16) | (IOC_TYPE << 8) | IOC_NR;
    let mut bitmap = [0_u8; BITMAP_BYTES];
    // SAFETY: bitmap is writable storage of the exact size encoded in the
    // request and the descriptor is owned by the caller for this operation.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::c_ulong,
            bitmap.as_mut_ptr(),
        )
    };
    if result < 0 {
        return 0;
    }
    let mut modifiers = 0;
    if bitmap[0] & (1 << 0) != 0 {
        modifiers |= 16;
    }
    if bitmap[0] & (1 << 1) != 0 {
        modifiers |= 2;
    }
    modifiers
}

#[cfg(target_os = "linux")]
pub(crate) fn evdev_modifier(code: u16) -> Option<u32> {
    Some(match code {
        29 | 97 => 4,    // KEY_LEFTCTRL / KEY_RIGHTCTRL
        42 | 54 => 1,    // KEY_LEFTSHIFT / KEY_RIGHTSHIFT
        56 | 100 => 8,   // KEY_LEFTALT / KEY_RIGHTALT
        125 | 126 => 64, // KEY_LEFTMETA / KEY_RIGHTMETA
        58 => 2,         // KEY_CAPSLOCK
        69 => 16,        // KEY_NUMLOCK
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn modifier_key_names(codes: impl Iterator<Item = u16>) -> Vec<String> {
    let mut names = codes
        .filter_map(evdev_key_name)
        .filter(|name| evdev_modifier_name(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

#[cfg(target_os = "linux")]
fn evdev_modifier_name(name: &str) -> bool {
    matches!(
        name,
        "LEFTCTRL"
            | "RIGHTCTRL"
            | "LEFTSHIFT"
            | "RIGHTSHIFT"
            | "LEFTALT"
            | "RIGHTALT"
            | "LEFTMETA"
            | "RIGHTMETA"
    )
}

#[cfg(target_os = "linux")]
fn evdev_lock_names(mask: u32) -> Vec<String> {
    let mut names = Vec::new();
    if mask & 2 != 0 {
        names.push("CAPSLOCK".into());
    }
    if mask & 16 != 0 {
        names.push("NUMLOCK".into());
    }
    names
}

#[cfg(target_os = "linux")]
pub(crate) fn evdev_key_name(code: u16) -> Option<&'static str> {
    Some(match code {
        1 => "ESCAPE",
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "-",
        13 => "=",
        14 => "BACKSPACE",
        15 => "TAB",
        16 => "Q",
        17 => "W",
        18 => "E",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        26 => "[",
        27 => "]",
        28 => "RETURN",
        29 => "LEFTCTRL",
        30 => "A",
        31 => "S",
        32 => "D",
        33 => "F",
        34 => "G",
        35 => "H",
        36 => "J",
        37 => "K",
        38 => "L",
        39 => ";",
        40 => "'",
        41 => "`",
        42 => "LEFTSHIFT",
        43 => "\\",
        44 => "Z",
        45 => "X",
        46 => "C",
        47 => "V",
        48 => "B",
        49 => "N",
        50 => "M",
        51 => ",",
        52 => ".",
        53 => "/",
        54 => "RIGHTSHIFT",
        55 => "KP_MULTIPLY",
        56 => "LEFTALT",
        57 => "SPACE",
        58 => "CAPSLOCK",
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
        69 => "NUMLOCK",
        70 => "SCROLLLOCK",
        71 => "KP_7",
        72 => "KP_8",
        73 => "KP_9",
        74 => "KP_SUBTRACT",
        75 => "KP_4",
        76 => "KP_5",
        77 => "KP_6",
        78 => "KP_ADD",
        79 => "KP_1",
        80 => "KP_2",
        81 => "KP_3",
        82 => "KP_0",
        83 => "KP_DECIMAL",
        87 => "F11",
        88 => "F12",
        96 => "KP_ENTER",
        97 => "RIGHTCTRL",
        98 => "KP_DIVIDE",
        100 => "RIGHTALT",
        102 => "HOME",
        103 => "UP",
        104 => "PAGE_UP",
        105 => "LEFT",
        106 => "RIGHT",
        107 => "END",
        108 => "DOWN",
        109 => "PAGE_DOWN",
        110 => "INSERT",
        111 => "DELETE",
        113 => "VOLUME_MUTE",
        114 => "VOLUME_DOWN",
        115 => "VOLUME_UP",
        119 => "PAUSE",
        125 => "LEFTMETA",
        126 => "RIGHTMETA",
        _ => return None,
    })
}

