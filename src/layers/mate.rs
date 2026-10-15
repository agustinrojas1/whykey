use std::collections::HashMap;

use crate::key::KeyCombo;
use crate::layers::gsettings::{self, GsettingsBinding, GsettingsMeta};
use crate::layers::{BindingRecord, LayerResult};

/// Read-only MATE global keyboard-shortcut adapter.
pub struct Mate;

const SCHEMAS: &[&str] = &[
    "org.mate.Marco.global-keybindings",
    "org.mate.SettingsDaemon.plugins.media-keys",
];

/// Marco's custom shortcuts: `run-command-N` in the global keybindings holds
/// the accelerator, and `command-N` here holds the command it runs (MATE 1.28).
const COMMANDS_SCHEMA: &str = "org.mate.Marco.keybinding-commands";

static META: GsettingsMeta = GsettingsMeta {
    layer: "MATE",
    error_prefix: "MATE: ",
    unavailable_summary: "could not inspect MATE global shortcuts",
    no_match_summary: "no active MATE global shortcut found",
    match_summary: "MATE global shortcut consumes the key",
    source_details: &[
        "source: gsettings list-recursively org.mate.Marco.global-keybindings",
        "source: gsettings list-recursively org.mate.Marco.keybinding-commands",
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
    crate::recording::env_var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(crate::recording::env_var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("mate"))
        })
        || crate::recording::env_var_os("MATE_DESKTOP_SESSION_ID").is_some()
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
    let commands = match gsettings::run_list(&["list-recursively", COMMANDS_SCHEMA]) {
        Ok(output) => parse_commands(&output),
        Err(error) => {
            errors.push(mate_error(COMMANDS_SCHEMA, error));
            HashMap::new()
        }
    };
    for &schema in SCHEMAS {
        match gsettings::run_list(&["list-recursively", schema]) {
            Ok(output) => bindings.extend(parse_bindings(schema, &output, &commands)),
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

/// `(key, value)` rows of `schema` in `gsettings list-recursively` output.
fn schema_rows<'a>(schema: &'a str, content: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> {
    content.lines().filter_map(move |line| {
        let mut fields = line.trim().splitn(3, char::is_whitespace);
        let row_schema = fields.next()?;
        let name = fields.next()?;
        let value = fields.next()?;
        (row_schema == schema).then_some((name, value.trim()))
    })
}

/// Map `command-N` to its command. The unset default is a single space.
fn parse_commands(content: &str) -> HashMap<String, String> {
    schema_rows(COMMANDS_SCHEMA, content)
        .filter_map(|(name, value)| {
            let command = gsettings::quoted_values(value).into_iter().next()?;
            (!command.trim().is_empty()).then(|| (name.to_owned(), command))
        })
        .collect()
}

/// Parse one MATE schema. A `run-command-N` binding runs the command stored
/// under `command-N`; other keys carry no command.
fn parse_bindings(
    schema: &str,
    content: &str,
    commands: &HashMap<String, String>,
) -> Vec<GsettingsBinding> {
    let mut bindings = Vec::new();
    for (name, value) in schema_rows(schema, content) {
        let command = name
            .strip_prefix("run-command-")
            .and_then(|number| commands.get(&format!("command-{number}")))
            .cloned();
        for raw_binding in gsettings::quoted_values(value) {
            // "disabled" is the schema's unbound sentinel, not a key.
            if raw_binding == "disabled" {
                continue;
            }
            let Some(combo) = gsettings::parse_accelerator(&raw_binding) else {
                continue;
            };
            let action = format!("{schema} {name}");
            bindings.push(GsettingsBinding {
                combo,
                action: action.clone(),
                command: command.clone(),
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
            "org.mate.Marco.global-keybindings run-command-1 '<Alt>F2'\n\
             org.mate.Marco.global-keybindings cycle-group '<Alt>F6'\n",
            &HashMap::new(),
        );
        assert_eq!(marco.len(), 2);
        assert_eq!(marco[0].combo, "alt+f2".parse().unwrap());
        assert_eq!(marco[0].command, None);
        assert_eq!(marco[1].combo, "alt+f6".parse().unwrap());
        assert_eq!(
            marco[1].action,
            "org.mate.Marco.global-keybindings cycle-group"
        );
        let media = parse_bindings(
            SCHEMAS[1],
            "org.mate.SettingsDaemon.plugins.media-keys screensaver ['<Super>l']\n",
            &HashMap::new(),
        );
        assert_eq!(media[0].combo, "super+l".parse().unwrap());
    }

    #[test]
    fn run_command_binding_takes_command_from_keybinding_commands() {
        let commands = parse_commands(
            "org.mate.Marco.keybinding-commands command-1 'touch /tmp/fired'\n\
             org.mate.Marco.keybinding-commands command-2 ' '\n",
        );
        assert_eq!(
            commands.get("command-1").map(String::as_str),
            Some("touch /tmp/fired")
        );
        assert!(!commands.contains_key("command-2"));
        let marco = parse_bindings(
            SCHEMAS[0],
            "org.mate.Marco.global-keybindings run-command-1 '<Super>t'\n\
             org.mate.Marco.global-keybindings run-command-2 '<Super>y'\n\
             org.mate.Marco.global-keybindings run-command-3 'disabled'\n",
            &commands,
        );
        assert_eq!(marco.len(), 2);
        assert_eq!(marco[0].combo, "super+t".parse().unwrap());
        assert_eq!(
            marco[0].action,
            "org.mate.Marco.global-keybindings run-command-1"
        );
        assert_eq!(marco[0].command.as_deref(), Some("touch /tmp/fired"));
        assert_eq!(marco[1].combo, "super+y".parse().unwrap());
        assert_eq!(marco[1].command, None);
    }

    #[test]
    fn ignores_disabled_and_unknown_values() {
        assert!(quoted_values("[]").is_empty());
        assert!(parse_accelerator("<Unknown>r").is_none());
        assert!(parse_accelerator("<Alt>").is_none());
    }
}
