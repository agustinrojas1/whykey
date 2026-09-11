use std::env;
use std::fs;
use std::path::PathBuf;

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome, openbox};

/// Read-only labwc adapter. labwc intentionally keeps an Openbox-compatible
/// `rc.xml` keybind format, so the shared conservative XML scanner is reused.
pub struct Labwc;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let (_path, bindings) = load_bindings().map_err(|error| format!("labwc: {error}"))?;
    Ok(bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "labwc",
                binding.combo.compact_display(),
                binding.action,
                "configured; runtime activation conditional",
            )
            .with_context(format!("rc.xml keybind ({})", binding.key_name))
        })
        .collect())
}

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("labwc"))
        })
        || config_path().is_some_and(|path| path.is_file())
}

pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

impl Labwc {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let (path, bindings) = match load_bindings() {
            Ok(value) => value,
            Err(error) => {
                return LayerResult::new(
                    "labwc",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect labwc keybinds",
                    vec![error],
                );
            }
        };
        let matches = bindings
            .iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        let mut details = vec![format!("config: {}", path.display())];
        if matches.is_empty() {
            details.push(
                "no matching static labwc keybind found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult::new(
                "labwc",
                LayerId::Compositor,
                Outcome::Unknown,
                "labwc shortcut state is conditional",
                details,
            );
        }
        for binding in matches {
            details.push(format!("binding: {}", binding.action));
        }
        LayerResult::new(
            "labwc",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching labwc keybind configured; runtime activation is conditional",
            details,
        )
    }
}

fn load_bindings() -> Result<(PathBuf, Vec<openbox::Binding>), String> {
    let path = config_path().ok_or_else(|| {
        "labwc rc.xml was not found; runtime keybind state remains unknown".to_owned()
    })?;
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok((path, openbox::parse_bindings(&content)))
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("LABWC_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("labwc/rc.xml");
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_openbox_compatible_labwc_keybinds() {
        let bindings = openbox::parse_bindings(
            r#"<keybind key="W-C-t"><action name="Execute"><command>foot</command></action></keybind>"#,
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].combo, "super+ctrl+t".parse().unwrap());
    }
}
