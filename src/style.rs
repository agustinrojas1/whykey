//! Small, dependency-free terminal presentation helpers shared by text
//! renderers. Machine-readable output never goes through this module.

use std::env;
use std::fmt;
use std::io::IsTerminal;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl FromStr for ColorChoice {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "invalid color mode '{value}'; expected auto, always, or never"
            )),
        }
    }
}

impl fmt::Display for ColorChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    pub color: ColorChoice,
    pub verbose: bool,
    pub width: usize,
}

impl RenderOptions {
    pub fn plain(verbose: bool) -> Self {
        Self {
            color: ColorChoice::Never,
            verbose,
            width: 80,
        }
    }

    pub fn cli(color: ColorChoice, verbose: bool) -> Self {
        Self {
            color,
            verbose,
            width: terminal_width(),
        }
    }

    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width.max(20);
        self
    }

    pub fn colored(self) -> bool {
        match self.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                env::var_os("NO_COLOR").is_none()
                    && env::var("TERM").ok().as_deref() != Some("dumb")
                    && std::io::stdout().is_terminal()
            }
        }
    }

    pub fn bold(self, text: impl AsRef<str>) -> String {
        self.paint("1", text)
    }

    pub fn accent(self, text: impl AsRef<str>) -> String {
        self.paint("36", text)
    }

    pub fn muted(self, text: impl AsRef<str>) -> String {
        self.paint("2", text)
    }

    pub fn uncertain(self, text: impl AsRef<str>) -> String {
        self.paint("33", text)
    }

    pub fn failure(self, text: impl AsRef<str>) -> String {
        self.paint("31", text)
    }

    pub fn success(self, text: impl AsRef<str>) -> String {
        self.paint("32", text)
    }

    fn paint(self, code: &str, text: impl AsRef<str>) -> String {
        let text = text.as_ref();
        if self.colored() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

pub fn terminal_width() -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 20)
        .unwrap_or(80)
}

/// Wrap ordinary prose with a hanging continuation indent. Long path-like
/// values are kept intact so verbose evidence remains copyable.
pub fn wrap_hanging(text: &str, width: usize, first: &str, continuation: &str) -> Vec<String> {
    let width = width.max(1);
    let first_limit = width.saturating_sub(display_width(first));
    let continuation_limit = width.saturating_sub(display_width(continuation));
    let mut lines = Vec::new();
    let mut paragraph = true;
    let mut current = String::new();
    let mut current_width = 0;
    for word in text.split_whitespace() {
        let word_width = display_width(word);
        let limit = if paragraph {
            first_limit
        } else {
            continuation_limit
        };
        if !current.is_empty() && current_width + 1 + word_width > limit {
            let indent = if lines.is_empty() {
                first
            } else {
                continuation
            };
            lines.push(format!("{indent}{current}"));
            current.clear();
            current_width = 0;
            paragraph = false;
        }
        if !current.is_empty() {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() {
        let indent = if lines.is_empty() {
            first
        } else {
            continuation
        };
        lines.push(format!("{indent}{current}"));
    }
    if lines.is_empty() {
        lines.push(first.to_owned());
    }
    lines
}

/// Display width for the small set of ANSI escapes and Unicode characters
/// that appear in reports. Styling is applied after wrapping, but keeping the
/// parser here makes the helper safe for callers that already styled text.
pub fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' {
            if chars.next() == Some('[') {
                for escape in chars.by_ref() {
                    if ('@'..='~').contains(&escape) {
                        break;
                    }
                }
            }
            continue;
        }
        width += unicode_width(character);
    }
    width
}

fn unicode_width(character: char) -> usize {
    if character == '\0' || ('\u{300}'..='\u{36f}').contains(&character) {
        return 0;
    }
    if character.is_control() {
        return 0;
    }
    let code = character as u32;
    if (0x1100..=0x115f).contains(&code)
        || (0x2e80..=0xa4cf).contains(&code)
        || (0xac00..=0xd7a3).contains(&code)
        || (0xf900..=0xfaff).contains(&code)
        || (0xff01..=0xff60).contains(&code)
        || (0x1f300..=0x1faff).contains(&code)
    {
        2
    } else {
        1
    }
}

pub fn human_key(key: &crate::key::KeyCombo) -> String {
    let mut parts = Vec::new();
    for (mask, name) in [
        (4, "Ctrl"),
        (8, "Alt"),
        (1, "Shift"),
        (64, "Super"),
        (2, "Caps"),
        (16, "Mod2"),
        (32, "Mod3"),
        (128, "Mod5"),
    ] {
        if key.modmask() & mask != 0 {
            parts.push(name.to_owned());
        }
    }
    parts.push(human_key_name(key.key()));
    parts.join("+")
}

/// Normalize a stored machine/display key for human-facing text while
/// leaving JSON and protocol values untouched. Sequence displays are handled
/// one compact combination at a time.
pub fn human_key_text(value: &str) -> String {
    let compact = value.replace(" + ", "+");
    if let Ok(key) = compact.parse::<crate::key::KeyCombo>() {
        return human_key(&key);
    }
    compact
        .split_whitespace()
        .map(|part| {
            part.parse::<crate::key::KeyCombo>()
                .map_or_else(|_| part.to_owned(), |key| human_key(&key))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn human_key_name(value: &str) -> String {
    match value {
        "LEFT" => "Left".into(),
        "RIGHT" => "Right".into(),
        "UP" => "Up".into(),
        "DOWN" => "Down".into(),
        "RETURN" => "Return".into(),
        "ESCAPE" => "Esc".into(),
        "SPACE" => "Space".into(),
        "TAB" => "Tab".into(),
        "PAGE_UP" => "Page Up".into(),
        "PAGE_DOWN" => "Page Down".into(),
        "BACKSPACE" => "Backspace".into(),
        "DELETE" => "Delete".into(),
        "INSERT" => "Insert".into(),
        "HOME" => "Home".into(),
        "END" => "End".into(),
        other if other.chars().count() == 1 => other.to_ascii_uppercase(),
        other => other
            .split('_')
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map_or_else(String::new, |first| first.to_uppercase().collect())
                    + chars.as_str().to_ascii_lowercase().as_str()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_human_shortcuts_without_spaces() {
        let key: crate::key::KeyCombo = "ctrl+z".parse().unwrap();
        assert_eq!(human_key(&key), "Ctrl+Z");
        let key: crate::key::KeyCombo = "super+k".parse().unwrap();
        assert_eq!(human_key(&key), "Super+K");
        assert_eq!(human_key_text("CTRL + Z"), "Ctrl+Z");
        assert_eq!(human_key_text("CTRL+X CTRL+S"), "Ctrl+X Ctrl+S");
    }

    #[test]
    fn wraps_with_a_hanging_indent() {
        let lines = wrap_hanging("one two three four", 12, "  ", "    ");
        assert_eq!(lines, vec!["  one two", "    three", "    four"]);
    }

    #[test]
    fn display_width_ignores_ansi_and_counts_wide_characters() {
        assert_eq!(display_width("\x1b[33mCtrl+Z\x1b[0m"), 6);
        assert_eq!(display_width("界"), 2);
    }

    #[test]
    fn explicit_color_modes_are_stable() {
        assert_eq!(ColorChoice::from_str("never").unwrap(), ColorChoice::Never);
        assert!(RenderOptions::plain(false).bold("x").is_ascii());
    }
}
