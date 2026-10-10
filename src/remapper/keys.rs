//! Physical key names used by Linux remappers, in the evdev namespace.
//!
//! Remappers rewrite kernel key codes before XKB turns them into keysyms,
//! while a query names a keysym. Only keys whose keysym does not depend on
//! the XKB layout map to a key code without further evidence; printable
//! keys map through US positions only when the layout is known to be US.

use crate::key::KeyCombo;

pub(crate) const SHIFT: u32 = 1;
pub(crate) const CTRL: u32 = 4;
pub(crate) const ALT: u32 = 8;
pub(crate) const SUPER: u32 = 64;
/// ISO_Level3_Shift (Mod5), held through the right Alt key.
pub(crate) const ALTGR: u32 = 128;
const MODIFIER_BITS: u32 = SHIFT | CTRL | ALT | SUPER | ALTGR;

/// One key press as a remapper sees it: held modifiers plus an evdev code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Press {
    pub(crate) mods: u32,
    pub(crate) code: u16,
}

impl Press {
    pub(crate) fn new(mods: u32, code: u16) -> Self {
        Self { mods, code }
    }
}

/// Linux `input-event-codes.h` names (without `KEY_`) for the keys
/// remapper configurations commonly name.
const EVDEV: &[(u16, &str)] = &[
    (1, "ESC"),
    (2, "1"),
    (3, "2"),
    (4, "3"),
    (5, "4"),
    (6, "5"),
    (7, "6"),
    (8, "7"),
    (9, "8"),
    (10, "9"),
    (11, "0"),
    (12, "MINUS"),
    (13, "EQUAL"),
    (14, "BACKSPACE"),
    (15, "TAB"),
    (16, "Q"),
    (17, "W"),
    (18, "E"),
    (19, "R"),
    (20, "T"),
    (21, "Y"),
    (22, "U"),
    (23, "I"),
    (24, "O"),
    (25, "P"),
    (26, "LEFTBRACE"),
    (27, "RIGHTBRACE"),
    (28, "ENTER"),
    (29, "LEFTCTRL"),
    (30, "A"),
    (31, "S"),
    (32, "D"),
    (33, "F"),
    (34, "G"),
    (35, "H"),
    (36, "J"),
    (37, "K"),
    (38, "L"),
    (39, "SEMICOLON"),
    (40, "APOSTROPHE"),
    (41, "GRAVE"),
    (42, "LEFTSHIFT"),
    (43, "BACKSLASH"),
    (44, "Z"),
    (45, "X"),
    (46, "C"),
    (47, "V"),
    (48, "B"),
    (49, "N"),
    (50, "M"),
    (51, "COMMA"),
    (52, "DOT"),
    (53, "SLASH"),
    (54, "RIGHTSHIFT"),
    (55, "KPASTERISK"),
    (56, "LEFTALT"),
    (57, "SPACE"),
    (58, "CAPSLOCK"),
    (59, "F1"),
    (60, "F2"),
    (61, "F3"),
    (62, "F4"),
    (63, "F5"),
    (64, "F6"),
    (65, "F7"),
    (66, "F8"),
    (67, "F9"),
    (68, "F10"),
    (69, "NUMLOCK"),
    (70, "SCROLLLOCK"),
    (71, "KP7"),
    (72, "KP8"),
    (73, "KP9"),
    (74, "KPMINUS"),
    (75, "KP4"),
    (76, "KP5"),
    (77, "KP6"),
    (78, "KPPLUS"),
    (79, "KP1"),
    (80, "KP2"),
    (81, "KP3"),
    (82, "KP0"),
    (83, "KPDOT"),
    (86, "102ND"),
    (87, "F11"),
    (88, "F12"),
    (96, "KPENTER"),
    (97, "RIGHTCTRL"),
    (98, "KPSLASH"),
    (99, "SYSRQ"),
    (100, "RIGHTALT"),
    (102, "HOME"),
    (103, "UP"),
    (104, "PAGEUP"),
    (105, "LEFT"),
    (106, "RIGHT"),
    (107, "END"),
    (108, "DOWN"),
    (109, "PAGEDOWN"),
    (110, "INSERT"),
    (111, "DELETE"),
    (113, "MUTE"),
    (114, "VOLUMEDOWN"),
    (115, "VOLUMEUP"),
    (116, "POWER"),
    (117, "KPEQUAL"),
    (119, "PAUSE"),
    (121, "KPCOMMA"),
    (125, "LEFTMETA"),
    (126, "RIGHTMETA"),
    (127, "COMPOSE"),
    (139, "MENU"),
    (142, "SLEEP"),
    (158, "BACK"),
    (159, "FORWARD"),
    (163, "NEXTSONG"),
    (164, "PLAYPAUSE"),
    (165, "PREVIOUSSONG"),
    (183, "F13"),
    (184, "F14"),
    (185, "F15"),
    (186, "F16"),
    (187, "F17"),
    (188, "F18"),
    (189, "F19"),
    (190, "F20"),
    (191, "F21"),
    (192, "F22"),
    (193, "F23"),
    (194, "F24"),
    (224, "BRIGHTNESSDOWN"),
    (225, "BRIGHTNESSUP"),
];

/// Keysyms every XKB layout produces from the same key, with the primary
/// key code and keypad codes that produce the same Whykey key name.
const LAYOUT_INVARIANT: &[(&str, u16, &[u16])] = &[
    ("ESCAPE", 1, &[]),
    ("BACKSPACE", 14, &[]),
    ("TAB", 15, &[]),
    ("RETURN", 28, &[96]),
    ("SPACE", 57, &[]),
    ("CAPS_LOCK", 58, &[]),
    ("F1", 59, &[]),
    ("F2", 60, &[]),
    ("F3", 61, &[]),
    ("F4", 62, &[]),
    ("F5", 63, &[]),
    ("F6", 64, &[]),
    ("F7", 65, &[]),
    ("F8", 66, &[]),
    ("F9", 67, &[]),
    ("F10", 68, &[]),
    ("F11", 87, &[]),
    ("F12", 88, &[]),
    ("F13", 183, &[]),
    ("F14", 184, &[]),
    ("F15", 185, &[]),
    ("F16", 186, &[]),
    ("F17", 187, &[]),
    ("F18", 188, &[]),
    ("F19", 189, &[]),
    ("F20", 190, &[]),
    ("F21", 191, &[]),
    ("F22", 192, &[]),
    ("F23", 193, &[]),
    ("F24", 194, &[]),
    ("NUM_LOCK", 69, &[]),
    ("SCROLL_LOCK", 70, &[]),
    ("PRINT", 99, &[]),
    ("PAUSE", 119, &[]),
    ("HOME", 102, &[71]),
    ("UP", 103, &[72]),
    ("PAGE_UP", 104, &[73]),
    ("LEFT", 105, &[75]),
    ("RIGHT", 106, &[77]),
    ("END", 107, &[79]),
    ("DOWN", 108, &[80]),
    ("PAGE_DOWN", 109, &[81]),
    ("INSERT", 110, &[82]),
    ("DELETE", 111, &[83]),
    ("MENU", 127, &[]),
    ("CONTROL_L", 29, &[]),
    ("CONTROL_R", 97, &[]),
    ("SHIFT_L", 42, &[]),
    ("SHIFT_R", 54, &[]),
    ("ALT_L", 56, &[]),
    ("SUPER_L", 125, &[]),
    ("SUPER_R", 126, &[]),
    ("XF86AUDIOMUTE", 113, &[]),
    ("XF86AUDIOLOWERVOLUME", 114, &[]),
    ("XF86AUDIORAISEVOLUME", 115, &[]),
    ("XF86AUDIOPLAY", 164, &[]),
    ("XF86AUDIONEXT", 163, &[]),
    ("XF86AUDIOPREV", 165, &[]),
    ("XF86MONBRIGHTNESSDOWN", 224, &[]),
    ("XF86MONBRIGHTNESSUP", 225, &[]),
];

/// US-layout characters for printable keys: (code, unshifted, shifted).
const US_PRINTABLE: &[(u16, char, char)] = &[
    (2, '1', '!'),
    (3, '2', '@'),
    (4, '3', '#'),
    (5, '4', '$'),
    (6, '5', '%'),
    (7, '6', '^'),
    (8, '7', '&'),
    (9, '8', '*'),
    (10, '9', '('),
    (11, '0', ')'),
    (12, '-', '_'),
    (13, '=', '+'),
    (16, 'q', 'Q'),
    (17, 'w', 'W'),
    (18, 'e', 'E'),
    (19, 'r', 'R'),
    (20, 't', 'T'),
    (21, 'y', 'Y'),
    (22, 'u', 'U'),
    (23, 'i', 'I'),
    (24, 'o', 'O'),
    (25, 'p', 'P'),
    (26, '[', '{'),
    (27, ']', '}'),
    (30, 'a', 'A'),
    (31, 's', 'S'),
    (32, 'd', 'D'),
    (33, 'f', 'F'),
    (34, 'g', 'G'),
    (35, 'h', 'H'),
    (36, 'j', 'J'),
    (37, 'k', 'K'),
    (38, 'l', 'L'),
    (39, ';', ':'),
    (40, '\'', '"'),
    (41, '`', '~'),
    (43, '\\', '|'),
    (44, 'z', 'Z'),
    (45, 'x', 'X'),
    (46, 'c', 'C'),
    (47, 'v', 'V'),
    (48, 'b', 'B'),
    (49, 'n', 'N'),
    (50, 'm', 'M'),
    (51, ',', '<'),
    (52, '.', '>'),
    (53, '/', '?'),
];

/// Keypad keys that produce a digit when NumLock is on.
const KEYPAD_DIGITS: &[(char, u16)] = &[
    ('0', 82),
    ('1', 79),
    ('2', 80),
    ('3', 81),
    ('4', 75),
    ('5', 76),
    ('6', 77),
    ('7', 71),
    ('8', 72),
    ('9', 73),
];

/// Look up a Linux key name, case-insensitively, with or without `KEY_`.
pub(crate) fn evdev_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let bare = upper.strip_prefix("KEY_").unwrap_or(&upper);
    EVDEV
        .iter()
        .find(|(_, candidate)| *candidate == bare)
        .map(|(code, _)| *code)
}

/// `KEY_<NAME>` for a known code, otherwise `code N`.
pub(crate) fn evdev_name(code: u16) -> String {
    EVDEV
        .iter()
        .find(|(candidate, _)| *candidate == code)
        .map_or_else(|| format!("code {code}"), |(_, name)| format!("KEY_{name}"))
}

/// The key code of an X keysym name that every XKB layout places on the
/// same key, e.g. `Escape` or `Control_L`.
pub(crate) fn keysym_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let upper = match upper.as_str() {
        "PRIOR" => "PAGE_UP",
        "NEXT" => "PAGE_DOWN",
        other => other,
    };
    LAYOUT_INVARIANT
        .iter()
        .find(|(candidate, ..)| *candidate == upper)
        .map(|(_, code, _)| *code)
}

/// The modifier bit a held modifier key contributes.
pub(crate) fn modifier_bit(code: u16) -> Option<u32> {
    match code {
        29 | 97 => Some(CTRL),
        42 | 54 => Some(SHIFT),
        56 => Some(ALT),
        100 => Some(ALTGR),
        125 | 126 => Some(SUPER),
        _ => None,
    }
}

/// The keys that hold a modifier bit.
pub(crate) fn modifier_codes(bit: u32) -> &'static [u16] {
    match bit {
        CTRL => &[29, 97],
        SHIFT => &[42, 54],
        ALT => &[56],
        ALTGR => &[100],
        SUPER => &[125, 126],
        _ => &[],
    }
}

/// Every single modifier bit in a mask, lowest first.
pub(crate) fn modifier_bits(mask: u32) -> impl Iterator<Item = u32> {
    [SHIFT, CTRL, ALT, SUPER, ALTGR]
        .into_iter()
        .filter(move |bit| mask & bit != 0)
}

/// Whether the XKB layout is known to be plain US, so printable keys sit
/// at their US positions.
pub(crate) fn layout_is_us() -> bool {
    crate::xkb::Rmlvo::from_environment().is_some_and(|rmlvo| {
        rmlvo.layout.trim().eq_ignore_ascii_case("us")
            && rmlvo
                .variant
                .as_deref()
                .is_none_or(|variant| variant.trim().is_empty())
    })
}

/// The physical presses that can produce the queried key, and whether the
/// mapping relies on the US layout for a printable key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryPresses {
    pub(crate) presses: Vec<Press>,
    pub(crate) layout_assumed: bool,
}

/// Map a query to physical presses. A captured keycode is exact; a
/// layout-independent keysym maps to its keys; a printable keysym maps to
/// its US position, flagged unless the layout is known to be US.
pub(crate) fn query_presses(
    key: &KeyCombo,
    keycode: Option<u16>,
    layout_us: bool,
) -> Option<QueryPresses> {
    let mods = key.modmask() & MODIFIER_BITS;
    if let Some(code) = keycode {
        return Some(QueryPresses {
            presses: vec![Press::new(mods, code)],
            layout_assumed: false,
        });
    }
    let name = key.key();
    if let Some((_, primary, alternates)) = LAYOUT_INVARIANT
        .iter()
        .find(|(candidate, ..)| *candidate == name)
    {
        let presses = std::iter::once(*primary)
            .chain(alternates.iter().copied())
            .map(|code| Press::new(mods, code))
            .collect();
        return Some(QueryPresses {
            presses,
            layout_assumed: false,
        });
    }
    let mut characters = name.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    let lowered = character.to_ascii_lowercase();
    let (code, shifted) = US_PRINTABLE.iter().find_map(|(code, plain, shifted)| {
        if *plain == lowered {
            Some((*code, false))
        } else if *shifted == character && !shifted.is_ascii_alphabetic() {
            Some((*code, true))
        } else {
            None
        }
    })?;
    let mods = if shifted { mods | SHIFT } else { mods };
    let mut presses = vec![Press::new(mods, code)];
    if let Some((_, keypad)) = KEYPAD_DIGITS.iter().find(|(digit, _)| *digit == character) {
        presses.push(Press::new(mods, *keypad));
    }
    Some(QueryPresses {
        presses,
        layout_assumed: !layout_us,
    })
}

/// The key combination downstream layers receive for an emitted press, when
/// its keysym does not depend on an unknown layout.
pub(crate) fn downstream_combo(press: Press, layout_us: bool) -> Option<KeyCombo> {
    if let Some((name, ..)) = LAYOUT_INVARIANT
        .iter()
        .find(|(_, primary, _)| *primary == press.code)
    {
        return Some(KeyCombo::from_parts(press.mods, name));
    }
    if !layout_us {
        return None;
    }
    if press.code == 100 {
        return Some(KeyCombo::from_parts(press.mods, "ALT_R"));
    }
    US_PRINTABLE
        .iter()
        .find(|(code, ..)| *code == press.code)
        .map(|(_, plain, _)| KeyCombo::from_parts(press.mods, plain.to_string()))
}

/// Human-readable press for evidence lines, e.g. `Ctrl+KEY_A`.
pub(crate) fn describe(press: Press) -> String {
    let mut text = String::new();
    for (bit, name) in [
        (CTRL, "Ctrl"),
        (ALT, "Alt"),
        (SHIFT, "Shift"),
        (SUPER, "Super"),
        (ALTGR, "AltGr"),
    ] {
        if press.mods & bit != 0 {
            text.push_str(name);
            text.push('+');
        }
    }
    text.push_str(&evdev_name(press.code));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_bits_match_query_modmask() {
        for (name, bit) in [
            ("shift", SHIFT),
            ("ctrl", CTRL),
            ("alt", ALT),
            ("super", SUPER),
            ("altgr", ALTGR),
        ] {
            assert_eq!(crate::key::modifier_mask(name), Some(bit));
        }
    }

    #[test]
    fn printable_keys_are_flagged_without_a_known_layout() {
        let key = KeyCombo::parse_query("ctrl+a").unwrap();
        let unknown = query_presses(&key, None, false).unwrap();
        assert_eq!(unknown.presses, vec![Press::new(CTRL, 30)]);
        assert!(unknown.layout_assumed);
        assert!(!query_presses(&key, None, true).unwrap().layout_assumed);
        let escape = KeyCombo::parse_query("escape").unwrap();
        assert!(!query_presses(&escape, None, false).unwrap().layout_assumed);
        assert_eq!(downstream_combo(Press::new(0, 30), false), None);
        assert_eq!(
            downstream_combo(Press::new(CTRL, 48), true),
            Some(KeyCombo::parse_query("ctrl+b").unwrap())
        );
    }
}
