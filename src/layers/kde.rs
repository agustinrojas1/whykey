use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, LayerId, LayerResult, Outcome, SourceLocation,
};

/// Read-only KDE Plasma global shortcut adapter.
pub struct Kde;

/// Return the static KDE inventory while keeping runtime activation separate.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let path = config_path().ok_or_else(|| {
        "KDE Plasma: kglobalshortcutsrc was not found; runtime-only shortcuts remain unknown"
            .to_owned()
    })?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("KDE Plasma: config: {}: {error}", path.display()))?;
    Ok(parse_bindings(&content)
        .into_iter()
        .map(|binding| {
            let Binding {
                combo,
                group,
                action,
                description,
            } = binding;
            BindingRecord::new(
                "KDE Plasma",
                combo.compact_display(),
                description.map_or_else(|| action.clone(), |d| format!("{action} ({d})")),
                "configured; runtime registration conditional",
            )
            .with_context(format!("{group}; live runtime: org.kde.KGlobalAccel"))
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
        .flat_map(|desktop| desktop.split(':').map(str::to_owned).collect::<Vec<_>>())
        .any(|desktop| {
            matches!(
                desktop.trim().to_ascii_lowercase().as_str(),
                "kde" | "kde plasma" | "plasma" | "plasmawayland" | "plasma x11"
            )
        })
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

impl Kde {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(path) = config_path() else {
            return LayerResult::new(
                "KDE Plasma",
                LayerId::Compositor,
                Outcome::Unavailable,
                "KDE global shortcut configuration is unavailable",
                vec![
                    "kglobalshortcutsrc was not found; runtime-only shortcuts remain unknown"
                        .into(),
                ],
            );
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                return LayerResult::new(
                    "KDE Plasma",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not read KDE global shortcut configuration",
                    vec![format!("config: {}: {error}", path.display())],
                );
            }
        };
        let matches = parse_bindings(&content)
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let mut details = vec![format!("config: {}", path.display())];
        details.push(crate::runtime_readers::evidence_line(
            "kglobalaccel-runtime",
        ));
        if matches.is_empty() {
            details.push(
                "no matching static shortcut found; runtime D-Bus registrations may differ".into(),
            );
            return LayerResult::new(
                "KDE Plasma",
                LayerId::Compositor,
                Outcome::Unknown,
                "KDE global shortcut state is conditional",
                details,
            );
        }
        for binding in &matches {
            let action = binding.description.as_deref().map_or_else(
                || binding.action.clone(),
                |description| format!("{} ({description})", binding.action),
            );
            details.push(format!("{}: {action}", binding.group));
        }
        let binding_evidence = matches.first().map(|binding| BindingEvidence {
            dispatcher: Some("KGlobalAccel".into()),
            action: Some(binding.action.clone()),
            description: binding.description.clone(),
            submap: None,
            scope: BindingScope::Unknown,
            source: Some(SourceLocation::new(path.display().to_string(), None)),
            has_universal_match: false,
            uncertainty: Some(crate::layers::UncertaintyReason::EndpointUnavailable),
        });
        let result = LayerResult::new(
            "KDE Plasma",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching KDE global shortcut configured; runtime activation is conditional",
            details,
        );
        if let Some(binding) = binding_evidence {
            result.with_binding(binding)
        } else {
            result
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    group: String,
    action: String,
    description: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("KGLOBALSHORTCUTS_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("kglobalshortcutsrc");
    path.is_file().then_some(path)
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let mut group = String::from("unknown KDE component");
    let mut bindings = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            group = line[1..line.len() - 1].to_owned();
            continue;
        }
        let Some((action, value)) = line.split_once('=') else {
            continue;
        };
        let mut fields = value.split(',').map(str::trim);
        let Some(trigger) = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
            .and_then(|value| value.parse::<KeyCombo>().ok())
        else {
            continue;
        };
        bindings.push(Binding {
            combo: trigger,
            group: group.clone(),
            action: action.trim().to_owned(),
            description: fields
                .nth(1)
                .filter(|description| !description.is_empty())
                .map(str::to_owned),
        });
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kglobalshortcutsrc_bindings_and_ignores_disabled_entries() {
        let bindings = parse_bindings(
            "[org.kde.krunner.desktop]\n_launch=Alt+F2,none,Run Command\n_disabled=none,none,Disabled\n",
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].combo.to_string(), "ALT + F2");
        assert_eq!(bindings[0].group, "org.kde.krunner.desktop");
        assert_eq!(bindings[0].description.as_deref(), Some("Run Command"));
    }
    #[test]
    fn skips_malformed_lines_without_guessing_bindings() {
        assert!(parse_bindings("[group]\nthis line has no equals\n").is_empty());
        assert!(parse_bindings("[group]\n_action=ctrl+a b,none,Desc\n").is_empty());
    }
}
