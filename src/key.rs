use std::fmt;
use std::str::FromStr;

use serde::Serialize;

const SHIFT_MASK: u32 = 1;
const CAPS_MASK: u32 = 2;
const CTRL_MASK: u32 = 4;
const ALT_MASK: u32 = 8;
const MOD2_MASK: u32 = 16;
const MOD3_MASK: u32 = 32;
const SUPER_MASK: u32 = 64;
const MOD5_MASK: u32 = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct KeyCombo {
    modifiers: u32,
    key: String,
}

impl KeyCombo {
    pub(crate) fn from_parts(modifiers: u32, key: impl AsRef<str>) -> Self {
        Self {
            modifiers,
            key: normalize_key_str(key.as_ref()),
        }
    }

    pub fn modmask(&self) -> u32 {
        self.modifiers
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// A compact form suitable for separating consecutive combinations.
    ///
    /// The regular `Display` implementation intentionally uses spaces around
    /// `+` for human-readable single-key reports. A sequence needs an
    /// unambiguous boundary between combinations, so it uses this compact
    /// representation instead.
    pub fn compact_display(&self) -> String {
        let modifiers = [
            (CTRL_MASK, "CTRL"),
            (ALT_MASK, "ALT"),
            (SHIFT_MASK, "SHIFT"),
            (SUPER_MASK, "SUPER"),
            (CAPS_MASK, "CAPS"),
            (MOD2_MASK, "MOD2"),
            (MOD3_MASK, "MOD3"),
            (MOD5_MASK, "MOD5"),
        ];

        let mut display = String::new();
        for (mask, name) in modifiers {
            if self.modifiers & mask != 0 {
                display.push_str(name);
                display.push('+');
            }
        }
        display.push_str(&self.key);
        display
    }
}

const MAX_SEQUENCE_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeySequence(Vec<KeyCombo>);

impl KeySequence {
    pub fn as_slice(&self) -> &[KeyCombo] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeySequenceError(String);

impl fmt::Display for ParseKeySequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ParseKeySequenceError {}

impl FromStr for KeySequence {
    type Err = ParseKeySequenceError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let tokens: Vec<_> = input.split_whitespace().collect();
        if tokens.is_empty() {
            return Err(ParseKeySequenceError("key sequence is empty".into()));
        }
        if tokens.len() > MAX_SEQUENCE_LENGTH {
            return Err(ParseKeySequenceError(format!(
                "key sequence has {} steps; the maximum is {MAX_SEQUENCE_LENGTH}",
                tokens.len()
            )));
        }

        let steps = tokens.len();
        tokens
            .into_iter()
            .enumerate()
            .map(|(index, token)| {
                KeyCombo::parse_query(token).map_err(|error| {
                    ParseKeySequenceError(if steps == 1 {
                        format!("invalid key combination '{}': {error}", abbreviate(token))
                    } else {
                        format!(
                            "invalid key combination at step {} ('{}'): {error}",
                            index + 1,
                            abbreviate(token)
                        )
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, combo) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" ")?;
            }
            formatter.write_str(&combo.compact_display())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyComboError(String);

impl fmt::Display for ParseKeyComboError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ParseKeyComboError {}

impl KeyCombo {
    /// Parse a combination typed by a user. Unlike [`FromStr`], which also
    /// reads keys from compositor and application configuration, this
    /// rejects key names Whykey does not recognize instead of reporting on a
    /// key that cannot exist.
    pub fn parse_query(input: &str) -> Result<Self, ParseKeyComboError> {
        parse_combo(input, true)
    }
}

impl FromStr for KeyCombo {
    type Err = ParseKeyComboError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        parse_combo(input, false)
    }
}

/// How many characters of user input an error message echoes.
const ECHO_LIMIT: usize = 64;
/// How many `+` segments an error describes before summarizing the rest.
const MAX_DESCRIBED_SEGMENTS: usize = 4;

/// Shorten user input echoed in an error: the first 64 characters, then
/// `…`. A 120 000-character argument must not produce a
/// 120 000-character message.
pub fn abbreviate(text: &str) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(ECHO_LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]).into(),
        None => text.into(),
    }
}

fn parse_combo(input: &str, validate_key: bool) -> Result<KeyCombo, ParseKeyComboError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseKeyComboError("key combination is empty".into()));
    }

    let mut modifiers = 0;
    let mut remainder = input;
    while let Some((raw_modifier, rest)) = remainder.split_once('+') {
        let part = raw_modifier.trim();
        if part.is_empty() {
            break;
        }
        if let Some(mask) = modifier_mask(part) {
            if modifiers & mask != 0 {
                return Err(ParseKeyComboError(format!(
                    "modifier '{}' appears more than once",
                    abbreviate(part)
                )));
            }
            modifiers |= mask;
            remainder = rest;
            continue;
        }
        break;
    }

    let remainder = remainder.trim();
    if remainder.is_empty() {
        return Err(ParseKeyComboError(
            "a combination must contain a non-modifier key".into(),
        ));
    }
    if modifier_mask(remainder).is_some() {
        return Err(ParseKeyComboError(
            "a combination must contain a non-modifier key".into(),
        ));
    }
    if remainder.contains('+') && remainder != "+" {
        let segments: Vec<&str> = remainder.split('+').map(str::trim).collect();
        let describe = |token: &str| {
            if token.is_empty() {
                "an empty segment".to_owned()
            } else {
                format!("'{}'", abbreviate(token))
            }
        };
        let found = if segments.len() <= MAX_DESCRIBED_SEGMENTS {
            segments
                .iter()
                .map(|token| describe(token))
                .collect::<Vec<_>>()
                .join(" and ")
        } else {
            let mut found = segments[..MAX_DESCRIBED_SEGMENTS - 1]
                .iter()
                .map(|token| describe(token))
                .collect::<Vec<_>>()
                .join(", ");
            let rest = &segments[MAX_DESCRIBED_SEGMENTS - 1..];
            let empty = rest.iter().filter(|token| token.is_empty()).count();
            found.push_str(&format!(
                " and {} more segments ({empty} empty)",
                rest.len()
            ));
            found
        };
        return Err(ParseKeyComboError(format!(
            "a combination must contain exactly one non-modifier key, but found {found} \
             (modifiers: ctrl, alt, shift, super, hyper, altgr, caps, num, mod2, mod3, mod5)"
        )));
    }
    let key = normalize_key(remainder)?;
    if validate_key && !is_known_key(&key) {
        return Err(unknown_key_error(remainder));
    }

    Ok(KeyCombo { modifiers, key })
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let modifiers = [
            (CTRL_MASK, "CTRL"),
            (ALT_MASK, "ALT"),
            (SHIFT_MASK, "SHIFT"),
            (SUPER_MASK, "SUPER"),
            (CAPS_MASK, "CAPS"),
            (MOD2_MASK, "MOD2"),
            (MOD3_MASK, "MOD3"),
            (MOD5_MASK, "MOD5"),
        ];

        let mut wrote_modifier = false;
        for (mask, name) in modifiers {
            if self.modifiers & mask != 0 {
                if wrote_modifier {
                    formatter.write_str(" + ")?;
                }
                formatter.write_str(name)?;
                wrote_modifier = true;
            }
        }

        if wrote_modifier {
            formatter.write_str(" + ")?;
        }
        formatter.write_str(&self.key)
    }
}

/// The modmask bit for one modifier name or alias, if it is a modifier.
pub(crate) fn modifier_mask(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "ctl" => Some(CTRL_MASK),
        "alt" | "mod1" => Some(ALT_MASK),
        "shift" => Some(SHIFT_MASK),
        "super" | "meta" | "win" | "mod4" => Some(SUPER_MASK),
        "caps" | "capslock" | "lock" => Some(CAPS_MASK),
        "mod2" | "num" | "numlock" => Some(MOD2_MASK),
        "mod3" | "hyper" => Some(MOD3_MASK),
        // AltGr is ISO_Level3_Shift, which XKB maps to Mod5 (Hyprland
        // modmask 128).
        "mod5" | "altgr" | "iso_level3_shift" => Some(MOD5_MASK),
        _ => None,
    }
}

pub(crate) fn normalize_key_str(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "arrowleft" | "arrow_left" | "leftarrow" | "left_arrow" => "LEFT".into(),
        "arrowright" | "arrow_right" | "rightarrow" | "right_arrow" => "RIGHT".into(),
        "arrowup" | "arrow_up" | "uparrow" | "up_arrow" => "UP".into(),
        "arrowdown" | "arrow_down" | "downarrow" | "down_arrow" => "DOWN".into(),
        "enter" | "return" => "RETURN".into(),
        // Keypad keys are distinct keysyms (KP_Enter 0xff8d is not Return
        // 0xff0d) with their own keycodes, so they keep their own names.
        "kp_enter" | "kp_return" => "KP_ENTER".into(),
        "kp_prior" | "kp_page_up" => "KP_PAGE_UP".into(),
        "kp_next" | "kp_page_down" => "KP_PAGE_DOWN".into(),
        "esc" | "escape" => "ESCAPE".into(),
        "spacebar" | "space" => "SPACE".into(),
        "pgup" | "pageup" | "page_up" | "prior" => "PAGE_UP".into(),
        "pgdown" | "pagedown" | "page_down" | "next" => "PAGE_DOWN".into(),
        "backspace" => "BACKSPACE".into(),
        "delete" | "del" => "DELETE".into(),
        "insert" | "ins" => "INSERT".into(),
        "home" => "HOME".into(),
        "end" => "END".into(),
        "tab" | "iso_left_tab" => "TAB".into(),
        "exclam" => "!".into(),
        "at" => "@".into(),
        "numbersign" => "#".into(),
        "dollar" => "$".into(),
        "percent" => "%".into(),
        "asciicircum" => "^".into(),
        "ampersand" => "&".into(),
        "asterisk" => "*".into(),
        "parenleft" => "(".into(),
        "parenright" => ")".into(),
        "minus" => "-".into(),
        "underscore" => "_".into(),
        "equal" => "=".into(),
        "plus" => "+".into(),
        "bracketleft" => "[".into(),
        "bracketright" => "]".into(),
        "braceleft" => "{".into(),
        "braceright" => "}".into(),
        "semicolon" => ";".into(),
        "colon" => ":".into(),
        "apostrophe" => "'".into(),
        "quotedbl" => "\"".into(),
        "grave" => "`".into(),
        "asciitilde" => "~".into(),
        "backslash" => "\\".into(),
        "bar" => "|".into(),
        "comma" => ",".into(),
        "less" => "<".into(),
        "period" => ".".into(),
        "greater" => ">".into(),
        "slash" => "/".into(),
        "question" => "?".into(),
        value => value.to_ascii_uppercase(),
    }
}

fn normalize_key(value: &str) -> Result<String, ParseKeyComboError> {
    if value.chars().any(char::is_whitespace) {
        return Err(ParseKeyComboError(format!(
            "key '{}' contains whitespace",
            abbreviate(value)
        )));
    }

    if value.to_ascii_lowercase().starts_with("code:") {
        let code = value.split_at(5).1;
        if code.is_empty() || !code.chars().all(|character| character.is_ascii_digit()) {
            return Err(ParseKeyComboError(format!(
                "invalid keycode '{}'",
                abbreviate(value)
            )));
        }
        return Ok(format!("CODE:{code}"));
    }

    Ok(normalize_key_str(value))
}

/// Named keys accepted in user queries, in their normalized spelling.
/// Aliases (for example `enter`, `pgup`, `comma`) are folded into these or
/// into single characters by [`normalize_key_str`] before the check.
const NAMED_KEYS: &[&str] = &[
    "LEFT",
    "RIGHT",
    "UP",
    "DOWN",
    "RETURN",
    "ESCAPE",
    "SPACE",
    "TAB",
    "BACKSPACE",
    "DELETE",
    "INSERT",
    "HOME",
    "END",
    "PAGE_UP",
    "PAGE_DOWN",
    "PRINT",
    "PAUSE",
    "BREAK",
    "SYS_REQ",
    "SCROLL_LOCK",
    "MENU",
    "CAPS_LOCK",
    "NUM_LOCK",
    "MULTI_KEY",
    "MODE_SWITCH",
    "HELP",
    "UNDO",
    "REDO",
    "FIND",
    "CANCEL",
    "SELECT",
    "EXECUTE",
    "CLEAR",
    "BEGIN",
    "LINEFEED",
    "SHIFT_L",
    "SHIFT_R",
    "CONTROL_L",
    "CONTROL_R",
    "ALT_L",
    "ALT_R",
    "SUPER_L",
    "SUPER_R",
    "META_L",
    "META_R",
    "HYPER_L",
    "HYPER_R",
    "KP_ENTER",
    "KP_HOME",
    "KP_END",
    "KP_0",
    "KP_1",
    "KP_2",
    "KP_3",
    "KP_4",
    "KP_5",
    "KP_6",
    "KP_7",
    "KP_8",
    "KP_9",
    "KP_ADD",
    "KP_SUBTRACT",
    "KP_MULTIPLY",
    "KP_DIVIDE",
    "KP_EQUAL",
    "KP_DECIMAL",
    "KP_SEPARATOR",
    "KP_DELETE",
    "KP_INSERT",
    "KP_UP",
    "KP_DOWN",
    "KP_LEFT",
    "KP_RIGHT",
    "KP_BEGIN",
    "KP_SPACE",
    "KP_TAB",
    "KP_PAGE_UP",
    "KP_PAGE_DOWN",
    "KP_F1",
    "KP_F2",
    "KP_F3",
    "KP_F4",
    "MOUSE_UP",
    "MOUSE_DOWN",
    "MOUSE_LEFT",
    "MOUSE_RIGHT",
];

/// Whether a normalized key names a real key: one character, a named key,
/// a function key F1–F35, an `XF86*` media key, a `CODE:N` keycode, or a
/// Hyprland `MOUSE:N` button.
fn is_known_key(key: &str) -> bool {
    let mut characters = key.chars();
    if characters.next().is_some() && characters.next().is_none() {
        return true;
    }
    if NAMED_KEYS.contains(&key) || key.starts_with("CODE:") {
        return true;
    }
    if let Some(number) = key.strip_prefix('F') {
        return !number.starts_with('0')
            && number
                .parse::<u8>()
                .is_ok_and(|number| (1..=35).contains(&number));
    }
    if let Some(button) = key.strip_prefix("MOUSE:") {
        return !button.is_empty() && button.chars().all(|character| character.is_ascii_digit());
    }
    key.strip_prefix("XF86").is_some_and(|name| {
        !name.is_empty()
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

/// Spellings offered as suggestions for a mistyped key, besides
/// [`NAMED_KEYS`] and function keys.
const SUGGESTED_ALIASES: &[&str] = &[
    "enter",
    "esc",
    "pgup",
    "pgdown",
    "comma",
    "period",
    "slash",
    "backslash",
    "semicolon",
    "apostrophe",
    "grave",
    "minus",
    "equal",
    "plus",
    "bracketleft",
    "bracketright",
];

fn unknown_key_error(raw: &str) -> ParseKeyComboError {
    let shown = abbreviate(raw);
    if raw.chars().nth(ECHO_LIMIT).is_some() {
        // No real key name is this long; a suggestion would only cost time.
        return ParseKeyComboError(format!("unknown key '{shown}'"));
    }
    let lowered = raw.to_lowercase();
    let function_keys = (1..=35).map(|number| format!("f{number}"));
    let suggestion = NAMED_KEYS
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .chain(function_keys)
        .chain(SUGGESTED_ALIASES.iter().map(|alias| (*alias).to_owned()))
        .map(|candidate| (edit_distance(&lowered, &candidate), candidate))
        .min_by_key(|(distance, _)| *distance)
        .filter(|(distance, _)| *distance <= (lowered.chars().count() / 2).max(1));
    let hint = "use a single character, a named key such as return, left, or f1, an XF86 media key, or code:N";
    match suggestion {
        Some((_, candidate)) => ParseKeyComboError(format!(
            "unknown key '{shown}'; did you mean '{candidate}'? ({hint})"
        )),
        None => ParseKeyComboError(format!("unknown key '{shown}'; {hint}")),
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_character) in left.chars().enumerate() {
        current[0] = row + 1;
        for (column, right_character) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_character != *right_character);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_a_combo() {
        let combo: KeyCombo = "shift+control+ArrowLeft".parse().unwrap();

        assert_eq!(combo.modmask(), SHIFT_MASK | CTRL_MASK);
        assert_eq!(combo.key(), "LEFT");
        assert_eq!(combo.to_string(), "CTRL + SHIFT + LEFT");
    }

    #[test]
    fn accepts_common_modifier_aliases() {
        let combo: KeyCombo = "win+c".parse().unwrap();

        assert_eq!(combo.modmask(), SUPER_MASK);
        assert_eq!(combo.to_string(), "SUPER + C");
    }

    #[test]
    fn accepts_terminal_key_name_aliases() {
        assert_eq!("ctrl+LeftArrow".parse::<KeyCombo>().unwrap().key(), "LEFT");
        assert_eq!("Return".parse::<KeyCombo>().unwrap().key(), "RETURN");
        assert_eq!("Page_Down".parse::<KeyCombo>().unwrap().key(), "PAGE_DOWN");
        assert_eq!("Escape".parse::<KeyCombo>().unwrap().key(), "ESCAPE");
    }

    #[test]
    fn keypad_keys_stay_distinct_from_main_keys() {
        for (keypad_name, main_name, normalized) in [
            ("KP_Enter", "Return", "KP_ENTER"),
            ("kp_return", "enter", "KP_ENTER"),
            ("KP_1", "1", "KP_1"),
            ("KP_Home", "Home", "KP_HOME"),
            ("KP_End", "End", "KP_END"),
            ("KP_Prior", "Prior", "KP_PAGE_UP"),
            ("kp_page_up", "page_up", "KP_PAGE_UP"),
            ("KP_Next", "Next", "KP_PAGE_DOWN"),
            ("KP_Add", "plus", "KP_ADD"),
            ("KP_Subtract", "minus", "KP_SUBTRACT"),
            ("KP_Multiply", "asterisk", "KP_MULTIPLY"),
            ("KP_Divide", "slash", "KP_DIVIDE"),
            ("KP_Equal", "equal", "KP_EQUAL"),
        ] {
            let keypad = KeyCombo::parse_query(&format!("super+{keypad_name}")).unwrap();
            let main = KeyCombo::parse_query(&format!("super+{main_name}")).unwrap();
            assert_eq!(keypad.key(), normalized, "{keypad_name}");
            assert_ne!(keypad, main, "{keypad_name} must not match {main_name}");
        }
    }

    #[test]
    fn rejects_two_non_modifier_keys() {
        let error = "ctrl+a+b".parse::<KeyCombo>().unwrap_err();

        assert!(
            error.to_string().starts_with(
                "a combination must contain exactly one non-modifier key, but found 'a' and 'b'"
            ),
            "{error}"
        );
    }

    #[test]
    fn rejects_modifier_without_key() {
        let error = "ctrl+shift".parse::<KeyCombo>().unwrap_err();

        assert_eq!(
            error.to_string(),
            "a combination must contain a non-modifier key"
        );
    }

    #[test]
    fn rejects_empty_segments() {
        let error = "ctrl++left".parse::<KeyCombo>().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("but found an empty segment and 'left'"),
            "{error}"
        );
    }

    #[test]
    fn query_rejects_unknown_key_names() {
        let error = KeyCombo::parse_query("hyper+banana").unwrap_err();
        assert!(
            error.to_string().starts_with("unknown key 'banana'"),
            "{error}"
        );

        let error = KeyCombo::parse_query("ctrl+retrun").unwrap_err();
        assert!(
            error.to_string().contains("did you mean 'return'?"),
            "{error}"
        );

        let error = "hyper+banana".parse::<KeySequence>().unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("invalid key combination 'hyper+banana': unknown key 'banana'"),
            "{error}"
        );

        // Configuration readers keep the lenient parser: an unfamiliar
        // keysym in a config file is not a user error.
        assert!("hyper+banana".parse::<KeyCombo>().is_ok());
    }

    #[test]
    fn query_accepts_every_supported_key_form() {
        for input in [
            "a",
            "é",
            "super+1",
            "super+comma",
            "ctrl+]",
            "ctrl++",
            "ctrl+slash",
            "f1",
            "F35",
            "Prior",
            "kp_enter",
            "KP_Decimal",
            "Print",
            "Scroll_Lock",
            "XF86AudioRaiseVolume",
            "ctrl+code:30",
            "super+mouse:272",
            "super+Super_L",
            "shift+Tab",
            "ctrl+left",
        ] {
            assert!(KeyCombo::parse_query(input).is_ok(), "{input} should parse");
        }
        for input in ["f0", "f36", "f01", "XF86", "mouse:", "KP_BANANA", "=="] {
            assert!(KeyCombo::parse_query(input).is_err(), "{input} should fail");
        }
    }

    #[test]
    fn accepts_altgr_as_mod5() {
        for input in ["altgr+q", "ISO_Level3_Shift+q", "mod5+q"] {
            let combo = KeyCombo::parse_query(input).unwrap();
            assert_eq!(combo.modmask(), MOD5_MASK, "{input}");
            assert_eq!(combo.key(), "Q");
        }
    }

    #[test]
    fn rejects_meta_key_instead_of_aliasing_altgr() {
        // Mod5 means AltGr, so `meta-key` must not silently query AltGr.
        let error = KeyCombo::parse_query("meta-key+k").unwrap_err();
        assert!(
            error.to_string().starts_with(
                "a combination must contain exactly one non-modifier key, but found 'meta-key' and 'k'"
            ),
            "{error}"
        );
        assert!(
            error.to_string().contains("modifiers: ctrl, alt"),
            "{error}"
        );
        assert!(KeyCombo::parse_query("meta-key").is_err());
    }

    #[test]
    fn accepts_plus_as_the_key() {
        let combo: KeyCombo = "ctrl++".parse().unwrap();
        assert_eq!(combo.to_string(), "CTRL + +");

        let combo: KeyCombo = "ctrl+==".parse().unwrap();
        assert_eq!(combo.to_string(), "CTRL + ==");
    }

    #[test]
    fn accepts_numeric_keycodes() {
        let combo: KeyCombo = "ctrl+code:30".parse().unwrap();
        assert_eq!(combo.to_string(), "CTRL + CODE:30");
        assert!("ctrl+code:x".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn displays_a_combo_compactly_for_sequences() {
        let combo: KeyCombo = "ctrl+shift+left".parse().unwrap();
        assert_eq!(combo.compact_display(), "CTRL+SHIFT+LEFT");
    }

    #[test]
    fn parses_a_sequence_of_combinations() {
        let sequence: KeySequence = "ctrl+x ctrl+s".parse().unwrap();

        assert_eq!(sequence.len(), 2);
        assert_eq!(sequence.as_slice()[0].key(), "X");
        assert_eq!(sequence.as_slice()[1].key(), "S");
        assert_eq!(sequence.to_string(), "CTRL+X CTRL+S");
    }

    #[test]
    fn rejects_an_empty_sequence() {
        let error = "  ".parse::<KeySequence>().unwrap_err();
        assert_eq!(error.to_string(), "key sequence is empty");
    }

    #[test]
    fn identifies_the_invalid_sequence_step() {
        let error = "ctrl+x ctrl+shift".parse::<KeySequence>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid key combination at step 2 ('ctrl+shift'): a combination must contain a non-modifier key"
        );
    }

    #[test]
    fn bounds_sequence_length() {
        let input = std::iter::repeat_n("a", MAX_SEQUENCE_LENGTH + 1)
            .collect::<Vec<_>>()
            .join(" ");
        let error = input.parse::<KeySequence>().unwrap_err();
        assert!(error.to_string().contains("maximum is 64"));
    }
}
