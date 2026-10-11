use super::config::{CONFIG_HINTS, LuaBindHint, lua_hint_key};
use super::keyboard::{
    compile_main_xkb_keymap, main_keyboard_active_layout_index, summarize_keyboards,
    xkb_keycodes_for_key, xkb_symbols_for_physical_key,
};
use super::matching::{analyze, boolish_value, filter_whykey_capture_bindings};
use super::*;

/// Lua that reads whykey's capture state without touching compositor or
/// global state (only locals, `type`, and string concatenation) and reports
/// the submap saved before entering `__whykey_capture`. The tag separates a
/// genuine answer, including a real submap literally named `none` or
/// `default`, from Lua errors, an unset value, or a Hyprland build without
/// `repl`.
const PREVIOUS_SUBMAP_LUA: &str = "local s = _G.__whykey_capture; \
     local p = s and s.previous_submap; \
     if type(p) == \"string\" then return \"whykey-submap:\" .. p end; \
     return \"whykey-submap-unset:\" .. type(p)";

fn query_previous_submap_from_lua() -> Result<String, String> {
    let reply = run_hyprctl(&["repl", PREVIOUS_SUBMAP_LUA])
        .map_err(|error| format!("`hyprctl repl` failed: {error}"))?;
    parse_previous_submap_reply(&reply)
}

fn parse_previous_submap_reply(reply: &str) -> Result<String, String> {
    let reply = reply.strip_suffix('\n').unwrap_or(reply);
    let reply = reply.strip_suffix('\r').unwrap_or(reply);
    if let Some(submap) = reply.strip_prefix("whykey-submap:") {
        // `hl.get_current_submap()` reports the default submap as "".
        return Ok(if submap.is_empty() {
            "default".into()
        } else {
            submap.to_owned()
        });
    }
    if let Some(kind) = reply.strip_prefix("whykey-submap-unset:") {
        return Err(format!(
            "whykey capture state holds no previous submap (Lua value: {kind})"
        ));
    }
    let first_line = reply.lines().next().unwrap_or_default();
    Err(format!("unexpected `hyprctl repl` reply: {first_line:?}"))
}

/// Check Hyprland IPC using the same instance selection as key inspection.
/// This is public so `doctor` cannot disagree with the inspection path when
/// the compositor was launched without exporting its instance signature.
pub fn ipc_available() -> bool {
    run_hyprctl(&["binds", "-j"]).is_ok() && run_hyprctl(&["submap", "-j"]).is_ok()
}

/// Source label of runtime Hyprland inventory records.
pub(crate) const INVENTORY_SOURCE: &str = "Hyprland (hyprctl binds -j)";

/// Return Hyprland bindings as inventory records for the global listing.
/// The raw JSON conversion stays separate from key-specific matching so
/// malformed entries can be reported here without changing inspection.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let value = binding_inventory_json().map_err(|error| format!("Hyprland: {error}"))?;
    // Runtime `code:N` keys are shown as the active layout's key so the
    // inventory can be filtered and compared by the key a user types.
    let keymap = main_keymap();
    let keymap = keymap
        .as_ref()
        .map(|(keymap, group)| (keymap.as_str(), *group));
    Ok(collect_records(&value, &CONFIG_HINTS.lua, keymap))
}

/// The main keyboard's compiled keymap and active layout group.
fn main_keymap() -> Option<(String, usize)> {
    let devices = run_hyprctl(&["devices", "-j"]).ok()?;
    Some((
        compile_main_xkb_keymap(&devices)?,
        main_keyboard_active_layout_index(&devices)?,
    ))
}

/// The key the main keyboard's active layout produces for a `code:N`
/// query (`F23` for `code:201`), so inspect names it like `bindings` does.
/// `None` for other keys or when Hyprland's keymap is unavailable.
pub fn code_key_symbol_for_query(key: &KeyCombo) -> Option<String> {
    if !key.key().starts_with("CODE:") || !applicable() {
        return None;
    }
    let (keymap, group) = main_keymap()?;
    code_key_symbol(key.key(), Some((&keymap, group))).map(|(_, symbol)| symbol)
}

fn binding_inventory_json() -> Result<serde_json::Value, String> {
    let output = query_bindings(run_hyprctl)?;
    serde_json::from_str(&output)
        .map_err(|error| format!("hyprctl returned invalid binding data: {error}"))
}

/// Run `hyprctl binds -j` and fill in keys it leaves empty (D-041).
/// Hyprland's JSON reports `key: ""` and `keycode: 0` for Lua binds whose
/// keys are only `code:N`, while the plain-text listing prints the bind's
/// display key, so the text form is queried only when JSON has such entries.
fn query_bindings(run: impl Fn(&[&str]) -> Result<String, String>) -> Result<String, String> {
    let json = run(&["binds", "-j"])?;
    Ok(recover_unreported_keys(&json, || run(&["binds"]).ok()).unwrap_or(json))
}

/// Whether a `hyprctl binds -j` entry names neither a key nor a keycode.
fn key_unreported(entry: &serde_json::Value) -> bool {
    entry.get("key").and_then(serde_json::Value::as_str) == Some("")
        && entry
            .get("keycode")
            .and_then(crate::layers::json_u32)
            .is_none_or(|keycode| keycode == 0)
        && !entry
            .get("catch_all")
            .and_then(boolish_value)
            .unwrap_or(false)
}

/// Fields both `hyprctl binds` formats print for one bind; Lua binds carry a
/// unique `__lua` registry reference in `arg`.
#[derive(Debug, PartialEq, Eq, Hash)]
struct BindIdentity {
    dispatcher: String,
    arg: String,
    modmask: u32,
    submap: String,
    description: String,
}

impl BindIdentity {
    fn of_json(entry: &serde_json::Value) -> Option<Self> {
        let text = |name: &str| {
            entry
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        Some(Self {
            dispatcher: text("dispatcher")?,
            arg: text("arg").unwrap_or_default(),
            modmask: entry.get("modmask").and_then(crate::layers::json_u32)?,
            submap: text("submap").unwrap_or_default(),
            description: text("description").unwrap_or_default(),
        })
    }
}

/// Return `bindings_json` with keys recovered from the text listing, or
/// `None` when nothing was recovered. A key is filled in only when every
/// text block with the same identity agrees on one single-key display.
pub(super) fn recover_unreported_keys(
    bindings_json: &str,
    text: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(bindings_json).ok()?;
    let entries = value.as_array_mut()?;
    if !entries.iter().any(key_unreported) {
        return None;
    }
    let text = text()?;
    let mut candidates = text_bind_keys(&text);
    // The text listing prints a reported key verbatim; drop those so an
    // identity shared with a resolved bind cannot lend it its key.
    for entry in entries.iter().filter(|entry| !key_unreported(entry)) {
        let (Some(identity), Some(key)) = (
            BindIdentity::of_json(entry),
            entry.get("key").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let Some(keys) = candidates.get_mut(&identity) else {
            continue;
        };
        if let Some(position) = keys.iter().position(|display| display == key) {
            keys.swap_remove(position);
        }
    }
    let mut recovered = false;
    for entry in entries.iter_mut().filter(|entry| key_unreported(entry)) {
        let Some(identity) = BindIdentity::of_json(entry) else {
            continue;
        };
        let Some(displays) = candidates.get(&identity) else {
            continue;
        };
        let mut keys = displays
            .iter()
            .map(|display| display_key(display, identity.modmask));
        let Some(Some(key)) = keys.next() else {
            continue;
        };
        if keys.all(|other| other == Some(key)) {
            entry["key"] = key.into();
            recovered = true;
        }
    }
    recovered.then(|| value.to_string())
}

/// Group the `key:` lines of plain-text `hyprctl binds` by bind identity.
/// Hyprland v0.56.2 `bindsRequest` prints `bind<flags>` followed by tab-
/// indented `modmask`, `submap`, `key`, `keycode`, `catchall`,
/// `description`, `dispatcher`, and `arg` lines.
fn text_bind_keys(text: &str) -> HashMap<BindIdentity, Vec<String>> {
    let mut blocks: Vec<Vec<(&str, &str)>> = Vec::new();
    for line in text.lines() {
        if line.starts_with("bind") {
            blocks.push(Vec::new());
            continue;
        }
        let Some((name, value)) = line
            .strip_prefix('\t')
            .and_then(|field| field.split_once(':'))
        else {
            continue;
        };
        if let Some(block) = blocks.last_mut() {
            block.push((name, value.strip_prefix(' ').unwrap_or(value)));
        }
    }
    let mut groups: HashMap<BindIdentity, Vec<String>> = HashMap::new();
    for block in blocks {
        let field = |name: &str| {
            block
                .iter()
                .find(|(field, _)| *field == name)
                .map(|(_, value)| *value)
        };
        let (
            Some(modmask),
            Some(key),
            Some(dispatcher),
            Some(arg),
            Some(submap),
            Some(description),
        ) = (
            field("modmask").and_then(|mask| mask.parse().ok()),
            field("key"),
            field("dispatcher"),
            field("arg"),
            field("submap"),
            field("description"),
        )
        else {
            continue;
        };
        groups
            .entry(BindIdentity {
                dispatcher: dispatcher.to_owned(),
                arg: arg.to_owned(),
                modmask,
                submap: submap.to_owned(),
                description: description.to_owned(),
            })
            .or_default()
            .push(key.to_owned());
    }
    groups
}

/// The key of a Lua bind display string such as `SUPER + SHIFT + code:10`.
/// Hyprland requires modifiers first; a display naming several keys (a
/// multi-key bind) or whose modifiers disagree with `modmask` yields `None`.
fn display_key(display: &str, modmask: u32) -> Option<&str> {
    let mut mask = 0;
    let mut key = None;
    for segment in display.split('+').map(str::trim) {
        if key.is_none() {
            if let Some(bit) = lua_modifier_bit(segment) {
                mask |= bit;
                continue;
            }
        }
        if key.replace(segment).is_some() {
            return None;
        }
    }
    key.filter(|key| !key.is_empty() && *key != "catchall" && mask == modmask)
}

/// Modifier names accepted by Hyprland v0.56.2 Lua `modFromSv`
/// (case-sensitive).
fn lua_modifier_bit(name: &str) -> Option<u32> {
    Some(match name {
        "SHIFT" => 1,
        "CAPS" => 2,
        "CTRL" | "CONTROL" => 4,
        "ALT" | "MOD1" => 8,
        "MOD2" => 16,
        "MOD3" => 32,
        "SUPER" | "WIN" | "LOGO" | "MOD4" | "META" => 64,
        "MOD5" => 128,
        _ => return None,
    })
}

/// `keymap` is the main keyboard's compiled keymap and active layout group;
/// with it, `code:N` keys are displayed as the key that layout produces.
pub(super) fn collect_records(
    value: &serde_json::Value,
    lua_hints: &[LuaBindHint],
    keymap: Option<(&str, usize)>,
) -> Vec<BindingRecord> {
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
            .and_then(crate::layers::json_u32)
            .unwrap_or_default();
        let description = binding
            .get("description")
            .and_then(serde_json::Value::as_str)
            .filter(|description| !description.is_empty());
        let keycode = binding
            .get("keycode")
            .and_then(crate::layers::json_u32)
            .filter(|keycode| *keycode != 0);
        let catch_all = binding
            .get("catch_all")
            .and_then(boolish_value)
            .unwrap_or(false);
        // Lua binds with `code:` keys come back with neither a key nor a
        // keycode unless the text listing recovered it; fall back to a
        // literal config key or mark the record so it is never grouped under
        // a bare modifier prefix.
        let (raw_key, certainty) = if !key.is_empty() || catch_all {
            (Some(key.to_owned()), "runtime effective binding")
        } else if let Some(keycode) = keycode {
            (Some(format!("code:{keycode}")), "runtime effective binding")
        } else if let Some(hint_key) =
            description.and_then(|description| lua_hint_key(mask, description, lua_hints))
        {
            (
                Some(hint_key.to_owned()),
                "runtime effective binding; key from static config",
            )
        } else {
            (None, crate::bindings::UNRESOLVED_KEY_CERTAINTY)
        };
        let mut certainty = certainty.to_owned();
        let key = match raw_key {
            None => format!(
                "{}<key not reported>",
                crate::layers::combo_display(mask, "")
            ),
            Some(raw_key) => match code_key_symbol(&raw_key, keymap) {
                Some((code, symbol)) => {
                    certainty.push_str(&format!("{CODE_SHOWN_PREFIX}{code}{CODE_SHOWN_SUFFIX}"));
                    crate::layers::combo_display(mask, &symbol)
                }
                None => crate::layers::combo_display(mask, &raw_key),
            },
        };
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
        if let Some(description) = description {
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
            source: INVENTORY_SOURCE.into(),
            key,
            action,
            context: Some(submap),
            certainty,
        });
    }
    records
}

const CODE_SHOWN_PREFIX: &str = "; code:";
const CODE_SHOWN_SUFFIX: &str = " shown as its key in the active XKB layout";

/// The keycode of an inventory record whose `code:N` key is listed as its
/// symbol in the active layout, read back from the record's certainty.
pub(crate) fn displayed_keycode(certainty: &str) -> Option<u32> {
    let (_, rest) = certainty.split_once(CODE_SHOWN_PREFIX)?;
    rest.strip_suffix(CODE_SHOWN_SUFFIX)?.parse().ok()
}

/// The active-layout key name of a `code:N` binding key, with `N`.
fn code_key_symbol(raw_key: &str, keymap: Option<(&str, usize)>) -> Option<(u32, String)> {
    let (keymap, group) = keymap?;
    let code = crate::key::normalize_key_str(raw_key)
        .strip_prefix("CODE:")?
        .parse::<u32>()
        .ok()?;
    let symbol = crate::xkb::preferred_symbol_for_xkb_keycode(
        keymap,
        crate::xkb::XkbKeycode::from(code),
        group,
        0,
    )?;
    Some((code, symbol))
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
    let binds = query_bindings(|arguments| run_hyprctl_with_instance(arguments, instance));
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

pub(super) fn inspect_system(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    let bindings_raw = query_bindings(run_hyprctl)?;
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

pub(super) fn inspect_system_with_probe(
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

    let mut submap_note = None;
    let bindings_json = if active_submap == "__whykey_capture" {
        active_submap = match query_previous_submap_from_lua() {
            Ok(submap) => submap,
            Err(reason) => {
                submap_note = Some(format!(
                    "Hyprland is in whykey's capture submap and the pre-capture submap is unknown ({reason}); bindings were evaluated as in the default submap"
                ));
                "default".into()
            }
        };
        filter_whykey_capture_bindings(&bindings_raw)
    } else {
        bindings_raw
    };
    let keycodes = devices_json
        .as_deref()
        .map(|devices| xkb_keycodes_for_key(key, devices))
        .unwrap_or_default();
    let hints = &*CONFIG_HINTS;
    let lua_hints = hints.lua.as_slice();

    let mut result = analyze(
        key,
        &bindings_json,
        &active_submap,
        &keycodes,
        lua_hints,
        &hints.declarative,
        physical_input,
    )?
    .with_tier(EvidenceTier::LiveQuery);
    result.details.extend(submap_note);
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
    // The instance inventory does not change while one command runs, so
    // query it once instead of before every hyprctl call.
    static DISCOVERED: LazyLock<Option<String>> = LazyLock::new(|| {
        let mut command = Command::new("hyprctl");
        command.args(["instances", "-j"]);
        let output = command::output(&mut command).ok()?;
        if !output.status.success() {
            return None;
        }
        let instances = parse_hyprland_instances(&output.stdout)?;
        select_hyprland_instance(&instances, env::var("WAYLAND_DISPLAY").ok().as_deref())
    });
    DISCOVERED.clone()
}

/// Parse the instance inventory conservatively. Hyprland's inventory has
/// gained fields across releases, and a malformed/stale entry should not
/// hide otherwise usable instances from a multi-instance session.
pub(super) fn parse_hyprland_instances(bytes: &[u8]) -> Option<Vec<HyprlandInstance>> {
    let values: Vec<serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<HyprlandInstance>(value).ok())
            .filter(|instance| !instance.instance.trim().is_empty())
            .collect(),
    )
}

pub(super) fn has_hyprland_signature() -> bool {
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

pub(super) fn select_hyprland_instance(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_submap_reply_separates_answers_from_failures() {
        // Genuine answers, including names the old sentinels swallowed.
        assert_eq!(
            parse_previous_submap_reply("whykey-submap:\n").as_deref(),
            Ok("default")
        );
        assert_eq!(
            parse_previous_submap_reply("whykey-submap:none\n").as_deref(),
            Ok("none")
        );
        assert_eq!(
            parse_previous_submap_reply("whykey-submap:resize").as_deref(),
            Ok("resize")
        );
        // Failures never become a submap name.
        for reply in [
            "whykey-submap-unset:nil\n",
            "unknown request\n",
            "default\n",
            "",
            "error: [string \"...\"]:1: attempt to index a nil value\n",
        ] {
            assert!(parse_previous_submap_reply(reply).is_err(), "{reply:?}");
        }
    }
}
