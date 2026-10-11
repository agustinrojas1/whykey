use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{EvidenceTier, LayerId, LayerResult, Outcome, TerminalInput};
use crate::xkb::{self, KeyboardLayout, LevelText, XkbState};

const SHIFT_MASK: u32 = 1;
const CAPS_MASK: u32 = 2;
const NUM_MASK: u32 = 16;
/// AltGr (ISO_Level3_Shift on Mod5): an XKB level shift, not a modifier the
/// terminal reports.
const ALTGR_MASK: u32 = 128;

/// A terminal's normal output for a key, or why it cannot be predicted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyEncoding {
    Bytes(Vec<u8>),
    /// A keypad key: `normal` in the default (numeric) keypad mode,
    /// `application` once a program turns on application keypad mode
    /// (DECKPAM, for example through the terminfo `smkx` string).
    Keypad {
        normal: Vec<u8>,
        application: Vec<u8>,
    },
    /// A dead keysym starts composition in the terminal; nothing is sent
    /// until the next key completes it.
    DeadKey(String),
    /// The bytes cannot be predicted; the reason names the missing evidence.
    Unknown(String),
    /// No default encoding is modeled for this key.
    Unmodeled,
}

impl KeyEncoding {
    fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(bytes) | Self::Keypad { normal: bytes, .. } => Some(bytes),
            Self::DeadKey(_) | Self::Unknown(_) | Self::Unmodeled => None,
        }
    }
}

/// Attach a pass-through key's default encoding to the terminal finding.
/// `composes` says whether this finding describes the terminal that would
/// compose a dead key; it then consumes the key, since no byte reaches the
/// PTY until composition completes.
pub(crate) fn with_default_encoding(
    mut layer: LayerResult,
    encoding: KeyEncoding,
    description: impl Into<String>,
    composes: bool,
) -> (LayerResult, Option<TerminalInput>) {
    match encoding {
        KeyEncoding::Bytes(bytes) => (layer, Some(TerminalInput::predicted(bytes, description))),
        KeyEncoding::Keypad {
            normal,
            application,
        } => {
            layer.details.push(format!(
                "in application keypad mode (DECKPAM, which programs set through terminfo smkx) \
                 the terminal sends {} instead",
                crate::layers::format_bytes(&application)
            ));
            (layer, Some(TerminalInput::predicted(normal, description)))
        }
        KeyEncoding::DeadKey(keysym) => {
            if composes && layer.outcome == Outcome::Pass {
                layer.outcome = Outcome::Consumed;
                layer.summary =
                    "starts dead-key composition; no bytes are sent until the next key".into();
                layer.dead_key = Some(keysym.clone());
            }
            layer.details.push(format!(
                "dead key {keysym}: composition sends no bytes until the next key"
            ));
            (layer, None)
        }
        KeyEncoding::Unknown(reason) => {
            layer
                .details
                .push(format!("normal encoding is unknown: {reason}"));
            (layer, None)
        }
        KeyEncoding::Unmodeled => (layer, None),
    }
}

pub struct Ghostty {
    config_path: PathBuf,
    use_cli: bool,
}

impl Default for Ghostty {
    fn default() -> Self {
        Self {
            config_path: default_config_path(),
            use_cli: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    trigger: KeyCombo,
    action: String,
    unconsumed: bool,
    global: bool,
    all: bool,
    performable: bool,
}

#[derive(Debug, Default)]
struct ConfigState {
    bindings: Vec<Binding>,
    loaded_files: Vec<PathBuf>,
    source: Option<String>,
    unresolved_triggers: Vec<String>,
}

impl Ghostty {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        self.analyze(key, None, None).0
    }
}
/// Whether Ghostty is the active terminal. Used by the unified terminal
/// identity so text, doctor, capabilities, and schema context agree.
pub fn is_active() -> bool {
    known_non_ghostty_terminal().is_none()
}

impl Ghostty {
    /// Analyze once: route finding plus normal input bytes from a single
    /// config load. The listener's probe protocol only affects the byte
    /// prediction, never the binding layer itself. `layout` resolves the
    /// text of AltGr (level-3) keys.
    pub fn analyze(
        &self,
        key: &KeyCombo,
        protocol_flags: Option<u32>,
        layout: Option<&KeyboardLayout>,
    ) -> (LayerResult, Option<TerminalInput>) {
        if let Some(program) = known_non_ghostty_terminal() {
            return generic_fallback(&program, key, protocol_flags, layout);
        }
        let config = match self.load_config() {
            Ok(config) => config,
            Err(message) => {
                return (
                    LayerResult::new(
                        "Ghostty",
                        LayerId::Terminal,
                        Outcome::Unavailable,
                        "could not inspect Ghostty configuration",
                        vec![message],
                    ),
                    None,
                );
            }
        };
        let layer = inspect_config(key, &self.config_path, &config, layout)
            .with_optional_tier(config.tier());
        let has_binding = config
            .bindings
            .iter()
            .any(|binding| &binding.trigger == key);
        if has_binding {
            let input = Self::input_from_config(&config, key, protocol_flags, layout);
            return (layer, input);
        }
        with_default_encoding(
            layer,
            default_encoding_for_protocol(key, protocol_flags, layout),
            "Ghostty default encoding",
            true,
        )
    }

    fn input_from_config(
        config: &ConfigState,
        key: &KeyCombo,
        protocol_flags: Option<u32>,
        layout: Option<&KeyboardLayout>,
    ) -> Option<TerminalInput> {
        let binding = config
            .bindings
            .iter()
            .find(|binding| &binding.trigger == key)?;
        if binding.global || binding.all || binding.action == "ignore" {
            return None;
        }
        let default = || default_encoding_for_protocol(key, protocol_flags, layout).into_bytes();
        if binding.performable {
            return default().map(|bytes| {
                TerminalInput::predicted(
                    bytes,
                    "default encoding when the action is not performable",
                )
            });
        }
        if let Some(bytes) = action_encoding(&binding.action) {
            if binding.unconsumed {
                let mut combined = bytes.clone();
                if let Some(default) = default() {
                    combined.extend(default);
                    return Some(TerminalInput::configured(
                        combined,
                        "explicit Ghostty output followed by normal key encoding",
                    ));
                }
            }
            return Some(TerminalInput::configured(
                bytes,
                "explicit Ghostty binding output",
            ));
        }
        if binding.unconsumed {
            return default().map(|bytes| {
                TerminalInput::predicted(bytes, "default encoding from an unconsumed binding")
            });
        }
        None
    }
    /// Resolve the bytes that Ghostty would normally put on the child PTY.
    ///
    /// The listener temporarily enables Kitty's keyboard protocol to identify
    /// modifiers. Those probe bytes are not the bytes that Bash would usually
    /// receive, so downstream layers use this separate value instead.
    pub fn normal_input_with_protocol(
        &self,
        key: &KeyCombo,
        protocol_flags: Option<u32>,
    ) -> Option<TerminalInput> {
        self.analyze(key, protocol_flags, None).1
    }

    fn load_config(&self) -> Result<ConfigState, String> {
        if self.use_cli {
            if let Some(config) = load_cli_keybinds() {
                return Ok(config);
            }
        }
        load_config_file(&self.config_path)
    }
}

impl ConfigState {
    /// `ghostty +list-keybinds` (the only producer of `source`) evaluates
    /// the active configuration the way Ghostty does; otherwise the loaded
    /// files were read statically. `None` when no file was read.
    fn tier(&self) -> Option<EvidenceTier> {
        if self.source.is_some() {
            Some(EvidenceTier::LiveQuery)
        } else if !self.loaded_files.is_empty() {
            Some(EvidenceTier::ConfigFile)
        } else {
            None
        }
    }
}

/// Analysis for a terminal that is not Ghostty: no bindings to inspect, but
/// the terminal still emits its normal encoding, including AltGr level-3
/// text and dead-key composition.
fn generic_fallback(
    program: &str,
    key: &KeyCombo,
    protocol_flags: Option<u32>,
    layout: Option<&KeyboardLayout>,
) -> (LayerResult, Option<TerminalInput>) {
    let detail = if program == "terminal identity unknown" {
        "terminal identity is unavailable; Ghostty bindings were skipped".into()
    } else {
        format!("TERM_PROGRAM={program}; Ghostty bindings were skipped")
    };
    let layer = LayerResult::new(
        "Ghostty",
        LayerId::Terminal,
        Outcome::Pass,
        "not applicable to the current terminal",
        vec![detail],
    );
    let description = if program == "terminal identity unknown" {
        "generic terminal fallback; terminal identity is unknown".into()
    } else {
        format!("generic terminal fallback; {program} is not Ghostty")
    };
    // The generic fallback models the active terminal itself, so a
    // dead key composes there exactly as in a named terminal.
    let encoding = default_encoding_for_protocol(key, protocol_flags, layout);
    let layer = if matches!(encoding, KeyEncoding::DeadKey(_)) {
        LayerResult {
            layer: "Terminal",
            ..layer
        }
    } else {
        layer
    };
    with_default_encoding(layer, encoding, description, true)
}

fn known_non_ghostty_terminal() -> Option<String> {
    let Some(program) = crate::layers::terminal_app::terminal_program() else {
        return Some("terminal identity unknown".into());
    };
    let normalized = program.to_ascii_lowercase();
    if matches!(normalized.as_str(), "ghostty" | "xterm-ghostty") {
        return None;
    }
    Some(program)
}

fn inspect_config(
    key: &KeyCombo,
    config_path: &Path,
    config: &ConfigState,
    layout: Option<&KeyboardLayout>,
) -> LayerResult {
    let Some(binding) = config
        .bindings
        .iter()
        .find(|binding| &binding.trigger == key)
    else {
        let mut details = vec![format!("config: {}", config_path.display())];
        if let Some(source) = &config.source {
            details.push(format!("source: {source}"));
        }
        if !config.loaded_files.is_empty() {
            details.push(format!(
                "loaded {} config file(s)",
                config.loaded_files.len()
            ));
        }
        if !config.unresolved_triggers.is_empty() {
            details.push(format!(
                "{} trigger(s) not resolved (physical or multi-key): {}",
                config.unresolved_triggers.len(),
                config
                    .unresolved_triggers
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(trigger) = unresolved_single_key_trigger(config, key) {
            details.push(format!("possible physical binding: {trigger}"));
            return LayerResult::new(
                "Ghostty",
                LayerId::Terminal,
                Outcome::Unknown,
                "possible physical binding may capture the key",
                details,
            );
        }
        match default_encoding_for_protocol(key, None, layout) {
            KeyEncoding::Bytes(encoding)
            | KeyEncoding::Keypad {
                normal: encoding, ..
            } => {
                details.push(format_encoding_detail(&encoding));
                return LayerResult::new(
                    "Ghostty",
                    LayerId::Terminal,
                    Outcome::Pass,
                    "no Ghostty binding; forwarded to terminal",
                    details,
                );
            }
            // The analysis records the specific reason with the input.
            KeyEncoding::DeadKey(_) | KeyEncoding::Unknown(_) => {}
            KeyEncoding::Unmodeled => {
                details.push("default key encoding is layout-dependent or unsupported".into());
            }
        }
        return LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::Pass,
            "no Ghostty binding; forwarded to terminal",
            details,
        );
    };

    let mut details = vec![format!(
        "binding: {} = {}",
        trigger_label(binding),
        binding.action
    )];
    if binding.global {
        details.push("scope: global".into());
    } else if binding.all {
        details.push("scope: all terminal surfaces".into());
    }

    if let Some(encoding) = action_encoding(&binding.action) {
        details.push(format_encoding_detail(&encoding));
        if binding.unconsumed {
            return LayerResult::new(
                "Ghostty",
                LayerId::Terminal,
                Outcome::HandledAndPassed,
                "binding sends a sequence and leaves it unconsumed",
                details,
            );
        }
        return LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::HandledAndPassed,
            "binding sends a sequence to the terminal",
            details,
        );
    }

    if binding.action == "ignore" {
        return LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::Consumed,
            "binding ignores the key input",
            details,
        );
    }

    if binding.performable {
        details.push("action may pass through when it is not performable".into());
        return LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::HandledUncertain,
            "binding may consume the key depending on terminal state",
            details,
        );
    }

    if binding.unconsumed {
        return LayerResult::new(
            "Ghostty",
            LayerId::Terminal,
            Outcome::HandledAndPassed,
            "binding runs and leaves the key unconsumed",
            details,
        );
    }

    LayerResult::new(
        "Ghostty",
        LayerId::Terminal,
        Outcome::Consumed,
        "binding consumes the key",
        details,
    )
}

fn load_config_file(path: &Path) -> Result<ConfigState, String> {
    let mut state = ConfigState::default();
    let mut visited = HashSet::new();
    if path.exists() {
        load_file(path, &mut state, &mut visited, false)?;
    }
    Ok(state)
}

fn load_file(
    path: &Path,
    state: &mut ConfigState,
    visited: &mut HashSet<PathBuf>,
    optional: bool,
) -> Result<(), String> {
    let path = expand_path(path);
    let canonical = match fs::canonicalize(&path) {
        Ok(path) => path,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to resolve {}: {error}", path.display())),
    };
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }

    let content = fs::read_to_string(&canonical)
        .map_err(|error| format!("failed to read {}: {error}", canonical.display()))?;
    state.loaded_files.push(canonical.clone());
    let mut includes = Vec::new();

    for (line_number, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        match name {
            "config-file" => {
                let (optional, include) = parse_config_file(value);
                let include = resolve_include(&canonical, &include);
                includes.push((optional, include, line_number + 1));
            }
            "keybind" => parse_keybind(value, state)
                .map_err(|error| format!("{}:{}: {error}", canonical.display(), line_number + 1))?,
            _ => {}
        }
    }

    for (optional, include, line_number) in includes {
        load_file(&include, state, visited, optional)
            .map_err(|error| format!("{}:{}: {error}", canonical.display(), line_number))?;
    }

    Ok(())
}

fn load_cli_keybinds() -> Option<ConfigState> {
    let mut command = Command::new("ghostty");
    command.args(["+list-keybinds", "--plain"]);
    let output = command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let output = String::from_utf8(output.stdout).ok()?;
    parse_cli_keybinds(&output)
}

fn parse_cli_keybinds(output: &str) -> Option<ConfigState> {
    let mut state = ConfigState {
        source: Some("ghostty +list-keybinds --plain".into()),
        ..ConfigState::default()
    };
    for line in output.lines() {
        let Some(value) = line.strip_prefix("keybind =") else {
            continue;
        };
        let _ = parse_keybind(value.trim(), &mut state);
    }
    Some(state)
}

fn parse_keybind(value: &str, state: &mut ConfigState) -> Result<(), String> {
    if value == "clear" {
        state.bindings.clear();
        state.unresolved_triggers.clear();
        return Ok(());
    }

    // Ghostty uses `=` both as the key name (the `=` key) and as the
    // trigger/action separator. Splitting at the last one keeps bindings such
    // as `ctrl+==increase_font_size` unambiguous.
    let Some((raw_trigger, raw_action)) = value.rsplit_once('=') else {
        return Err("keybind must contain trigger=action".into());
    };
    let action = raw_action.trim().to_owned();
    let Some((trigger, prefixes)) = parse_trigger(raw_trigger.trim()) else {
        state
            .unresolved_triggers
            .push(format!("{}={action}", raw_trigger.trim()));
        return Ok(());
    };

    if action == "unbind" {
        state.bindings.retain(|binding| binding.trigger != trigger);
        return Ok(());
    }

    state.bindings.retain(|binding| binding.trigger != trigger);
    state.bindings.push(Binding {
        trigger,
        action,
        unconsumed: prefixes.unconsumed,
        global: prefixes.global,
        all: prefixes.all,
        performable: prefixes.performable,
    });
    Ok(())
}

#[derive(Debug, Default)]
struct TriggerPrefixes {
    unconsumed: bool,
    global: bool,
    all: bool,
    performable: bool,
}

fn parse_trigger(raw_trigger: &str) -> Option<(KeyCombo, TriggerPrefixes)> {
    if raw_trigger.contains('>') {
        return None;
    }

    let mut trigger = raw_trigger.trim();
    let mut prefixes = TriggerPrefixes::default();
    while let Some((prefix, rest)) = trigger.split_once(':') {
        match prefix.to_ascii_lowercase().as_str() {
            "unconsumed" => prefixes.unconsumed = true,
            "global" => prefixes.global = true,
            "all" => prefixes.all = true,
            "performable" => prefixes.performable = true,
            "physical" => return None,
            _ => break,
        }
        trigger = rest.trim();
    }

    trigger.parse().ok().map(|trigger| (trigger, prefixes))
}

fn parse_config_file(value: &str) -> (bool, PathBuf) {
    let value = value.trim();
    let (optional, value) = value
        .strip_prefix('?')
        .map_or((false, value), |value| (true, value));
    (optional, crate::util::unquote(value.trim()).into())
}

fn resolve_include(parent: &Path, include: &Path) -> PathBuf {
    let include = expand_path(include);
    if include.is_absolute() {
        include
    } else {
        parent
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(include)
    }
}

fn expand_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_owned()
}

fn default_config_path() -> PathBuf {
    let base = crate::util::config_base().unwrap_or_else(|| PathBuf::from(".config"));
    let modern = base.join("ghostty/config.ghostty");
    if modern.exists() {
        modern
    } else {
        let legacy = base.join("ghostty/config");
        if legacy.exists() { legacy } else { modern }
    }
}

fn trigger_label(binding: &Binding) -> String {
    let mut label = binding.trigger.to_string();
    let mut prefixes = Vec::new();
    if binding.global {
        prefixes.push("global");
    } else if binding.all {
        prefixes.push("all");
    }
    if binding.unconsumed {
        prefixes.push("unconsumed");
    }
    if binding.performable {
        prefixes.push("performable");
    }
    if !prefixes.is_empty() {
        label = format!("{}:{label}", prefixes.join(":"));
    }
    label
}

fn unresolved_single_key_trigger(config: &ConfigState, key: &KeyCombo) -> Option<String> {
    config.unresolved_triggers.iter().find_map(|entry| {
        let trigger = entry.strip_prefix("physical:")?.split_once('=')?.0.trim();
        (trigger.parse::<KeyCombo>().ok().as_ref() == Some(key)).then(|| entry.to_owned())
    })
}

fn action_encoding(action: &str) -> Option<Vec<u8>> {
    if let Some(value) = action.strip_prefix("csi:") {
        return Some(format!("\x1b[{value}").into_bytes());
    }
    if let Some(value) = action.strip_prefix("esc:") {
        return Some(format!("\x1b{value}").into_bytes());
    }
    action.strip_prefix("text:").map(decode_text)
}

fn decode_text(value: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            let Some('x') = chars.next() else {
                bytes.push(b'\\');
                continue;
            };
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
            } else {
                bytes.extend_from_slice(format!("\\x{hex}").as_bytes());
            }
        } else {
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        }
    }
    bytes
}

/// The terminal's normal encoding for `key`. AltGr is a level shift: XKB
/// consumes it into the level-3 (level-4 with Shift) keysym and the
/// terminal sends that symbol's text, so it is resolved through `layout`
/// rather than encoded as a modifier.
pub(crate) fn default_encoding_for_protocol(
    key: &KeyCombo,
    protocol_flags: Option<u32>,
    layout: Option<&KeyboardLayout>,
) -> KeyEncoding {
    if key.modmask() & ALTGR_MASK != 0 {
        return altgr_encoding(key, protocol_flags, layout);
    }
    if let Some(encoding) = keypad_encoding(key, protocol_flags) {
        return encoding;
    }
    modifier_encoding(key, protocol_flags).map_or(KeyEncoding::Unmodeled, KeyEncoding::Bytes)
}

fn altgr_encoding(
    key: &KeyCombo,
    protocol_flags: Option<u32>,
    layout: Option<&KeyboardLayout>,
) -> KeyEncoding {
    let mask = key.modmask();
    if mask & !(ALTGR_MASK | SHIFT_MASK | CAPS_MASK | NUM_MASK) != 0 {
        return KeyEncoding::Unknown(
            "AltGr combined with Ctrl, Alt, Super, or another modifier is not modeled; \
             the terminal's handling of the remaining modifiers is terminal-specific"
                .into(),
        );
    }
    if protocol_flags.is_some_and(|flags| flags & 8 != 0) {
        return KeyEncoding::Unknown(
            "the Kitty report-all-keys mode sends AltGr text as an escape code whose key \
             and modifier fields are terminal-specific"
                .into(),
        );
    }
    let Some(layout) = layout else {
        return KeyEncoding::Unknown(
            "the active XKB keymap is unavailable, so the AltGr (level 3) symbol cannot be \
             resolved"
                .into(),
        );
    };
    let state = XkbState::from_modifier_mask(
        mask,
        Some(layout.group),
        mask & CAPS_MASK != 0,
        mask & NUM_MASK != 0,
        false,
    );
    match xkb::resolve_text(layout, key.key(), &state) {
        LevelText::Text { text, .. } => {
            let mut encoded = [0; 4];
            KeyEncoding::Bytes(text.encode_utf8(&mut encoded).as_bytes().to_vec())
        }
        LevelText::Dead { keysym } => KeyEncoding::DeadKey(keysym),
        LevelText::Unresolved(reason) => KeyEncoding::Unknown(reason),
    }
}

/// Encoding of a key whose modifiers the terminal reports itself.
fn modifier_encoding(key: &KeyCombo, protocol_flags: Option<u32>) -> Option<Vec<u8>> {
    if let Some(flags) = protocol_flags {
        if flags & (1 | 8) != 0 {
            if let Some(bytes) = kitty_encoding(key) {
                return Some(bytes);
            }
        }
    }

    let key_name = key.key();
    let modifiers = key_modifier_parameter(key.modmask());
    let arrow = match key_name {
        "LEFT" => 'D',
        "RIGHT" => 'C',
        "UP" => 'A',
        "DOWN" => 'B',
        _ => return default_non_arrow_encoding(key_name, key.modmask()),
    };
    if modifiers == 1 {
        Some(format!("\x1b[{arrow}").into_bytes())
    } else {
        Some(format!("\x1b[1;{modifiers}{arrow}").into_bytes())
    }
}

/// A keypad key (`KP_*`): the main key it reports as in legacy encoding,
/// its Kitty keyboard protocol code, and its application keypad mode final
/// byte (`ESC O <final>`) when it is a numeric keypad key.
struct KeypadKey {
    main: &'static str,
    kitty_code: u32,
    application: Option<u8>,
}

fn keypad_key(key: &str) -> Option<KeypadKey> {
    let (main, kitty_code, application) = match key {
        "KP_0" => ("0", 57399, Some(b'p')),
        "KP_1" => ("1", 57400, Some(b'q')),
        "KP_2" => ("2", 57401, Some(b'r')),
        "KP_3" => ("3", 57402, Some(b's')),
        "KP_4" => ("4", 57403, Some(b't')),
        "KP_5" => ("5", 57404, Some(b'u')),
        "KP_6" => ("6", 57405, Some(b'v')),
        "KP_7" => ("7", 57406, Some(b'w')),
        "KP_8" => ("8", 57407, Some(b'x')),
        "KP_9" => ("9", 57408, Some(b'y')),
        "KP_DECIMAL" => (".", 57409, Some(b'n')),
        "KP_DIVIDE" => ("/", 57410, Some(b'o')),
        "KP_MULTIPLY" => ("*", 57411, Some(b'j')),
        "KP_SUBTRACT" => ("-", 57412, Some(b'm')),
        "KP_ADD" => ("+", 57413, Some(b'k')),
        "KP_ENTER" => ("RETURN", 57414, Some(b'M')),
        "KP_EQUAL" => ("=", 57415, Some(b'X')),
        "KP_SEPARATOR" => (",", 57416, Some(b'l')),
        "KP_LEFT" => ("LEFT", 57417, None),
        "KP_RIGHT" => ("RIGHT", 57418, None),
        "KP_UP" => ("UP", 57419, None),
        "KP_DOWN" => ("DOWN", 57420, None),
        "KP_PAGE_UP" => ("PAGE_UP", 57421, None),
        "KP_PAGE_DOWN" => ("PAGE_DOWN", 57422, None),
        "KP_HOME" => ("HOME", 57423, None),
        "KP_END" => ("END", 57424, None),
        "KP_INSERT" => ("INSERT", 57425, None),
        "KP_DELETE" => ("DELETE", 57426, None),
        _ => return None,
    };
    Some(KeypadKey {
        main,
        kitty_code,
        application,
    })
}

/// Keypad keys are their own keys (KP_Enter is not Return). In legacy
/// encoding the terminal sends the bytes of the matching main key, or
/// `ESC O <final>` in application keypad mode. The Kitty disambiguate flag
/// reports keys that type no text with their own code; text keys stay text
/// until report-all-keys (flag 8) or a modifier makes them escape codes.
fn keypad_encoding(key: &KeyCombo, protocol_flags: Option<u32>) -> Option<KeyEncoding> {
    let keypad = keypad_key(key.key())?;
    let mask = key.modmask();
    let types_text = keypad.main.chars().count() == 1;
    if let Some(flags) = protocol_flags.filter(|flags| flags & (1 | 8) != 0) {
        if types_text && mask == 0 && flags & 8 == 0 {
            return Some(KeyEncoding::Bytes(keypad.main.as_bytes().to_vec()));
        }
        let modifier = key_modifier_parameter(mask);
        let code = keypad.kitty_code;
        return Some(KeyEncoding::Bytes(if modifier == 1 {
            format!("\x1b[{code}u").into_bytes()
        } else {
            format!("\x1b[{code};{modifier}u").into_bytes()
        }));
    }
    let normal = modifier_encoding(&KeyCombo::from_parts(mask, keypad.main), None);
    Some(match (normal, keypad.application) {
        (Some(normal), Some(application)) if mask == 0 => KeyEncoding::Keypad {
            normal,
            application: vec![0x1b, b'O', application],
        },
        (Some(normal), _) => KeyEncoding::Bytes(normal),
        (None, _) => KeyEncoding::Unmodeled,
    })
}

fn kitty_encoding(key: &KeyCombo) -> Option<Vec<u8>> {
    // Kitty reports the unshifted key: letters are lowercase codepoints.
    let codepoint = if key.key().chars().count() == 1 {
        key.key().chars().next()?.to_ascii_lowercase() as u32
    } else {
        match key.key() {
            "TAB" => 9,
            "RETURN" => 13,
            "ESCAPE" => 27,
            "SPACE" => 32,
            _ => return None,
        }
    };
    let modifier = key_modifier_parameter(key.modmask());
    Some(format!("\x1b[{codepoint};{modifier}u").into_bytes())
}

fn default_non_arrow_encoding(key: &str, mask: u32) -> Option<Vec<u8>> {
    if mask & 4 != 0 && mask & !(4 | 8) == 0 {
        if let Some(byte) = crate::layers::terminal::legacy_control_byte(key) {
            // Alt prefixes the control byte with ESC in legacy encoding.
            return Some(if mask & 8 != 0 {
                vec![0x1b, byte]
            } else {
                vec![byte]
            });
        }
    }
    // `KeyCombo` names letters in uppercase; a letter types lowercase unless
    // Shift is held.
    if key.chars().count() == 1 && mask & !(1 | 8) == 0 {
        let shifted = mask & 1 != 0;
        if shifted && !key.chars().all(|character| character.is_ascii_alphabetic()) {
            return None;
        }
        let text = if shifted {
            key.to_ascii_uppercase()
        } else {
            key.to_ascii_lowercase()
        };
        let mut bytes = if mask & 8 != 0 {
            vec![0x1b]
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }
    // Alt prefixes Escape with ESC in legacy encoding (xterm, tmux, VTE).
    if key == "ESCAPE" && mask == 8 {
        return Some(vec![0x1b, 0x1b]);
    }
    if mask != 0 {
        return modified_navigation_encoding(key, mask);
    }
    match key {
        "RETURN" => Some(vec![b'\r']),
        "TAB" => Some(vec![b'\t']),
        "BACKSPACE" => Some(vec![0x7f]),
        "ESCAPE" => Some(vec![0x1b]),
        "HOME" => Some(b"\x1b[H".to_vec()),
        "END" => Some(b"\x1b[F".to_vec()),
        "INSERT" => Some(b"\x1b[2~".to_vec()),
        "DELETE" => Some(b"\x1b[3~".to_vec()),
        "PAGE_UP" => Some(b"\x1b[5~".to_vec()),
        "PAGE_DOWN" => Some(b"\x1b[6~".to_vec()),
        value if value.starts_with('F') => function_key_encoding(value, 0),
        _ => None,
    }
}

fn modified_navigation_encoding(key: &str, mask: u32) -> Option<Vec<u8>> {
    let modifier = key_modifier_parameter(mask);
    match key {
        "HOME" => Some(format!("\x1b[1;{modifier}H").into_bytes()),
        "END" => Some(format!("\x1b[1;{modifier}F").into_bytes()),
        "INSERT" => Some(format!("\x1b[2;{modifier}~").into_bytes()),
        "DELETE" => Some(format!("\x1b[3;{modifier}~").into_bytes()),
        "PAGE_UP" => Some(format!("\x1b[5;{modifier}~").into_bytes()),
        "PAGE_DOWN" => Some(format!("\x1b[6;{modifier}~").into_bytes()),
        "TAB" if mask == 1 => Some(b"\x1b[Z".to_vec()),
        value if value.starts_with('F') => function_key_encoding(value, modifier),
        _ => None,
    }
}

fn function_key_encoding(key: &str, modifier: u32) -> Option<Vec<u8>> {
    let number = key.strip_prefix('F')?.parse::<u8>().ok()?;
    if !(1..=35).contains(&number) {
        return None;
    }
    if modifier == 0 {
        return match number {
            1 => Some(b"\x1bOP".to_vec()),
            2 => Some(b"\x1bOQ".to_vec()),
            3 => Some(b"\x1bOR".to_vec()),
            4 => Some(b"\x1bOS".to_vec()),
            5 => Some(b"\x1b[15~".to_vec()),
            6 => Some(b"\x1b[17~".to_vec()),
            7 => Some(b"\x1b[18~".to_vec()),
            8 => Some(b"\x1b[19~".to_vec()),
            9 => Some(b"\x1b[20~".to_vec()),
            10 => Some(b"\x1b[21~".to_vec()),
            11 => Some(b"\x1b[23~".to_vec()),
            12 => Some(b"\x1b[24~".to_vec()),
            13 => Some(b"\x1b[25~".to_vec()),
            14 => Some(b"\x1b[26~".to_vec()),
            15 => Some(b"\x1b[28~".to_vec()),
            16 => Some(b"\x1b[29~".to_vec()),
            17 => Some(b"\x1b[31~".to_vec()),
            18 => Some(b"\x1b[32~".to_vec()),
            19 => Some(b"\x1b[33~".to_vec()),
            20 => Some(b"\x1b[34~".to_vec()),
            21 => Some(b"\x1b[35~".to_vec()),
            22 => Some(b"\x1b[36~".to_vec()),
            23 => Some(b"\x1b[37~".to_vec()),
            24 => Some(b"\x1b[38~".to_vec()),
            25 => Some(b"\x1b[39~".to_vec()),
            26 => Some(b"\x1b[40~".to_vec()),
            27 => Some(b"\x1b[41~".to_vec()),
            28 => Some(b"\x1b[42~".to_vec()),
            29 => Some(b"\x1b[43~".to_vec()),
            30 => Some(b"\x1b[44~".to_vec()),
            31 => Some(b"\x1b[45~".to_vec()),
            32 => Some(b"\x1b[46~".to_vec()),
            33 => Some(b"\x1b[47~".to_vec()),
            34 => Some(b"\x1b[48~".to_vec()),
            35 => Some(b"\x1b[49~".to_vec()),
            _ => None,
        };
    }
    let code = match number {
        1..=4 => {
            return Some(
                format!("\x1b[1;{modifier}{}", char::from(b'P' + number - 1)).into_bytes(),
            );
        }
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        value => 24 + u32::from(value - 12),
    };
    Some(format!("\x1b[{code};{modifier}~").into_bytes())
}

/// Kitty/xterm modifier parameter. Mod5 (AltGr) is absent on purpose: it is
/// a level shift resolved by [`altgr_encoding`], not the Kitty Meta bit.
fn key_modifier_parameter(mask: u32) -> u32 {
    let mut parameter = 1;
    if mask & 1 != 0 {
        parameter += 1;
    }
    if mask & 4 != 0 {
        parameter += 4;
    }
    if mask & 8 != 0 {
        parameter += 2;
    }
    if mask & 64 != 0 {
        parameter += 8;
    }
    if mask & 32 != 0 {
        parameter += 16;
    }
    if mask & 2 != 0 {
        parameter += 64;
    }
    if mask & 16 != 0 {
        parameter += 128;
    }
    parameter
}

fn format_encoding_detail(bytes: &[u8]) -> String {
    if bytes.len() == 1 && bytes[0] < 0x20 {
        return format!("byte: 0x{:02x}", bytes[0]);
    }
    let sequence = bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "ESC".to_owned(),
            b' ' => "SPACE".to_owned(),
            byte if byte.is_ascii_graphic() => (*byte as char).to_string(),
            byte => format!("0x{byte:02x}"),
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("sequence: {sequence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{LayerStatus, Propagation};

    fn inspect(key: &str, config: &str) -> LayerResult {
        let key: KeyCombo = key.parse().unwrap();
        let mut state = ConfigState::default();
        for line in config.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("keybind =") {
                parse_keybind(value.trim(), &mut state).unwrap();
            }
        }
        inspect_config(&key, Path::new("/tmp/ghostty/config"), &state, None)
    }

    fn default_encoding(key: &KeyCombo) -> Option<Vec<u8>> {
        default_encoding_for_protocol(key, None, None).into_bytes()
    }

    #[test]
    fn forwards_ctrl_left_with_the_default_sequence() {
        let result = inspect("ctrl+left", "");

        assert_eq!(result.status(), LayerStatus::NotHandled);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "sequence: ESC [ 1 ; 5 D")
        );
    }

    #[test]
    fn forwards_ctrl_z_as_a_control_byte() {
        let result = inspect("ctrl+z", "");

        assert!(result.details.iter().any(|detail| detail == "byte: 0x1a"));
    }

    #[test]
    fn encodes_common_navigation_and_function_keys() {
        assert_eq!(
            default_encoding(&"insert".parse().unwrap()),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            default_encoding(&"f5".parse().unwrap()),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            default_encoding(&"ctrl+f5".parse().unwrap()),
            Some(b"\x1b[15;5~".to_vec())
        );
        assert_eq!(
            default_encoding(&"shift+tab".parse().unwrap()),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn parses_a_csi_binding_as_terminal_output() {
        let result = inspect("ctrl+left", "keybind = ctrl+left=csi:1;5D");

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(result.summary.contains("sends a sequence"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "sequence: ESC [ 1 ; 5 D")
        );
    }

    #[test]
    fn reports_consuming_actions() {
        let result = inspect("ctrl+left", "keybind = ctrl+left=new_tab");

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Stops);
    }

    #[test]
    fn respects_unconsumed_prefix() {
        let result = inspect(
            "ctrl+left",
            "keybind = unconsumed:ctrl+left=copy_to_clipboard",
        );

        assert_eq!(result.status(), LayerStatus::Handled);
        assert_eq!(result.propagation(), Propagation::Continues);
    }

    #[test]
    fn clear_and_unbind_change_the_effective_binding() {
        let result = inspect(
            "ctrl+left",
            "keybind = ctrl+left=new_tab\nkeybind = clear\nkeybind = ctrl+right=unbind",
        );

        assert_eq!(result.status(), LayerStatus::NotHandled);
    }

    #[test]
    fn later_bindings_override_earlier_bindings() {
        let result = inspect(
            "ctrl+left",
            "keybind = ctrl+left=new_tab\nkeybind = ctrl+left=csi:1;5D",
        );

        assert_eq!(result.propagation(), Propagation::Continues);
        assert!(result.summary.contains("sends a sequence"));
    }

    #[test]
    fn reconstructs_legacy_input_for_ctrl_f() {
        let ghostty = Ghostty {
            config_path: PathBuf::from("/tmp/whykey-missing-config"),
            use_cli: false,
        };
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let input = ghostty.normal_input_with_protocol(&key, Some(0));

        assert_eq!(
            input.as_ref().map(|input| input.bytes.as_slice()),
            Some(&[0x06][..])
        );
    }

    #[test]
    fn list_keybinds_is_live_files_are_static_and_nothing_read_has_no_tier() {
        let cli = parse_cli_keybinds("keybind = ctrl+t=new_tab\n").unwrap();
        assert_eq!(cli.tier(), Some(EvidenceTier::LiveQuery));
        let file = ConfigState {
            loaded_files: vec![PathBuf::from("/home/u/.config/ghostty/config")],
            ..ConfigState::default()
        };
        assert_eq!(file.tier(), Some(EvidenceTier::ConfigFile));
        assert_eq!(ConfigState::default().tier(), None);

        let missing = Ghostty {
            config_path: PathBuf::from("/tmp/whykey-missing-config"),
            use_cli: false,
        };
        let (layer, _) = missing.analyze(&"ctrl+t".parse().unwrap(), None, None);
        assert_eq!(layer.tier, None);
    }

    #[test]
    fn respects_an_existing_kitty_keyboard_protocol() {
        let ghostty = Ghostty {
            config_path: PathBuf::from("/tmp/whykey-missing-config"),
            use_cli: false,
        };
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let input = ghostty.normal_input_with_protocol(&key, Some(1));

        assert_eq!(
            input.as_ref().map(|input| input.bytes.as_slice()),
            Some(&b"\x1b[102;5u"[..])
        );
    }

    #[test]
    fn encodes_ctrl_punctuation_and_space_as_control_bytes() {
        for (key, bytes) in [
            ("ctrl+bracketright", vec![0x1d]),
            ("ctrl+bracketleft", vec![0x1b]),
            ("ctrl+backslash", vec![0x1c]),
            ("ctrl+asciicircum", vec![0x1e]),
            ("ctrl+underscore", vec![0x1f]),
            ("ctrl+at", vec![0x00]),
            ("ctrl+space", vec![0x00]),
            ("ctrl+alt+x", vec![0x1b, 0x18]),
        ] {
            let combo: KeyCombo = key.parse().unwrap();
            assert_eq!(default_encoding(&combo), Some(bytes), "{key}");
        }
    }

    #[test]
    fn encodes_unicode_text_as_utf8_without_kitty_protocol() {
        let key: KeyCombo = "é".parse().unwrap();
        assert_eq!(
            default_encoding_for_protocol(&key, Some(0), None),
            KeyEncoding::Bytes("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn encodes_keypad_keys_as_their_own_keys() {
        let encode = |key: &str, flags: Option<u32>| {
            default_encoding_for_protocol(&key.parse().unwrap(), flags, None)
        };
        let keypad = |normal: &[u8], application: &[u8]| KeyEncoding::Keypad {
            normal: normal.to_vec(),
            application: application.to_vec(),
        };
        // Legacy: the main key's bytes, ESC O x in application keypad mode.
        assert_eq!(encode("kp_enter", None), keypad(b"\r", b"\x1bOM"));
        assert_eq!(encode("kp_1", None), keypad(b"1", b"\x1bOq"));
        assert_eq!(encode("kp_add", None), keypad(b"+", b"\x1bOk"));
        assert_eq!(encode("kp_divide", None), keypad(b"/", b"\x1bOo"));
        assert_eq!(encode("kp_equal", None), keypad(b"=", b"\x1bOX"));
        assert_eq!(
            encode("kp_home", None),
            KeyEncoding::Bytes(b"\x1b[H".to_vec())
        );
        assert_eq!(
            encode("ctrl+kp_prior", None),
            KeyEncoding::Bytes(b"\x1b[5;5~".to_vec())
        );
        // Kitty disambiguate: keys without text get their own code; text
        // keys stay text unless modified or under report-all-keys.
        assert_eq!(
            encode("kp_enter", Some(1)),
            KeyEncoding::Bytes(b"\x1b[57414u".to_vec())
        );
        assert_eq!(
            encode("kp_home", Some(1)),
            KeyEncoding::Bytes(b"\x1b[57423u".to_vec())
        );
        assert_eq!(encode("kp_1", Some(1)), KeyEncoding::Bytes(b"1".to_vec()));
        assert_eq!(
            encode("kp_1", Some(1 | 8)),
            KeyEncoding::Bytes(b"\x1b[57400u".to_vec())
        );
        assert_eq!(
            encode("ctrl+kp_enter", Some(1)),
            KeyEncoding::Bytes(b"\x1b[57414;5u".to_vec())
        );
        // Return itself is unchanged.
        assert_eq!(encode("return", None), KeyEncoding::Bytes(b"\r".to_vec()));

        let (layer, input) = with_default_encoding(
            LayerResult::pass("Terminal", LayerId::Terminal, "forwarded", Vec::new()),
            encode("kp_enter", None),
            "default encoding",
            true,
        );
        assert_eq!(input.unwrap().bytes, b"\r");
        assert!(
            layer
                .details
                .iter()
                .any(|detail| detail.contains("application keypad mode")
                    && detail.contains("ESC O M")),
            "{:?}",
            layer.details
        );
    }

    #[test]
    fn encodes_extended_modifier_bits_for_kitty() {
        let key: KeyCombo = "caps+num+hyper+x".parse().unwrap();
        assert_eq!(key_modifier_parameter(key.modmask()), 1 + 64 + 128 + 16);
    }

    #[test]
    fn encodes_alt_escape_as_escape_prefixed_escape() {
        let key: KeyCombo = "alt+escape".parse().unwrap();
        assert_eq!(default_encoding(&key), Some(vec![0x1b, 0x1b]));
        assert_eq!(
            default_encoding_for_protocol(&key, Some(1), None).into_bytes(),
            Some(b"\x1b[27;3u".to_vec())
        );
    }

    const ALTGR_KEYMAP: &str = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AE02> = 11;
    <AD01> = 24;
    <RTRN> = 36;
};
xkb_symbols "pc" {
    key <AE02> { [ 2, at, twosuperior, dead_doubleacute ] };
    key <AD01> { [ q, Q, adiaeresis, Adiaeresis ] };
    key <RTRN> { [ Return ] };
};
};
"#;

    #[test]
    fn altgr_sends_level_three_text_instead_of_a_meta_modifier() {
        let layout = KeyboardLayout {
            keymap: ALTGR_KEYMAP.into(),
            group: 0,
            physical_keycode: None,
        };
        let encode = |key: &str, flags: Option<u32>, layout: Option<&KeyboardLayout>| {
            default_encoding_for_protocol(&key.parse().unwrap(), flags, layout)
        };
        let text = |value: &str| KeyEncoding::Bytes(value.as_bytes().to_vec());
        assert_eq!(encode("altgr+q", None, Some(&layout)), text("ä"));
        assert_eq!(encode("altgr+q", Some(1), Some(&layout)), text("ä"));
        assert_eq!(encode("altgr+shift+q", None, Some(&layout)), text("Ä"));
        assert_eq!(
            encode("altgr+shift+2", None, Some(&layout)),
            KeyEncoding::DeadKey("dead_doubleacute".into())
        );
        // No level 3, no keymap, or an unmodeled protocol: honest unknowns.
        for (key, flags, layout) in [
            ("altgr+return", None, Some(&layout)),
            ("altgr+q", None, None),
            ("altgr+q", Some(8), Some(&layout)),
            ("altgr+ctrl+q", None, Some(&layout)),
        ] {
            assert!(
                matches!(encode(key, flags, layout), KeyEncoding::Unknown(_)),
                "{key} {flags:?}"
            );
        }
        // A dead key is consumed by the terminal's composition.
        let pass = LayerResult::new("Ghostty", LayerId::Terminal, Outcome::Pass, "", vec![]);
        let (layer, input) = with_default_encoding(
            pass,
            KeyEncoding::DeadKey("dead_doubleacute".into()),
            "Ghostty default encoding",
            true,
        );
        assert_eq!(layer.outcome, Outcome::Consumed);
        assert_eq!(layer.dead_key.as_deref(), Some("dead_doubleacute"));
        assert!(input.is_none());
    }

    #[test]
    fn generic_terminal_fallback_resolves_altgr_level_three() {
        let layout = KeyboardLayout {
            keymap: ALTGR_KEYMAP.into(),
            group: 0,
            physical_keycode: None,
        };
        let (layer, input) =
            generic_fallback("tern", &"altgr+q".parse().unwrap(), None, Some(&layout));
        assert_eq!(layer.outcome, Outcome::Pass);
        assert_eq!(input.expect("level-3 text bytes").bytes, "ä".as_bytes());

        let (layer, input) = generic_fallback(
            "tern",
            &"altgr+shift+2".parse().unwrap(),
            None,
            Some(&layout),
        );
        assert_eq!(layer.outcome, Outcome::Consumed);
        assert_eq!(layer.layer, "Terminal");
        assert!(input.is_none(), "a dead key sends no bytes");
    }

    #[test]
    fn parses_effective_ghostty_keybinds() {
        let config = parse_cli_keybinds(
            "keybind = ctrl+shift+c=copy_to_clipboard:mixed\nkeybind = ctrl+z=close_surface\nkeybind = ctrl+==increase_font_size:1\n",
        )
        .unwrap();

        assert_eq!(config.bindings.len(), 3);
        assert_eq!(config.bindings[0].trigger.to_string(), "CTRL + SHIFT + C");
        assert_eq!(config.bindings[1].action, "close_surface");
        assert_eq!(config.bindings[2].trigger.to_string(), "CTRL + =");
        assert_eq!(
            config.source.as_deref(),
            Some("ghostty +list-keybinds --plain")
        );
    }

    #[test]
    fn includes_are_applied_after_the_parent_file() {
        let directory =
            std::env::temp_dir().join(format!("whykey-ghostty-include-{}", std::process::id()));
        let parent = directory.join("parent.conf");
        let child = directory.join("child.conf");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            &parent,
            "config-file = child.conf\nkeybind = ctrl+left=csi:A\n",
        )
        .unwrap();
        std::fs::write(&child, "keybind = ctrl+left=csi:B\n").unwrap();

        let config = load_config_file(&parent).unwrap();
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_config(&key, &parent, &config, None);

        assert!(result.details.iter().any(|detail| detail.contains("csi:B")));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn reports_unresolved_physical_triggers() {
        let result = inspect(
            "ctrl+left",
            "keybind = physical:ctrl+left=copy_to_clipboard\nkeybind = ctrl+left=unbind",
        );
        assert!(result.details.iter().any(|detail| {
            detail.contains("trigger(s) not resolved") && detail.contains("physical:ctrl+left")
        }));
    }

    #[test]
    fn treats_matching_physical_trigger_as_uncertain() {
        let config = parse_cli_keybinds(
            "keybind = physical:ctrl+left=copy_to_clipboard\nkeybind = ctrl+left=unbind",
        )
        .unwrap();
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        let result = inspect_config(&key, Path::new("/tmp/config"), &config, None);
        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.summary.contains("physical binding"));
    }
}
