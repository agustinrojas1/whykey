use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, TerminalInput};

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
        self.analyze(key, None).0
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
    /// prediction, never the binding layer itself.
    pub fn analyze(
        &self,
        key: &KeyCombo,
        protocol_flags: Option<u32>,
    ) -> (LayerResult, Option<TerminalInput>) {
        if let Some(program) = known_non_ghostty_terminal() {
            let detail = if program == "terminal identity unknown" {
                "terminal identity is unavailable; Ghostty bindings were skipped".into()
            } else {
                format!("TERM_PROGRAM={program}; Ghostty bindings were skipped")
            };
            let layer = LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::Pass,
                summary: "not applicable to the current terminal".into(),
                details: vec![detail],
            };
            let input = default_encoding_for_protocol(key, protocol_flags).map(|bytes| {
                TerminalInput::predicted(
                    bytes,
                    if program == "terminal identity unknown" {
                        "generic terminal fallback; terminal identity is unknown".into()
                    } else {
                        format!("generic terminal fallback; {program} is not Ghostty")
                    },
                )
            });
            return (layer, input);
        }
        let config = match self.load_config() {
            Ok(config) => config,
            Err(message) => {
                return (
                    LayerResult {
                        verbose_details: Vec::new(),
                        binding: None,
                        layer: "Ghostty",
                        id: LayerId::Terminal,
                        outcome: Outcome::Unavailable,
                        summary: "could not inspect Ghostty configuration".into(),
                        details: vec![message],
                    },
                    None,
                );
            }
        };
        let layer = inspect_config(key, &self.config_path, &config);
        let has_binding = config
            .bindings
            .iter()
            .any(|binding| &binding.trigger == key);
        let input = Self::input_from_config(&config, key, protocol_flags).or_else(|| {
            if has_binding {
                None
            } else {
                default_encoding_for_protocol(key, protocol_flags)
                    .map(|bytes| TerminalInput::predicted(bytes, "Ghostty default encoding"))
            }
        });
        (layer, input)
    }

    fn input_from_config(
        config: &ConfigState,
        key: &KeyCombo,
        protocol_flags: Option<u32>,
    ) -> Option<TerminalInput> {
        let binding = config
            .bindings
            .iter()
            .find(|binding| &binding.trigger == key)?;
        if binding.global || binding.all || binding.action == "ignore" {
            return None;
        }
        if binding.performable {
            return default_encoding_for_protocol(key, protocol_flags).map(|bytes| {
                TerminalInput::predicted(
                    bytes,
                    "default encoding when the action is not performable",
                )
            });
        }
        if let Some(bytes) = action_encoding(&binding.action) {
            if binding.unconsumed {
                let mut combined = bytes.clone();
                if let Some(default) = default_encoding_for_protocol(key, protocol_flags) {
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
            return default_encoding_for_protocol(key, protocol_flags).map(|bytes| {
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
        self.analyze(key, protocol_flags).1
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

fn known_non_ghostty_terminal() -> Option<String> {
    let Some(program) = env::var("TERM_PROGRAM")
        .ok()
        .or_else(|| env::var("LC_TERMINAL").ok())
        .or_else(|| {
            let term = env::var("TERM").ok()?;
            let normalized = term.to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "xterm-kitty" | "foot" | "foot-direct" | "xterm-ghostty"
            )
            .then_some(term)
        })
    else {
        return Some("terminal identity unknown".into());
    };
    let normalized = program.to_ascii_lowercase();
    if matches!(normalized.as_str(), "ghostty" | "xterm-ghostty") {
        return None;
    }
    Some(program)
}

fn inspect_config(key: &KeyCombo, config_path: &Path, config: &ConfigState) -> LayerResult {
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
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::Unknown,
                summary: "possible physical binding may capture the key".into(),
                details,
            };
        }
        if let Some(encoding) = default_encoding(key) {
            details.push(format_encoding_detail(&encoding));
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::Pass,
                summary: "no Ghostty binding; forwarded to terminal".into(),
                details,
            };
        }

        details.push("default key encoding is layout-dependent or unsupported".into());
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Ghostty",
            id: LayerId::Terminal,
            outcome: Outcome::Pass,
            summary: "no Ghostty binding; forwarded to terminal".into(),
            details,
        };
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
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Ghostty",
                id: LayerId::Terminal,
                outcome: Outcome::HandledAndPassed,
                summary: "binding sends a sequence and leaves it unconsumed".into(),
                details,
            };
        }
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Ghostty",
            id: LayerId::Terminal,
            outcome: Outcome::HandledAndPassed,
            summary: "binding sends a sequence to the terminal".into(),
            details,
        };
    }

    if binding.action == "ignore" {
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Ghostty",
            id: LayerId::Terminal,
            outcome: Outcome::Consumed,
            summary: "binding ignores the key input".into(),
            details,
        };
    }

    if binding.performable {
        details.push("action may pass through when it is not performable".into());
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Ghostty",
            id: LayerId::Terminal,
            outcome: Outcome::HandledUncertain,
            summary: "binding may consume the key depending on terminal state".into(),
            details,
        };
    }

    if binding.unconsumed {
        return LayerResult {
            verbose_details: Vec::new(),
            binding: None,
            layer: "Ghostty",
            id: LayerId::Terminal,
            outcome: Outcome::HandledAndPassed,
            summary: "binding runs and leaves the key unconsumed".into(),
            details,
        };
    }

    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "Ghostty",
        id: LayerId::Terminal,
        outcome: Outcome::Consumed,
        summary: "binding consumes the key".into(),
        details,
    }
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

fn default_encoding(key: &KeyCombo) -> Option<Vec<u8>> {
    default_encoding_for_protocol(key, None)
}

fn default_encoding_for_protocol(key: &KeyCombo, protocol_flags: Option<u32>) -> Option<Vec<u8>> {
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

fn kitty_encoding(key: &KeyCombo) -> Option<Vec<u8>> {
    let codepoint = if key.key().chars().count() == 1 {
        key.key().chars().next()? as u32
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
    if key.len() == 1 && mask & 4 != 0 && mask & !(4 | 8) == 0 {
        let character = key.as_bytes()[0].to_ascii_uppercase();
        if character.is_ascii_alphabetic() {
            return Some(vec![character & 0x1f]);
        }
        if character == b' ' {
            return Some(vec![0]);
        }
    }
    if mask == 0 && key.chars().count() == 1 {
        return Some(key.as_bytes().to_vec());
    }
    if mask == 8 && key.chars().count() == 1 {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(key.as_bytes());
        return Some(bytes);
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
    if mask & 128 != 0 {
        parameter += 32;
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
        inspect_config(&key, Path::new("/tmp/ghostty/config"), &state)
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
    fn respects_an_existing_kitty_keyboard_protocol() {
        let ghostty = Ghostty {
            config_path: PathBuf::from("/tmp/whykey-missing-config"),
            use_cli: false,
        };
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let input = ghostty.normal_input_with_protocol(&key, Some(1));

        assert_eq!(
            input.as_ref().map(|input| input.bytes.as_slice()),
            Some(&b"\x1b[70;5u"[..])
        );
    }

    #[test]
    fn encodes_unicode_text_as_utf8_without_kitty_protocol() {
        let key: KeyCombo = "é".parse().unwrap();
        assert_eq!(
            default_encoding_for_protocol(&key, Some(0)),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn encodes_extended_modifier_bits_for_kitty() {
        let key: KeyCombo = "caps+num+hyper+meta-key+x".parse().unwrap();
        assert_eq!(
            key_modifier_parameter(key.modmask()),
            1 + 64 + 128 + 16 + 32
        );
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
        let result = inspect_config(&key, &parent, &config);

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
        let result = inspect_config(&key, Path::new("/tmp/config"), &config);
        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert_eq!(result.propagation(), Propagation::Indeterminate);
        assert!(result.summary.contains("physical binding"));
    }
}
