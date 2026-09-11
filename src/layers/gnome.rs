use std::env;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::gsettings::{self, GsettingsBinding, GsettingsMeta};
use crate::layers::{BindingRecord, LayerResult};

const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";

/// Read-only GNOME global shortcut adapter.
pub struct Gnome;

static META: GsettingsMeta = GsettingsMeta {
    layer: "GNOME",
    error_prefix: "GNOME: ",
    unavailable_summary: "could not inspect GNOME global shortcuts",
    no_match_summary: "no active GNOME global shortcut found",
    match_summary: "GNOME global shortcut consumes the key",
    source_details: &["source: gsettings org.gnome.settings-daemon.plugins.media-keys"],
    inventory_source: "GNOME",
    inventory_certainty: "configured",
    inventory_context: "global media/custom shortcut",
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
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("gnome"))
        })
        || env::var_os("GNOME_DESKTOP_SESSION_ID").is_some()
}

pub fn ipc_available() -> bool {
    gsettings::run_list(&["list-recursively", MEDIA_KEYS_SCHEMA]).is_ok()
}

/// Return GNOME bindings as normalized values for the global inventory.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    gsettings::inventory(&META, load_bindings())
}

impl Gnome {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        gsettings::inspect(&META, load_bindings(), key)
    }
}

fn load_bindings() -> Result<Vec<GsettingsBinding>, String> {
    let output = gsettings::run_list(&["list-recursively", MEDIA_KEYS_SCHEMA])?;
    let mut bindings = Vec::new();
    let mut custom_paths = Vec::new();
    for line in output.lines() {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if schema != MEDIA_KEYS_SCHEMA {
            continue;
        }
        if name == "custom-keybindings" {
            custom_paths.extend(
                gsettings::quoted_values(value)
                    .into_iter()
                    .filter(|path| path.starts_with('/')),
            );
            continue;
        }
        for raw_binding in gsettings::quoted_values(value) {
            if let Some(combo) = gsettings::parse_gnome_accelerator(&raw_binding) {
                bindings.push(GsettingsBinding {
                    combo,
                    action: name.to_owned(),
                    command: None,
                    detail: format!("binding: {name} (media-keys)"),
                });
            }
        }
    }

    for path in custom_paths {
        let schema = format!("{MEDIA_KEYS_SCHEMA}.custom-keybinding:{path}");
        let Some(raw_binding) = run_gsettings_get(&schema, "binding")? else {
            continue;
        };
        let Some(combo) = gsettings::quoted_values(&raw_binding)
            .into_iter()
            .find_map(|value| gsettings::parse_gnome_accelerator(&value))
        else {
            continue;
        };
        let name = run_gsettings_get(&schema, "name")?
            .and_then(|value| gsettings::quoted_values(&value).into_iter().next())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "custom shortcut".into());
        let command = run_gsettings_get(&schema, "command")?
            .and_then(|value| gsettings::quoted_values(&value).into_iter().next())
            .filter(|value| !value.is_empty());
        let detail = format!(
            "binding: {name}{}{}",
            command
                .as_deref()
                .map(|command| format!(" — command: {command}"))
                .unwrap_or_default(),
            " (custom-keybinding)"
        );
        bindings.push(GsettingsBinding {
            combo,
            action: name,
            command,
            detail,
        });
    }
    Ok(bindings)
}

fn run_gsettings_get(schema: &str, key: &str) -> Result<Option<String>, String> {
    let mut command = Command::new("gsettings");
    command.args(["get", schema, key]);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    String::from_utf8(output.stdout)
        .map(Some)
        .map_err(|error| format!("gsettings returned invalid UTF-8: {error}"))
}

#[cfg(test)]
mod tests {
    use crate::layers::gsettings::{parse_gnome_accelerator as parse_accelerator, quoted_values};

    #[test]
    fn parses_gnome_modifier_notation() {
        assert_eq!(
            parse_accelerator("<Primary><Alt>Left").unwrap(),
            "ctrl+alt+left".parse().unwrap()
        );
        assert_eq!(
            parse_accelerator("<Super>Return").unwrap(),
            "super+return".parse().unwrap()
        );
        assert_eq!(
            parse_accelerator("<BogusMod>Left"),
            None,
            "an unknown modifier never becomes a false positive binding"
        );
    }
    #[test]
    fn keeps_gnome_historical_modifier_table() {
        // GNOME never accepted these aliases; they must keep skipping.
        assert_eq!(parse_accelerator("<Meta>x"), None);
        assert_eq!(
            parse_accelerator("<Control>x").unwrap(),
            "ctrl+x".parse().unwrap()
        );
        // GNOME folded mod3 into hyper, unlike the shared table.
        assert_eq!(
            parse_accelerator("<mod3>x").unwrap(),
            "hyper+x".parse().unwrap()
        );
    }
    #[test]
    fn extracts_gsettings_strings() {
        assert_eq!(
            quoted_values("['<Super>l', '<Control><Alt>t']"),
            vec!["<Super>l", "<Control><Alt>t"]
        );
        assert!(quoted_values("@as []").is_empty());
    }
}
