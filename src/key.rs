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
    pub(crate) fn from_parts(modifiers: u32, key: impl Into<String>) -> Self {
        Self {
            modifiers,
            key: key.into().to_ascii_uppercase(),
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

        tokens
            .into_iter()
            .enumerate()
            .map(|(index, token)| {
                token.parse::<KeyCombo>().map_err(|error| {
                    ParseKeySequenceError(format!(
                        "invalid key combination at step {} ('{token}'): {error}",
                        index + 1
                    ))
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

impl FromStr for KeyCombo {
    type Err = ParseKeyComboError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
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
                        "modifier '{part}' appears more than once"
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
            return Err(ParseKeyComboError(
                "a combination must contain exactly one non-modifier key".into(),
            ));
        }
        let key = normalize_key(remainder)?;

        Ok(Self { modifiers, key })
    }
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

fn modifier_mask(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "ctl" => Some(CTRL_MASK),
        "alt" | "mod1" => Some(ALT_MASK),
        "shift" => Some(SHIFT_MASK),
        "super" | "meta" | "win" | "mod4" => Some(SUPER_MASK),
        "caps" | "capslock" | "lock" => Some(CAPS_MASK),
        "mod2" | "num" | "numlock" => Some(MOD2_MASK),
        "mod3" | "hyper" => Some(MOD3_MASK),
        "mod5" | "meta-key" => Some(MOD5_MASK),
        _ => None,
    }
}

fn normalize_key(value: &str) -> Result<String, ParseKeyComboError> {
    if value.chars().any(char::is_whitespace) {
        return Err(ParseKeyComboError(format!(
            "key '{value}' contains whitespace"
        )));
    }

    let normalized = match value.to_ascii_lowercase().as_str() {
        "arrowleft" | "arrow_left" | "leftarrow" | "left_arrow" => "LEFT",
        "arrowright" | "arrow_right" | "rightarrow" | "right_arrow" => "RIGHT",
        "arrowup" | "arrow_up" | "uparrow" | "up_arrow" => "UP",
        "arrowdown" | "arrow_down" | "downarrow" | "down_arrow" => "DOWN",
        "enter" | "return" | "kp_enter" | "kp_return" => "RETURN",
        "esc" | "escape" => "ESCAPE",
        "spacebar" => "SPACE",
        "pgup" | "pageup" | "page_up" | "prior" => "PAGE_UP",
        "pgdown" | "pagedown" | "page_down" | "next" => "PAGE_DOWN",
        "backspace" => "BACKSPACE",
        "delete" | "del" => "DELETE",
        "insert" | "ins" => "INSERT",
        "home" => "HOME",
        "end" => "END",
        "tab" => "TAB",
        "space" => "SPACE",
        value if value.starts_with("code:") => {
            let code = value.strip_prefix("code:").unwrap_or_default();
            if code.is_empty() || !code.chars().all(|character| character.is_ascii_digit()) {
                return Err(ParseKeyComboError(format!("invalid keycode '{value}'")));
            }
            return Ok(format!("CODE:{code}"));
        }
        _ => return Ok(value.to_ascii_uppercase()),
    };

    Ok(normalized.into())
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
        assert_eq!("KP_Enter".parse::<KeyCombo>().unwrap().key(), "RETURN");
        assert_eq!("Escape".parse::<KeyCombo>().unwrap().key(), "ESCAPE");
    }

    #[test]
    fn rejects_two_non_modifier_keys() {
        let error = "ctrl+a+b".parse::<KeyCombo>().unwrap_err();

        assert_eq!(
            error.to_string(),
            "a combination must contain exactly one non-modifier key"
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
        assert_eq!(
            error.to_string(),
            "a combination must contain exactly one non-modifier key"
        );
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
