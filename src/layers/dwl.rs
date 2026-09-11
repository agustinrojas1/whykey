//! Read-only dwl configuration adapter.
//!
//! dwl has no stable compositor IPC for global shortcuts. This adapter reads
//! the `static const Key keys[]` table from a local `config.h` and reports
//! conditional evidence only; it never claims runtime activation.

use std::env;
use std::fs;
use std::path::Path;

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;

pub struct Dwl;

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("dwl"))
        })
        || config_path().is_some_and(|path| path.is_file())
}

pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let path = config_path().ok_or_else(|| "dwl config.h was not found".to_owned())?;
    let bindings = load_bindings(&path)?;
    Ok(bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "dwl",
                binding.combo.compact_display(),
                binding.action,
                "configured; runtime activation conditional",
            )
            .with_context(format!("dwl config.h: {}", path.display()))
        })
        .collect())
}

impl Dwl {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let path = match config_path() {
            Some(path) => path,
            None => {
                return LayerResult::new(
                    "dwl",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect dwl config.h",
                    vec!["set DWL_CONFIG or provide ~/.config/dwl/config.h".into()],
                );
            }
        };
        let bindings = match load_bindings(&path) {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult::new(
                    "dwl",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect dwl config.h",
                    vec![error],
                );
            }
        };
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let source = format!("source: {}", path.display());
        if matches.is_empty() {
            return LayerResult::new(
                "dwl",
                LayerId::Compositor,
                Outcome::Pass,
                "no matching dwl key binding found",
                vec![source],
            );
        }
        let mut details = vec![source];
        details.extend(
            matches
                .iter()
                .map(|binding| format!("binding: {}", binding.action)),
        );
        details.push(
            "dwl has no stable global-shortcut IPC; config.h evidence is conditional until runtime verification exists".into(),
        );
        LayerResult::new(
            "dwl",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "dwl config.h contains a matching key binding",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
}

fn load_bindings(path: &Path) -> Result<Vec<Binding>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_CONFIG_BYTES}-byte config limit",
            path.display()
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    Ok(parse_bindings(&content))
}

fn config_path() -> Option<std::path::PathBuf> {
    if let Some(path) = env::var_os("DWL_CONFIG") {
        return Some(path.into());
    }
    let base = crate::util::config_base()?;
    Some(base.join("dwl/config.h"))
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.split_once("//").map_or(raw_line, |(line, _)| line);
        let Some(open) = line.find('{') else { continue };
        let Some(close) = line[open + 1..].find('}') else {
            continue;
        };
        let fields = line[open + 1..open + 1 + close]
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>();
        if fields.len() < 3 || !fields[1].contains("XKB_KEY_") {
            continue;
        }
        let Some(combo) = parse_key(fields[0], fields[1]) else {
            continue;
        };
        let action = fields[2]
            .split_whitespace()
            .next()
            .unwrap_or("unknown action")
            .to_owned();
        bindings.push(Binding { combo, action });
        if bindings.len() >= MAX_BINDINGS {
            break;
        }
    }
    bindings
}

fn parse_key(modifiers: &str, symbol: &str) -> Option<KeyCombo> {
    let mut parts = Vec::new();
    for modifier in modifiers.split('|').map(str::trim) {
        match modifier {
            "MODKEY" | "WLR_MODIFIER_LOGO" => parts.push("super"),
            "WLR_MODIFIER_SHIFT" => parts.push("shift"),
            "WLR_MODIFIER_CTRL" => parts.push("ctrl"),
            "WLR_MODIFIER_ALT" => parts.push("alt"),
            "0" | "WLR_MODIFIER_NONE" => {}
            _ => return None,
        }
    }
    let key = symbol.strip_prefix("XKB_KEY_")?;
    let normalized = key.to_ascii_lowercase();
    let key = match normalized.as_str() {
        "return" => "return".to_owned(),
        "escape" => "escape".to_owned(),
        "space" => "space".to_owned(),
        "tab" => "tab".to_owned(),
        "left" => "left".to_owned(),
        "right" => "right".to_owned(),
        "up" => "up".to_owned(),
        "down" => "down".to_owned(),
        value => value.to_owned(),
    };
    if parts.is_empty() {
        key.parse().ok()
    } else {
        format!("{}+{key}", parts.join("+")).parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dwl_key_table_without_executing_config() {
        let bindings = parse_bindings(
            "static const Key keys[] = {\n { MODKEY|WLR_MODIFIER_SHIFT, XKB_KEY_Return, spawn, {.v = termcmd} },\n { MODKEY, XKB_KEY_q, quit, {0} },\n};",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo.compact_display(), "SHIFT+SUPER+RETURN");
        assert_eq!(bindings[0].action, "spawn");
        assert_eq!(bindings[1].combo.compact_display(), "SUPER+Q");
    }

    #[test]
    fn skips_unknown_modifier_rows() {
        assert!(parse_bindings("{ MODKEY|CUSTOM, XKB_KEY_q, quit, {0} },").is_empty());
    }
}
