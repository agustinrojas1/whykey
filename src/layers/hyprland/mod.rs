use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, LayerId, LayerResult, LayerStatus, Outcome,
    PhysicalInput, Propagation, SourceLocation,
};
use crate::xkb;

pub struct Hyprland;

#[derive(Debug, Clone)]
pub(crate) struct Probe {
    binds: Result<String, String>,
    submap: Result<String, String>,
    devices: Option<String>,
}

impl Probe {
    pub(crate) fn ipc_available(&self) -> bool {
        self.binds.is_ok() && self.submap.is_ok()
    }
}

thread_local! {
    static INSTANCE_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run an inspection with an explicitly selected Hyprland instance. The
/// override is thread-local and restored immediately, so a CLI query cannot
/// mutate the user's session environment or affect later work in the process.
pub fn with_instance_override<T>(instance: &str, operation: impl FnOnce() -> T) -> T {
    let previous = INSTANCE_OVERRIDE.with(|value| value.replace(Some(instance.to_owned())));
    let result = operation();
    INSTANCE_OVERRIDE.with(|value| value.replace(previous));
    result
}

fn instance_override() -> Option<String> {
    INSTANCE_OVERRIDE.with(|value| value.borrow().clone())
}

#[derive(Debug, Deserialize)]
struct HyprlandInstance {
    instance: String,
    #[serde(default)]
    wl_socket: String,
}

#[derive(Debug, Deserialize)]
struct Binding {
    modmask: u32,
    key: String,
    #[serde(default)]
    keycode: crate::xkb::XkbKeycode,
    #[serde(default)]
    catch_all: bool,
    #[serde(default)]
    non_consuming: bool,
    #[serde(default)]
    auto_consuming: bool,
    #[serde(default)]
    release: bool,
    #[serde(default, rename = "longPress")]
    long_press: bool,
    #[serde(default)]
    submap: String,
    #[serde(default, deserialize_with = "deserialize_boolish")]
    submap_universal: bool,
    #[serde(default)]
    ignore_mods: serde_json::Value,
    #[serde(default)]
    locked: serde_json::Value,
    #[serde(default)]
    repeat: serde_json::Value,
    #[serde(default)]
    transparent: serde_json::Value,
    #[serde(default)]
    dont_inhibit: serde_json::Value,
    #[serde(default)]
    allow_input_capture: serde_json::Value,
    /// Older Hyprland versions expose this as a string; newer per-device
    /// bindings may expose the structured `{ inclusive, list }` value. Keep
    /// it as JSON so either shape remains inspectable instead of invalidating
    /// the whole `hyprctl binds -j` response.
    #[serde(default)]
    device: serde_json::Value,
    #[serde(default)]
    description: String,
    dispatcher: String,
    #[serde(default)]
    arg: String,
}

impl Hyprland {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        self.inspect_with_input(key, None)
    }

    pub fn inspect_with_input(
        &self,
        key: &KeyCombo,
        physical_input: Option<&PhysicalInput>,
    ) -> LayerResult {
        self.inspect_with_probe(key, physical_input, None)
    }

    pub(crate) fn inspect_with_probe(
        &self,
        key: &KeyCombo,
        physical_input: Option<&PhysicalInput>,
        probe: Option<&Probe>,
    ) -> LayerResult {
        if remote_session_without_compositor() {
            return LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Pass,
                "not applicable in this remote session",
                vec![
                    "SSH session detected without a local Hyprland IPC signature".into(),
                    "the remote terminal/session layers are inspected instead".into(),
                ],
            );
        }
        let result = match probe {
            Some(probe) => inspect_system_with_probe(key, physical_input, probe),
            None => inspect_system(key, physical_input),
        };
        match result {
            Ok(result) => result,
            Err(message) => ipc_uncertain_result(message),
        }
    }
}

fn ipc_uncertain_result(message: String) -> LayerResult {
    let mut details = vec![message];
    if let Some(config) = hyprland_config_paths()
        .into_iter()
        .find(|path| path.is_file())
    {
        details.push(format!(
            "static Hyprland configuration found at {}; active IPC state could not be verified",
            config.display()
        ));
    }
    details.push(
        "the active submap, runtime overrides, and input-inhibitor state remain unknown".into(),
    );
    LayerResult::new(
        "Hyprland",
        LayerId::Compositor,
        Outcome::UncertainContinues,
        "Hyprland IPC is unavailable; effective binding state is unknown",
        details,
    )
}

pub(crate) fn remote_session_without_compositor() -> bool {
    remote_session_without_compositor_state(
        env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
        instance_override().is_some() || has_hyprland_signature(),
    )
}

fn remote_session_without_compositor_state(ssh: bool, compositor_signature: bool) -> bool {
    ssh && !compositor_signature
}

/// Check Hyprland IPC using the same instance selection as key inspection.
/// This is public so `doctor` cannot disagree with the inspection path when
/// the compositor was launched without exporting its instance signature.
pub fn ipc_available() -> bool {
    run_hyprctl(&["binds", "-j"]).is_ok() && run_hyprctl(&["submap", "-j"]).is_ok()
}

/// Return Hyprland bindings as inventory records for the global listing.
/// The raw JSON conversion stays separate from key-specific matching so
/// malformed entries can be reported here without changing inspection.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let value = binding_inventory_json().map_err(|error| format!("Hyprland: {error}"))?;
    Ok(collect_records(&value))
}

fn binding_inventory_json() -> Result<serde_json::Value, String> {
    let output = run_hyprctl(&["binds", "-j"])?;
    serde_json::from_str(&output)
        .map_err(|error| format!("hyprctl returned invalid binding data: {error}"))
}

fn collect_records(value: &serde_json::Value) -> Vec<BindingRecord> {
    let mut records = Vec::new();
    let Some(bindings) = value.as_array() else {
        return records;
    };
    for binding in bindings {
        let Some(key) = binding.get("key").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let mask = binding
            .get("modmask")
            .and_then(super::json_u32)
            .unwrap_or_default();
        let action = binding
            .get("dispatcher")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown dispatcher");
        let mut action = action.to_owned();
        if let Some(argument) = binding
            .get("arg")
            .and_then(serde_json::Value::as_str)
            .filter(|argument| !argument.is_empty())
        {
            action.push(' ');
            action.push_str(argument);
        }
        if let Some(description) = binding
            .get("description")
            .and_then(serde_json::Value::as_str)
            .filter(|description| !description.is_empty())
        {
            action.push_str("; ");
            action.push_str(description);
        }
        let submap = binding
            .get("submap")
            .and_then(serde_json::Value::as_str)
            .filter(|submap| !submap.is_empty() && *submap != "reset")
            .map_or_else(|| "default".into(), str::to_owned);
        // Typed filter fields come straight from the payload. `device` stays
        // absent for structured per-device scopes instead of guessing.
        let device = binding
            .get("device")
            .and_then(serde_json::Value::as_str)
            .filter(|device| !device.is_empty())
            .map(str::to_owned);
        records.push(BindingRecord {
            device,
            submap: Some(submap.clone()),
            source: "Hyprland (hyprctl binds -j)".into(),
            key: super::combo_display(mask, key),
            action,
            context: Some(submap),
            certainty: "runtime effective binding".into(),
        });
    }
    records
}

pub fn focused_pid() -> Result<u32, String> {
    let output = run_hyprctl(&["activewindow", "-j"])?;
    let value: serde_json::Value = serde_json::from_str(&output)
        .map_err(|error| format!("hyprctl returned invalid active-window data: {error}"))?;
    value
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "Hyprland active window does not expose a valid PID".into())
}

pub fn applicable() -> bool {
    if instance_override().is_some() {
        return true;
    }
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return has_hyprland_signature();
    }
    if has_hyprland_signature() || desktop_hint_is_hyprland() {
        return true;
    }
    // Do not query every running hyprctl instance for a session explicitly
    // identified as another desktop. This matters on systems where a stale
    // Hyprland IPC socket remains available while the user is in KDE, i3, or
    // Sway, and keeps adapter selection tied to the observed session.
    if desktop_hint_is_other_desktop() || !wayland_session_evidence() {
        return false;
    }
    discover_hyprland_instance().is_some()
}

pub(crate) fn collect_probe() -> Option<Probe> {
    if !applicable() {
        return None;
    }
    let instance = instance_override().or_else(|| {
        (!has_hyprland_signature() && !desktop_hint_is_other_desktop())
            .then(discover_hyprland_instance)
            .flatten()
    });
    let instance = instance.as_deref();
    let binds = run_hyprctl_with_instance(&["binds", "-j"], instance);
    let submap = run_hyprctl_with_instance(&["submap", "-j"], instance);
    let devices = if binds.is_ok() && submap.is_ok() {
        run_hyprctl_with_instance(&["devices", "-j"], instance).ok()
    } else {
        None
    };
    Some(Probe {
        binds,
        submap,
        devices,
    })
}

fn inspect_system(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    let bindings_raw = run_hyprctl(&["binds", "-j"])?;
    let active_submap_json = run_hyprctl(&["submap", "-j"])?;
    let devices_json = run_hyprctl(&["devices", "-j"]).ok();
    inspect_system_with_values(
        key,
        physical_input,
        bindings_raw,
        active_submap_json,
        devices_json,
    )
}

fn inspect_system_with_probe(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    probe: &Probe,
) -> Result<LayerResult, String> {
    inspect_system_with_values(
        key,
        physical_input,
        probe.binds.clone()?,
        probe.submap.clone()?,
        probe.devices.clone(),
    )
}

fn inspect_system_with_values(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    bindings_raw: String,
    active_submap_json: String,
    devices_json: Option<String>,
) -> Result<LayerResult, String> {
    let mut active_submap: String = serde_json::from_str(&active_submap_json)
        .map_err(|error| format!("hyprctl returned an invalid submap: {error}"))?;

    let bindings_json = if active_submap == "__whykey_capture" {
        active_submap = query_previous_submap_from_lua().unwrap_or_else(|| "default".into());
        filter_whykey_capture_bindings(&bindings_raw)
    } else {
        bindings_raw
    };
    let keycodes = devices_json
        .as_deref()
        .map(|devices| xkb_keycodes_for_key(key, devices))
        .unwrap_or_default();
    let lua_hints = lua_config_bindings();
    let config_ignore_mods = config_has_ignore_mods(key, &lua_hints);

    let mut result = super::hyprland_matching::analyze(
        key,
        &bindings_json,
        &active_submap,
        &keycodes,
        &lua_hints,
        config_ignore_mods,
        physical_input,
    )?;
    if let Some(instance) = instance_override() {
        result
            .details
            .push(format!("selected Hyprland instance: {instance}"));
    }
    if let Some(devices_json) = devices_json.as_deref() {
        if let Some(summary) = summarize_keyboards(devices_json) {
            result.verbose_details.push(summary);
        }
        if let Some(group) = main_keyboard_active_layout_index(devices_json) {
            result
                .verbose_details
                .push(format!("active XKB layout group index: {group}"));
        } else {
            result.details.push(
                "active XKB layout group was not exposed; layout-specific symbol matching remains conditional"
                    .into(),
            );
        }
        if let Some(input) = physical_input {
            match xkb_symbols_for_physical_key(input.keycode, devices_json) {
                Some(symbols) if !symbols.is_empty() => result.verbose_details.push(format!(
                    "active XKB keymap maps evdev keycode {} to symbols: {}",
                    input.keycode.get(),
                    symbols.join(", ")
                )),
                _ => result.verbose_details.push(format!(
                    "active XKB keymap did not resolve evdev keycode {} to a symbol",
                    input.keycode.get()
                )),
            }
        }
        if keycodes.len() == 1 {
            result.verbose_details.push(format!(
                "XKB layout resolves {key} to keycode {} for the main keyboard",
                keycodes[0].get()
            ));
        } else if !keycodes.is_empty() {
            let values = keycodes
                .iter()
                .map(|code| code.get().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            result.verbose_details.push(format!(
                "XKB layout resolves {key} to multiple keycodes ({values}) for the main keyboard"
            ));
        }
    }
    if matches!(
        result.outcome,
        Outcome::Unknown
            | Outcome::Unavailable
            | Outcome::HandledUncertain
            | Outcome::UncertainContinues
    ) {
        result
            .details
            .push("active input-inhibitor state is not exposed by hyprctl".into());
    }
    if !matches!(result.outcome, Outcome::Pass | Outcome::Redirected) {
        if let Some(hint) = lua_hints.iter().rev().find(|hint| hint.combo == *key) {
            let description = hint
                .description
                .as_deref()
                .or(hint.action.as_deref())
                .unwrap_or("binding");
            result.details.push(format!(
                "Lua source hint: {}:{} — {}",
                hint.source.display(),
                hint.line_number,
                description
            ));
        }
    }
    Ok(result)
}

fn run_hyprctl(arguments: &[&str]) -> Result<String, String> {
    let instance = instance_override().or_else(|| {
        (!has_hyprland_signature())
            .then(discover_hyprland_instance)
            .flatten()
    });
    run_hyprctl_with_instance(arguments, instance.as_deref())
}

fn run_hyprctl_with_instance(arguments: &[&str], instance: Option<&str>) -> Result<String, String> {
    let mut command = Command::new("hyprctl");
    if let Some(instance) = instance {
        command.args(["-i", instance]);
    }
    command.args(arguments);
    let output = command::output(&mut command).map_err(|error| error.to_string())?;

    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("hyprctl exited with {}", output.status)
        } else {
            format!("hyprctl: {message}")
        });
    }

    String::from_utf8(output.stdout)
        .map_err(|error| format!("hyprctl output was not UTF-8: {error}"))
}

fn discover_hyprland_instance() -> Option<String> {
    if let Some(instance) = instance_override() {
        return Some(instance);
    }
    if let Some(signature) = env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        if !signature.is_empty() {
            return signature.into_string().ok();
        }
    }
    let mut command = Command::new("hyprctl");
    command.args(["instances", "-j"]);
    let output = command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let instances = parse_hyprland_instances(&output.stdout)?;
    select_hyprland_instance(&instances, env::var("WAYLAND_DISPLAY").ok().as_deref())
}

/// Parse the instance inventory conservatively. Hyprland's inventory has
/// gained fields across releases, and a malformed/stale entry should not
/// hide otherwise usable instances from a multi-instance session.
fn parse_hyprland_instances(bytes: &[u8]) -> Option<Vec<HyprlandInstance>> {
    let values: Vec<serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<HyprlandInstance>(value).ok())
            .filter(|instance| !instance.instance.trim().is_empty())
            .collect(),
    )
}

fn has_hyprland_signature() -> bool {
    env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|value| !value.is_empty())
}

fn desktop_hint_is_hyprland() -> bool {
    let desktop = desktop_hint();
    desktop.is_some_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("hyprland"))
    })
}

fn desktop_hint_is_other_desktop() -> bool {
    desktop_hint().is_some_and(|desktop| {
        !desktop
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("hyprland"))
    })
}

fn desktop_hint() -> Option<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("XDG_SESSION_DESKTOP")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn wayland_session_evidence() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
        || env::var("XDG_SESSION_TYPE")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
}

fn select_hyprland_instance(
    instances: &[HyprlandInstance],
    wayland_display: Option<&str>,
) -> Option<String> {
    if let Some(display) = wayland_display.filter(|display| !display.is_empty()) {
        let matches = instances
            .iter()
            .filter(|instance| instance.wl_socket == display)
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return Some(matches[0].instance.clone());
        }
    }
    (instances.len() == 1).then(|| instances[0].instance.clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigBindHint {
    key: String,
    ignore_mods: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaBindHint {
    combo: KeyCombo,
    description: Option<String>,
    action: Option<String>,
    ignore_mods: Option<bool>,
    source: PathBuf,
    line_number: usize,
}

/// Read only the declarative bind syntax needed to recover `ignore_mods`.
///
/// `hyprctl binds -j` intentionally omits that field on several Hyprland
/// versions. This parser is deliberately conservative: unresolved variables,
/// globs, and Lua expressions are skipped, so it can only add evidence and
/// never turns an unknown binding into a false exact match.
fn config_has_ignore_mods(key: &KeyCombo, lua_hints: &[LuaBindHint]) -> bool {
    if lua_hints
        .iter()
        .any(|hint| hint.ignore_mods == Some(true) && hint.combo.key() == key.key())
    {
        return true;
    }
    let Some(root) = hyprland_config_paths()
        .into_iter()
        .find(|path| path.is_file())
    else {
        return false;
    };
    let mut visited = HashSet::new();
    let mut variables = HashMap::new();
    let mut hints = Vec::new();
    collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
    hints
        .iter()
        .any(|hint| hint.ignore_mods && hint.key.eq_ignore_ascii_case(key.key()))
}

fn hyprland_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("HYPRLAND_CONFIG") {
        paths.push(PathBuf::from(path));
    }
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("hypr/hyprland.conf"));
    }
    paths.push(PathBuf::from("/etc/xdg/hypr/hyprland.conf"));
    paths
}

/// Scan the small, literal subset of Lua used by common Hyprland helpers
/// (`o.bind` and `hl.bind`). Lua is executable code, so this never evaluates
/// it; dynamic key expressions remain intentionally unknown.
fn lua_config_bindings() -> Vec<LuaBindHint> {
    let mut roots = Vec::new();
    // Load packaged defaults first and user configuration last, matching the
    // normal Omarchy require order so the final matching hint is the useful
    // one when a user overrides a default binding.
    roots.push(PathBuf::from("/usr/share/omarchy/default/hypr"));
    if let Some(omarchy) = env::var_os("OMARCHY_PATH") {
        roots.push(PathBuf::from(omarchy).join("default/hypr"));
    }
    if let Some(base) = crate::util::config_base() {
        roots.push(base.join("hypr"));
    }

    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        collect_lua_files(&root, &mut seen, &mut files, 0);
    }

    let mut hints = Vec::new();
    for path in files {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        hints.extend(parse_lua_bind_hints(&content, &path));
    }
    hints
}

fn parse_lua_bind_hints(content: &str, source: &Path) -> Vec<LuaBindHint> {
    const MAX_CALL_LENGTH: usize = 64 * 1024;
    let mut hints = Vec::new();
    let mut pending = String::new();
    let mut start_line = 0;
    let mut skipped = None;

    for (index, raw_line) in content.lines().enumerate() {
        let sanitized = strip_lua_comments_and_long_strings(raw_line, &mut skipped);
        let line = sanitized.trim();
        if pending.is_empty() {
            if find_lua_bind_call(line).is_none() {
                continue;
            }
            start_line = index + 1;
        } else {
            pending.push('\n');
        }
        pending.push_str(line);

        let complete = find_lua_bind_call(&pending)
            .and_then(|(_, open)| lua_call_arguments(&pending[open + 1..]))
            .is_some();
        if complete {
            if let Some(hint) = parse_lua_bind_hint(&pending, source, start_line) {
                hints.push(hint);
            }
            pending.clear();
        } else if pending.len() > MAX_CALL_LENGTH {
            pending.clear();
        }
    }
    hints
}

/// A Lua long-bracket comment or string that continues onto a later line.
/// The closing delimiter is retained rather than a nesting counter because
/// Lua long brackets do not nest.
type LuaSkippedDelimiter = Option<String>;

fn strip_lua_comments_and_long_strings(line: &str, skipped: &mut LuaSkippedDelimiter) -> String {
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        if let Some(delimiter) = skipped.as_deref() {
            if let Some(offset) = line[index..].find(delimiter) {
                index += offset + delimiter.len();
                *skipped = None;
            } else {
                break;
            }
            continue;
        }

        let character = line[index..]
            .chars()
            .next()
            .expect("index stays on a UTF-8 character boundary");
        if matches!(character, '"' | '\'') {
            let quote = character;
            output.push(character);
            index += character.len_utf8();
            let mut escaped = false;
            while index < line.len() {
                let next = line[index..]
                    .chars()
                    .next()
                    .expect("index stays on a UTF-8 character boundary");
                output.push(next);
                index += next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            continue;
        }

        if character == '-' && line[index..].starts_with("--") {
            let comment_start = index + 2;
            if let Some((open_len, delimiter)) = lua_long_bracket(line, comment_start) {
                *skipped = Some(delimiter);
                index = comment_start + open_len;
                continue;
            }
            break;
        }

        if character == '[' {
            if let Some((open_len, delimiter)) = lua_long_bracket(line, index) {
                *skipped = Some(delimiter);
                index += open_len;
                continue;
            }
        }

        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn lua_long_bracket(line: &str, start: usize) -> Option<(usize, String)> {
    let bytes = line.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut cursor = start + 1;
    while bytes.get(cursor) == Some(&b'=') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'[') {
        return None;
    }
    let equals = &line[start + 1..cursor];
    Some((cursor + 1 - start, format!("]{equals}]")))
}

fn collect_lua_files(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    depth: usize,
) {
    const MAX_LUA_DEPTH: usize = 5;
    const MAX_LUA_FILES: usize = 512;
    if depth > MAX_LUA_DEPTH || files.len() >= MAX_LUA_FILES {
        return;
    }
    // Follow a symlink only for the explicitly selected root. A symlinked
    // child could otherwise make a config scan unexpectedly walk an unrelated
    // tree (or a whole filesystem hierarchy).
    if depth > 0
        && fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        return;
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !seen.insert(canonical.clone()) {
        return;
    }
    let Ok(metadata) = fs::metadata(&canonical) else {
        return;
    };
    if metadata.is_file() {
        if canonical.extension().and_then(|value| value.to_str()) == Some("lua") {
            files.push(canonical);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(&canonical) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_lua_files(&entry.path(), seen, files, depth + 1);
        if files.len() >= MAX_LUA_FILES {
            break;
        }
    }
}

fn parse_lua_bind_hint(line: &str, source: &Path, line_number: usize) -> Option<LuaBindHint> {
    let (prefix, open) = find_lua_bind_call(line)?;
    let arguments = lua_call_arguments(&line[open + 1..])?;
    let combo = lua_literal_argument(arguments.first()?)?.parse().ok()?;
    let (description, action, options_index) = if prefix == "o.bind" {
        (
            arguments
                .get(1)
                .and_then(|value| lua_literal_argument(value)),
            arguments
                .get(2)
                .and_then(|value| lua_literal_argument(value)),
            3,
        )
    } else {
        (
            None,
            arguments
                .get(1)
                .and_then(|value| lua_literal_argument(value)),
            2,
        )
    };
    let ignore_mods = lua_ignore_mods_option(arguments.get(options_index).copied());
    Some(LuaBindHint {
        combo,
        description,
        action,
        ignore_mods,
        source: source.to_owned(),
        line_number,
    })
}

fn find_lua_bind_call(line: &str) -> Option<(&'static str, usize)> {
    let prefixes = ["o.bind", "hl.bind", "bind"];
    let mut index = 0;
    while index < line.len() {
        let character = line[index..]
            .chars()
            .next()
            .expect("index stays on a UTF-8 character boundary");
        if matches!(character, '"' | '\'') {
            let quote = character;
            index += character.len_utf8();
            let mut escaped = false;
            while index < line.len() {
                let next = line[index..]
                    .chars()
                    .next()
                    .expect("index stays on a UTF-8 character boundary");
                index += next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            continue;
        }
        if character == '-' && line[index..].starts_with("--") {
            break;
        }
        for prefix in prefixes {
            if !line[index..].starts_with(prefix) {
                continue;
            }
            let has_boundary = line[..index]
                .chars()
                .next_back()
                .is_none_or(|value| !value.is_alphanumeric() && value != '_');
            let is_qualified_bare_bind = prefix == "bind"
                && line[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|value| value == '.' || value == ':');
            if !has_boundary || is_qualified_bare_bind {
                continue;
            }
            let after_name = &line[index + prefix.len()..];
            let whitespace = after_name.len() - after_name.trim_start().len();
            if after_name[whitespace..].starts_with('(') {
                return Some((prefix, index + prefix.len() + whitespace));
            }
        }
        index += character.len_utf8();
    }
    None
}

fn lua_call_arguments(input: &str) -> Option<Vec<&str>> {
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut parentheses = 0_u32;
    let mut braces = 0_u32;
    let mut brackets = 0_u32;
    let mut quote = None;
    let mut escaped = false;

    for (index, character) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '(' => parentheses += 1,
            ')' if parentheses > 0 => parentheses -= 1,
            ')' if braces == 0 && brackets == 0 => {
                let argument = input[start..index].trim();
                if !argument.is_empty() {
                    arguments.push(argument);
                }
                return Some(arguments);
            }
            '{' => braces += 1,
            '}' if braces > 0 => braces -= 1,
            '[' => brackets += 1,
            ']' if brackets > 0 => brackets -= 1,
            ',' if parentheses == 0 && braces == 0 && brackets == 0 => {
                arguments.push(input[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    None
}

fn lua_literal_argument(argument: &str) -> Option<String> {
    let argument = argument.trim();
    let quote = matches!(argument.chars().next(), Some('"' | '\''));
    if !quote {
        return None;
    }

    // Only accept a complete quoted Lua string. In particular, do not turn
    // `"SUPER+C" .. suffix` (or a helper call containing a string) into a
    // literal binding: doing so could make a dynamic declaration look like
    // authoritative runtime evidence.
    let mut characters = argument.char_indices();
    let (_, delimiter) = characters.next()?;
    let mut value = String::new();
    let mut escaped = false;
    let mut closing_byte = None;
    for (index, character) in characters {
        if escaped {
            value.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == delimiter {
            closing_byte = Some(index + character.len_utf8());
            break;
        } else {
            value.push(character);
        }
    }
    let closing_byte = closing_byte?;
    if escaped || !argument[closing_byte..].trim().is_empty() {
        return None;
    }
    Some(value)
}

fn lua_ignore_mods_option(options: Option<&str>) -> Option<bool> {
    let Some(options) = options else {
        return Some(false);
    };
    let options = options.trim();
    if !options.starts_with('{') || !options.ends_with('}') {
        return None;
    }
    let normalized = options
        .split_whitespace()
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.contains("ignore_mods=true") {
        Some(true)
    } else {
        Some(false)
    }
}

fn lua_ignore_mods_for_binding(binding: &Binding, hints: &[LuaBindHint]) -> Option<bool> {
    hints
        .iter()
        .rev()
        .find(|hint| {
            hint.combo.modmask() == binding.modmask
                && hint.combo.key().eq_ignore_ascii_case(&binding.key)
        })
        .and_then(|hint| hint.ignore_mods)
}

fn collect_config_hints(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    variables: &mut HashMap<String, String>,
    hints: &mut Vec<ConfigBindHint>,
    depth: usize,
) {
    const MAX_CONFIG_DEPTH: usize = 32;
    if depth > MAX_CONFIG_DEPTH {
        return;
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !visited.insert(canonical.clone()) {
        return;
    }
    let Ok(content) = fs::read_to_string(&canonical) else {
        return;
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = parse_config_variable(line) {
            variables.insert(name, value);
        }
    }

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(include) = parse_config_include(line) {
            let include = resolve_config_path(&include, &canonical, variables);
            if !include.to_string_lossy().contains('*')
                && !include.to_string_lossy().contains('?')
                && !include.to_string_lossy().contains('[')
            {
                collect_config_hints(&include, visited, variables, hints, depth + 1);
            }
        }
        if let Some(hint) = parse_config_bind_hint(line, variables) {
            hints.push(hint);
        }
    }
}

fn parse_config_variable(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once('=')?;
    let name = name.trim().strip_prefix('$')?;
    if name.is_empty() || name.chars().any(|character| character.is_whitespace()) {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then(|| (name.to_owned(), value.to_owned()))
}

fn parse_config_include(line: &str) -> Option<String> {
    let (directive, value) = line.split_once('=')?;
    if !matches!(directive.trim(), "source" | "include") {
        return None;
    }
    let value = value.trim().trim_matches(['"', '\'']);
    (!value.is_empty()).then(|| value.to_owned())
}

fn resolve_config_path(
    raw: &str,
    relative_to: &Path,
    variables: &HashMap<String, String>,
) -> PathBuf {
    let mut value = raw.to_owned();
    for _ in 0..8 {
        let Some(start) = value.find('$') else {
            break;
        };
        let end = value[start + 1..]
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map(|offset| start + 1 + offset)
            .unwrap_or(value.len());
        let name = &value[start + 1..end];
        let replacement = variables.get(name).cloned().or_else(|| env::var(name).ok());
        let Some(replacement) = replacement else {
            break;
        };
        value.replace_range(start..end, &replacement);
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        if value == "~" {
            return home;
        }
        if let Some(rest) = value.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        relative_to
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn parse_config_bind_hint(
    line: &str,
    variables: &HashMap<String, String>,
) -> Option<ConfigBindHint> {
    let (directive, values) = line.split_once('=')?;
    let directive = directive.trim();
    let flags = directive
        .strip_prefix("bind[")
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_default();
    if !directive.starts_with("bind") {
        return None;
    }
    let mut fields = values.split(',').map(str::trim);
    let modifiers = resolve_inline_variables(fields.next()?, variables);
    let key = resolve_inline_variables(fields.next()?, variables);
    if key.is_empty() || key.contains(' ') && key.split_whitespace().count() > 1 {
        return None;
    }
    let combo_text = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{key}", modifiers.replace(' ', "+"))
    };
    let combo: KeyCombo = combo_text.parse().ok()?;
    Some(ConfigBindHint {
        key: combo.key().to_owned(),
        ignore_mods: flags.contains('i'),
    })
}

fn resolve_inline_variables(raw: &str, variables: &HashMap<String, String>) -> String {
    let mut value = raw.trim().to_owned();
    for (name, replacement) in variables {
        value = value.replace(&format!("${name}"), replacement);
    }
    value.trim_matches(['"', '\'']).trim().to_owned()
}

fn summarize_keyboards(devices_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(devices_json).ok()?;
    let keyboards = value.get("keyboards")?.as_array()?;
    if keyboards.is_empty() {
        return Some("active keyboards: none reported".into());
    }
    let labels: Vec<_> = keyboards
        .iter()
        .filter_map(|keyboard| {
            let name = keyboard.get("name").and_then(serde_json::Value::as_str)?;
            let keymap = keyboard
                .get("active_keymap")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| format!(", keymap {value}"))
                .unwrap_or_default();
            let main = keyboard
                .get("main")
                .and_then(serde_json::Value::as_bool)
                .filter(|value| *value)
                .map(|_| ", main")
                .unwrap_or_default();
            Some(format!("{name}{keymap}{main}"))
        })
        .collect();
    let main_index = keyboards
        .iter()
        .position(|keyboard| {
            keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true)
        })
        .unwrap_or(0);
    let main = labels.get(main_index)?;
    let additional: Vec<_> = labels
        .iter()
        .enumerate()
        .filter_map(|(index, label)| (index != main_index).then_some(label.as_str()))
        .collect();
    if additional.is_empty() {
        Some(format!("main keyboard: {main}"))
    } else {
        let preview = additional
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if additional.len() > 3 { ", ..." } else { "" };
        Some(format!(
            "main keyboard: {main}; {} additional keyboard(s): {preview}{suffix}",
            additional.len()
        ))
    }
}

fn xkb_keycodes_for_key(key: &KeyCombo, devices_json: &str) -> Vec<crate::xkb::XkbKeycode> {
    if key.key().starts_with("CODE:") {
        return Vec::new();
    }
    let Some(keymap) = compile_main_xkb_keymap(devices_json) else {
        return Vec::new();
    };
    let Some(group) = main_keyboard_active_layout_index(devices_json) else {
        return Vec::new();
    };
    let keycodes = parse_xkb_symbol_keycodes_for_group(&keymap, group);
    keycodes
        .get(&xkb_symbol_name(key).unwrap_or_default())
        .map(|codes| {
            codes
                .iter()
                .map(|code| crate::xkb::XkbKeycode::from(*code))
                .collect()
        })
        .unwrap_or_default()
}

fn xkb_symbols_for_physical_key(
    evdev_keycode: crate::xkb::EvdevKeycode,
    devices_json: &str,
) -> Option<Vec<String>> {
    let keymap = compile_main_xkb_keymap(devices_json)?;
    let group = main_keyboard_active_layout_index(devices_json)?;
    Some(xkb::symbols_for_evdev_keycode(
        &keymap,
        evdev_keycode,
        group,
    ))
}

pub(crate) fn main_keyboard_active_layout_index(devices_json: &str) -> Option<usize> {
    let devices = serde_json::from_str::<serde_json::Value>(devices_json).ok()?;
    let keyboards = devices
        .get("keyboards")
        .and_then(serde_json::Value::as_array)?;
    let keyboard = keyboards
        .iter()
        .find(|keyboard| keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| keyboards.first())?;
    keyboard
        .get("active_layout_index")
        .and_then(serde_json::Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
}

pub(crate) fn compile_main_xkb_keymap(devices_json: &str) -> Option<String> {
    let Ok(devices) = serde_json::from_str::<serde_json::Value>(devices_json) else {
        return None;
    };
    let keyboards = devices
        .get("keyboards")
        .and_then(serde_json::Value::as_array)?;
    let keyboard = keyboards
        .iter()
        .find(|keyboard| keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| keyboards.first());
    let keyboard = keyboard?;

    let layout = keyboard
        .get("layout")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let mut rmlvo = xkb::Rmlvo {
        rules: "evdev".into(),
        model: "pc105".into(),
        layout: layout.into(),
        variant: None,
        options: None,
    };
    for (field, flag, default) in [("rules", "rules", "evdev"), ("model", "model", "pc105")] {
        let value = keyboard
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(default);
        match flag {
            "rules" => rmlvo.rules = value.into(),
            "model" => rmlvo.model = value.into(),
            "layout" => rmlvo.layout = value.into(),
            _ => unreachable!(),
        }
    }
    rmlvo.variant = keyboard
        .get("variant")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    rmlvo.options = keyboard
        .get("options")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    xkb::compile_keymap(&rmlvo)
}

fn xkb_symbol_name(key: &KeyCombo) -> Option<String> {
    let name = match key.key() {
        "LEFT" => "Left",
        "RIGHT" => "Right",
        "UP" => "Up",
        "DOWN" => "Down",
        "RETURN" => "Return",
        "ESCAPE" => "Escape",
        "BACKSPACE" => "BackSpace",
        "DELETE" => "Delete",
        "INSERT" => "Insert",
        "PAGE_UP" => "Prior",
        "PAGE_DOWN" => "Next",
        "SPACE" => "space",
        "TAB" => "Tab",
        "HOME" => "Home",
        "END" => "End",
        value if value.starts_with('F') && value[1..].parse::<u8>().is_ok() => value,
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphanumeric() => {
            return Some(value.to_ascii_lowercase());
        }
        "!" => "exclam",
        "@" => "at",
        "#" => "numbersign",
        "$" => "dollar",
        "%" => "percent",
        "^" => "asciicircum",
        "&" => "ampersand",
        "*" => "asterisk",
        "(" => "parenleft",
        ")" => "parenright",
        "=" => "equal",
        "-" => "minus",
        "_" => "underscore",
        "+" => "plus",
        "[" => "bracketleft",
        "]" => "bracketright",
        ";" => "semicolon",
        ":" => "colon",
        "'" => "apostrophe",
        "\"" => "quotedbl",
        "," => "comma",
        "<" => "less",
        "." => "period",
        ">" => "greater",
        "/" => "slash",
        "?" => "question",
        "\\" => "backslash",
        "|" => "bar",
        "~" => "asciitilde",
        "`" => "grave",
        _ => return None,
    };
    Some(name.into())
}

#[cfg(test)]
fn parse_xkb_symbol_keycodes(keymap: &str) -> HashMap<String, Vec<u32>> {
    xkb::parse_symbol_keycodes(keymap)
}

fn parse_xkb_symbol_keycodes_for_group(
    keymap: &str,
    group_index: usize,
) -> HashMap<String, Vec<u32>> {
    xkb::parse_symbol_keycodes_for_group(keymap, group_index)
}

#[cfg(test)]
fn parse_xkb_symbol_key_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("key <")?;
    let (name, _) = rest.split_once('>')?;
    Some(name.to_owned())
}

#[cfg(test)]
fn parse_xkb_symbol_fragment(line: &str) -> Option<Vec<String>> {
    let symbols = line.rsplit_once('[')?.1.split_once(']')?.0;
    let symbols = symbols
        .split(',')
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty() && *symbol != "NoSymbol")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!symbols.is_empty()).then_some(symbols)
}

#[cfg(test)]
fn inspect_json(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
) -> Result<LayerResult, String> {
    inspect_json_with_keycode(key, bindings_json, active_submap, &[], &[], false, None)
}

pub(crate) fn inspect_json_with_keycode(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[LuaBindHint],
    config_ignore_mods: bool,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    let (bindings, skipped_bindings) = parse_bindings(bindings_json)?;
    let active_submap = normalize_submap(active_submap);

    let active_bindings: Vec<_> = bindings
        .iter()
        .filter(|binding| binding_is_active(binding, active_submap))
        .collect();

    // Keep same-key bindings from other submaps visible. They do not handle
    // the current event, but hiding them makes mode-dependent configurations
    // look as if the key is unbound everywhere.
    let inactive_bindings: Vec<_> = bindings
        .iter()
        .filter(|binding| !binding_is_active(binding, active_submap))
        .filter(|binding| binding_matches_key(binding, key, keycodes, physical_input))
        .filter(|binding| {
            physical_device_match(&binding.device, physical_input) != DeviceMatch::NoMatch
        })
        .collect();

    let matches: Vec<_> = active_bindings
        .iter()
        .copied()
        .filter_map(|binding| {
            if !binding_matches_key(binding, key, keycodes, physical_input) {
                None
            } else {
                let device_scoped = has_device_scope(&binding.device);
                let device_match = physical_device_match(&binding.device, physical_input);
                if device_match == DeviceMatch::NoMatch {
                    return None;
                }
                let certainty = if binding.modmask == key.modmask() {
                    MatchCertainty::Exact
                } else if boolish_value(&binding.ignore_mods) == Some(true) {
                    MatchCertainty::ModifierInsensitive
                } else if boolish_value(&binding.ignore_mods) == Some(false) {
                    return None;
                } else if let Some(ignore_mods) = lua_ignore_mods_for_binding(binding, lua_hints) {
                    if ignore_mods {
                        MatchCertainty::ModifierInsensitive
                    } else {
                        return None;
                    }
                } else if config_ignore_mods {
                    MatchCertainty::ModifierInsensitive
                } else {
                    MatchCertainty::PossibleIgnoreMods
                };
                Some(BindingMatch {
                    binding,
                    // Without a physical capture we cannot know whether a
                    // per-device binding applies. A captured evdev device has
                    // already passed that filter, so retain modifier certainty.
                    certainty: if device_scoped
                        && (physical_input.is_none() || device_match == DeviceMatch::Unknown)
                    {
                        MatchCertainty::PossibleDevice
                    } else {
                        certainty
                    },
                })
            }
        })
        .collect();

    if matches.is_empty() {
        if skipped_bindings > 0 {
            return Ok(LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unknown,
                "effective bindings are incomplete; no definitive match",
                details_with_physical_input(
                    vec![
                        format!("active submap: {active_submap}"),
                        format!(
                            "could not parse {skipped_bindings} binding entr{}",
                            if skipped_bindings == 1 { "y" } else { "ies" }
                        ),
                    ],
                    physical_input,
                ),
            )
            .with_verbose_details(inactive_submap_details(&inactive_bindings)));
        }
        return Ok(LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "no active binding found",
            details_with_physical_input(
                vec![format!("active submap: {active_submap}")],
                physical_input,
            ),
        )
        .with_verbose_details(inactive_submap_details(&inactive_bindings)));
    }

    let mut details = details_with_physical_input(
        vec![format!("active submap: {active_submap}")],
        physical_input,
    );
    // Same-key bindings from other submaps explain mode-dependent setups;
    // they stay one `--verbose` away in normal reports.
    let mut verbose_details: Vec<String> = Vec::new();
    if skipped_bindings > 0 {
        details.push(format!(
            "could not parse {skipped_bindings} effective binding entr{}; other bindings may be missing",
            if skipped_bindings == 1 { "y" } else { "ies" }
        ));
    }
    let has_exact_match = matches.iter().any(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        )
    });
    let has_possible_match = matches
        .iter()
        .any(|matched| matched.certainty == MatchCertainty::PossibleIgnoreMods);
    let has_possible_device = matches
        .iter()
        .any(|matched| matched.certainty == MatchCertainty::PossibleDevice);

    if has_possible_match {
        details.push("Hyprland does not expose the ignore_mods flag through hyprctl.".into());
    }
    if config_ignore_mods
        && matches
            .iter()
            .any(|matched| matched.certainty == MatchCertainty::ModifierInsensitive)
    {
        details.push(
            "the loaded declarative config contains an ignore_mods binding for this key".into(),
        );
    }
    if has_possible_device {
        if physical_input.and_then(|i| i.device.as_ref()).is_some() {
            details.push(
                "device-specific binding could not be matched by name with certainty; it remains possible.".into(),
            );
        } else {
            details.push(
                "device-specific binding found; the active keyboard device is unknown.".into(),
            );
        }
    } else if let Some(input) = physical_input {
        if input.device.is_some() {
            details.push(
                "device-specific bindings were checked against the captured evdev device.".into(),
            );
        } else {
            details.push(
                "The physical keycode was captured, but the source keyboard is unavailable.".into(),
            );
            details.push("Device-specific binding matching remains uncertain.".into());
        }
    }
    if !inactive_bindings.is_empty() {
        verbose_details.push(format!(
            "{} matching binding(s) exist in inactive submap(s); current submap is {active_submap}",
            inactive_bindings.len()
        ));
        verbose_details.extend(inactive_submap_details(&inactive_bindings));
    }
    if !has_exact_match {
        details.push(format!("no exact binding for {key}"));
    }

    for matched in &matches {
        let label = match matched.certainty {
            MatchCertainty::Exact => "binding",
            MatchCertainty::PossibleIgnoreMods => "possible modifier-insensitive binding",
            MatchCertainty::ModifierInsensitive => "modifier-insensitive binding",
            MatchCertainty::PossibleDevice => "possible device-specific binding",
        };
        details.push(format!(
            "{label}: {} ({})",
            binding_action(matched.binding),
            binding_scope(matched.binding, active_submap)
        ));
    }

    let exact_matches = matches.iter().filter(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        )
    });
    if exact_matches.clone().any(|matched| {
        boolish_value(&matched.binding.locked) == Some(true)
            || boolish_value(&matched.binding.dont_inhibit) == Some(true)
    }) {
        details.push(
            "matching binding is allowed while an input inhibitor is active (locked/dont_inhibit)"
                .into(),
        );
    }
    if exact_matches
        .clone()
        .any(|matched| boolish_value(&matched.binding.allow_input_capture) == Some(true))
    {
        details
            .push("matching binding remains active while client input capture is enabled".into());
    }

    if matches.iter().any(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        ) && dispatcher_is_opaque(matched.binding)
            && !matched.binding.non_consuming
    }) {
        details
            .push("hyprctl does not expose the forwarding result of an opaque dispatcher.".into());
    }
    if matches
        .iter()
        .any(|matched| matched.binding.dispatcher == "__lua")
    {
        details.push(
            "Lua/plugin dispatcher is active; its runtime action is not executed by whykey.".into(),
        );
    }

    let propagation = if skipped_bindings > 0 {
        Propagation::Indeterminate
    } else {
        binding_propagation(&matches)
    };
    let status = if skipped_bindings > 0 {
        LayerStatus::Indeterminate
    } else if has_exact_match {
        LayerStatus::Handled
    } else {
        LayerStatus::Indeterminate
    };
    let outcome = Outcome::from_parts(&status, &propagation);
    let summary = if skipped_bindings > 0 {
        "effective bindings are incomplete; handling cannot be proven"
    } else {
        match (&status, &propagation) {
            (LayerStatus::Handled, Propagation::Continues) => {
                "active binding found; event is forwarded"
            }
            (LayerStatus::Handled, Propagation::Stops) => "active binding found; event is consumed",
            (LayerStatus::Handled, Propagation::Redirected) => {
                "active binding found; event is redirected to another window"
            }
            (LayerStatus::Handled, Propagation::Indeterminate) => {
                "active binding found; forwarding cannot be determined"
            }
            (LayerStatus::Indeterminate, Propagation::Continues) if has_possible_device => {
                "no device-independent exact binding found; the active keyboard is unknown"
            }
            (LayerStatus::Indeterminate, Propagation::Continues) => {
                "no exact binding found; a same-key binding may ignore modifiers"
            }
            (LayerStatus::Indeterminate, Propagation::Indeterminate) => {
                "no exact binding found; forwarding cannot be proven"
            }
            _ => unreachable!("binding matches cannot produce this state"),
        }
    };

    let has_universal_match = matches
        .iter()
        .any(|matched| matched.binding.submap_universal);
    let binding = primary_match(&matches).map(|matched| {
        let mut evidence = binding_evidence(matched.binding, lua_hints, has_universal_match);
        if evidence.uncertainty.is_none() && matched.certainty == MatchCertainty::PossibleIgnoreMods
        {
            evidence.uncertainty = Some(crate::layers::UncertaintyReason::ModifierAmbiguity);
        }
        evidence
    });

    Ok({
        let mut result =
            LayerResult::new("Hyprland", LayerId::Compositor, outcome, summary, details)
                .with_verbose_details(verbose_details);
        result.binding = binding;
        result
    })
}

/// The match the conclusion describes: an exact binding when one exists,
/// otherwise the first candidate. Evidence never invents a stronger match.
fn primary_match<'a>(matches: &'a [BindingMatch<'a>]) -> Option<BindingMatch<'a>> {
    matches
        .iter()
        .find(|matched| {
            matches!(
                matched.certainty,
                MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
            )
        })
        .or_else(|| matches.first())
        .copied()
}

/// Typed binding evidence for the renderer and schema v2. The human-readable
/// `details` stay untouched; decisions must read these fields instead.
fn binding_evidence(
    binding: &Binding,
    lua_hints: &[LuaBindHint],
    has_universal_match: bool,
) -> BindingEvidence {
    let action = format!("{} {}", binding.dispatcher, binding.arg);
    BindingEvidence {
        dispatcher: none_if_empty(&binding.dispatcher),
        action: none_if_empty(&action),
        description: none_if_empty(&binding.description),
        submap: Some(normalize_submap(&binding.submap).to_string()),
        scope: if binding.submap_universal {
            BindingScope::Universal
        } else {
            BindingScope::Submap(normalize_submap(&binding.submap).to_string())
        },
        source: hint_source(binding, lua_hints),
        has_universal_match,
        uncertainty: if BindingEvidence::dispatcher_is_opaque(&binding.dispatcher) {
            Some(crate::layers::UncertaintyReason::OpaqueDispatcher)
        } else {
            None
        },
    }
}

fn none_if_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Link a runtime binding to its static config hint only on an exact
/// description match. Anything weaker would be a guessed file hint.
fn hint_source(binding: &Binding, lua_hints: &[LuaBindHint]) -> Option<SourceLocation> {
    let description = none_if_empty(&binding.description)?;
    let hint = lua_hints
        .iter()
        .find(|hint| hint.description.as_deref() == Some(&description))?;
    Some(SourceLocation {
        file: hint.source.display().to_string(),
        line: u32::try_from(hint.line_number).ok(),
    })
}

fn parse_bindings(bindings_json: &str) -> Result<(Vec<Binding>, usize), String> {
    let value: serde_json::Value = serde_json::from_str(bindings_json)
        .map_err(|error| format!("hyprctl returned invalid binding data: {error}"))?;
    let Some(entries) = value.as_array() else {
        return Err("hyprctl returned invalid binding data: expected an array".into());
    };
    let mut bindings = Vec::with_capacity(entries.len());
    let mut skipped = 0;
    for entry in entries {
        match serde_json::from_value::<Binding>(entry.clone()) {
            Ok(binding) => bindings.push(binding),
            Err(_) => skipped += 1,
        }
    }
    Ok((bindings, skipped))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchCertainty {
    Exact,
    PossibleIgnoreMods,
    ModifierInsensitive,
    PossibleDevice,
}

#[derive(Debug, Clone, Copy)]
struct BindingMatch<'a> {
    binding: &'a Binding,
    certainty: MatchCertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlowState {
    found: bool,
    dispatcher_passes: bool,
    dispatcher_redirects: bool,
    processing: bool,
}

#[derive(Debug, Clone, Copy)]
struct DispatcherOutcome {
    success: bool,
    passes: bool,
    redirects: bool,
}

fn normalize_submap(submap: &str) -> &str {
    match submap {
        "" | "reset" => "default",
        other => other,
    }
}

fn binding_is_active(binding: &Binding, active_submap: &str) -> bool {
    binding.submap_universal || normalize_submap(&binding.submap) == active_submap
}

fn inactive_submap_details(bindings: &[&Binding]) -> Vec<String> {
    bindings
        .iter()
        .map(|binding| {
            let submap = normalize_submap(&binding.submap);
            format!(
                "inactive submap binding: {} ({submap})",
                binding_action(binding)
            )
        })
        .collect()
}

fn binding_matches_key(
    binding: &Binding,
    combo: &KeyCombo,
    keycodes: &[crate::xkb::XkbKeycode],
    physical_input: Option<&PhysicalInput>,
) -> bool {
    if let Some(keycode) = combo.key().strip_prefix("CODE:") {
        // An explicit numeric query is already an assertion about the
        // binding's code; do not let a separately captured physical event
        // broaden it to another code.
        return keycode
            .parse::<u32>()
            .ok()
            .map(crate::xkb::XkbKeycode::from)
            == Some(binding.keycode);
    }
    if binding.keycode != crate::xkb::XkbKeycode::from(0) {
        return keycodes.contains(&binding.keycode)
            || physical_input
                .is_some_and(|input| physical_keycode_matches(binding.keycode, input.keycode));
    }
    binding.catch_all || binding.key.eq_ignore_ascii_case(combo.key())
}

fn physical_keycode_matches(
    binding_keycode: crate::xkb::XkbKeycode,
    evdev_keycode: crate::xkb::EvdevKeycode,
) -> bool {
    // `Binding.keycode` is an XKB keycode per the compositor. Physical
    // capture carries a Linux evdev code, so convert it once at this
    // boundary instead of accepting both numeric namespaces.
    evdev_keycode
        .to_xkb()
        .is_some_and(|xkb_keycode| binding_keycode == xkb_keycode)
}

fn binding_propagation(matches: &[BindingMatch<'_>]) -> Propagation {
    let mut states = vec![FlowState {
        found: false,
        dispatcher_passes: false,
        dispatcher_redirects: false,
        processing: true,
    }];

    for matched in matches {
        let previous = states;
        let mut next = Vec::new();

        if matches!(
            matched.certainty,
            MatchCertainty::PossibleIgnoreMods | MatchCertainty::PossibleDevice
        ) {
            next.extend(previous.iter().copied());
        }

        for state in previous {
            if !state.processing {
                next.push(state);
                continue;
            }

            for outcome in dispatcher_outcomes(matched.binding) {
                let found = if matched.binding.release
                    && !matched.binding.non_consuming
                    && !matched.binding.auto_consuming
                    && !dispatcher_is_special(matched.binding)
                {
                    true
                } else if matched.binding.long_press || matched.binding.non_consuming {
                    state.found
                } else if matched.binding.auto_consuming {
                    state.found || outcome.success
                } else {
                    true
                };
                let next_state = FlowState {
                    found,
                    dispatcher_passes: outcome.passes,
                    dispatcher_redirects: outcome.redirects,
                    processing: matched.binding.dispatcher != "submap",
                };
                if !next.contains(&next_state) {
                    next.push(next_state);
                }
            }
        }

        states = next;
    }

    let outcomes: Vec<_> = states
        .iter()
        .map(|state| {
            if state.dispatcher_redirects {
                Propagation::Redirected
            } else if state.dispatcher_passes || !state.found {
                Propagation::Continues
            } else {
                Propagation::Stops
            }
        })
        .collect();
    if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Continues)
    {
        Propagation::Continues
    } else if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Stops)
    {
        Propagation::Stops
    } else if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Redirected)
    {
        Propagation::Redirected
    } else {
        Propagation::Indeterminate
    }
}

fn dispatcher_outcomes(binding: &Binding) -> Vec<DispatcherOutcome> {
    if binding.dispatcher == "pass" {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: true,
        }];
    }

    if binding.release
        && !binding.non_consuming
        && !binding.auto_consuming
        && !dispatcher_is_special(binding)
    {
        return vec![DispatcherOutcome {
            success: false,
            passes: false,
            redirects: false,
        }];
    }

    if binding.release && binding.auto_consuming {
        return vec![DispatcherOutcome {
            success: false,
            passes: true,
            redirects: false,
        }];
    }

    if binding.long_press {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: false,
        }];
    }

    if binding.non_consuming {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: false,
        }];
    }

    if dispatcher_is_opaque(binding) {
        return vec![
            DispatcherOutcome {
                success: true,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: true,
                passes: true,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: true,
                redirects: false,
            },
        ];
    }

    if binding.auto_consuming {
        return vec![
            DispatcherOutcome {
                success: true,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: false,
                redirects: false,
            },
        ];
    }

    vec![DispatcherOutcome {
        success: true,
        passes: false,
        redirects: false,
    }]
}
fn dispatcher_is_opaque(binding: &Binding) -> bool {
    BindingEvidence::dispatcher_is_opaque(&binding.dispatcher)
}

fn dispatcher_is_special(binding: &Binding) -> bool {
    matches!(
        binding.dispatcher.as_str(),
        "global" | "pass" | "sendshortcut" | "mouse"
    )
}

fn binding_scope<'a>(binding: &Binding, active_submap: &'a str) -> &'a str {
    if binding.submap_universal {
        "all submaps"
    } else {
        active_submap
    }
}

fn binding_action(binding: &Binding) -> String {
    let mut action = binding.dispatcher.clone();
    if !binding.arg.is_empty() {
        action.push(' ');
        action.push_str(&binding.arg);
    }
    if !binding.description.is_empty() {
        action.push_str("; ");
        action.push_str(&binding.description);
    }
    let mut flags = Vec::new();
    for (name, value) in [
        ("locked", &binding.locked),
        ("repeat", &binding.repeat),
        ("transparent", &binding.transparent),
        ("dont_inhibit", &binding.dont_inhibit),
        ("allow_input_capture", &binding.allow_input_capture),
    ] {
        if boolish_value(value) == Some(true) {
            flags.push(name);
        }
    }
    if has_device_scope(&binding.device) {
        flags.push("device-specific");
    }
    if !flags.is_empty() {
        action.push_str(" [");
        action.push_str(&flags.join(", "));
        action.push(']');
    }
    action
}

fn boolish_value(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn query_previous_submap_from_lua() -> Option<String> {
    let output = run_hyprctl(&[
        "repl",
        "return (_G.__whykey_capture and _G.__whykey_capture.previous_submap ~= \"\" and _G.__whykey_capture.previous_submap) or \"default\"",
    ])
    .ok()?;
    let trimmed = output.trim().to_string();
    if trimmed.is_empty()
        || trimmed == "default"
        || trimmed == "unknown request"
        || trimmed == "none"
        || trimmed == "\"default\""
    {
        Some("default".into())
    } else {
        Some(trimmed)
    }
}

fn filter_whykey_capture_bindings(bindings_json: &str) -> String {
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(bindings_json) {
        if let Some(arr) = val.as_array_mut() {
            arr.retain(|b| b.get("submap").and_then(|s| s.as_str()) != Some("__whykey_capture"));
            if let Ok(filtered) = serde_json::to_string(&val) {
                return filtered;
            }
        }
    }
    bindings_json.to_owned()
}

fn has_device_scope(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => true,
    }
}

pub(crate) fn main_keyboard_lock_state(devices_json: &str) -> (bool, bool) {
    let Ok(devices) = serde_json::from_str::<serde_json::Value>(devices_json) else {
        return (false, false);
    };
    let keyboards = devices
        .get("keyboards")
        .and_then(serde_json::Value::as_array);
    let Some(keyboards) = keyboards else {
        return (false, false);
    };
    let keyboard = keyboards
        .iter()
        .find(|keyboard| keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| keyboards.first());
    let Some(keyboard) = keyboard else {
        return (false, false);
    };
    let caps = keyboard
        .get("capsLock")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let num = keyboard
        .get("numLock")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (caps, num)
}

fn details_with_physical_input(
    mut details: Vec<String>,
    physical_input: Option<&PhysicalInput>,
) -> Vec<String> {
    if let Some(input) = physical_input {
        match &input.device {
            Some(device) => details.push(format!("captured evdev device: {device}")),
            None => details.push("captured keyboard device: unavailable".into()),
        }
        let xkb_candidate = input
            .keycode
            .to_xkb()
            .map_or("unknown".into(), |code| code.get().to_string());
        details.push(format!(
            "physical keycode: {} (XKB keycode {})",
            input.keycode.get(),
            xkb_candidate
        ));
    }
    details
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceMatch {
    Match,
    NoMatch,
    Unknown,
}

fn physical_device_match(
    scope: &serde_json::Value,
    physical_input: Option<&PhysicalInput>,
) -> DeviceMatch {
    let Some(physical_input) = physical_input else {
        return DeviceMatch::Unknown;
    };
    let Some(device_name) = physical_input.device.as_deref() else {
        return DeviceMatch::Unknown;
    };
    match scope {
        serde_json::Value::String(name) => device_name_match(name, device_name),
        serde_json::Value::Object(object) => {
            let inclusive = object
                .get("inclusive")
                .and_then(boolish_value)
                .unwrap_or(true);
            let names = object
                .get("list")
                .and_then(serde_json::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if names.is_empty() {
                return DeviceMatch::Unknown;
            }
            let listed = names
                .iter()
                .map(|name| device_name_match(name, device_name))
                .fold(DeviceMatch::Unknown, |state, current| match current {
                    DeviceMatch::Match => DeviceMatch::Match,
                    DeviceMatch::NoMatch => state,
                    DeviceMatch::Unknown => {
                        if state == DeviceMatch::Match {
                            state
                        } else {
                            DeviceMatch::Unknown
                        }
                    }
                });
            match (inclusive, listed) {
                (true, DeviceMatch::Match) => DeviceMatch::Match,
                (true, DeviceMatch::NoMatch | DeviceMatch::Unknown) => DeviceMatch::Unknown,
                (false, DeviceMatch::Match) => DeviceMatch::NoMatch,
                (false, DeviceMatch::NoMatch | DeviceMatch::Unknown) => DeviceMatch::Unknown,
            }
        }
        _ => DeviceMatch::Unknown,
    }
}

fn device_name_match(configured: &str, actual: &str) -> DeviceMatch {
    let configured = configured.trim();
    if configured.is_empty() || configured.contains('*') || configured.contains('?') {
        return DeviceMatch::Unknown;
    }
    if configured.eq_ignore_ascii_case(actual.trim())
        || normalize_device_name(configured) == normalize_device_name(actual)
    {
        DeviceMatch::Match
    } else {
        // libinput/Hyprland and evdev can use different names for the same
        // composite keyboard. A mismatch is therefore not proof of exclusion.
        DeviceMatch::Unknown
    }
}

fn normalize_device_name(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect()
}

fn deserialize_boolish<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Boolish {
        Bool(bool),
        String(String),
    }

    match Boolish::deserialize(deserializer)? {
        Boolish::Bool(value) => Ok(value),
        Boolish::String(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Boolish::String(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Boolish::String(value) => Err(serde::de::Error::custom(format!(
            "expected true or false, got '{value}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_ssh_without_a_compositor_signature_as_not_applicable() {
        assert!(remote_session_without_compositor_state(true, false));
        assert!(!remote_session_without_compositor_state(true, true));
        assert!(!remote_session_without_compositor_state(false, false));
    }

    #[test]
    fn keeps_ipc_failure_conditional_and_allows_downstream_inspection() {
        let result = ipc_uncertain_result("hyprctl: stale socket".into());

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(result.summary.contains("IPC is unavailable"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("stale socket"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("active submap"))
        );
    }

    #[test]
    fn selects_hyprland_instance_for_current_wayland_socket() {
        let instances: Vec<HyprlandInstance> =
            serde_json::from_str(include_str!("../../../tests/fixtures/hyprland/instances.json"))
                .unwrap();
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-1")),
            Some("second".into())
        );
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-9")),
            None
        );
    }

    #[test]
    fn selects_the_only_hyprland_instance_without_wayland_identity() {
        let instances = vec![HyprlandInstance {
            instance: "only".into(),
            wl_socket: "wayland-0".into(),
        }];
        assert_eq!(
            select_hyprland_instance(&instances, None),
            Some("only".into())
        );
    }

    #[test]
    fn leaves_ambiguous_instances_unselected_without_socket_identity() {
        let instances: Vec<HyprlandInstance> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hyprland/instances-ambiguous.json"
        ))
        .unwrap();
        assert_eq!(select_hyprland_instance(&instances, None), None);
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-9")),
            None
        );
    }

    #[test]
    fn leaves_duplicate_socket_matches_unselected() {
        let instances = vec![
            HyprlandInstance {
                instance: "first".into(),
                wl_socket: "wayland-0".into(),
            },
            HyprlandInstance {
                instance: "second".into(),
                wl_socket: "wayland-0".into(),
            },
        ];
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-0")),
            None
        );
    }

    #[test]
    fn keeps_valid_instances_when_one_inventory_entry_is_malformed() {
        let payload = include_bytes!("../../../tests/fixtures/hyprland/instances-version-matrix.json");
        let instances = parse_hyprland_instances(payload).unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(
            select_hyprland_instance(&instances, Some("wayland-2")),
            Some("second".into())
        );
    }

    const BINDINGS: &str = include_str!("../../../tests/fixtures/hyprland/binds-representative.json");

    #[test]
    fn preserves_a_universal_match_when_a_regular_binding_is_primary() {
        let combo: KeyCombo = "ctrl+x".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "X",
                "submap": "default",
                "submap_universal": false,
                "dispatcher": "exec",
                "arg": "regular"
            },
            {
                "modmask": 4,
                "key": "X",
                "submap": "default",
                "submap_universal": true,
                "dispatcher": "exec",
                "arg": "universal"
            }
        ]"#;
        let result = inspect_json(&combo, bindings, "default").unwrap();
        let evidence = result.binding.expect("a primary binding is expected");
        assert_eq!(evidence.scope, BindingScope::Submap("default".into()));
        assert!(evidence.has_universal_match);
    }

    #[test]
    fn sanitized_fixture_matrix_covers_legacy_structured_and_malformed_shapes() {
        let matrix: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hyprland/version-matrix.json"
        ))
        .unwrap();
        assert_eq!(matrix["schema_version"], 1);
        let entries = matrix["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 4);

        for entry in entries {
            let binds = entry["binds"].as_str().unwrap();
            let devices = entry["devices"].as_str().unwrap();
            let binds_json = match binds {
                "binds-representative.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-representative.json")
                }
                "binds-structured-device.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-structured-device.json")
                }
                "binds-malformed-entry.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-malformed-entry.json")
                }
                "binds-invalid-object.json" => {
                    include_str!("../../../tests/fixtures/hyprland/binds-invalid-object.json")
                }
                other => panic!("unexpected binds fixture {other}"),
            };
            let devices_json = match devices {
                "devices-keyboards.json" => {
                    include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json")
                }
                "devices-layout-groups.json" => {
                    include_str!("../../../tests/fixtures/hyprland/devices-layout-groups.json")
                }
                other => panic!("unexpected devices fixture {other}"),
            };
            serde_json::from_str::<serde_json::Value>(binds_json).unwrap();
            serde_json::from_str::<serde_json::Value>(devices_json).unwrap();
        }
    }

    #[test]
    fn reports_a_binding_in_the_default_submap() {
        let combo: KeyCombo = "super+c".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.details[1].contains("__lua 61"));
        assert!(result.details[1].contains("Universal copy"));
    }

    #[test]
    fn ignores_a_binding_in_an_inactive_submap() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(
            result
                .verbose_details
                .iter()
                .any(|detail| detail.contains("inactive submap binding")),
            "inactive submaps stay behind --verbose"
        );
        assert!(
            result
                .details
                .iter()
                .all(|detail| !detail.contains("inactive submap binding"))
        );
    }

    #[test]
    fn reports_that_a_non_consuming_binding_is_forwarded() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "resize").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn reports_auto_consuming_as_indeterminate() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.details[1].contains("all submaps"));
    }

    #[test]
    fn warns_when_a_same_key_binding_may_ignore_modifiers() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let result = inspect_json(&combo, BINDINGS, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.summary.contains("no exact binding"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("no exact binding for CTRL + C"))
        );
    }

    #[test]
    fn honors_explicit_ignore_mods() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "C",
            "ignore_mods": true,
            "dispatcher": "exec",
            "arg": "notify-send copy"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
        assert!(result.details[1].contains("modifier-insensitive binding"));
    }

    #[test]
    fn reports_inhibitor_safe_binding_flags() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "Z",
            "locked": true,
            "allow_input_capture": true,
            "dispatcher": "exec",
            "arg": "notify-send suspend"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("input inhibitor"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("input capture"))
        );
    }

    #[test]
    fn matches_numeric_keycode_combinations() {
        let combo: KeyCombo = "ctrl+code:30".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 30,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn keeps_device_specific_bindings_conditional() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": "at-translated-set-2-keyboard",
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("device-specific"))
        );
    }

    #[test]
    fn accepts_structured_device_scope_from_newer_hyprland_json() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = include_str!("../../../tests/fixtures/hyprland/binds-structured-device.json");

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("device-specific"))
        );
    }

    #[test]
    fn physical_capture_resolves_device_specific_binding() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": "example-keyboard",
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("Example-Keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(46),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("captured evdev device"))
        );
    }

    #[test]
    fn matched_binding_populates_typed_evidence() {
        let combo: KeyCombo = "ctrl+super+return".parse().unwrap();
        let bindings = r#"[{
            "modmask": 68,
            "key": "Return",
            "dispatcher": "__lua",
            "arg": "285",
            "description": "Herdr",
            "submap": "default",
            "submap_universal": true
        }]"#;
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], false, None).unwrap();
        let evidence = result.binding.expect("a match must carry evidence");
        assert_eq!(evidence.dispatcher.as_deref(), Some("__lua"));
        assert_eq!(evidence.action.as_deref(), Some("__lua 285"));
        assert_eq!(evidence.description.as_deref(), Some("Herdr"));
        assert_eq!(evidence.submap.as_deref(), Some("default"));
        assert_eq!(evidence.scope, BindingScope::Universal);
        assert!(evidence.is_universal());
        assert!(evidence.is_opaque());
    }

    #[test]
    fn evidence_links_a_config_hint_only_on_exact_description() {
        let combo: KeyCombo = "ctrl+super+return".parse().unwrap();
        let bindings = r#"[{
            "modmask": 68,
            "key": "Return",
            "dispatcher": "__lua",
            "arg": "285",
            "description": "Herdr",
            "submap": "default"
        }]"#;
        let hints = vec![LuaBindHint {
            combo: combo.clone(),
            description: Some("Herdr".into()),
            action: None,
            ignore_mods: None,
            source: PathBuf::from("/home/user/.config/hypr/bindings.lua"),
            line_number: 42,
        }];
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &hints, false, None)
                .unwrap();
        let evidence = result.binding.expect("a match must carry evidence");
        assert_eq!(evidence.scope, BindingScope::Submap("default".into()));
        let source = evidence.source.expect("an exact hint must link");
        assert_eq!(source.file, "/home/user/.config/hypr/bindings.lua");
        assert_eq!(source.line, Some(42));
        let unrelated =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], false, None).unwrap();
        assert!(
            unrelated
                .binding
                .as_ref()
                .is_some_and(|evidence| evidence.source.is_none()),
            "no hint must mean no file guess"
        );
    }

    #[test]
    fn physical_capture_keeps_unproven_device_mismatch_conditional() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "C",
            "device": {"inclusive": true, "list": ["other-keyboard"]},
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("example-keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(46),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("could not be matched by name"))
        );
    }

    #[test]
    fn physical_evdev_code_matches_xkb_code_offset() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn physical_evdev_code_does_not_match_raw_xkb_number() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 105,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn explicit_code_query_is_not_broadened_by_physical_capture() {
        let combo: KeyCombo = "ctrl+code:30".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let physical = PhysicalInput {
            device: Some("keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::from(105),
        };

        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[],
            &[],
            false,
            Some(&physical),
        )
        .unwrap();

        assert_eq!(result.status(), LayerStatus::NotHandled);
    }

    #[test]
    fn keeps_valid_bindings_when_one_effective_entry_has_an_unexpected_shape() {
        let combo: KeyCombo = "ctrl+c".parse().unwrap();
        let bindings = include_str!("../../../tests/fixtures/hyprland/binds-malformed-entry.json");

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("could not parse 1"))
        );
    }

    #[test]
    fn summarizes_active_keyboard_keymaps() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json");
        assert_eq!(
            summarize_keyboards(devices).as_deref(),
            Some(
                "main keyboard: at-translated-set-2-keyboard, keymap English (US), main; 1 additional keyboard(s): rk68-consumer, keymap English (US)",
            )
        );
    }

    #[test]
    fn pass_dispatcher_redirects_without_non_consuming() {
        let combo: KeyCombo = "ctrl+p".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "P",
            "dispatcher": "pass",
            "arg": "class:example"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Redirected);
    }

    #[test]
    fn release_binding_consumes_the_press_until_release() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "dispatcher": "exec",
            "arg": "notify-send released"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn non_consuming_release_binding_passes_the_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "non_consuming": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn auto_consuming_release_binding_passes_the_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "auto_consuming": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn release_pass_binding_is_redirected_on_press() {
        let combo: KeyCombo = "ctrl+r".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "R",
            "release": true,
            "dispatcher": "pass",
            "arg": "class:example"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Redirected);
    }

    #[test]
    fn long_press_binding_passes_the_original_press() {
        let combo: KeyCombo = "ctrl+l".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "L",
            "longPress": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn release_long_press_binding_still_suppresses_the_press() {
        let combo: KeyCombo = "ctrl+l".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "L",
            "release": true,
            "longPress": true,
            "dispatcher": "exec"
        }]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn submap_binding_stops_later_bindings() {
        let combo: KeyCombo = "ctrl+m".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "M",
                "dispatcher": "submap",
                "arg": "resize"
            },
            {
                "modmask": 4,
                "key": "M",
                "dispatcher": "pass",
                "arg": "class:example"
            }
        ]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn possible_ignore_mods_bindings_are_combined_with_exact_matches() {
        let combo: KeyCombo = "ctrl+x".parse().unwrap();
        let bindings = r#"[
            {
                "modmask": 4,
                "key": "X",
                "non_consuming": true,
                "dispatcher": "exec"
            },
            {
                "modmask": 64,
                "key": "X",
                "dispatcher": "exec"
            }
        ]"#;

        let result = inspect_json(&combo, bindings, "default").unwrap();

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("possible modifier-insensitive"))
        );
    }

    #[test]
    fn parses_xkb_keycodes_and_symbols() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <LEFT> = 113;
    <FK01> = 67;
};
xkb_symbols "pc" {
    key <LEFT> { [ Left ] };
    key <FK01> {
        type = "CTRL+ALT",
        symbols[1] = [ F1, F1 ]
    };
};
};
"#;
        let mapping = parse_xkb_symbol_keycodes(keymap);
        assert_eq!(mapping.get("Left"), Some(&vec![113]));
        assert_eq!(
            parse_xkb_symbol_fragment("symbols[1] = [ F1, F1 ]"),
            Some(vec!["F1".into(), "F1".into()])
        );
        assert_eq!(
            parse_xkb_symbol_key_name("key <FK01> {"),
            Some("FK01".into())
        );
        assert_eq!(mapping.get("F1"), Some(&vec![67]));
    }

    #[test]
    fn selects_only_symbols_from_the_active_xkb_layout_group() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, Q ]
        symbols[2] = [ adiaeresis, Adiaeresis ]
    };
};
};
"#;

        let first = parse_xkb_symbol_keycodes_for_group(keymap, 0);
        assert_eq!(first.get("q"), Some(&vec![24]));
        assert_eq!(first.get("Q"), Some(&vec![24]));
        let second = parse_xkb_symbol_keycodes_for_group(keymap, 1);
        assert_eq!(second.get("adiaeresis"), Some(&vec![24]));
        assert_eq!(second.get("Adiaeresis"), Some(&vec![24]));
        let missing = parse_xkb_symbol_keycodes_for_group(keymap, 3);
        assert!(missing.is_empty());
    }

    #[test]
    fn reads_the_main_keyboard_active_layout_group() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-layout-groups.json");
        assert_eq!(main_keyboard_active_layout_index(devices), Some(2));
    }

    #[test]
    fn does_not_compile_a_guessed_us_layout_when_devices_omit_layout() {
        let devices = include_str!("../../../tests/fixtures/hyprland/devices-keyboards.json");
        assert!(compile_main_xkb_keymap(devices).is_none());
    }

    #[test]
    fn matches_symbolic_query_against_xkb_keycode_binding() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 4,
            "key": "",
            "keycode": 113,
            "dispatcher": "exec"
        }]"#;
        let result = inspect_json_with_keycode(
            &combo,
            bindings,
            "default",
            &[crate::xkb::XkbKeycode::from(113)],
            &[],
            false,
            None,
        )
        .unwrap();
        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn parses_ignore_mods_from_declarative_bind_syntax() {
        let variables = HashMap::from([(String::from("mainMod"), String::from("SUPER"))]);
        let hint = parse_config_bind_hint("bind[i] = $mainMod, Z, exec, test", &variables)
            .expect("bind hint");
        assert_eq!(hint.key, "Z");
        assert!(hint.ignore_mods);
    }

    #[test]
    fn skips_unresolved_declarative_bind_variables() {
        assert!(
            parse_config_bind_hint("bind[i] = $unknown, Z, exec, test", &HashMap::new()).is_none()
        );
    }

    #[test]
    fn resolves_relative_config_includes() {
        let variables = HashMap::new();
        let path = resolve_config_path(
            "parts/binds.conf",
            Path::new("/tmp/hyprland.conf"),
            &variables,
        );
        assert_eq!(path, PathBuf::from("/tmp/parts/binds.conf"));
    }

    #[test]
    fn follows_literal_includes_and_variables_for_ignore_mods() {
        let base = std::env::temp_dir().join(format!("whykey-hypr-config-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let root = base.join("hyprland.conf");
        let child = base.join("bindings.conf");
        fs::write(&root, "$mainMod = SUPER\nsource = bindings.conf\n").unwrap();
        fs::write(&child, "bind[i] = $mainMod, Z, exec, test\n").unwrap();

        let mut visited = HashSet::new();
        let mut variables = HashMap::new();
        let mut hints = Vec::new();
        collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
        assert!(hints.iter().any(|hint| hint.key == "Z" && hint.ignore_mods));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn uses_config_hint_to_resolve_same_key_modifiers() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "Z",
            "dispatcher": "exec"
        }]"#;
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[], true, None).unwrap();
        assert_eq!(result.status(), LayerStatus::Handled);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("ignore_mods"))
        );
    }

    #[test]
    fn parses_literal_omarchy_lua_binding() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + C", "Copy", "wl-copy")"#,
            Path::new("/tmp/bindings.lua"),
            7,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.to_string(), "SUPER + C");
        assert_eq!(hint.description.as_deref(), Some("Copy"));
        assert_eq!(hint.action.as_deref(), Some("wl-copy"));
        assert_eq!(hint.ignore_mods, Some(false));
        assert_eq!(hint.line_number, 7);
    }

    #[test]
    fn parses_lua_ignore_mods_option() {
        let hint = parse_lua_bind_hint(
            r#"hl.bind("Z", hl.dsp.exec_cmd("suspend"), { ignore_mods = true })"#,
            Path::new("/tmp/bindings.lua"),
            3,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.key(), "Z");
        assert_eq!(hint.ignore_mods, Some(true));
    }

    #[test]
    fn leaves_dynamic_lua_options_unknown() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + LEFT", "Move", "movefocus l", options)"#,
            Path::new("/tmp/bindings.lua"),
            5,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.ignore_mods, None);
    }

    #[test]
    fn parses_multiline_lua_function_binding() {
        let hints = parse_lua_bind_hints(
            r#"
o.bind("SUPER + CTRL + Z", "Zoom in", function()
  local zoom = hl.get_config("cursor.zoom_factor") or 1
  hl.config({ cursor = { zoom_factor = zoom + 1 } })
end)
"#,
            Path::new("/tmp/utilities.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "CTRL + SUPER + Z");
        assert_eq!(hints[0].description.as_deref(), Some("Zoom in"));
        assert_eq!(hints[0].ignore_mods, Some(false));
        assert_eq!(hints[0].line_number, 2);
    }

    #[test]
    fn ignores_bind_text_inside_lua_strings_and_long_comments() {
        let hints = parse_lua_bind_hints(
            r##"
local quoted = "o.bind(\"SUPER + A\", \"fake\", \"fake\")"
-- o.bind("SUPER + B", "fake", "fake")
--[=[
o.bind("SUPER + C", "fake", "fake")
]=]
local long_string = [=[hl.bind("SUPER + D", "fake")]=]
o.bind("SUPER + E", "Real", "exec")
"##,
            Path::new("/tmp/bindings.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "SUPER + E");
        assert_eq!(hints[0].description.as_deref(), Some("Real"));
    }

    #[test]
    fn resumes_code_after_a_closed_long_comment() {
        let hints = parse_lua_bind_hints(
            "--[[ ignored ]] o.bind(\"SUPER + F\", \"Find\", \"search\")",
            Path::new("/tmp/bindings.lua"),
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].combo.to_string(), "SUPER + F");
    }

    #[test]
    fn does_not_reduce_an_unrecognized_object_method_to_a_global_bind() {
        assert!(
            parse_lua_bind_hints(
                "custom.bind(\"SUPER + G\", \"Unknown\", \"exec\")",
                Path::new("/tmp/bindings.lua"),
            )
            .is_empty()
        );
    }

    #[test]
    fn parses_representative_consecutive_omarchy_bindings() {
        let hints = parse_lua_bind_hints(
            include_str!("../../../tests/fixtures/hyprland/omarchy-bindings.lua"),
            Path::new("tests/fixtures/hyprland/omarchy-bindings.lua"),
        );
        assert_eq!(hints.len(), 5);
        assert!(hints.iter().all(|hint| hint.ignore_mods == Some(false)));
    }

    #[test]
    fn lua_default_options_rule_out_a_different_modifier_match() {
        let combo: KeyCombo = "ctrl+left".parse().unwrap();
        let bindings = r#"[{
            "modmask": 64,
            "key": "LEFT",
            "dispatcher": "movefocus"
        }]"#;
        let hint = parse_lua_bind_hint(
            r#"o.bind("SUPER + LEFT", "Move", "movefocus l")"#,
            Path::new("/tmp/tiling.lua"),
            1,
        )
        .expect("Lua binding hint");
        let result =
            inspect_json_with_keycode(&combo, bindings, "default", &[], &[hint], false, None)
                .unwrap();
        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn accepts_optional_space_before_lua_call_parenthesis() {
        let hint = parse_lua_bind_hint(
            r#"o.bind ( "CTRL + F", "Find", "search" )"#,
            Path::new("/tmp/bindings.lua"),
            4,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.combo.to_string(), "CTRL + F");
    }

    #[test]
    fn ignores_dynamic_lua_binding_expressions() {
        assert!(
            parse_lua_bind_hint(
                "o.bind(modifier .. \" + C\", \"Copy\", command)",
                Path::new("/tmp/bindings.lua"),
                1,
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_literal_prefix_concatenated_with_dynamic_lua_expression() {
        assert!(
            parse_lua_bind_hint(
                "o.bind(\"SUPER + C\" .. suffix, \"Copy\", command)",
                Path::new("/tmp/bindings.lua"),
                1,
            )
            .is_none()
        );
    }

    #[test]
    fn accepts_escaped_characters_only_inside_a_complete_lua_literal() {
        let hint = parse_lua_bind_hint(
            r#"o.bind("CTRL + F", "Find\nHere", "search")"#,
            Path::new("/tmp/bindings.lua"),
            1,
        )
        .expect("Lua binding hint");
        assert_eq!(hint.description.as_deref(), Some("Find\nHere"));
        assert!(lua_literal_argument(r#""CTRL + F"   "#).is_some());
        assert!(lua_literal_argument(r#""CTRL + F" .. suffix"#).is_none());
    }

    #[test]
    fn rejects_invalid_json() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let error = inspect_json(&combo, "not json", "default").unwrap_err();

        assert!(error.starts_with("hyprctl returned invalid binding data:"));
    }

    #[test]
    fn rejects_a_valid_json_object_instead_of_a_binding_array() {
        let combo: KeyCombo = "ctrl+z".parse().unwrap();
        let error = inspect_json(
            &combo,
            include_str!("../../../tests/fixtures/hyprland/binds-invalid-object.json"),
            "default",
        )
        .unwrap_err();

        assert!(error.contains("expected an array"));
    }
}
