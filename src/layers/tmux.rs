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
    if table != "prefix" && prefixes.contains(&tmux_query_key(key)) {
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

/// Finds the binding for `key` in `list-keys` output. tmux prints each line
/// as `bind-key [-r] [-N note] -T TABLE KEY COMMAND...`: the key is the token
/// right after the table name, quoted (`"M-{"`) or backslash-escaped (`\%`,
/// `C-\\`) when it holds a character special to tmux's parser.
fn find_binding(output: &[u8], key: &KeyCombo, expected_table: &str) -> Option<Binding> {
    let text = std::str::from_utf8(output).ok()?;
    let wanted = tmux_query_key(key);
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("bind-key")?;
        let after_table_flag = rest[rest.find(" -T ")? + 4..].trim_start();
        let table_end = after_table_flag
            .find(char::is_whitespace)
            .unwrap_or(after_table_flag.len());
        if &after_table_flag[..table_end] != expected_table {
            return None;
        }
        let (raw_key, action) = split_key_token(after_table_flag[table_end..].trim_start())?;
        if parse_tmux_key(&raw_key)? != wanted {
            return None;
        }
        Some(Binding {
            action: action.split_whitespace().collect::<Vec<_>>().join(" "),
            description: line.to_owned(),
        })
    })
}

/// Reads one tmux key token: `"..."` quoted, or a bare token whose `\x`
/// escapes stand for `x`. Returns the unescaped key and the remaining text.
fn split_key_token(text: &str) -> Option<(String, &str)> {
    let mut key = String::new();
    let mut chars = text.char_indices();
    if text.starts_with('"') {
        chars.next();
        while let Some((index, character)) = chars.next() {
            match character {
                '\\' => key.push(chars.next()?.1),
                '"' => return Some((key, &text[index + 1..])),
                other => key.push(other),
            }
        }
        return None;
    }
    while let Some((index, character)) = chars.next() {
        match character {
            '\\' => key.push(chars.next()?.1),
            character if character.is_whitespace() => return Some((key, &text[index..])),
            other => key.push(other),
        }
    }
    (!key.is_empty()).then_some((key, ""))
}

/// The tmux key a queried combination arrives as. A shifted punctuation
/// character already names the shifted symbol (`shift+%` is `%`), while
/// Shift on a letter selects the uppercase key, as tmux's case-sensitive
/// `C` versus `c` does.
fn tmux_query_key(key: &KeyCombo) -> KeyCombo {
    let mut characters = key.key().chars();
    match (characters.next(), characters.next()) {
        (Some(character), None)
            if key.modmask() & SHIFT != 0 && character.is_ascii_punctuation() =>
        {
            KeyCombo::from_parts(key.modmask() & !SHIFT, key.key())
        }
        _ => key.clone(),
    }
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

const SHIFT: u32 = 1;
const CTRL: u32 = 4;
const ALT: u32 = 8;

/// Parses an unescaped tmux key name into the combination it matches. Single
/// characters are case-sensitive in tmux (`C` is Shift+c), so an uppercase
/// letter carries Shift; `BTab` is Shift+Tab.
fn parse_tmux_key(raw: &str) -> Option<KeyCombo> {
    let mut modifiers = 0;
    let mut key = raw;
    loop {
        if key.len() > 1 {
            if let Some(rest) = key.strip_prefix('^') {
                modifiers |= CTRL;
                key = rest;
                continue;
            }
        }
        let Some((prefix, rest)) = key.split_once('-') else {
            break;
        };
        if rest.is_empty() {
            break;
        }
        match prefix {
            "C" | "c" => modifiers |= CTRL,
            "M" | "m" => modifiers |= ALT,
            "S" | "s" => modifiers |= SHIFT,
            _ => break,
        }
        key = rest;
    }
    let mut characters = key.chars();
    if let (Some(character), None) = (characters.next(), characters.next()) {
        if character.is_ascii_uppercase() {
            modifiers |= SHIFT;
        }
        return Some(KeyCombo::from_parts(modifiers, key));
    }
    let normalized = match key.to_ascii_lowercase().as_str() {
        "left" => "LEFT",
        "right" => "RIGHT",
        "up" => "UP",
        "down" => "DOWN",
        "home" => "HOME",
        "end" => "END",
        "npage" | "pagedown" | "pgdn" => "PAGE_DOWN",
        "ppage" | "pageup" | "pgup" => "PAGE_UP",
        "ic" | "insert" => "INSERT",
        "dc" | "delete" => "DELETE",
        "escape" => "ESCAPE",
        "bspace" => "BACKSPACE",
        "tab" => "TAB",
        "btab" => {
            modifiers |= SHIFT;
            "TAB"
        }
        "space" => "SPACE",
        "enter" => "RETURN",
        "kpenter" => "KP_ENTER",
        "kp/" => "KP_DIVIDE",
        "kp*" => "KP_MULTIPLY",
        "kp-" => "KP_SUBTRACT",
        "kp+" => "KP_ADD",
        "kp." => "KP_DECIMAL",
        value
            if value.len() == 3
                && value.starts_with("kp")
                && value.as_bytes()[2].is_ascii_digit() =>
        {
            return Some(KeyCombo::from_parts(
                modifiers,
                format!("KP_{}", &value[2..]),
            ));
        }
        value if value.starts_with('f') && value[1..].parse::<u8>().is_ok() => {
            return Some(KeyCombo::from_parts(modifiers, value.to_ascii_uppercase()));
        }
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

    /// Verbatim `tmux 3.7c list-keys` lines (uppercase before lowercase,
    /// escaped and quoted keys, `-r` flags before `-T`).
    const LIST_KEYS: &[u8] = br##"bind-key    -T prefix \"      split-window
bind-key    -T prefix \#      list-buffers
bind-key    -T prefix \$      command-prompt -I "#S" { rename-session "%%" }
bind-key    -T prefix \%      split-window -h
bind-key    -T prefix \'      command-prompt -T window-target -p index { select-window -t ":%%" }
bind-key    -T prefix -       delete-buffer
bind-key    -T prefix \;      last-pane
bind-key    -T prefix C       new-session -c "#{pane_current_path}"
bind-key    -T prefix D       choose-client -Z
bind-key    -T prefix c       new-window -c "#{pane_current_path}"
bind-key    -T prefix d       detach-client
bind-key    -T prefix \{      swap-pane -U
bind-key    -T prefix \}      swap-pane -D
bind-key    -T prefix \~      show-messages
bind-key -r -T prefix DC      refresh-client -c
bind-key    -T prefix C-z     suspend-client
bind-key  -T root M-Escape                  kill-pane
bind-key  -T root "M-{"                     display-message x
bind-key  -T root C-\\                      display-message q
bind-key  -T root BSpace                    display-message z
bind-key  -T root BTab                      display-message y
bind-key  -T root M-S-a                     display-message a
bind-key  -T root M--                       display-message m
"##;

    fn action(table: &str, key: &str) -> Option<String> {
        let key = KeyCombo::parse_query(key).unwrap();
        find_binding(LIST_KEYS, &key, table).map(|binding| binding.action)
    }

    #[test]
    fn tmux_keys_are_case_sensitive() {
        assert_eq!(
            action("prefix", "c").as_deref(),
            Some("new-window -c \"#{pane_current_path}\"")
        );
        assert_eq!(
            action("prefix", "shift+c").as_deref(),
            Some("new-session -c \"#{pane_current_path}\"")
        );
        assert_eq!(action("prefix", "d").as_deref(), Some("detach-client"));
        assert_eq!(
            action("prefix", "shift+d").as_deref(),
            Some("choose-client -Z")
        );
        assert_eq!(
            action("root", "alt+shift+a").as_deref(),
            Some("display-message a")
        );
        assert_eq!(action("root", "alt+a"), None);
    }

    #[test]
    fn escaped_quoted_and_dash_keys_match() {
        for (key, expected) in [
            ("percent", "split-window -h"),
            ("shift+percent", "split-window -h"),
            ("quotedbl", "split-window"),
            ("numbersign", "list-buffers"),
            (
                "dollar",
                "command-prompt -I \"#S\" { rename-session \"%%\" }",
            ),
            (
                "apostrophe",
                "command-prompt -T window-target -p index { select-window -t \":%%\" }",
            ),
            ("semicolon", "last-pane"),
            ("minus", "delete-buffer"),
            ("braceleft", "swap-pane -U"),
            ("braceright", "swap-pane -D"),
            ("asciitilde", "show-messages"),
            ("delete", "refresh-client -c"),
            ("ctrl+z", "suspend-client"),
        ] {
            assert_eq!(action("prefix", key).as_deref(), Some(expected), "{key}");
        }
        assert_eq!(
            action("root", "alt+braceleft").as_deref(),
            Some("display-message x")
        );
        assert_eq!(
            action("root", "ctrl+backslash").as_deref(),
            Some("display-message q")
        );
        assert_eq!(
            action("root", "alt+minus").as_deref(),
            Some("display-message m")
        );
    }

    #[test]
    fn named_tmux_keys_match() {
        assert_eq!(action("root", "alt+escape").as_deref(), Some("kill-pane"));
        assert_eq!(
            action("root", "backspace").as_deref(),
            Some("display-message z")
        );
        assert_eq!(
            action("root", "shift+tab").as_deref(),
            Some("display-message y")
        );
        assert_eq!(action("root", "tab"), None);
        assert_eq!(
            parse_tmux_key("^B").unwrap().to_string(),
            "CTRL + SHIFT + B"
        );
        assert_eq!(parse_tmux_key("^b").unwrap().to_string(), "CTRL + B");
        for (tmux, query) in [
            ("KPEnter", "kp_enter"),
            ("KP1", "kp_1"),
            ("KP/", "kp_divide"),
        ] {
            assert_eq!(
                parse_tmux_key(tmux),
                Some(KeyCombo::parse_query(query).unwrap()),
                "{tmux}"
            );
        }
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
