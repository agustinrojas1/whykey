use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingScope, LayerId, LayerResult, Outcome, UncertaintyReason,
};

pub fn inspect(key: &KeyCombo) -> LayerResult {
    inspect_with_byte_status(key, true)
}

pub fn inspect_with_byte_status(key: &KeyCombo, bytes_verified: bool) -> LayerResult {
    let active_table = tmux_active_table();
    let root_output = match list_keys("root") {
        Ok(output) => output,
        Err(message) => return unavailable(message),
    };

    let mut details = vec!["table: root (direct terminal input)".into()];
    if let Some(table) = &active_table {
        details.push(format!("active client key table: {table}"));
    } else {
        details.push("active client key table: unknown".into());
    }
    let prefixes = tmux_prefixes();
    for prefix in &prefixes {
        details.push(format!("prefix: {prefix}"));
    }
    if let Some(pane) = std::env::var_os("TMUX_PANE") {
        details.push(format!("client pane: {}", pane.to_string_lossy()));
    }

    if let Some(table) = active_table.as_deref() {
        if table != "root" {
            match list_keys(table) {
                Ok(output) => {
                    if let Some(binding) = find_binding(&output, key, table) {
                        let context = if matches!(table, "prefix" | "prefix2") {
                            "active prefix table (prefix already pressed)"
                        } else {
                            "active client table"
                        };
                        details.push(format!("table: {table} ({context})"));
                        details.push(format!("binding: {}", binding.description));
                        return result_for_binding(&binding.action, details, bytes_verified);
                    }
                }
                Err(message) => {
                    details.push(format!("could not read active table {table}: {message}"));
                    return LayerResult {
                        verbose_details: Vec::new(),
                        binding: None,
                        layer: "tmux",
                        id: LayerId::Multiplexer,
                        outcome: Outcome::Unknown,
                        summary: "active tmux table could not be inspected".into(),
                        details,
                    };
                }
            }
            details.push(format!("active table {table} has no binding for this key"));
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "tmux",
                id: LayerId::Multiplexer,
                outcome: Outcome::Pass,
                summary: "no binding in the active tmux table; forwarded to the pane".into(),
                details,
            };
        }
    }

    if let Some(binding) = find_binding(&root_output, key, "root") {
        details.push(format!("binding: {}", binding.description));
        return result_for_binding(&binding.action, details, bytes_verified);
    }

    if active_table.is_none() {
        for table in ["prefix", "prefix2"] {
            let Ok(output) = list_keys(table) else {
                continue;
            };
            if let Some(binding) = find_binding(&output, key, table) {
                details.push(format!(
                    "table: {table} (requires the configured prefix first)"
                ));
                details.push(format!("binding: {}", binding.description));
                return result_for_binding(&binding.action, details, bytes_verified);
            }
        }
    }

    if active_table.is_none()
        && prefixes
            .iter()
            .filter_map(|prefix| parse_tmux_key(prefix))
            .any(|prefix| prefix == *key)
    {
        details.push("the configured prefix starts a tmux key sequence".into());
        let outcome = if bytes_verified {
            Outcome::Consumed
        } else {
            Outcome::HandledUncertain
        };
        let summary = if bytes_verified {
            "prefix key is consumed by tmux".into()
        } else {
            "configured prefix matches, but terminal byte delivery is unverified".into()
        };
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "tmux",
            id: LayerId::Multiplexer,
            outcome,
            summary,
            details,
        };
    }

    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "tmux",
        id: LayerId::Multiplexer,
        outcome: Outcome::Pass,
        summary: if bytes_verified {
            "no direct tmux binding; forwarded to the shell".into()
        } else {
            "no direct tmux binding; terminal byte delivery remains unverified".into()
        },
        details,
    }
}

fn list_keys(table: &str) -> Result<Vec<u8>, String> {
    let mut command = Command::new("tmux");
    command.args(["list-keys", "-a", "-T", table]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

fn tmux_active_table() -> Option<String> {
    let mut command = Command::new("tmux");
    if let Some(pane) = std::env::var_os("TMUX_PANE") {
        let pane = pane.to_string_lossy().into_owned();
        command.args(["display-message", "-p", "-t", &pane, "#{client_key_table}"]);
    } else {
        command.args(["display-message", "-p", "#{client_key_table}"]);
    }
    let output = command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let table = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!table.is_empty()).then_some(table)
}

fn result_for_binding(action: &str, mut details: Vec<String>, bytes_verified: bool) -> LayerResult {
    let binding = BindingEvidence {
        dispatcher: None,
        action: Some(action.to_owned()),
        description: None,
        submap: None,
        scope: BindingScope::Unknown,
        source: None,
        has_universal_match: false,
        uncertainty: if !bytes_verified {
            Some(UncertaintyReason::UnverifiedTerminalBytes)
        } else {
            None
        },
    };
    if !bytes_verified {
        details.push(
            "candidate binding matches in tmux, but terminal byte delivery is unverified".into(),
        );
        return LayerResult {
            verbose_details: Vec::new(),
            binding: Some(binding),
            layer: "tmux",
            id: LayerId::Multiplexer,
            outcome: Outcome::HandledUncertain,
            summary: "candidate binding matches in tmux root table; delivery is unverified".into(),
            details,
        };
    }
    if action == "send-keys" || action.starts_with("send-keys ") {
        details.push("tmux emits another key sequence downstream".into());
        return LayerResult {
            verbose_details: Vec::new(),
            binding: Some(binding),
            layer: "tmux",
            id: LayerId::Multiplexer,
            outcome: Outcome::HandledAndPassed,
            summary: "binding remaps the key and forwards generated input".into(),
            details,
        };
    }
    LayerResult {
        verbose_details: Vec::new(),
        binding: Some(binding),
        layer: "tmux",
        id: LayerId::Multiplexer,
        outcome: Outcome::Consumed,
        summary: "binding consumes the key".into(),
        details,
    }
}

fn unavailable(message: String) -> LayerResult {
    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "tmux",
        id: LayerId::Multiplexer,
        outcome: Outcome::Unavailable,
        summary: "could not inspect tmux key bindings".into(),
        details: vec![if message.is_empty() {
            "tmux did not return an error message".into()
        } else {
            format!("tmux: {message}")
        }],
    }
}

struct Binding {
    action: String,
    description: String,
}

fn find_binding(output: &[u8], key: &KeyCombo, expected_table: &str) -> Option<Binding> {
    let text = std::str::from_utf8(output).ok()?;
    text.lines().find_map(|line| {
        let tokens: Vec<_> = line.split_whitespace().collect();
        if tokens.first().copied() != Some("bind-key") {
            return None;
        }
        let table_index = tokens.iter().position(|token| *token == "-T")?;
        if tokens.get(table_index + 1).copied() != Some(expected_table) {
            return None;
        }
        let key_index = tokens
            .iter()
            .enumerate()
            .skip(table_index + 2)
            .find_map(|(index, token)| (!token.starts_with('-')).then_some(index))?;
        let parsed = parse_tmux_key(tokens[key_index])?;
        if parsed != *key {
            return None;
        }
        let action = tokens.get(key_index + 1..)?.join(" ");
        Some(Binding {
            action,
            description: line.to_owned(),
        })
    })
}

fn tmux_prefixes() -> Vec<String> {
    ["prefix", "prefix2"]
        .into_iter()
        .filter_map(|option| {
            let mut command = Command::new("tmux");
            command.args(["show-options", "-gqv", option]);
            let output = command::output(&mut command).ok()?;
            if !output.status.success() {
                return None;
            }
            let prefix = String::from_utf8(output.stdout).ok()?.trim().to_owned();
            (!prefix.is_empty()).then_some(prefix)
        })
        .collect()
}

fn parse_tmux_key(raw: &str) -> Option<KeyCombo> {
    let mut modifiers = 0;
    let mut key = raw;
    while let Some((prefix, rest)) = key.split_once('-') {
        match prefix.to_ascii_uppercase().as_str() {
            "C" => modifiers |= 4,
            "M" => modifiers |= 8,
            "S" => modifiers |= 1,
            _ => break,
        }
        key = rest;
    }
    let key_lower = key.to_ascii_lowercase();
    let normalized = match key_lower.as_str() {
        "left" => "LEFT",
        "right" => "RIGHT",
        "up" => "UP",
        "down" => "DOWN",
        "home" => "HOME",
        "end" => "END",
        "npage" | "pagedown" => "PAGE_DOWN",
        "ppage" | "pageup" => "PAGE_UP",
        "ic" | "insert" => "INSERT",
        "dc" | "delete" => "DELETE",
        "btab" | "backtab" => "TAB",
        "space" => "SPACE",
        "enter" | "return" => "RETURN",
        value if value.starts_with('f') && value[1..].parse::<u8>().is_ok() => {
            return Some(KeyCombo::from_parts(modifiers, value.to_ascii_uppercase()));
        }
        value if value.len() == 1 => value,
        _ => return None,
    };
    Some(KeyCombo::from_parts(modifiers, normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_key_names() {
        assert_eq!(parse_tmux_key("C-Left").unwrap().to_string(), "CTRL + LEFT");
        assert_eq!(
            parse_tmux_key("C-BogusKeyXYZ"),
            None,
            "an unknown tmux key name never becomes a false positive binding"
        );
        assert_eq!(
            parse_tmux_key("M-S-x").unwrap().to_string(),
            "ALT + SHIFT + X"
        );
        assert_eq!(parse_tmux_key("C-b").unwrap().to_string(), "CTRL + B");
    }

    #[test]
    fn finds_a_root_binding() {
        let output = b"bind-key -T root C-Left send-keys C-b\n";
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let binding = find_binding(output, &key, "root").unwrap();
        assert_eq!(binding.action, "send-keys C-b");
    }

    #[test]
    fn finds_a_binding_after_the_prefix() {
        let output = b"bind-key -T prefix C-Left display-message moved\n";
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let binding = find_binding(output, &key, "prefix").unwrap();
        assert_eq!(binding.action, "display-message moved");
    }

    #[test]
    fn parses_custom_table_names_without_special_casing_them() {
        let output = b"bind-key -T resize C-Left resize-pane -L 5\n";
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let binding = find_binding(output, &key, "resize").unwrap();
        assert_eq!(binding.action, "resize-pane -L 5");
    }
}
