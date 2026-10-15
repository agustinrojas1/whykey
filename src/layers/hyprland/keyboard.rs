use super::*;

pub(super) fn summarize_keyboards(devices_json: &str) -> Option<String> {
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

pub(super) fn xkb_keycodes_for_key(
    key: &KeyCombo,
    devices_json: &str,
) -> Vec<crate::xkb::XkbKeycode> {
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

pub(super) fn xkb_symbols_for_physical_key(
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

/// Name prefix Hyprland gives virtual keyboards (`zwp_virtual_keyboard_v1`
/// clients such as wtype): `hl-virtual-keyboard[-<process>]`
/// (v0.56.2 src/protocols/VirtualKeyboard.cpp).
pub(crate) const VIRTUAL_KEYBOARD_PREFIX: &str = "hl-virtual-keyboard";

/// What `hyprctl devices -j` says about the keyboards a key event may come
/// from: the active keymap name of the physical keyboard whose layout
/// whykey decodes with, and the keyboards currently attached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KeyboardInventory {
    pub(crate) reference_keymap: Option<String>,
    /// `(name, active keymap, main)` per keyboard.
    pub(crate) keyboards: Vec<(String, String, bool)>,
}

pub(crate) fn keyboard_inventory(devices_json: &str) -> KeyboardInventory {
    let Ok(devices) = serde_json::from_str::<serde_json::Value>(devices_json) else {
        return KeyboardInventory::default();
    };
    let keyboards = devices
        .get("keyboards")
        .and_then(serde_json::Value::as_array)
        .map(|keyboards| {
            keyboards
                .iter()
                .filter_map(|keyboard| {
                    let text = |field: &str| keyboard.get(field)?.as_str().map(str::to_owned);
                    Some((
                        text("name")?,
                        text("active_keymap").unwrap_or_default(),
                        keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let physical = |main: bool| {
        keyboards
            .iter()
            .find(|(name, _, is_main)| {
                *is_main == main && !name.starts_with(VIRTUAL_KEYBOARD_PREFIX)
            })
            .map(|(_, keymap, _)| keymap.clone())
    };
    KeyboardInventory {
        reference_keymap: physical(true).or_else(|| physical(false)),
        keyboards,
    }
}

/// The name of layout `group` (0-based) in a compiled keymap's
/// `xkb_symbols` (`name[1]="English (US)";`, or `name[Group1]=` from older
/// libxkbcommon): the name Hyprland reports as a keyboard's active keymap.
pub(crate) fn keymap_layout_name(keymap: &str, group: usize) -> Option<String> {
    let symbols = &keymap[keymap.find("xkb_symbols")?..];
    let wanted = group + 1;
    symbols.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("name[")?;
        let (index, value) = rest.split_once(']')?;
        let index = index.strip_prefix("Group").unwrap_or(index);
        if index.parse::<usize>().ok()? != wanted {
            return None;
        }
        let value = value.trim().strip_prefix('=')?.trim();
        let value = value.strip_suffix(';').unwrap_or(value).trim();
        Some(value.strip_prefix('"')?.strip_suffix('"')?.to_owned())
    })
}

/// Whether Hyprland's main keyboard uses the plain US layout (no variant),
/// so printable keys sit at their US positions.
pub(crate) fn main_keyboard_is_plain_us(devices_json: &str) -> bool {
    main_keyboard_rmlvo(devices_json).is_some_and(|rmlvo| {
        rmlvo.layout.trim().eq_ignore_ascii_case("us")
            && rmlvo
                .variant
                .as_deref()
                .is_none_or(|variant| variant.trim().is_empty())
    })
}

pub(crate) fn compile_main_xkb_keymap(devices_json: &str) -> Option<String> {
    compile_keymap_cached(main_keyboard_rmlvo(devices_json)?)
}

/// The RMLVO of Hyprland's main keyboard (or its first keyboard).
fn main_keyboard_rmlvo(devices_json: &str) -> Option<xkb::Rmlvo> {
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
    Some(rmlvo)
}

/// One inspection resolves the query key and the captured physical key
/// against the same keymap; compile it once per RMLVO instead of spawning
/// `xkbcli` for each lookup. A changed layout simply recompiles.
fn compile_keymap_cached(rmlvo: xkb::Rmlvo) -> Option<String> {
    static LAST: Mutex<Option<(xkb::Rmlvo, Option<String>)>> = Mutex::new(None);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached, keymap)) = last.as_ref() {
        if *cached == rmlvo {
            return keymap.clone();
        }
    }
    let keymap = xkb::compile_keymap(&rmlvo);
    *last = Some((rmlvo, keymap.clone()));
    keymap
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
        "KP_ENTER" => "KP_Enter",
        "KP_HOME" => "KP_Home",
        "KP_END" => "KP_End",
        "KP_PAGE_UP" => "KP_Prior",
        "KP_PAGE_DOWN" => "KP_Next",
        "KP_UP" => "KP_Up",
        "KP_DOWN" => "KP_Down",
        "KP_LEFT" => "KP_Left",
        "KP_RIGHT" => "KP_Right",
        "KP_BEGIN" => "KP_Begin",
        "KP_INSERT" => "KP_Insert",
        "KP_DELETE" => "KP_Delete",
        "KP_ADD" => "KP_Add",
        "KP_SUBTRACT" => "KP_Subtract",
        "KP_MULTIPLY" => "KP_Multiply",
        "KP_DIVIDE" => "KP_Divide",
        "KP_EQUAL" => "KP_Equal",
        "KP_DECIMAL" => "KP_Decimal",
        "KP_SEPARATOR" => "KP_Separator",
        "KP_0" => "KP_0",
        "KP_1" => "KP_1",
        "KP_2" => "KP_2",
        "KP_3" => "KP_3",
        "KP_4" => "KP_4",
        "KP_5" => "KP_5",
        "KP_6" => "KP_6",
        "KP_7" => "KP_7",
        "KP_8" => "KP_8",
        "KP_9" => "KP_9",
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
pub(super) fn parse_xkb_symbol_keycodes(keymap: &str) -> HashMap<String, Vec<u32>> {
    xkb::parse_symbol_keycodes(keymap)
}

pub(super) fn parse_xkb_symbol_keycodes_for_group(
    keymap: &str,
    group_index: usize,
) -> HashMap<String, Vec<u32>> {
    xkb::parse_symbol_keycodes_for_group(keymap, group_index)
}

#[cfg(test)]
pub(super) fn parse_xkb_symbol_key_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("key <")?;
    let (name, _) = rest.split_once('>')?;
    Some(name.to_owned())
}

#[cfg(test)]
pub(super) fn parse_xkb_symbol_fragment(line: &str) -> Option<Vec<String>> {
    let symbols = line.rsplit_once('[')?.1.split_once(']')?.0;
    let symbols = symbols
        .split(',')
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty() && *symbol != "NoSymbol")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!symbols.is_empty()).then_some(symbols)
}
