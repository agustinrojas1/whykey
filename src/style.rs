//! Small, dependency-free terminal presentation helpers shared by text
//! renderers. Machine-readable output never goes through this module.

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
                crate::recording::env_var_os("NO_COLOR").is_none()
                    && crate::recording::env_var("TERM").ok().as_deref() != Some("dumb")
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
    crate::recording::env_var("COLUMNS")
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
    let mut parts = human_modifiers(key.modmask());
    parts.push(human_key_name(key.key()));
    parts.join("+")
}

/// Human names of the modifiers in an X11/Hyprland modmask, in display
/// order. Mod5 is shown as AltGr: XKB layouts put ISO_Level3_Shift there.
pub fn human_modifiers(modmask: u32) -> Vec<String> {
    [
        (4, "Ctrl"),
        (8, "Alt"),
        (1, "Shift"),
        (64, "Super"),
        (2, "Caps"),
        (16, "Mod2"),
        (32, "Mod3"),
        (128, "AltGr"),
    ]
    .into_iter()
    .filter(|(mask, _)| modmask & mask != 0)
    .map(|(_, name)| name.to_owned())
    .collect()
}

/// Normalize a stored machine/display key for human-facing text while
/// leaving JSON and protocol values untouched. Sequence displays are handled
/// one compact combination at a time.
pub fn human_key_text(value: &str) -> String {
    let compact = value.replace(" + ", "+");
    // A placeholder such as `SUPER+<key not reported>` names no key; only
    // its modifiers are humanized.
    if let Some((prefix, placeholder)) = compact.split_once('<') {
        let modifiers = prefix
            .split('+')
            .filter(|part| !part.is_empty())
            .map(|part| crate::key::modifier_mask(part).map(human_modifiers))
            .collect::<Option<Vec<_>>>();
        if let Some(modifiers) = modifiers {
            let mut parts: Vec<String> = modifiers.into_iter().flatten().collect();
            parts.push(format!("<{placeholder}"));
            return parts.join("+");
        }
    }
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
        other if other.starts_with("SWITCH:") => {
            // Hyprland lid/tablet switches: `switch:[on:|off:]<device name>`.
            // The device name's case is lost in storage; title-case its words.
            let mut parts: Vec<&str> = other.splitn(3, ':').collect();
            let name = parts.pop().unwrap_or_default();
            let name = name
                .split(' ')
                .map(title_case)
                .collect::<Vec<_>>()
                .join(" ");
            format!("{}:{name}", parts.join(":").to_ascii_lowercase())
        }
        other if other.starts_with("CODE:") => other.to_ascii_lowercase(),
        other if other.starts_with("XF86") => xf86_name(&other[4..]),
        other => other
            .split('_')
            .map(title_case)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map_or_else(String::new, |first| first.to_uppercase().collect())
        + chars.as_str().to_ascii_lowercase().as_str()
}

/// Key names are stored upper-cased, which loses the camel case of XF86
/// keysym names (`XF86AUDIOMUTE`). Restore the libxkbcommon spelling
/// (`XF86AudioMute`); an unlisted name keeps `XF86` and the stored letters.
fn xf86_name(suffix: &str) -> String {
    XF86_NAMES
        .split_ascii_whitespace()
        .find(|name| name.eq_ignore_ascii_case(suffix))
        .map_or_else(|| format!("XF86{suffix}"), |name| format!("XF86{name}"))
}

/// Every `XKB_KEY_XF86*` name in libxkbcommon's `xkbcommon-keysyms.h`, without
/// the `XF86` prefix, deduplicated case-insensitively.
const XF86_NAMES: &str = "\
    10ChannelsDown 10ChannelsUp 3DMode Accessibility ActionOnSelection AddFavorite Addressbook \
    ALSToggle ApplicationLeft ApplicationRight AppSelect AspectRatio Assistant AttendantOff \
    AttendantOn AttendantToggle Audio AudioBassBoost AudioChannelMode AudioCycleTrack \
    AudioDesc AudioForward AudioLowerVolume AudioMedia AudioMicMute AudioMute AudioNext \
    AudioPause AudioPlay AudioPreset AudioPrev AudioRaiseVolume AudioRandomPlay AudioRecord \
    AudioRepeat AudioRewind AudioStop AutopilotEngageToggle Away Back BackForward Battery Blue \
    Bluetooth Book Break BrightnessAdjust BrightnessAuto BrightnessMax BrightnessMin \
    Buttonconfig Calculater Calculator Calendar CamelCaseKernelName CameraAccessDisable \
    CameraAccessEnable CameraAccessToggle CameraDown CameraFocus CameraLeft CameraRight \
    CameraUp CameraZoomIn CameraZoomOut CD ChannelDown ChannelUp Clear ClearGrab ClearvuSonar \
    Close Community ContextMenu ContextualInsert ContextualQuery ContrastAdjust ControlPanel \
    Copy Cut CycleAngle Data Database DeleteLine DeleteToEndOfLine DeleteToEndOfScreen Dictate \
    Display DisplayOff DisplayToggle Documents DoNotDisturb DOS DualRangeRadar DVD Editor \
    Eject ElectronicPrivacyScreenOff ElectronicPrivacyScreenOn EmojiPicker Excel Exit Explorer \
    FastReverse Favorites Finance FishingChart Fn FnRightShift Fn_1 Fn_2 Fn_B Fn_D Fn_E Fn_Esc \
    Fn_F Fn_F1 Fn_F10 Fn_F11 Fn_F12 Fn_F2 Fn_F3 Fn_F4 Fn_F5 Fn_F6 Fn_F7 Fn_F8 Fn_F9 Fn_S Foo \
    Forward FrameBack FrameForward FullScreen Game Go GoTo GraphicsEditor Green HangupPhone \
    Hibernate History HomePage HotLinks Images Info InsertLine iTouch Journal \
    KbdBrightnessDown KbdBrightnessUp KbdInputAssistAccept KbdInputAssistCancel \
    KbdInputAssistNext KbdInputAssistNextgroup KbdInputAssistPrev KbdInputAssistPrevgroup \
    KbdLcdMenu1 KbdLcdMenu2 KbdLcdMenu3 KbdLcdMenu4 KbdLcdMenu5 KbdLightOnOff Keyboard Launch0 \
    Launch1 Launch2 Launch3 Launch4 Launch5 Launch6 Launch7 Launch8 Launch9 LaunchA LaunchB \
    LaunchC LaunchD LaunchE LaunchF LeftDown LeftUp LightBulb LightsToggle LinkPhone \
    LogGrabInfo LogOff LogWindowTree Macro1 Macro10 Macro11 Macro12 Macro13 Macro14 Macro15 \
    Macro16 Macro17 Macro18 Macro19 Macro2 Macro20 Macro21 Macro22 Macro23 Macro24 Macro25 \
    Macro26 Macro27 Macro28 Macro29 Macro3 Macro30 Macro4 Macro5 Macro6 Macro7 Macro8 Macro9 \
    MacroPreset1 MacroPreset2 MacroPreset3 MacroPresetCycle MacroRecordStart MacroRecordStop \
    Mail MailForward Market MarkWaypoint MediaLanguageMenu MediaPlayer MediaPlayPause \
    MediaPlaySlow MediaRepeat MediaSelectAuxiliary MediaSelectCable MediaSelectCD \
    MediaSelectDVD MediaSelectHome MediaSelectPC MediaSelectProgramGuide MediaSelectRadio \
    MediaSelectSatellite MediaSelectTape MediaSelectTeletext MediaSelectTuner MediaSelectTV \
    MediaSelectVCR MediaSelectVCRPlus MediaTitleMenu MediaTopMenu Meeting Memo MenuKB MenuPB \
    Messenger ModeLock MonBrightnessAuto MonBrightnessCycle MonBrightnessDown MonBrightnessUp \
    Music MyComputer MySites NavChart NavInfo New News NextElement NextFavorite Next_VMode \
    NotificationCenter NumberEntryMode Numeric0 Numeric1 Numeric11 Numeric12 Numeric2 Numeric3 \
    Numeric4 Numeric5 Numeric6 Numeric7 Numeric8 Numeric9 NumericA NumericB NumericC NumericD \
    NumericPound NumericStar OfficeHome OK OnScreenKeyboard Open OpenURL Option Paste \
    PauseRecord PerformanceMode Phone PickupPhone Pictures PowerDown PowerOff Presentation \
    PreviousElement Prev_VMode PrivacyScreenToggle Q RadarOverlay Red Refresh \
    RefreshRateToggle Reload Reply RFKill RightDown RightUp RockerDown RockerEnter RockerUp \
    RootMenu RotateWindows RotationKB RotationLockToggle RotationPB Save ScreenSaver \
    ScrollClick ScrollDown ScrollUp Search Select SelectiveScreenshot Send Shop SidevuSonar \
    SingleRangeRadar Sleep SlowReverse Sos Spell SpellCheck SplitScreen Sport Standby Start \
    Stop StopRecord Subtitle Support Suspend Switch_VT_1 Switch_VT_10 Switch_VT_11 \
    Switch_VT_12 Switch_VT_2 Switch_VT_3 Switch_VT_4 Switch_VT_5 Switch_VT_6 Switch_VT_7 \
    Switch_VT_8 Switch_VT_9 Taskmanager TaskPane Terminal Time ToDoList Tools TopMenu \
    TouchpadOff TouchpadOn TouchpadToggle TraditionalSonar Travel Ungrab Unmute User1KB \
    User2KB UserPB UWB VendorHome VendorLogo Video VideoPhone View VOD VoiceCommand Voicemail \
    WakeUp WebCam WheelButton WLAN Word WPSButton WWAN WWW Xfer Yellow ZoomIn ZoomOut \
    ZoomReset";

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
    fn keeps_unreported_key_placeholders_readable() {
        assert_eq!(
            human_key_text("ALT+SUPER+<key not reported>"),
            "Alt+Super+<key not reported>"
        );
        assert_eq!(human_key_text("CTRL+<"), "Ctrl+<");
    }

    #[test]
    fn keeps_xf86_camel_case_and_lowercase_keycodes() {
        assert_eq!(human_key_text("XF86AUDIOMUTE"), "XF86AudioMute");
        assert_eq!(
            human_key_text("SUPER+XF86TOUCHPADOFF"),
            "Super+XF86TouchpadOff"
        );
        assert_eq!(
            human_key_text("SHIFT+SUPER+CODE:201"),
            "Shift+Super+code:201"
        );
        let lid = crate::key::KeyCombo::from_parts(0, "SWITCH:OFF:LID SWITCH");
        assert_eq!(human_key(&lid), "switch:off:Lid Switch");
    }

    #[test]
    fn names_mod5_altgr() {
        let key: crate::key::KeyCombo = "altgr+shift+q".parse().unwrap();
        assert_eq!(human_key(&key), "Shift+AltGr+Q");
        assert_eq!(human_key_text("MOD5+Q"), "AltGr+Q");
        assert_eq!(human_modifiers(64 | 128), ["Super", "AltGr"]);
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
