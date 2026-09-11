use std::env;

use crate::key::KeyCombo;
use crate::layers::gsettings::{self, GsettingsBinding, GsettingsMeta};
use crate::layers::{BindingRecord, LayerResult};

/// Read-only MATE global keyboard-shortcut adapter.
pub struct Mate;

const SCHEMAS: &[&str] = &[
    "org.mate.Marco.global-keybindings",
    "org.mate.SettingsDaemon.plugins.media-keys",
];

static META: GsettingsMeta = GsettingsMeta {
    layer: "MATE",
    error_prefix: "MATE: ",
    unavailable_summary: "could not inspect MATE global shortcuts",
    no_match_summary: "no active MATE global shortcut found",
    match_summary: "MATE global shortcut consumes the key",
    source_details: &[
        "source: gsettings list-recursively org.mate.Marco.global-keybindings",
        "source: gsettings list-recursively org.mate.SettingsDaemon.plugins.media-keys",
    ],
    inventory_source: "MATE",
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
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("mate"))
        })
        || env::var_os("MATE_DESKTOP_SESSION_ID").is_some()
}

pub fn ipc_available() -> bool {
    SCHEMAS
        .iter()
        .any(|&schema| gsettings::run_list(&["list-recursively", schema]).is_ok())
}

/// Return MATE's live GSettings shortcut values as normalized entries.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let (bindings, errors) = load_bindings();
    if bindings.is_empty() && !errors.is_empty() {
        return gsettings::inventory(&META, Err(errors.join("; ")));
    }
    gsettings::inventory(&META, Ok(bindings))
}

impl Mate {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let (bindings, errors) = load_bindings();
        if bindings.is_empty() && !errors.is_empty() {
            return gsettings::inspect(&META, Err(errors.join("; ")), key);
        }
        // Partial per-schema failures stay visible after the match details,
        // exactly as the pre-consolidation adapter reported them.
        let mut result = gsettings::inspect(&META, Ok(bindings), key);
        result.details.extend(errors);
        result
    }
}

fn load_bindings() -> (Vec<GsettingsBinding>, Vec<String>) {
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for &schema in SCHEMAS {
        match gsettings::run_list(&["list-recursively", schema]) {
            Ok(output) => bindings.extend(parse_bindings(schema, &output)),
            Err(error) => errors.push(mate_error(schema, error)),
        }
    }
    (bindings, errors)
}

/// Restore the per-schema `gsettings {schema} ...` error shape this adapter
/// historically reported now that the process runner is shared.
fn mate_error(schema: &str, error: String) -> String {
    error.replacen("gsettings", &format!("gsettings {schema}"), 1)
}

fn parse_bindings(schema: &str, content: &str) -> Vec<GsettingsBinding> {
    let mut bindings = Vec::new();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut fields = line.splitn(3, char::is_whitespace);
        let Some(row_schema) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else { continue };
        let Some(value) = fields.next() else { continue };
        if row_schema != schema {
            continue;
        }
        for raw_binding in gsettings::quoted_values(value) {
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
    fn parses_marco_and_media_key_rows() {
        let marco = parse_bindings(
            SCHEMAS[0],
            "org.mate.Marco.global-keybindings run-command-1 '<Alt>F2'\n",
        );
        assert_eq!(marco.len(), 1);
        assert_eq!(marco[0].combo, "alt+f2".parse().unwrap());
        let media = parse_bindings(
            SCHEMAS[1],
            "org.mate.SettingsDaemon.plugins.media-keys screensaver ['<Super>l']\n",
        );
        assert_eq!(media[0].combo, "super+l".parse().unwrap());
    }

    #[test]
    fn ignores_disabled_and_unknown_values() {
        assert!(quoted_values("[]").is_empty());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(parse_accelerator("<Alt>").is_none());
    }
}
