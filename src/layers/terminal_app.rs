use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, TerminalInput, format_bytes};
use crate::util::unquote;

#[derive(Debug, Clone)]
struct Binding {
    trigger: KeyCombo,
    action: String,
    output: Option<Vec<u8>>,
    description: String,
}

pub fn inspect(key: &KeyCombo) -> LayerResult {
    let Some(kind) = terminal_kind() else {
        return not_applicable("terminal identity is not known");
    };
    if kind == TerminalKind::WezTerm {
        let (bindings, source) = match load_wezterm_bindings() {
            Ok(value) => value,
            Err(error) => {
                return LayerResult {
                    binding: None,
                    layer: kind.layer_name(),
                    id: LayerId::Terminal,
                    outcome: Outcome::Unknown,
                    summary: "could not inspect WezTerm effective key assignments".into(),
                    details: vec![error],
                };
            }
        };
        return inspect_bindings(kind, key, &bindings, &source);
    }
    if kind == TerminalKind::Konsole {
        let (bindings, source) = match load_konsole_bindings() {
            Ok(value) => value,
            Err(error) => {
                return LayerResult {
                    binding: None,
                    layer: kind.layer_name(),
                    id: LayerId::Terminal,
                    outcome: Outcome::Unknown,
                    summary: "could not inspect Konsole keytab bindings".into(),
                    details: vec![error],
                };
            }
        };
        return inspect_bindings(kind, key, &bindings, &source);
    }
    let Some(path) = config_path(kind) else {
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::Unknown,
            summary: format!("{kind} is active but no config file was found"),
            details: vec!["set the terminal's config path or use its defaults".into()],
        };
    };
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            return LayerResult {
                binding: None,
                layer: kind.layer_name(),
                id: LayerId::Terminal,
                outcome: Outcome::Unknown,
                summary: format!("could not read {kind} configuration"),
                details: vec![format!("config: {}: {error}", path.display())],
            };
        }
    };
    let bindings = parse_bindings(kind, &content);
    inspect_bindings(kind, key, &bindings, &format!("config: {}", path.display()))
}

fn inspect_bindings(
    kind: TerminalKind,
    key: &KeyCombo,
    bindings: &[Binding],
    source: &str,
) -> LayerResult {
    let Some(binding) = bindings
        .iter()
        .rev()
        .find(|binding| &binding.trigger == key)
    else {
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::Pass,
            summary: format!("no {kind} binding; forwarded to the terminal input"),
            details: vec![source.into()],
        };
    };

    let mut details = vec![source.into(), format!("binding: {}", binding.description)];
    if kind == TerminalKind::WezTerm
        && wezterm_binding_table(&binding.description)
            .is_some_and(|table| !table.eq_ignore_ascii_case("default"))
    {
        details.push(
            "this binding belongs to a non-default WezTerm key table; its active state is runtime-dependent".into(),
        );
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::Unknown,
            summary: format!("{kind} binding found in a conditional key table"),
            details,
        };
    }
    if let Some(output) = &binding.output {
        details.push(format!("sequence: {}", format_bytes(output)));
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::HandledAndPassed,
            summary: format!("{kind} sends a sequence to the PTY"),
            details,
        };
    }
    if action_forwards(&binding.action) {
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::HandledAndPassed,
            summary: format!("{kind} forwards the key to the PTY"),
            details,
        };
    }
    if action_is_consuming(&binding.action) {
        return LayerResult {
            binding: None,
            layer: kind.layer_name(),
            id: LayerId::Terminal,
            outcome: Outcome::Consumed,
            summary: format!("{kind} consumes the key"),
            details,
        };
    }
    details.push("action semantics are terminal-specific".into());
    LayerResult {
        binding: None,
        layer: kind.layer_name(),
        id: LayerId::Terminal,
        outcome: Outcome::Unknown,
        summary: format!("{kind} binding found; propagation is unknown"),
        details,
    }
}

/// Analyze once: route finding plus normal input bytes from a single config
/// load. Reused for the visible terminal layer and downstream TTY/shell
/// analysis so the file is read once per static inspection.
pub fn analyze(key: &KeyCombo) -> (LayerResult, Option<TerminalInput>) {
    let Some(kind) = terminal_kind() else {
        return (not_applicable("terminal identity is not known"), None);
    };
    if kind == TerminalKind::WezTerm {
        let (bindings, source) = match load_wezterm_bindings() {
            Ok(value) => value,
            Err(error) => {
                return (
                    LayerResult {
                        binding: None,
                        layer: kind.layer_name(),
                        id: LayerId::Terminal,
                        outcome: Outcome::Unknown,
                        summary: "could not inspect WezTerm effective key assignments".into(),
                        details: vec![error],
                    },
                    None,
                );
            }
        };
        let layer = inspect_bindings(kind, key, &bindings, &source);
        let input = bindings
            .into_iter()
            .rev()
            .find(|binding| {
                &binding.trigger == key
                    && wezterm_binding_table(&binding.description)
                        .is_none_or(|table| table.eq_ignore_ascii_case("default"))
            })
            .and_then(|binding| binding.output)
            .map(|bytes| TerminalInput::configured(bytes, "explicit WezTerm SendString output"));
        return (layer, input);
    }
    if kind == TerminalKind::Konsole {
        let (bindings, source) = match load_konsole_bindings() {
            Ok(value) => value,
            Err(error) => {
                return (
                    LayerResult {
                        binding: None,
                        layer: kind.layer_name(),
                        id: LayerId::Terminal,
                        outcome: Outcome::Unknown,
                        summary: "could not inspect Konsole keytab bindings".into(),
                        details: vec![error],
                    },
                    None,
                );
            }
        };
        let layer = inspect_bindings(kind, key, &bindings, &source);
        let input = bindings
            .into_iter()
            .rev()
            .find(|binding| &binding.trigger == key)
            .and_then(|binding| binding.output)
            .map(|bytes| TerminalInput::configured(bytes, "explicit Konsole keytab output"));
        return (layer, input);
    }
    let Some(path) = config_path(kind) else {
        return (
            LayerResult {
                binding: None,
                layer: kind.layer_name(),
                id: LayerId::Terminal,
                outcome: Outcome::Unknown,
                summary: format!("{kind} is active but no config file was found"),
                details: vec!["set the terminal's config path or use its defaults".into()],
            },
            None,
        );
    };
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            return (
                LayerResult {
                    binding: None,
                    layer: kind.layer_name(),
                    id: LayerId::Terminal,
                    outcome: Outcome::Unknown,
                    summary: format!("could not read {kind} configuration"),
                    details: vec![format!("config: {}: {error}", path.display())],
                },
                None,
            );
        }
    };
    let bindings = parse_bindings(kind, &content);
    let layer = inspect_bindings(kind, key, &bindings, &format!("config: {}", path.display()));
    let input = bindings
        .into_iter()
        .rev()
        .find(|binding| &binding.trigger == key)
        .and_then(|binding| binding.output)
        .map(|bytes| TerminalInput::configured(bytes, format!("explicit {kind} terminal output")));
    (layer, input)
}

pub fn normal_input(key: &KeyCombo) -> Option<TerminalInput> {
    analyze(key).1
}

pub fn applicable() -> bool {
    terminal_kind().is_some()
}

pub fn detected_name() -> Option<&'static str> {
    terminal_kind().map(TerminalKind::layer_name)
}

fn not_applicable(message: &str) -> LayerResult {
    LayerResult {
        binding: None,
        layer: "Terminal adapter",
        id: LayerId::Terminal,
        outcome: Outcome::Pass,
        summary: message.into(),
        details: vec![],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalKind {
    Kitty,
    Alacritty,
    Foot,
    WezTerm,
    Konsole,
}

impl TerminalKind {
    fn layer_name(self) -> &'static str {
        match self {
            Self::Kitty => "Kitty",
            Self::Alacritty => "Alacritty",
            Self::Foot => "Foot",
            Self::WezTerm => "WezTerm",
            Self::Konsole => "Konsole",
        }
    }
}

impl std::fmt::Display for TerminalKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.layer_name())
    }
}

pub(crate) fn terminal_kind() -> Option<TerminalKind> {
    if env::var_os("KONSOLE_PROFILE_NAME").is_some() || env::var_os("KONSOLE_VERSION").is_some() {
        return Some(TerminalKind::Konsole);
    }
    if env::var_os("KITTY_WINDOW_ID").is_some() {
        return Some(TerminalKind::Kitty);
    }
    if env::var_os("WEZTERM_PANE").is_some() {
        return Some(TerminalKind::WezTerm);
    }
    if env::var_os("ALACRITTY_SOCKET").is_some() {
        return Some(TerminalKind::Alacritty);
    }
    let program = env::var("TERM_PROGRAM")
        .ok()
        .or_else(|| env::var("LC_TERMINAL").ok())
        .or_else(|| {
            let term = env::var("TERM").ok()?;
            let normalized = term.to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "xterm-kitty" | "foot" | "foot-direct" | "alacritty" | "wezterm" | "konsole"
            )
            .then_some(term)
        })?
        .to_ascii_lowercase();
    if program == "kitty" {
        Some(TerminalKind::Kitty)
    } else if matches!(program.as_str(), "alacritty" | "alacritty-terminal") {
        Some(TerminalKind::Alacritty)
    } else if matches!(program.as_str(), "foot" | "footclient") {
        Some(TerminalKind::Foot)
    } else if matches!(program.as_str(), "wezterm" | "wezterm-gui") {
        Some(TerminalKind::WezTerm)
    } else if program == "konsole" {
        Some(TerminalKind::Konsole)
    } else {
        None
    }
}

fn config_path(kind: TerminalKind) -> Option<PathBuf> {
    if let Some(path) = match kind {
        TerminalKind::Alacritty => env::var_os("ALACRITTY_CONFIG_FILE"),
        TerminalKind::Kitty => None,
        TerminalKind::Foot => env::var_os("FOOT_CONFIG"),
        TerminalKind::WezTerm => None,
        TerminalKind::Konsole => None,
    } {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let candidates = match kind {
        TerminalKind::Kitty => {
            let kitty_base = env::var_os("KITTY_CONFIG_DIRECTORY")
                .map(PathBuf::from)
                .unwrap_or_else(|| base.join("kitty"));
            vec![kitty_base.join("kitty.conf")]
        }
        TerminalKind::Alacritty => vec![
            base.join("alacritty/alacritty.toml"),
            base.join("alacritty/alacritty.yml"),
            base.join("alacritty/alacritty.yaml"),
        ],
        TerminalKind::Foot => vec![base.join("foot/foot.ini")],
        TerminalKind::WezTerm => unreachable!("WezTerm uses its effective key command"),
        TerminalKind::Konsole => unreachable!("Konsole resolves its active keytab"),
    };
    candidates.into_iter().find(|path| path.is_file())
}

fn parse_bindings(kind: TerminalKind, content: &str) -> Vec<Binding> {
    match kind {
        TerminalKind::Kitty => parse_kitty(content),
        TerminalKind::Alacritty => parse_alacritty(content),
        TerminalKind::Foot => parse_foot(content),
        TerminalKind::WezTerm => parse_wezterm(content),
        TerminalKind::Konsole => parse_konsole(content),
    }
}

fn load_wezterm_bindings() -> Result<(Vec<Binding>, String), String> {
    let mut command = Command::new("wezterm");
    command.arg("show-keys");
    let output = command::output(&mut command)
        .map_err(|error| format!("wezterm show-keys is unavailable: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if error.is_empty() {
            format!("wezterm show-keys exited with {}", output.status)
        } else {
            format!("wezterm show-keys failed: {error}")
        });
    }
    let content = String::from_utf8_lossy(&output.stdout);
    Ok((
        parse_wezterm(&content),
        "source: wezterm show-keys (loads the active WezTerm config)".into(),
    ))
}

fn load_konsole_bindings() -> Result<(Vec<Binding>, String), String> {
    let profile_name = env::var("KONSOLE_PROFILE_NAME")
        .ok()
        .or_else(konsole_default_profile)
        .unwrap_or_else(|| "default".into());
    let keytab_name = konsole_profile_keytab(&profile_name).unwrap_or_else(|| "default".into());
    let keytab_path = env::var_os("KONSOLE_KEYTAB")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| find_konsole_keytab(&keytab_name))
        .ok_or_else(|| {
            format!("could not find Konsole keytab '{keytab_name}' for profile '{profile_name}'")
        })?;
    let content = fs::read_to_string(&keytab_path).map_err(|error| {
        format!(
            "could not read Konsole keytab {}: {error}",
            keytab_path.display()
        )
    })?;
    Ok((
        parse_konsole(&content),
        format!(
            "keytab: {} (profile: {profile_name})",
            keytab_path.display()
        ),
    ))
}

fn konsole_default_profile() -> Option<String> {
    let base = crate::util::config_base()?;
    let content = fs::read_to_string(base.join("konsolerc")).ok()?;
    let profile = ini_value(&content, "DefaultProfile")?;
    Some(profile.trim_end_matches(".profile").to_owned())
}

fn konsole_profile_keytab(profile_name: &str) -> Option<String> {
    let base = crate::util::data_base()?;
    let candidates = [
        base.join("konsole").join(format!("{profile_name}.profile")),
        base.join("konsole").join(profile_name),
    ];
    candidates
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .and_then(|content| ini_value(&content, "KeyBindings"))
}

fn ini_value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_owned())
    })
}

fn find_konsole_keytab(name: &str) -> Option<PathBuf> {
    let candidates = [
        crate::util::data_base().map(|base| base.join("konsole")),
        crate::util::config_base().map(|base| base.join("konsole")),
        Some(PathBuf::from("/usr/share/konsole")),
        Some(PathBuf::from("/usr/share/konsole/keyboard-layouts")),
        Some(PathBuf::from("/usr/local/share/konsole")),
    ];
    let names = if name.ends_with(".keytab") {
        vec![name.to_owned()]
    } else {
        vec![name.to_owned(), format!("{name}.keytab")]
    };
    candidates
        .into_iter()
        .flatten()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
}

fn parse_wezterm(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut table = "default";
    for raw in content.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix("Key Table:") {
            table = name.trim();
            continue;
        }
        if line.is_empty()
            || line == "Mouse"
            || line.starts_with('-')
            || line.chars().all(|character| character == '-')
            || !line.contains("->")
        {
            continue;
        }
        let Some((left, action)) = line.split_once("->") else {
            continue;
        };
        let mut tokens = left.split_whitespace();
        let mut modifiers = Vec::new();
        let mut key = None;
        for token in tokens.by_ref() {
            let token = token.trim_matches('|');
            if token.is_empty() {
                continue;
            }
            if wezterm_modifier(token) {
                if !token.eq_ignore_ascii_case("none") {
                    modifiers.push(token.to_ascii_lowercase());
                }
            } else {
                key = Some(token);
                break;
            }
        }
        let Some(key) = key else {
            continue;
        };
        let Some(trigger) = parse_wezterm_key(key, &modifiers) else {
            continue;
        };
        let action = action.trim().to_owned();
        let output = wezterm_action_output(&action);
        let description = format!("{trigger} -> {action} ({table})");
        bindings.push(Binding {
            trigger,
            action: action.clone(),
            output,
            description,
        });
    }
    bindings
}

fn wezterm_binding_table(description: &str) -> Option<&str> {
    description
        .rsplit_once(" (")
        .and_then(|(_, table)| table.strip_suffix(')'))
}

fn wezterm_modifier(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "CTRL"
            | "CONTROL"
            | "SHIFT"
            | "ALT"
            | "OPT"
            | "META"
            | "SUPER"
            | "CMD"
            | "WIN"
            | "LEADER"
            | "NONE"
    )
}

fn parse_wezterm_key(key: &str, modifiers: &[String]) -> Option<KeyCombo> {
    if key.starts_with("phys:") || key.starts_with("mapped:") || key.starts_with("raw:") {
        return None;
    }
    let mut parts = modifiers.to_vec();
    parts.push(key.to_owned());
    parts.join("+").parse().ok()
}

fn wezterm_action_output(action: &str) -> Option<Vec<u8>> {
    let action = action.trim();
    let payload = action
        .strip_prefix("SendString(")
        .or_else(|| action.strip_prefix("SendString {"))?;
    let start = payload.find(['"', '\''])?;
    let quote = payload.as_bytes()[start] as char;
    let end = payload[start + 1..].rfind(quote)? + start + 1;
    Some(decode_wezterm_escape(&payload[start + 1..end]))
}

fn decode_wezterm_escape(value: &str) -> Vec<u8> {
    let mut normalized = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("\\u{") {
        normalized.push_str(&remaining[..start]);
        let after = &remaining[start + 3..];
        let Some(end) = after.find('}') else {
            normalized.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        let code = &after[..end];
        if let Ok(value) = u32::from_str_radix(code, 16) {
            if let Some(character) = char::from_u32(value) {
                let mut encoded = [0; 4];
                normalized.push_str(character.encode_utf8(&mut encoded));
            } else {
                normalized.push_str(&remaining[start..start + 3 + end + 1]);
            }
        } else {
            normalized.push_str(&remaining[start..start + 3 + end + 1]);
        }
        remaining = &after[end + 1..];
    }
    normalized.push_str(remaining);
    decode_escape(&normalized)
}

fn parse_konsole(content: &str) -> Vec<Binding> {
    content
        .lines()
        .filter_map(|raw| {
            let line = raw.split('#').next()?.trim();
            let rest = line.strip_prefix("key ")?;
            let (left, output) = rest.split_once(':')?;
            let mut fields = left.split_whitespace();
            let key_and_modes = fields.next()?;
            let (key, attached_modes) = split_konsole_key_modes(key_and_modes);
            let mut modes = Vec::new();
            if !attached_modes.is_empty() {
                modes.push(attached_modes);
            }
            modes.extend(fields.map(str::to_owned));
            let modifiers = parse_konsole_modifiers(&modes)?;
            let trigger = parse_alacritty_key(key, &modifiers)?;
            let raw_output = output.trim();
            let decoded = if raw_output.starts_with('"') || raw_output.starts_with('\'') {
                Some(decode_escape(unquote(raw_output)))
            } else {
                None
            };
            let action = decoded
                .as_ref()
                .map(|_| "send_sequence".to_owned())
                .unwrap_or_else(|| raw_output.to_owned());
            Some(Binding {
                trigger,
                action,
                output: decoded,
                description: line.to_owned(),
            })
        })
        .collect()
}

fn split_konsole_key_modes(value: &str) -> (&str, String) {
    let Some(index) = value.find(['+', '-']) else {
        return (value, String::new());
    };
    (&value[..index], value[index..].to_owned())
}

fn parse_konsole_modifiers(modes: &[String]) -> Option<String> {
    let mut modifiers = Vec::new();
    for mode in modes {
        let mut current_sign = None;
        let mut current_name = String::new();
        let flush = |sign: Option<char>, name: &str, modifiers: &mut Vec<&'static str>| {
            if name.is_empty() {
                return sign.is_none();
            }
            if sign != Some('+') {
                return false;
            }
            let normalized = match name.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => "ctrl",
                "alt" => "alt",
                "shift" => "shift",
                _ => return false,
            };
            modifiers.push(normalized);
            true
        };
        for character in mode.chars() {
            if character == '+' || character == '-' {
                if !flush(current_sign, &current_name, &mut modifiers) {
                    return None;
                }
                current_name.clear();
                current_sign = Some(character);
            } else {
                current_name.push(character);
            }
        }
        if !flush(current_sign, &current_name, &mut modifiers) {
            return None;
        }
    }
    Some(modifiers.join("+"))
}

fn parse_kitty(content: &str) -> Vec<Binding> {
    content
        .lines()
        .filter_map(|raw| {
            let line = raw.split('#').next()?.trim();
            let mut fields = line.split_whitespace();
            (fields.next()? == "map").then_some(())?;
            let mut trigger = fields.next()?;
            while trigger.starts_with("--") {
                if !trigger.contains('=') {
                    fields.next()?;
                }
                trigger = fields.next()?;
            }
            let trigger = parse_key(trigger)?;
            let action = fields.collect::<Vec<_>>().join(" ");
            let output = action.strip_prefix("send_text ").and_then(|value| {
                value
                    .split_once(' ')
                    .map(|(_, payload)| decode_escape(payload))
            });
            let description = format!("{trigger} {action}");
            Some(Binding {
                trigger,
                action: action.clone(),
                output,
                description,
            })
        })
        .collect()
}

fn parse_alacritty(content: &str) -> Vec<Binding> {
    if !content
        .lines()
        .any(|line| line.trim() == "[[keyboard.bindings]]")
    {
        return parse_alacritty_yaml(content);
    }
    let mut bindings = Vec::new();
    let mut block = Vec::new();
    let mut in_binding = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[[keyboard.bindings]]" {
            if let Some(binding) = alacritty_block(&block) {
                bindings.push(binding);
            }
            block.clear();
            in_binding = true;
        } else if in_binding {
            block.push(trimmed);
        }
    }
    if let Some(binding) = alacritty_block(&block) {
        bindings.push(binding);
    }
    bindings
}

fn parse_alacritty_yaml(content: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut in_key_bindings = false;
    let mut key = None;
    let mut mods = None;
    let mut action = None;
    let mut chars = None;

    let finish = |bindings: &mut Vec<Binding>,
                  key: &mut Option<String>,
                  mods: &mut Option<String>,
                  action: &mut Option<String>,
                  chars: &mut Option<String>| {
        let Some(key_value) = key.take() else {
            mods.take();
            action.take();
            chars.take();
            return;
        };
        let mods_value = mods.take().unwrap_or_default();
        let Some(trigger) = parse_alacritty_key(&key_value, &mods_value) else {
            action.take();
            chars.take();
            return;
        };
        let action_name = action.take().or_else(|| chars.clone()).unwrap_or_default();
        let output = chars.take().map(|value| decode_escape(&value));
        let description = format!("{trigger} = {action_name}");
        bindings.push(Binding {
            trigger,
            action: action_name.clone(),
            output,
            description,
        });
    };

    for raw in content.lines() {
        let without_comment = raw.split('#').next().unwrap_or_default();
        let trimmed = without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "key_bindings:" {
            in_key_bindings = true;
            continue;
        }
        if !in_key_bindings {
            continue;
        }
        let indentation = without_comment.len() - without_comment.trim_start().len();
        if indentation == 0 {
            finish(&mut bindings, &mut key, &mut mods, &mut action, &mut chars);
            in_key_bindings = false;
            continue;
        }
        if let Some(item) = trimmed.strip_prefix('-') {
            finish(&mut bindings, &mut key, &mut mods, &mut action, &mut chars);
            let item = item.trim();
            if item.starts_with('{') && item.ends_with('}') {
                for (name, value) in yaml_inline_fields(&item[1..item.len() - 1]) {
                    yaml_field(&name, &value, &mut key, &mut mods, &mut action, &mut chars);
                }
            } else if let Some((name, value)) = item.split_once(':') {
                yaml_field(name, value, &mut key, &mut mods, &mut action, &mut chars);
            }
            continue;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            yaml_field(name, value, &mut key, &mut mods, &mut action, &mut chars);
        }
    }
    finish(&mut bindings, &mut key, &mut mods, &mut action, &mut chars);
    bindings
}

fn yaml_inline_fields(value: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quote = None;
    for (index, character) in value.char_indices() {
        if (character == '\'' || character == '"')
            && (index == 0 || value.as_bytes()[index - 1] != b'\\')
        {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
        } else if character == ',' && quote.is_none() {
            if let Some(field) = yaml_field_pair(&value[start..index]) {
                fields.push(field);
            }
            start = index + character.len_utf8();
        }
    }
    if let Some(field) = yaml_field_pair(&value[start..]) {
        fields.push(field);
    }
    fields
}

fn yaml_field_pair(value: &str) -> Option<(String, String)> {
    let (name, value) = value.split_once(':')?;
    Some((name.trim().to_owned(), unquote(value.trim()).to_owned()))
}

fn yaml_field(
    name: &str,
    value: &str,
    key: &mut Option<String>,
    mods: &mut Option<String>,
    action: &mut Option<String>,
    chars: &mut Option<String>,
) {
    let value = unquote(value.trim()).to_owned();
    match name.trim() {
        "key" => *key = Some(value),
        "mods" => *mods = Some(value),
        "action" => *action = Some(value),
        "chars" => *chars = Some(value),
        _ => {}
    }
}

fn alacritty_block(lines: &[&str]) -> Option<Binding> {
    let key = field_value(lines, "key")?;
    let mods = field_value(lines, "mods").unwrap_or_default();
    let trigger = parse_alacritty_key(&key, &mods)?;
    let action = field_value(lines, "action")
        .or_else(|| field_value(lines, "chars"))
        .unwrap_or_default();
    let output = field_value(lines, "chars").map(|value| decode_escape(&value));
    let description = format!("{trigger} = {action}");
    Some(Binding {
        trigger,
        action: action.clone(),
        output,
        description,
    })
}

fn field_value(lines: &[&str], field: &str) -> Option<String> {
    lines.iter().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == field).then(|| unquote(value.trim()).to_owned())
    })
}

fn parse_alacritty_key(key: &str, mods: &str) -> Option<KeyCombo> {
    let mut parts = Vec::new();
    for modifier in mods.split(['|', '+', ' ']) {
        if modifier.is_empty() {
            continue;
        }
        parts.push(match modifier.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => "ctrl",
            "alt" => "alt",
            "shift" => "shift",
            "super" | "command" | "logo" => "super",
            _ => return None,
        });
    }
    parts.push(key);
    parts.join("+").parse().ok()
}

fn parse_foot(content: &str) -> Vec<Binding> {
    let mut in_bindings = false;
    content
        .lines()
        .filter_map(|raw| {
            let line = raw.split('#').next()?.trim();
            if line.starts_with('[') {
                in_bindings = line.eq_ignore_ascii_case("[key-bindings]");
                return None;
            }
            if !in_bindings || line.is_empty() {
                return None;
            }
            let (raw_trigger, action) = line.split_once('=')?;
            let trigger = parse_key(raw_trigger.trim())?;
            Some(Binding {
                trigger,
                action: action.trim().to_owned(),
                output: None,
                description: line.to_owned(),
            })
        })
        .collect()
}

fn parse_key(raw: &str) -> Option<KeyCombo> {
    raw.parse().ok()
}

fn action_forwards(action: &str) -> bool {
    let action = action.trim().to_ascii_lowercase();
    action.starts_with("sendkey")
        || action.starts_with("sendtext")
        || action.starts_with("receivechar")
        || action.starts_with("receive_char")
        || action.starts_with("disabledefaultassignment")
        || action.starts_with("pass")
}

fn action_is_consuming(action: &str) -> bool {
    let action = action
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split(['(', '{', '='])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let compact = action
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    matches!(
        compact.as_str(),
        "noop"
            | "ignore"
            | "none"
            | "copytoclipboard"
            | "pastefromclipboard"
            | "pastefromselection"
            | "clipboardcopy"
            | "clipboardpaste"
            | "launch"
            | "kitten"
            | "closewindow"
            | "newwindow"
            | "scrolllineup"
            | "scrolllinedown"
            | "scrollpageup"
            | "scrollpagedown"
            | "searchforward"
            | "searchbackward"
            | "copy"
            | "paste"
            | "clearselection"
            | "spawnnewinstance"
            | "quit"
            | "togglefullscreen"
            | "minimize"
            | "hide"
            | "increasefont"
            | "decreasefont"
            | "resetfontsize"
            | "scrollup"
            | "scrolldown"
            | "copyto"
            | "pastefrom"
            | "activatecommandpalette"
            | "activatetab"
            | "activatetabrelative"
            | "activatepanedirection"
            | "activatepane"
            | "splithorizontal"
            | "splitvertical"
            | "closecurrentpane"
            | "closecurrenttab"
            | "spawnwindow"
            | "scrollupline"
            | "scrollupage"
            | "scrolldownline"
            | "scrolldownpage"
            | "scrolluptotop"
            | "scrolldowntobottom"
            | "scrolltop"
            | "scrollbottom"
            | "findnext"
            | "findprevious"
    ) || compact.starts_with("move")
}

fn decode_escape(value: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            continue;
        }
        match chars.next() {
            Some('e') | Some('E') => bytes.push(0x1b),
            Some('n') => bytes.push(b'\n'),
            Some('r') => bytes.push(b'\r'),
            Some('t') => bytes.push(b'\t'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                }
            }
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(codepoint) = u32::from_str_radix(&hex, 16) {
                    if let Some(character) = char::from_u32(codepoint) {
                        let mut encoded = [0; 4];
                        bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
                    }
                }
            }
            Some(other) => bytes.push(other as u8),
            None => bytes.push(b'\\'),
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::LayerStatus;

    #[test]
    fn parses_kitty_send_text_binding() {
        let bindings = parse_kitty("map ctrl+left send_text all \\e[1;5D\n");
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].output.as_deref(), Some(b"\x1b[1;5D".as_slice()));
    }

    #[test]
    fn parses_alacritty_toml_binding() {
        let content = r#"
[[keyboard.bindings]]
key = "Left"
mods = "Control"
chars = "\u001b[1;5D"
"#;
        let bindings = parse_alacritty(content);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].output.as_deref(), Some(b"\x1b[1;5D".as_slice()));
    }

    #[test]
    fn parses_alacritty_yaml_inline_binding() {
        let content = r#"
key_bindings:
  - { key: Left, mods: Control, chars: "\e[1;5D" }
  - { key: C, mods: Control|Shift, action: Copy }
"#;
        let bindings = parse_alacritty(content);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].output.as_deref(), Some(b"\x1b[1;5D".as_slice()));
        assert!(action_is_consuming(&bindings[1].action));
    }

    #[test]
    fn parses_alacritty_yaml_multiline_binding() {
        let content = "key_bindings:\n  - key: F\n    mods: Control\n    action: SearchForward\n";
        let bindings = parse_alacritty(content);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + F");
        assert!(action_is_consuming(&bindings[0].action));
    }

    #[test]
    fn parses_foot_key_bindings() {
        let bindings = parse_foot("[key-bindings]\nctrl+left=clipboard-copy\n");
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].action, "clipboard-copy");
    }

    #[test]
    fn parses_wezterm_effective_key_assignments() {
        let content = "Default key table\n-----------------\n    CTRL                 c                ->   CopyTo=\"Clipboard\"\n    SHIFT | CTRL         Tab              ->   ActivateTabRelative(-1)\n\nKey Table: copy_mode\n--------------------\n    CTRL                 f                ->   SendString(\"\\\\u{1b}[1;5C\")\n";
        let bindings = parse_wezterm(content);
        assert_eq!(bindings.len(), 3);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + C");
        assert!(action_is_consuming(&bindings[0].action));
        assert_eq!(bindings[1].trigger.to_string(), "CTRL + SHIFT + TAB");
        assert_eq!(bindings[2].output.as_deref(), Some(b"\x1b[1;5C".as_slice()));
    }

    #[test]
    fn keeps_non_default_wezterm_tables_conditional() {
        let content = "Key Table: copy_mode\n--------------------\n    CTRL                 f                ->   CopyTo=\"Clipboard\"\n";
        let bindings = parse_wezterm(content);
        assert_eq!(
            wezterm_binding_table(&bindings[0].description),
            Some("copy_mode")
        );
        let result = inspect_bindings(
            TerminalKind::WezTerm,
            &"ctrl+f".parse().unwrap(),
            &bindings,
            "source: test",
        );
        assert_eq!(result.status(), LayerStatus::Indeterminate);
        assert!(result.summary.contains("conditional key table"));
    }

    #[test]
    fn treats_wezterm_send_key_as_forwarding() {
        assert!(action_forwards("SendKey { key = \"f\", mods = \"CTRL\" }"));
        assert!(!action_is_consuming(
            "SendKey { key = \"f\", mods = \"CTRL\" }"
        ));
    }

    #[test]
    fn parses_konsole_keytab_sequence() {
        let content =
            "keyboard \"custom\"\nkey Left +Ctrl : \"\\E[1;5D\"\nkey Up +Shift : scrollUpLine\n";
        let bindings = parse_konsole(content);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].output.as_deref(), Some(b"\x1b[1;5D".as_slice()));
        assert!(action_is_consuming(&bindings[1].action));
    }

    #[test]
    fn ignores_konsole_mode_dependent_rules() {
        let bindings = parse_konsole("key Left -Shift-Ansi : \"\\E[D\"\n");
        assert!(bindings.is_empty());
    }

    #[test]
    fn classifies_standard_consuming_actions() {
        assert!(action_is_consuming("copy_to_clipboard"));
        assert!(action_is_consuming("clipboard-copy"));
        assert!(!action_is_consuming("set_tab_title"));
    }

    #[test]
    fn recognizes_alacritty_receive_char_as_forwarding() {
        assert!(action_forwards("ReceiveChar"));
    }

    #[test]
    fn recognizes_konsole_camel_case_actions() {
        assert!(action_is_consuming("scrollLineUp"));
        assert!(action_forwards("sendText \"hello\""));
    }

    #[test]
    fn later_terminal_binding_overrides_earlier_one() {
        let bindings =
            parse_kitty("map ctrl+f send_text all first\nmap ctrl+f send_text all second\n");
        let binding = bindings
            .iter()
            .rev()
            .find(|binding| binding.trigger.key() == "F")
            .unwrap();
        assert_eq!(binding.action, "send_text all second");
    }
}
