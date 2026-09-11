use std::env;

use crate::key::KeyCombo;
use crate::layers::gsettings::{self, GsettingsBinding, GsettingsMeta};
use crate::layers::{BindingRecord, LayerResult};

/// Read-only Cinnamon global keyboard-shortcut adapter.
pub struct Cinnamon;

const SCHEMA_PREFIX: &str = "org.cinnamon.desktop.keybindings";

static META: GsettingsMeta = GsettingsMeta {
    layer: "Cinnamon",
    error_prefix: "Cinnamon: ",
    unavailable_summary: "could not inspect Cinnamon global shortcuts",
    no_match_summary: "no active Cinnamon global shortcut found",
    match_summary: "Cinnamon global shortcut consumes the key",
    source_details: &["source: gsettings list-recursively org.cinnamon.desktop.keybindings"],
    inventory_source: "Cinnamon",
    inventory_certainty: "runtime GSettings value",
    inventory_context: "global GSettings shortcut",
};

pub fn applicable() -> bool {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop.split(':').any(|name| {
                matches!(
                    name.trim().to_ascii_lowercase().as_str(),
                    "cinnamon" | "x-cinnamon"
                )
            })
        })
        || env::var_os("CINNAMON_VERSION").is_some()
}

pub fn ipc_available() -> bool {
    gsettings::run_list(&["list-recursively", SCHEMA_PREFIX]).is_ok()
}

/// Return Cinnamon's live GSettings shortcut values as normalized entries.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    gsettings::inventory(&META, load_bindings())
}

impl Cinnamon {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        gsettings::inspect(&META, load_bindings(), key)
    }
}

fn load_bindings() -> Result<Vec<GsettingsBinding>, String> {
    let output = gsettings::run_list(&["list-recursively", SCHEMA_PREFIX])?;
    Ok(parse_bindings(&output))
}

fn parse_bindings(content: &str) -> Vec<GsettingsBinding> {
    let mut bindings = Vec::new();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if !schema.starts_with(SCHEMA_PREFIX) {
            continue;
        }
        let values = gsettings::quoted_values(value);
        for raw_binding in values {
            let Some(combo) = gsettings::parse_accelerator(&raw_binding) else {
                continue;
            };
            let action = format!("{schema} {name}");
            bindings.push(GsettingsBinding {
                combo,
                action: action.clone(),
                command: None,
                detail: format!("binding: {action}"),
            });
        }
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::gsettings::{parse_accelerator, quoted_values};

    #[test]
    fn parses_cinnamon_gsettings_rows() {
        let bindings = parse_bindings(
            "org.cinnamon.desktop.keybindings.wm switch-to-workspace-1 ['<Super>1']\norg.cinnamon.desktop.keybindings.media-keys terminal ['<Primary><Alt>t']\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+1".parse().unwrap());
        assert!(bindings[0].action.contains("switch-to-workspace-1"));
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    #[test]
    fn ignores_empty_and_unknown_accelerators() {
        assert!(parse_accelerator("<Super>").is_none());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(quoted_values("[]").is_empty());
    }
    #[test]
    fn keeps_shared_modifier_superset() {
        // Unlike GNOME, the shared table accepts these historical aliases.
        assert_eq!(
            parse_accelerator("<Meta>x").unwrap(),
            "super+x".parse().unwrap()
        );
        assert_eq!(
            parse_accelerator("<mod3>x").unwrap(),
            "mod3+x".parse().unwrap()
        );
    }
}
