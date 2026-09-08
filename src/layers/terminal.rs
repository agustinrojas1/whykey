use std::fs::File;
use std::os::fd::AsRawFd;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, TerminalInput};

pub struct Terminal;

impl Terminal {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(byte) = control_byte_for_key(key) else {
            return LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "no matching terminal special character".into(),
                details: vec![],
            };
        };

        let input = TerminalInput::predicted(vec![byte], "legacy control byte");
        let state = match read_tty_state() {
            Ok(state) => state,
            Err(message) => {
                return LayerResult {
                    layer: "TTY driver",
                    id: LayerId::Tty,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect terminal settings".into(),
                    details: vec![message, format_input(&input)],
                };
            }
        };
        self.inspect_byte(byte, &input, &state)
    }
}

impl Terminal {
    pub fn inspect_input(&self, input: &TerminalInput) -> LayerResult {
        let [byte] = input.bytes.as_slice() else {
            return LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "does not consume this byte sequence".into(),
                details: vec![format_input(input)],
            };
        };
        let state = match read_tty_state() {
            Ok(state) => state,
            Err(message) => {
                return LayerResult {
                    layer: "TTY driver",
                    id: LayerId::Tty,
                    outcome: Outcome::Unavailable,
                    summary: "could not inspect terminal settings".into(),
                    details: vec![message, format_input(input)],
                };
            }
        };
        self.inspect_byte(*byte, input, &state)
    }

    /// Inspect input with a caller-provided termios snapshot. Capture mode
    /// uses this because the live terminal is deliberately in raw mode while
    /// reports are rendered.
    pub fn inspect_input_with_termios(
        &self,
        input: &TerminalInput,
        termios: &libc::termios,
    ) -> LayerResult {
        let state = tty_state_from_termios(termios);
        self.inspect_input_with_state(input, &state)
    }

    fn inspect_input_with_state(&self, input: &TerminalInput, state: &TtyState) -> LayerResult {
        let [byte] = input.bytes.as_slice() else {
            return LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "does not consume this byte sequence".into(),
                details: vec![format_input(input)],
            };
        };
        self.inspect_byte(*byte, input, state)
    }

    fn inspect_byte(&self, byte: u8, input: &TerminalInput, state: &TtyState) -> LayerResult {
        let Some(control) = control_character_byte(byte, state) else {
            return LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "does not consume this byte sequence".into(),
                details: vec![format_input(input)],
            };
        };

        let configured = format_control_char(control.byte);
        let details = || {
            vec![
                format!("special character: {} = {configured}", control.name),
                format!("byte: 0x{:02x}", control.byte),
            ]
        };

        match control.kind {
            ControlKind::Signal if !state.signal_processing => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "signal processing is disabled; byte is forwarded".into(),
                details: {
                    let mut details = details();
                    details.push("ISIG is disabled".into());
                    details
                },
            },
            ControlKind::Signal => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Consumed,
                summary: format!("interprets byte 0x{:02x} as {}", byte, control.name),
                details: {
                    let mut details = details();
                    details.push(format!(
                        "kernel sends {} to the foreground process",
                        control.signal
                    ));
                    details
                },
            },
            ControlKind::Flow if !state.input_flow_control => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "input flow control is disabled; byte is forwarded".into(),
                details: {
                    let mut details = details();
                    details.push("IXON is disabled".into());
                    details
                },
            },
            ControlKind::Flow => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Consumed,
                summary: format!("interprets byte 0x{:02x} as {}", byte, control.name),
                details: {
                    let mut details = details();
                    details.push("terminal flow control consumes this byte".into());
                    details
                },
            },
            ControlKind::Line if !state.canonical => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: "canonical line editing is disabled; byte is forwarded".into(),
                details: {
                    let mut details = details();
                    details.push("ICANON is disabled".into());
                    details
                },
            },
            ControlKind::Line
                if matches!(control.name, "VLNEXT" | "VDISCARD") && !state.extended_processing =>
            {
                LayerResult {
                    layer: "TTY driver",
                    id: LayerId::Tty,
                    outcome: Outcome::Pass,
                    summary: "extended terminal processing is disabled; byte is forwarded".into(),
                    details: {
                        let mut details = details();
                        details.push("IEXTEN is disabled".into());
                        details
                    },
                }
            }
            ControlKind::Line => LayerResult {
                layer: "TTY driver",
                id: LayerId::Tty,
                outcome: Outcome::Pass,
                summary: format!("{} is configured for canonical line editing", control.name),
                details: {
                    let mut details = details();
                    details
                        .push("Bash/Readline may handle this byte while editing a command".into());
                    details
                },
            },
        }
    }
}

fn format_input(input: &TerminalInput) -> String {
    let bytes = input.display_bytes();
    format!(
        "normal input: {bytes} ({}, {})",
        input.confidence_label(),
        input.description
    )
}

#[derive(Clone, Copy)]
struct ControlCharacter {
    byte: u8,
    name: &'static str,
    kind: ControlKind,
    signal: &'static str,
}

#[derive(Clone, Copy)]
enum ControlKind {
    Signal,
    Flow,
    Line,
}

fn control_byte_for_key(key: &KeyCombo) -> Option<u8> {
    if key.modmask() & 4 == 0 || key.modmask() & !(4 | 1) != 0 {
        return None;
    }
    if key.key() == "SPACE" {
        return Some(0);
    }
    let key = key.key().as_bytes();
    if key.len() != 1 {
        return None;
    }
    match key[0] {
        b'A'..=b'Z' => Some(key[0] & 0x1f),
        b'@'..=b'_' => Some(key[0] & 0x1f),
        _ => None,
    }
}

fn control_character_byte(byte: u8, state: &TtyState) -> Option<ControlCharacter> {
    let controls = [
        (state.intr, "VINTR", ControlKind::Signal, "SIGINT"),
        (state.quit, "VQUIT", ControlKind::Signal, "SIGQUIT"),
        (state.suspend, "VSUSP", ControlKind::Signal, "SIGTSTP"),
        (state.start, "VSTART", ControlKind::Flow, ""),
        (state.stop, "VSTOP", ControlKind::Flow, ""),
        (state.eof, "VEOF", ControlKind::Line, ""),
        (state.erase, "VERASE", ControlKind::Line, ""),
        (state.kill, "VKILL", ControlKind::Line, ""),
        (state.reprint, "VREPRINT", ControlKind::Line, ""),
        (state.werase, "VWERASE", ControlKind::Line, ""),
        (state.lnext, "VLNEXT", ControlKind::Line, ""),
        (state.discard, "VDISCARD", ControlKind::Line, ""),
        (state.eol, "VEOL", ControlKind::Line, ""),
        (state.eol2, "VEOL2", ControlKind::Line, ""),
    ];
    controls
        .into_iter()
        .find_map(|(configured, name, kind, signal)| {
            (configured == Some(byte)).then_some(ControlCharacter {
                byte,
                name,
                kind,
                signal,
            })
        })
}

fn format_control_char(value: u8) -> String {
    match value {
        0x00..=0x1f => format!("^{}", char::from(value + b'@')),
        0x7f => "^?".into(),
        value => char::from(value).to_string(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TtyState {
    signal_processing: bool,
    canonical: bool,
    input_flow_control: bool,
    extended_processing: bool,
    intr: Option<u8>,
    quit: Option<u8>,
    erase: Option<u8>,
    kill: Option<u8>,
    eof: Option<u8>,
    suspend: Option<u8>,
    start: Option<u8>,
    stop: Option<u8>,
    reprint: Option<u8>,
    werase: Option<u8>,
    lnext: Option<u8>,
    discard: Option<u8>,
    eol: Option<u8>,
    eol2: Option<u8>,
}

fn read_tty_state() -> Result<TtyState, String> {
    let tty =
        File::open("/dev/tty").map_err(|error| format!("failed to open /dev/tty: {error}"))?;
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(tty.as_raw_fd(), &mut termios) } != 0 {
        return Err(format!(
            "failed to read terminal settings: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(tty_state_from_termios(&termios))
}

fn tty_state_from_termios(termios: &libc::termios) -> TtyState {
    TtyState {
        signal_processing: termios.c_lflag & libc::ISIG != 0,
        canonical: termios.c_lflag & libc::ICANON != 0,
        input_flow_control: termios.c_iflag & libc::IXON != 0,
        extended_processing: termios.c_lflag & libc::IEXTEN != 0,
        intr: control_char(termios.c_cc[libc::VINTR]),
        quit: control_char(termios.c_cc[libc::VQUIT]),
        erase: control_char(termios.c_cc[libc::VERASE]),
        kill: control_char(termios.c_cc[libc::VKILL]),
        eof: control_char(termios.c_cc[libc::VEOF]),
        suspend: control_char(termios.c_cc[libc::VSUSP]),
        start: control_char(termios.c_cc[libc::VSTART]),
        stop: control_char(termios.c_cc[libc::VSTOP]),
        reprint: control_char(termios.c_cc[libc::VREPRINT]),
        werase: control_char(termios.c_cc[libc::VWERASE]),
        lnext: control_char(termios.c_cc[libc::VLNEXT]),
        discard: control_char(termios.c_cc[libc::VDISCARD]),
        eol: control_char(termios.c_cc[libc::VEOL]),
        eol2: control_char(termios.c_cc[libc::VEOL2]),
    }
}

fn control_char(value: libc::cc_t) -> Option<u8> {
    if value == libc::_POSIX_VDISABLE {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{LayerStatus, Propagation};

    #[test]
    fn recognizes_a_vsusp_binding() {
        assert_eq!(control_char(b'\x03'), Some(b'\x03'));
        assert_eq!(control_char(b'\x1a'), Some(b'\x1a'));
        assert_eq!(format_control_char(b'\x03'), "^C");
        assert_eq!(format_control_char(b'\x1a'), "^Z");
    }

    #[test]
    fn recognizes_isig_and_another_suspend_character() {
        assert_eq!(control_char(b'\x19'), Some(b'\x19'));
        assert_eq!(control_char(libc::_POSIX_VDISABLE), None);
    }

    #[test]
    fn uses_a_saved_termios_snapshot_during_capture() {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        termios.c_lflag = libc::ISIG;
        termios.c_cc[libc::VSUSP] = b'\x1a';
        let input = TerminalInput::configured(vec![b'\x1a'], "captured control byte");

        let result = Terminal.inspect_input_with_termios(&input, &termios);

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
        assert!(result.summary.contains("VSUSP"));
    }
}
