//! KDE Plasma global shortcuts.
//!
//! The running kglobalaccel daemon is asked over D-Bus (`busctl --user`,
//! never auto-starting it): `globalShortcutsByKey` finds the shortcuts that
//! use a key and each component's `isActive` says whether kglobalaccel still
//! grabs them. Without the daemon, `kglobalshortcutsrc`, the file it
//! persists to, is a configured-only hint.

use crate::recording::PathInputs;
use std::path::PathBuf;
use std::process::Command;

use crate::key::{KeyCombo, modifier_mask, normalize_key_str};
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, EvidenceTier, LayerId, LayerResult, Outcome,
    SourceLocation, UncertaintyReason,
};

const LAYER: &str = "KDE Plasma";
const SERVICE: &str = "org.kde.kglobalaccel";
const ROOT_OBJECT: &str = "/kglobalaccel";
const ROOT_INTERFACE: &str = "org.kde.KGlobalAccel";
const COMPONENT_INTERFACE: &str = "org.kde.kglobalaccel.Component";

/// Read-only KDE Plasma global shortcut adapter.
pub struct Kde;

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    crate::recording::env_var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(crate::recording::env_var("XDG_SESSION_DESKTOP").ok())
        .flat_map(|desktop| desktop.split(':').map(str::to_owned).collect::<Vec<_>>())
        .any(|desktop| {
            matches!(
                desktop.trim().to_ascii_lowercase().as_str(),
                "kde" | "kde plasma" | "plasma" | "plasmawayland" | "plasma x11"
            )
        })
}

pub fn ipc_available() -> bool {
    runtime_reachable().is_ok() || config_path().is_some()
}

/// Whether the kglobalaccel daemon answers; the error explains why not.
pub(crate) fn runtime_reachable() -> Result<usize, String> {
    let reply = call(ROOT_OBJECT, ROOT_INTERFACE, "allComponents", &[])?;
    Ok(first_arg(&reply)
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len))
}

/// Return KDE bindings: live kglobalaccel registrations, or the persisted
/// configuration when the daemon is not running.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    match runtime_inventory() {
        Ok(records) => Ok(records),
        Err(runtime_error) => file_inventory().map_err(|file_error| {
            format!("KDE Plasma: kglobalaccel D-Bus: {runtime_error}; {file_error}")
        }),
    }
}

fn runtime_inventory() -> Result<Vec<BindingRecord>, String> {
    let reply = call(ROOT_OBJECT, ROOT_INTERFACE, "allComponents", &[])?;
    let paths = first_arg(&reply)
        .and_then(serde_json::Value::as_array)
        .ok_or("allComponents reply has no object paths")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    for path in paths {
        let active = component_active(&path)?;
        let infos = call(&path, COMPONENT_INTERFACE, "allShortcutInfos", &[])?;
        for info in shortcut_infos(&infos) {
            for code in &info.keys {
                let Some(combo) = decode_qt_key(*code) else {
                    continue;
                };
                records.push(
                    BindingRecord::new(
                        LAYER,
                        combo.compact_display(),
                        format!("{} ({})", info.action_name, info.action),
                        if active {
                            "live kglobalaccel"
                        } else {
                            "live kglobalaccel; component inactive, not grabbed"
                        },
                    )
                    .with_context(info.component_name.clone()),
                );
            }
        }
    }
    Ok(records)
}

fn file_inventory() -> Result<Vec<BindingRecord>, String> {
    let path = config_path().ok_or("kglobalshortcutsrc was not found")?;
    let content = crate::recording::read_to_string(&path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok(parse_config(&content)
        .bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                LAYER,
                binding.combo.compact_display(),
                binding.description.map_or_else(
                    || binding.action.clone(),
                    |description| format!("{} ({description})", binding.action),
                ),
                "configured in kglobalshortcutsrc; kglobalaccel not running",
            )
            .with_context(binding.group)
        })
        .collect())
}

impl Kde {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        if key.is_alone() {
            return inspect_alone(key, read_kwinrc());
        }
        let Some(codes) = encode_qt_key(key) else {
            return LayerResult::pass(
                LAYER,
                LayerId::Compositor,
                "no KDE global shortcut can use this key",
                vec![
                    "Qt key sequences cannot express this key or modifier, so kglobalaccel cannot bind it"
                        .into(),
                ],
            );
        };
        match runtime_matches(&codes) {
            Ok(matches) => {
                let owner = wayland_owner_missing_keys();
                let inhibitable = inhibitable(
                    crate::recording::env_var("XDG_SESSION_TYPE")
                        .ok()
                        .as_deref(),
                    key,
                );
                let mut result = inspect_runtime(&matches, owner.as_deref(), inhibitable);
                if let Some(alias) = us_shifted_alias(key) {
                    result.details.push(format!(
                        "also asked for {} assuming a US layout: Qt records Shift+digit as the shifted symbol",
                        alias.compact_display()
                    ));
                }
                result
            }
            Err(error) => inspect_file(key, &error),
        }
    }
}

/// `kwinrc`, or `Ok(None)` when it does not exist.
fn read_kwinrc() -> Result<Option<(PathBuf, String)>, String> {
    let Some(path) = crate::util::config_base().map(|base| base.join("kwinrc")) else {
        return Ok(None);
    };
    if !path.recorded_is_file() {
        return Ok(None);
    }
    crate::recording::read_to_string(&path)
        .map(|content| Some((path.clone(), content)))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The value of one key in `[ModifierOnlyShortcuts]` of a `kwinrc`.
fn modifier_only_shortcut(content: &str, name: &str) -> Option<String> {
    let mut in_group = false;
    let mut value = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_group = &line[1..line.len() - 1] == "ModifierOnlyShortcuts";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((key, found)) = line.split_once('=') {
            if key.trim() == name {
                // KConfig: the last assignment wins.
                value = Some(found.trim().to_owned());
            }
        }
    }
    value
}

/// A lone modifier on KDE: KWin's `[ModifierOnlyShortcuts]` group in
/// `kwinrc` names the D-Bus call it makes when the modifier is pressed and
/// released on its own (Plasma sets `Meta` to open the launcher).
fn inspect_alone(key: &KeyCombo, kwinrc: Result<Option<(PathBuf, String)>, String>) -> LayerResult {
    let shown = crate::style::human_key(key);
    let name = match key.alone_mask() {
        crate::key::CTRL_MASK => "Control",
        crate::key::ALT_MASK => "Alt",
        crate::key::SHIFT_MASK => "Shift",
        crate::key::SUPER_MASK => "Meta",
        _ => {
            return LayerResult::pass(
                LAYER,
                LayerId::Compositor,
                format!("KWin has no modifier-only shortcut for {shown}"),
                vec!["KWin's [ModifierOnlyShortcuts] covers Shift, Control, Alt and Meta".into()],
            );
        }
    };
    let (path, content) = match kwinrc {
        Ok(Some(found)) => found,
        Ok(None) => {
            return LayerResult::new(
                LAYER,
                LayerId::Compositor,
                Outcome::Pass,
                format!("no modifier-only shortcut configured for {shown}"),
                vec!["kwinrc was not found; KWin then defines no [ModifierOnlyShortcuts]".into()],
            )
            .with_tier(EvidenceTier::ConfigFile);
        }
        Err(error) => {
            return LayerResult::unavailable(
                LAYER,
                LayerId::Compositor,
                "could not read KWin's modifier-only shortcuts",
                vec![error],
            );
        }
    };
    let mut details = vec![format!("config: {}", path.display())];
    let value = modifier_only_shortcut(&content, name).filter(|value| !value.is_empty());
    let Some(value) = value else {
        details.push(format!("[ModifierOnlyShortcuts] has no {name} entry"));
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            format!("no modifier-only shortcut configured for {shown}"),
            details,
        )
        .with_tier(EvidenceTier::ConfigFile);
    };
    let fields: Vec<&str> = value.split(',').map(str::trim).collect();
    let action = match fields.as_slice() {
        [service, object, interface, method, ..] => {
            format!("{service} {object} {interface}.{method}")
        }
        _ => value.clone(),
    };
    details.push(format!(
        "binding: [ModifierOnlyShortcuts] {name}={value} (KWin calls it when {shown} is pressed and released with no other key)"
    ));
    details.push(
        "the configuration is read from the file; a running KWin may have been changed through D-Bus since"
            .into(),
    );
    let evidence = BindingEvidence {
        dispatcher: Some("KWin".into()),
        action: Some(action),
        description: Some(format!("ModifierOnlyShortcuts {name}")),
        submap: None,
        scope: BindingScope::Universal,
        source: Some(SourceLocation::new(path.display().to_string(), None)),
        has_universal_match: false,
        uncertainty: Some(UncertaintyReason::EndpointUnavailable),
    };
    LayerResult::new(
        LAYER,
        LayerId::Compositor,
        Outcome::HandledUncertain,
        format!("KWin runs a D-Bus call when {shown} is pressed and released"),
        details,
    )
    .with_binding(evidence)
    .with_tier(EvidenceTier::ConfigFile)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShortcutInfo {
    action: String,
    action_name: String,
    component: String,
    component_name: String,
    keys: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMatch {
    info: ShortcutInfo,
    active: bool,
}

fn runtime_matches(codes: &[i32]) -> Result<Vec<RuntimeMatch>, String> {
    let mut infos: Vec<ShortcutInfo> = Vec::new();
    for code in codes {
        let code = code.to_string();
        // globalShortcutsByKey(QKeySequence key, MatchType type): one key
        // in the sequence, MatchType Equal (0).
        let reply = call(
            ROOT_OBJECT,
            ROOT_INTERFACE,
            "globalShortcutsByKey",
            &["(ai)(i)", "1", &code, "0"],
        )?;
        for info in shortcut_infos(&reply) {
            if !infos.contains(&info) {
                infos.push(info);
            }
        }
    }
    let mut matches = Vec::new();
    for info in infos {
        let reply = call(
            ROOT_OBJECT,
            ROOT_INTERFACE,
            "getComponent",
            &["s", &info.component],
        )?;
        let path = first_arg(&reply)
            .and_then(serde_json::Value::as_str)
            .ok_or("getComponent reply has no object path")?
            .to_owned();
        matches.push(RuntimeMatch {
            active: component_active(&path)?,
            info,
        });
    }
    Ok(matches)
}

fn inspect_runtime(
    matches: &[RuntimeMatch],
    wayland_owner: Option<&str>,
    inhibitable: bool,
) -> LayerResult {
    let mut details = vec![format!(
        "source: live kglobalaccel D-Bus ({SERVICE} globalShortcutsByKey)"
    )];
    for found in matches {
        details.push(format!(
            "shortcut: {} / {} ({}/{}){}",
            found.info.component_name,
            found.info.action_name,
            found.info.component,
            found.info.action,
            if found.active {
                ""
            } else {
                " — component inactive: kglobalaccel does not grab its keys"
            }
        ));
    }
    if let Some(owner) = wayland_owner {
        if matches.iter().any(|found| found.active) {
            details.push(format!(
                "{SERVICE} is owned by {owner}, not kwin_wayland: KWin's in-process shortcut registry did not start, so KWin never reads this key"
            ));
            return LayerResult::pass(
                LAYER,
                LayerId::Compositor,
                "KDE global shortcut is registered, but KWin does not read Wayland keys",
                details,
            )
            .with_tier(EvidenceTier::LiveQuery);
        }
    }
    let Some(active) = matches.iter().find(|found| found.active) else {
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            if matches.is_empty() {
                "no KDE global shortcut uses this key"
            } else {
                "KDE global shortcut registered but inactive"
            },
            details,
        )
        .with_tier(EvidenceTier::LiveQuery);
    };
    details.push(
        "kglobalaccel reports activity per component (isActive); a single action its application released while others stay registered still looks active"
            .into(),
    );
    if inhibitable {
        details.push(
            "not active while the focused window holds a keyboard-shortcuts inhibitor (KWin grants one without asking) or an Xwayland keyboard grab"
                .into(),
        );
    }
    let evidence = BindingEvidence::action(
        active.info.action_name.clone(),
        BindingScope::Universal,
        Some(SourceLocation::new(
            format!("kglobalaccel component {}", active.info.component),
            None,
        )),
    )
    .with_dispatcher("KGlobalAccel")
    .with_description(active.info.action.clone());
    LayerResult::new(
        LAYER,
        LayerId::Compositor,
        Outcome::Consumed,
        "active KDE global shortcut consumes the key",
        details,
    )
    .with_binding(evidence)
    .with_tier(EvidenceTier::LiveQuery)
}

fn inspect_file(key: &KeyCombo, runtime_error: &str) -> LayerResult {
    let runtime_line = format!("kglobalaccel D-Bus unavailable: {runtime_error}");
    let Some(path) = config_path() else {
        return LayerResult::unavailable(
            LAYER,
            LayerId::Compositor,
            "KDE global shortcuts are unavailable",
            vec![runtime_line, "kglobalshortcutsrc was not found".into()],
        );
    };
    let content = match crate::recording::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            return LayerResult::unavailable(
                LAYER,
                LayerId::Compositor,
                "could not read KDE global shortcut configuration",
                vec![runtime_line, format!("config: {}: {error}", path.display())],
            );
        }
    };
    let parsed = parse_config(&content);
    let mut details = vec![runtime_line, format!("config: {}", path.display())];
    let alias = us_shifted_alias(key);
    let matches = parsed
        .bindings
        .iter()
        .filter(|binding| binding.combo == *key || alias.as_ref() == Some(&binding.combo))
        .collect::<Vec<_>>();
    if let Some(alias) = alias
        .as_ref()
        .filter(|alias| matches.iter().any(|binding| binding.combo == **alias))
    {
        details.push(format!(
            "matched {} as {} assuming a US layout: Qt records Shift+digit as the shifted symbol",
            alias.compact_display(),
            key.compact_display()
        ));
    }
    let Some(first) = matches.first() else {
        details.push(
            "no configured shortcut uses this key; applications register theirs with kglobalaccel at runtime"
                .into(),
        );
        details.extend(parsed.unparsed);
        return LayerResult::new(
            LAYER,
            LayerId::Compositor,
            Outcome::Unknown,
            "KDE global shortcut state is conditional",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile);
    };
    for binding in &matches {
        let action = binding.description.as_deref().map_or_else(
            || binding.action.clone(),
            |description| format!("{} ({description})", binding.action),
        );
        details.push(format!("{}: {action}", binding.group));
    }
    let evidence = BindingEvidence {
        dispatcher: Some("KGlobalAccel".into()),
        action: Some(first.action.clone()),
        description: first.description.clone(),
        submap: None,
        scope: BindingScope::Unknown,
        source: Some(SourceLocation::new(path.display().to_string(), None)),
        has_universal_match: false,
        uncertainty: Some(UncertaintyReason::EndpointUnavailable),
    };
    LayerResult::new(
        LAYER,
        LayerId::Compositor,
        Outcome::HandledUncertain,
        "matching KDE global shortcut configured; kglobalaccel is not running to confirm it",
        details,
    )
    .with_binding(evidence)
    .with_tier(EvidenceTier::ConfigFile)
}

/// The process that owns `org.kde.kglobalaccel`, when it is kglobalacceld on
/// a Wayland session. On Wayland KWin reads keys through the registry it runs
/// in-process, and only the KWin that won the bus name has that registry;
/// kglobalacceld exits at once on Wayland, so a kglobalacceld that owns the
/// name (started first, without XDG_SESSION_TYPE) never sees a key press.
/// Everything else returns None: the owner is not known to miss keys.
fn wayland_owner_missing_keys() -> Option<String> {
    let session = crate::recording::env_var("XDG_SESSION_TYPE").ok();
    if session.as_deref() != Some("wayland") {
        return None;
    }
    let owner = owner_process_name()?;
    owner_misses_wayland_keys(session.as_deref(), &owner).then_some(owner)
}

fn owner_misses_wayland_keys(session_type: Option<&str>, owner: &str) -> bool {
    session_type == Some("wayland") && owner == "kglobalacceld"
}

/// `/proc/<pid>/comm` of the connection that owns `org.kde.kglobalaccel`.
fn owner_process_name() -> Option<String> {
    let mut command = Command::new("busctl");
    command.args([
        "--user",
        "--auto-start=no",
        "--json=short",
        "call",
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetConnectionUnixProcessID",
        "s",
        SERVICE,
    ]);
    let output = crate::command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let pid = busctl_pid(&output.stdout)?;
    let comm = crate::recording::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end().to_owned())
}

/// The PID in a `busctl --json=short` reply to GetConnectionUnixProcessID:
/// `{"type":"u","data":[1234]}` (the return values are an array).
fn busctl_pid(stdout: &[u8]) -> Option<u64> {
    let reply: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    reply.get("data")?.get(0)?.as_u64()
}

/// Whether a focused client can suspend a KWin shortcut on this key. On
/// Wayland, KWin's GlobalShortcutFilter skips global shortcuts while the
/// focused surface holds a keyboard-shortcuts inhibitor or an Xwayland
/// keyboard grab (`WaylandServer::isKeyboardShortcutsInhibited`); only the
/// power key is handled before that check (kwin src/input.cpp). No KWin
/// interface reports the inhibitor, so whykey can only say that it may apply.
fn inhibitable(session_type: Option<&str>, key: &KeyCombo) -> bool {
    session_type == Some("wayland") && key.key() != normalize_key_str("XF86PowerOff")
}

fn component_active(path: &str) -> Result<bool, String> {
    let reply = call(path, COMPONENT_INTERFACE, "isActive", &[])?;
    first_arg(&reply)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| format!("isActive reply for {path} is not a boolean"))
}

/// `busctl --user --json=short call` against kglobalaccel. `--auto-start=no`
/// keeps the probe from launching the daemon.
fn call(
    object: &str,
    interface: &str,
    method: &str,
    args: &[&str],
) -> Result<serde_json::Value, String> {
    let mut command = Command::new("busctl");
    command
        .args([
            "--user",
            "--auto-start=no",
            "--json=short",
            "call",
            SERVICE,
            object,
            interface,
            method,
        ])
        .args(args);
    let output =
        crate::command::output(&mut command).map_err(|error| format!("busctl: {error}"))?;
    if !output.status.success() {
        return Err(super::gsettings::command_error(
            "busctl",
            output.status,
            &output.stderr,
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("busctl returned invalid {method} JSON: {error}"))
}

fn first_arg(reply: &serde_json::Value) -> Option<&serde_json::Value> {
    reply.get("data")?.as_array()?.first()
}

/// Decode `a(ssssssaiai)` KGlobalShortcutInfo lists: action unique and
/// friendly name, component unique and friendly name, context unique and
/// friendly name, keys, default keys.
fn shortcut_infos(reply: &serde_json::Value) -> Vec<ShortcutInfo> {
    let Some(entries) = first_arg(reply).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let fields = entry.as_array()?;
            let text = |index: usize| Some(fields.get(index)?.as_str()?.to_owned());
            Some(ShortcutInfo {
                action: text(0)?,
                action_name: text(1)?,
                component: text(2)?,
                component_name: text(3)?,
                keys: fields
                    .get(6)?
                    .as_array()?
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .filter_map(|code| i32::try_from(code).ok())
                    .collect(),
            })
        })
        .collect()
}

// Qt::KeyboardModifier flags packed into a QKeyCombination int.
const QT_SHIFT: i32 = 0x0200_0000;
const QT_CONTROL: i32 = 0x0400_0000;
const QT_ALT: i32 = 0x0800_0000;
const QT_META: i32 = 0x1000_0000;
const QT_KEYPAD: i32 = 0x2000_0000;
const QT_GROUP_SWITCH: i32 = 0x4000_0000;
const QT_MODIFIERS: i32 = QT_SHIFT | QT_CONTROL | QT_ALT | QT_META | QT_KEYPAD | QT_GROUP_SWITCH;

const QT_KEY_BACKTAB: i32 = 0x0100_0002;
const QT_KEY_F1: i32 = 0x0100_0030;
const QT_KEY_F35: i32 = 0x0100_0052;

/// Qt::Key values (qnamespace.h), the XKB keysym Qt maps to each
/// (qxkbcommon.cpp), and the QKeySequence portable names
/// (qkeysequence.cpp) used in kglobalshortcutsrc. The first row for a
/// keysym is the one used to ask kglobalaccel.
const QT_KEYS: &[(i32, &str, &[&str])] = &[
    (0x20, "space", &["Space"]),
    (0x0100_0000, "Escape", &["Esc", "Escape"]),
    (0x0100_0001, "Tab", &["Tab"]),
    (0x0100_0003, "BackSpace", &["Backspace"]),
    (0x0100_0004, "Return", &["Return"]),
    (0x0100_0005, "KP_Enter", &["Enter"]),
    (0x0100_0006, "Insert", &["Ins", "Insert"]),
    (0x0100_0007, "Delete", &["Del", "Delete"]),
    (0x0100_0008, "Pause", &["Pause"]),
    (0x0100_0009, "Print", &["Print", "Print Screen"]),
    (0x0100_000a, "Sys_Req", &["SysReq", "System Request"]),
    (0x0100_000b, "Clear", &["Clear"]),
    (0x0100_0010, "Home", &["Home"]),
    (0x0100_0011, "End", &["End"]),
    (0x0100_0012, "Left", &["Left"]),
    (0x0100_0013, "Up", &["Up"]),
    (0x0100_0014, "Right", &["Right"]),
    (0x0100_0015, "Down", &["Down"]),
    (0x0100_0016, "Page_Up", &["PgUp", "Page Up"]),
    (0x0100_0017, "Page_Down", &["PgDown", "Page Down"]),
    (0x0100_0024, "Caps_Lock", &["CapsLock", "Caps Lock"]),
    (
        0x0100_0025,
        "Num_Lock",
        &["NumLock", "Num Lock", "Number Lock"],
    ),
    (0x0100_0026, "Scroll_Lock", &["ScrollLock", "Scroll Lock"]),
    (0x0100_0053, "Super_L", &[]),
    (0x0100_0054, "Super_R", &[]),
    (0x0100_0055, "Menu", &["Menu"]),
    (0x0100_0056, "Hyper_L", &[]),
    (0x0100_0057, "Hyper_R", &[]),
    (0x0100_0058, "Help", &["Help"]),
    (0x0100_0061, "XF86Back", &["Back"]),
    (0x0100_0062, "XF86Forward", &["Forward"]),
    (0x0100_0063, "XF86Stop", &["Stop"]),
    (0x0100_0064, "XF86Refresh", &["Refresh"]),
    (0x0100_0070, "XF86AudioLowerVolume", &["Volume Down"]),
    (0x0100_0071, "XF86AudioMute", &["Volume Mute"]),
    (0x0100_0072, "XF86AudioRaiseVolume", &["Volume Up"]),
    (0x0100_0080, "XF86AudioPlay", &["Media Play"]),
    (0x0100_0081, "XF86AudioStop", &["Media Stop"]),
    (0x0100_0082, "XF86AudioPrev", &["Media Previous"]),
    (0x0100_0083, "XF86AudioNext", &["Media Next"]),
    (0x0100_0084, "XF86AudioRecord", &["Media Record"]),
    (0x0100_0085, "XF86AudioPause", &["Media Pause"]),
    (0x0100_0090, "XF86HomePage", &["Home Page"]),
    (0x0100_0091, "XF86Favorites", &["Favorites"]),
    (0x0100_0092, "XF86Search", &["Search"]),
    (0x0100_0093, "XF86Standby", &["Standby"]),
    (0x0100_00a0, "XF86Mail", &["Launch Mail"]),
    (0x0100_00a1, "XF86AudioMedia", &["Launch Media"]),
    (0x0100_00a2, "XF86Launch0", &["Launch (0)"]),
    (0x0100_00a3, "XF86Launch1", &["Launch (1)"]),
    (0x0100_00a4, "XF86Launch2", &["Launch (2)"]),
    (0x0100_00a5, "XF86Launch3", &["Launch (3)"]),
    (0x0100_00a6, "XF86Launch4", &["Launch (4)"]),
    (0x0100_00a7, "XF86Launch5", &["Launch (5)"]),
    (0x0100_00a8, "XF86Launch6", &["Launch (6)"]),
    (0x0100_00a9, "XF86Launch7", &["Launch (7)"]),
    (0x0100_00aa, "XF86Launch8", &["Launch (8)"]),
    (0x0100_00ab, "XF86Launch9", &["Launch (9)"]),
    (0x0100_00ac, "XF86LaunchA", &["Launch (A)"]),
    (0x0100_00ad, "XF86LaunchB", &["Launch (B)"]),
    (0x0100_00ae, "XF86LaunchC", &["Launch (C)"]),
    (0x0100_00af, "XF86LaunchD", &["Launch (D)"]),
    (0x0100_00b0, "XF86LaunchE", &["Launch (E)"]),
    (0x0100_00b1, "XF86LaunchF", &["Launch (F)"]),
    (
        0x0100_00b2,
        "XF86MonBrightnessUp",
        &["Monitor Brightness Up"],
    ),
    (
        0x0100_00b3,
        "XF86MonBrightnessDown",
        &["Monitor Brightness Down"],
    ),
    (0x0100_00b4, "XF86KbdLightOnOff", &["Keyboard Light On/Off"]),
    (
        0x0100_00b5,
        "XF86KbdBrightnessUp",
        &["Keyboard Brightness Up"],
    ),
    (
        0x0100_00b6,
        "XF86KbdBrightnessDown",
        &["Keyboard Brightness Down"],
    ),
    (0x0100_00b7, "XF86PowerOff", &["Power Off"]),
    (0x0100_00b8, "XF86WakeUp", &["Wake Up"]),
    (0x0100_00b9, "XF86Eject", &["Eject"]),
    (0x0100_00ba, "XF86ScreenSaver", &["Screensaver"]),
    (0x0100_00bb, "XF86WWW", &["WWW"]),
    (0x0100_00cb, "XF86Calculator", &["Calculator"]),
    (0x0100_00cf, "XF86Copy", &["Copy"]),
    (0x0100_00d0, "XF86Cut", &["Cut"]),
    (0x0100_00d1, "XF86Display", &["Display"]),
    (0x0100_00d5, "XF86Explorer", &["Browser"]),
    (0x0100_00e2, "XF86Paste", &["Paste"]),
    (0x0100_00e4, "XF86Calendar", &["Calendar"]),
    (0x0100_00f1, "XF86Tools", &["Tools"]),
    (0x0100_00fe, "XF86Battery", &["Battery"]),
    (0x0100_00ff, "XF86Bluetooth", &["Bluetooth"]),
    (0x0100_0100, "XF86WLAN", &["Wireless"]),
    (0x0100_0108, "XF86Hibernate", &["Hibernate"]),
    (0x0100_010c, "XF86Suspend", &["Suspend"]),
    (0x0100_0110, "XF86TouchpadToggle", &["Touchpad Toggle"]),
    (0x0100_0111, "XF86TouchpadOn", &["Touchpad On"]),
    (0x0100_0112, "XF86TouchpadOff", &["Touchpad Off"]),
    (0x0100_0113, "XF86AudioMicMute", &["Microphone Mute"]),
    (0x0100_0122, "Find", &["Find"]),
    (0x0102_0004, "XF86Sleep", &["Sleep"]),
];

/// QKeySequence name of `Qt::Key_MediaTogglePlayPause`. No XKB keysym
/// produces that key: Qt and KWin map XF86AudioPlay to `Key_MediaPlay`
/// (qxkbcommon.cpp), so a shortcut on it cannot fire from a keyboard.
const QT_NAME_MEDIA_TOGGLE_PLAY_PAUSE: &str = "Toggle Media Play/Pause";

/// Keypad keysyms and the main key Qt reports for them together with
/// `KeypadModifier` (`Num+` in portable text). Keypad Enter is a key of
/// its own (`Key_Enter`) and is handled through [`QT_KEYS`].
const QT_KEYPAD_KEYS: &[(&str, &str)] = &[
    ("KP_0", "0"),
    ("KP_1", "1"),
    ("KP_2", "2"),
    ("KP_3", "3"),
    ("KP_4", "4"),
    ("KP_5", "5"),
    ("KP_6", "6"),
    ("KP_7", "7"),
    ("KP_8", "8"),
    ("KP_9", "9"),
    ("KP_DECIMAL", "."),
    ("KP_SEPARATOR", ","),
    ("KP_DIVIDE", "/"),
    ("KP_MULTIPLY", "*"),
    ("KP_SUBTRACT", "-"),
    ("KP_ADD", "+"),
    ("KP_EQUAL", "="),
    ("KP_HOME", "HOME"),
    ("KP_END", "END"),
    ("KP_LEFT", "LEFT"),
    ("KP_UP", "UP"),
    ("KP_RIGHT", "RIGHT"),
    ("KP_DOWN", "DOWN"),
    ("KP_PAGE_UP", "PAGE_UP"),
    ("KP_PAGE_DOWN", "PAGE_DOWN"),
    ("KP_INSERT", "INSERT"),
    ("KP_DELETE", "DELETE"),
    ("KP_BEGIN", "CLEAR"),
];

fn modifier(name: &str) -> i32 {
    modifier_mask(name).map_or(0, |mask| mask as i32)
}

/// Decode one QKeyCombination int (`Qt::Key | Qt::KeyboardModifiers`).
fn decode_qt_key(code: i32) -> Option<KeyCombo> {
    if code <= 0 || code & QT_GROUP_SWITCH != 0 {
        return None;
    }
    let mut mask = 0;
    for (flag, name) in [
        (QT_SHIFT, "shift"),
        (QT_CONTROL, "ctrl"),
        (QT_ALT, "alt"),
        (QT_META, "super"),
    ] {
        if code & flag != 0 {
            mask |= modifier(name);
        }
    }
    let key = code & !QT_MODIFIERS;
    let mut name = if key == QT_KEY_BACKTAB {
        // Qt reports Shift+Tab as Backtab.
        mask |= modifier("shift");
        "Tab".to_owned()
    } else if (QT_KEY_F1..=QT_KEY_F35).contains(&key) {
        format!("F{}", key - QT_KEY_F1 + 1)
    } else if let Some((_, keysym, _)) = QT_KEYS.iter().find(|(qt, _, _)| *qt == key) {
        (*keysym).to_owned()
    } else if (0x21..0x0100_0000).contains(&key) {
        // Printable keys are their (uppercase) Unicode code point.
        char::from_u32(u32::try_from(key).ok()?)?.to_string()
    } else {
        return None;
    };
    if code & QT_KEYPAD != 0 {
        let main = normalize_key_str(&name);
        if let Some((keypad, _)) = QT_KEYPAD_KEYS.iter().find(|(_, key)| *key == main) {
            name = (*keypad).to_owned();
        }
    }
    Some(KeyCombo::from_parts(u32::try_from(mask).ok()?, name))
}

/// The shortcut KDE records for a Shift+digit query on a US layout,
/// `super+shift+1` → `super+!`: the shifted symbol, with Shift folded into
/// it. Lab-verified with kglobalacceld 6.7.5 on X11: `Meta+!` fires for
/// Super+Shift+1, `Meta+Shift+!` does not.
fn us_shifted_alias(combo: &KeyCombo) -> Option<KeyCombo> {
    let shift = modifier("shift") as u32;
    if combo.modmask() & shift == 0 {
        return None;
    }
    let mut characters = combo.key().chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    let shifted = crate::remapper::keys::us_shifted_symbol(character)?;
    Some(KeyCombo::from_parts(
        combo.modmask() & !shift,
        shifted.to_string(),
    ))
}

/// The QKeyCombination ints that represent `combo`, or `None` when Qt cannot
/// express it (Hyper/Mod5/lock modifiers, raw keycodes, unknown keys).
fn encode_qt_key(combo: &KeyCombo) -> Option<Vec<i32>> {
    let mask = i32::try_from(combo.modmask()).ok()?;
    let mut flags = 0;
    let mut covered = 0;
    for (flag, name) in [
        (QT_SHIFT, "shift"),
        (QT_CONTROL, "ctrl"),
        (QT_ALT, "alt"),
        (QT_META, "super"),
    ] {
        let bit = modifier(name);
        if mask & bit != 0 {
            flags |= flag;
            covered |= bit;
        }
    }
    if mask & !covered != 0 {
        return None;
    }
    let mut key = combo.key();
    if let Some((_, main)) = QT_KEYPAD_KEYS.iter().find(|(keypad, _)| *keypad == key) {
        flags |= QT_KEYPAD;
        key = main;
    }
    let mut codes = QT_KEYS
        .iter()
        .filter(|(_, keysym, _)| normalize_key_str(keysym) == key)
        .map(|(qt, _, _)| flags | qt)
        .collect::<Vec<_>>();
    if codes.is_empty() {
        if let Some(number) = key
            .strip_prefix('F')
            .and_then(|number| number.parse::<i32>().ok())
            .filter(|number| (1..=35).contains(number))
        {
            codes.push(flags | (QT_KEY_F1 + number - 1));
        } else {
            let mut characters = key.chars();
            let character = characters.next()?;
            if characters.next().is_some() {
                return None;
            }
            let upper = character.to_uppercase().next()?;
            codes.push(flags | i32::try_from(u32::from(upper)).ok()?);
        }
    }
    // Qt reports keypad Enter as Key_Enter with or without KeypadModifier.
    if key == "KP_ENTER" {
        codes.push(flags | QT_KEYPAD | 0x0100_0005);
    }
    if let Some(alias) = us_shifted_alias(combo) {
        let symbol = alias.key().chars().next()?;
        codes.push((flags & !QT_SHIFT) | i32::try_from(u32::from(symbol)).ok()?);
    }
    Some(codes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    group: String,
    action: String,
    description: Option<String>,
}

#[derive(Debug, Default)]
struct ParsedConfig {
    bindings: Vec<Binding>,
    /// Shortcuts whose key names whykey could not map.
    unparsed: Vec<String>,
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = crate::recording::env_var_os("KGLOBALSHORTCUTS_CONFIG") {
        let path = PathBuf::from(path);
        if path.recorded_is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("kglobalshortcutsrc");
    path.recorded_is_file().then_some(path)
}

/// Parse kglobalshortcutsrc. Each entry is a KConfig list
/// `current keys,default keys,friendly name`; alternatives within a key
/// field are tab-separated (written as `\t`).
fn parse_config(content: &str) -> ParsedConfig {
    let mut group = String::from("unknown KDE component");
    let mut parsed = ParsedConfig::default();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            // Plasma 6 nests .desktop launchers: `[services][app.desktop]`.
            group = line[1..line.len() - 1].replace("][", "/");
            continue;
        }
        let Some((action, value)) = line.split_once('=') else {
            continue;
        };
        let action = action.trim();
        if action.starts_with("_k_") {
            continue;
        }
        let fields = kconfig_list(value);
        let Some(current) = fields.first() else {
            continue;
        };
        let description = fields
            .get(2)
            .filter(|description| !description.is_empty())
            .cloned();
        for sequence in current.split('\t') {
            // Only the first chord of a multi-key sequence can be grabbed.
            let chord = sequence.split(", ").next().unwrap_or("").trim();
            if chord.is_empty() || chord.eq_ignore_ascii_case("none") {
                continue;
            }
            match parse_portable(chord) {
                Some(combo) => parsed.bindings.push(Binding {
                    combo,
                    group: group.clone(),
                    action: action.to_owned(),
                    description: description.clone(),
                }),
                None => parsed.unparsed.push(
                    if chord
                        .rsplit('+')
                        .next()
                        .is_some_and(|key| key.eq_ignore_ascii_case(QT_NAME_MEDIA_TOGGLE_PLAY_PAUSE))
                    {
                        format!(
                            "not compared: [{group}] {action}={chord}: no XKB keysym produces Qt's Toggle Media Play/Pause key (XF86AudioPlay is Media Play)"
                        )
                    } else {
                        format!("not compared: [{group}] {action}={chord}")
                    },
                ),
            }
        }
    }
    parsed
}

/// Split a KConfig string list on unescaped commas, resolving KConfig
/// escapes (`\t`, `\n`, `\s`, `\\`, `\,`, `\;`).
fn kconfig_list(value: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut characters = value.trim().chars();
    while let Some(character) = characters.next() {
        let field = fields.last_mut().expect("fields is never empty");
        match character {
            '\\' => match characters.next() {
                Some('t') => field.push('\t'),
                Some('n') => field.push('\n'),
                Some('s') => field.push(' '),
                Some(other) => field.push(other),
                None => field.push('\\'),
            },
            ',' => fields.push(String::new()),
            other => field.push(other),
        }
    }
    fields
}

/// Parse one QKeySequence portable-text chord (`Meta+Shift+Print`,
/// `Ctrl+Alt+Del`, `Volume Up`).
fn parse_portable(chord: &str) -> Option<KeyCombo> {
    let mut flags = 0;
    let mut rest = chord;
    while let Some((name, tail)) = rest.split_once('+') {
        let flag = match name.to_ascii_lowercase().as_str() {
            "meta" => QT_META,
            "ctrl" => QT_CONTROL,
            "alt" => QT_ALT,
            "shift" => QT_SHIFT,
            "num" => QT_KEYPAD,
            _ => break,
        };
        flags |= flag;
        rest = tail;
    }
    let key = if let Some((qt, _, _)) = QT_KEYS
        .iter()
        .find(|(_, _, names)| names.iter().any(|name| name.eq_ignore_ascii_case(rest)))
    {
        *qt
    } else if let Some(number) = rest
        .strip_prefix(['F', 'f'])
        .and_then(|number| number.parse::<i32>().ok())
        .filter(|number| (1..=35).contains(number))
    {
        QT_KEY_F1 + number - 1
    } else {
        let mut characters = rest.chars();
        let character = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        i32::try_from(u32::from(character.to_uppercase().next()?)).ok()?
    };
    decode_qt_key(flags | key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    #[test]
    fn decodes_qt_key_combinations() {
        // Meta+T as kglobalaccel reported it in the lab.
        assert_eq!(decode_qt_key(268_435_540), Some(combo("super+t")));
        assert_eq!(
            decode_qt_key(QT_CONTROL | QT_ALT | 0x0100_0007),
            Some(combo("ctrl+alt+delete"))
        );
        assert_eq!(
            decode_qt_key(QT_META | QT_SHIFT | 0x0100_0009),
            Some(combo("super+shift+print"))
        );
        assert_eq!(decode_qt_key(0x0100_0071), Some(combo("xf86audiomute")));
        assert_eq!(decode_qt_key(QT_ALT | 0x0100_0031), Some(combo("alt+f2")));
        assert_eq!(
            decode_qt_key(QT_ALT | QT_KEY_BACKTAB),
            Some(combo("alt+shift+tab"))
        );
        assert_eq!(decode_qt_key(QT_META | 0x31), Some(combo("super+1")));
        assert_eq!(decode_qt_key(QT_GROUP_SWITCH | 0x41), None);
        assert_eq!(decode_qt_key(0), None);
    }

    #[test]
    fn encodes_queries_for_kglobalaccel() {
        assert_eq!(encode_qt_key(&combo("super+t")), Some(vec![268_435_540]));
        assert_eq!(
            encode_qt_key(&combo("ctrl+alt+page_up")),
            Some(vec![QT_CONTROL | QT_ALT | 0x0100_0016])
        );
        assert_eq!(
            encode_qt_key(&combo("super+return")),
            Some(vec![QT_META | 0x0100_0004])
        );
        assert_eq!(
            encode_qt_key(&combo("super+kp_enter")),
            Some(vec![
                QT_META | 0x0100_0005,
                QT_META | QT_KEYPAD | 0x0100_0005
            ])
        );
        assert_eq!(encode_qt_key(&combo("f12")), Some(vec![0x0100_003b]));
        assert_eq!(encode_qt_key(&combo("hyper+t")), None);
        assert_eq!(encode_qt_key(&combo("code:49")), None);
        for code in [
            QT_META | 0x54,
            QT_CONTROL | QT_SHIFT | 0x0100_0070,
            QT_ALT | 0x0100_0035,
        ] {
            let decoded = decode_qt_key(code).unwrap();
            assert!(
                encode_qt_key(&decoded).unwrap().contains(&code),
                "{code:#x}"
            );
        }
    }

    #[test]
    fn parses_kglobalshortcutsrc_entries() {
        let parsed = parse_config(
            "[org.kde.krunner.desktop]\n_k_friendly_name=KRunner\n_launch=Alt+F2\\tSearch,Alt+F2\\tSearch,KRunner\n\
             [kwin]\nWindow Close=Alt+F4,Alt+F4,Close Window\nDisabled=none,none,Disabled\n\
             Volume=Volume Up,none,Raise\nComma=Meta+\\,,none,Comma thing\nWeird=Meta+Frobnicate,none,Weird\n",
        );
        let found = parsed
            .bindings
            .iter()
            .map(|binding| (binding.combo.clone(), binding.action.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            vec![
                (combo("alt+f2"), "_launch"),
                (combo("xf86search"), "_launch"),
                (combo("alt+f4"), "Window Close"),
                (combo("xf86audioraisevolume"), "Volume"),
                (combo("super+,"), "Comma"),
            ]
        );
        assert_eq!(parsed.bindings[0].group, "org.kde.krunner.desktop");
        assert_eq!(parsed.bindings[0].description.as_deref(), Some("KRunner"));
        assert_eq!(
            parsed.unparsed,
            vec!["not compared: [kwin] Weird=Meta+Frobnicate"]
        );
    }

    #[test]
    fn decodes_busctl_shortcut_info_replies() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"type":"a(ssssssaiai)","data":[[["launch","Launch thing","whykeylab","Whykey Lab","default","Default Context",[268435540],[]]]]}"#,
        )
        .unwrap();
        let infos = shortcut_infos(&reply);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].component, "whykeylab");
        assert_eq!(infos[0].action_name, "Launch thing");
        assert_eq!(infos[0].keys, vec![268_435_540]);

        let info = infos[0].clone();
        let active = inspect_runtime(
            &[RuntimeMatch {
                info: info.clone(),
                active: true,
            }],
            None,
            false,
        );
        assert_eq!(active.outcome, Outcome::Consumed);
        let inactive = inspect_runtime(
            &[RuntimeMatch {
                info,
                active: false,
            }],
            None,
            false,
        );
        assert_eq!(inactive.outcome, Outcome::Pass);
        assert!(
            inactive
                .details
                .iter()
                .any(|line| line.contains("inactive"))
        );
        assert_eq!(inspect_runtime(&[], None, false).outcome, Outcome::Pass);
        assert_eq!(
            inspect_runtime(&[], None, false).tier,
            Some(EvidenceTier::LiveQuery)
        );
        assert_eq!(inactive.tier, Some(EvidenceTier::LiveQuery));
    }

    #[test]
    fn kglobalacceld_owning_the_name_on_wayland_is_not_a_key_reader() {
        // KWin reads Wayland keys only through its own registry, which exists
        // when KWin won org.kde.kglobalaccel. A kglobalacceld that owns the
        // name on Wayland does not see keys, so an active shortcut there is
        // not consumed.
        assert!(owner_misses_wayland_keys(Some("wayland"), "kglobalacceld"));
        assert!(!owner_misses_wayland_keys(Some("wayland"), "kwin_wayland"));
        assert!(!owner_misses_wayland_keys(Some("x11"), "kglobalacceld"));
        assert!(!owner_misses_wayland_keys(None, "kglobalacceld"));

        let info = ShortcutInfo {
            action: "Window Close".into(),
            action_name: "Close Window".into(),
            component: "kwin".into(),
            component_name: "KWin".into(),
            keys: vec![QT_ALT | 0x0100_0034],
        };
        let active = [RuntimeMatch { info, active: true }];
        let owned = inspect_runtime(&active, Some("kglobalacceld"), true);
        assert_eq!(owned.outcome, Outcome::Pass);
        assert_eq!(owned.tier, Some(EvidenceTier::LiveQuery));
        assert!(
            owned
                .details
                .iter()
                .any(|line| line.contains("owned by kglobalacceld, not kwin_wayland"))
        );
        assert_eq!(busctl_pid(br#"{"type":"u","data":[1059]}"#), Some(1059));
        assert_eq!(busctl_pid(br#"{"type":"u","data":1059}"#), None);
        // The same shortcut owned by KWin still consumes the key.
        assert_eq!(
            inspect_runtime(&active, None, false).outcome,
            Outcome::Consumed
        );
    }

    #[test]
    fn wayland_shortcuts_note_that_an_inhibitor_suspends_them() {
        // KWin skips global shortcuts while the focused surface holds a
        // keyboard-shortcuts inhibitor or an Xwayland keyboard grab, except
        // for the power key; on X11 there is no inhibitor.
        assert!(inhibitable(Some("wayland"), &combo("alt+f4")));
        assert!(!inhibitable(Some("wayland"), &combo("xf86poweroff")));
        assert!(!inhibitable(Some("x11"), &combo("alt+f4")));
        assert!(!inhibitable(None, &combo("alt+f4")));

        let info = ShortcutInfo {
            action: "Window Close".into(),
            action_name: "Close Window".into(),
            component: "kwin".into(),
            component_name: "KWin".into(),
            keys: vec![QT_ALT | 0x0100_0034],
        };
        let mentions_inhibitor = |result: &LayerResult| {
            result
                .details
                .iter()
                .any(|line| line.contains("keyboard-shortcuts inhibitor"))
        };
        let active = [RuntimeMatch {
            info: info.clone(),
            active: true,
        }];
        // The key is still consumed: whykey cannot see the inhibitor.
        let wayland = inspect_runtime(&active, None, true);
        assert_eq!(wayland.outcome, Outcome::Consumed);
        assert!(mentions_inhibitor(&wayland));
        assert!(!mentions_inhibitor(&inspect_runtime(&active, None, false)));
        // Nothing to suspend when no active shortcut consumes the key.
        let inactive = inspect_runtime(
            &[RuntimeMatch {
                info,
                active: false,
            }],
            None,
            true,
        );
        assert_eq!(inactive.outcome, Outcome::Pass);
        assert!(!mentions_inhibitor(&inactive));
        assert!(!mentions_inhibitor(&inspect_runtime(
            &active,
            Some("kglobalacceld"),
            true
        )));
    }

    #[test]
    fn maps_launch_media_keypad_and_shifted_digit_keys() {
        assert_eq!(decode_qt_key(0x0100_00a2), Some(combo("xf86launch0")));
        assert_eq!(decode_qt_key(0x0100_00ab), Some(combo("xf86launch9")));
        assert_eq!(parse_portable("Launch (5)"), Some(combo("xf86launch5")));
        assert_eq!(
            encode_qt_key(&combo("super+xf86launch1")),
            Some(vec![QT_META | 0x0100_00a3])
        );
        // Keypad keys carry KeypadModifier (`Num+` in portable text).
        assert_eq!(parse_portable("Meta+Num+1"), Some(combo("super+kp_1")));
        assert_eq!(parse_portable("Num+Home"), Some(combo("kp_home")));
        assert_eq!(parse_portable("Num+-"), Some(combo("kp_subtract")));
        assert_eq!(
            encode_qt_key(&combo("super+kp_1")),
            Some(vec![QT_META | QT_KEYPAD | 0x31])
        );
        assert_eq!(parse_portable("Meta+1"), Some(combo("super+1")));
        // Shift+digit is recorded as the shifted symbol (US layout).
        assert_eq!(
            encode_qt_key(&combo("super+shift+1")),
            Some(vec![QT_META | QT_SHIFT | 0x31, QT_META | 0x21])
        );
        assert_eq!(
            us_shifted_alias(&combo("super+shift+1")),
            parse_portable("Meta+!")
        );
        assert_eq!(us_shifted_alias(&combo("super+1")), None);
        let parsed = parse_config("[mediacontrol]\nplaypause=Toggle Media Play/Pause,none,Play\n");
        assert!(parsed.bindings.is_empty());
        assert!(parsed.unparsed[0].contains("no XKB keysym"));
    }

    #[test]
    fn kwinrc_modifier_only_shortcut_names_the_dbus_call() {
        let content = "[Windows]\nMeta=nope\n[ModifierOnlyShortcuts]\nMeta=org.kde.plasmashell,/PlasmaShell,org.kde.PlasmaShell,activateLauncherMenu\nAlt=\n";
        let path = PathBuf::from("/home/u/.config/kwinrc");
        let key = KeyCombo::parse_alone("super").unwrap();
        let result = inspect_alone(&key, Ok(Some((path.clone(), content.into()))));
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::ConfigFile));
        let evidence = result.binding.unwrap();
        assert_eq!(
            evidence.action.as_deref(),
            Some("org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.activateLauncherMenu")
        );
        // An empty value and a missing entry both leave the modifier free.
        for name in ["alt", "ctrl"] {
            let key = KeyCombo::parse_alone(name).unwrap();
            let result = inspect_alone(&key, Ok(Some((path.clone(), content.into()))));
            assert_eq!(result.outcome, Outcome::Pass, "{name}");
        }
    }

    #[test]
    fn a_missing_kwinrc_has_no_modifier_only_shortcuts() {
        let key = KeyCombo::parse_alone("super").unwrap();
        let result = inspect_alone(&key, Ok(None));
        assert_eq!(result.outcome, Outcome::Pass);
    }
}
