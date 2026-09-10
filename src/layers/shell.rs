use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, TerminalInput, readline};

pub fn inspect(key: &KeyCombo) -> LayerResult {
    match shell_name().as_deref() {
        Some("bash") => readline::Readline.inspect(key),
        Some("zsh") => inspect_zsh(key, None),
        Some("fish") => inspect_fish(key, None),
        Some(shell) => unsupported(shell),
        None => unsupported("unknown shell"),
    }
}

pub fn inspect_input(input: &TerminalInput) -> LayerResult {
    match shell_name().as_deref() {
        Some("bash") => readline::Readline.inspect_input(input),
        Some("zsh") => inspect_zsh_bytes(input),
        Some("fish") => inspect_fish_bytes(input),
        Some(shell) => unsupported(shell),
        None => unsupported("unknown shell"),
    }
}

/// Inspect the shell associated with a caller-selected process. This avoids
/// using whykey's own `SHELL` environment when `inspect --pid` or
/// `inspect --focused` targets a different terminal session.
pub fn inspect_input_for_pid(input: &TerminalInput, target_pid: u32) -> LayerResult {
    let Some(shell) = ancestor_shell(target_pid) else {
        return inspect_input(input);
    };
    match shell.as_str() {
        "bash" => readline::Readline.inspect_input(input),
        "zsh" => inspect_zsh_bytes(input),
        "fish" => inspect_fish_bytes(input),
        other => unsupported(other),
    }
}

fn unsupported(shell: &str) -> LayerResult {
    LayerResult {
        binding: None,
        layer: "Shell input",
        id: LayerId::Shell,
        outcome: Outcome::Unavailable,
        summary: format!("shell '{shell}' is not inspected"),
        details: vec!["install shell integration or use a supported shell adapter".into()],
    }
}

fn shell_name() -> Option<String> {
    if env::var_os("WHYKEY_READLINE_BINDINGS").is_some()
        || env::var_os("WHYKEY_READLINE_MACROS").is_some()
        || env::var_os("WHYKEY_READLINE_SHELL_BINDINGS").is_some()
    {
        return Some("bash".into());
    }
    if env::var_os("WHYKEY_ZLE_BINDINGS").is_some() {
        return Some("zsh".into());
    }
    if env::var_os("WHYKEY_FISH_BINDINGS").is_some() {
        return Some("fish".into());
    }
    env::var_os("SHELL")
        .and_then(|shell| PathBuf::from(shell).file_name().map(|name| name.to_owned()))
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
}

fn ancestor_shell(start_pid: u32) -> Option<String> {
    let mut pid = start_pid;
    for _ in 0..16 {
        let command = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let name = command
            .split(|byte| *byte == 0)
            .find(|part| !part.is_empty())
            .and_then(|part| std::str::from_utf8(part).ok())
            .and_then(|part| PathBuf::from(part).file_name().map(|name| name.to_owned()))
            .map(|name| name.to_string_lossy().to_ascii_lowercase());
        if let Some(name) = name.filter(|name| matches!(name.as_str(), "bash" | "zsh" | "fish")) {
            return Some(name);
        }
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(") ")?;
        let fields: Vec<_> = stat[close + 2..].split_whitespace().collect();
        let parent = fields.get(1)?.parse::<u32>().ok()?;
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    None
}

fn inspect_zsh_bytes(input: &TerminalInput) -> LayerResult {
    if let Some((sequence, default_binding)) = zsh_known_binding(&input.bytes) {
        return inspect_configured_shell_binding("Zsh / ZLE", sequence, default_binding, input);
    }
    let sequence = shell_sequence_literal(&input.bytes);
    if let Some(binding) = zsh_binding(&sequence) {
        return inspect_dynamic_shell_binding("Zsh / ZLE", &sequence, &binding, input);
    }
    no_shell_binding("Zsh / ZLE", input)
}

fn inspect_fish_bytes(input: &TerminalInput) -> LayerResult {
    if let Some((sequence, default_binding)) = fish_known_binding(&input.bytes) {
        return inspect_configured_shell_binding("Fish", sequence, default_binding, input);
    }
    let sequence = shell_sequence_literal(&input.bytes);
    if let Some(binding) = fish_binding(&sequence) {
        return inspect_dynamic_shell_binding("Fish", &sequence, &binding, input);
    }
    let mut result = no_shell_binding("Fish", input);
    append_fish_mode_detail(&mut result.details);
    result
}

fn inspect_configured_shell_binding(
    layer: &'static str,
    sequence: &str,
    default_binding: &'static str,
    input: &TerminalInput,
) -> LayerResult {
    let binding = match layer {
        "Zsh / ZLE" => zsh_binding(sequence),
        "Fish" => fish_binding(sequence),
        _ => None,
    };
    let configured = binding.is_some();
    let binding = binding.unwrap_or_else(|| default_binding.to_owned());
    let source = if runtime_shell_snapshot(layer) {
        "binding source: live shell snapshot"
    } else if configured {
        "binding source: shell configuration"
    } else {
        "binding source: common shell default (inferred)"
    };
    LayerResult {
        binding: None,
        layer,
        id: LayerId::Shell,
        outcome: if configured {
            Outcome::Consumed
        } else {
            Outcome::Unknown
        },
        summary: if configured {
            format!("bound to {binding}")
        } else {
            format!("common default likely binds to {binding}")
        },
        details: {
            let mut details = vec![
                format!(
                    "sequence: {}",
                    crate::layers::format_bytes(sequence.as_bytes())
                ),
                format!("input: {}", input.display_bytes()),
                source.into(),
            ];
            if layer == "Fish" {
                append_fish_mode_detail(&mut details);
            }
            details
        },
    }
}

fn runtime_shell_snapshot(layer: &str) -> bool {
    match layer {
        "Zsh / ZLE" => env::var_os("WHYKEY_ZLE_BINDINGS").is_some(),
        "Fish" => env::var_os("WHYKEY_FISH_BINDINGS").is_some(),
        _ => false,
    }
}

fn append_fish_mode_detail(details: &mut Vec<String>) {
    if let Some(mode) = env::var_os("WHYKEY_FISH_MODE") {
        details.push(format!("active Fish bind mode: {}", mode.to_string_lossy()));
    } else if env::var_os("WHYKEY_FISH_BINDINGS").is_some() {
        details.push("active Fish bind mode is unknown".into());
    }
}

fn inspect_dynamic_shell_binding(
    layer: &'static str,
    sequence: &str,
    binding: &str,
    input: &TerminalInput,
) -> LayerResult {
    let source = if runtime_shell_snapshot(layer) {
        "binding source: live shell snapshot"
    } else {
        "binding source: shell configuration"
    };
    let mut details = vec![
        format!(
            "sequence: {}",
            crate::layers::format_bytes(sequence.as_bytes())
        ),
        format!("input: {}", input.display_bytes()),
        source.into(),
    ];
    if layer == "Fish" {
        append_fish_mode_detail(&mut details);
    }
    LayerResult {
        binding: None,
        layer,
        id: LayerId::Shell,
        outcome: Outcome::Consumed,
        summary: format!("bound to {binding}"),
        details,
    }
}

fn no_shell_binding(layer: &'static str, input: &TerminalInput) -> LayerResult {
    LayerResult {
        binding: None,
        layer,
        id: LayerId::Shell,
        outcome: Outcome::Pass,
        summary: "no known shell binding".into(),
        details: vec![format!("input: {}", input.display_bytes())],
    }
}

fn shell_sequence_literal(bytes: &[u8]) -> String {
    let mut sequence = String::new();
    for byte in bytes {
        match byte {
            0x1b => sequence.push_str("\\e"),
            0x20..=0x7e if *byte != b'\\' && *byte != b'\'' && *byte != b'"' => {
                sequence.push(*byte as char)
            }
            byte => sequence.push_str(&format!("\\x{byte:02x}")),
        }
    }
    sequence
}

fn inspect_zsh(key: &KeyCombo, input: Option<&TerminalInput>) -> LayerResult {
    let Some((sequence, default_binding)) = zsh_known_key(key) else {
        return LayerResult {
            binding: None,
            layer: "Zsh / ZLE",
            id: LayerId::Shell,
            outcome: Outcome::Pass,
            summary: "no known Zsh binding".into(),
            details: vec!["static ZLE defaults are only mapped for common control keys".into()],
        };
    };
    if let Some(input) = input {
        inspect_configured_shell_binding("Zsh / ZLE", sequence, default_binding, input)
    } else {
        let binding = zsh_binding(sequence).unwrap_or_else(|| default_binding.into());
        LayerResult {
            binding: None,
            layer: "Zsh / ZLE",
            id: LayerId::Shell,
            outcome: Outcome::Unknown,
            summary: format!("common default likely binds to {binding}"),
            details: vec![format!(
                "sequence: {}",
                crate::layers::format_bytes(sequence.as_bytes())
            )],
        }
    }
}

fn inspect_fish(key: &KeyCombo, input: Option<&TerminalInput>) -> LayerResult {
    let Some((sequence, default_binding)) = fish_known_key(key) else {
        return LayerResult {
            binding: None,
            layer: "Fish",
            id: LayerId::Shell,
            outcome: Outcome::Pass,
            summary: "no known Fish binding".into(),
            details: {
                let mut details = vec![
                    "Fish defaults and active mode are not available without runtime bind output"
                        .into(),
                ];
                append_fish_mode_detail(&mut details);
                details
            },
        };
    };
    if let Some(input) = input {
        inspect_configured_shell_binding("Fish", sequence, default_binding, input)
    } else {
        let binding = fish_binding(sequence).unwrap_or_else(|| default_binding.into());
        LayerResult {
            binding: None,
            layer: "Fish",
            id: LayerId::Shell,
            outcome: Outcome::Unknown,
            summary: format!("common default likely binds to {binding}"),
            details: vec![format!(
                "sequence: {}",
                crate::layers::format_bytes(sequence.as_bytes())
            )],
        }
    }
}

fn zsh_known_key(key: &KeyCombo) -> Option<(&'static str, &'static str)> {
    if key.modmask() == 4 && key.key().len() == 1 {
        return match key.key() {
            "A" => Some(("\x01", "beginning-of-line")),
            "B" => Some(("\x02", "backward-char")),
            "D" => Some(("\x04", "delete-char")),
            "E" => Some(("\x05", "end-of-line")),
            "F" => Some(("\x06", "forward-char")),
            "N" => Some(("\x0e", "down-line-or-history")),
            "P" => Some(("\x10", "up-line-or-history")),
            "W" => Some(("\x17", "backward-kill-word")),
            _ => None,
        };
    }
    None
}

fn fish_known_key(key: &KeyCombo) -> Option<(&'static str, &'static str)> {
    zsh_known_key(key)
}

fn zsh_known_binding(sequence: &[u8]) -> Option<(&'static str, &'static str)> {
    let key = match sequence {
        [0x01] => "A",
        [0x02] => "B",
        [0x04] => "D",
        [0x05] => "E",
        [0x06] => "F",
        [0x0e] => "N",
        [0x10] => "P",
        [0x17] => "W",
        _ => return None,
    };
    zsh_known_key(&format!("ctrl+{key}").parse().ok()?)
}

fn fish_known_binding(sequence: &[u8]) -> Option<(&'static str, &'static str)> {
    zsh_known_binding(sequence)
}

fn zsh_binding(sequence: &str) -> Option<String> {
    if let Some(runtime) = env::var_os("WHYKEY_ZLE_BINDINGS") {
        return find_zsh_binding(&runtime.to_string_lossy(), sequence);
    }
    config_binding(zsh_paths(), sequence, |line| line.contains("bindkey"))
}

fn fish_binding(sequence: &str) -> Option<String> {
    if let Some(runtime) = env::var_os("WHYKEY_FISH_BINDINGS") {
        let mode = env::var("WHYKEY_FISH_MODE").ok();
        return find_fish_binding(&runtime.to_string_lossy(), sequence, mode.as_deref());
    }
    let mode = env::var("WHYKEY_FISH_MODE").ok();
    fish_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(path).ok()?;
        // A static config has no live mode, so Fish's documented default mode
        // is the only mode-specific binding we can safely select.
        find_fish_binding(&content, sequence, mode.as_deref().or(Some("default")))
    })
}

fn find_zsh_binding(content: &str, sequence: &str) -> Option<String> {
    let active_map = zsh_active_keymap(content);
    let mut active_binding = None;
    let mut unqualified_binding = None;
    for line in content.lines() {
        let Some(line) = line.split('#').next() else {
            continue;
        };
        let line = line.trim();
        if !line.starts_with("bindkey") || line.starts_with("bindkey -A") {
            continue;
        }
        let tokens = quoted_or_bare_tokens(line);
        let (map, offset) = if tokens.get(1).is_some_and(|token| token == "-M") {
            (tokens.get(2).cloned(), 3)
        } else {
            (None, 1)
        };
        if let Some(active_map) = active_map.as_deref() {
            if let Some(map) = map.as_deref() {
                if map != active_map {
                    continue;
                }
            }
        }
        let Some(raw) = tokens.get(offset) else {
            continue;
        };
        if decode_shell_sequence(raw) != sequence {
            continue;
        }
        let Some(action) = tokens.get(offset + 1).cloned() else {
            continue;
        };
        if map.is_some() {
            active_binding = Some(action);
        } else {
            unqualified_binding = Some(action);
        }
    }
    active_binding.or(unqualified_binding)
}

fn find_fish_binding(content: &str, sequence: &str, active_mode: Option<&str>) -> Option<String> {
    let mut active_binding = None;
    let mut unqualified_binding = None;

    for line in content.lines() {
        let Some(line) = line.split('#').next() else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() || !line.starts_with("bind ") {
            continue;
        }
        let tokens = quoted_or_bare_tokens(line);
        if tokens.first().map(String::as_str) != Some("bind") {
            continue;
        }
        let mut mode = None;
        for index in 1..tokens.len() {
            let Some(token) = tokens.get(index) else {
                continue;
            };
            if token == "-M" || token == "--mode" {
                mode = tokens.get(index + 1).map(String::as_str);
            }
        }
        let Some(index) = tokens
            .iter()
            .position(|token| decode_fish_sequence(token) == sequence)
        else {
            continue;
        };
        let Some(action) = tokens.get(index + 1) else {
            continue;
        };

        match (active_mode, mode) {
            (Some(active), Some(candidate)) if candidate == active => {
                active_binding = Some(action.clone());
            }
            (_, None) => {
                unqualified_binding = Some(action.clone());
            }
            _ => {}
        }
    }

    active_binding.or(unqualified_binding)
}

fn zsh_active_keymap(content: &str) -> Option<String> {
    let mut active_map = None;
    for line in content.lines() {
        let tokens = quoted_or_bare_tokens(line.trim());
        if tokens.first().is_some_and(|token| token == "bindkey") {
            if tokens.get(1).is_some_and(|token| token == "-A")
                && tokens.get(3).is_some_and(|token| token == "main")
            {
                active_map = tokens.get(2).cloned();
            }
            if tokens.get(1).is_some_and(|token| token == "-e") {
                active_map = Some("emacs".into());
            }
            if tokens.get(1).is_some_and(|token| token == "-v") {
                active_map = Some("viins".into());
            }
        }
    }
    active_map
}

fn config_binding<F>(paths: Vec<PathBuf>, sequence: &str, predicate: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let mut binding = None;
    for path in paths {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(value) = find_config_binding(&content, sequence, &predicate) {
            binding = Some(value);
        }
    }
    binding
}

fn find_config_binding<F>(content: &str, sequence: &str, predicate: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    content
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            if !predicate(line) {
                return None;
            }
            let tokens = quoted_or_bare_tokens(line);
            let raw = tokens
                .iter()
                .find(|token| decode_shell_sequence(token) == sequence)?;
            let index = tokens.iter().position(|token| token == raw)?;
            tokens.get(index + 1).cloned()
        })
        .last()
}

fn zsh_paths() -> Vec<PathBuf> {
    let Some(base) = env::var_os("ZDOTDIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
    else {
        return Vec::new();
    };
    // zsh reads .zshenv before .zshrc; later definitions override earlier
    // ones, so preserve that execution order when resolving a static bind.
    vec![base.join(".zshenv"), base.join(".zshrc")]
}

fn fish_paths() -> Vec<PathBuf> {
    crate::util::config_base()
        .map(|base| vec![base.join("fish/config.fish")])
        .unwrap_or_default()
}

fn quoted_or_bare_tokens(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|token| token.trim_matches(['"', '\'']).to_owned())
        .collect()
}

fn decode_fish_sequence(value: &str) -> String {
    let decoded = decode_shell_sequence(value);
    if decoded != value {
        return decoded;
    }
    if value.contains(',') {
        return value
            .split(',')
            .map(decode_fish_sequence)
            .collect::<String>();
    }

    let mut rest = value.to_ascii_lowercase();
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_key = false;
    loop {
        let (flag, prefix) = if let Some(rest) = rest.strip_prefix("ctrl-") {
            ("ctrl", rest)
        } else if let Some(rest) = rest.strip_prefix("alt-") {
            ("alt", rest)
        } else if let Some(rest) = rest.strip_prefix("shift-") {
            ("shift", rest)
        } else if let Some(rest) = rest.strip_prefix("super-") {
            ("super", rest)
        } else {
            break;
        };
        rest = prefix.to_owned();
        match flag {
            "ctrl" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "super" => super_key = true,
            _ => unreachable!(),
        }
    }

    let arrow = match rest.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        _ => None,
    };
    if let Some(final_byte) = arrow {
        if super_key {
            return value.to_owned();
        }
        let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
        return if modifier == 1 {
            format!("\x1b[{final_byte}")
        } else {
            format!("\x1b[1;{modifier}{final_byte}")
        };
    }

    let named = match rest.as_str() {
        "home" => Some("H"),
        "end" => Some("F"),
        _ => None,
    };
    if let Some(final_byte) = named {
        if super_key {
            return value.to_owned();
        }
        let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
        return if modifier == 1 {
            format!("\x1b[{final_byte}")
        } else {
            format!("\x1b[1;{modifier}{final_byte}")
        };
    }

    let standalone = match rest.as_str() {
        "tab" => Some('\t'.to_string()),
        "enter" => Some('\r'.to_string()),
        "escape" => Some('\x1b'.to_string()),
        "backspace" => Some('\x7f'.to_string()),
        "delete" => Some("\x1b[3~".into()),
        "pageup" => Some("\x1b[5~".into()),
        "pagedown" => Some("\x1b[6~".into()),
        _ => None,
    };
    if let Some(sequence) = standalone {
        if ctrl || alt || shift || super_key {
            return value.to_owned();
        }
        return sequence;
    }

    let character = match rest.as_str() {
        "space" => ' ',
        value if value.chars().count() == 1 => value.chars().next().unwrap_or_default(),
        _ => return value.to_owned(),
    };
    if super_key {
        return value.to_owned();
    }
    if ctrl {
        let byte = character.to_ascii_uppercase() as u8;
        return char::from(byte & 0x1f).to_string();
    }
    let character = if shift {
        character.to_ascii_uppercase()
    } else {
        character
    };
    if alt {
        format!("\x1b{character}")
    } else {
        character.to_string()
    }
}

fn decode_shell_sequence(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character == '^' {
            if let Some(next) = chars.next() {
                output.push(char::from((next.to_ascii_uppercase() as u8) & 0x1f));
            }
        } else if character == '\\' {
            match chars.next() {
                Some('e') | Some('E') => output.push('\x1b'),
                Some('c') | Some('C') => {
                    let next = chars.next();
                    let next = if next == Some('-') {
                        chars.next()
                    } else {
                        next
                    };
                    if let Some(next) = next {
                        output.push(char::from((next.to_ascii_uppercase() as u8) & 0x1f));
                    }
                }
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Ok(value) = u8::from_str_radix(&hex, 16) {
                        output.push(char::from(value));
                    }
                }
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('t') => output.push('\t'),
                Some('\\') => output.push('\\'),
                Some('\'') => output.push('\''),
                Some('"') => output.push('"'),
                Some(other) => output.push(other),
                None => output.push('\\'),
            }
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_zsh_and_fish_control_notation() {
        assert_eq!(decode_shell_sequence("^F"), "\x06");
        assert_eq!(decode_shell_sequence("\\cf"), "\x06");
        assert_eq!(decode_shell_sequence("\\c-f"), "\x06");
    }

    #[test]
    fn formats_arbitrary_terminal_bytes_for_shell_lookup() {
        assert_eq!(shell_sequence_literal(b"\x1b[1;5D"), "\\e[1;5D");
        assert_eq!(shell_sequence_literal(&[0x06]), "\\x06");
    }

    #[test]
    fn finds_a_runtime_zle_binding() {
        let content = "bindkey '^F' forward-char\n";
        assert_eq!(
            find_config_binding(content, "\x06", |line| line.starts_with("bindkey")),
            Some("forward-char".into())
        );
    }

    #[test]
    fn finds_a_non_letter_zle_binding() {
        let content = "bindkey -M emacs '\\e[1;5D' backward-word\n";
        assert_eq!(
            find_config_binding(content, "\x1b[1;5D", |line| line.starts_with("bindkey")),
            Some("backward-word".into())
        );
    }

    #[test]
    fn selects_the_active_zle_keymap_from_runtime_bindings() {
        let content = r#"bindkey -A viins main
bindkey -M emacs '^F' forward-char
bindkey -M viins '^F' vi-forward-word"#;
        assert_eq!(zsh_active_keymap(content).as_deref(), Some("viins"));
        assert_eq!(
            find_zsh_binding(content, "\x06").as_deref(),
            Some("vi-forward-word")
        );
    }

    #[test]
    fn last_effective_zle_binding_wins() {
        let content = r#"bindkey -A emacs main
bindkey -M emacs '^F' old-widget
bindkey -M emacs '^F' forward-char"#;
        assert_eq!(
            find_zsh_binding(content, "\x06").as_deref(),
            Some("forward-char")
        );
    }

    #[test]
    fn last_zle_main_keymap_alias_wins() {
        let content = "bindkey -A emacs main\nbindkey -A viins main\n";
        assert_eq!(zsh_active_keymap(content).as_deref(), Some("viins"));
    }

    #[test]
    fn selects_the_active_fish_bind_mode() {
        let content = r#"bind -M insert \cF backward-char
	bind -M default \cF forward-char"#;
        assert_eq!(
            find_fish_binding(content, "\x06", Some("default")).as_deref(),
            Some("forward-char")
        );
        assert_eq!(
            find_fish_binding(content, "\x06", Some("insert")).as_deref(),
            Some("backward-char")
        );
    }

    #[test]
    fn decodes_common_shell_escapes() {
        assert_eq!(decode_shell_sequence(r#"\n\r\t\\\"'"#), "\n\r\t\\\"'");
    }

    #[test]
    fn decodes_fish_named_key_sequences() {
        assert_eq!(decode_fish_sequence("ctrl-f"), "\x06");
        assert_eq!(decode_fish_sequence("ctrl-left"), "\x1b[1;5D");
        assert_eq!(decode_fish_sequence("alt-x"), "\x1bx");
        assert_eq!(decode_fish_sequence("left"), "\x1b[D");
        assert_eq!(decode_fish_sequence("ctrl-x,ctrl-e"), "\x18\x05");
        assert_eq!(decode_fish_sequence("pageup"), "\x1b[5~");
    }

    #[test]
    fn matches_fish_named_keys_in_runtime_output() {
        let content = r#"bind -M default ctrl-left backward-word"#;
        assert_eq!(
            find_fish_binding(content, "\x1b[1;5D", Some("default")).as_deref(),
            Some("backward-word")
        );
    }

    #[test]
    fn does_not_claim_a_mode_specific_fish_binding_without_active_mode() {
        let content = r#"bind -M insert \cF backward-char"#;
        assert_eq!(find_fish_binding(content, "\x06", None), None);
    }

    #[test]
    fn last_effective_fish_binding_wins() {
        let content = r#"bind -M default \cF backward-char
	bind -M default \cF forward-char"#;
        assert_eq!(
            find_fish_binding(content, "\x06", Some("default")).as_deref(),
            Some("forward-char")
        );
    }
}
