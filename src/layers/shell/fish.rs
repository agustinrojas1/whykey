//! Fish line editor.
//!
//! Fish decodes each terminal key press into one key (`ctrl-t`, `alt-f`,
//! `ctrl-left`) before matching, so escape sequences never split. Bindings
//! belong to a bind mode (`default`, or `insert`/`default`/`visual` with vi
//! key bindings); within the active mode a user binding shadows the preset
//! binding of the same key. A key that matches nothing in the mode falls
//! through to the generic binding (`bind ''`), which inserts printable text.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Origin, Splitter, Verdict, Word, display_path, finish, home_dir, legacy_key_bytes};
use crate::layers::{LayerResult, Outcome, TerminalInput, format_bytes};

const LAYER: &str = "Fish";
const INIT_HINT: &str = "functions, plugins and conditionals in the fish config may rebind keys — load `whykey shell-init fish | source` for the live bindings";
const MAX_CONTINUATIONS: usize = 4;
const PRESETS: &[&str] = &[
    "fish_default_key_bindings",
    "fish_vi_key_bindings",
    "fish_hybrid_key_bindings",
];

pub(super) fn inspect(inputs: &[TerminalInput], exe: Option<&Path>) -> LayerResult {
    let bindings = match crate::recording::env_var_os("WHYKEY_FISH_BINDINGS") {
        Some(dump) => Bindings::live(
            &dump.to_string_lossy(),
            crate::recording::env_var("WHYKEY_FISH_MODE")
                .ok()
                .filter(|mode| !mode.is_empty()),
        ),
        None => Bindings::from_config(exe),
    };
    finish(
        LAYER,
        &bindings.origin,
        INIT_HINT,
        evaluate(&bindings, inputs),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: String,
    keys: Vec<u8>,
    /// The bound command line, words joined by spaces.
    command: String,
    preset: bool,
    /// `-m`/`--sets-mode`: the mode the binding switches to.
    sets_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Bindings {
    /// The bind mode at the prompt.
    mode: String,
    mode_known: bool,
    entries: Vec<Entry>,
    origin: Origin,
    source: String,
    /// Preset bindings could not be read, so an absent binding is unknown.
    complete: bool,
}

impl Bindings {
    /// `bind --all` output and `$fish_bind_mode` captured by shell-init.
    fn live(dump: &str, mode: Option<String>) -> Self {
        let mut bindings = Self {
            mode: mode.clone().unwrap_or_else(|| "default".into()),
            mode_known: mode.is_some(),
            entries: Vec::new(),
            origin: Origin::Live,
            source: "live fish bindings from shell-init (`bind --all`)".into(),
            complete: true,
        };
        bindings.apply_script(dump);
        bindings
    }

    /// The installed fish's preset bindings for the configured key binding
    /// set (`fish --no-config`), with the `bind` commands of the config
    /// files applied in fish's order.
    fn from_config(exe: Option<&Path>) -> Self {
        let files = config_files();
        let read: Vec<(PathBuf, String)> = files
            .into_iter()
            .filter_map(|path| {
                crate::recording::read_to_string(&path)
                    .ok()
                    .map(|text| (path, text))
            })
            .collect();
        let universal = universal_key_bindings();
        // The preset is the user's choice when a universal variable or a
        // config file selects it; otherwise it is fish's default.
        let mut preset_chosen = universal.is_some();
        let mut preset = universal.unwrap_or_else(|| PRESETS[0].to_owned());
        for (_, text) in &read {
            if let Some(selected) = selected_preset(text) {
                preset = selected;
                preset_chosen = true;
            }
        }
        let program = exe.map_or_else(|| PathBuf::from("fish"), Path::to_path_buf);
        let mut command = Command::new(&program);
        command
            .args(["--no-config", "-c", &format!("{preset}; bind --all")])
            .env_remove("WHYKEY_FISH_BINDINGS");
        let presets = crate::command::output(&mut command)
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .filter(|text| !text.trim().is_empty());
        let mut source = match presets {
            Some(_) => format!(
                "{preset} presets of {} (`fish --no-config`)",
                program.display()
            ),
            None => "fish preset bindings unavailable".to_owned(),
        };
        let with_binds: Vec<String> = read
            .iter()
            .filter(|(_, text)| text.contains("bind"))
            .map(|(path, _)| display_path(path))
            .collect();
        if !with_binds.is_empty() {
            source.push_str(&format!(" plus bind lines of {}", with_binds.join(", ")));
        }
        source.push_str(" (static reading, not the running shell)");
        let vi = preset != PRESETS[0];
        let mut bindings = Self {
            // fish_vi_key_bindings and the hybrid set start every prompt in
            // insert mode.
            mode: if vi { "insert" } else { "default" }.into(),
            mode_known: true,
            entries: Vec::new(),
            origin: Origin::reconstructed(
                preset_chosen || !with_binds.is_empty(),
                presets.is_some(),
            ),
            source,
            complete: presets.is_some(),
        };
        if let Some(text) = presets {
            bindings.apply_script(&text);
        }
        for (_, text) in &read {
            bindings.apply_script(text);
        }
        bindings
    }

    fn apply_script(&mut self, script: &str) {
        for words in commands(script) {
            let start = words
                .iter()
                .position(|word| word.text != "builtin")
                .unwrap_or(words.len());
            if words.get(start).is_some_and(|word| word.text == "bind") {
                self.apply_bind(&words[start + 1..]);
            }
        }
    }

    /// Apply one `bind` command (arguments after the command name).
    fn apply_bind(&mut self, words: &[Word]) {
        let mut mode = "default".to_owned();
        let mut sets_mode = None;
        let (mut preset, mut erase, mut terminfo) = (false, false, false);
        let mut index = 0;
        while let Some(word) = words.get(index) {
            match word.text.as_str() {
                "--preset" => preset = true,
                "--user" => preset = false,
                "-e" | "--erase" => erase = true,
                "-k" | "--key" => terminfo = true,
                "-s" | "--silent" => {}
                "-M" | "--mode" => {
                    index += 1;
                    let Some(value) = words.get(index) else {
                        return;
                    };
                    mode = value.text.clone();
                }
                "-m" | "--sets-mode" => {
                    index += 1;
                    sets_mode = words.get(index).map(|value| value.text.clone());
                }
                "--" => {
                    index += 1;
                    break;
                }
                text if text.starts_with("--mode=") => mode = text["--mode=".len()..].to_owned(),
                text if text.starts_with("--sets-mode=") => {
                    sets_mode = Some(text["--sets-mode=".len()..].to_owned());
                }
                // Listing (`-a`, `-L`, `-f`, `-K`) and help bind nothing.
                text if text.starts_with('-') && text.len() > 1 => return,
                _ => break,
            }
            index += 1;
        }
        let Some(key) = words.get(index) else {
            return;
        };
        if key.expands {
            return;
        }
        let Some(keys) = key_bytes(&key.text, terminfo) else {
            return;
        };
        if erase {
            self.entries.retain(|entry| {
                !(entry.mode == mode && entry.keys == keys && entry.preset == preset)
            });
            return;
        }
        let command = words[index + 1..]
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if command.is_empty() {
            return;
        }
        self.entries.push(Entry {
            mode,
            keys,
            command,
            preset,
            sets_mode,
        });
    }

    /// Effective bindings of the active mode: presets first, then user
    /// bindings, which shadow them; later definitions replace earlier ones.
    fn table(&self) -> BTreeMap<Vec<u8>, &Entry> {
        let mut table = BTreeMap::new();
        for preset in [true, false] {
            for entry in self
                .entries
                .iter()
                .filter(|entry| entry.preset == preset && entry.mode == self.mode)
                .filter(|entry| !entry.keys.is_empty())
            {
                table.insert(entry.keys.clone(), entry);
            }
        }
        table
    }
}

enum Resolution<'a> {
    Bound { entry: &'a Entry, prefix_of: usize },
    Prefix { count: usize },
    Unbound,
}

fn resolve<'a>(table: &BTreeMap<Vec<u8>, &'a Entry>, keys: &[u8]) -> Resolution<'a> {
    let longer = continuations(table, keys).len();
    match table.get(keys) {
        Some(entry) => Resolution::Bound {
            entry,
            prefix_of: longer,
        },
        None if longer > 0 => Resolution::Prefix { count: longer },
        None => Resolution::Unbound,
    }
}

fn continuations<'t, 'a>(
    table: &'t BTreeMap<Vec<u8>, &'a Entry>,
    prefix: &[u8],
) -> Vec<&'t &'a Entry> {
    table
        .range(prefix.to_vec()..)
        .skip_while(|(keys, _)| keys.as_slice() == prefix)
        .take_while(|(keys, _)| keys.starts_with(prefix))
        .map(|(_, entry)| entry)
        .collect()
}

fn evaluate(bindings: &Bindings, inputs: &[TerminalInput]) -> Verdict {
    let Some((last, earlier)) = inputs.split_last() else {
        return Verdict {
            outcome: Outcome::Unknown,
            summary: "no terminal input to evaluate".into(),
            action: None,
            details: vec![],
        };
    };
    let table = bindings.table();
    // Keys still waiting for a longer binding. A key that completes nothing
    // is dispatched on its own and the next key starts afresh.
    let mut pending: Vec<u8> = Vec::new();
    for input in earlier {
        let key = normalize_input(&input.bytes);
        let mut candidate = pending.clone();
        candidate.extend_from_slice(&key);
        pending = match resolve(&table, &candidate) {
            Resolution::Prefix { .. } | Resolution::Bound { prefix_of: 1.., .. } => candidate,
            Resolution::Bound { .. } => Vec::new(),
            Resolution::Unbound => match resolve(&table, &key) {
                Resolution::Prefix { .. } | Resolution::Bound { prefix_of: 1.., .. } => key,
                _ => Vec::new(),
            },
        };
    }
    let key = normalize_input(&last.bytes);
    let mut keys = pending.clone();
    keys.extend_from_slice(&key);
    let mut resolution = resolve(&table, &keys);
    let mut details = vec![
        format!("sequence: {}", format_bytes(&keys)),
        format!("input: {} ({})", last.display_bytes(), last.description),
    ];
    if matches!(resolution, Resolution::Unbound) && !pending.is_empty() {
        details.push(format!(
            "{} completes no binding after {}; fish runs the earlier keys on their own and reads this key afresh",
            fish_notation(&keys),
            fish_notation(&pending)
        ));
        keys = key;
        resolution = resolve(&table, &keys);
    } else if !pending.is_empty() {
        details.push(format!(
            "continues the sequence {} begun by earlier keys",
            fish_notation(&pending)
        ));
    }
    details.push(if bindings.mode_known {
        format!("bind mode: {}", bindings.mode)
    } else {
        "bind mode unknown; assuming default".into()
    });
    details.push(format!("binding source: {}", bindings.source));
    let (outcome, summary, action) = match resolution {
        Resolution::Bound { entry, prefix_of } => {
            details.push(format!(
                "{} binding for {}",
                if entry.preset { "preset" } else { "user" },
                fish_notation(&keys)
            ));
            if let Some(mode) = &entry.sets_mode {
                details.push(format!("switches to bind mode {mode}"));
            }
            if prefix_of > 0 {
                details.push(format!(
                    "{} is also a prefix; fish waits for another key before running it (continues with {})",
                    fish_notation(&keys),
                    format_continuations(&continuations(&table, &keys))
                ));
            }
            (
                Outcome::Consumed,
                format!("bound to {}", entry.command),
                Some(entry.command.clone()),
            )
        }
        Resolution::Prefix { count } => {
            details.push(format!(
                "continues with {}",
                format_continuations(&continuations(&table, &keys))
            ));
            (
                Outcome::Consumed,
                format!(
                    "prefix key {}; fish waits for the next key",
                    fish_notation(&keys)
                ),
                Some(format!("a prefix key ({count} continuations)")),
            )
        }
        Resolution::Unbound if !bindings.complete => (
            Outcome::Unknown,
            "fish preset bindings could not be read; the binding is unknown".to_owned(),
            None,
        ),
        Resolution::Unbound => (
            Outcome::Pass,
            format!(
                "not bound in bind mode {}: fish reads the key and drops it",
                bindings.mode
            ),
            None,
        ),
    };
    Verdict {
        outcome,
        summary,
        action,
        details,
    }
}

fn format_continuations(following: &[&&Entry]) -> String {
    let mut listed = following
        .iter()
        .take(MAX_CONTINUATIONS)
        .map(|entry| format!("{} ({})", fish_notation(&entry.keys), entry.command))
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

fn fish_notation(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        match byte {
            0x1b => text.push_str("\\e"),
            0x7f => text.push_str("\\x7f"),
            0x00..=0x1f => {
                text.push_str("\\c");
                text.push(char::from(byte + 0x60));
            }
            0x20..=0x7e => text.push(char::from(*byte)),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    text
}

/// Terminal input in the legacy encoding fish bindings are compared in:
/// CSI-u (`\e[116;5u`, kitty keyboard protocol, which fish 4 enables) and
/// SS3 cursor keys (`\eOA`, application cursor mode) become their xterm
/// forms.
fn normalize_input(bytes: &[u8]) -> Vec<u8> {
    if let [
        0x1b,
        b'O',
        final_byte @ (b'A' | b'B' | b'C' | b'D' | b'H' | b'F'),
    ] = bytes
    {
        return vec![0x1b, b'[', *final_byte];
    }
    if let Some(body) = bytes
        .strip_prefix(b"\x1b[")
        .and_then(|rest| rest.strip_suffix(b"u"))
    {
        let text = String::from_utf8_lossy(body);
        let mut fields = text.split(';');
        let code = fields
            .next()
            .and_then(|code| code.split(':').next())
            .and_then(|code| code.parse::<u32>().ok());
        let modifiers = fields
            .next()
            .and_then(|mods| mods.split(':').next())
            .and_then(|mods| mods.parse::<u32>().ok())
            .unwrap_or(1)
            .saturating_sub(1);
        let base = match code {
            Some(13) => Some("enter".to_owned()),
            Some(9) => Some("tab".to_owned()),
            Some(27) => Some("escape".to_owned()),
            Some(127) => Some("backspace".to_owned()),
            Some(32) => Some("space".to_owned()),
            Some(code) => char::from_u32(code)
                .filter(char::is_ascii_graphic)
                .map(String::from),
            None => None,
        };
        if let Some(base) = base {
            if let Some(legacy) = legacy_key_bytes(
                modifiers & 4 != 0,
                modifiers & 2 != 0,
                modifiers & 1 != 0,
                &base,
            ) {
                return legacy;
            }
        }
    }
    bytes.to_vec()
}

/// Bytes of a `bind` key argument: fish 4 key names (`ctrl-x,ctrl-e`,
/// `alt-left`), fish 3 terminfo names with `-k`, or a raw sequence whose
/// escapes the tokenizer already decoded (`\ct` → 0x14).
fn key_bytes(key: &str, terminfo: bool) -> Option<Vec<u8>> {
    if terminfo {
        let name = match key {
            "dc" => "delete",
            "ic" => "insert",
            "ppage" => "pageup",
            "npage" => "pagedown",
            "btab" => return Some(b"\x1b[Z".to_vec()),
            other => other,
        };
        return legacy_key_bytes(false, false, false, name);
    }
    if key.is_empty() {
        return Some(Vec::new());
    }
    if let Some(bytes) = named_keys(key) {
        return Some(bytes);
    }
    Some(key.as_bytes().to_vec())
}

/// `ctrl-x,ctrl-e` → `[0x18, 0x05]`; `None` when any part is not a key name.
fn named_keys(key: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in key.split(',') {
        bytes.extend(named_key(part)?);
    }
    Some(bytes)
}

fn named_key(name: &str) -> Option<Vec<u8>> {
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut rest = name;
    loop {
        if let Some(after) = rest.strip_prefix("ctrl-") {
            ctrl = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("alt-") {
            alt = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("shift-") {
            shift = true;
            rest = after;
        } else if rest.starts_with("super-")
            || rest.starts_with("hyper-")
            || rest.starts_with("meta-")
        {
            return None;
        } else {
            break;
        }
    }
    let modified = ctrl || alt || shift;
    let base = match rest {
        "minus" => "-",
        "comma" => ",",
        "ret" => "enter",
        "esc" => "escape",
        "pgup" => "pageup",
        "pgdown" => "pagedown",
        other => other,
    };
    if !modified {
        // A bare single character is a literal key, not a name; unmodified
        // names must be keys fish knows by name.
        let named =
            matches!(
                base,
                "enter"
                    | "tab"
                    | "escape"
                    | "backspace"
                    | "delete"
                    | "insert"
                    | "space"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
                    | "up"
                    | "down"
                    | "left"
                    | "right"
            ) || (base.len() > 1 && base.starts_with('f') && base[1..].parse::<u8>().is_ok());
        if !named {
            return None;
        }
    }
    legacy_key_bytes(ctrl, alt, shift, base)
}

/// `$fish_key_bindings` stored as a universal variable.
fn universal_key_bindings() -> Option<String> {
    let path = crate::util::config_base()?.join("fish/fish_variables");
    let text = crate::recording::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        line.strip_prefix("SETUVAR fish_key_bindings:")
            .or_else(|| line.strip_prefix("SETUVAR --export fish_key_bindings:"))
            .map(str::trim)
            .filter(|value| PRESETS.contains(value))
            .map(str::to_owned)
    })
}

/// The key binding set a config file selects, by calling a preset function
/// or setting `fish_key_bindings`; the last selection wins.
fn selected_preset(text: &str) -> Option<String> {
    let mut selected = None;
    for words in commands(text) {
        let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        if let Some(name) = texts.first().filter(|name| PRESETS.contains(name)) {
            selected = Some((*name).to_owned());
        }
        if texts.first() == Some(&"set") {
            if let Some(position) = texts.iter().position(|text| *text == "fish_key_bindings") {
                if let Some(value) = texts
                    .get(position + 1)
                    .filter(|value| PRESETS.contains(value))
                {
                    selected = Some((*value).to_owned());
                }
            }
        }
    }
    selected
}

/// Config files in the order fish sources them for an interactive shell:
/// `conf.d` snippets (by name), `config.fish`, then the
/// `fish_user_key_bindings` function that runs when bindings initialize.
fn config_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let user = crate::util::config_base()
        .or_else(|| home_dir().map(|home| home.join(".config")))
        .map(|base| base.join("fish"));
    let mut snippets = Vec::new();
    for dir in [
        user.as_ref().map(|dir| dir.join("conf.d")),
        Some(PathBuf::from("/etc/fish/conf.d")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(entries) = crate::recording::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "fish")
                {
                    snippets.push(path);
                }
            }
        }
    }
    // fish sources same-named snippets once, user directory first; order by
    // file name across directories.
    snippets.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    snippets.dedup_by(|a, b| a.file_name() == b.file_name());
    files.extend(snippets);
    files.push(PathBuf::from("/etc/fish/config.fish"));
    if let Some(user) = user {
        files.push(user.join("config.fish"));
        files.push(user.join("functions/fish_user_key_bindings.fish"));
    }
    files
}

/// Split fish source into commands of words with fish quoting: `'…'` (only
/// `\'` and `\\` escape), `"…"`, and backslash escapes in bare words
/// (`\ct`, `\e`, `\x1b`, `\[`). Separators: `;`, `|`, `&`, newline.
fn commands(script: &str) -> Vec<Vec<Word>> {
    let mut out = Splitter::default();
    let mut chars = script.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                out.started = true;
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '\'' => break,
                        '\\' if matches!(chars.peek(), Some('\'' | '\\')) => {
                            if let Some(next) = chars.next() {
                                out.push(next);
                            }
                        }
                        other => out.push(other),
                    }
                }
            }
            '"' => {
                out.started = true;
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(next @ ('"' | '$' | '\\')) => out.push(next),
                            Some('\n') | None => {}
                            Some(next) => {
                                out.push('\\');
                                out.push(next);
                            }
                        },
                        '$' => {
                            out.expands = true;
                            out.push('$');
                        }
                        other => out.push(other),
                    }
                }
            }
            '\\' => {
                out.started = true;
                bare_escape(&mut chars, &mut out.word);
            }
            '#' if !out.started => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
                out.end_command();
            }
            ';' | '|' | '&' | '\n' => out.end_command(),
            ' ' | '\t' => out.end_word(),
            other => {
                if matches!(other, '$' | '(' | '*' | '~' | '{') {
                    out.expands = true;
                }
                out.push(other);
            }
        }
    }
    out.finish()
}

fn bare_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, word: &mut String) {
    let Some(escape) = chars.next() else {
        return;
    };
    let hex = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>, digits: usize| {
        let mut value = 0u32;
        for _ in 0..digits {
            match chars.peek().and_then(|digit| digit.to_digit(16)) {
                Some(digit) => {
                    value = value * 16 + digit;
                    chars.next();
                }
                None => break,
            }
        }
        value
    };
    match escape {
        'a' => word.push('\x07'),
        'b' => word.push('\x08'),
        'e' => word.push('\x1b'),
        'f' => word.push('\x0c'),
        'n' => word.push('\n'),
        'r' => word.push('\r'),
        't' => word.push('\t'),
        'v' => word.push('\x0b'),
        'c' => {
            if let Some(next) = chars.next() {
                word.push(match next {
                    '?' => '\x7f',
                    other => char::from(other.to_ascii_uppercase() as u8 & 0x1f),
                });
            }
        }
        'x' | 'X' => word.push(char::from_u32(hex(chars, 2)).unwrap_or('\u{fffd}')),
        'u' => word.push(char::from_u32(hex(chars, 4)).unwrap_or('\u{fffd}')),
        'U' => word.push(char::from_u32(hex(chars, 8)).unwrap_or('\u{fffd}')),
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
        '\n' => {}
        other => word.push(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(script: &str, mode: &str) -> Bindings {
        Bindings::live(script, Some(mode.to_owned()))
    }

    fn verdict(bindings: &Bindings, keys: &[&[u8]]) -> Verdict {
        let inputs: Vec<TerminalInput> = keys
            .iter()
            .map(|bytes| TerminalInput::predicted(bytes.to_vec(), "test"))
            .collect();
        evaluate(bindings, &inputs)
    }

    const PRESET: &str = r#"bind --preset '' self-insert
bind --preset ctrl-t transpose-chars
bind --preset ctrl-x fish_clipboard_copy
bind --preset ctrl-e end-of-line
bind --preset ctrl-left 'if fish_in_macos_terminal; commandline -f backward-token; else commandline -f backward-word; end'
bind --preset -M insert ctrl-left backward-word
bind --preset -M insert -m default escape 'commandline -f backward-char'
"#;

    #[test]
    fn decodes_key_names_and_raw_sequences() {
        assert_eq!(key_bytes("ctrl-t", false), Some(vec![0x14]));
        assert_eq!(key_bytes("ctrl-x,ctrl-e", false), Some(vec![0x18, 0x05]));
        assert_eq!(key_bytes("alt-f", false), Some(b"\x1bf".to_vec()));
        assert_eq!(key_bytes("ctrl-left", false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_bytes("\x14", false), Some(vec![0x14]));
        assert_eq!(key_bytes("a", false), Some(b"a".to_vec()));
        assert_eq!(key_bytes("right", true), Some(b"\x1b[C".to_vec()));
        let words = &commands("bind \\ct 'commandline -i \"a b\" \\'c\\''")[0];
        assert_eq!(words[1].text, "\x14");
        assert_eq!(words[2].text, "commandline -i \"a b\" 'c'");
        assert_eq!(commands("bind \\e\\[1\\;5D bar")[0][1].text, "\x1b[1;5D");
    }

    #[test]
    fn user_bindings_shadow_presets_regardless_of_order() {
        let bindings = bindings(
            &format!("bind ctrl-t 'commandline -i hi'\n{PRESET}"),
            "default",
        );
        let result = verdict(&bindings, &[&[0x14]]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.summary, "bound to commandline -i hi");
    }

    #[test]
    fn bindings_belong_to_their_mode() {
        let user = format!("{PRESET}bind ctrl-t 'commandline -i hi'\n");
        let insert = bindings(&user, "insert");
        assert_eq!(
            verdict(&insert, &[&[0x14]]).summary,
            "not bound in bind mode insert: fish reads the key and drops it"
        );
        assert_eq!(
            verdict(&insert, &[b"\x1b[1;5D"]).summary,
            "bound to backward-word"
        );
        let result = verdict(&insert, &[&[0x1b]]);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "switches to bind mode default")
        );
    }

    #[test]
    fn earlier_keys_without_a_longer_binding_run_on_their_own() {
        let bindings = bindings(PRESET, "default");
        let result = verdict(&bindings, &[&[0x18], &[0x05]]);
        assert_eq!(result.summary, "bound to end-of-line");
        let chained = bindings_with_sequence();
        assert_eq!(
            verdict(&chained, &[&[0x18], &[0x05]]).summary,
            "bound to edit"
        );
        assert_eq!(
            verdict(&chained, &[&[0x18]]).summary,
            "bound to fish_clipboard_copy"
        );
    }

    fn bindings_with_sequence() -> Bindings {
        bindings(&format!("{PRESET}bind ctrl-x,ctrl-e edit\n"), "default")
    }

    #[test]
    fn kitty_protocol_input_matches_legacy_bindings() {
        assert_eq!(normalize_input(b"\x1b[116;5u"), vec![0x14]);
        assert_eq!(normalize_input(b"\x1b[102;3u"), b"\x1bf".to_vec());
        assert_eq!(normalize_input(b"\x1bOD"), b"\x1b[D".to_vec());
    }

    #[test]
    fn erased_bindings_are_removed() {
        let bindings = bindings(&format!("{PRESET}bind --preset -e ctrl-t\n"), "default");
        assert_eq!(verdict(&bindings, &[&[0x14]]).outcome, Outcome::Pass);
    }

    #[test]
    fn config_selects_the_vi_preset() {
        assert_eq!(
            selected_preset("if status is-interactive\n  fish_vi_key_bindings\nend").as_deref(),
            Some("fish_vi_key_bindings")
        );
        assert_eq!(
            selected_preset("set -g fish_key_bindings fish_hybrid_key_bindings").as_deref(),
            Some("fish_hybrid_key_bindings")
        );
    }
}
