use std::env;

use crate::key::KeyCombo;
use crate::layers::gsettings::{self, GsettingsBinding, GsettingsMeta};
use crate::layers::{BindingRecord, LayerResult};

/// Read-only Cinnamon global keyboard-shortcut adapter.
pub struct Cinnamon;

const SCHEMA_PREFIX: &str = "org.cinnamon.desktop.keybindings";

/// The shell's own schema and its child schemas: muffin's window bindings
/// (`.wm`) and the media keys (`.media-keys`). `gsettings list-recursively`
/// on the parent prints only the parent's keys, so each child is read too.
const SCHEMAS: &[&str] = &[
    SCHEMA_PREFIX,
    "org.cinnamon.desktop.keybindings.wm",
    "org.cinnamon.desktop.keybindings.media-keys",
];

/// Cinnamon's relocatable custom shortcuts; `command` holds the command.
const CUSTOM: gsettings::CustomKeybindings = gsettings::CustomKeybindings {
    list_schema: "org.cinnamon.desktop.keybindings",
    list_key: "custom-list",
    child_schema: "org.cinnamon.desktop.keybindings.custom-keybinding",
    path: "/org/cinnamon/desktop/keybindings/custom-keybindings/",
    command_key: "command",
};

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
    if super::compositor::remote_session() {
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
    let mut bindings =
        bindings_from_listings(|schema| gsettings::run_list(&["list-recursively", schema]))?;
    bindings.extend(gsettings::custom_bindings(&CUSTOM)?);
    Ok(bindings)
}

/// Bindings from `list-recursively` output for each schema in `SCHEMAS`.
fn bindings_from_listings(
    list: impl Fn(&str) -> Result<String, String>,
) -> Result<Vec<GsettingsBinding>, String> {
    let mut bindings = Vec::new();
    for schema in SCHEMAS {
        match list(schema) {
            Ok(output) => bindings.extend(parse_bindings(&output)),
            // A child schema comes with its package (cinnamon-settings-daemon
            // for the media keys); without it there are no such bindings.
            Err(error) if *schema != SCHEMA_PREFIX && error.contains("No such schema") => {}
            Err(error) => return Err(error),
        }
    }
    Ok(bindings)
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
    fn reads_muffin_bindings_from_their_child_schema() {
        // `gsettings list-recursively` on the parent prints only its own keys,
        // so muffin's `.wm` bindings are read from their own schema.
        let bindings = bindings_from_listings(|schema| match schema {
            "org.cinnamon.desktop.keybindings" => Ok(
                "org.cinnamon.desktop.keybindings looking-glass-keybinding ['<Super>l']\n".into(),
            ),
            "org.cinnamon.desktop.keybindings.wm" => {
                Ok("org.cinnamon.desktop.keybindings.wm toggle-maximized ['<Super>q']\n".into())
            }
            _ => Err(
                "gsettings: No such schema 'org.cinnamon.desktop.keybindings.media-keys'".into(),
            ),
        })
        .unwrap();
        assert!(bindings.iter().any(|binding| {
            binding.combo == "super+q".parse().unwrap()
                && binding.action == "org.cinnamon.desktop.keybindings.wm toggle-maximized"
        }));
        assert!(
            bindings
                .iter()
                .any(|binding| binding.combo == "super+l".parse().unwrap())
        );
        // A missing parent schema is still an error.
        assert!(bindings_from_listings(|_| Err("gsettings: boom".into())).is_err());
    }

    #[test]
    fn ignores_empty_and_unknown_accelerators() {
        assert!(parse_accelerator("<Super>").is_none());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(quoted_values("[]").is_empty());
    }

    #[test]
    fn parses_custom_keybinding_values() {
        let rows = gsettings::custom_binding_rows(
            &CUSTOM,
            "custom0",
            "['<Super>t']",
            "'touch /tmp/fired'",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].combo, "super+t".parse().unwrap());
        assert_eq!(rows[0].command.as_deref(), Some("touch /tmp/fired"));
        assert!(gsettings::custom_binding_rows(&CUSTOM, "custom1", "@as []", "''").is_empty());
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
