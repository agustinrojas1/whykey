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

