use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};

/// Read-only sxhkd binding adapter for bspwm-style sessions.
pub struct Sxhkd;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let path = config_path().ok_or_else(|| {
        "sxhkd: sxhkdrc was not found; runtime sxhkd state remains unknown".to_owned()
    })?;
    let content = resolved_config(&path)
        .map_err(|error| format!("sxhkd: config: {}: {error}", path.display()))?;
    Ok(parse_bindings(&content)
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "sxhkd",
                binding.combo.compact_display(),
                binding.command,
                "configured; runtime activation conditional",
            )
            .with_context("sxhkdrc static binding")
        })
        .collect())
}

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
                .any(|name| name.trim().eq_ignore_ascii_case("bspwm"))
        })
        || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

impl Sxhkd {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(path) = config_path() else {
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unavailable,
                "sxhkd configuration is unavailable",
                vec!["set SXHKD_CONFIG or provide ~/.config/sxhkd/sxhkdrc".into()],
            );
        };
        let content = match resolved_config(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult::new(
                    "sxhkd",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not read sxhkd configuration",
                    vec![format!("config: {}: {error}", path.display())],
                );
            }
        };
        let matches = parse_bindings(&content)
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let mut details = vec![format!("config: {}", path.display())];
        if matches.is_empty() {
            details.push(
                "no matching static sxhkd binding found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unknown,
                "sxhkd shortcut state is conditional",
                details,
            );
        }
        for binding in &matches {
            details.push(format!("binding: {}", binding.command));
        }
        LayerResult::new(
            "sxhkd",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching sxhkd shortcut configured; runtime activation is conditional",
            details,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    command: String,
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("SXHKD_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("sxhkd/sxhkdrc");
    path.is_file().then_some(path)
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut bindings = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw_key = lines[index].trim();
        index += 1;
        if raw_key.is_empty() || raw_key.starts_with('#') || raw_key.starts_with("^") {
            continue;
        }
        let normalized = raw_key.split_whitespace().collect::<String>();
        if !normalized.contains('+') || normalized.contains(['{', '}', '|']) {
            continue;
        }
        let Ok(combo) = normalized.parse::<KeyCombo>() else {
            continue;
        };
        let Some(command_index) = (index..lines.len()).find(|position| {
            let line = lines[*position];
            line.chars().next().is_some_and(char::is_whitespace)
                && !line.trim().is_empty()
                && !line.trim_start().starts_with('#')
        }) else {
            break;
        };
        bindings.push(Binding {
            combo,
            command: lines[command_index].trim().to_owned(),
        });
        index = command_index + 1;
    }
    bindings
}

const MAX_INCLUDE_DEPTH: usize = 16;
const MAX_RESOLVED_BYTES: usize = 256 * 1024;

fn resolved_config(root: &Path) -> Result<String, std::io::Error> {
    let mut visited = std::collections::HashSet::new();
    let mut output = String::new();
    collect_config(root, &mut visited, &mut output, 0)?;
    Ok(output)
}

fn collect_config(
    path: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    output: &mut String,
    depth: usize,
) -> Result<(), std::io::Error> {
    if depth > MAX_INCLUDE_DEPTH {
        return Ok(());
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }
    let content = crate::util::read_bounded(&canonical, MAX_RESOLVED_BYTES).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "config is unreadable or too large",
        )
    })?;
    for line in content.lines() {
        let trimmed = line.trim();
        let include = trimmed
            .strip_prefix("include ")
            .or_else(|| trimmed.strip_prefix("source "))
            .map(str::trim)
            .map(|value| value.trim_matches(['"', '\'']));
        if let Some(include) = include {
            let include_path = Path::new(include);
            let include_path = if include_path.is_absolute() {
                include_path.to_owned()
            } else {
                canonical
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(include_path)
            };
            if include_path.is_file() {
                collect_config(&include_path, visited, output, depth + 1)?;
            }
        } else if output.len() < MAX_RESOLVED_BYTES {
            output.push_str(line);
            output.push('\n');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_sxhkd_key_command_pairs() {
        let bindings = parse_bindings(
            "# comment\nsuper + Return\n    alacritty\n\nctrl + alt + t\n    notify-send terminal\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert_eq!(bindings[0].command, "alacritty");
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    #[test]
    fn ignores_dynamic_sxhkd_chords() {
        assert!(parse_bindings("super + {a,b}\n  echo dynamic\n").is_empty());
    }

    #[test]
    fn resolves_nested_includes_and_cycles_without_repeating_files() {
        let root = std::env::temp_dir().join(format!("whykey-sxhkd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("root"), "include child\nsuper + Return\n  root\n").unwrap();
        fs::write(root.join("child"), "source root\nctrl + x\n  child\n").unwrap();
        let content = resolved_config(&root.join("root")).unwrap();
        let bindings = parse_bindings(&content);
        assert_eq!(bindings.len(), 2);
        assert!(bindings.iter().any(|binding| binding.command == "root"));
        assert!(bindings.iter().any(|binding| binding.command == "child"));
        let _ = fs::remove_dir_all(root);
    }
}
