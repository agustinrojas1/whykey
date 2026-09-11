//! Read-only herbstluftwm key-binding adapter.
//!
//! herbstluftwm exposes its effective key table through `herbstclient`; this
//! adapter keeps that query bounded and treats the returned command text as
//! evidence only. It never executes the binding action or mutates the WM.

use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};

const MAX_BINDINGS: usize = 4096;

pub struct Herbstluftwm;

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    env::var_os("HERBSTLUFTWM_SOCKET").is_some_and(|value| !value.is_empty())
        || env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .into_iter()
            .chain(env::var("XDG_SESSION_DESKTOP").ok())
            .any(|desktop| {
                desktop
                    .split(':')
                    .any(|name| name.trim().eq_ignore_ascii_case("herbstluftwm"))
            })
}

pub fn ipc_available() -> bool {
    query().is_ok()
}

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    Ok(query()?
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "herbstluftwm",
                binding.combo.compact_display(),
                binding.action,
                "herbstclient list_keybinds; runtime activation conditional",
            )
        })
        .collect())
}

impl Herbstluftwm {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let bindings = match query() {
            Ok(bindings) => bindings,
            Err(error) => {
                return LayerResult::new(
                    "herbstluftwm",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect herbstluftwm key bindings",
                    vec![error],
                );
            }
        };
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return LayerResult::new(
                "herbstluftwm",
                LayerId::Compositor,
                Outcome::Pass,
                "no matching herbstluftwm key binding found",
                vec!["source: herbstclient list_keybinds".into()],
            );
        }
        let mut details = vec!["source: herbstclient list_keybinds".into()];
        details.extend(
            matches
                .iter()
                .map(|binding| format!("binding: {}", binding.action)),
        );
        details.push("runtime activation and hook ordering remain conditional".into());
        LayerResult::new(
            "herbstluftwm",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "herbstluftwm exposes a matching key binding",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
}

fn query() -> Result<Vec<Binding>, String> {
    let mut command = Command::new("herbstclient");
    command.arg("list_keybinds");
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("herbstclient exited with {}", output.status)
        } else {
            format!("herbstclient: {stderr}")
        });
    }
    parse_bindings(&String::from_utf8_lossy(&output.stdout))
}

fn parse_bindings(content: &str) -> Result<Vec<Binding>, String> {
    let mut bindings = Vec::new();
    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, action)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let raw_key = raw_key.trim();
        if raw_key.contains(['{', '}', '|']) {
            continue;
        }
        let normalized = [
            ("Mod4-", "super+"),
            ("Super-", "super+"),
            ("Mod1-", "alt+"),
            ("Alt-", "alt+"),
            ("Control-", "ctrl+"),
            ("Ctrl-", "ctrl+"),
            ("Shift-", "shift+"),
        ]
        .iter()
        .find_map(|(prefix, replacement)| {
            raw_key
                .strip_prefix(prefix)
                .map(|key| format!("{replacement}{key}"))
        })
        .unwrap_or_else(|| raw_key.to_owned());
        let Some(combo) = normalized.parse::<KeyCombo>().ok() else {
            continue;
        };
        bindings.push(Binding {
            combo,
            action: action.trim().to_owned(),
        });
        if bindings.len() > MAX_BINDINGS {
            return Err(format!(
                "herbstclient returned more than {MAX_BINDINGS} bindings"
            ));
        }
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_effective_key_table_and_ignores_comments() {
        let bindings = parse_bindings("Mod4-Return spawn foot\n# ignored\nCtrl-c quit\n").unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert_eq!(bindings[0].action, "spawn foot");
    }

    #[test]
    fn skips_dynamic_or_malformed_rows() {
        let bindings = parse_bindings("not-a-binding\nMod4-{a,b} spawn dynamic\n").unwrap();
        assert!(bindings.is_empty());
    }
}
