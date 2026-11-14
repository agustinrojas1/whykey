use crate::layers::{Checked, EvidenceTier, LayerId, LayerResult, Outcome, TerminalInput};

pub struct Terminal;

impl Terminal {
    pub fn inspect_input(&self, input: &TerminalInput) -> LayerResult {
        // No byte of the input is a special character: nothing to read.
        if !needs_termios(input) {
            return no_control_byte(input);
        }
        let state = match read_tty_state() {
            Ok(state) => state,
            Err(message) => {
                return LayerResult::new(
                    "TTY driver",
                    LayerId::Tty,
                    Outcome::Unavailable,
                    "could not inspect terminal settings",
                    vec![message, format_input(input)],
                );
            }
        };
        self.inspect_sequence(input, &state)
    }

    /// Inspect input against the terminal of another process (`inspect
    /// --pid`, `--focused`). That terminal's settings, not whykey's own, say
    /// what the TTY driver does with the byte. When they cannot be read the
    /// answer is unknown; whykey's own terminal is never substituted.
    pub fn inspect_input_for_target(&self, input: &TerminalInput, target_pid: u32) -> LayerResult {
        if !needs_termios(input) {
            return no_control_byte(input);
        }
        self.inspect_input_for_target_termios(input, crate::layers::tty_target::read(target_pid))
    }

    fn inspect_input_for_target_termios(
        &self,
        input: &TerminalInput,
        settings: Result<crate::layers::tty_target::TargetTermios, String>,
    ) -> LayerResult {
        match settings {
            Ok(target) => {
                let state = tty_state_from_termios(&target.termios);
                let mut result = self.inspect_input_with_state(input, &state);
                result.details.extend(target.details);
                result
            }
            Err(message) => LayerResult::new(
                "TTY driver",
                LayerId::Tty,
                Outcome::Unknown,
                "could not read the target's terminal settings; TTY checks unevaluated",
                vec![
                    message,
                    "whykey's own terminal is not substituted: the target may use different \
                     settings"
                        .into(),
                    format_input(input),
                ],
            ),
        }
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
    pub fn inspect_unknown(&self) -> LayerResult {
        LayerResult::new(
            "TTY driver",
            LayerId::Tty,
            Outcome::Unknown,
            "terminal byte encoding is unknown; TTY signal checks unevaluated",
            vec![
                "termios special-character processing depends on bytes emitted by the terminal"
                    .into(),
            ],
        )
    }

    fn inspect_input_with_state(&self, input: &TerminalInput, state: &TtyState) -> LayerResult {
        self.inspect_sequence(input, state)
    }

    /// The TTY driver reads every byte the terminal writes: a control byte
    /// anywhere in a sequence (ESC followed by Ctrl+C's 0x03) is acted on.
    /// The first byte it consumes decides; otherwise the key passes. The
    /// bytes are judged against termios settings read from the terminal
    /// device, so the answer is a live query.
    fn inspect_sequence(&self, input: &TerminalInput, state: &TtyState) -> LayerResult {
        let mut results = input
            .bytes
            .iter()
            .map(|byte| classify_byte(*byte, input, state));
        let mut result = results
            .by_ref()
            .find(|result| result.outcome != Outcome::Pass)
            .unwrap_or_else(|| {
                termios_pass(
                    "does not consume this byte sequence",
                    vec![format_input(input)],
                )
            })
            .with_tier(EvidenceTier::LiveQuery);
        if let Some(alias) = legacy_byte_alias(&input.bytes) {
            result.details.push(alias.into());
        }
        result
    }
}

/// True when the terminal's settings are needed: a single byte is always
/// judged against them, a longer sequence only when one of its bytes can be a
/// special character (ESC is not one by default, and the escape sequences of
/// cursor and function keys hold none).
fn needs_termios(input: &TerminalInput) -> bool {
    input.bytes.len() == 1
        || input
            .bytes
            .iter()
            .any(|byte| (*byte < 0x20 && *byte != 0x1b) || *byte == 0x7f)
}

/// The input has no byte the driver can act on.
fn no_control_byte(input: &TerminalInput) -> LayerResult {
    LayerResult::pass(
        "TTY driver",
        LayerId::Tty,
        "does not consume this byte sequence",
        vec![format_input(input)],
        Checked::new(
            EvidenceTier::Default,
            ["the input bytes (none is a terminal special character)"],
        ),
    )
}

/// A pass from the termios settings the terminal device reported.
fn termios_pass(summary: impl Into<String>, details: Vec<String>) -> LayerResult {
    LayerResult::pass(
        "TTY driver",
        LayerId::Tty,
        summary,
        details,
        Checked::new(
            EvidenceTier::LiveQuery,
            ["termios settings of the terminal (ISIG, IXON, ICANON, IEXTEN, special characters)"],
        ),
    )
}

fn classify_byte(byte: u8, input: &TerminalInput, state: &TtyState) -> LayerResult {
    let Some(control) = control_character_byte(byte, state) else {
        return termios_pass(
            "does not consume this byte sequence",
            vec![format_input(input)],
        );
    };

    let configured = format_control_char(control.byte);
    let details = || {
        vec![
            format!("special character: {} = {configured}", control.name),
            format!("byte: 0x{:02x}", control.byte),
        ]
    };

    match control.kind {
        ControlKind::Signal if !state.signal_processing => {
            termios_pass("signal processing is disabled; byte is forwarded", {
                let mut details = details();
                details.push("ISIG is disabled".into());
                details
            })
        }
        ControlKind::Signal => LayerResult::new(
            "TTY driver",
            LayerId::Tty,
            Outcome::Consumed,
            format!(
                "terminal signal handling; interprets byte 0x{byte:02x} as {} ({})",
                control.name, control.signal
            ),
            {
                let mut details = details();
                details.push(format!(
                    "ISIG is enabled; kernel sends {} to the foreground process",
                    control.signal
                ));
                details
            },
        ),
        ControlKind::Flow if !state.input_flow_control => termios_pass(
            "output flow control (IXON) is disabled; byte is forwarded",
            {
                let mut details = details();
                details.push("IXON is disabled".into());
                details
            },
        ),
        ControlKind::Flow => LayerResult::new(
            "TTY driver",
            LayerId::Tty,
            Outcome::Consumed,
            format!(
                "terminal flow control (IXON); interprets byte 0x{byte:02x} as {}",
                control.name
            ),
            {
                let mut details = details();
                details.push(format!(
                    "IXON is enabled; the TTY consumes this byte and {} terminal output",
                    if control.name == "VSTOP" {
                        "suspends"
                    } else {
                        "resumes"
                    }
                ));
                details
            },
        ),
        ControlKind::Line if !state.canonical => {
            termios_pass("canonical line editing is disabled; byte is forwarded", {
                let mut details = details();
                details.push("ICANON is disabled".into());
                details
            })
        }
        ControlKind::Line
            if matches!(control.name, "VLNEXT" | "VDISCARD") && !state.extended_processing =>
        {
            termios_pass(
                "extended terminal processing is disabled; byte is forwarded",
                {
                    let mut details = details();
                    details.push("IEXTEN is disabled".into());
                    details
                },
            )
        }
        ControlKind::Line => termios_pass(
            format!("{} is configured for canonical line editing", control.name),
            {
                let mut details = details();
                details.push("Bash/Readline may handle this byte while editing a command".into());
                details
            },
        ),
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

/// Legacy (non-Kitty-protocol) control byte for Ctrl plus `key`, where `key`
/// is a normalized `KeyCombo::key()` name: Ctrl+letter, Ctrl+@ / Ctrl+Space
/// (0x00), Ctrl+[ (0x1b), Ctrl+\ (0x1c), Ctrl+] (0x1d), Ctrl+^ (0x1e) and
/// Ctrl+_ (0x1f). Shared by terminal encoders so the TTY and shell layers
/// see the same byte.
pub(crate) fn legacy_control_byte(key: &str) -> Option<u8> {
    match key.as_bytes() {
        b"SPACE" => Some(0),
        [character] if character.is_ascii_alphabetic() => {
            Some(character.to_ascii_uppercase() & 0x1f)
        }
        [character @ (b'@' | b'[' | b'\\' | b']' | b'^' | b'_')] => Some(character & 0x1f),
        _ => None,
    }
}

/// Keys that are indistinguishable from `bytes` in legacy terminal encoding.
pub(crate) fn legacy_byte_alias(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x00] => {
            Some("0x00 is sent for both Ctrl+@ and Ctrl+Space; programs cannot tell them apart")
        }
        [0x08] => Some(
            "0x08 is Ctrl+H; terminals configured to send ^H for Backspace produce the same byte",
        ),
        [0x09] => Some("0x09 is also Tab; Ctrl+I and Tab are indistinguishable in legacy encoding"),
        [0x0d] => {
            Some("0x0d is also Return; Ctrl+M and Return are indistinguishable in legacy encoding")
        }
        [0x1b] => Some(
            "0x1b is also Escape; Ctrl+[ and Escape are indistinguishable, and 0x1b starts Alt/escape sequences",
        ),
        _ => None,
    }
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
    let termios = crate::recording::tty_termios()?;
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

    fn termios_with(lflag: libc::tcflag_t, iflag: libc::tcflag_t) -> libc::termios {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        termios.c_lflag = lflag;
        termios.c_iflag = iflag;
        termios.c_cc[libc::VINTR] = 0x03;
        termios.c_cc[libc::VQUIT] = 0x1c;
        termios.c_cc[libc::VSUSP] = 0x1a;
        termios.c_cc[libc::VSTART] = 0x11;
        termios.c_cc[libc::VSTOP] = 0x13;
        termios
    }

    fn inspect_byte(byte: u8, termios: &libc::termios) -> LayerResult {
        let input = TerminalInput::predicted(vec![byte], "legacy control byte");
        Terminal.inspect_input_with_termios(&input, termios)
    }

    #[test]
    fn labels_ctrl_s_and_ctrl_q_as_flow_control_only_when_ixon_is_set() {
        let with_ixon = termios_with(libc::ISIG, libc::IXON);
        for byte in [0x13, 0x11] {
            let result = inspect_byte(byte, &with_ixon);
            assert_eq!(result.outcome, Outcome::Consumed);
            assert!(result.summary.starts_with("terminal flow control (IXON)"));
            assert!(!result.summary.contains("signal"));
        }

        let without_ixon = termios_with(libc::ISIG, 0);
        for byte in [0x13, 0x11] {
            assert_eq!(inspect_byte(byte, &without_ixon).outcome, Outcome::Pass);
        }
    }

    #[test]
    fn signal_characters_are_consumed_only_when_isig_is_set() {
        let with_isig = termios_with(libc::ISIG, 0);
        for (byte, signal) in [(0x03, "SIGINT"), (0x1c, "SIGQUIT"), (0x1a, "SIGTSTP")] {
            let result = inspect_byte(byte, &with_isig);
            assert_eq!(result.outcome, Outcome::Consumed);
            assert!(result.summary.starts_with("terminal signal handling"));
            assert!(result.summary.contains(signal));
        }

        let without_isig = termios_with(0, libc::IXON);
        for byte in [0x03, 0x1c, 0x1a] {
            assert_eq!(inspect_byte(byte, &without_isig).outcome, Outcome::Pass);
        }
    }

    #[test]
    fn termios_answers_are_live_and_multibyte_input_has_no_source() {
        let termios = termios_with(libc::ISIG, libc::IXON);
        for byte in [0x03, 0x13, b'a'] {
            assert_eq!(
                inspect_byte(byte, &termios).tier,
                Some(EvidenceTier::LiveQuery)
            );
        }
        // A sequence is judged byte by byte against the same settings, and
        // one without a special character needs none.
        let input = TerminalInput::predicted(b"\x1b[A".to_vec(), "cursor key");
        let result = Terminal.inspect_input_with_termios(&input, &termios);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(
            no_control_byte(&input).tier,
            Some(EvidenceTier::Default),
            "without termios the pass rests on the bytes alone"
        );
        // A control byte inside the sequence is acted on, whatever precedes it.
        let alt_ctrl_c = TerminalInput::predicted(b"\x1b\x03".to_vec(), "Ctrl+Alt+C");
        assert_eq!(
            Terminal
                .inspect_input_with_termios(&alt_ctrl_c, &termios)
                .outcome,
            Outcome::Consumed
        );
    }

    #[test]
    fn encodes_ctrl_punctuation_as_legacy_control_bytes() {
        for (key, byte) in [
            ("]", 0x1d),
            ("[", 0x1b),
            ("\\", 0x1c),
            ("^", 0x1e),
            ("_", 0x1f),
            ("@", 0x00),
            ("SPACE", 0x00),
            ("X", 0x18),
        ] {
            assert_eq!(legacy_control_byte(key), Some(byte), "ctrl+{key}");
        }
        assert_eq!(legacy_control_byte("1"), None);
        assert_eq!(legacy_control_byte("LEFT"), None);
    }

    #[test]
    fn states_escape_and_tab_ambiguity_in_tty_evidence() {
        let termios = termios_with(libc::ISIG, 0);
        for (byte, key) in [(0x1b, "Escape"), (0x09, "Tab")] {
            let result = inspect_byte(byte, &termios);
            assert!(
                result.details.iter().any(|detail| detail.contains(key)),
                "{:?}",
                result.details
            );
        }
    }

    fn target_termios(
        lflag: libc::tcflag_t,
        iflag: libc::tcflag_t,
    ) -> crate::layers::tty_target::TargetTermios {
        crate::layers::tty_target::TargetTermios {
            termios: termios_with(lflag, iflag),
            details: vec!["termios read from /dev/pts/9, the controlling terminal of pid 7".into()],
        }
    }

    #[test]
    fn a_raw_mode_target_forwards_the_bytes_a_cooked_terminal_would_consume() {
        let input = TerminalInput::predicted(vec![0x13], "legacy control byte");
        let cooked = termios_with(libc::ISIG | libc::ICANON, libc::IXON);
        assert_eq!(
            Terminal.inspect_input_with_termios(&input, &cooked).outcome,
            Outcome::Consumed
        );
        // vim and less clear ISIG, ICANON and IXON on their terminal.
        let result = Terminal.inspect_input_for_target_termios(&input, Ok(target_termios(0, 0)));
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("/dev/pts/9")),
            "{:?}",
            result.details
        );
        let interrupt = TerminalInput::predicted(vec![0x03], "legacy control byte");
        let result =
            Terminal.inspect_input_for_target_termios(&interrupt, Ok(target_termios(0, 0)));
        assert_eq!(result.outcome, Outcome::Pass);
        let result = Terminal
            .inspect_input_for_target_termios(&interrupt, Ok(target_termios(libc::ISIG, 0)));
        assert_eq!(result.outcome, Outcome::Consumed);
    }

    #[test]
    fn an_unreadable_target_terminal_is_unknown_not_whykeys_own() {
        let input = TerminalInput::predicted(vec![0x13], "legacy control byte");
        let result = Terminal.inspect_input_for_target_termios(
            &input,
            Err("failed to open /dev/pts/9: Permission denied (os error 13)".into()),
        );
        assert_eq!(result.outcome, Outcome::Unknown);
        assert_ne!(result.tier, Some(EvidenceTier::LiveQuery));
        assert!(
            result.summary.contains("could not read"),
            "{}",
            result.summary
        );
        assert!(
            result.details[0].contains("Permission denied"),
            "{:?}",
            result.details
        );
    }

    #[test]
    fn a_multibyte_target_input_needs_no_terminal_read() {
        let input = TerminalInput::predicted(b"\x1b[A".to_vec(), "cursor key");
        // No process has this id, so a read would fail; none is attempted.
        let result = Terminal.inspect_input_for_target(&input, u32::MAX);
        assert_eq!(result.outcome, Outcome::Pass);
    }
}
