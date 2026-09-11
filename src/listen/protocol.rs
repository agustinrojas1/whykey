fn get_termios(fd: i32) -> io::Result<libc::termios> {
    let mut termios = std::mem::MaybeUninit::uninit();
    // SAFETY: `termios` points to writable storage and `fd` comes from an open file.
    let result = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr initialized the structure on success.
    Ok(unsafe { termios.assume_init() })
}

fn set_termios(fd: i32, termios: &libc::termios) -> io::Result<()> {
    // SAFETY: `termios` points to a valid structure and `fd` comes from an open file.
    let result = unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn flush_input(fd: i32) -> io::Result<()> {
    // SAFETY: `fd` is the controlling terminal owned by this session.
    if unsafe { libc::tcflush(fd, libc::TCIFLUSH) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn query_keyboard_protocol(tty: &mut File) -> io::Result<(Option<u32>, Vec<u8>)> {
    tty.write_all(b"\x1b[?u")?;
    tty.flush()?;
    if !wait_for_input(tty.as_raw_fd(), 30)? {
        return Ok((None, Vec::new()));
    }

    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    tty.read_exact(&mut byte)?;
    response.push(byte[0]);
    if response[0] != 0x1b {
        return Ok((None, response));
    }
    if !wait_for_input(tty.as_raw_fd(), 12)? {
        return Ok((None, response));
    }
    tty.read_exact(&mut byte)?;
    response.push(byte[0]);
    if response[1] != b'[' {
        return Ok((None, response));
    }
    while response.len() < 32 && wait_for_input(tty.as_raw_fd(), 12)? {
        tty.read_exact(&mut byte)?;
        response.push(byte[0]);
        if byte[0] == b'u' {
            break;
        }
    }

    if let Some(value) = parse_protocol_response(&response) {
        return Ok((Some(value), Vec::new()));
    }
    Ok((None, response))
}

pub(crate) fn parse_protocol_response(response: &[u8]) -> Option<u32> {
    if !response.starts_with(b"\x1b[?") || response.last() != Some(&b'u') {
        return None;
    }
    std::str::from_utf8(&response[3..response.len() - 1])
        .ok()?
        .parse()
        .ok()
}

fn read_event(reader: &mut InputReader<'_>, deadline: Option<Instant>) -> io::Result<ReadEvent> {
    let bytes = reader.read_raw_event(deadline)?;
    if bytes.is_empty() {
        return Ok(ReadEvent::Idle);
    }
    Ok(ReadEvent::Key(Box::new(decode_bytes(
        bytes,
    )?)))
}

fn remaining_millis(deadline: Instant) -> i32 {
    deadline
        .checked_duration_since(Instant::now())
        .map(|remaining| remaining.as_millis().min(i32::MAX as u128) as i32)
        .unwrap_or(0)
}

fn wait_for_input(fd: i32, timeout_ms: i32) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one valid descriptor entry for the duration of the call.
    let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            // SignalGuard records the signal in an atomic flag. Returning an
            // idle poll lets the capture loop observe that flag and produce
            // its normal restoration diagnostic instead of leaking EINTR.
            Ok(false)
        } else {
            Err(error)
        }
    } else if result == 0 {
        Ok(false)
    } else if pollfd.revents & libc::POLLIN != 0 {
        Ok(true)
    } else if pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "controlling terminal closed while waiting for input",
        ))
    } else {
        Ok(false)
    }
}

fn is_complete_escape_sequence(bytes: &[u8]) -> bool {
    let Some(last) = bytes.last().copied() else {
        return false;
    };
    bytes.len() > 2 && (last.is_ascii_alphabetic() || last == b'~')
}

pub(crate) fn decode_bytes(bytes: Vec<u8>) -> Result<ObservedKey, io::Error> {
    let decoded = decode_key(&bytes);
    // Metadata from a Kitty-looking byte sequence is evidence only when the
    // sequence itself decoded successfully. Otherwise malformed input must
    // remain a raw observation, not acquire an apparently valid alternate
    // key or associated text field.
    let associated_text = decoded
        .as_ref()
        .and_then(|_| associated_text_from_bytes(&bytes));
    let alternate_keys = decoded.as_ref().and_then(|_| alternate_keys(&bytes));
    let alternate_key = alternate_keys
        .as_ref()
        .and_then(|keys| keys.first().cloned());
    let (combo, encoding, event_type) = decoded.unwrap_or_else(|| {
        let raw = bytes
            .iter()
            .fold(String::with_capacity(bytes.len() * 2), |mut raw, byte| {
                let _ = write!(raw, "{byte:02x}");
                raw
            });
        (
            KeyCombo::from_parts(0, format!("RAW:{raw}")),
            "unrecognized terminal bytes; physical/layout origin is unknown".into(),
            // Keep a valid Kitty lifecycle marker even when its key code is
            // not one Whykey names yet. The listener must never turn an
            // unknown release into a fake press and report it as a shortcut.
            kitty_event_type(&bytes).unwrap_or(KeyEventType::Press),
        )
    });
    Ok(ObservedKey {
        combo,
        raw: bytes,
        raw_display: None,
        modifier_state: None,
        associated_text,
        physical_keycode: None,
        encoding,
        protocol_flags: None,
        event_type,
        alternate_keys,
        alternate_key,
        source: CaptureSource::Terminal,
        disposition: CaptureDisposition::ObservedOnly,
    })
}

fn kitty_event_type(bytes: &[u8]) -> Option<KeyEventType> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let modifier_and_event = body.split(';').nth(1)?;
    match modifier_and_event.split(':').nth(1)? {
        "1" => Some(KeyEventType::Press),
        "2" => Some(KeyEventType::Repeat),
        "3" => Some(KeyEventType::Release),
        _ => None,
    }
}

fn alternate_keys(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let field = body.split(';').next()?;
    kitty_alternate_names(field)
}

fn kitty_alternate_names(field: &str) -> Option<Vec<String>> {
    let mut values = Vec::new();
    for value in field.split(':').skip(1).filter(|value| !value.is_empty()) {
        let codepoint = value.parse::<u32>().ok()?;
        values.push(kitty_key_name(codepoint)?);
    }
    Some(values)
}

fn associated_text_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.last() != Some(&b'u') || !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let body = std::str::from_utf8(&bytes[2..bytes.len() - 1]).ok()?;
    let text_field = body.split(';').nth(2)?;
    if text_field.is_empty() {
        return None;
    }
    let mut text = String::new();
    for value in text_field.split(':') {
        let codepoint = value.parse::<u32>().ok()?;
        let character = char::from_u32(codepoint)?;
        if character.is_control() {
            return None;
        }
        text.push(character);
    }
    (!text.is_empty()).then_some(text)
}

fn decode_key(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    if bytes.first() == Some(&0x1b) {
        return decode_escape(bytes);
    }
    if bytes.len() > 1 {
        return decode_utf8_text(bytes);
    }

    let (modifiers, key): (u32, String) = match bytes.first().copied()? {
        0x00 => (4, "SPACE".into()),
        b'\r' => (0, "RETURN".into()),
        b'\n' => (4, "J".into()),
        b'\t' => (0, "TAB".into()),
        0x01..=0x08 | 0x0b..=0x0c | 0x0e..=0x1a => (4, char::from(b'A' + bytes[0] - 1).to_string()),
        0x7f => (0, "BACKSPACE".into()),
        value if value.is_ascii_graphic() || value == b' ' => (0, char::from(value).to_string()),
        _ => return None,
    };
    let encoding = match bytes[0] {
        b'\t' => "legacy byte input; could also be CTRL + I",
        b'\r' => "legacy byte input; could also be CTRL + M",
        b'A'..=b'Z' => "legacy text byte; Shift/CapsLock cannot be distinguished",
        _ => "legacy byte input",
    };
    Some((
        KeyCombo::from_parts(modifiers, key),
        encoding.into(),
        KeyEventType::Press,
    ))
}

fn decode_utf8_text(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() || character.is_control() {
        return None;
    }
    let key = character.to_uppercase().collect::<String>();
    Some((
        KeyCombo::from_parts(0, key),
        "UTF-8 text input; Compose/layout origin cannot be distinguished".into(),
        KeyEventType::Press,
    ))
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

fn decode_escape(bytes: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    if bytes.len() == 1 {
        return Some((
            KeyCombo::from_parts(0, "ESCAPE"),
            "legacy Escape key".into(),
            KeyEventType::Press,
        ));
    }
    if bytes[1] == b'[' {
        return decode_csi(&bytes[2..]);
    }
    if bytes[1] == b'O' {
        return decode_ss3(&bytes[2..]);
    }
    if bytes.len() > 2 {
        let (combo, _, event_type) = decode_utf8_text(&bytes[1..])?;
        return Some((
            KeyCombo::from_parts(combo.modmask() | 8, combo.key()),
            "Alt-prefixed UTF-8 input; layout/Compose origin is ambiguous".into(),
            event_type,
        ));
    }
    if bytes.len() == 2 {
        let (modifiers, key) = decode_simple_byte(bytes[1])?;
        return Some((
            KeyCombo::from_parts(modifiers | 8, key),
            "legacy Alt-prefixed input".into(),
            KeyEventType::Press,
        ));
    }
    None
}

fn decode_csi(body: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let final_byte = *body.last()?;
    let params = std::str::from_utf8(&body[..body.len() - 1]).ok()?;
    if final_byte == b'u' {
        return decode_kitty(params);
    }

    let key = match final_byte {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'P' => "F1",
        b'Q' => "F2",
        b'S' => "F4",
        b'Z' if params.is_empty() => "TAB",
        b'~' => csi_tilde_key(params.split(';').next()?)?,
        _ => return None,
    };
    let modifiers = if final_byte == b'Z' {
        1
    } else {
        params
            .split(';')
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
            .map(xterm_modifiers)
            .unwrap_or(0)
    };
    Some((
        KeyCombo::from_parts(modifiers, key),
        format!("CSI sequence, final byte {}", char::from(final_byte)),
        KeyEventType::Press,
    ))
}

fn csi_tilde_key(number: &str) -> Option<&'static str> {
    Some(match number {
        "1" | "7" => "HOME",
        "2" => "INSERT",
        "3" => "DELETE",
        "4" | "8" => "END",
        "5" => "PAGE_UP",
        "6" => "PAGE_DOWN",
        "11" => "F1",
        "12" => "F2",
        "13" => "F3",
        "14" => "F4",
        "15" => "F5",
        "17" => "F6",
        "18" => "F7",
        "19" => "F8",
        "20" => "F9",
        "21" => "F10",
        "23" => "F11",
        "24" => "F12",
        "25" => "F13",
        "26" => "F14",
        "28" => "F15",
        "29" => "F16",
        "31" => "F17",
        "32" => "F18",
        "33" => "F19",
        "34" => "F20",
        _ => return None,
    })
}

fn decode_ss3(body: &[u8]) -> Option<(KeyCombo, String, KeyEventType)> {
    let key = match *body.last()? {
        b'A' => "UP",
        b'B' => "DOWN",
        b'C' => "RIGHT",
        b'D' => "LEFT",
        b'H' => "HOME",
        b'F' => "END",
        b'P' => "F1",
        b'Q' => "F2",
        b'R' => "F3",
        b'S' => "F4",
        _ => return None,
    };
    Some((
        KeyCombo::from_parts(0, key),
        "SS3 sequence".into(),
        KeyEventType::Press,
    ))
}

fn decode_kitty(params: &str) -> Option<(KeyCombo, String, KeyEventType)> {
    let mut fields = params.split(';');
    let key_field = fields.next()?;
    let codepoint = key_field.split(':').next()?.parse::<u32>().ok()?;
    kitty_alternate_names(key_field)?;
    let modifier_and_event = fields.next().unwrap_or("1");
    let mut modifier_fields = modifier_and_event.split(':');
    let modifier_field = modifier_fields
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("1")
        .parse::<u32>()
        .ok()?;
    let event_type = match modifier_fields.next().unwrap_or("1") {
        "1" => KeyEventType::Press,
        "2" => KeyEventType::Repeat,
        "3" => KeyEventType::Release,
        // Kitty reserves the event-type field. Treat an unknown value as an
        // unrecognized sequence instead of silently reporting a press.
        _ => return None,
    };
    // A second colon in the modifier/event field is not part of the Kitty
    // grammar. Reject it rather than accepting a malformed event as valid.
    if modifier_fields.next().is_some() {
        return None;
    }
    // The protocol has at most three semicolon-separated fields: key,
    // modifiers/event, and associated text. Extra fields are malformed.
    let _associated_text = fields.next();
    if fields.next().is_some() {
        return None;
    }
    let key = kitty_key_name(codepoint)?;
    Some((
        KeyCombo::from_parts(xterm_modifiers(modifier_field), key),
        "Kitty keyboard protocol".into(),
        event_type,
    ))
}

fn kitty_key_name(codepoint: u32) -> Option<String> {
    Some(match codepoint {
        9 => "TAB".into(),
        13 => "RETURN".into(),
        27 => "ESCAPE".into(),
        32 => "SPACE".into(),
        57358 => "CAPSLOCK".into(),
        57359 => "SCROLLLOCK".into(),
        57360 => "NUMLOCK".into(),
        57361 => "PRINTSCREEN".into(),
        57362 => "PAUSE".into(),
        57363 => "MENU".into(),
        57376..=57398 => format!("F{}", codepoint - 57363),
        57399..=57408 => format!("KP_{}", codepoint - 57399),
        57409 => "KP_DECIMAL".into(),
        57410 => "KP_DIVIDE".into(),
        57411 => "KP_MULTIPLY".into(),
        57412 => "KP_SUBTRACT".into(),
        57413 => "KP_ADD".into(),
        57414 => "KP_ENTER".into(),
        57415 => "KP_EQUAL".into(),
        57416 => "KP_SEPARATOR".into(),
        57417 => "KP_LEFT".into(),
        57418 => "KP_RIGHT".into(),
        57419 => "KP_UP".into(),
        57420 => "KP_DOWN".into(),
        57421 => "KP_PAGE_UP".into(),
        57422 => "KP_PAGE_DOWN".into(),
        57423 => "KP_HOME".into(),
        57424 => "KP_END".into(),
        57425 => "KP_INSERT".into(),
        57426 => "KP_DELETE".into(),
        57427 => "KP_BEGIN".into(),
        57428 => "MEDIA_PLAY".into(),
        57429 => "MEDIA_PAUSE".into(),
        57430 => "MEDIA_PLAY_PAUSE".into(),
        57431 => "MEDIA_REVERSE".into(),
        57432 => "MEDIA_STOP".into(),
        57433 => "MEDIA_FAST_FORWARD".into(),
        57434 => "MEDIA_REWIND".into(),
        57435 => "MEDIA_TRACK_NEXT".into(),
        57436 => "MEDIA_TRACK_PREVIOUS".into(),
        57437 => "MEDIA_RECORD".into(),
        57438 => "LOWER_VOLUME".into(),
        57439 => "RAISE_VOLUME".into(),
        57440 => "MUTE_VOLUME".into(),
        57441 => "LEFT_SHIFT".into(),
        57442 => "LEFT_CONTROL".into(),
        57443 => "LEFT_ALT".into(),
        57444 => "LEFT_SUPER".into(),
        57445 => "LEFT_HYPER".into(),
        57446 => "LEFT_META".into(),
        57447 => "RIGHT_SHIFT".into(),
        57448 => "RIGHT_CONTROL".into(),
        57449 => "RIGHT_ALT".into(),
        57450 => "RIGHT_SUPER".into(),
        57451 => "RIGHT_HYPER".into(),
        57452 => "RIGHT_META".into(),
        57453 => "ISO_LEVEL3_SHIFT".into(),
        57454 => "ISO_LEVEL5_SHIFT".into(),
        0 => "TEXT".into(),
        value => {
            let character = char::from_u32(value)?;
            if character.is_control() {
                return None;
            }
            character.to_uppercase().collect()
        }
    })
}

fn decode_simple_byte(byte: u8) -> Option<(u32, String)> {
    if (0x01..=0x1a).contains(&byte) {
        return Some((4, char::from(b'A' + byte - 1).to_string()));
    }
    if byte.is_ascii_alphabetic() {
        return Some((0, char::from(byte).to_ascii_uppercase().to_string()));
    }
    match byte {
        b'\r' => Some((0, "RETURN".into())),
        b'\t' => Some((0, "TAB".into())),
        b' ' => Some((0, "SPACE".into())),
        _ => None,
    }
}

fn xterm_modifiers(value: u32) -> u32 {
    let value = value.saturating_sub(1);
    let mut modifiers = 0;
    if value & 1 != 0 {
        modifiers |= 1;
    }
    if value & 2 != 0 {
        modifiers |= 8;
    }
    if value & 4 != 0 {
        modifiers |= 4;
    }
    if value & 8 != 0 {
        modifiers |= 64;
    }
    if value & 16 != 0 {
        modifiers |= 32;
    }
    if value & 32 != 0 {
        modifiers |= 128;
    }
    if value & 64 != 0 {
        modifiers |= 2;
    }
    if value & 128 != 0 {
        modifiers |= 16;
    }
    modifiers
}

