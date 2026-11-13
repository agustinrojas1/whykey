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
    inspect_in(&TmuxContext::from_process(), preceding, key, bytes_verified)
}

/// Which tmux server and pane an inspection speaks to. whykey's own session
/// (the `$TMUX` it inherited and `$TMUX_PANE`) is the default; `--pid` and
/// `--focused` name another process, whose server and pane are these.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmuxContext {
    /// `-S path` or `-L name`; empty means the server `$TMUX` names.
    pub server_args: Vec<String>,
    pub pane: Option<String>,
}

impl TmuxContext {
    pub fn from_process() -> Self {
        Self {
            server_args: Vec::new(),
            pane: crate::recording::env_var_os("TMUX_PANE")
                .map(|pane| pane.to_string_lossy().into_owned()),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(&self.server_args);
        command
    }
}

/// [`inspect_with_byte_status`] for an explicit tmux server and pane.
pub fn inspect_in(
    context: &TmuxContext,
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
) -> LayerResult {
    let pane = tmux_pane_state(context);
    let prefixes = tmux_prefixes(context);
    let mut details = Vec::new();
    if let Some(pane) = &context.pane {
        details.push(format!("client pane: {pane}"));
    }
    let mut cache = HashMap::new();
    let mut tables = |table: &str| {
        cache
            .entry(table.to_owned())
            .or_insert_with(|| list_keys(context, table))
            .clone()
    };
    let mut result = inspect_with_tables(
        &pane,
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
    pane: &PaneState,
    raw_prefixes: &[String],
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
    tables: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
) -> LayerResult {
    let result = resolve_with_tables(pane, raw_prefixes, preceding, key, bytes_verified, tables);
    if result.outcome == Outcome::Unavailable {
        result
    } else {
        result.with_tier(EvidenceTier::LiveQuery)
    }
}

/// Mirrors tmux's client key handling: the prefix switches to the prefix
/// table, a binding runs and returns the client to root (or the table named
/// by `switch-client -T`), and an unbound key in a non-root table is retried
/// in root and otherwise discarded. A pane in copy mode takes the keys of the
/// client's root state through its own table (`copy-mode` or `copy-mode-vi`)
/// instead of root, so that table is where a key starts.
fn resolve_with_tables(
    pane: &PaneState,
    raw_prefixes: &[String],
    preceding: &[KeyCombo],
    key: &KeyCombo,
    bytes_verified: bool,
    tables: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>,
) -> LayerResult {
    let mut details = Vec::new();
    match pane.client_table.as_deref() {
        Some(table) => details.push(format!("active client key table: {table}")),
        None => details.push("active client key table: unknown; assuming root".into()),
    }
    let mut mode = pane.mode.clone();
    match &mode {
        PaneMode::Normal => {}
        PaneMode::Copy { name, table } => details.push(format!(
            "pane mode: {name}; keys the client's root table would take go to the {table} table"
        )),
        PaneMode::Unmodelled { name } => details.push(format!(
            "pane mode: {name}; whykey does not model how tmux routes keys in this mode"
        )),
    }
    for prefix in raw_prefixes {
        details.push(format!("prefix: {prefix}"));
    }
    let prefixes = raw_prefixes
        .iter()
        .filter_map(|prefix| parse_tmux_key(prefix))
        .collect::<Vec<_>>();

    // The mode table applies where the client is in its default (root) table;
    // a pending prefix or `switch-client -T` table keeps priority.
    let mut table = match pane.client_table.as_deref() {
        Some(table) if table != "root" => table.to_owned(),
        _ => mode.home_table().to_owned(),
    };
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
        if let Resolution::Bound { binding, .. } = &resolution {
            let after = mode.after(&binding.action, pane.copy_table);
            if after != mode {
                details.push(format!(
                    "sequence: {earlier} changes the pane mode to {}",
                    after.name()
                ));
                mode = after;
            }
        }
        let next = resolution.next_table(mode.home_table());
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
            if matches!(mode, PaneMode::Copy { .. }) && table == mode.home_table() {
                details.push(format!(
                    "no binding in tmux {table} table or root; the pane is in {}, so tmux does not pass the key to the application",
                    mode.name()
                ));
            } else {
                details.push(format!(
                    "no binding in tmux {table} table or root; tmux returns to root and drops the key"
                ));
            }
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
        Resolution::Forwarded => {
            if let PaneMode::Unmodelled { name } = &mode {
                details.push(format!(
                    "no binding in the tmux {table} table, but the pane is in {name}, which takes keys itself"
                ));
                return LayerResult::new(
                    "tmux",
                    LayerId::Multiplexer,
                    Outcome::Unknown,
                    format!("pane is in tmux {name}; whether it takes the key is not modelled"),
                    details,
                );
            }
            LayerResult::new(
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
            )
        }
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
    /// The key table the next key starts in after this one was handled:
    /// `home` (root, or the copy-mode table of a pane in copy mode) unless
    /// the key was the prefix or a `switch-client -T` binding.
    fn next_table(&self, home: &str) -> String {
        match self {
            Self::PrefixEntered => "prefix".into(),
            Self::Bound { binding, .. } => switch_client_table(&binding.action)
                .unwrap_or(home)
                .to_owned(),
            Self::Discarded { .. } | Self::Forwarded | Self::TableUnavailable { .. } => home.into(),
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

fn list_keys(context: &TmuxContext, table: &str) -> Result<Vec<u8>, String> {
    let mut command = context.command();
    command.args(["list-keys", "-a", "-T", table]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

/// What tmux reports for the pane whykey inspects (`$TMUX_PANE`, else the
/// current client's pane), from one `display-message` call.
#[derive(Debug, Default)]
struct PaneState {
    client_table: Option<String>,
    mode: PaneMode,
    /// The copy-mode table the window's `mode-keys` selects, when known.
    copy_table: Option<&'static str>,
}

/// The pane's mode decides which table receives the keys of the client's
/// default state. tmux uses `copy-mode`, or `copy-mode-vi` under
/// `mode-keys vi`, for a pane in copy mode or view mode; the other modes
/// (tree, client, buffer, customize) take keys themselves.
#[derive(Debug, Default, Clone, PartialEq)]
enum PaneMode {
    #[default]
    Normal,
    Copy {
        name: String,
        table: &'static str,
    },
    Unmodelled {
        name: String,
    },
}

impl PaneMode {
    fn home_table(&self) -> &str {
        match self {
            Self::Copy { table, .. } => table,
            Self::Normal | Self::Unmodelled { .. } => "root",
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Normal => "no mode",
            Self::Copy { name, .. } | Self::Unmodelled { name } => name,
        }
    }

    /// The mode after a binding with `action` ran: `copy-mode` enters copy
    /// mode, a copy-mode command that cancels (`send-keys -X cancel`,
    /// `copy-pipe-and-cancel`, ...) leaves it, and the `choose-*` family
    /// opens a mode whose keys are not modelled.
    fn after(&self, action: &str, copy_table: Option<&'static str>) -> Self {
        let mut mode = self.clone();
        for command in action
            .split(" \\; ")
            .flat_map(|command| command.split("; "))
        {
            let tokens = command
                .trim()
                .trim_end_matches([';', '\\'])
                .split_whitespace()
                .collect::<Vec<_>>();
            match tokens.first().copied() {
                Some("copy-mode") => {
                    mode = match copy_table {
                        Some(table) => Self::Copy {
                            name: "copy-mode".into(),
                            table,
                        },
                        None => Self::Unmodelled {
                            name: "copy-mode (mode-keys could not be read)".into(),
                        },
                    };
                }
                Some("choose-tree" | "choose-buffer" | "choose-client" | "customize-mode") => {
                    mode = Self::Unmodelled {
                        name: format!("{} mode", tokens[0]),
                    };
                }
                Some("send-keys" | "send")
                    if is_copy_mode_command(&tokens)
                        && matches!(mode, Self::Copy { .. })
                        && tokens.iter().any(|token| token.ends_with("cancel")) =>
                {
                    mode = Self::Normal;
                }
                _ => {}
            }
        }
        mode
    }
}

/// `send-keys -X` runs a copy-mode command instead of sending keys.
fn is_copy_mode_command(tokens: &[&str]) -> bool {
    tokens.iter().skip(1).any(|token| {
        token.len() > 1
            && token.starts_with('-')
            && token[1..].chars().all(|c| c.is_ascii_alphabetic())
            && token.contains('X')
    })
}

const PANE_FORMAT: &str = "#{client_key_table}\t#{pane_in_mode}\t#{pane_mode}\t#{mode-keys}";

fn tmux_pane_state(context: &TmuxContext) -> PaneState {
    let mut command = context.command();
    if let Some(pane) = &context.pane {
        command.args(["display-message", "-p", "-t", pane, PANE_FORMAT]);
    } else {
        command.args(["display-message", "-p", PANE_FORMAT]);
    }
    let Ok(output) = command::output(&mut command) else {
        return PaneState::default();
    };
    if !output.status.success() {
        return PaneState::default();
    }
    parse_pane_state(&String::from_utf8_lossy(&output.stdout))
}

/// Parses [`PANE_FORMAT`] output. An older tmux expands the formats it lacks
/// (`pane_mode`, `mode-keys`) to nothing, so a pane reported in a mode whose
/// name or `mode-keys` is missing is [`PaneMode::Unmodelled`], never guessed.
fn parse_pane_state(output: &str) -> PaneState {
    let mut fields = output.trim_end_matches(['\n', '\r']).split('\t');
    let client_table = fields
        .next()
        .map(str::trim)
        .filter(|table| !table.is_empty())
        .map(str::to_owned);
    let in_mode = fields.next().map(str::trim) == Some("1");
    let pane_mode = fields.next().map(str::trim).unwrap_or("");
    let copy_table = match fields.next().map(str::trim) {
        Some("vi") => Some("copy-mode-vi"),
        Some("emacs") => Some("copy-mode"),
        _ => None,
    };
    let mode = if !in_mode {
        PaneMode::Normal
    } else {
        match (pane_mode, copy_table) {
            ("copy-mode" | "view-mode", Some(table)) => PaneMode::Copy {
                name: pane_mode.to_owned(),
                table,
            },
            ("copy-mode" | "view-mode", None) => PaneMode::Unmodelled {
                name: format!("{pane_mode} (mode-keys could not be read)"),
            },
            ("", _) => PaneMode::Unmodelled {
                name: "a pane mode tmux did not name".into(),
            },
            (other, _) => PaneMode::Unmodelled {
                name: other.to_owned(),
            },
        }
    };
    PaneState {
        client_table,
        mode,
        copy_table,
    }
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
    let is_send_keys = action == "send-keys" || action.starts_with("send-keys ");
    if is_send_keys && !is_copy_mode_command(&action.split_whitespace().collect::<Vec<_>>()) {
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

fn tmux_prefixes(context: &TmuxContext) -> Vec<String> {
    ["prefix", "prefix2"]
        .into_iter()
        .filter_map(|option| {
            let mut command = context.command();
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

    fn client_in(table: &str) -> PaneState {
        PaneState {
            client_table: Some(table.to_owned()),
            ..PaneState::default()
        }
    }

    fn run(preceding: &[&str], key: &str) -> LayerResult {
        let preceding = preceding
            .iter()
            .map(|key| key.parse().unwrap())
            .collect::<Vec<KeyCombo>>();
        inspect_with_tables(
            &client_in("root"),
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
            &PaneState::default(),
            &[],
            &[],
            &"ctrl+left".parse().unwrap(),
            true,
            &mut fake_tables,
        );
        assert_eq!(assumed.tier, Some(EvidenceTier::LiveQuery));
        let unreachable = inspect_with_tables(
            &client_in("root"),
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

    /// `list-keys -a -T` lines in the shape tmux 3.x prints for the default
    /// copy-mode tables, plus a root and prefix table with a `copy-mode`
    /// entry and a binding in root only.
    fn mode_tables(table: &str) -> Result<Vec<u8>, String> {
        Ok(match table {
            "root" => b"bind-key -T root C-t display-message root-bound\n".to_vec(),
            "prefix" => b"bind-key -T prefix [ copy-mode\nbind-key -T prefix t choose-tree -Zs\n".to_vec(),
            "copy-mode" => b"bind-key -T copy-mode C-c send-keys -X cancel\nbind-key -T copy-mode q send-keys -X cancel\nbind-key -T copy-mode Space send-keys -X begin-selection\n".to_vec(),
            "copy-mode-vi" => b"bind-key -T copy-mode-vi q send-keys -X cancel\nbind-key -T copy-mode-vi h send-keys -X cursor-left\nbind-key -T copy-mode-vi Enter send-keys -X copy-pipe-and-cancel\n".to_vec(),
            _ => Vec::new(),
        })
    }

    fn in_pane(state: &str, preceding: &[&str], key: &str) -> LayerResult {
        let preceding = preceding
            .iter()
            .map(|key| key.parse().unwrap())
            .collect::<Vec<KeyCombo>>();
        inspect_with_tables(
            &parse_pane_state(state),
            &["C-b".to_owned()],
            &preceding,
            &key.parse().unwrap(),
            true,
            &mut mode_tables,
        )
    }

    const VI_COPY: &str = "root\t1\tcopy-mode\tvi\n";
    const EMACS_COPY: &str = "root\t1\tcopy-mode\temacs\n";

    fn action_of(result: &LayerResult) -> Option<String> {
        result
            .binding
            .as_ref()
            .and_then(|binding| binding.action.clone())
    }

    #[test]
    fn pane_state_is_read_from_one_format_line() {
        let normal = parse_pane_state("root\t0\t\temacs\n");
        assert_eq!(normal.client_table.as_deref(), Some("root"));
        assert_eq!(normal.mode, PaneMode::Normal);
        assert_eq!(normal.copy_table, Some("copy-mode"));

        let vi = parse_pane_state(VI_COPY);
        assert_eq!(
            vi.mode,
            PaneMode::Copy {
                name: "copy-mode".into(),
                table: "copy-mode-vi"
            }
        );
        let emacs = parse_pane_state(EMACS_COPY);
        assert_eq!(emacs.mode.home_table(), "copy-mode");
        let view = parse_pane_state("root\t1\tview-mode\tvi\n");
        assert_eq!(view.mode.home_table(), "copy-mode-vi");

        let pending_prefix = parse_pane_state("prefix\t1\tcopy-mode\tvi\n");
        assert_eq!(pending_prefix.client_table.as_deref(), Some("prefix"));

        let no_client = parse_pane_state("\t0\t\temacs\n");
        assert_eq!(no_client.client_table, None);
    }

    #[test]
    fn pane_state_degrades_on_a_tmux_without_the_mode_formats() {
        // tmux before pane_mode / mode-keys expands the missing formats to
        // nothing: not in a mode reads as normal, and an unnamed mode is
        // never guessed to be copy mode.
        assert_eq!(parse_pane_state("root\t0\t\t\n").mode, PaneMode::Normal);
        assert_eq!(parse_pane_state("root\n").mode, PaneMode::Normal);
        assert!(matches!(
            parse_pane_state("root\t1\t\t\n").mode,
            PaneMode::Unmodelled { .. }
        ));
        let unreadable = parse_pane_state("root\t1\tcopy-mode\t\n");
        assert!(
            matches!(&unreadable.mode, PaneMode::Unmodelled { name } if name.contains("mode-keys"))
        );
        assert_eq!(
            parse_pane_state("root\t1\ttree-mode\temacs\n").mode,
            PaneMode::Unmodelled {
                name: "tree-mode".into()
            }
        );
    }

    #[test]
    fn vi_copy_mode_resolves_keys_in_the_vi_table() {
        let result = in_pane(VI_COPY, &[], "h");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            result.summary,
            "binding in tmux copy-mode-vi table consumes the key"
        );
        assert_eq!(
            action_of(&result).as_deref(),
            Some("send-keys -X cursor-left")
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.starts_with("pane mode: copy-mode;")),
            "{:?}",
            result.details
        );
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
    }

    #[test]
    fn copy_mode_commands_are_not_generated_input() {
        // `send-keys -X` runs a copy-mode command; it sends nothing downstream.
        let result = in_pane(EMACS_COPY, &[], "q");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(action_of(&result).as_deref(), Some("send-keys -X cancel"));
        assert!(!is_copy_mode_command(&["send-keys", "C-x"]));
        assert!(!is_copy_mode_command(&["send-keys", "-l", "X"]));
        assert!(is_copy_mode_command(&["send-keys", "-X", "cancel"]));
        assert!(is_copy_mode_command(&[
            "send-keys",
            "-N",
            "3",
            "-X",
            "cursor-down"
        ]));
    }

    #[test]
    fn key_bound_only_in_the_other_copy_table_is_dropped() {
        // `h` is bound in copy-mode-vi but not in the emacs table.
        let result = in_pane(EMACS_COPY, &[], "h");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result.summary.contains("copy-mode table"),
            "{}",
            result.summary
        );
        assert!(result.summary.contains("discards"), "{}", result.summary);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("does not pass the key to the application")),
            "{:?}",
            result.details
        );
        // Outside copy mode the same key reaches the pane.
        let normal = in_pane("root\t0\t\temacs\n", &[], "h");
        assert_eq!(normal.outcome, Outcome::Pass);
    }

    #[test]
    fn root_bindings_still_apply_in_copy_mode() {
        let result = in_pane(VI_COPY, &[], "ctrl+t");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            result.summary,
            "binding in tmux root table consumes the key"
        );
    }

    #[test]
    fn the_prefix_still_wins_in_copy_mode() {
        let result = in_pane(VI_COPY, &[], "ctrl+b");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.summary.starts_with("tmux prefix key"));
        // After it, the prefix table, not the copy-mode table, takes the key.
        let after = in_pane(VI_COPY, &["ctrl+b"], "q");
        assert!(
            after.summary.contains("tmux prefix table"),
            "{}",
            after.summary
        );
        assert!(after.summary.contains("discards"), "{}", after.summary);
    }

    #[test]
    fn a_pending_prefix_in_the_client_table_keeps_priority_over_the_mode() {
        let result = in_pane("prefix\t1\tcopy-mode\tvi\n", &[], "h");
        assert!(
            result.summary.contains("tmux prefix table"),
            "{}",
            result.summary
        );
    }

    #[test]
    fn entering_copy_mode_with_the_prefix_sends_the_next_key_to_the_mode_table() {
        let result = in_pane("root\t0\t\tvi\n", &["ctrl+b", "["], "h");
        assert_eq!(
            result.summary,
            "binding in tmux copy-mode-vi table consumes the key"
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("changes the pane mode to copy-mode")),
            "{:?}",
            result.details
        );
        // Without the mode, the same bare key is forwarded.
        assert_eq!(in_pane("root\t0\t\tvi\n", &[], "h").outcome, Outcome::Pass);
    }

    #[test]
    fn cancelling_copy_mode_returns_following_keys_to_root() {
        let result = in_pane(VI_COPY, &["q"], "h");
        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("root table"), "{}", result.summary);
        let still_in_mode = in_pane(VI_COPY, &["h"], "h");
        assert!(still_in_mode.summary.contains("copy-mode-vi table"));
        let pipe_and_cancel = in_pane(VI_COPY, &["enter"], "h");
        assert_eq!(pipe_and_cancel.outcome, Outcome::Pass);
    }

    #[test]
    fn a_mode_that_whykey_cannot_model_is_reported_uncertain() {
        let unbound = in_pane("root\t1\ttree-mode\temacs\n", &[], "h");
        assert_eq!(unbound.outcome, Outcome::Unknown);
        assert!(unbound.summary.contains("tree-mode"), "{}", unbound.summary);
        // A root binding still runs in front of the mode.
        let bound = in_pane("root\t1\ttree-mode\temacs\n", &[], "ctrl+t");
        assert_eq!(bound.outcome, Outcome::Consumed);
        // A pane in a mode tmux did not name is not assumed to be normal.
        let unnamed = in_pane("root\t1\t\t\n", &[], "h");
        assert_eq!(unnamed.outcome, Outcome::Unknown);
        // The prefix menu opening choose-tree makes the next key uncertain.
        let after_choose = in_pane("root\t0\t\temacs\n", &["ctrl+b", "t"], "h");
        assert_eq!(after_choose.outcome, Outcome::Unknown);
    }

    #[test]
    fn the_copy_mode_table_being_unreadable_is_not_a_confident_answer() {
        let result = inspect_with_tables(
            &parse_pane_state(VI_COPY),
            &["C-b".to_owned()],
            &[],
            &"h".parse().unwrap(),
            true,
            &mut |table| {
                if table == "copy-mode-vi" {
                    Err("unknown table".into())
                } else {
                    mode_tables(table)
                }
            },
        );
        assert_eq!(result.outcome, Outcome::Unknown);
    }
}
