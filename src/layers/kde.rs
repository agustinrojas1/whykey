//! KDE Plasma global shortcuts.
//!
//! The running kglobalaccel daemon is asked over D-Bus (`busctl --user`,
//! never auto-starting it): `globalShortcutsByKey` finds the shortcuts that
//! use a key and each component's `isActive` says whether kglobalaccel still
//! grabs them. Without the daemon, `kglobalshortcutsrc`, the file it
//! persists to, is a configured-only hint.

use std::env;
use std::fs;
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
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
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
    let content = fs::read_to_string(&path)
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
            Ok(matches) => inspect_runtime(&matches),
            Err(error) => inspect_file(key, &error),
        }
    }
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

fn inspect_runtime(matches: &[RuntimeMatch]) -> LayerResult {
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
    let content = match fs::read_to_string(&path) {
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
    let matches = parsed
        .bindings
        .iter()
        .filter(|binding| binding.combo == *key)
        .collect::<Vec<_>>();
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
    let name = if key == QT_KEY_BACKTAB {
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
    Some(KeyCombo::from_parts(u32::try_from(mask).ok()?, name))
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
    let key = combo.key();
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
    if let Some(path) = env::var_os("KGLOBALSHORTCUTS_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let path = base.join("kglobalshortcutsrc");
    path.is_file().then_some(path)
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
                None => parsed
                    .unparsed
                    .push(format!("not compared: [{group}] {action}={chord}")),
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
        let active = inspect_runtime(&[RuntimeMatch {
            info: info.clone(),
            active: true,
        }]);
        assert_eq!(active.outcome, Outcome::Consumed);
        let inactive = inspect_runtime(&[RuntimeMatch {
            info,
            active: false,
        }]);
        assert_eq!(inactive.outcome, Outcome::Pass);
        assert!(
            inactive
                .details
                .iter()
                .any(|line| line.contains("inactive"))
        );
        assert_eq!(inspect_runtime(&[]).outcome, Outcome::Pass);
        assert_eq!(inspect_runtime(&[]).tier, Some(EvidenceTier::LiveQuery));
        assert_eq!(inactive.tier, Some(EvidenceTier::LiveQuery));
    }
}
