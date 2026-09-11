//! Opt-in compositor adapters described by small user-owned manifests.
//!
//! Discovery is deliberately separate from execution. Reading a manifest is
//! safe; commands are run only by the inventory and inspection paths after a
//! manifest has passed validation.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::command;
use crate::layers::compositor;
use crate::layers::{BindingRecord, LayerId, LayerResult, Outcome};
use crate::util;

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_MANIFESTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionAdapter {
    pub id: String,
    pub display: String,
    pub tier: String,
    pub manifest_path: PathBuf,
    pub applicable_env: Vec<String>,
    pub applicable_desktop: Vec<String>,
    pub bindings_cmd: Vec<String>,
    pub focused_cmd: Option<Vec<String>>,
    pub reload_gen_cmd: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryResult {
    pub records: Vec<BindingRecord>,
    pub malformed_entries: usize,
    pub error: Option<String>,
}

impl InventoryResult {
    fn unavailable(error: String) -> Self {
        Self {
            records: Vec::new(),
            malformed_entries: 0,
            error: Some(error),
        }
    }
}

/// Find and validate all user-owned compositor manifests.
pub fn discover() -> (Vec<ExtensionAdapter>, Vec<ManifestWarning>) {
    let directory = env::var_os("WHYKEY_ADAPTER_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| util::config_base().map(|base| base.join("whykey/adapters")));
    directory.map_or_else(|| (Vec::new(), Vec::new()), |path| discover_in(&path))
}

pub fn discover_in(directory: &Path) -> (Vec<ExtensionAdapter>, Vec<ManifestWarning>) {
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (Vec::new(), warnings);
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.len() > MAX_MANIFESTS {
        for path in paths.drain(MAX_MANIFESTS..) {
            warnings.push(ManifestWarning {
                path,
                message: format!("manifest directory exceeds the {MAX_MANIFESTS}-file limit"),
            });
        }
    }

    let mut adapters = Vec::new();
    let mut ids = HashSet::new();
    let builtin_ids = crate::registry::DESKTOPS
        .iter()
        .map(|entry| entry.id)
        .collect::<HashSet<_>>();
    for path in paths {
        let Some(content) = util::read_bounded(&path, MAX_MANIFEST_BYTES) else {
            warnings.push(ManifestWarning {
                path,
                message: format!(
                    "manifest is missing, unreadable, or exceeds {MAX_MANIFEST_BYTES} bytes"
                ),
            });
            continue;
        };
        match parse_manifest(&path, &content) {
            Ok((adapter, manifest_warnings)) => {
                warnings.extend(
                    manifest_warnings
                        .into_iter()
                        .map(|message| ManifestWarning {
                            path: path.clone(),
                            message,
                        }),
                );
                if builtin_ids.contains(adapter.id.as_str()) {
                    warnings.push(ManifestWarning {
                        path,
                        message: format!(
                            "manifest id '{}' shadows a built-in adapter and was ignored",
                            adapter.id
                        ),
                    });
                } else if !ids.insert(adapter.id.clone()) {
                    warnings.push(ManifestWarning {
                        path,
                        message: format!("duplicate manifest id '{}' was ignored", adapter.id),
                    });
                } else {
                    adapters.push(adapter);
                }
            }
            Err(messages) => warnings.extend(messages.into_iter().map(|message| ManifestWarning {
                path: path.clone(),
                message,
            })),
        }
    }
    (adapters, warnings)
}

pub fn applicable(adapter: &ExtensionAdapter) -> bool {
    applicable_with_context(adapter, &compositor::current_context())
}

pub fn applicable_with_context(adapter: &ExtensionAdapter, context: &compositor::Context) -> bool {
    let env_match = adapter
        .applicable_env
        .iter()
        .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()));
    let desktop_match = context.desktop.as_deref().is_some_and(|desktop| {
        desktop.split(':').any(|value| {
            adapter
                .applicable_desktop
                .iter()
                .any(|candidate| value.eq_ignore_ascii_case(candidate))
        })
    });
    env_match || desktop_match
}

/// Execute a manifest's bounded inventory command and parse its output.
pub fn inventory_report(adapter: &ExtensionAdapter) -> InventoryResult {
    let Some(program) = adapter.bindings_cmd.first() else {
        return InventoryResult::unavailable(format!("{}: bindings_cmd is empty", adapter.id));
    };
    let mut command = Command::new(program);
    command.args(&adapter.bindings_cmd[1..]);
    let output = match command::output(&mut command) {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return InventoryResult::unavailable(format_command_failure(
                adapter,
                output.status.to_string(),
                &output.stderr,
            ));
        }
        Err(error) => {
            return InventoryResult::unavailable(format!(
                "{} ({}): bindings command unavailable: {error}",
                adapter.id, adapter.display
            ));
        }
    };

    if let Ok(entries) = serde_json::from_slice::<Vec<JsonBinding>>(&output.stdout) {
        return parse_json_entries(adapter, entries);
    }
    parse_literal_entries(adapter, &String::from_utf8_lossy(&output.stdout))
}

/// Compatibility helper for callers that only need records.
pub fn binding_inventory(adapter: &ExtensionAdapter) -> Result<Vec<BindingRecord>, String> {
    let report = inventory_report(adapter);
    report.error.map_or_else(|| Ok(report.records), Err)
}

pub fn inspect(adapter: &ExtensionAdapter, key: &crate::key::KeyCombo) -> LayerResult {
    inspect_with_report(adapter, key, inventory_report(adapter))
}

pub fn inspect_with_report(
    adapter: &ExtensionAdapter,
    key: &crate::key::KeyCombo,
    report: InventoryResult,
) -> LayerResult {
    if let Some(error) = report.error {
        return LayerResult::unavailable(
            "Compositor extension",
            LayerId::Compositor,
            format!("{} inventory unavailable", adapter.display),
            vec![error],
        );
    }
    let key = key.compact_display();
    if let Some(binding) = report
        .records
        .iter()
        .find(|binding| binding.key.eq_ignore_ascii_case(&key))
    {
        let mut details = vec![
            format!("manifest: {}", adapter.manifest_path.display()),
            "runtime activation remains conditional; the manifest reports static configuration"
                .into(),
        ];
        if report.malformed_entries > 0 {
            details.push(format!(
                "{} malformed binding entr{} skipped",
                report.malformed_entries,
                if report.malformed_entries == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }
        return LayerResult::new(
            "Compositor extension",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            format!("{} reports {} for {}", adapter.display, binding.action, key),
            details,
        )
        .with_binding(crate::layers::BindingEvidence {
            dispatcher: None,
            action: Some(binding.action.clone()),
            description: binding.context.clone(),
            submap: binding.submap.clone(),
            scope: crate::layers::BindingScope::Unknown,
            source: Some(crate::layers::SourceLocation {
                file: adapter.manifest_path.display().to_string(),
                line: None,
            }),
            has_universal_match: false,
            uncertainty: Some(crate::layers::UncertaintyReason::EndpointUnavailable),
        });
    }
    LayerResult::pass(
        "Compositor extension",
        LayerId::Compositor,
        format!("{} has no binding for {key}", adapter.display),
        vec![format!("manifest: {}", adapter.manifest_path.display())],
    )
}

pub fn focused_pid(adapter: &ExtensionAdapter) -> Result<u32, String> {
    let Some(argv) = adapter.focused_cmd.as_ref() else {
        return Err(format!(
            "{} ({}): no focused_cmd",
            adapter.id, adapter.display
        ));
    };
    let Some(program) = argv.first() else {
        return Err(format!(
            "{} ({}): focused_cmd is empty",
            adapter.id, adapter.display
        ));
    };
    let mut command = Command::new(program);
    command.args(&argv[1..]);
    let output = command::output(&mut command).map_err(|error| {
        format!(
            "{} ({}): focused command unavailable: {error}",
            adapter.id, adapter.display
        )
    })?;
    if !output.status.success() {
        return Err(format_command_failure(
            adapter,
            output.status.to_string(),
            &output.stderr,
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if let Ok(pid) = text.trim().parse::<u32>() {
        return Ok(pid);
    }
    let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|error| {
        format!(
            "{} ({}): focused command returned invalid PID: {error}",
            adapter.id, adapter.display
        )
    })?;
    value
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| {
            format!(
                "{} ({}): focused command did not return a valid PID",
                adapter.id, adapter.display
            )
        })
}

#[derive(Debug, Deserialize)]
struct JsonBinding {
    key: Option<String>,
    action: Option<String>,
    context: Option<String>,
    device: Option<String>,
    submap: Option<String>,
}

fn parse_json_entries(adapter: &ExtensionAdapter, entries: Vec<JsonBinding>) -> InventoryResult {
    let mut records = Vec::new();
    let mut malformed_entries = 0;
    for entry in entries {
        let Some(raw_key) = entry.key else {
            malformed_entries += 1;
            continue;
        };
        let Some(action) = entry.action.filter(|value| !value.trim().is_empty()) else {
            malformed_entries += 1;
            continue;
        };
        let Ok(key) = raw_key.parse::<crate::key::KeyCombo>() else {
            malformed_entries += 1;
            continue;
        };
        records.push(BindingRecord {
            source: adapter.display.clone(),
            key: key.compact_display(),
            action,
            context: entry.context,
            device: entry.device,
            submap: entry.submap,
            certainty: "script-reported; runtime activation conditional".into(),
        });
    }
    InventoryResult {
        records,
        malformed_entries,
        error: None,
    }
}

fn parse_literal_entries(adapter: &ExtensionAdapter, output: &str) -> InventoryResult {
    let mut records = Vec::new();
    let mut malformed_entries = 0;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let Some((raw_key, action)) = line.split_once(char::is_whitespace) else {
            malformed_entries += 1;
            continue;
        };
        let action = action.trim();
        let Ok(key) = raw_key.parse::<crate::key::KeyCombo>() else {
            malformed_entries += 1;
            continue;
        };
        if action.is_empty() {
            malformed_entries += 1;
            continue;
        }
        records.push(BindingRecord::new(
            adapter.display.clone(),
            key.compact_display(),
            action.to_owned(),
            "script-reported; runtime activation conditional",
        ));
    }
    InventoryResult {
        records,
        malformed_entries,
        error: None,
    }
}

fn format_command_failure(adapter: &ExtensionAdapter, status: String, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let tail = if stderr.chars().count() > 500 {
        stderr
            .chars()
            .rev()
            .take(500)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        stderr
    };
    if tail.is_empty() {
        format!(
            "{} ({}): bindings command exited with {status}",
            adapter.id, adapter.display
        )
    } else {
        format!(
            "{} ({}): bindings command exited with {status}: {tail}",
            adapter.id, adapter.display
        )
    }
}

fn parse_manifest(
    path: &Path,
    content: &str,
) -> Result<(ExtensionAdapter, Vec<String>), Vec<String>> {
    let mut id = None;
    let mut display = None;
    let mut tier = None;
    let mut applicable_env = None;
    let mut applicable_desktop = None;
    let mut bindings_cmd = None;
    let mut focused_cmd = None;
    let mut reload_gen_cmd = None;
    let mut warnings = Vec::new();

    for (line_number, raw_line) in content.lines().enumerate() {
        let stripped = strip_comment(raw_line);
        let line = stripped.trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            warnings.push(format!(
                "{}:{}: expected key = value",
                path.display(),
                line_number + 1
            ));
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        let parsed = match key {
            "id" | "display" | "tier" => parse_string(value).map(Value::String),
            "applicable_env" | "applicable_desktop" | "bindings_cmd" | "focused_cmd"
            | "reload_gen_cmd" => parse_string_list(value).map(Value::List),
            _ => {
                warnings.push(format!(
                    "{}:{}: unknown manifest key '{key}'",
                    path.display(),
                    line_number + 1
                ));
                continue;
            }
        };
        let Ok(parsed) = parsed else {
            warnings.push(format!(
                "{}:{}: invalid value for '{key}'",
                path.display(),
                line_number + 1
            ));
            continue;
        };
        match (key, parsed) {
            ("id", Value::String(value)) => id = Some(value),
            ("display", Value::String(value)) => display = Some(value),
            ("tier", Value::String(value)) => tier = Some(value),
            ("applicable_env", Value::List(value)) => applicable_env = Some(value),
            ("applicable_desktop", Value::List(value)) => applicable_desktop = Some(value),
            ("bindings_cmd", Value::List(value)) => bindings_cmd = Some(value),
            ("focused_cmd", Value::List(value)) => focused_cmd = Some(value),
            ("reload_gen_cmd", Value::List(value)) => reload_gen_cmd = Some(value),
            _ => unreachable!(),
        }
    }

    let id = id.unwrap_or_default();
    let display = display.unwrap_or_default();
    let tier = tier.unwrap_or_default();
    let applicable_env = applicable_env.unwrap_or_default();
    let applicable_desktop = applicable_desktop.unwrap_or_default();
    let bindings_cmd = bindings_cmd.unwrap_or_default();
    let errors = [
        (
            !valid_slug(&id),
            "id must contain only lowercase letters, digits, and hyphens",
        ),
        (display.trim().is_empty(), "display must not be empty"),
        (tier != "extension", "tier must be the literal 'extension'"),
        (
            applicable_env.is_empty() && applicable_desktop.is_empty(),
            "at least one applicability environment or desktop is required",
        ),
        (
            bindings_cmd.is_empty(),
            "bindings_cmd must contain an executable",
        ),
    ];
    let mut errors = errors
        .into_iter()
        .filter_map(|(invalid, message)| invalid.then_some(message.to_owned()))
        .collect::<Vec<_>>();
    for command in [
        &bindings_cmd,
        focused_cmd.as_ref().unwrap_or(&Vec::new()),
        reload_gen_cmd.as_ref().unwrap_or(&Vec::new()),
    ] {
        if let Some(message) = validate_argv(command) {
            errors.push(message);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok((
        ExtensionAdapter {
            id,
            display,
            tier,
            manifest_path: path.to_owned(),
            applicable_env,
            applicable_desktop,
            bindings_cmd,
            focused_cmd,
            reload_gen_cmd,
        },
        warnings,
    ))
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_argv(argv: &[String]) -> Option<String> {
    if argv.iter().any(|arg| arg.is_empty()) {
        return Some("command arguments must not be empty".into());
    }
    let explicit_shell = argv.first().is_some_and(|program| {
        matches!(
            Path::new(program)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("sh" | "bash" | "dash" | "zsh" | "fish")
        )
    });
    if !explicit_shell
        && argv.iter().any(|arg| {
            arg.chars().any(|character| {
                matches!(character, ';' | '|' | '&' | '$' | '>' | '<' | '`' | '\n')
            })
        })
    {
        return Some("command arguments contain shell metacharacters; use an explicit shell argv when needed".into());
    }
    None
}

fn strip_comment(line: &str) -> String {
    let mut quoted = None;
    for (index, character) in line.char_indices() {
        match (quoted, character) {
            (None, '"' | '\'') => quoted = Some(character),
            (Some(quote), character) if character == quote => quoted = None,
            (None, '#') => return line[..index].to_owned(),
            _ => {}
        }
    }
    line.to_owned()
}

#[derive(Debug)]
enum Value {
    String(String),
    List(Vec<String>),
}

fn parse_string(value: &str) -> Result<String, ()> {
    let value = value.trim();
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|_| ())
    } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        Ok(value[1..value.len() - 1].to_owned())
    } else {
        Err(())
    }
}

fn parse_string_list(value: &str) -> Result<Vec<String>, ()> {
    let value = value.trim();
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err(());
    }
    let inner = value[1..value.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    split_list(inner).into_iter().map(parse_string).collect()
}

fn split_list(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = None;
    for (index, character) in value.char_indices() {
        match (quoted, character) {
            (None, '"' | '\'') => quoted = Some(character),
            (Some(quote), character) if character == quote => quoted = None,
            (None, ',') => {
                parts.push(value[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(value[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> &'static str {
        r#"
id = "herbstluftwm"
display = "herbstluftwm"
tier = "extension"
applicable_env = ["HERBSTLUFTWM_SOCKET"]
applicable_desktop = ["herbstluftwm"]
bindings_cmd = ["herbstclient", "list_keybinds"]
focused_cmd = ["herbstclient", "attr", "clients.focus.winid"]
"#
    }

    #[test]
    fn parses_a_valid_manifest() {
        let path = Path::new("herbstluftwm.toml");
        let (adapter, warnings) = parse_manifest(path, manifest()).unwrap();
        assert_eq!(adapter.id, "herbstluftwm");
        assert_eq!(adapter.bindings_cmd[1], "list_keybinds");
        assert!(warnings.is_empty());
    }

    #[test]
    fn rejects_bad_ids_and_non_extension_tiers() {
        let result = parse_manifest(Path::new("bad.toml"), "id = \"Bad WM\"\ntier = \"t1\"");
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|error| error.contains("lowercase")));
        assert!(errors.iter().any(|error| error.contains("extension")));
    }

    #[test]
    fn missing_directory_is_empty() {
        let (adapters, warnings) = discover_in(Path::new("/definitely/missing/whykey-adapters"));
        assert!(adapters.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn applicability_uses_environment_or_desktop_hints() {
        let (adapter, _) = parse_manifest(Path::new("fixture.toml"), manifest()).unwrap();
        let context = compositor::Context {
            desktop: Some("herbstluftwm".into()),
            session_type: Some("wayland".into()),
            display_server: Some("Wayland".into()),
        };
        assert!(applicable_with_context(&adapter, &context));
    }

    #[test]
    fn json_inventory_skips_malformed_entries_without_losing_valid_records() {
        let (adapter, _) = parse_manifest(Path::new("fixture.toml"), manifest()).unwrap();
        let report = parse_json_entries(
            &adapter,
            vec![
                JsonBinding {
                    key: Some("super+return".into()),
                    action: Some("spawn foot".into()),
                    context: Some("default".into()),
                    device: None,
                    submap: None,
                },
                JsonBinding {
                    key: Some("super+ctrl".into()),
                    action: Some("ignored".into()),
                    context: None,
                    device: None,
                    submap: None,
                },
            ],
        );
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.malformed_entries, 1);
        assert_eq!(report.records[0].key, "SUPER+RETURN");
    }

    #[test]
    fn literal_inventory_splits_key_and_action() {
        let (adapter, _) = parse_manifest(Path::new("fixture.toml"), manifest()).unwrap();
        let report = parse_literal_entries(
            &adapter,
            "super+return spawn foot\nsuper+ctrl ignored\nctrl+x echo hi\n",
        );
        assert_eq!(report.records.len(), 2);
        assert_eq!(report.records[1].action, "echo hi");
        assert_eq!(report.malformed_entries, 1);
    }

    #[test]
    fn explicit_shell_argv_is_allowed_but_shell_metacharacters_are_not() {
        assert!(validate_argv(&["sh".into(), "-c".into(), "echo hi".into()]).is_none());
        assert!(validate_argv(&["herbstclient".into(), "list;rm".into()]).is_some());
    }
}
