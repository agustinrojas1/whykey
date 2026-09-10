use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, format_bytes};

/// Inspect the GNU Screen layer without sending commands to the running
/// session. Screen has no portable IPC endpoint that lists its effective
/// input table, so this adapter reads the same `.screenrc` directives that
/// define custom prefixes and `bind`/`bindkey` entries.
pub fn inspect(key: &KeyCombo) -> LayerResult {
    let mut details = Vec::new();
    let (prefix, config_lines, config_source) = load_config();
    details.push(format!("command prefix: {}", display_bytes(&prefix)));
    if let Some(source) = config_source {
        details.push(format!("config: {}", source.display()));
    } else {
        details.push("config: no readable .screenrc; GNU Screen defaults assumed".into());
    }

    if screen_key_bytes(key).is_some_and(|bytes| bytes == prefix) {
        details.push("this is Screen's command prefix; the next key is read by Screen".into());
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "GNU Screen",
            id: LayerId::Multiplexer,
            outcome: Outcome::Consumed,
            summary: "command prefix is consumed by Screen".into(),
            details,
        };
    }

    let candidates = screen_input_candidates(key);
    let bindkeys = parse_bindkeys(&config_lines);
    if let Some(binding) = bindkeys.iter().find(|binding| {
        candidates
            .iter()
            .any(|candidate| candidate.as_slice() == binding.key.as_slice())
    }) {
        details.push(format!(
            "bindkey sequence: {} → {}",
            display_bytes(&binding.key),
            binding.action
        ));
        details.push(format!("binding source: {}", binding.source));
        let forwards = binding.action.starts_with("stuff")
            || binding.action.starts_with("process")
            || binding.action.starts_with("writebuf");
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "GNU Screen",
            id: LayerId::Multiplexer,
            outcome: if forwards {
                Outcome::HandledAndPassed
            } else {
                Outcome::Consumed
            },
            summary: if forwards {
                "bindkey remaps the sequence and forwards generated input".into()
            } else {
                "bindkey consumes the sequence".into()
            },
            details,
        };
    }

    let command_bindings = parse_bindings(&config_lines);
    if let Some(binding) = command_bindings
        .iter()
        .find(|binding| screen_key_bytes(key).is_some_and(|bytes| bytes == binding.key))
    {
        details.push(format!(
            "bind {} → {} (requires the command prefix first)",
            display_bytes(&binding.key),
            binding.action
        ));
        details.push(format!("binding source: {}", binding.source));
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "GNU Screen",
            id: LayerId::Multiplexer,
            outcome: Outcome::UncertainContinues,
            summary: "binding exists after Screen's prefix; this key alone is forwarded".into(),
            details,
        };
    }

    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "GNU Screen",
        id: LayerId::Multiplexer,
        outcome: Outcome::Pass,
        summary: "no Screen binding; forwarded to the window's PTY".into(),
        details,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScreenBinding {
    key: Vec<u8>,
    action: String,
    source: String,
}

fn load_config() -> (Vec<u8>, Vec<(usize, String)>, Option<PathBuf>) {
    let paths = screen_config_paths();

    let mut prefix = vec![1]; // ^A, GNU Screen's default command character.
    let mut lines = Vec::new();
    let mut source = None;
    for path in paths {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        // The last readable file is the highest-priority source in the
        // effective order above; expose it in the report.
        source = Some(path.clone());
        for (index, raw) in content.lines().enumerate() {
            let line = raw.split_once('#').map_or(raw, |(head, _)| head).trim();
            if line.is_empty() {
                continue;
            }
            let tokens = shell_tokens(line);
            if tokens.first().is_some_and(|token| token == "escape") {
                if let Some(value) = tokens.get(1).and_then(|token| parse_screen_key(token)) {
                    prefix = value;
                }
            }
            lines.push((index + 1, line.to_owned()));
        }
    }
    (prefix, lines, source)
}

fn screen_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("SYSTEM_SCREENRC") {
        paths.push(PathBuf::from(path));
    } else {
        // These are the common install locations; unreadable/nonexistent
        // candidates are skipped below without making the layer unavailable.
        paths.push(PathBuf::from("/usr/local/etc/screenrc"));
        paths.push(PathBuf::from("/etc/screenrc"));
    }

    // Screen uses SCREENRC when set and otherwise falls back to ~/.screenrc.
    // Do not merge both user files: doing so could report bindings that the
    // running Screen process never loaded.
    if let Some(path) = env::var_os("SCREENRC") {
        paths.push(PathBuf::from(path));
    } else if let Some(home) = env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".screenrc"));
    }
    paths
}

fn parse_bindkeys(lines: &[(usize, String)]) -> Vec<ScreenBinding> {
    let mut bindings = Vec::new();
    for (line_number, line) in lines {
        let tokens = shell_tokens(line);
        if tokens.first().map(String::as_str) != Some("bindkey") {
            continue;
        }
        let Some(key_index) = bindkey_key_index(&tokens) else {
            continue;
        };
        let Some(key) = tokens
            .get(key_index)
            .and_then(|token| parse_screen_sequence(token))
        else {
            continue;
        };
        let action = tokens.get(key_index + 1..).unwrap_or_default().join(" ");
        apply_binding(&mut bindings, key, action, *line_number);
    }
    bindings
}

fn parse_bindings(lines: &[(usize, String)]) -> Vec<ScreenBinding> {
    let mut bindings = Vec::new();
    for (line_number, line) in lines {
        let tokens = shell_tokens(line);
        if tokens.first().map(String::as_str) != Some("bind") {
            continue;
        }
        let Some(key_index) = bind_key_index(&tokens) else {
            continue;
        };
        let Some(key) = tokens
            .get(key_index)
            .and_then(|token| parse_screen_key(token))
        else {
            continue;
        };
        let action = tokens.get(key_index + 1..).unwrap_or_default().join(" ");
        apply_binding(&mut bindings, key, action, *line_number);
    }
    bindings
}

fn apply_binding(bindings: &mut Vec<ScreenBinding>, key: Vec<u8>, action: String, line: usize) {
    let position = bindings.iter().position(|binding| binding.key == key);
    if action.is_empty() {
        if let Some(position) = position {
            bindings.remove(position);
        }
        return;
    }
    let binding = ScreenBinding {
        key,
        action,
        source: format!(".screenrc:{line}"),
    };
    if let Some(position) = position {
        bindings[position] = binding;
    } else {
        bindings.push(binding);
    }
}

fn screen_input_candidates(key: &KeyCombo) -> Vec<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some(bytes) = screen_key_bytes(key) {
        candidates.push(bytes);
    }
    let sequences = match key.key() {
        "LEFT" => Some(b"\x1b[1;5D".as_slice()),
        "RIGHT" => Some(b"\x1b[1;5C".as_slice()),
        "UP" => Some(b"\x1b[1;5A".as_slice()),
        "DOWN" => Some(b"\x1b[1;5B".as_slice()),
        _ => None,
    };
    if let Some(sequence) = sequences {
        candidates.push(sequence.to_vec());
    }
    candidates
}

fn screen_key_bytes(key: &KeyCombo) -> Option<Vec<u8>> {
    if key.modmask() == 4 {
        return match key.key() {
            "LEFT" => Some(b"\x1b[1;5D".to_vec()),
            "RIGHT" => Some(b"\x1b[1;5C".to_vec()),
            "UP" => Some(b"\x1b[1;5A".to_vec()),
            "DOWN" => Some(b"\x1b[1;5B".to_vec()),
            "SPACE" => Some(vec![0]),
            value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() => {
                Some(vec![value.as_bytes()[0].to_ascii_uppercase() - b'A' + 1])
            }
            _ => None,
        };
    }
    (key.modmask() == 0 && key.key().len() == 1).then(|| key.key().as_bytes().to_vec())
}

fn parse_screen_sequence(raw: &str) -> Option<Vec<u8>> {
    if raw.starts_with('"') && raw.ends_with('"') {
        return parse_screen_sequence(&raw[1..raw.len() - 1]);
    }
    if raw.starts_with('^') {
        let value = raw.as_bytes().get(1).copied()?;
        return Some(vec![if value == b'?' { 0x7f } else { value & 0x1f }]);
    }
    if let Some(sequence) = termcap_sequence(raw) {
        return Some(sequence.to_vec());
    }
    let mut bytes = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            bytes.push(character as u8);
            continue;
        }
        let next = chars.next()?;
        match next {
            'e' | 'E' => bytes.push(0x1b),
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            'x' => {
                let high = chars.next()?.to_digit(16)?;
                let low = chars.next()?.to_digit(16)?;
                bytes.push((high * 16 + low) as u8);
            }
            digit if ('0'..='7').contains(&digit) => {
                let mut value = digit.to_digit(8)?;
                for _ in 0..2 {
                    let Some(next_digit) = chars.peek().copied() else {
                        break;
                    };
                    let Some(next_value) = next_digit.to_digit(8) else {
                        break;
                    };
                    chars.next();
                    value = value * 8 + next_value;
                }
                bytes.push(value as u8);
            }
            other => bytes.push(other as u8),
        }
    }
    Some(bytes)
}

fn termcap_sequence(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "ku" => b"\x1b[A",
        "kd" => b"\x1b[B",
        "kl" => b"\x1b[D",
        "kr" => b"\x1b[C",
        "k1" => b"\x1bOP",
        "k2" => b"\x1bOQ",
        "k3" => b"\x1bOR",
        "k4" => b"\x1bOS",
        "k5" => b"\x1b[15~",
        "k6" => b"\x1b[17~",
        "k7" => b"\x1b[18~",
        "k8" => b"\x1b[19~",
        "k9" => b"\x1b[20~",
        "k;" => b"\x1b[21~",
        _ => return None,
    })
}

fn parse_screen_key(raw: &str) -> Option<Vec<u8>> {
    parse_screen_sequence(raw).filter(|bytes| !bytes.is_empty())
}

fn shell_tokens(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|token| token.trim_matches(['"', '\'']).to_owned())
        .collect()
}

fn bindkey_key_index(tokens: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < tokens.len() {
        if tokens[index] == "-k" {
            index += 1;
            return (index < tokens.len()).then_some(index);
        }
        if tokens[index].starts_with('-') {
            index += 1;
            continue;
        }
        return Some(index);
    }
    None
}

fn bind_key_index(tokens: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < tokens.len() {
        if tokens[index] == "-c" {
            index += 2;
            continue;
        }
        if tokens[index].starts_with('-') {
            index += 1;
            continue;
        }
        return Some(index);
    }
    None
}

fn display_bytes(bytes: &[u8]) -> String {
    format_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_screen_control_keys() {
        assert_eq!(parse_screen_key("^A"), Some(vec![1]));
        assert_eq!(parse_screen_key("\\032"), Some(vec![26]));
    }

    #[test]
    fn finds_bindkey_and_prefix_bindings() {
        let lines = vec![
            (1, "bindkey \"\\033[1;5D\" stuff \"left\"".into()),
            (2, "bind ^L windowlist".into()),
        ];
        let bindkeys = parse_bindkeys(&lines);
        assert_eq!(bindkeys[0].key, b"\x1b[1;5D".to_vec());
        let bindings = parse_bindings(&lines);
        assert_eq!(bindings[0].key, vec![12]);
    }

    #[test]
    fn maps_ctrl_letters_to_control_bytes() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        assert_eq!(screen_key_bytes(&key), Some(vec![26]));
    }

    #[test]
    fn later_bind_or_unbind_wins() {
        let lines = vec![
            (1, "bind ^K select 1".into()),
            (2, "bind ^K select 2".into()),
            (3, "bind ^L windowlist".into()),
            (4, "bind ^L".into()),
        ];
        let bindings = parse_bindings(&lines);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].action, "select 2");
    }

    #[test]
    fn parses_common_screen_termcap_names() {
        assert_eq!(parse_screen_sequence("ku"), Some(b"\x1b[A".to_vec()));
        assert_eq!(parse_screen_sequence("k1"), Some(b"\x1bOP".to_vec()));
    }
}
