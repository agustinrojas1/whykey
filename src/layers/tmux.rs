use std::collections::HashMap;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingScope, EvidenceTier, LayerId, LayerResult, Outcome, UncertaintyReason,
};

pub fn inspect(key: &KeyCombo) -> LayerResult {
    inspect_with_byte_status(&[], key, true)
}

/// Inspect `key` as tmux receives it. `preceding` holds the earlier keys of
/// the same sequence that reached tmux, so `ctrl+b ctrl+left` resolves
/// `ctrl+left` in the prefix table while a bare `ctrl+left` only consults
/// the client's current table (normally root).
pub fn inspect_with_byte_status(
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
) -> LayerResult {
    let active_table = tmux_active_table();
    let prefixes = tmux_prefixes();
    let mut details = Vec::new();
    if let Some(pane) = std::env::var_os("TMUX_PANE") {
        details.push(format!("client pane: {}", pane.to_string_lossy()));
    }
    let mut cache = HashMap::new();
    let mut tables = |table: &str| {
        cache
            .entry(table.to_owned())
            .or_insert_with(|| list_keys(table))
            .clone()
    };
    let mut result = inspect_with_tables(
        active_table.as_deref(),
        &prefixes,
        preceding,
        key,
        bytes_verified,
        &mut tables,
    );
    result.details.splice(0..0, details);
    result
}

/// Key-table resolution over an injectable `list-keys` source. The tables
/// come from the running tmux server, so every answer is a live query.
fn inspect_with_tables(
    active_table: Option<&str>,
    raw_prefixes: &[String],
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
    tables: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
) -> LayerResult {
    let result = resolve_with_tables(
        active_table,
        raw_prefixes,
        preceding,
        key,
        bytes_verified,
        tables,
    );
    if result.outcome == Outcome::Unavailable {
        result
    } else {
        result.with_tier(EvidenceTier::LiveQuery)
    }
}

/// Mirrors tmux's client key handling: the prefix switches to the prefix
/// table, a binding runs and returns the client to root (or the table named
/// by `switch-client -T`), and an unbound key in a non-root table is retried
/// in root and otherwise discarded.
fn resolve_with_tables(
    active_table: Option<&str>,
    raw_prefixes: &[String],
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
    tables: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
) -> LayerResult {
    let mut details = Vec::new();
    match active_table {
        Some(table) => details.push(format!("active client key table: {table}")),
        None => details.push("active client key table: unknown; assuming root".into()),
    }
    for prefix in raw_prefixes {
        details.push(format!("prefix: {prefix}"));
    }
    let prefixes = raw_prefixes
        .iter()
        .filter_map(|prefix| parse_tmux_key(prefix))
        .collect::<Vec<_>>();

    let mut table = active_table.unwrap_or("root").to_owned();
    for earlier in preceding {
        let resolution = resolve_key(&table, earlier, &prefixes, tables);
        if let Resolution::TableUnavailable { table, message } = resolution {
            details.push(format!("could not read tmux table {table}: {message}"));
            return LayerResult::new(
                "tmux",
                LayerId::Multiplexer,
                Outcome::Unknown,
                "tmux key-table state for this sequence could not be inspected",
                details,
            );
        }
        let next = resolution.next_table();
        details.push(format!(
            "sequence: {earlier} in table {table} -> table {next}"
        ));
        table = next;
    }

    match resolve_key(&table, key, &prefixes, tables) {
        Resolution::TableUnavailable { table, message } if table == "root" => unavailable(message),
        Resolution::TableUnavailable { table, message } => {
            details.push(format!("could not read tmux table {table}: {message}"));
            LayerResult::new(
                "tmux",
                LayerId::Multiplexer,
                Outcome::Unknown,
                format!("tmux {table} table could not be inspected"),
                details,
            )
        }
        Resolution::PrefixEntered => {
            details.push("the configured prefix switches the client to the prefix table".into());
            let (outcome, summary) = if bytes_verified {
                (
                    Outcome::Consumed,
                    "tmux prefix key; tmux waits for the next key",
                )
            } else {
                (
                    Outcome::HandledUncertain,
                    "tmux prefix key; terminal byte delivery is unverified",
                )
            };
            LayerResult::new("tmux", LayerId::Multiplexer, outcome, summary, details)
        }
        Resolution::Bound { table, binding } => {
            details.push(format!("table: {table}"));
            details.push(format!("binding: {}", binding.description));
            result_for_binding(&binding.action, &table, details, bytes_verified)
        }
        Resolution::Discarded { table } => {
            details.push(format!(
                "no binding in tmux {table} table or root; tmux returns to root and drops the key"
            ));
            let (outcome, summary) = if bytes_verified {
                (
                    Outcome::Consumed,
                    format!("no binding in the tmux {table} table; tmux discards the key"),
                )
            } else {
                (
                    Outcome::HandledUncertain,
                    format!(
                        "no binding in the tmux {table} table; terminal byte delivery is unverified"
                    ),
                )
            };
            LayerResult::new("tmux", LayerId::Multiplexer, outcome, summary, details)
        }
        Resolution::Forwarded => LayerResult::new(
            "tmux",
            LayerId::Multiplexer,
            Outcome::Pass,
            if bytes_verified {
                format!("no binding in the tmux {table} table; forwarded to the pane")
            } else {
                format!(
                    "no binding in the tmux {table} table; terminal byte delivery remains unverified"
                )
            },
            details,
        ),
    }
}

enum Resolution {
    /// The key is a configured prefix; the client enters the prefix table.
    PrefixEntered,
    Bound {
        table: String,
        binding: Binding,
    },
    /// No binding in a non-root table nor in root: tmux drops the key.
    Discarded {
        table: String,
    },
    /// No binding in the root table: the key goes to the pane.
    Forwarded,
    TableUnavailable {
        table: String,
        message: String,
    },
}

impl Resolution {
    /// The client key table after this key was handled.
    fn next_table(&self) -> String {
        match self {
            Self::PrefixEntered => "prefix".into(),
            Self::Bound { binding, .. } => switch_client_table(&binding.action)
                .unwrap_or("root")
                .to_owned(),
            Self::Discarded { .. } | Self::Forwarded | Self::TableUnavailable { .. } => {
                "root".into()
            }
        }
    }
}

fn resolve_key(
    table: &str,
    key: &KeyCombo,
    prefixes: &[KeyCombo],
    tables: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
) -> Resolution {
    // The prefix takes precedence over every table except the prefix table.
    if table != "prefix" && prefixes.contains(key) {
        return Resolution::PrefixEntered;
    }
    let output = match tables(table) {
        Ok(output) => output,
        Err(message) => {
            return Resolution::TableUnavailable {
                table: table.to_owned(),
                message,
            };
        }
    };
    if let Some(binding) = find_binding(&output, key, table) {
        return Resolution::Bound {
            table: table.to_owned(),
            binding,
        };
    }
    if table == "root" {
        return Resolution::Forwarded;
    }
    // tmux retries an unbound key from another table in root, then drops it.
    match resolve_key("root", key, prefixes, tables) {
        Resolution::Forwarded => Resolution::Discarded {
            table: table.to_owned(),
        },
        resolution => resolution,
    }
}

/// Target of the last `switch-client -T NAME` in a command list (tmux prints
/// chained commands separated by `\;`), the action that enters a custom table.
fn switch_client_table(action: &str) -> Option<&str> {
    action
        .split(" \\; ")
        .flat_map(|command| command.split("; "))
        .filter_map(|command| {
            let command = command.trim().trim_end_matches([';', '\\']);
            let mut tokens = command.split_whitespace();
            if !matches!(tokens.next(), Some("switch-client" | "switchc")) {
                return None;
            }
            tokens.skip_while(|token| *token != "-T").nth(1)
        })
        .last()
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

fn result_for_binding(
    action: &str,
    table: &str,
    mut details: Vec<String>,
    bytes_verified: bool,
) -> LayerResult {
    let binding = BindingEvidence {
        dispatcher: None,
        action: Some(action.to_owned()),
        description: None,
        submap: (table != "root").then(|| table.to_owned()),
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
        return LayerResult::new(
            "tmux",
            LayerId::Multiplexer,
            Outcome::HandledUncertain,
            format!("candidate binding matches in tmux {table} table; delivery is unverified"),
            details,
        )
        .with_binding(binding);
    }
    if action == "send-keys" || action.starts_with("send-keys ") {
        details.push("tmux emits another key sequence downstream".into());
        return LayerResult::new(
            "tmux",
            LayerId::Multiplexer,
            Outcome::HandledAndPassed,
            format!("binding in tmux {table} table remaps the key and forwards generated input"),
            details,
        )
        .with_binding(binding);
    }
    LayerResult::new(
        "tmux",
        LayerId::Multiplexer,
        Outcome::Consumed,
        format!("binding in tmux {table} table consumes the key"),
        details,
    )
    .with_binding(binding)
}

fn unavailable(message: String) -> LayerResult {
    LayerResult::new(
        "tmux",
        LayerId::Multiplexer,
        Outcome::Unavailable,
        "could not inspect tmux key bindings",
        vec![if message.is_empty() {
            "tmux did not return an error message".into()
        } else {
            format!("tmux: {message}")
        }],
    )
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

    fn fake_tables(table: &str) -> Result<Vec<u8>, String> {
        Ok(match table {
            "root" => b"bind-key -T root M-r switch-client -T resize\n".to_vec(),
            "prefix" => b"bind-key -T prefix C-Left select-pane -L\nbind-key -T prefix x select-pane -L \\; switch-client -T resize\n".to_vec(),
            "resize" => b"bind-key -T resize C-Left resize-pane -L 5\nbind-key -T resize h resize-pane -L \\; switch-client -T resize\n".to_vec(),
            _ => Vec::new(),
        })
    }

    fn run(preceding: &[&str], key: &str) -> LayerResult {
        let preceding = preceding
            .iter()
            .map(|key| key.parse().unwrap())
            .collect::<Vec<KeyCombo>>();
        inspect_with_tables(
            Some("root"),
            &["C-b".to_owned()],
            &preceding,
            &key.parse().unwrap(),
            true,
            &mut fake_tables,
        )
    }

    #[test]
    fn bare_key_bound_only_in_prefix_table_is_not_handled() {
        let result = run(&[], "ctrl+left");
        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("root table"), "{}", result.summary);
    }

    #[test]
    fn bare_prefix_key_is_reported_as_the_tmux_prefix() {
        let result = run(&[], "ctrl+b");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.summary.starts_with("tmux prefix key"));
    }

    #[test]
    fn list_keys_answers_are_live_even_with_the_table_assumed() {
        assert_eq!(run(&[], "ctrl+b").tier, Some(EvidenceTier::LiveQuery));
        let assumed = inspect_with_tables(
            None,
            &[],
            &[],
            &"ctrl+left".parse().unwrap(),
            true,
            &mut fake_tables,
        );
        assert_eq!(assumed.tier, Some(EvidenceTier::LiveQuery));
        let unreachable = inspect_with_tables(
            Some("root"),
            &[],
            &[],
            &"ctrl+left".parse().unwrap(),
            true,
            &mut |_| Err("no server running".into()),
        );
        assert_eq!(unreachable.outcome, Outcome::Unavailable);
        assert_eq!(unreachable.tier, None);
    }

    #[test]
    fn prefix_table_binding_matches_after_the_prefix() {
        let result = run(&["ctrl+b"], "ctrl+left");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            result.summary,
            "binding in tmux prefix table consumes the key"
        );
        assert_eq!(
            result.binding.and_then(|binding| binding.action).as_deref(),
            Some("select-pane -L")
        );
    }

    #[test]
    fn unbound_key_after_the_prefix_is_discarded() {
        let result = run(&["ctrl+b"], "ctrl+q");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.summary.contains("tmux prefix table"));
        assert!(result.summary.contains("discards"));
    }

    #[test]
    fn switch_client_enters_the_named_table_for_the_next_key() {
        let result = run(&["alt+r"], "ctrl+left");
        assert_eq!(
            result.summary,
            "binding in tmux resize table consumes the key"
        );
    }

    #[test]
    fn chained_switch_client_keeps_the_sticky_table() {
        assert_eq!(
            switch_client_table("select-pane -L \\; switch-client -T resize"),
            Some("resize")
        );
        assert_eq!(
            switch_client_table("switch-client -T a; switch-client -T b"),
            Some("b")
        );
        let result = run(&["ctrl+b", "x", "h", "h"], "ctrl+left");
        assert_eq!(
            result.summary,
            "binding in tmux resize table consumes the key"
        );
        let result = run(&["ctrl+b", "x", "h"], "h");
        assert_eq!(
            result.binding.and_then(|binding| binding.action).as_deref(),
            Some("resize-pane -L \\; switch-client -T resize")
        );
    }
}
