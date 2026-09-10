use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// X11 window managers whose user configuration is executable code. The
/// adapter only parses literal binding declarations and never evaluates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Awesome,
    Qtile,
    XMonad,
}

impl Desktop {
    pub fn name(self) -> &'static str {
        match self {
            Self::Awesome => "AwesomeWM",
            Self::Qtile => "Qtile",
            Self::XMonad => "XMonad",
        }
    }

    fn env_names(self) -> &'static [&'static str] {
        match self {
            Self::Awesome => &["AWESOME_CONFIG"],
            Self::Qtile => &["QTILE_CONFIG"],
            Self::XMonad => &["XMONAD_CONFIG"],
        }
    }
}

pub type BindingInventoryEntry = (KeyCombo, String, String);

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    detect().is_some()
}

pub fn ipc_available() -> bool {
    detect().and_then(config_path).is_some()
}

pub fn detect() -> Option<Desktop> {
    let desktop = env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .flat_map(|value| value.split(':').map(str::to_owned).collect::<Vec<_>>())
        .find_map(|name| desktop_from_name(&name));
    desktop.or_else(|| {
        [Desktop::Awesome, Desktop::Qtile, Desktop::XMonad]
            .into_iter()
            .find(|candidate| config_path(*candidate).is_some())
    })
}

pub fn binding_inventory() -> Result<Vec<BindingInventoryEntry>, String> {
    let desktop = detect()
        .ok_or_else(|| "no AwesomeWM, Qtile, or XMonad configuration was detected".to_owned())?;
    let path = config_path(desktop).ok_or_else(|| {
        format!(
            "{} configuration was not found; runtime bindings remain unknown",
            desktop.name()
        )
    })?;
    let content = read_config(&path)?;
    Ok(parse(desktop, &content)
        .into_iter()
        .map(|binding| (binding.combo, binding.action, binding.context))
        .collect())
}

pub struct Programmable;

impl Programmable {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(desktop) = detect() else {
            return LayerResult {
                binding: None,
                layer: "Programmable X11 WM",
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: "AwesomeWM, Qtile, and XMonad were not detected".into(),
                details: vec!["set XDG_CURRENT_DESKTOP or an explicit config variable".into()],
            };
        };
        let Some(path) = config_path(desktop) else {
            return LayerResult {
                binding: None,
                layer: desktop.name(),
                id: LayerId::Compositor,
                outcome: Outcome::Unavailable,
                summary: format!("{} configuration is unavailable", desktop.name()),
                details: vec![format!(
                    "set {} or provide the standard config path",
                    desktop.env_names()[0]
                )],
            };
        };
        let content = match read_config(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult {
                    binding: None,
                    layer: desktop.name(),
                    id: LayerId::Compositor,
                    outcome: Outcome::Unavailable,
                    summary: format!("could not read {} configuration", desktop.name()),
                    details: vec![error],
                };
            }
        };
        let matches = parse(desktop, &content)
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let mut details = vec![format!("config: {}", path.display())];
        if matches.is_empty() {
            details.push(
                "no matching literal binding found; executable helpers, modes, and runtime reload state remain unknown".into(),
            );
            return LayerResult {
                binding: None,
                layer: desktop.name(),
                id: LayerId::Compositor,
                outcome: Outcome::Unknown,
                summary: format!("{} shortcut state is conditional", desktop.name()),
                details,
            };
        }
        for binding in &matches {
            details.push(format!("{}: {}", binding.context, binding.action));
        }
        LayerResult {
            binding: None,
            layer: desktop.name(),
            id: LayerId::Compositor,
            outcome: Outcome::HandledUncertain,
            summary: format!(
                "matching {} binding configured; executable runtime and precedence are conditional",
                desktop.name()
            ),
            details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    context: String,
}

fn desktop_from_name(name: &str) -> Option<Desktop> {
    match name.trim().to_ascii_lowercase().as_str() {
        "awesome" | "awesomewm" => Some(Desktop::Awesome),
        "qtile" => Some(Desktop::Qtile),
        "xmonad" => Some(Desktop::XMonad),
        _ => None,
    }
}

fn config_path(desktop: Desktop) -> Option<PathBuf> {
    for variable in desktop.env_names() {
        if let Some(path) = env::var_os(variable) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let candidates = match desktop {
        Desktop::Awesome => vec![
            home.join(".config/awesome/rc.lua"),
            home.join(".config/awesome/config.lua"),
        ],
        Desktop::Qtile => vec![
            home.join(".config/qtile/config.py"),
            home.join(".config/qtile/config.pyc"),
        ],
        Desktop::XMonad => vec![
            home.join(".xmonad/xmonad.hs"),
            home.join(".xmonad/config.hs"),
        ],
    };
    candidates.into_iter().find(|path| path.is_file())
}

fn read_config(path: &PathBuf) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("config: {}: {error}", path.display()))?;
    if metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(format!(
            "config: {} exceeds the {MAX_CONFIG_BYTES}-byte safety limit",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|error| format!("config: {}: {error}", path.display()))
}

fn parse(desktop: Desktop, content: &str) -> Vec<Binding> {
    match desktop {
        Desktop::Awesome => parse_awesome(content),
        Desktop::Qtile => parse_qtile(content),
        Desktop::XMonad => parse_xmonad(content),
    }
}

fn parse_awesome(content: &str) -> Vec<Binding> {
    let modkey = content.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == "modkey").then(|| literal(value.trim()))?
    });
    calls(content, "awful.key")
        .into_iter()
        .filter_map(|call| {
            let args = split_top_level(call);
            if args.len() < 2 {
                return None;
            }
            let modifiers = parse_awesome_modifiers(&args[0], modkey.as_deref())?;
            let key = literal(args[1].trim())?;
            let combo = combo_from_names(&modifiers, &key)?;
            let action = find_description(call).unwrap_or_else(|| "awful.key handler".into());
            Some(Binding {
                combo,
                action,
                context: "awful.key literal declaration".into(),
            })
        })
        .collect()
}

fn parse_awesome_modifiers(value: &str, modkey: Option<&str>) -> Option<Vec<String>> {
    let value = value.trim();
    let values = if value.starts_with('{') && value.ends_with('}') {
        split_top_level(&value[1..value.len() - 1])
    } else {
        vec![value.to_owned()]
    };
    let mut modifiers = Vec::new();
    for value in values {
        let value = value.trim();
        let value = if value == "modkey" { modkey? } else { value };
        let value = literal(value).unwrap_or_else(|| value.to_owned());
        let modifier = match value.to_ascii_lowercase().as_str() {
            "mod4" | "super" | "meta" => "super",
            "mod1" | "alt" => "alt",
            "control" | "ctrl" => "ctrl",
            "shift" => "shift",
            _ => return None,
        };
        modifiers.push(modifier.into());
    }
    Some(modifiers)
}

fn parse_qtile(content: &str) -> Vec<Binding> {
    calls(content, "Key")
        .into_iter()
        .filter_map(|call| {
            let args = split_top_level(call);
            if args.len() < 2 {
                return None;
            }
            let modifiers = args[0]
                .trim()
                .strip_prefix('[')?
                .strip_suffix(']')?
                .split(',')
                .map(str::trim)
                .map(literal)
                .collect::<Option<Vec<_>>>()?;
            let modifiers = modifiers
                .into_iter()
                .map(|modifier| match modifier.to_ascii_lowercase().as_str() {
                    "mod4" | "super" => Some("super".into()),
                    "mod1" | "alt" => Some("alt".into()),
                    "control" | "ctrl" => Some("ctrl".into()),
                    "shift" => Some("shift".into()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let key = literal(args[1].trim())?;
            let combo = combo_from_names(&modifiers, &key)?;
            let action = args
                .get(2)
                .map(|action| action.trim().chars().take(160).collect())
                .filter(|action: &String| !action.is_empty())
                .unwrap_or_else(|| "Qtile Key handler".into());
            Some(Binding {
                combo,
                action,
                context: "Key literal declaration".into(),
            })
        })
        .collect()
}

fn parse_xmonad(content: &str) -> Vec<Binding> {
    content
        .lines()
        .filter_map(|line| {
            let key_start = line.find("xK_")?;
            let rest = &line[key_start + 3..];
            let key_end = rest
                .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .unwrap_or(rest.len());
            let key = &rest[..key_end];
            if key.is_empty() {
                return None;
            }
            let prefix = &line[..key_start];
            let mut modifiers = Vec::new();
            for (needle, modifier) in [
                ("controlMask", "ctrl"),
                ("mod1Mask", "alt"),
                ("mod4Mask", "super"),
                ("shiftMask", "shift"),
            ] {
                if prefix.contains(needle) {
                    modifiers.push(modifier.to_owned());
                }
            }
            let combo = combo_from_names(&modifiers, key)?;
            let action = line
                .split_once("),")
                .map(|(_, action)| action.trim().trim_end_matches(',').to_owned())
                .filter(|action| !action.is_empty())
                .unwrap_or_else(|| "XMonad key binding".into());
            Some(Binding {
                combo,
                action,
                context: "xK_* literal declaration".into(),
            })
        })
        .collect()
}

fn combo_from_names(modifiers: &[String], key: &str) -> Option<KeyCombo> {
    let mut value = modifiers.join("+");
    if !value.is_empty() {
        value.push('+');
    }
    value.push_str(key.trim_start_matches("XK_"));
    value.parse().ok()
}

fn literal(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 {
        return None;
    }
    let quote = value.as_bytes()[0] as char;
    if !matches!(quote, '\'' | '"') || value.as_bytes().last().copied()? as char != quote {
        return None;
    }
    Some(value[1..value.len() - 1].to_owned())
}

fn find_description(value: &str) -> Option<String> {
    let start = value.find("description")?;
    let remainder = value[start..].split_once('=')?.1.trim();
    let quoted = remainder.find(['\'', '"'])?;
    quoted_literal(&remainder[quoted..])
}

fn quoted_literal(value: &str) -> Option<String> {
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let mut escaped = false;
    for (index, character) in value.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return Some(value[1..index].to_owned());
        }
    }
    None
}

fn calls<'a>(content: &'a str, marker: &str) -> Vec<&'a str> {
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(relative) = content[offset..].find(marker) {
        let marker_start = offset + relative;
        if marker == "Key"
            && content[..marker_start]
                .rfind("KeyChord(")
                .is_some_and(|open| {
                    content[..marker_start]
                        .rfind(')')
                        .is_none_or(|close| open > close)
                })
        {
            offset = marker_start + marker.len();
            continue;
        }
        let start = marker_start + marker.len();
        let Some(open) = content[start..].find('(').map(|index| start + index) else {
            break;
        };
        if content[start..open]
            .chars()
            .any(|character| character.is_alphanumeric() || character == '_')
        {
            offset = start + marker.len();
            continue;
        }
        let Some(close) = balanced_end(content, open) else {
            break;
        };
        calls.push(&content[open + 1..close]);
        offset = close + 1;
    }
    calls
}

fn balanced_end(content: &str, open: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(open) {
        let character = *byte as char;
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == '(' {
            depth += 1;
        } else if character == ')' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn split_top_level(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if matches!(character, '(' | '[' | '{') {
            depth += 1;
        } else if matches!(character, ')' | ']' | '}') {
            depth = depth.saturating_sub(1);
        } else if character == ',' && depth == 0 {
            parts.push(value[start..index].trim().to_owned());
            start = index + character.len_utf8();
        }
    }
    let tail = value[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_owned());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_awesome_bindings_and_resolves_modkey() {
        let bindings = parse_awesome(
            "modkey = \"Mod4\"\nawful.key({ modkey, \"Shift\" }, \"c\", function() end, {description = \"close\"})",
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].combo, "super+shift+c".parse().unwrap());
        assert_eq!(bindings[0].action, "close");
    }

    #[test]
    fn parses_literal_qtile_key() {
        let bindings = parse_qtile("Key([\"mod4\", \"shift\"], \"h\", lazy.layout.left())");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].combo, "super+shift+h".parse().unwrap());
    }

    #[test]
    fn parses_literal_xmonad_key() {
        let bindings = parse_xmonad("((mod4Mask .|. shiftMask, xK_c), spawn \"notify\")");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].combo, "super+shift+c".parse().unwrap());
    }

    #[test]
    fn ignores_dynamic_or_unrecognized_declarations() {
        assert!(parse_awesome("awful.key(modkey, key_name, action)").is_empty());
        assert!(parse_qtile("Key(mods, key, lazy.spawn(cmd))").is_empty());
        assert!(parse_qtile("KeyChord([\"mod4\"], [Key([\"shift\"], \"h\", action)])").is_empty());
        assert!(parse_xmonad("((mod4Mask, keyName), action)").is_empty());
    }
}
