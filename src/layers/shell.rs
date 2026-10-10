//! Shell line editors: Bash/Readline, Zsh/ZLE and Fish.
//!
//! The shell that receives a key at its prompt is the interactive shell that
//! runs whykey, so it is found from the process tree rather than `$SHELL`
//! (the login shell, which is often a different program). Bindings come from
//! that shell's own dump when `whykey shell-init` supplied one; otherwise
//! from the installed shell's built-in defaults plus a static read of its
//! startup files, reported as unverified.

mod fish;
mod zle;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::layers::{
    BindingEvidence, BindingScope, LayerId, LayerResult, Outcome, TerminalInput, UncertaintyReason,
    readline,
};

pub fn inspect_input(input: &TerminalInput) -> LayerResult {
    inspect_sequence(std::slice::from_ref(input), None)
}

/// Inspect the shell associated with a caller-selected process. This avoids
/// using whykey's own shell when `inspect --pid` or `inspect --focused`
/// targets a different terminal session.
pub fn inspect_input_for_pid(input: &TerminalInput, target_pid: u32) -> LayerResult {
    inspect_sequence(std::slice::from_ref(input), Some(target_pid))
}

/// Evaluate the terminal inputs of consecutive key presses that reached the
/// shell, in order, and describe what the last one does. Each line editor
/// replays them through its keymap (prefix keys such as `\C-x` stay
/// pending), so `[\x18], [\x05]` resolves to `edit-and-execute-command` in
/// Bash. `target_pid` selects that process's ancestor shell instead of the
/// shell running whykey.
pub fn inspect_sequence(inputs: &[TerminalInput], target_pid: Option<u32>) -> LayerResult {
    let shell = match target_pid {
        Some(pid) => ancestor_shell(pid).or_else(running_shell),
        None => running_shell(),
    };
    let Some(shell) = shell else {
        return unsupported("unknown shell");
    };
    match shell.name.as_str() {
        "bash" => readline::Readline.inspect_sequence(inputs),
        "zsh" => zle::inspect(inputs, shell.exe.as_deref()),
        "fish" => fish::inspect(inputs, shell.exe.as_deref()),
        other => unsupported(other),
    }
}

pub fn inspect_unknown() -> LayerResult {
    LayerResult::new(
        "Shell input",
        LayerId::Shell,
        Outcome::Unknown,
        "terminal byte encoding is unknown; shell line editing unevaluated",
        vec![
            "shell line editors (Readline, ZLE, Fish) match against terminal byte sequences".into(),
        ],
    )
}

fn unsupported(shell: &str) -> LayerResult {
    LayerResult::new(
        "Shell input",
        LayerId::Shell,
        Outcome::Unavailable,
        format!("shell '{shell}' is not inspected"),
        vec!["install shell integration or use a supported shell adapter".into()],
    )
}

/// A shell process whose line editor receives keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellProcess {
    /// `bash`, `zsh`, `fish`, or the basename of `$SHELL` for other shells.
    pub name: String,
    /// The running executable, so defaults are read from the same build.
    pub exe: Option<PathBuf>,
}

/// The shell whose prompt receives keys typed in this terminal: the shell
/// that supplied a shell-init snapshot, else the nearest interactive shell
/// among whykey's ancestors (reached only through other shells, e.g. an
/// `sh -c` wrapper), else `$SHELL`.
pub fn running_shell() -> Option<ShellProcess> {
    let snapshot = if env::var_os("WHYKEY_READLINE_BINDINGS").is_some()
        || env::var_os("WHYKEY_READLINE_MACROS").is_some()
        || env::var_os("WHYKEY_READLINE_SHELL_BINDINGS").is_some()
    {
        Some("bash")
    } else if env::var_os("WHYKEY_ZLE_BINDINGS").is_some() {
        Some("zsh")
    } else if env::var_os("WHYKEY_FISH_BINDINGS").is_some() {
        Some("fish")
    } else {
        None
    };
    let parent = parent_shell();
    if let Some(name) = snapshot {
        let exe = parent
            .filter(|shell| shell.name == name)
            .and_then(|shell| shell.exe);
        return Some(ShellProcess {
            name: name.into(),
            exe,
        });
    }
    parent.or_else(login_shell)
}

fn login_shell() -> Option<ShellProcess> {
    let shell = env::var_os("SHELL")?;
    let name = Path::new(&shell)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(ShellProcess { name, exe: None })
}

/// Walk up from whykey's parent while the processes are shells; the first
/// interactive one is the line editor. A non-shell ancestor (a test runner,
/// an editor, a launcher) ends the walk: keys typed there do not reach a
/// shell prompt that this process can identify.
fn parent_shell() -> Option<ShellProcess> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..16 {
        let process = ProcessInfo::read(pid)?;
        let name = process.shell_name()?;
        if process.interactive() {
            return Some(ShellProcess {
                name: name.into(),
                exe: process.exe(),
            });
        }
        pid = process.parent?;
    }
    None
}

/// The nearest interactive shell at or above `start_pid`, skipping any
/// non-shell processes in between (`inspect --pid`).
fn ancestor_shell(start_pid: u32) -> Option<ShellProcess> {
    let mut pid = start_pid;
    for _ in 0..16 {
        let process = ProcessInfo::read(pid)?;
        if let Some(name) = process.shell_name() {
            if process.interactive() {
                return Some(ShellProcess {
                    name: name.into(),
                    exe: process.exe(),
                });
            }
        }
        pid = process.parent?;
    }
    None
}

struct ProcessInfo {
    pid: u32,
    argv: Vec<String>,
    parent: Option<u32>,
}

impl ProcessInfo {
    fn read(pid: u32) -> Option<Self> {
        let command = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let argv = command
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(") ")?;
        let parent = stat[close + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|field| field.parse::<u32>().ok())
            .filter(|parent| *parent != 0 && *parent != pid);
        Some(Self { pid, argv, parent })
    }

    /// Login shells run as `-zsh`; the basename names the program.
    fn shell_name(&self) -> Option<&'static str> {
        let program = self.argv.first()?;
        let name = Path::new(program.trim_start_matches('-'))
            .file_name()?
            .to_str()?;
        ["bash", "zsh", "fish"]
            .into_iter()
            .find(|shell| *shell == name)
    }

    /// A shell running `-c COMMAND` or a script file never shows a prompt.
    fn interactive(&self) -> bool {
        shell_reads_terminal(&self.argv[1..])
    }

    fn exe(&self) -> Option<PathBuf> {
        fs::read_link(format!("/proc/{}/exe", self.pid)).ok()
    }
}

/// Options of bash, zsh and fish that consume the following argument.
const OPTIONS_WITH_VALUE: &[&str] = &[
    "-o",
    "+o",
    "-O",
    "+O",
    "--rcfile",
    "--init-file",
    "-C",
    "--init-command",
    "-d",
    "--debug",
    "--debug-output",
    "-p",
    "--profile",
    "--profile-startup",
    "--features",
];

fn shell_reads_terminal(arguments: &[String]) -> bool {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let argument = argument.as_str();
        if argument == "--command" || argument.starts_with("--command=") {
            return false;
        }
        if OPTIONS_WITH_VALUE.contains(&argument) {
            arguments.next();
            continue;
        }
        if argument == "-" || argument == "--" {
            // Options end; anything after is a script operand.
            return arguments.next().is_none();
        }
        if let Some(cluster) = argument.strip_prefix('-') {
            if !cluster.starts_with('-') && cluster.contains('c') {
                return false;
            }
            continue;
        }
        if argument.starts_with('+') {
            continue;
        }
        return false;
    }
    true
}

/// Whether a line-editor keymap is the running shell's own dump or a
/// reconstruction from defaults and startup files.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Live,
    Static,
}

/// A line editor's verdict before origin-specific confidence is applied.
struct Verdict {
    outcome: Outcome,
    summary: String,
    /// The binding the verdict rests on, for unverified binding evidence.
    action: Option<String>,
    details: Vec<String>,
}

/// Live dumps are definitive. A static reconstruction cannot see plugins,
/// sourced files or conditionals, so a binding it finds is unverified and an
/// absent binding only means "probably unbound".
fn finish(layer: &'static str, origin: &Origin, init_hint: &str, verdict: Verdict) -> LayerResult {
    let Verdict {
        outcome,
        summary,
        action,
        mut details,
    } = verdict;
    if *origin == Origin::Live {
        return LayerResult::new(layer, LayerId::Shell, outcome, summary, details);
    }
    details.push(init_hint.into());
    let outcome = match outcome {
        Outcome::Consumed => Outcome::HandledUncertain,
        Outcome::Pass => Outcome::UncertainContinues,
        other => other,
    };
    let result = LayerResult::new(layer, LayerId::Shell, outcome, summary, details);
    match action {
        Some(action) => result.with_binding(
            BindingEvidence::action(action, BindingScope::Unknown, None)
                .with_uncertainty(UncertaintyReason::UnverifiedDefault),
        ),
        None => result,
    }
}

/// A shell word from a startup file or binding dump, quotes removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    pub(crate) text: String,
    /// Contains an expansion (`$var`, command substitution, glob) that a
    /// static reading cannot resolve.
    pub(crate) expands: bool,
}

/// Collects words into simple commands while a tokenizer scans source.
#[derive(Default)]
struct Splitter {
    commands: Vec<Vec<Word>>,
    words: Vec<Word>,
    word: String,
    expands: bool,
    started: bool,
}

impl Splitter {
    fn push(&mut self, character: char) {
        self.started = true;
        self.word.push(character);
    }

    fn end_word(&mut self) {
        if self.started {
            self.words.push(Word {
                text: std::mem::take(&mut self.word),
                expands: self.expands,
            });
        }
        self.started = false;
        self.expands = false;
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }

    fn finish(mut self) -> Vec<Vec<Word>> {
        self.end_command();
        self.commands
    }
}

/// Words that may precede a command without being it.
const RESERVED: &[&str] = &[
    "then", "else", "do", "{", "}", "!", "(", "builtin", "command",
];

/// The command words of a simple command, past reserved words such as
/// `then` or `builtin`.
pub(crate) fn command_words(words: &[Word]) -> &[Word] {
    let start = words
        .iter()
        .position(|word| !RESERVED.contains(&word.text.as_str()))
        .unwrap_or(words.len());
    &words[start..]
}

/// Split zsh or Bash source into simple commands of shell words, honouring
/// quotes (`'…'`, `"…"`, `$'…'`), backslashes, comments and the separators
/// `;`, `&`, `|` and newlines.
pub(crate) fn posix_commands(script: &str) -> Vec<Vec<Word>> {
    let mut out = Splitter::default();
    let mut chars = script.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                out.started = true;
                for quoted in chars.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    out.push(quoted);
                }
            }
            '"' => {
                out.started = true;
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(next @ ('$' | '`' | '"' | '\\')) => out.push(next),
                            Some('\n') => {}
                            Some(next) => {
                                out.push('\\');
                                out.push(next);
                            }
                            None => out.push('\\'),
                        },
                        '$' | '`' => {
                            out.expands = true;
                            out.push(quoted);
                        }
                        other => out.push(other),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                out.started = true;
                ansi_c_quoted(&mut chars, &mut out.word);
            }
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(next) => out.push(next),
            },
            '#' if !out.started => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
                out.end_command();
            }
            ';' | '&' | '|' | '\n' => out.end_command(),
            ' ' | '\t' => out.end_word(),
            other => {
                if matches!(other, '$' | '`' | '*' | '?' | '~') {
                    out.expands = true;
                }
                out.push(other);
            }
        }
    }
    out.finish()
}

/// The body of `$'…'` after the opening quote, with escapes decoded.
fn ansi_c_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, word: &mut String) {
    while let Some(character) = chars.next() {
        match character {
            '\'' => return,
            '\\' => {
                let Some(escape) = chars.next() else {
                    return;
                };
                match escape {
                    'e' | 'E' => word.push('\x1b'),
                    'n' => word.push('\n'),
                    'r' => word.push('\r'),
                    't' => word.push('\t'),
                    'a' => word.push('\x07'),
                    'b' => word.push('\x08'),
                    'f' => word.push('\x0c'),
                    'v' => word.push('\x0b'),
                    'x' => {
                        let mut value = 0u32;
                        for _ in 0..2 {
                            match chars.peek().and_then(|digit| digit.to_digit(16)) {
                                Some(digit) => {
                                    value = value * 16 + digit;
                                    chars.next();
                                }
                                None => break,
                            }
                        }
                        word.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                    }
                    'c' => {
                        if let Some(next) = chars.next() {
                            word.push(char::from(next.to_ascii_uppercase() as u8 & 0x1f));
                        }
                    }
                    '0'..='7' => {
                        let mut value = escape.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match chars.peek().and_then(|digit| digit.to_digit(8)) {
                                Some(digit) => {
                                    value = value * 8 + digit;
                                    chars.next();
                                }
                                None => break,
                            }
                        }
                        word.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                    }
                    other => word.push(other),
                }
            }
            other => word.push(other),
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `~/…` for paths under `$HOME`, for compact details.
fn display_path(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Legacy (xterm) bytes a key press produces, the encoding zsh and Fish
/// bindings are written against. `None` when the combination has no legacy
/// encoding (e.g. `super-x`).
fn legacy_key_bytes(ctrl: bool, alt: bool, shift: bool, base: &str) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |final_byte: char| {
        Some(if modifier == 1 {
            format!("\x1b[{final_byte}").into_bytes()
        } else {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        })
    };
    let tilde = |number: u8| {
        Some(if modifier == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        })
    };
    let ss3 = |final_byte: char| {
        Some(if modifier == 1 {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        })
    };
    match base {
        "up" => return csi('A'),
        "down" => return csi('B'),
        "right" => return csi('C'),
        "left" => return csi('D'),
        "home" => return csi('H'),
        "end" => return csi('F'),
        "insert" => return tilde(2),
        "delete" => return tilde(3),
        "pageup" => return tilde(5),
        "pagedown" => return tilde(6),
        "f1" => return ss3('P'),
        "f2" => return ss3('Q'),
        "f3" => return ss3('R'),
        "f4" => return ss3('S'),
        _ => {}
    }
    if let Some(number) = base.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        let code = match number {
            5 => 15,
            6..=10 => number + 11,
            11 | 12 => number + 12,
            _ => return None,
        };
        return tilde(code);
    }
    let alt_prefixed = |bytes: Vec<u8>| {
        if alt {
            let mut prefixed = vec![0x1b];
            prefixed.extend(bytes);
            prefixed
        } else {
            bytes
        }
    };
    let named = match base {
        "enter" | "return" => Some(b'\r'),
        "tab" => Some(b'\t'),
        "escape" | "esc" => Some(0x1b),
        "backspace" => Some(if ctrl { 0x08 } else { 0x7f }),
        "space" => Some(if ctrl { 0x00 } else { b' ' }),
        _ => None,
    };
    if let Some(byte) = named {
        if base == "tab" && shift && !ctrl {
            return Some(alt_prefixed(b"\x1b[Z".to_vec()));
        }
        if shift || (ctrl && !matches!(base, "backspace" | "space")) {
            return None;
        }
        return Some(alt_prefixed(vec![byte]));
    }
    let mut characters = base.chars();
    let character = characters.next()?;
    if characters.next().is_some() || !character.is_ascii() {
        return None;
    }
    if ctrl {
        let byte = match character.to_ascii_lowercase() {
            letter @ 'a'..='z' => letter as u8 & 0x1f,
            '@' | ' ' | '2' => 0x00,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '/' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(alt_prefixed(vec![byte]));
    }
    let character = if shift {
        character.to_ascii_uppercase()
    } else {
        character
    };
    Some(alt_prefixed(vec![character as u8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|argument| (*argument).to_owned()).collect()
    }

    #[test]
    fn interactive_shells_are_told_apart_from_commands_and_scripts() {
        assert!(shell_reads_terminal(&args(&[])));
        assert!(shell_reads_terminal(&args(&["-i"])));
        assert!(shell_reads_terminal(&args(&["-f"])));
        assert!(shell_reads_terminal(&args(&["--rcfile", "/tmp/rc", "-i"])));
        assert!(shell_reads_terminal(&args(&["-C", "set x 1"])));
        assert!(!shell_reads_terminal(&args(&["-c", "whykey ctrl+t"])));
        assert!(!shell_reads_terminal(&args(&["-ic", "whykey ctrl+t"])));
        assert!(!shell_reads_terminal(&args(&["--command=whykey"])));
        assert!(!shell_reads_terminal(&args(&["./script.sh"])));
        assert!(!shell_reads_terminal(&args(&["-e", "script.zsh"])));
    }

    #[test]
    fn encodes_legacy_key_bytes() {
        assert_eq!(legacy_key_bytes(true, false, false, "t"), Some(vec![0x14]));
        assert_eq!(
            legacy_key_bytes(false, true, false, "f"),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            legacy_key_bytes(true, false, false, "left"),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            legacy_key_bytes(false, false, false, "delete"),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            legacy_key_bytes(false, false, true, "tab"),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(legacy_key_bytes(true, false, false, "enter"), None);
    }
}
