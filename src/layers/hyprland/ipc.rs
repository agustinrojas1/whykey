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

    let mut result = analyze(
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
