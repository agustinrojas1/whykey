//! Zsh line editor (ZLE).
//!
//! Terminal bytes are resolved the way ZLE's key reader does: it keeps
//! reading while the bytes read so far prefix a longer binding, runs the
//! longest bound prefix, and re-reads the rest. With nothing bound it
//! discards what it read (`undefined-key`), which is why an unbound
//! `^[[1;5D` (Ctrl+Left) leaves `;5D` on the command line.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Origin, Verdict, Word, command_words, display_path, finish, home_dir, posix_commands};
use crate::layers::{LayerResult, Outcome, TerminalInput, format_bytes};

const LAYER: &str = "Zsh / ZLE";
const INIT_HINT: &str = "plugins and sourced files in ~/.zshrc may rebind keys — load `eval \"$(whykey shell-init zsh)\"` for the live keymap";
/// Prints the keymap the prompt starts in and its defaults without reading
/// any startup file.
const DEFAULTS_SCRIPT: &str =
    "bindkey -lL main; bindkey -LM emacs; bindkey -LM viins; bindkey -LM vicmd";
const MAX_CONTINUATIONS: usize = 4;

pub(super) fn inspect(inputs: &[TerminalInput], exe: Option<&Path>) -> LayerResult {
    let keymaps = match env::var_os("WHYKEY_ZLE_BINDINGS") {
        Some(dump) => Keymaps::live(&dump.to_string_lossy()),
        None => Keymaps::from_startup_files(exe),
    };
    finish(
        LAYER,
        &keymaps.origin,
        INIT_HINT,
        evaluate(&keymaps, inputs),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Widget(String),
    /// `bindkey -s`: the string is fed back as input.
    Macro(String),
}

impl Action {
    fn label(&self) -> String {
        match self {
            Self::Widget(widget) => widget.clone(),
            Self::Macro(text) => format!("macro {:?} (bindkey -s)", text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Keys {
    Exact(Vec<u8>),
    /// `bindkey -R`: every single byte in the inclusive range.
    Range(u8, u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// Keymap name, with aliases (`main`) resolved when the binding was made.
    map: String,
    keys: Keys,
    /// `None` removes the binding (`bindkey -r`, `undefined-key`).
    action: Option<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Keymaps {
    /// The keymap `main` links to: the one every prompt starts in.
    main: String,
    entries: Vec<Entry>,
    origin: Origin,
    source: String,
    /// The built-in defaults could not be read, so an absent binding is
    /// unknown rather than unbound.
    complete: bool,
}

impl Keymaps {
    fn new(origin: Origin, source: String) -> Self {
        Self {
            main: "emacs".into(),
            entries: Vec::new(),
            origin,
            source,
            complete: true,
        }
    }

    /// `bindkey -lL main; bindkey -L` from shell-init: the `main` alias and
    /// every binding of the keymap it links to.
    fn live(dump: &str) -> Self {
        let mut keymaps = Self::new(
            Origin::Live,
            "live ZLE keymap from shell-init (`bindkey -L`)".into(),
        );
        keymaps.apply_script(dump);
        keymaps
    }

    /// The installed zsh's defaults (`zsh -f`, which reads no startup file
    /// but still picks `viins` when `$VISUAL`/`$EDITOR` mention vi) with the
    /// `bindkey` commands of the startup files applied in zsh's order.
    fn from_startup_files(exe: Option<&Path>) -> Self {
        let program = exe.map_or_else(|| PathBuf::from("zsh"), Path::to_path_buf);
        let mut defaults = Command::new(&program);
        defaults
            .args(["-f", "-c", DEFAULTS_SCRIPT])
            .env_remove("WHYKEY_ZLE_BINDINGS");
        let defaults = crate::command::output(&mut defaults)
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .filter(|text| !text.trim().is_empty());
        let read: Vec<(PathBuf, String)> = startup_files()
            .into_iter()
            .filter_map(|path| fs::read_to_string(&path).ok().map(|text| (path, text)))
            .collect();
        let files: Vec<String> = read
            .iter()
            .filter(|(_, text)| text.contains("bindkey"))
            .map(|(path, _)| display_path(path))
            .collect();
        let mut source = match defaults {
            Some(_) => format!("defaults of {} (`zsh -f`)", program.display()),
            None => "zsh defaults unavailable".to_owned(),
        };
        if !files.is_empty() {
            source.push_str(&format!(" plus bindkey lines of {}", files.join(", ")));
        }
        source.push_str(" (static reading, not the running shell)");
        let origin = Origin::reconstructed(!files.is_empty(), defaults.is_some());
        let mut keymaps = Self::new(origin, source);
        match defaults {
            Some(text) => keymaps.apply_script(&text),
            None => keymaps.complete = false,
        }
        for (_, text) in &read {
            keymaps.apply_script(text);
        }
        keymaps
    }

    fn apply_script(&mut self, script: &str) {
        for words in posix_commands(script) {
            if let [command, arguments @ ..] = command_words(&words) {
                if command.text == "bindkey" {
                    self.apply_bindkey(arguments);
                }
            }
        }
    }

    fn resolve(&self, map: &str) -> String {
        if map == "main" {
            self.main.clone()
        } else {
            map.to_owned()
        }
    }

    /// Apply one `bindkey` command (arguments after the command name).
    fn apply_bindkey(&mut self, words: &[Word]) {
        let mut map = None;
        let (mut range, mut string, mut remove) = (false, false, false);
        let mut index = 0;
        while let Some(word) = words.get(index) {
            if word.expands {
                return;
            }
            let text = word.text.as_str();
            if text == "--" {
                index += 1;
                break;
            }
            let Some(flags) = text.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
                break;
            };
            for flag in flags.chars() {
                match flag {
                    'e' => {
                        self.main = "emacs".into();
                        map = Some("emacs".to_owned());
                    }
                    'v' => {
                        self.main = "viins".into();
                        map = Some("viins".to_owned());
                    }
                    'a' => map = Some("vicmd".to_owned()),
                    'M' => {
                        index += 1;
                        let Some(name) = words.get(index) else {
                            return;
                        };
                        map = Some(self.resolve(&name.text));
                    }
                    'R' => range = true,
                    's' => string = true,
                    'r' => remove = true,
                    'A' => {
                        // bindkey -A OLD NEW: NEW becomes an alias of OLD.
                        if let (Some(old), Some(new)) = (words.get(index + 1), words.get(index + 2))
                        {
                            if new.text == "main" {
                                self.main = self.resolve(&old.text);
                            }
                        }
                        return;
                    }
                    // Listing, keymap management and other queries bind nothing.
                    _ => return,
                }
            }
            index += 1;
        }
        let map = map.unwrap_or_else(|| self.main.clone());
        let operands = &words[index.min(words.len())..];
        if operands.iter().any(|word| word.expands) {
            return;
        }
        if remove {
            for word in operands {
                self.entries.push(Entry {
                    map: map.clone(),
                    keys: Keys::Exact(decode_keys(&word.text)),
                    action: None,
                });
            }
            return;
        }
        for pair in operands.chunks(2) {
            let [keys, action] = pair else {
                break;
            };
            let keys = if range {
                match decode_keys(&keys.text).as_slice() {
                    [first, b'-', last] => Keys::Range(*first, *last),
                    _ => continue,
                }
            } else {
                Keys::Exact(decode_keys(&keys.text))
            };
            let action = if string {
                Some(Action::Macro(action.text.clone()))
            } else if action.text == "undefined-key" {
                None
            } else {
                Some(Action::Widget(action.text.clone()))
            };
            self.entries.push(Entry {
                map: map.clone(),
                keys,
                action,
            });
        }
    }

    /// Effective bindings of one keymap, later commands overriding earlier
    /// ones.
    fn table(&self, map: &str) -> Table<'_> {
        let mut table = BTreeMap::new();
        for entry in self.entries.iter().filter(|entry| entry.map == map) {
            let sequences: Vec<Vec<u8>> = match &entry.keys {
                Keys::Exact(bytes) if bytes.is_empty() => continue,
                Keys::Exact(bytes) => vec![bytes.clone()],
                Keys::Range(first, last) => (*first..=*last).map(|byte| vec![byte]).collect(),
            };
            for sequence in sequences {
                match &entry.action {
                    Some(action) => {
                        table.insert(sequence, action);
                    }
                    None => {
                        table.remove(&sequence);
                    }
                }
            }
        }
        table
    }
}

/// One step of ZLE's key reader over the input bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step<'a> {
    /// Bytes `start..end` ran `action`.
    Run {
        start: usize,
        end: usize,
        action: &'a Action,
    },
    /// Bytes `start..end` matched nothing: ZLE beeps and drops them.
    Undefined { start: usize, end: usize },
    /// Input ended while bytes from `start` still prefix longer bindings; ZLE
    /// waits for more (only `KEYTIMEOUT` when `bound` is set).
    Pending {
        start: usize,
        bound: Option<&'a Action>,
    },
    /// `vi-cmd-mode` ran and the vicmd keymap is not known: bytes from
    /// `start` are read there.
    Vicmd { start: usize },
}

type Table<'a> = BTreeMap<Vec<u8>, &'a Action>;

/// Read `bytes` as ZLE does, starting in `main`; `vi-cmd-mode` switches the
/// rest of the input to `vicmd`.
fn read_keys<'a>(main: &Table<'a>, vicmd: &Table<'a>, bytes: &[u8]) -> Vec<Step<'a>> {
    let mut steps = Vec::new();
    let mut table = main;
    let mut start = 0;
    while start < bytes.len() {
        let mut bound: Option<(usize, &'a Action)> = None;
        let mut end = start;
        loop {
            end += 1;
            let sequence = &bytes[start..end];
            if let Some(action) = table.get(sequence) {
                bound = Some((end, *action));
            }
            if continuations(table, sequence).is_empty() {
                break;
            }
            if end == bytes.len() {
                steps.push(Step::Pending {
                    start,
                    bound: bound
                        .filter(|(bound_end, _)| *bound_end == end)
                        .map(|(_, action)| action),
                });
                return steps;
            }
        }
        match bound {
            Some((bound_end, action)) => {
                steps.push(Step::Run {
                    start,
                    end: bound_end,
                    action,
                });
                start = bound_end;
                if matches!(action, Action::Widget(widget) if widget == "vi-cmd-mode")
                    && !std::ptr::eq(table, vicmd)
                {
                    if vicmd.is_empty() {
                        if start < bytes.len() {
                            steps.push(Step::Vicmd { start });
                        }
                        return steps;
                    }
                    table = vicmd;
                }
            }
            None => {
                steps.push(Step::Undefined { start, end });
                start = end;
            }
        }
    }
    steps
}

fn continuations<'t, 'a>(
    table: &'t BTreeMap<Vec<u8>, &'a Action>,
    prefix: &[u8],
) -> Vec<(&'t Vec<u8>, &'t &'a Action)> {
    table
        .range(prefix.to_vec()..)
        .skip_while(|(sequence, _)| sequence.as_slice() == prefix)
        .take_while(|(sequence, _)| sequence.starts_with(prefix))
        .collect()
}

fn evaluate(keymaps: &Keymaps, inputs: &[TerminalInput]) -> Verdict {
    let Some(last) = inputs.last() else {
        return Verdict {
            outcome: Outcome::Unknown,
            summary: "no terminal input to evaluate".into(),
            action: None,
            details: vec![],
        };
    };
    let bytes: Vec<u8> = inputs
        .iter()
        .flat_map(|input| input.bytes.iter().copied())
        .collect();
    let last_start = bytes.len() - last.bytes.len();
    let table = keymaps.table(&keymaps.main);
    let vicmd = keymaps.table("vicmd");
    let steps = read_keys(&table, &vicmd, &bytes);
    // Steps that consume any byte of the last key press.
    let relevant: Vec<&Step> = steps
        .iter()
        .filter(|step| match step {
            Step::Run { end, .. } | Step::Undefined { end, .. } => *end > last_start,
            Step::Pending { .. } | Step::Vicmd { .. } => true,
        })
        .collect();

    let mut details = vec![
        format!("sequence: {}", format_bytes(&bytes)),
        format!("input: {} ({})", last.display_bytes(), last.description),
        format!("keymap: main → {}", keymaps.main),
    ];
    if keymaps.main == "viins" {
        details.push(
            "vi mode: each prompt starts in viins; after Esc keys are read from vicmd".into(),
        );
    }
    details.push(format!("binding source: {}", keymaps.source));

    let notation = |start: usize, end: usize| zle_notation(&bytes[start..end]);
    let (outcome, summary, action) = match relevant.first() {
        None => (Outcome::Pass, "no terminal input to evaluate".to_owned(), None),
        Some(Step::Pending { start, bound }) => {
            let pending = &bytes[*start..];
            let following = continuations(&table, pending);
            details.push(format!(
                "continues with {}",
                format_continuations(&following)
            ));
            match bound {
                Some(action) => {
                    details.push(format!(
                        "{} is also a prefix; ZLE waits KEYTIMEOUT (default 0.4 s) for another key before running it",
                        zle_notation(pending)
                    ));
                    (
                        Outcome::Consumed,
                        format!("bound to {}", action.label()),
                        Some(action.label()),
                    )
                }
                None => (
                    Outcome::Consumed,
                    format!(
                        "prefix key {}; ZLE waits for the next key",
                        zle_notation(pending)
                    ),
                    Some(format!(
                        "a prefix key ({} continuations)",
                        following.len()
                    )),
                ),
            }
        }
        Some(Step::Run { start, end, action }) => {
            if *start < last_start {
                details.push(format!(
                    "completes the sequence {} begun by earlier keys",
                    notation(*start, *end)
                ));
            }
            (
                Outcome::Consumed,
                format!("bound to {}", action.label()),
                Some(action.label()),
            )
        }
        Some(Step::Undefined { start, end }) if *start < last_start => {
            details.push(format!(
                "{} prefixes other bindings, but {} is not bound",
                notation(*start, last_start),
                notation(*start, *end)
            ));
            (
                Outcome::Consumed,
                format!(
                    "{} is not bound after prefix {}; ZLE beeps and discards the sequence",
                    notation(*start, *end),
                    notation(*start, last_start)
                ),
                None,
            )
        }
        Some(Step::Vicmd { .. }) => (
            Outcome::Unknown,
            "read in vi command mode (vicmd) after an earlier Esc; vicmd bindings were not captured"
                .to_owned(),
            None,
        ),
        Some(Step::Undefined { .. }) if !keymaps.complete => (
            Outcome::Unknown,
            "zsh defaults could not be read; the binding is unknown".to_owned(),
            None,
        ),
        Some(Step::Undefined { .. }) => (
            Outcome::Pass,
            format!("not bound in keymap {}", keymaps.main),
            None,
        ),
    };
    // Bytes of this key press that ZLE reads as further keys.
    let rest: Vec<&Step> = relevant.iter().skip(1).copied().collect();
    if !rest.is_empty() {
        let inserted: String = rest
            .iter()
            .filter_map(|step| match step {
                Step::Run { start, end, action } if action.label() == "self-insert" => {
                    Some(String::from_utf8_lossy(&bytes[*start..*end]).into_owned())
                }
                _ => None,
            })
            .collect();
        let read = rest
            .iter()
            .map(|step| match step {
                Step::Run { start, end, action } => {
                    format!("{} → {}", notation(*start, *end), action.label())
                }
                Step::Undefined { start, end } => {
                    format!("{} → undefined-key", notation(*start, *end))
                }
                Step::Pending { start, .. } => {
                    format!("{} → waits for more", zle_notation(&bytes[*start..]))
                }
                Step::Vicmd { start } => {
                    format!("{} → read in vicmd", zle_notation(&bytes[*start..]))
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        details.push(format!(
            "the remaining bytes are read as further keys: {read}"
        ));
        if !inserted.is_empty() {
            details.push(format!("{inserted:?} is inserted into the command line"));
        }
    }
    Verdict {
        outcome,
        summary,
        action,
        details,
    }
}

fn format_continuations(following: &[(&Vec<u8>, &&Action)]) -> String {
    let mut listed = following
        .iter()
        .take(MAX_CONTINUATIONS)
        .map(|(sequence, action)| format!("{} ({})", zle_notation(sequence), action.label()))
        .collect::<Vec<_>>()
        .join(", ");
    if following.len() > MAX_CONTINUATIONS {
        listed.push_str(&format!(
            ", and {} more",
            following.len() - MAX_CONTINUATIONS
        ));
    }
    listed
}

/// zsh's own notation (`bindkey -L`): `^X`, `^[`, `^?`, `\M-x`.
fn zle_notation(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        let mut byte = *byte;
        if byte >= 0x80 {
            text.push_str("\\M-");
            byte &= 0x7f;
        }
        match byte {
            0x7f => text.push_str("^?"),
            0x00..=0x1f => {
                text.push('^');
                text.push(char::from(byte + 0x40));
            }
            _ => text.push(char::from(byte)),
        }
    }
    text
}

/// Startup files zsh reads for an interactive shell, in order. `$ZDOTDIR`
/// relocates the per-user files.
fn startup_files() -> Vec<PathBuf> {
    let user = env::var_os("ZDOTDIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(home_dir);
    let mut files = vec![
        PathBuf::from("/etc/zshenv"),
        PathBuf::from("/etc/zsh/zshenv"),
    ];
    if let Some(user) = &user {
        files.push(user.join(".zshenv"));
    }
    files.push(PathBuf::from("/etc/zshrc"));
    files.push(PathBuf::from("/etc/zsh/zshrc"));
    if let Some(user) = &user {
        files.push(user.join(".zshrc"));
    }
    files
}

/// Decode a bindkey in-string (`^X`, `\e`, `\M-x`, `\C-x`, `\n`, octal and
/// hex escapes) into the bytes that trigger it.
fn decode_keys(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chars = text.chars().peekable();
    while chars.peek().is_some() {
        if let Some(byte) = decode_unit(&mut chars) {
            bytes.push(byte);
        } else if let Some(character) = chars.next() {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    }
    bytes
}

/// One escaped or control unit; `None` leaves a plain character unread.
fn decode_unit(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<u8> {
    let first = *chars.peek()?;
    if first == '^' {
        chars.next();
        return Some(match chars.next() {
            Some('?') => 0x7f,
            Some(next) => next.to_ascii_uppercase() as u8 & 0x1f,
            None => b'^',
        });
    }
    if first != '\\' {
        if first.is_ascii() {
            chars.next();
            return Some(first as u8);
        }
        return None;
    }
    chars.next();
    let escape = chars.next()?;
    Some(match escape {
        'e' | 'E' => 0x1b,
        'n' => b'\n',
        'r' => b'\r',
        't' => b'\t',
        'a' => 0x07,
        'b' => 0x08,
        'f' => 0x0c,
        'v' => 0x0b,
        'M' if chars.peek() == Some(&'-') => {
            chars.next();
            decode_unit(chars).unwrap_or(0) | 0x80
        }
        'C' if chars.peek() == Some(&'-') => {
            chars.next();
            match decode_unit(chars)? {
                b'?' => 0x7f,
                byte => byte.to_ascii_uppercase() & 0x1f,
            }
        }
        'x' => {
            let mut value = 0u8;
            for _ in 0..2 {
                match chars.peek().and_then(|digit| digit.to_digit(16)) {
                    Some(digit) => {
                        value = value.wrapping_mul(16).wrapping_add(digit as u8);
                        chars.next();
                    }
                    None => break,
                }
            }
            value
        }
        '0'..='7' => {
            let mut value = escape.to_digit(8).unwrap_or(0) as u8;
            for _ in 0..2 {
                match chars.peek().and_then(|digit| digit.to_digit(8)) {
                    Some(digit) => {
                        value = value.wrapping_mul(8).wrapping_add(digit as u8);
                        chars.next();
                    }
                    None => break,
                }
            }
            value
        }
        other if other.is_ascii() => other as u8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymaps(script: &str) -> Keymaps {
        let mut keymaps = Keymaps::new(Origin::Live, "test".into());
        keymaps.apply_script(script);
        keymaps
    }

    fn verdict(keymaps: &Keymaps, keys: &[&[u8]]) -> Verdict {
        let inputs: Vec<TerminalInput> = keys
            .iter()
            .map(|bytes| TerminalInput::predicted(bytes.to_vec(), "test"))
            .collect();
        evaluate(keymaps, &inputs)
    }

    const DEFAULTS: &str = r#"bindkey -A emacs main
bindkey -M emacs "^A" beginning-of-line
bindkey -M emacs "^T" transpose-chars
bindkey -M emacs "^X^B" vi-match-bracket
bindkey -M emacs "^Xu" undo
bindkey -M emacs "^[f" forward-word
bindkey -M emacs "^[[A" up-line-or-history
bindkey -R -M emacs " "-"~" self-insert
bindkey -R -M viins "^A"-"^C" self-insert
bindkey -R -M viins "^T"-"^U" self-insert
bindkey -a "^G" list-expand
"#;

    #[test]
    fn decodes_bindkey_notation() {
        assert_eq!(decode_keys("^T"), [0x14]);
        assert_eq!(decode_keys("^[[1;5D"), b"\x1b[1;5D");
        assert_eq!(decode_keys("\\e[1;5D"), b"\x1b[1;5D");
        assert_eq!(decode_keys("^?"), [0x7f]);
        assert_eq!(decode_keys("\\M-^@"), [0x80]);
        assert_eq!(decode_keys("\\C-x\\C-e"), [0x18, 0x05]);
        assert_eq!(decode_keys("^[\""), b"\x1b\"");
        assert_eq!(zle_notation(&[0x18, 0x05, 0x1b, b'f', 0x7f]), "^X^E^[f^?");
    }

    #[test]
    fn tokenizes_shell_quoting() {
        let parsed = posix_commands(
            "bindkey -M viins '^T' my-widget # note\nbindkey \"\\e[1;5D\" backward-word; bindkey $'\\e[H' beginning-of-line",
        );
        let texts: Vec<Vec<&str>> = parsed
            .iter()
            .map(|words| words.iter().map(|word| word.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                vec!["bindkey", "-M", "viins", "^T", "my-widget"],
                vec!["bindkey", "\\e[1;5D", "backward-word"],
                vec!["bindkey", "\x1b[H", "beginning-of-line"],
            ]
        );
        assert!(posix_commands("bindkey \"${terminfo[kcuu1]}\" up")[0][1].expands);
    }

    #[test]
    fn unqualified_bindings_follow_the_main_keymap_at_bind_time() {
        let keymaps = keymaps(&format!(
            "{DEFAULTS}bindkey '^T' my-widget\nbindkey -v\nbindkey '^G' vi-thing"
        ));
        assert_eq!(keymaps.main, "viins");
        let table = keymaps.table("viins");
        // ^T was bound in emacs before `bindkey -v`; viins keeps self-insert.
        assert_eq!(
            table.get(&vec![0x14]).map(|action| action.label()),
            Some("self-insert".into())
        );
        assert_eq!(
            table.get(&vec![0x07]).map(|action| action.label()),
            Some("vi-thing".into())
        );
    }

    #[test]
    fn live_dump_of_a_user_widget_is_definitive() {
        let keymaps = keymaps(&format!("{DEFAULTS}bindkey \"^T\" my-widget\n"));
        let result = verdict(&keymaps, &[&[0x14]]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.summary, "bound to my-widget");
    }

    #[test]
    fn unbound_escape_sequences_leave_their_tail_on_the_line() {
        let keymaps = keymaps(DEFAULTS);
        let result = verdict(&keymaps, &[b"\x1b[1;5D"]);
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.summary, "not bound in keymap emacs");
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "\";5D\" is inserted into the command line"),
            "{:?}",
            result.details
        );
    }

    #[test]
    fn prefixes_wait_and_unbound_continuations_are_discarded() {
        let keymaps = keymaps(DEFAULTS);
        let pending = verdict(&keymaps, &[&[0x18]]);
        assert_eq!(pending.summary, "prefix key ^X; ZLE waits for the next key");
        let discarded = verdict(&keymaps, &[&[0x18], &[0x05]]);
        assert_eq!(discarded.outcome, Outcome::Consumed);
        assert_eq!(
            discarded.summary,
            "^X^E is not bound after prefix ^X; ZLE beeps and discards the sequence"
        );
        let bound = verdict(&keymaps, &[&[0x18], b"u"]);
        assert_eq!(bound.summary, "bound to undo");
    }

    #[test]
    fn removed_and_undefined_bindings_are_unbound() {
        let keymaps = keymaps(&format!(
            "{DEFAULTS}bindkey -r '^T'\nbindkey '^A' undefined-key\nbindkey -s '^G' 'ls\\n'"
        ));
        assert_eq!(verdict(&keymaps, &[&[0x14]]).outcome, Outcome::Pass);
        assert_eq!(verdict(&keymaps, &[&[0x01]]).outcome, Outcome::Pass);
        assert_eq!(
            verdict(&keymaps, &[&[0x07]]).summary,
            "bound to macro \"ls\\\\n\" (bindkey -s)"
        );
    }
}
