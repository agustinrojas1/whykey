use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};

/// One normalized GSettings shortcut: the key plus the exact inspect detail
/// line the owning adapter formats for it at load time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GsettingsBinding {
    pub combo: KeyCombo,
    pub action: String,
    pub command: Option<String>,
    pub detail: String,
}

/// Per-desktop strings for the shared GSettings inspect/inventory engine.
/// Every string is copied verbatim from the original adapter so user-visible
/// output stays byte-for-byte identical. `source_details` is a list because
/// MATE reports one source line per schema.
pub(crate) struct GsettingsMeta {
    pub layer: &'static str,
    pub error_prefix: &'static str,
    pub unavailable_summary: &'static str,
    pub no_match_summary: &'static str,
    pub match_summary: &'static str,
    pub source_details: &'static [&'static str],
    pub inventory_source: &'static str,
    pub inventory_certainty: &'static str,
    pub inventory_context: &'static str,
}

pub(crate) fn inspect(
    meta: &GsettingsMeta,
    loaded: Result<Vec<GsettingsBinding>, String>,
    key: &KeyCombo,
) -> LayerResult {
    let bindings = match loaded {
        Ok(bindings) => bindings,
        Err(error) => {
            return LayerResult::unavailable(
                meta.layer,
                LayerId::Compositor,
                meta.unavailable_summary,
                vec![error],
            );
        }
    };
    let matches = bindings
        .iter()
        .filter(|binding| binding.combo == *key)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return LayerResult::pass(
            meta.layer,
            LayerId::Compositor,
            meta.no_match_summary,
            meta.source_details
                .iter()
                .map(|line| line.to_string())
                .collect(),
        );
    }
    let mut details: Vec<String> = meta
        .source_details
        .iter()
        .map(|line| line.to_string())
        .collect();
    for binding in matches {
        details.push(binding.detail.clone());
    }
    LayerResult::new(
        meta.layer,
        LayerId::Compositor,
        Outcome::Consumed,
        meta.match_summary,
        details,
    )
}

pub(crate) fn inventory(
    meta: &GsettingsMeta,
    loaded: Result<Vec<GsettingsBinding>, String>,
) -> Result<Vec<BindingRecord>, String> {
    loaded
        .map_err(|error| format!("{}{error}", meta.error_prefix))
        .map(|bindings| {
            bindings
                .into_iter()
                .map(|binding| {
                    let GsettingsBinding {
                        combo,
                        action,
                        command,
                        ..
                    } = binding;
                    BindingRecord::new(
                        meta.inventory_source,
                        combo.compact_display(),
                        command.map_or(action.clone(), |c| format!("{action}: {c}")),
                        meta.inventory_certainty,
                    )
                    .with_context(meta.inventory_context)
                })
                .collect()
        })
}

pub(crate) fn run_list(args: &[&str]) -> Result<String, String> {
    let mut gsettings = Command::new("gsettings");
    gsettings.args(args);
    let output = command::output(&mut gsettings).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(command_error("gsettings", output.status, &output.stderr));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gsettings returned invalid UTF-8: {error}"))
}

pub(crate) fn command_error(
    program: &str,
    status: std::process::ExitStatus,
    stderr: &[u8],
) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    if message.is_empty() {
        format!("{program} exited with {status}")
    } else {
        format!("{program}: {message}")
    }
}

pub(crate) fn quoted_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.trim().chars() {
        if let Some(active_quote) = quote {
            if escaped {
                current.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                values.push(std::mem::take(&mut current));
                quote = None;
            } else {
                current.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        }
    }
    if values.is_empty() {
        let value = value.trim();
        if !value.is_empty() && !value.starts_with('@') && value != "[]" {
            values.push(value.trim_matches(['[', ']', ' ', '\n']).to_owned());
        }
    }
    values
}

pub(crate) fn parse_accelerator(value: &str) -> Option<KeyCombo> {
    parse_accelerator_impl(value, false)
}

/// GNOME's exact historical modifier table: it never accepted `ctl`, `meta`,
/// or `numlock`, and folded `mod3` into `hyper`. Kept byte-faithful so exotic
/// GNOME triggers keep their old skip-or-match behavior.
pub(crate) fn parse_gnome_accelerator(value: &str) -> Option<KeyCombo> {
    parse_accelerator_impl(value, true)
}

fn parse_accelerator_impl(value: &str, gnome: bool) -> Option<KeyCombo> {
    let mut remainder = value.trim();
    let mut modifiers = Vec::new();
    while remainder.starts_with('<') {
        let end = remainder.find('>')?;
        let modifier = &remainder[1..end];
        let normalized = match modifier.to_ascii_lowercase().as_str() {
            "control" | "ctrl" | "primary" => "ctrl",
            "ctl" if !gnome => "ctrl",
            "alt" | "mod1" => "alt",
            "shift" => "shift",
            "super" | "win" | "mod4" => "super",
            "meta" if !gnome => "super",
            "hyper" if gnome => "hyper",
            "hyper" => "mod3",
            "mod3" if gnome => "hyper",
            "mod3" => "mod3",
            "mod2" | "num" => "mod2",
            "numlock" if !gnome => "mod2",
            "mod5" => "mod5",
            _ => return None,
        };
        modifiers.push(normalized);
        remainder = remainder[end + 1..].trim();
    }
    if !modifiers.is_empty() {
        if remainder.is_empty() {
            return None;
        }
        return format!("{}+{remainder}", modifiers.join("+")).parse().ok();
    }
    remainder.parse().ok()
}
