use crate::recording::PathInputs;
use std::path::PathBuf;
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{EvidenceTier, LayerId, LayerResult, Outcome, TerminalInput, format_bytes};
use crate::util::unquote;

mod defaults;
mod kitty;

/// Where a binding came from: the user's configuration (or the program's own
/// effective listing), or the terminal's built-in defaults transcribed in
/// [`defaults`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Config,
    Default,
}

#[derive(Debug, Clone)]
struct Binding {
    trigger: KeyCombo,
    action: String,
    output: Option<Vec<u8>>,
    description: String,
    origin: Origin,
    /// The configuration removes this key's binding (kitty `map key no_op`):
    /// the key reaches the program.
    unmaps: bool,
    /// The binding acts only in some terminal state; the reason.
    uncertain: Option<String>,
}

impl Binding {
    fn new(
        trigger: KeyCombo,
        action: impl Into<String>,
        output: Option<Vec<u8>>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            trigger,
            action: action.into(),
            output,
            description: description.into(),
            origin: Origin::Config,
            unmaps: false,
            uncertain: None,
        }
    }
}

/// Every binding the terminal would apply (its defaults, overridden by the
/// configuration), and where they came from.
struct Loaded {
    bindings: Vec<Binding>,
    /// Evidence line naming the file or command read.
    source: String,
    /// Tier of an answer that comes from `bindings` that are not defaults.
    tier: EvidenceTier,
    /// Whether a configuration file (or the live program) was read; without
    /// one the answer rests on the defaults alone.
    config_read: bool,
    /// Extra evidence lines.
    notes: Vec<String>,
    /// Reasons a key without a binding cannot be reported as unbound: part of
    /// the configuration was not followed.
    caveats: Vec<String>,
}

fn unavailable(kind: TerminalKind, summary: String, details: Vec<String>) -> Box<LayerResult> {
    Box::new(LayerResult::new(
        kind.layer_name(),
        LayerId::Terminal,
        Outcome::Unknown,
        summary,
        details,
    ))
}

fn load(kind: TerminalKind) -> Result<Loaded, Box<LayerResult>> {
    match kind {
        TerminalKind::WezTerm => {
            let (bindings, source) = load_wezterm_bindings().map_err(|error| {
                unavailable(
                    kind,
                    "could not inspect WezTerm effective key assignments".into(),
                    vec![error],
                )
            })?;
            // `wezterm show-keys` lists the defaults it applies as well.
            Ok(Loaded {
                bindings,
                source,
                tier: EvidenceTier::LiveQuery,
                config_read: true,
                notes: Vec::new(),
                caveats: Vec::new(),
            })
        }
        TerminalKind::Konsole => {
            let (keytab, source) = load_konsole_bindings().map_err(|error| {
                unavailable(
                    kind,
                    "could not inspect Konsole keytab bindings".into(),
                    vec![error],
                )
            })?;
            // The keytab is read last, so it wins over a default shortcut.
            let mut bindings = defaults::konsole();
            bindings.extend(keytab);
            let mut caveats = Vec::new();
            if konsole_has_custom_shortcuts() {
                caveats.push(
                    "Konsole's own shortcut settings (konsolerc [Shortcuts] or a kxmlgui rc file) are not read"
                        .to_owned(),
                );
            }
            Ok(Loaded {
                bindings,
                source,
                tier: EvidenceTier::ConfigFile,
                config_read: true,
                notes: Vec::new(),
                caveats,
            })
        }
        TerminalKind::Kitty | TerminalKind::Alacritty | TerminalKind::Foot => load_file(kind),
    }
}

fn load_file(kind: TerminalKind) -> Result<Loaded, Box<LayerResult>> {
    let Some(path) = config_path(kind) else {
        let version = defaults::version(kind).unwrap_or_default();
        let bindings = match kind {
            TerminalKind::Kitty => defaults::kitty("ctrl+shift"),
            TerminalKind::Alacritty => defaults::alacritty(),
            _ => defaults::foot(&[]),
        };
        return Ok(Loaded {
            bindings,
            source: format!(
                "no {kind} configuration file found; built-in {kind} {version} defaults only"
            ),
            tier: EvidenceTier::Default,
            config_read: false,
            notes: Vec::new(),
            caveats: Vec::new(),
        });
    };
    let source = format!("config: {}", path.display());
    if kind == TerminalKind::Kitty {
        let loaded = kitty::load(&path).map_err(|error| {
            unavailable(
                kind,
                format!("could not read {kind} configuration"),
                vec![error],
            )
        })?;
        let mut notes = loaded.notes;
        notes.extend(
            loaded
                .files
                .iter()
                .skip(1)
                .map(|file| format!("included: {}", file.display())),
        );
        return Ok(Loaded {
            bindings: loaded.bindings,
            source,
            tier: EvidenceTier::ConfigFile,
            config_read: true,
            notes,
            caveats: loaded.caveats,
        });
    }
    let content = crate::recording::read_to_string(&path).map_err(|error| {
        unavailable(
            kind,
            format!("could not read {kind} configuration"),
            vec![format!("config: {}: {error}", path.display())],
        )
    })?;
    Ok(loaded_from_content(kind, &content, source))
}

/// Alacritty or foot: the terminal's defaults overridden by `content`.
fn loaded_from_content(kind: TerminalKind, content: &str, source: String) -> Loaded {
    let mut caveats = Vec::new();
    let bindings = match kind {
        TerminalKind::Alacritty => {
            if alacritty_imports(content) {
                caveats.push(
                    "the configuration imports other files, which are not followed".to_owned(),
                );
            }
            let mut bindings = defaults::alacritty();
            bindings.extend(parse_alacritty(content));
            bindings
        }
        _ => {
            let config = parse_foot_config(content);
            if config.includes {
                caveats.push(
                    "the configuration includes other files, which are not followed".to_owned(),
                );
            }
            let mut bindings = defaults::foot(&config.replaced);
            bindings.extend(config.bindings);
            bindings
        }
    };
    Loaded {
        bindings,
        source,
        tier: EvidenceTier::ConfigFile,
        config_read: true,
        notes: Vec::new(),
        caveats,
    }
}

/// Terminal finding for `key` from everything `loaded` holds.
fn inspect_loaded(kind: TerminalKind, key: &KeyCombo, loaded: &Loaded) -> LayerResult {
    let found = loaded
        .bindings
        .iter()
        .rev()
        .find(|binding| &binding.trigger == key);
    let mut layer = resolve_bindings(kind, key, &loaded.bindings, &loaded.source);
    layer.details.extend(loaded.notes.iter().cloned());
    let Some(binding) = found else {
        if let Some(version) = defaults::version(kind) {
            layer.details.push(format!(
                "{kind} {version} default bindings checked: none match"
            ));
        }
        if layer.outcome == Outcome::Pass && !loaded.caveats.is_empty() {
            layer.outcome = Outcome::UncertainContinues;
            layer.summary = format!("no {kind} binding found, but {}", loaded.caveats[0]);
            layer.details.extend(loaded.caveats.iter().skip(1).cloned());
        }
        let tier = if loaded.config_read {
            loaded.tier
        } else {
            EvidenceTier::Default
        };
        return layer.with_tier(tier);
    };
    let tier = match binding.origin {
        Origin::Default => EvidenceTier::Default,
        Origin::Config => loaded.tier,
    };
    layer.with_tier(tier)
}

fn resolve_bindings(
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
        return LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::Pass,
            format!("no {kind} binding; forwarded to the terminal input"),
            vec![source.into()],
        );
    };

    let mut details = vec![source.into(), format!("binding: {}", binding.description)];
    if binding.unmaps {
        return LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::Pass,
            format!("{kind} configuration unmaps the key; forwarded to the terminal input"),
            details,
        );
    }
    if kind == TerminalKind::WezTerm
        && wezterm_binding_table(&binding.description)
            .is_some_and(|table| !table.eq_ignore_ascii_case("default"))
    {
        details.push(
            "this binding belongs to a non-default WezTerm key table; its active state is runtime-dependent".into(),
        );
        return LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::Unknown,
            format!("{kind} binding found in a conditional key table"),
            details,
        );
    }
    if let Some(output) = &binding.output {
        details.push(format!("sequence: {}", format_bytes(output)));
        // Konsole's keytab is its encoding table, not a shortcut: the key passes
        // as these bytes. In other terminals a byte-only binding is a user
        // rewrite (send_text, chars, ...): the terminal acts on the key and the
        // program receives the replacement bytes instead.
        let (outcome, summary) = if kind == TerminalKind::Konsole {
            (
                Outcome::Pass,
                format!("{kind} keytab encodes the key for the PTY"),
            )
        } else {
            (
                Outcome::HandledAndPassed,
                format!("{kind} binding rewrites the key to a sequence for the PTY"),
            )
        };
        return LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            outcome,
            summary,
            details,
        );
    }
    if let Some(reason) = &binding.uncertain {
        details.push(reason.clone());
        return LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::HandledUncertain,
            format!("{kind} binding found; it acts only in some terminal states"),
            details,
        );
    }
    let effect = if binding.origin == Origin::Default && kind == TerminalKind::Konsole {
        // Every Konsole shortcut in the default table is an application action.
        Effect::Consumes
    } else {
        action_effect(kind, &binding.action)
    };
    match effect {
        Effect::Forwards => LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::HandledAndPassed,
            format!("{kind} forwards the key to the PTY"),
            details,
        ),
        Effect::Consumes => LayerResult::new(
            kind.layer_name(),
            LayerId::Terminal,
            Outcome::Consumed,
            format!("{kind} consumes the key"),
            details,
        ),
        Effect::Depends(reason) => {
            details.push(reason.into());
            LayerResult::new(
                kind.layer_name(),
                LayerId::Terminal,
                Outcome::HandledUncertain,
                format!("{kind} binding found; it may or may not consume the key"),
                details,
            )
        }
        Effect::Unknown => {
            details.push("action semantics are terminal-specific".into());
            LayerResult::new(
                kind.layer_name(),
                LayerId::Terminal,
                Outcome::Unknown,
                format!("{kind} binding found; propagation is unknown"),
                details,
            )
        }
    }
}

/// What a bound action does with the key event.
enum Effect {
    Forwards,
    Consumes,
    Depends(&'static str),
    Unknown,
}

/// kitty, Alacritty and foot run every action they bind except the few that
/// hand the key on; WezTerm and Konsole name many actions, so those use the
/// shared lists.
fn action_effect(kind: TerminalKind, action: &str) -> Effect {
    let lower = action.trim().to_ascii_lowercase();
    let head = lower.split_whitespace().next().unwrap_or_default();
    match kind {
        TerminalKind::Kitty => match head {
            "send_key" | "send_text" => Effect::Forwards,
            "copy_or_interrupt" | "copy_and_clear_or_interrupt" | "copy_or_noop" => {
                Effect::Depends(
                    "it copies a selection if there is one and otherwise passes the key on",
                )
            }
            "combine" if lower.contains("send_") => {
                Effect::Depends("the combined actions send text to the program")
            }
            "" => Effect::Unknown,
            _ => Effect::Consumes,
        },
        TerminalKind::Alacritty => {
            if lower.contains("receivechar") || lower.contains("receive_char") {
                Effect::Forwards
            } else if lower.is_empty() {
                Effect::Unknown
            } else {
                Effect::Consumes
            }
        }
        TerminalKind::Foot => {
            if lower.is_empty() {
                Effect::Unknown
            } else {
                Effect::Consumes
            }
        }
        TerminalKind::WezTerm | TerminalKind::Konsole => {
            if action_forwards(action) {
                Effect::Forwards
            } else if action_is_consuming(action) {
                Effect::Consumes
            } else {
                Effect::Unknown
            }
        }
    }
}

/// Analyze once: route finding plus normal input bytes from a single config
/// load. Reused for the visible terminal layer and downstream TTY/shell
/// analysis so the file is read once per static inspection. `layout`
/// resolves the text of AltGr (level-3) keys.
pub fn analyze(
    key: &KeyCombo,
    protocol_flags: Option<u32>,
    layout: Option<&crate::xkb::KeyboardLayout>,
) -> (LayerResult, Option<TerminalInput>) {
    let Some(kind) = terminal_kind() else {
        return (not_applicable("terminal identity is not known"), None);
    };
    let (layer, input) = analyze_kind(kind, key);
    // A key nothing binds (or that only a caveat keeps from "unbound")
    // continues to the PTY, where the terminal emits its normal encoding
    // (shared with the Ghostty/generic path).
    if input.is_some() || !matches!(layer.outcome, Outcome::Pass | Outcome::UncertainContinues) {
        return (layer, input);
    }
    crate::layers::ghostty::with_default_encoding(
        layer,
        crate::layers::ghostty::default_encoding_for_protocol(key, protocol_flags, layout),
        format!("{kind} default encoding"),
        true,
    )
}

fn analyze_kind(kind: TerminalKind, key: &KeyCombo) -> (LayerResult, Option<TerminalInput>) {
    let loaded = match load(kind) {
        Ok(loaded) => loaded,
        Err(layer) => return (*layer, None),
    };
    let layer = inspect_loaded(kind, key, &loaded);
    let output = loaded
        .bindings
        .into_iter()
        .rev()
        .find(|binding| {
            &binding.trigger == key
                && (kind != TerminalKind::WezTerm
                    || wezterm_binding_table(&binding.description)
                        .is_none_or(|table| table.eq_ignore_ascii_case("default")))
        })
        .and_then(|binding| binding.output);
    let description = match kind {
        TerminalKind::WezTerm => "explicit WezTerm SendString output".to_owned(),
        TerminalKind::Konsole => "explicit Konsole keytab output".to_owned(),
        _ => format!("explicit {kind} terminal output"),
    };
    (
        layer,
        output.map(|bytes| TerminalInput::configured(bytes, description)),
    )
}

pub fn applicable() -> bool {
    terminal_kind().is_some()
}

pub fn detected_name() -> Option<&'static str> {
    terminal_kind().map(TerminalKind::layer_name)
}

fn not_applicable(message: &str) -> LayerResult {
    LayerResult::new(
        "Terminal adapter",
        LayerId::Terminal,
        Outcome::UncertainContinues,
        message,
        vec![],
    )
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
    if crate::recording::env_var_os("KONSOLE_PROFILE_NAME").is_some()
        || crate::recording::env_var_os("KONSOLE_VERSION").is_some()
    {
        return Some(TerminalKind::Konsole);
    }
    if crate::recording::env_var_os("KITTY_WINDOW_ID").is_some() {
        return Some(TerminalKind::Kitty);
    }
    if crate::recording::env_var_os("WEZTERM_PANE").is_some() {
        return Some(TerminalKind::WezTerm);
    }
    if crate::recording::env_var_os("ALACRITTY_SOCKET").is_some() {
        return Some(TerminalKind::Alacritty);
    }
    kind_from_program(&terminal_program()?)
}

fn kind_from_program(program: &str) -> Option<TerminalKind> {
    match program.to_ascii_lowercase().as_str() {
        "kitty" => Some(TerminalKind::Kitty),
        "alacritty" | "alacritty-terminal" => Some(TerminalKind::Alacritty),
        "foot" | "footclient" => Some(TerminalKind::Foot),
        "wezterm" | "wezterm-gui" => Some(TerminalKind::WezTerm),
        "konsole" => Some(TerminalKind::Konsole),
        _ => None,
    }
}

/// Terminal program named by `TERM_PROGRAM` / `LC_TERMINAL`, or inferred
/// from a terminal-specific `TERM`. Empty variables count as unset. The
/// single identity source shared by the Ghostty and generic terminal
/// adapters.
pub(crate) fn terminal_program() -> Option<String> {
    program_from_env(|name| crate::recording::env_var(name).ok())
}

fn program_from_env(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    set("TERM_PROGRAM")
        .or_else(|| set("LC_TERMINAL"))
        .or_else(|| program_from_term(&var("TERM")?).map(str::to_owned))
}

fn program_from_term(term: &str) -> Option<&'static str> {
    match term.to_ascii_lowercase().as_str() {
        "xterm-kitty" => Some("kitty"),
        "foot" | "foot-direct" => Some("foot"),
        "alacritty" | "alacritty-direct" => Some("alacritty"),
        "wezterm" => Some("wezterm"),
        "konsole" | "konsole-direct" | "konsole-256color" => Some("konsole"),
        "xterm-ghostty" | "ghostty" => Some("ghostty"),
        _ => None,
    }
}

fn config_path(kind: TerminalKind) -> Option<PathBuf> {
    if let Some(path) = match kind {
        TerminalKind::Alacritty => crate::recording::env_var_os("ALACRITTY_CONFIG_FILE"),
        TerminalKind::Kitty => None,
        TerminalKind::Foot => crate::recording::env_var_os("FOOT_CONFIG"),
        TerminalKind::WezTerm => None,
        TerminalKind::Konsole => None,
    } {
        let path = PathBuf::from(path);
        if path.recorded_is_file() {
            return Some(path);
        }
    }
    let base = crate::util::config_base()?;
    let candidates = match kind {
        TerminalKind::Kitty => {
            let kitty_base = crate::recording::env_var_os("KITTY_CONFIG_DIRECTORY")
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
    candidates.into_iter().find(|path| path.recorded_is_file())
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
    let profile_name = crate::recording::env_var("KONSOLE_PROFILE_NAME")
        .ok()
        .or_else(konsole_default_profile)
        .unwrap_or_else(|| "default".into());
    let keytab_name = konsole_profile_keytab(&profile_name).unwrap_or_else(|| "default".into());
    let keytab_path = crate::recording::env_var_os("KONSOLE_KEYTAB")
        .map(PathBuf::from)
        .filter(|path| path.recorded_is_file())
        .or_else(|| find_konsole_keytab(&keytab_name))
        .ok_or_else(|| {
            format!("could not find Konsole keytab '{keytab_name}' for profile '{profile_name}'")
        })?;
    let content = crate::recording::read_to_string(&keytab_path).map_err(|error| {
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
    let content = crate::recording::read_to_string(base.join("konsolerc")).ok()?;
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
        .find_map(|path| crate::recording::read_to_string(path).ok())
        .and_then(|content| ini_value(&content, "KeyBindings"))
}

/// Whether the user changed Konsole's own shortcuts: a non-empty
/// `[Shortcuts]` group in `konsolerc`, or a shortcut in a kxmlgui rc file.
fn konsole_has_custom_shortcuts() -> bool {
    let in_konsolerc = crate::util::config_base()
        .and_then(|base| crate::recording::read_to_string(base.join("konsolerc")).ok())
        .is_some_and(|content| {
            let mut in_shortcuts = false;
            content.lines().any(|line| {
                let line = line.trim();
                if line.starts_with('[') {
                    in_shortcuts = line.eq_ignore_ascii_case("[Shortcuts]");
                    false
                } else {
                    in_shortcuts && line.contains('=')
                }
            })
        });
    in_konsolerc
        || crate::util::data_base()
            .map(|base| base.join("kxmlgui5/konsole"))
            .and_then(|dir| crate::recording::read_dir(dir).ok())
            .is_some_and(|entries| {
                entries.flatten().any(|entry| {
                    crate::recording::read_to_string(entry.path())
                        .is_ok_and(|content| content.contains("shortcut=\""))
                })
            })
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
        .find(|path| path.recorded_is_file())
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
        bindings.push(Binding::new(trigger, action.clone(), output, description));
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
    // show-keys writes a shifted letter in capitals without a SHIFT token:
    // `CTRL C` is ctrl+shift+c, while `CTRL c` is ctrl+c.
    let shifted_letter = key.len() == 1 && key.as_bytes()[0].is_ascii_uppercase();
    if shifted_letter && !modifiers.iter().any(|modifier| modifier == "shift") {
        parts.push("shift".to_owned());
    }
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
            Some(Binding::new(trigger, action, decoded, line))
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
    let mut mode = None;

    let finish = |bindings: &mut Vec<Binding>,
                  key: &mut Option<String>,
                  mods: &mut Option<String>,
                  action: &mut Option<String>,
                  chars: &mut Option<String>,
                  mode: &mut Option<String>| {
        let mode_value = mode.take();
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
        let mut binding = Binding::new(trigger, action_name, output, description);
        binding.uncertain = alacritty_mode_note(mode_value.as_deref());
        bindings.push(binding);
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
            finish(
                &mut bindings,
                &mut key,
                &mut mods,
                &mut action,
                &mut chars,
                &mut mode,
            );
            in_key_bindings = false;
            continue;
        }
        if let Some(item) = trimmed.strip_prefix('-') {
            finish(
                &mut bindings,
                &mut key,
                &mut mods,
                &mut action,
                &mut chars,
                &mut mode,
            );
            let item = item.trim();
            if item.starts_with('{') && item.ends_with('}') {
                for (name, value) in yaml_inline_fields(&item[1..item.len() - 1]) {
                    yaml_field(
                        &name,
                        &value,
                        &mut key,
                        &mut mods,
                        &mut action,
                        &mut chars,
                        &mut mode,
                    );
                }
            } else if let Some((name, value)) = item.split_once(':') {
                yaml_field(
                    name,
                    value,
                    &mut key,
                    &mut mods,
                    &mut action,
                    &mut chars,
                    &mut mode,
                );
            }
            continue;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            yaml_field(
                name,
                value,
                &mut key,
                &mut mods,
                &mut action,
                &mut chars,
                &mut mode,
            );
        }
    }
    finish(
        &mut bindings,
        &mut key,
        &mut mods,
        &mut action,
        &mut chars,
        &mut mode,
    );
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
    mode: &mut Option<String>,
) {
    let value = unquote(value.trim()).to_owned();
    match name.trim() {
        "key" => *key = Some(value),
        "mods" => *mods = Some(value),
        "action" => *action = Some(value),
        "chars" => *chars = Some(value),
        "mode" => *mode = Some(value),
        "command" => {
            action.get_or_insert_with(|| "command".to_owned());
        }
        _ => {}
    }
}

fn alacritty_block(lines: &[&str]) -> Option<Binding> {
    let key = field_value(lines, "key")?;
    let mods = field_value(lines, "mods").unwrap_or_default();
    let trigger = parse_alacritty_key(&key, &mods)?;
    let action = field_value(lines, "action")
        .or_else(|| field_value(lines, "chars"))
        .or_else(|| field_value(lines, "command").map(|_| "command".to_owned()))
        .unwrap_or_default();
    let output = field_value(lines, "chars").map(|value| decode_escape(&value));
    let description = format!("{trigger} = {action}");
    let mut binding = Binding::new(trigger, action, output, description);
    binding.uncertain = alacritty_mode_note(field_value(lines, "mode").as_deref());
    Some(binding)
}

/// An Alacritty binding with a `mode` condition applies only while the
/// terminal is in that state (alternate screen, Vi mode, ...).
fn alacritty_mode_note(mode: Option<&str>) -> Option<String> {
    let mode = mode?.trim();
    (!mode.is_empty()).then(|| format!("the binding applies only in mode \"{mode}\""))
}

/// Whether an Alacritty configuration pulls in other files (`import` in TOML
/// under `[general]`, or at the top level of the older YAML format).
fn alacritty_imports(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default().trim();
        line.strip_prefix("import")
            .is_some_and(|rest| rest.trim_start().starts_with(['=', ':']))
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

/// What foot's configuration contributes: the bindings it adds, the actions
/// it mentions in `[key-bindings]` (each loses its default keys, even when
/// set to `none`), and whether it includes other files.
#[derive(Default)]
struct FootConfig {
    bindings: Vec<Binding>,
    replaced: Vec<String>,
    includes: bool,
}

#[cfg(test)]
fn parse_foot(content: &str) -> Vec<Binding> {
    parse_foot_config(content).bindings
}

fn parse_foot_config(content: &str) -> FootConfig {
    let mut config = FootConfig::default();
    let mut section = String::new();
    for raw in content.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).to_ascii_lowercase();
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if section.is_empty() || section == "main" {
            // Includes may sit anywhere before the first section header.
            config.includes |= name == "include";
        }
        match section.as_str() {
            "key-bindings" => {
                config.replaced.push(name.to_owned());
                // foot binds `action=keys`; pipe actions take `action=[command] keys`.
                let keys = if value.starts_with('[') {
                    value.split_once(']').map_or("", |(_, rest)| rest)
                } else {
                    value
                };
                // foot writes `none` for an unbound action; it is not a key.
                for key in keys
                    .split_whitespace()
                    .filter(|key| !key.eq_ignore_ascii_case("none"))
                {
                    if let Some(trigger) = parse_key(key) {
                        config
                            .bindings
                            .push(Binding::new(trigger, name, None, line));
                    }
                }
            }
            // `[text-bindings]` is `bytes=keys`: the key sends the bytes.
            "text-bindings" => {
                let bytes = decode_escape(name);
                for key in value
                    .split_whitespace()
                    .filter(|key| !key.eq_ignore_ascii_case("none"))
                {
                    if let Some(trigger) = parse_key(key) {
                        config.bindings.push(Binding::new(
                            trigger,
                            "text-binding",
                            Some(bytes.clone()),
                            line,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    config
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
            | "command"
            | "pipevisible"
            | "pipescrollback"
            | "pipeselected"
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
            | "emitevent"
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
    fn terminal_specific_term_values_resolve_to_one_identity() {
        assert_eq!(program_from_term("xterm-kitty"), Some("kitty"));
        assert_eq!(program_from_term("foot-direct"), Some("foot"));
        assert_eq!(program_from_term("alacritty"), Some("alacritty"));
        assert_eq!(program_from_term("wezterm"), Some("wezterm"));
        assert_eq!(program_from_term("konsole"), Some("konsole"));
        assert_eq!(program_from_term("xterm-ghostty"), Some("ghostty"));
        assert_eq!(program_from_term("xterm-256color"), None);
        // Every TERM-inferred program maps to its adapter; TERM=xterm-kitty
        // used to yield "xterm-kitty", which no adapter matched.
        for (term, kind) in [
            ("xterm-kitty", TerminalKind::Kitty),
            ("foot", TerminalKind::Foot),
            ("alacritty", TerminalKind::Alacritty),
            ("wezterm", TerminalKind::WezTerm),
            ("konsole", TerminalKind::Konsole),
        ] {
            assert_eq!(
                program_from_term(term).and_then(kind_from_program),
                Some(kind)
            );
        }
    }

    #[test]
    fn empty_terminal_variables_count_as_unset() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(program_from_env(env(&[("TERM_PROGRAM", "")])), None);
        assert_eq!(
            program_from_env(env(&[("TERM_PROGRAM", ""), ("LC_TERMINAL", "iTerm2")])),
            Some("iTerm2".into())
        );
        assert_eq!(
            program_from_env(env(&[
                ("TERM_PROGRAM", ""),
                ("LC_TERMINAL", ""),
                ("TERM", "xterm-kitty")
            ])),
            Some("kitty".into())
        );
        assert_eq!(
            program_from_env(env(&[("TERM_PROGRAM", "foot"), ("TERM", "xterm-kitty")])),
            Some("foot".into())
        );
    }

    #[test]
    fn parses_kitty_send_text_binding() {
        let loaded = kitty::parse_str(
            "map ctrl+left send_text all \\e[1;5D\n",
            std::path::Path::new("/nowhere"),
        );
        let binding = loaded.bindings.last().unwrap();
        assert_eq!(binding.trigger.to_string(), "CTRL + LEFT");
        assert_eq!(binding.output.as_deref(), Some(b"\x1b[1;5D".as_slice()));
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
    fn alacritty_command_binding_is_consuming() {
        let toml = r#"
[[keyboard.bindings]]
key = "C"
mods = "Control|Shift"
command = { program = "sh", args = ["-c", "touch /tmp/fired"] }
"#;
        let bindings = parse_alacritty(toml);
        assert_eq!(bindings[0].action, "command");
        assert!(action_is_consuming(&bindings[0].action));
        let yaml = "key_bindings:\n  - { key: C, mods: Control|Shift, command: { program: sh } }\n";
        let bindings = parse_alacritty(yaml);
        assert!(action_is_consuming(&bindings[0].action));
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
        let bindings = parse_foot("[key-bindings]\nclipboard-copy=Control+Left\n");
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + LEFT");
        assert_eq!(bindings[0].action, "clipboard-copy");
    }

    #[test]
    fn foot_pipe_binding_is_consuming() {
        let content = "[key-bindings]\nclipboard-copy=none\npipe-visible=[sh -c \"touch /tmp/fired\"] Control+Shift+c\n";
        let bindings = parse_foot(content);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].trigger.to_string(), "CTRL + SHIFT + C");
        assert_eq!(bindings[0].action, "pipe-visible");
        assert!(action_is_consuming(&bindings[0].action));
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
    fn wezterm_capital_letter_is_shift_and_emit_event_consumes() {
        let content = "Default key table\n-----------------\n    CTRL                 C                ->   EmitEvent(\"user-defined-0\")\n";
        let bindings = parse_wezterm(content);
        assert_eq!(bindings.len(), 1);
        let shifted: KeyCombo = "ctrl+shift+c".parse().unwrap();
        assert_eq!(bindings[0].trigger, shifted);
        let result = resolve_bindings(
            TerminalKind::WezTerm,
            &"ctrl+shift+c".parse().unwrap(),
            &bindings,
            "test",
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            resolve_bindings(
                TerminalKind::WezTerm,
                &"ctrl+c".parse().unwrap(),
                &bindings,
                "test"
            )
            .outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn keeps_non_default_wezterm_tables_conditional() {
        let content = "Key Table: copy_mode\n--------------------\n    CTRL                 f                ->   CopyTo=\"Clipboard\"\n";
        let bindings = parse_wezterm(content);
        assert_eq!(
            wezterm_binding_table(&bindings[0].description),
            Some("copy_mode")
        );
        let loaded = Loaded {
            bindings,
            source: "source: test".into(),
            tier: EvidenceTier::LiveQuery,
            config_read: true,
            notes: Vec::new(),
            caveats: Vec::new(),
        };
        let result = inspect_loaded(TerminalKind::WezTerm, &"ctrl+f".parse().unwrap(), &loaded);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
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
        let loaded = kitty::parse_str(
            "map ctrl+f send_text all first\nmap ctrl+f send_text all second\n",
            std::path::Path::new("/nowhere"),
        );
        let binding = loaded
            .bindings
            .iter()
            .rev()
            .find(|binding| binding.trigger.to_string() == "CTRL + F")
            .unwrap();
        assert_eq!(binding.action, "send_text all second");
    }

    // ---- Built-in default bindings (D-186): a stock configuration is not "unbound".

    fn key(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    fn stock(kind: TerminalKind) -> Loaded {
        let bindings = match kind {
            TerminalKind::Kitty => defaults::kitty("ctrl+shift"),
            TerminalKind::Alacritty => defaults::alacritty(),
            TerminalKind::Foot => defaults::foot(&[]),
            _ => defaults::konsole(),
        };
        Loaded {
            bindings,
            source: "no configuration file".into(),
            tier: EvidenceTier::Default,
            config_read: false,
            notes: Vec::new(),
            caveats: Vec::new(),
        }
    }

    fn configured(kind: TerminalKind, content: &str) -> Loaded {
        match kind {
            TerminalKind::Kitty => {
                let loaded = kitty::parse_str(content, std::path::Path::new("/nowhere"));
                Loaded {
                    bindings: loaded.bindings,
                    source: "config: kitty.conf".into(),
                    tier: EvidenceTier::ConfigFile,
                    config_read: true,
                    notes: loaded.notes,
                    caveats: loaded.caveats,
                }
            }
            _ => loaded_from_content(kind, content, "config: test".into()),
        }
    }

    fn verdict(kind: TerminalKind, loaded: &Loaded, text: &str) -> (Outcome, Option<EvidenceTier>) {
        let layer = inspect_loaded(kind, &key(text), loaded);
        (layer.outcome, layer.tier)
    }

    #[test]
    fn stock_kitty_consumes_its_default_shortcuts() {
        let loaded = stock(TerminalKind::Kitty);
        for text in [
            "ctrl+shift+c",
            "ctrl+shift+v",
            "ctrl+shift+t",
            "ctrl+shift+w",
            "ctrl+shift+enter",
            "ctrl+shift+f11",
            "ctrl+tab",
            "shift+insert",
            "ctrl+shift+equal",
        ] {
            assert_eq!(
                verdict(TerminalKind::Kitty, &loaded, text),
                (Outcome::Consumed, Some(EvidenceTier::Default)),
                "{text}"
            );
        }
        let layer = inspect_loaded(TerminalKind::Kitty, &key("ctrl+shift+c"), &loaded);
        assert!(
            layer
                .details
                .iter()
                .any(|detail| detail.contains("Kitty 0.50.0 default"))
        );
        // A key kitty does not bind still passes.
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "alt+x"),
            (Outcome::Pass, Some(EvidenceTier::Default))
        );
        // The first key of a multi-key default starts a sequence.
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+p").0,
            Outcome::Consumed
        );
    }

    #[test]
    fn kitty_config_overrides_and_unbinds_its_defaults() {
        // A mapping replaces the default; the tier says the config decided it.
        let loaded = configured(TerminalKind::Kitty, "map kitty_mod+c send_text all copy\n");
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+c"),
            (Outcome::HandledAndPassed, Some(EvidenceTier::ConfigFile))
        );
        // An empty `map` / `no_op` hands the key to the program.
        for line in ["map kitty_mod+c", "map ctrl+shift+c no_op"] {
            let loaded = configured(TerminalKind::Kitty, line);
            assert_eq!(
                verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+c"),
                (Outcome::Pass, Some(EvidenceTier::ConfigFile)),
                "{line}"
            );
        }
        // discard_event swallows it.
        let loaded = configured(TerminalKind::Kitty, "map kitty_mod+c discard_event\n");
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+c").0,
            Outcome::Consumed
        );
        // clear_all_shortcuts frees every default.
        let loaded = configured(
            TerminalKind::Kitty,
            "clear_all_shortcuts yes\nmap kitty_mod+f12 launch ls\n",
        );
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+c").0,
            Outcome::Pass
        );
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+f12"),
            (Outcome::Consumed, Some(EvidenceTier::ConfigFile))
        );
    }

    #[test]
    fn kitty_mod_moves_the_default_shortcuts() {
        let loaded = configured(TerminalKind::Kitty, "kitty_mod ctrl+alt\n");
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+c").0,
            Outcome::Pass
        );
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+alt+c"),
            (Outcome::Consumed, Some(EvidenceTier::Default))
        );
    }

    #[test]
    fn kitty_map_lines_with_kitty_mod_are_not_dropped() {
        let loaded = configured(
            TerminalKind::Kitty,
            "map kitty_mod+f12 launch ls\nmap kitty_mod+l send_text all \\e[1;5D\n",
        );
        assert_eq!(
            verdict(TerminalKind::Kitty, &loaded, "ctrl+shift+f12"),
            (Outcome::Consumed, Some(EvidenceTier::ConfigFile))
        );
        let layer = inspect_loaded(TerminalKind::Kitty, &key("ctrl+shift+l"), &loaded);
        assert_eq!(layer.outcome, Outcome::HandledAndPassed);
    }

    #[test]
    fn kitty_follows_include_and_globinclude_relative_to_the_config_dir() {
        let directory =
            std::env::temp_dir().join(format!("whykey-kitty-include-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("conf.d")).unwrap();
        let main = directory.join("kitty.conf");
        std::fs::write(
            &main,
            "include keys.conf\nglobinclude conf.d/*.conf\nmap ctrl+f3 launch last\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("keys.conf"),
            "kitty_mod ctrl+alt\nmap kitty_mod+f1 launch a\ninclude nested.conf\n",
        )
        .unwrap();
        std::fs::write(directory.join("nested.conf"), "map ctrl+f2 launch nested\n").unwrap();
        std::fs::write(
            directory.join("conf.d/one.conf"),
            "map ctrl+f4 launch glob\n",
        )
        .unwrap();

        let loaded = kitty::load(&main).unwrap();
        let find = |text: &str| {
            let combo = key(text);
            loaded
                .bindings
                .iter()
                .rev()
                .find(|binding| binding.trigger == combo)
                .map(|binding| binding.action.clone())
        };
        assert_eq!(find("ctrl+alt+f1").as_deref(), Some("launch a"));
        assert_eq!(find("ctrl+f2").as_deref(), Some("launch nested"));
        assert_eq!(find("ctrl+f4").as_deref(), Some("launch glob"));
        assert_eq!(find("ctrl+f3").as_deref(), Some("launch last"));
        assert_eq!(loaded.files.len(), 4);
        assert!(loaded.caveats.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn kitty_unfollowed_includes_make_a_missing_binding_uncertain() {
        let loaded = configured(TerminalKind::Kitty, "geninclude gen.py\n");
        let layer = inspect_loaded(TerminalKind::Kitty, &key("alt+x"), &loaded);
        assert_eq!(layer.outcome, Outcome::UncertainContinues);
        assert!(layer.summary.contains("geninclude"));
    }

    #[test]
    fn stock_foot_consumes_its_default_shortcuts() {
        let loaded = stock(TerminalKind::Foot);
        for text in [
            "ctrl+shift+c",
            "ctrl+shift+v",
            "ctrl+shift+n",
            "ctrl+shift+r",
            "ctrl+shift+o",
            "ctrl+minus",
            "ctrl+0",
            "shift+insert",
            "shift+page_up",
        ] {
            assert_eq!(
                verdict(TerminalKind::Foot, &loaded, text),
                (Outcome::Consumed, Some(EvidenceTier::Default)),
                "{text}"
            );
        }
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "alt+x"),
            (Outcome::Pass, Some(EvidenceTier::Default))
        );
    }

    #[test]
    fn foot_config_replaces_the_keys_of_an_action_it_mentions() {
        // `none` unbinds the action's default keys.
        let loaded = configured(TerminalKind::Foot, "[key-bindings]\nclipboard-copy=none\n");
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "ctrl+shift+c"),
            (Outcome::Pass, Some(EvidenceTier::ConfigFile))
        );
        // Other defaults stay.
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "ctrl+shift+v").0,
            Outcome::Consumed
        );
        // Rebinding the action moves it.
        let loaded = configured(
            TerminalKind::Foot,
            "[key-bindings]\nclipboard-copy=Control+Shift+y\n",
        );
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "ctrl+shift+c").0,
            Outcome::Pass
        );
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "ctrl+shift+y"),
            (Outcome::Consumed, Some(EvidenceTier::ConfigFile))
        );
    }

    #[test]
    fn foot_text_bindings_rewrite_the_key() {
        let loaded = configured(
            TerminalKind::Foot,
            "[text-bindings]\n\\x03=Control+Mod1+c\n",
        );
        let layer = inspect_loaded(TerminalKind::Foot, &key("ctrl+alt+c"), &loaded);
        assert_eq!(layer.outcome, Outcome::HandledAndPassed);
    }

    #[test]
    fn foot_include_makes_a_missing_binding_uncertain() {
        let loaded = configured(
            TerminalKind::Foot,
            "include=/etc/foot/extra.ini\n[main]\nterm=foot\n",
        );
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "alt+x").0,
            Outcome::UncertainContinues
        );
        // A key that does match is still answered.
        assert_eq!(
            verdict(TerminalKind::Foot, &loaded, "ctrl+shift+c").0,
            Outcome::Consumed
        );
    }

    #[test]
    fn stock_alacritty_consumes_its_default_shortcuts() {
        let loaded = stock(TerminalKind::Alacritty);
        for text in [
            "ctrl+shift+c",
            "ctrl+shift+v",
            "ctrl+shift+f",
            "ctrl+shift+space",
            "shift+insert",
            "ctrl+0",
            "ctrl+minus",
            "ctrl+equal",
        ] {
            assert_eq!(
                verdict(TerminalKind::Alacritty, &loaded, text),
                (Outcome::Consumed, Some(EvidenceTier::Default)),
                "{text}"
            );
        }
        // Ctrl+L clears a log notice and still reaches the program.
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "ctrl+l"),
            (Outcome::HandledAndPassed, Some(EvidenceTier::Default))
        );
        // Scrolling is off in the alternate screen.
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "shift+page_up"),
            (Outcome::HandledUncertain, Some(EvidenceTier::Default))
        );
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "alt+x"),
            (Outcome::Pass, Some(EvidenceTier::Default))
        );
    }

    #[test]
    fn alacritty_config_overrides_its_defaults() {
        let loaded = configured(
            TerminalKind::Alacritty,
            "[[keyboard.bindings]]\nkey = \"C\"\nmods = \"Control|Shift\"\naction = \"ReceiveChar\"\n\n[[keyboard.bindings]]\nkey = \"V\"\nmods = \"Control|Shift\"\naction = \"None\"\n",
        );
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "ctrl+shift+c"),
            (Outcome::HandledAndPassed, Some(EvidenceTier::ConfigFile))
        );
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "ctrl+shift+v"),
            (Outcome::Consumed, Some(EvidenceTier::ConfigFile))
        );
    }

    #[test]
    fn alacritty_import_and_mode_conditions_are_uncertain() {
        let loaded = configured(
            TerminalKind::Alacritty,
            "[general]\nimport = [\"/etc/alacritty/extra.toml\"]\n",
        );
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "alt+x").0,
            Outcome::UncertainContinues
        );
        let loaded = configured(
            TerminalKind::Alacritty,
            "[[keyboard.bindings]]\nkey = \"K\"\nmods = \"Control\"\nmode = \"~Alt\"\naction = \"ScrollPageUp\"\n",
        );
        assert_eq!(
            verdict(TerminalKind::Alacritty, &loaded, "ctrl+k").0,
            Outcome::HandledUncertain
        );
    }

    #[test]
    fn konsole_default_shortcuts_are_consumed_and_the_keytab_still_wins() {
        let mut loaded = stock(TerminalKind::Konsole);
        loaded.config_read = true;
        loaded.tier = EvidenceTier::ConfigFile;
        for text in [
            "ctrl+shift+t",
            "ctrl+shift+c",
            "ctrl+shift+v",
            "f11",
            "ctrl+minus",
        ] {
            assert_eq!(
                verdict(TerminalKind::Konsole, &loaded, text),
                (Outcome::Consumed, Some(EvidenceTier::Default)),
                "{text}"
            );
        }
        assert_eq!(
            verdict(TerminalKind::Konsole, &loaded, "alt+x").0,
            Outcome::Pass
        );
        loaded
            .bindings
            .extend(parse_konsole("key F11 +Ctrl : \"\\E[23;5~\"\n"));
        // The keytab entry is later than the default and is the answer.
        let layer = inspect_loaded(TerminalKind::Konsole, &key("ctrl+f11"), &loaded);
        assert_eq!(layer.outcome, Outcome::Pass);
        assert_eq!(layer.tier, Some(EvidenceTier::ConfigFile));
    }

    #[test]
    fn konsole_custom_shortcuts_make_a_missing_binding_uncertain() {
        let mut loaded = stock(TerminalKind::Konsole);
        loaded
            .caveats
            .push("Konsole's own shortcut settings are not read".into());
        let layer = inspect_loaded(TerminalKind::Konsole, &key("alt+x"), &loaded);
        assert_eq!(layer.outcome, Outcome::UncertainContinues);
        // A default shortcut is still reported (and its tier says "default").
        assert_eq!(
            verdict(TerminalKind::Konsole, &loaded, "ctrl+shift+t"),
            (Outcome::Consumed, Some(EvidenceTier::Default))
        );
    }

    #[test]
    fn a_terminal_with_an_unfollowed_config_part_still_forwards_the_encoding() {
        // `analyze` keeps the encoding analysis for an UncertainContinues layer.
        let loaded = configured(TerminalKind::Kitty, "geninclude gen.py\n");
        let layer = inspect_loaded(TerminalKind::Kitty, &key("alt+x"), &loaded);
        let (layer, input) = crate::layers::ghostty::with_default_encoding(
            layer,
            crate::layers::ghostty::default_encoding_for_protocol(&key("alt+x"), None, None),
            "kitty default encoding",
            true,
        );
        assert_eq!(layer.outcome, Outcome::UncertainContinues);
        assert_eq!(input.expect("alt+x bytes").bytes, b"\x1bx");
    }
}
