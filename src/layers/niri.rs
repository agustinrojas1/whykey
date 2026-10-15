use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, EvidenceTier, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;
/// Niri's own include limit ("includes cannot be N levels deep").
const MAX_INCLUDE_DEPTH: usize = 10;
/// Output name niri gives its winit backend, which runs it nested in another
/// compositor. A TTY (DRM) session names outputs after connectors instead.
const NESTED_OUTPUT: &str = "winit";

/// Read-only Niri `config.kdl` binding adapter.
pub struct Niri;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let loaded = load_config().map_err(|error| format!("Niri: {error}"))?;
    let mods = mod_candidates(&loaded);
    let mut records = Vec::new();
    for binding in &loaded.bindings {
        for combo in binding.combos(&mods.modifiers) {
            records.push(
                BindingRecord::new(
                    "Niri",
                    combo.compact_display(),
                    binding.action.clone(),
                    "configured; runtime activation conditional",
                )
                .with_context("config.kdl binds block"),
            );
        }
    }
    Ok(records)
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
                .any(|name| name.trim().eq_ignore_ascii_case("niri"))
        })
        || env::var_os("NIRI_SOCKET").is_some()
        || config_path().is_some_and(|path| path.is_file())
}

/// Niri does not expose a stable global binding inventory command across
/// releases; a readable config is therefore the available source.
pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.is_file())
}

impl Niri {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let loaded = match load_config() {
            Ok(loaded) => loaded,
            Err(error) => {
                return LayerResult::new(
                    "Niri",
                    LayerId::Compositor,
                    Outcome::Unavailable,
                    "could not inspect Niri key bindings",
                    vec![error],
                );
            }
        };
        let mods = mod_candidates(&loaded);
        if let Some(error) = mods.error.clone() {
            return LayerResult::new(
                "Niri",
                LayerId::Compositor,
                Outcome::Unavailable,
                "could not inspect Niri key bindings",
                vec![error],
            );
        }

        let mut matches = Vec::new();
        for binding in &loaded.bindings {
            if binding.combos(&mods.modifiers).contains(key) {
                matches.push(binding);
            }
        }

        let mut details = vec![format!("source: {}", sources_description(&loaded))];
        details.extend(mods.notes.iter().cloned());
        details.extend(
            loaded
                .gaps
                .iter()
                .map(|gap| format!("not inspected: {gap}")),
        );
        if matches.is_empty() {
            let (outcome, summary) = if loaded.gaps.is_empty() {
                (
                    Outcome::Pass,
                    "no matching Niri binding found in config.kdl",
                )
            } else {
                (
                    Outcome::Unknown,
                    "a Niri binding could not be read, so no match can be ruled out",
                )
            };
            return LayerResult::new("Niri", LayerId::Compositor, outcome, summary, details)
                .with_tier(EvidenceTier::ConfigFile);
        }
        for binding in matches {
            details.push(format!(
                "binding: {} [{}]",
                binding.action,
                binding.source.display()
            ));
        }
        details.push(
            "Niri runtime reload state, inhibitor state, and active mode remain conditional; \
             niri's IPC does not report its active binds"
                .into(),
        );
        LayerResult::new(
            "Niri",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "Niri config contains a matching binding",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile)
    }
}

/// One bind as written: literal modifiers, and whether it uses `Mod`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    modifiers: Vec<&'static str>,
    uses_mod: bool,
    key: String,
    action: String,
    source: PathBuf,
}

impl Binding {
    /// The combinations this bind answers to. `Mod` stands for each modifier
    /// in `mods` (more than one while the backend is unknown).
    fn combos(&self, mods: &[&str]) -> Vec<KeyCombo> {
        let candidates: Vec<Option<&str>> = if self.uses_mod {
            mods.iter().map(|modifier| Some(*modifier)).collect()
        } else {
            vec![None]
        };
        candidates
            .into_iter()
            .filter_map(|mod_name| {
                let mut parts: Vec<&str> = self.modifiers.clone();
                if let Some(modifier) = mod_name {
                    parts.push(modifier);
                }
                parts.push(&self.key);
                parts.join("+").parse::<KeyCombo>().ok()
            })
            .collect()
    }
}

#[derive(Debug, Default)]
struct Loaded {
    bindings: Vec<Binding>,
    /// `mod-key`: what `Mod` means on a TTY (niri default: Super).
    mod_key: Option<String>,
    /// `mod-key-nested`: what `Mod` means under winit (niri default: Alt).
    mod_key_nested: Option<String>,
    /// Binds or settings whykey could not read.
    gaps: Vec<String>,
    sources: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct ModCandidates {
    /// Modifier names `Mod` may stand for; more than one when unknown.
    modifiers: Vec<&'static str>,
    notes: Vec<String>,
    error: Option<String>,
}

/// Which modifier `Mod` is, from `mod-key` / `mod-key-nested` and the
/// backend. The backend comes from niri's IPC: under winit the output is named
/// `winit`. Only read when some bind uses `Mod`.
fn mod_candidates(loaded: &Loaded) -> ModCandidates {
    if !loaded.bindings.iter().any(|binding| binding.uses_mod) {
        return ModCandidates::default();
    }
    let tty = match loaded.mod_key.as_deref() {
        None => "super",
        Some(value) => match modifier_name(value) {
            Some(name) => name,
            None => {
                return ModCandidates {
                    error: Some(format!(
                        "mod-key \"{value}\" is not a modifier whykey understands"
                    )),
                    ..ModCandidates::default()
                };
            }
        },
    };
    let nested = match loaded.mod_key_nested.as_deref() {
        None => "alt",
        Some(value) => match modifier_name(value) {
            Some(name) => name,
            None => {
                return ModCandidates {
                    error: Some(format!(
                        "mod-key-nested \"{value}\" is not a modifier whykey understands"
                    )),
                    ..ModCandidates::default()
                };
            }
        },
    };
    match output_names() {
        Ok(names) if names.iter().any(|name| name == NESTED_OUTPUT) => ModCandidates {
            modifiers: vec![nested],
            notes: vec![format!(
                "Mod: niri runs nested (output {NESTED_OUTPUT}), so Mod is {nested}"
            )],
            error: None,
        },
        Ok(names) if !names.is_empty() => ModCandidates {
            modifiers: vec![tty],
            notes: vec![format!("Mod: niri runs on a TTY, so Mod is {tty}")],
            error: None,
        },
        Ok(_) => ModCandidates {
            modifiers: unique(&[tty, nested]),
            notes: vec![
                "Mod: niri reports no outputs, so the backend is unknown and Mod could be either modifier"
                    .into(),
            ],
            error: None,
        },
        Err(error) => ModCandidates {
            modifiers: unique(&[tty, nested]),
            notes: vec![format!(
                "Mod: the backend is unknown ({error}), so Mod could be either modifier"
            )],
            error: None,
        },
    }
}

fn unique(values: &[&'static str]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for value in values {
        if !out.contains(value) {
            out.push(*value);
        }
    }
    out
}

/// The output names of the running niri (`niri msg --json outputs`).
fn output_names() -> Result<Vec<String>, String> {
    let output = Command::new("niri")
        .args(["msg", "--json", "outputs"])
        .output()
        .map_err(|error| format!("niri msg: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "niri msg outputs exited {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("niri msg outputs: {error}"))?;
    value
        .as_object()
        .map(|outputs| outputs.keys().cloned().collect())
        .ok_or_else(|| "niri msg outputs is not an object".to_owned())
}

/// The modifier a `mod-key` value names.
fn modifier_name(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "super" | "mod4" | "win" | "meta" => Some("super"),
        "alt" | "mod1" => Some("alt"),
        "ctrl" | "control" => Some("ctrl"),
        "shift" => Some("shift"),
        _ => None,
    }
}

fn load_config() -> Result<Loaded, String> {
    let path = config_path().ok_or_else(|| "Niri config.kdl was not found".to_owned())?;
    let mut loaded = Loaded::default();
    read_into(&path, 0, &mut loaded)?;
    Ok(loaded)
}

/// Read one file and, depth first, the files it includes. Includes resolve
/// against the including file's directory, as niri resolves them.
fn read_into(path: &Path, depth: usize, loaded: &mut Loaded) -> Result<(), String> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(format!(
            "{} is included more than {MAX_INCLUDE_DEPTH} levels deep",
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_CONFIG_BYTES}-byte config limit",
            path.display()
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    loaded.sources.push(path.to_path_buf());
    let parsed = parse_document(&content, path);
    if loaded.mod_key.is_none() {
        loaded.mod_key = parsed.mod_key;
    }
    if loaded.mod_key_nested.is_none() {
        loaded.mod_key_nested = parsed.mod_key_nested;
    }
    loaded.gaps.extend(parsed.gaps);
    for binding in parsed.bindings {
        if loaded.bindings.len() >= MAX_BINDINGS {
            loaded.gaps.push(format!("more than {MAX_BINDINGS} binds"));
            break;
        }
        loaded.bindings.push(binding);
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for include in parsed.includes {
        let child = base.join(&include.path);
        if include.optional && !child.exists() {
            continue;
        }
        read_into(&child, depth + 1, loaded)?;
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Document {
    bindings: Vec<Binding>,
    includes: Vec<Include>,
    mod_key: Option<String>,
    mod_key_nested: Option<String>,
    gaps: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Include {
    path: String,
    optional: bool,
}

/// Top-level `include`, `mod-key`, `mod-key-nested` and the `binds` block.
fn parse_document(content: &str, source: &Path) -> Document {
    let mut document = Document::default();
    let mut depth = 0_i32;
    let mut in_binds = false;
    let mut pending: Option<(String, String)> = None;
    for raw_line in content.lines() {
        let line = raw_line.split_once("//").map_or(raw_line, |(line, _)| line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if in_binds {
            parse_bind_line(trimmed, source, &mut document, &mut pending);
        } else {
            // `mod-key` and `mod-key-nested` are children of `input`; niri
            // rejects them at the top level.
            if let Some(value) = trimmed.strip_prefix("mod-key-nested") {
                document.mod_key_nested = quoted_value(value);
            } else if let Some(value) = trimmed.strip_prefix("mod-key") {
                document.mod_key = quoted_value(value);
            }
            if depth == 0 {
                if let Some(rest) = trimmed.strip_prefix("include") {
                    match parse_include(rest) {
                        Some(include) => document.includes.push(include),
                        None => document.gaps.push(format!(
                            "{}: unreadable include {trimmed}",
                            source.display()
                        )),
                    }
                } else if trimmed.starts_with("binds") && trimmed.contains('{') {
                    in_binds = true;
                }
            }
        }
        depth += brace_delta(trimmed);
        if in_binds && depth <= 0 && pending.is_none() {
            in_binds = false;
            depth = 0;
        }
    }
    document
}

fn parse_bind_line(
    trimmed: &str,
    source: &Path,
    document: &mut Document,
    pending: &mut Option<(String, String)>,
) {
    if let Some((raw_key, action)) = pending.as_mut() {
        action.push(' ');
        action.push_str(trimmed);
        if trimmed.contains('}') {
            let action = action.trim_end_matches('}').trim().trim_matches(';').trim();
            let raw_key = raw_key.clone();
            let action = action.to_owned();
            *pending = None;
            if !action.is_empty() {
                push_bind(document, &raw_key, action, source);
            }
        }
        return;
    }
    if trimmed.starts_with("include") {
        document.gaps.push(format!(
            "{}: include inside binds is not read",
            source.display()
        ));
        return;
    }
    let Some(open) = trimmed.find('{') else {
        return;
    };
    let Some(raw_key) = trimmed[..open].split_whitespace().next() else {
        return;
    };
    let rest = trimmed[open + 1..].trim();
    if rest.contains('}') {
        let action = rest
            .trim_end_matches('}')
            .trim()
            .trim_matches(';')
            .trim()
            .to_owned();
        if !action.is_empty() {
            push_bind(document, raw_key, action, source);
        }
    } else {
        *pending = Some((raw_key.to_owned(), rest.to_owned()));
    }
}

fn push_bind(document: &mut Document, raw_key: &str, action: String, source: &Path) {
    match parse_accelerator(raw_key) {
        Some((modifiers, uses_mod, key)) => document.bindings.push(Binding {
            modifiers,
            uses_mod,
            key,
            action,
            source: source.to_path_buf(),
        }),
        None => document.gaps.push(format!(
            "{}: bind {raw_key} is not a key combination whykey reads",
            source.display()
        )),
    }
}

fn brace_delta(value: &str) -> i32 {
    value.chars().fold(0, |delta, character| match character {
        '{' => delta + 1,
        '}' => delta - 1,
        _ => delta,
    })
}

/// `"value"` after a node name, or `None`.
fn quoted_value(rest: &str) -> Option<String> {
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_owned())
}

/// `include "path"` or `include optional=true "path"`.
fn parse_include(rest: &str) -> Option<Include> {
    let path = quoted_value(rest)?;
    if path.is_empty() {
        return None;
    }
    let optional = rest
        .split('"')
        .next()
        .is_some_and(|properties| properties.replace(' ', "").contains("optional=true"));
    Some(Include { path, optional })
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("NIRI_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = crate::util::config_base()?;
    Some(base.join("niri/config.kdl"))
}

fn sources_description(loaded: &Loaded) -> String {
    if loaded.sources.is_empty() {
        return "Niri config.kdl".into();
    }
    loaded
        .sources
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse an accelerator such as `Mod+Shift+Q` into its literal modifiers, the
/// `Mod` flag and the key. `None` for a dynamic or unrecognized one.
fn parse_accelerator(value: &str) -> Option<(Vec<&'static str>, bool, String)> {
    if value.contains(['{', '}', '"']) {
        return None;
    }
    let mut parts = value.split('+').collect::<Vec<_>>();
    let key = parts.pop()?.trim();
    if key.is_empty() || key.eq_ignore_ascii_case("mod") && parts.is_empty() {
        return None;
    }
    let mut modifiers = Vec::new();
    let mut uses_mod = false;
    for modifier in parts {
        match modifier.trim().to_ascii_lowercase().as_str() {
            "mod" => uses_mod = true,
            "super" | "meta" | "mod4" | "win" => modifiers.push("super"),
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "mod1" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "capslock" | "caps" => modifiers.push("caps"),
            "numlock" | "num" => modifiers.push("mod2"),
            _ => return None,
        }
    }
    Some((modifiers, uses_mod, key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    fn bindings_of(content: &str) -> Vec<Binding> {
        parse_document(content, Path::new("config.kdl")).bindings
    }

    #[test]
    fn parses_single_and_multiline_niri_bindings() {
        let bindings = bindings_of(
            "binds {\n  Mod+Return { spawn \"foot\"; }\n  Mod+Shift+Q {\n    close-window\n  }\n}\n",
        );
        assert_eq!(bindings.len(), 2);
        assert!(bindings[0].uses_mod);
        assert_eq!(bindings[0].key, "Return");
        assert!(bindings[0].action.contains("spawn"));
        assert_eq!(bindings[1].modifiers, vec!["shift"]);
        assert_eq!(bindings[1].key, "Q");
    }

    #[test]
    fn bind_properties_before_the_block_are_ignored() {
        let bindings = bindings_of(
            "binds {\n  Super+T allow-when-locked=true hotkey-overlay-title=null { spawn \"foot\"; }\n  Super+U repeat=false { close-window; }\n}\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combos(&[]), vec![combo("super+t")]);
        assert_eq!(bindings[1].combos(&[]), vec![combo("super+u")]);
    }

    #[test]
    fn mod_takes_the_candidate_modifier() {
        let bind = &bindings_of("binds {\n  Mod+T { spawn \"foot\"; }\n}\n")[0];
        assert_eq!(bind.combos(&["super"]), vec![combo("super+t")]);
        assert_eq!(bind.combos(&["alt"]), vec![combo("alt+t")]);
        assert_eq!(
            bind.combos(&["super", "alt"]),
            vec![combo("super+t"), combo("alt+t")]
        );
    }

    #[test]
    fn rejects_dynamic_or_modifier_only_accelerators() {
        assert!(parse_accelerator("Mod+{key}").is_none());
        assert!(parse_accelerator("Mod").is_none());
        assert!(parse_accelerator("Frob+T").is_none());
        assert!(parse_accelerator("Mod+Ctrl+Q").is_some());
    }

    #[test]
    fn reads_mod_key_settings_and_includes() {
        let document = parse_document(
            "mod-key \"Alt\"\nmod-key-nested \"Super\"\ninclude \"binds.kdl\"\ninclude optional=true \"local.kdl\"\n",
            Path::new("config.kdl"),
        );
        assert_eq!(document.mod_key.as_deref(), Some("Alt"));
        assert_eq!(document.mod_key_nested.as_deref(), Some("Super"));
        assert_eq!(
            document.includes,
            vec![
                Include {
                    path: "binds.kdl".into(),
                    optional: false
                },
                Include {
                    path: "local.kdl".into(),
                    optional: true
                },
            ]
        );
    }

    #[test]
    fn mod_key_is_read_inside_the_input_block() {
        let document = parse_document(
            "input {\n    mod-key-nested \"Super\"\n    mod-key \"Alt\"\n}\nbinds {\n  Mod+T { spawn \"foot\"; }\n}\n",
            Path::new("config.kdl"),
        );
        assert_eq!(document.mod_key_nested.as_deref(), Some("Super"));
        assert_eq!(document.mod_key.as_deref(), Some("Alt"));
        assert_eq!(document.bindings.len(), 1);
    }

    #[test]
    fn unreadable_binds_are_gaps_not_silent() {
        let document = parse_document(
            "binds {\n  Frob+T { spawn \"x\"; }\n  Mod+{key} { spawn \"y\"; }\n}\n",
            Path::new("config.kdl"),
        );
        assert!(document.bindings.is_empty());
        assert_eq!(document.gaps.len(), 2);
    }

    #[test]
    fn mod_key_values_name_modifiers() {
        assert_eq!(modifier_name("Super"), Some("super"));
        assert_eq!(modifier_name("alt"), Some("alt"));
        assert_eq!(modifier_name("ISO_Level3_Shift"), None);
    }

    #[test]
    fn includes_of_a_document_are_loaded_from_their_directory() {
        let dir = std::env::temp_dir().join(format!("whykey-niri-include-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.kdl"), "include \"binds.kdl\"\n").unwrap();
        fs::write(
            dir.join("binds.kdl"),
            "binds {\n  Super+T { spawn \"foot\"; }\n}\n",
        )
        .unwrap();
        let mut loaded = Loaded::default();
        read_into(&dir.join("config.kdl"), 0, &mut loaded).unwrap();
        assert_eq!(loaded.bindings.len(), 1);
        assert_eq!(loaded.bindings[0].combos(&[]), vec![combo("super+t")]);
        assert_eq!(loaded.sources.len(), 2);
        let _ = fs::remove_dir_all(dir);
    }
}
