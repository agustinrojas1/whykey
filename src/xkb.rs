//! Compositor-independent XKB keymap compilation and inspection.
//!
//! The direct libxkbcommon API is intentionally not a hard build dependency:
//! many supported systems do not ship its development headers.  `xkbcli`
//! uses the same libxkbcommon compiler and gives us a bounded, read-only
//! fallback when RMLVO data is available from the environment.

use std::collections::HashMap;
use std::env;
use std::process::Command;

use crate::command;

type KeycodeMap = HashMap<String, Vec<u32>>;
type SymbolGroupMap = HashMap<String, HashMap<usize, Vec<String>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rmlvo {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: Option<String>,
    pub options: Option<String>,
}

impl Rmlvo {
    /// Read an explicit layout from the environment.
    ///
    /// We require a layout override rather than silently compiling `us`: a
    /// guessed map could make a physical evdev event look more certain than
    /// the compositor's actual layout.
    pub fn from_environment() -> Option<Self> {
        let layout = env_value("WHYKEY_XKB_LAYOUT", "XKB_DEFAULT_LAYOUT")?;
        if layout.trim().is_empty() {
            return None;
        }
        Some(Self {
            rules: env_value_or("WHYKEY_XKB_RULES", "XKB_DEFAULT_RULES", "evdev"),
            model: env_value_or("WHYKEY_XKB_MODEL", "XKB_DEFAULT_MODEL", "pc105"),
            layout,
            variant: env_value("WHYKEY_XKB_VARIANT", "XKB_DEFAULT_VARIANT"),
            options: env_value("WHYKEY_XKB_OPTIONS", "XKB_DEFAULT_OPTIONS"),
        })
    }
}

fn env_value(primary: &str, fallback: &str) -> Option<String> {
    env::var(primary)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var(fallback)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn env_value_or(primary: &str, fallback: &str, default: &str) -> String {
    env_value(primary, fallback).unwrap_or_else(|| default.into())
}

pub fn compile_keymap(rmlvo: &Rmlvo) -> Option<String> {
    let mut command = Command::new("xkbcli");
    command.args([
        "compile-keymap",
        "--include-defaults",
        "--rules",
        &rmlvo.rules,
        "--model",
        &rmlvo.model,
        "--layout",
        &rmlvo.layout,
    ]);
    if let Some(variant) = rmlvo.variant.as_deref() {
        command.args(["--variant", variant]);
    }
    if let Some(options) = rmlvo.options.as_deref() {
        command.args(["--options", options]);
    }
    let output = command::output(&mut command).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn compile_keymap_from_environment() -> Option<String> {
    compile_keymap(&Rmlvo::from_environment()?)
}

/// Read an optional zero-based active group supplied by an integration or a
/// test harness. A compositor remains the authoritative source when it can
/// expose this state; evdev has no standard way to query it itself.
pub fn group_from_environment() -> usize {
    env::var("WHYKEY_XKB_GROUP")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

/// Return the symbols in one XKB group for a Linux evdev key code.
pub fn symbols_for_evdev_keycode(keymap: &str, evdev_keycode: u16, group: usize) -> Vec<String> {
    let candidates = [u32::from(evdev_keycode), u32::from(evdev_keycode) + 8];
    let mut symbols = parse_symbol_keycodes_for_group(keymap, group)
        .into_iter()
        .filter_map(|(symbol, codes)| {
            codes
                .iter()
                .any(|code| candidates.contains(code))
                .then_some(symbol)
        })
        .collect::<Vec<_>>();
    symbols.sort();
    symbols.dedup();
    symbols
}

/// Select one symbol level for a physical key while retaining XKB's declared
/// level order. Callers can apply their observed Shift/Caps/AltGr state
/// without losing the distinction between unshifted and shifted symbols.
pub fn preferred_symbol_for_evdev_keycode(
    keymap: &str,
    evdev_keycode: u16,
    group: usize,
    level: usize,
) -> Option<String> {
    let candidates = [u32::from(evdev_keycode), u32::from(evdev_keycode) + 8];
    let (keycodes, symbols) = parse_keymap(keymap);
    keycodes.into_iter().find_map(|(name, codes)| {
        if !codes.iter().any(|code| candidates.contains(code)) {
            return None;
        }
        let levels = symbols.get(&name)?.get(&group)?;
        levels
            .get(level)
            .or_else(|| levels.first())
            .cloned()
            .filter(|symbol| !symbol.is_empty())
    })
}

pub fn parse_symbol_keycodes_for_group(
    keymap: &str,
    group_index: usize,
) -> HashMap<String, Vec<u32>> {
    let (keycodes, symbols) = parse_keymap(keymap);
    symbols
        .into_iter()
        .flat_map(|(name, groups)| {
            let codes = keycodes.get(&name).cloned().unwrap_or_default();
            groups
                .get(&group_index)
                .cloned()
                .into_iter()
                .flat_map(move |symbols_for_key| {
                    symbols_for_key
                        .into_iter()
                        .filter(|symbol| !symbol.is_empty())
                        .flat_map({
                            let codes = codes.clone();
                            move |symbol| {
                                codes
                                    .clone()
                                    .into_iter()
                                    .map(move |code| (symbol.clone(), code))
                            }
                        })
                })
        })
        .fold(HashMap::new(), |mut result, (symbol, code)| {
            let values = result.entry(symbol).or_default();
            if !values.contains(&code) {
                values.push(code);
            }
            result
        })
}

fn parse_keymap(keymap: &str) -> (KeycodeMap, SymbolGroupMap) {
    let mut keycodes: KeycodeMap = HashMap::new();
    let mut symbols: SymbolGroupMap = HashMap::new();
    let mut in_keycodes = false;
    let mut in_symbols = false;
    let mut pending_symbol_key = None;

    for line in keymap.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("xkb_keycodes") {
            in_keycodes = true;
            in_symbols = false;
            continue;
        }
        if trimmed.starts_with("xkb_symbols") {
            in_keycodes = false;
            in_symbols = true;
            pending_symbol_key = None;
            continue;
        }
        if trimmed.starts_with("xkb_") {
            in_keycodes = false;
            in_symbols = false;
            pending_symbol_key = None;
        }
        if in_keycodes {
            if let Some((name, value)) = parse_keycode_line(trimmed) {
                keycodes.entry(name).or_default().push(value);
            }
        }
        if in_symbols {
            if let Some((name, group, symbols_for_key)) = parse_symbol_line(trimmed) {
                symbols
                    .entry(name)
                    .or_default()
                    .insert(group, symbols_for_key);
                pending_symbol_key = None;
            } else if let Some(name) = parse_symbol_key_name(trimmed) {
                pending_symbol_key = Some(name);
            } else if let Some(name) = pending_symbol_key.clone() {
                if let Some(symbols_for_key) = parse_symbol_fragment(trimmed) {
                    let group = parse_symbol_group(trimmed).unwrap_or(0);
                    symbols
                        .entry(name)
                        .or_default()
                        .insert(group, symbols_for_key);
                } else if trimmed.starts_with('}') {
                    pending_symbol_key = None;
                }
            }
        }
    }

    (keycodes, symbols)
}

/// Return every symbol-to-keycode mapping, regardless of layout group.
///
/// This is used by symbolic desktop adapters that need to preserve the full
/// candidate set when the compositor does not expose an active group.
pub fn parse_symbol_keycodes(keymap: &str) -> HashMap<String, Vec<u32>> {
    let mut result = HashMap::new();
    let mut groups = 0;
    while groups < 32 {
        let selected = parse_symbol_keycodes_for_group(keymap, groups);
        if selected.is_empty() && groups > 0 {
            break;
        }
        for (symbol, codes) in selected {
            let values = result.entry(symbol).or_insert_with(Vec::new);
            for code in codes {
                if !values.contains(&code) {
                    values.push(code);
                }
            }
        }
        groups += 1;
    }
    result
}

fn parse_keycode_line(line: &str) -> Option<(String, u32)> {
    let (raw_name, raw_value) = line.split_once('=')?;
    let name = raw_name.trim().strip_prefix('<')?.strip_suffix('>')?;
    let value = raw_value.trim().trim_end_matches(';').parse().ok()?;
    Some((name.to_owned(), value))
}

fn parse_symbol_line(line: &str) -> Option<(String, usize, Vec<String>)> {
    let name = parse_symbol_key_name(line)?;
    let fragment = parse_symbol_fragment(line)?;
    let group = parse_symbol_group(line).unwrap_or(0);
    Some((name, group, fragment))
}

fn parse_symbol_key_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("key <")?;
    let (name, _) = rest.split_once('>')?;
    Some(name.to_owned())
}

fn parse_symbol_fragment(line: &str) -> Option<Vec<String>> {
    let symbols = line.rsplit_once('[')?.1.split_once(']')?.0;
    let symbols = symbols
        .split(',')
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty())
        .map(|symbol| {
            if symbol == "NoSymbol" {
                String::new()
            } else {
                symbol.to_owned()
            }
        })
        .collect::<Vec<_>>();
    (!symbols.is_empty()).then_some(symbols)
}

fn parse_symbol_group(line: &str) -> Option<usize> {
    let (prefix, _) = line.split_once('=')?;
    let start = prefix.find("symbols[")? + "symbols[".len();
    let end = prefix[start..].find(']')? + start;
    let group = prefix[start..end].trim().parse::<usize>().ok()?;
    group.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYMAP: &str = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
    <AE01> = 10;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, Q ]
        symbols[2] = [ a, A ]
    };
    key <AE01> {
        symbols[1] = [ 1, exclam ]
    };
};
};
"#;

    #[test]
    fn selects_symbols_by_group_and_evdev_offset() {
        let first = symbols_for_evdev_keycode(KEYMAP, 16, 0);
        assert_eq!(first, vec!["Q", "q"]);
        let second_layout = symbols_for_evdev_keycode(KEYMAP, 16, 1);
        assert_eq!(second_layout, vec!["A", "a"]);
        let second = symbols_for_evdev_keycode(KEYMAP, 2, 0);
        assert_eq!(second, vec!["1", "exclam"]);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(KEYMAP, 16, 0, 0),
            Some("q".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(KEYMAP, 16, 0, 1),
            Some("Q".into())
        );
    }

    #[test]
    fn ignores_unknown_groups() {
        assert!(parse_symbol_keycodes_for_group(KEYMAP, 4).is_empty());
    }

    #[test]
    fn preserves_no_symbol_level_positions() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, NoSymbol, Q ]
    };
};
};
"#;
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, 16, 0, 0),
            Some("q".into())
        );
        assert_eq!(preferred_symbol_for_evdev_keycode(keymap, 16, 0, 1), None);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, 16, 0, 2),
            Some("Q".into())
        );
        let symbols = symbols_for_evdev_keycode(keymap, 16, 0);
        assert_eq!(symbols, vec!["Q", "q"]);
    }

    #[test]
    fn parses_the_wire_shape_emitted_by_xkbcli_for_multiple_layouts() {
        let rmlvo = Rmlvo {
            rules: "evdev".into(),
            model: "pc105".into(),
            layout: "us,de".into(),
            variant: None,
            options: None,
        };
        let Some(keymap) = compile_keymap(&rmlvo) else {
            // xkbcli is an optional runtime dependency on systems without
            // libxkbcommon tools; the static parser fixture above remains
            // authoritative in that environment.
            return;
        };
        let us = symbols_for_evdev_keycode(&keymap, 21, 0);
        let german = symbols_for_evdev_keycode(&keymap, 21, 1);
        assert!(us.iter().any(|symbol| symbol == "y"));
        assert!(german.iter().any(|symbol| symbol == "z"));
    }
}
