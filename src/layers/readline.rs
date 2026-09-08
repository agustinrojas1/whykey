use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, TerminalInput, format_bytes};

pub struct Readline;

impl Readline {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        if !is_bash() {
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Unavailable,
                summary: "current shell is not Bash".into(),
                details: vec!["Readline mapping was not inspected".into()],
            };
        }

        let Some(bytes) = readline_key_bytes(key) else {
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Pass,
                summary: "no known Readline binding".into(),
                details: vec![],
            };
        };
        let Some((sequence, default_binding)) = readline_binding(&bytes) else {
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Pass,
                summary: "no known Readline binding".into(),
                details: vec![format!("sequence: {}", crate::layers::format_bytes(&bytes))],
            };
        };
        let configured = inputrc_binding(sequence);
        let mut details = vec![format_sequence(sequence)];
        append_mode_detail(&mut details);
        if let Some(binding) = configured {
            details.push(format!("binding source: {}", binding_source(sequence)));
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Consumed,
                summary: format!("bound to {binding}"),
                details,
            };
        }
        if readline_mode_is_emacs() {
            details.push("binding source: Readline default".into());
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Consumed,
                summary: format!("bound to {default_binding}"),
                details,
            };
        }
        details.push("Emacs defaults were not applied because the active keymap is vi".into());
        LayerResult {
            layer: "Bash / Readline",
            id: LayerId::Shell,
            outcome: Outcome::Pass,
            summary: "no known binding in the active vi keymap".into(),
            details,
        }
    }
}

impl Readline {
    pub fn inspect_input(&self, input: &TerminalInput) -> LayerResult {
        if !is_bash() {
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Unavailable,
                summary: "current shell is not Bash".into(),
                details: vec!["Readline mapping was not inspected".into()],
            };
        }

        let Some((sequence, default_binding)) = readline_binding(&input.bytes) else {
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Pass,
                summary: "no known Readline binding".into(),
                details: vec![format_input(input)],
            };
        };

        let binding = inputrc_binding(sequence)
            .or_else(|| readline_mode_is_emacs().then_some(default_binding.into()));
        let Some(binding) = binding else {
            let mut details = vec![format_input(input)];
            append_mode_detail(&mut details);
            return LayerResult {
                layer: "Bash / Readline",
                id: LayerId::Shell,
                outcome: Outcome::Pass,
                summary: "no known Readline binding in the active keymap".into(),
                details,
            };
        };
        let mut details = vec![
            format_sequence(sequence),
            format!("binding source: {}", binding_source(sequence)),
        ];
        append_mode_detail(&mut details);
        LayerResult {
            layer: "Bash / Readline",
            id: LayerId::Shell,
            outcome: Outcome::Consumed,
            summary: format!("bound to {binding}"),
            details,
        }
    }
}

fn readline_binding(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    match bytes {
        [0x01] => Some(("\x01", "beginning-of-line")),
        [0x02] => Some(("\x02", "backward-char")),
        [0x04] => Some(("\x04", "delete-char (or end-of-file)")),
        [0x05] => Some(("\x05", "end-of-line")),
        [0x06] => Some(("\x06", "forward-char")),
        [0x07] => Some(("\x07", "abort")),
        [0x08] => Some(("\x08", "backward-delete-char")),
        [0x0a] => Some(("\x0a", "accept-line")),
        [0x0b] => Some(("\x0b", "kill-line")),
        [0x0c] => Some(("\x0c", "clear-display")),
        [0x0e] => Some(("\x0e", "next-history")),
        [0x10] => Some(("\x10", "previous-history")),
        [0x12] => Some(("\x12", "reverse-search-history")),
        [0x15] => Some(("\x15", "unix-line-discard")),
        [0x17] => Some(("\x17", "unix-word-rubout")),
        [0x18] => Some(("\x18", "complete-into-braces")),
        [0x19] => Some(("\x19", "yank")),
        [0x1d] => Some(("\x1d", "list-choices")),
        [0x1f] => Some(("\x1f", "undo")),
        [0x1b, b'f'] => Some(("\x1bf", "forward-word")),
        [0x1b, b'b'] => Some(("\x1bb", "backward-word")),
        [0x1b, b'[', b'1', b';', b'5', b'D'] => Some(("\x1b[1;5D", "backward-word")),
        [0x1b, b'[', b'1', b';', b'5', b'C'] => Some(("\x1b[1;5C", "forward-word")),
        _ => None,
    }
}

fn readline_key_bytes(key: &KeyCombo) -> Option<Vec<u8>> {
    if key.modmask() != 4 {
        return None;
    }
    match key.key() {
        "LEFT" => Some(b"\x1b[1;5D".to_vec()),
        "RIGHT" => Some(b"\x1b[1;5C".to_vec()),
        "UP" => Some(b"\x1b[1;5A".to_vec()),
        "DOWN" => Some(b"\x1b[1;5B".to_vec()),
        "SPACE" => Some(vec![0]),
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() => {
            Some(vec![value.as_bytes()[0].to_ascii_uppercase() - b'A' + 1])
        }
        _ => None,
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

fn format_sequence(sequence: &str) -> String {
    format!("sequence: {}", format_bytes(sequence.as_bytes()))
}

fn is_bash() -> bool {
    if env::var_os("WHYKEY_READLINE_BINDINGS").is_some()
        || env::var_os("WHYKEY_READLINE_MACROS").is_some()
        || env::var_os("WHYKEY_READLINE_SHELL_BINDINGS").is_some()
    {
        return true;
    }
    env::var_os("SHELL")
        .and_then(|shell| PathBuf::from(shell).file_name().map(|name| name == "bash"))
        .unwrap_or(false)
}

fn readline_mode_is_emacs() -> bool {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_VARIABLES") {
        let snapshot = snapshot.to_string_lossy().to_ascii_lowercase();
        return !snapshot.contains("editing-mode is set to `vi'")
            && !snapshot.contains("editing-mode is set to 'vi'")
            && !snapshot.contains("keymap is set to `vi")
            && !snapshot.contains("keymap is set to 'vi");
    }

    for path in inputrc_paths() {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(is_emacs) = inputrc_editing_mode_is_emacs(&content) {
            return is_emacs;
        }
    }
    true
}

fn inputrc_editing_mode_is_emacs(content: &str) -> Option<bool> {
    content.lines().rev().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("set editing-mode")?.trim();
        match value.to_ascii_lowercase().as_str() {
            "emacs" => Some(true),
            "vi" => Some(false),
            _ => None,
        }
    })
}

fn active_keymap() -> Option<String> {
    let snapshot = env::var_os("WHYKEY_READLINE_VARIABLES")?
        .to_string_lossy()
        .into_owned();
    snapshot.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let value = lower.strip_prefix("keymap is set to ")?;
        Some(value.trim().trim_matches(['`', '\'', '"']).to_owned())
    })
}

fn append_mode_detail(details: &mut Vec<String>) {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_VARIABLES") {
        let snapshot = snapshot.to_string_lossy();
        if let Some(line) = snapshot.lines().find(|line| {
            line.to_ascii_lowercase()
                .starts_with("editing-mode is set to")
        }) {
            details.push(line.trim().to_owned());
        }
        if let Some(line) = snapshot
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("keymap is set to"))
        {
            details.push(line.trim().to_owned());
        }
    }
}

fn inputrc_binding(sequence: &str) -> Option<String> {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_BINDINGS") {
        if let Some(binding) = snapshot_binding_current(&snapshot.to_string_lossy(), sequence) {
            return Some(binding);
        }
    }
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_MACROS") {
        if let Some(binding) = macro_binding(&snapshot.to_string_lossy(), sequence) {
            return Some(binding);
        }
    }
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_SHELL_BINDINGS") {
        if let Some(binding) = macro_binding(&snapshot.to_string_lossy(), sequence) {
            return Some(binding);
        }
    }
    inputrc_file_binding(sequence)
}

fn inputrc_file_binding(sequence: &str) -> Option<String> {
    inputrc_paths()
        .into_iter()
        .find_map(|path| inputrc_binding_in_path(&path, sequence, 0))
}

fn inputrc_binding_in_path(path: &PathBuf, sequence: &str, depth: usize) -> Option<String> {
    const MAX_INCLUDE_DEPTH: usize = 16;
    if depth > MAX_INCLUDE_DEPTH {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut active = true;
    let mut conditions = Vec::new();
    let mut binding = None;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(include) = line.strip_prefix("$include") {
            if !active {
                continue;
            }
            let include = resolve_inputrc_include(include.trim(), base);
            if let Some(value) = inputrc_binding_in_path(&include, sequence, depth + 1) {
                binding = Some(value);
            }
            continue;
        }
        if let Some(condition) = line.strip_prefix("$if") {
            let matches = inputrc_condition(condition.trim());
            conditions.push((active, matches));
            active = active && matches;
            continue;
        }
        if line == "$else" {
            if let Some((parent, matches)) = conditions.last().copied() {
                active = parent && !matches;
            }
            continue;
        }
        if line == "$endif" {
            if let Some((parent, _)) = conditions.pop() {
                active = parent;
            }
            continue;
        }
        if !active || line.is_empty() || line.starts_with('#') || line.starts_with('$') {
            continue;
        }
        let Some((raw_sequence, action)) = line.split_once(':') else {
            continue;
        };
        let raw_sequence = raw_sequence.trim().trim_matches('"');
        if decode_sequence(raw_sequence) == sequence {
            // Readline applies later matching declarations in the same file,
            // including declarations after an explicit $include.
            binding = Some(action.trim().to_owned());
        }
    }
    binding
}

fn resolve_inputrc_include(raw: &str, base: &std::path::Path) -> PathBuf {
    let raw = raw.trim().trim_matches(['"', '\'']);
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        if raw == "~" {
            return home;
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn inputrc_condition(condition: &str) -> bool {
    let condition = condition.trim().to_ascii_lowercase();
    if condition == "bash" {
        // whykey only reaches this parser after identifying Bash as the
        // current shell, so the application-name conditional is satisfied.
        return true;
    }
    if let Some(term) = condition.strip_prefix("term=") {
        let current = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
        return if let Some(prefix) = term.strip_suffix('*') {
            current.starts_with(prefix)
        } else {
            current == term
        };
    }
    match condition.strip_prefix("mode=") {
        Some("emacs") => readline_mode_is_emacs(),
        Some("vi") | Some("vi-insertion") | Some("vi-command") => !readline_mode_is_emacs(),
        Some(_) => false,
        None => false,
    }
}

fn inputrc_paths() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("INPUTRC") {
        return vec![PathBuf::from(path)];
    }
    if let Some(home) = env::var_os("HOME") {
        let user = PathBuf::from(home).join(".inputrc");
        if user.is_file() {
            return vec![user];
        }
    }
    vec![PathBuf::from("/etc/inputrc")]
}

fn binding_source(sequence: &str) -> &'static str {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_BINDINGS") {
        if snapshot_binding_current(&snapshot.to_string_lossy(), sequence).is_some() {
            return "current Bash bind -P snapshot";
        }
    }
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_MACROS") {
        if macro_binding(&snapshot.to_string_lossy(), sequence).is_some() {
            return "current Bash bind -S snapshot";
        }
    }
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_SHELL_BINDINGS") {
        if macro_binding(&snapshot.to_string_lossy(), sequence).is_some() {
            return "current Bash bind -X snapshot";
        }
    }
    if inputrc_file_binding(sequence).is_some() {
        "INPUTRC"
    } else {
        "Readline default"
    }
}

fn snapshot_binding(snapshot: &str, sequence: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let (binding, keys) = line.split_once(" can be found on ")?;
        keys.split(',').find_map(|raw_key| {
            let raw_key = raw_key.trim().trim_end_matches('.').trim_matches('"');
            (decode_sequence(raw_key) == sequence).then(|| binding.to_owned())
        })
    })
}

fn snapshot_binding_current(snapshot: &str, sequence: &str) -> Option<String> {
    let Some(map) = active_keymap() else {
        return snapshot_binding(snapshot, sequence);
    };
    snapshot_binding_for_map(snapshot, sequence, &map)
}

fn snapshot_binding_for_map(snapshot: &str, sequence: &str, map: &str) -> Option<String> {
    let marker = format!("# WHYKEY_KEYMAP {map}");
    let before_marker = snapshot.split_once(&marker).map(|(before, _)| before);
    let section = snapshot
        .split_once(&marker)
        .map(|(_, section)| section)
        .unwrap_or(snapshot);
    let section = section.split("# WHYKEY_KEYMAP ").next().unwrap_or(section);
    snapshot_binding(section, sequence)
        .or_else(|| before_marker.and_then(|before| snapshot_binding(before, sequence)))
}

fn macro_binding(snapshot: &str, sequence: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (raw_sequence, macro_value) = if let Some((raw, value)) = line.split_once(" outputs ") {
            (raw.trim(), value.trim())
        } else {
            let (raw, value) = split_quoted_tokens(line)?;
            (raw, value)
        };
        let raw_sequence = raw_sequence.trim().trim_matches('"');
        if decode_sequence(raw_sequence) != sequence {
            return None;
        }
        let macro_value = macro_value.trim();
        Some(if macro_value.is_empty() {
            "macro".into()
        } else {
            format!("macro {macro_value}")
        })
    })
}

fn split_quoted_tokens(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if !line.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    let end = line.char_indices().skip(1).find_map(|(index, character)| {
        if escaped {
            escaped = false;
            return None;
        }
        if character == '\\' {
            escaped = true;
            return None;
        }
        (character == '"').then_some(index)
    })?;
    let value = line[end + 1..].trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    Some((&line[1..end], value))
}

fn decode_sequence(sequence: &str) -> String {
    let mut decoded = String::with_capacity(sequence.len());
    let mut chars = sequence.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }

        let Some(escaped) = chars.next() else {
            decoded.push('\\');
            break;
        };
        match escaped {
            'e' | 'E' => decoded.push('\x1b'),
            'C' if chars.next() == Some('-') => {
                if let Some(control) = chars.next() {
                    let byte = control.to_ascii_uppercase() as u8;
                    decoded.push(char::from(byte & 0x1f));
                }
            }
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    decoded.push(char::from(byte));
                }
            }
            other => {
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_inputrc_escape_notation() {
        assert_eq!(decode_sequence("\\e[1;5D"), "\x1b[1;5D");
    }

    #[test]
    fn maps_ctrl_f_to_forward_char() {
        assert_eq!(readline_binding(&[0x06]), Some(("\x06", "forward-char")));
        assert_eq!(format_sequence("\x06"), "sequence: 0x06");
    }

    #[test]
    fn decodes_inputrc_control_notation() {
        assert_eq!(decode_sequence("\\C-f"), "\x06");
    }

    #[test]
    fn reads_a_bash_bind_snapshot() {
        let snapshot = r#"forward-char can be found on "\C-f".
backward-word can be found on "\e[1;5D"."#;
        assert_eq!(
            snapshot_binding(snapshot, "\x06").as_deref(),
            Some("forward-char")
        );
        assert_eq!(
            snapshot_binding(snapshot, "\x1b[1;5D").as_deref(),
            Some("backward-word")
        );
    }

    #[test]
    fn reads_bash_macro_and_shell_command_snapshots() {
        let macros = r#"\C-y outputs abc
"#;
        let shell_commands = r#""\C-x" "echo hi""#;

        assert_eq!(macro_binding(macros, "\x19").as_deref(), Some("macro abc"));
        assert_eq!(
            macro_binding(shell_commands, "\x18").as_deref(),
            Some("macro echo hi")
        );
    }

    #[test]
    fn selects_the_requested_readline_keymap_section() {
        let snapshot = r#"forward-char can be found on "\C-f".
# WHYKEY_KEYMAP vi-insertion
vi-forward-word can be found on "\C-f".
# WHYKEY_KEYMAP emacs
forward-char can be found on "\C-f"."#;
        assert_eq!(
            snapshot_binding_for_map(snapshot, "\x06", "vi-insertion").as_deref(),
            Some("vi-forward-word")
        );
    }

    #[test]
    fn falls_back_to_current_snapshot_when_map_section_is_empty() {
        let snapshot = r#"forward-char can be found on "\C-f".
# WHYKEY_KEYMAP vi-insertion
# WHYKEY_KEYMAP emacs"#;
        assert_eq!(
            snapshot_binding_for_map(snapshot, "\x06", "vi-insertion").as_deref(),
            Some("forward-char")
        );
    }

    #[test]
    fn parses_quoted_snapshot_tokens() {
        assert_eq!(
            split_quoted_tokens(r#""\e[1;5D" "backward-word""#),
            Some((r#"\e[1;5D"#, "backward-word"))
        );
    }

    #[test]
    fn ignores_unknown_inputrc_conditions() {
        assert!(!inputrc_condition("term=unknown-terminal"));
    }

    #[test]
    fn recognizes_the_bash_application_condition() {
        assert!(inputrc_condition("Bash"));
    }

    #[test]
    fn reads_editing_mode_from_inputrc() {
        assert_eq!(
            inputrc_editing_mode_is_emacs("set editing-mode vi\n"),
            Some(false)
        );
        assert_eq!(
            inputrc_editing_mode_is_emacs("set editing-mode vi\nset editing-mode emacs\n"),
            Some(true)
        );
        assert_eq!(inputrc_editing_mode_is_emacs("set bell-style none\n"), None);
    }

    #[test]
    fn follows_literal_inputrc_includes_and_applies_later_overrides() {
        let base = std::env::temp_dir().join(format!("whykey-inputrc-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let included = base.join("system.inputrc");
        let root = base.join("user.inputrc");
        fs::write(&included, "\"\\C-f\": included\n").unwrap();
        fs::write(&root, "$include system.inputrc\n\"\\C-f\": local\n").unwrap();

        assert_eq!(
            inputrc_binding_in_path(&root, "\x06", 0).as_deref(),
            Some("local")
        );
        let _ = fs::remove_dir_all(base);
    }
}
