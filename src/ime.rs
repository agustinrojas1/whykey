use std::collections::BTreeMap;
use std::process::Command;

use serde::Serialize;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{Checked, EvidenceTier, LayerId, LayerResult, Outcome};

/// Read-only input-method context discovered from the current session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Observed,
    Conditional,
    Unobserved,
}

impl EvidenceStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Conditional => "conditional",
            Self::Unobserved => "unobserved",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TextReconstruction {
    /// Text committed by the application cannot be attributed to one key
    /// from a passive terminal/compositor observation.
    pub committed_text: EvidenceStatus,
    /// Compose/dead-key state belongs to the input-method and terminal state
    /// machines, not to the daemon-level engine query.
    pub compose_dead_key: EvidenceStatus,
    /// Application-side preedit is exposed by the client toolkit, not by the
    /// read-only IBus/Fcitx5 controller methods used here.
    pub application_preedit: EvidenceStatus,
}

impl Default for TextReconstruction {
    fn default() -> Self {
        Self {
            committed_text: EvidenceStatus::Conditional,
            compose_dead_key: EvidenceStatus::Unobserved,
            application_preedit: EvidenceStatus::Unobserved,
        }
    }
}

impl TextReconstruction {
    fn detail(&self) -> String {
        format!(
            "text reconstruction: committed text {}, Compose/dead-key history {}, application preedit {}",
            self.committed_text.label(),
            self.compose_dead_key.label(),
            self.application_preedit.label()
        )
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Detection {
    pub engine: String,
    pub sources: Vec<String>,
    pub processes: Vec<String>,
    /// Engine selected by the input-method daemon at observation time, when
    /// its documented read-only D-Bus controller is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_engine: Option<String>,
    /// Fcitx5's daemon state. IBus does not expose an equivalent state in its
    /// small command-line query, so this remains absent for IBus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// A failed runtime query is evidence about observability, not evidence
    /// that the input method is inactive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_error: Option<String>,
    /// The daemon answered that no engine is selected (IBus reports "No
    /// global engine"). Keys then reach the client unchanged; this is an
    /// answer, not a failed query.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_engine: bool,
    /// IBus only: its own panel (`ibus-ui-gtk3`) is running. The panel is
    /// the process that grabs IBus's `triggers` hotkeys.
    #[serde(default, skip_serializing_if = "is_false")]
    pub panel: bool,
    /// Typed limits of what a passive diagnostic can reconstruct. Keeping
    /// these separate from `query_error` prevents an unreachable daemon from
    /// being mistaken for an inactive IME or for observed committed text.
    pub text_reconstruction: TextReconstruction,
}

/// Detect IBus/Fcitx5 without opening or changing an input-method connection.
pub fn detect() -> Vec<Detection> {
    let snapshot = crate::util::ProcessSnapshot::collect();
    detect_with_snapshot(&snapshot)
}

pub fn detect_with_snapshot(snapshot: &crate::util::ProcessSnapshot) -> Vec<Detection> {
    if crate::recording::env_var_os("SSH_CONNECTION").is_some()
        || crate::recording::env_var_os("SSH_TTY").is_some()
    {
        return Vec::new();
    }
    let ibus_panel = snapshot
        .comm_names()
        .iter()
        .any(|process| process == IBUS_PANEL_PROCESS);
    let mut found: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (variable, value) in [
        (
            "GTK_IM_MODULE",
            crate::recording::env_var("GTK_IM_MODULE").ok(),
        ),
        (
            "QT_IM_MODULE",
            crate::recording::env_var("QT_IM_MODULE").ok(),
        ),
        ("XMODIFIERS", crate::recording::env_var("XMODIFIERS").ok()),
    ] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let value_lower = value.to_ascii_lowercase();
        let engine = if value_lower.contains("fcitx") {
            "fcitx5"
        } else if value_lower.contains("ibus") {
            "ibus"
        } else {
            continue;
        };
        found
            .entry(engine.into())
            .or_default()
            .0
            .push(format!("{variable}={value}"));
    }
    for process in snapshot.comm_names() {
        let engine = process_engine(&process);
        if let Some(engine) = engine {
            found.entry(engine.into()).or_default().1.push(process);
        }
    }
    found
        .into_iter()
        .map(|(engine, (mut sources, mut processes))| {
            sources.sort();
            sources.dedup();
            processes.sort();
            processes.dedup();
            let runtime = query_runtime(&engine);
            let panel = ibus_panel && engine == "ibus";
            Detection {
                engine,
                sources,
                processes,
                active_engine: runtime.active_engine,
                state: runtime.state,
                query_error: runtime.error,
                no_engine: runtime.no_engine,
                panel,
                text_reconstruction: TextReconstruction::default(),
            }
        })
        .collect()
}

/// Describe the input-method stage without claiming it consumed a key. An IME
/// can turn several keys into committed text, but a terminal-side capture
/// cannot reconstruct the original Compose/dead-key sequence from that text.
pub fn inspect() -> LayerResult {
    let detections = detect();
    inspect_with_detections(&detections)
}

/// One hotkey list of a Fcitx5 component: its section name, Fcitx5's
/// built-in default and what the hotkey does.
struct HotkeyOption {
    name: &'static str,
    defaults: &'static [&'static str],
    action: &'static str,
}

/// A Fcitx5 component that acts on a key whatever the input-method state:
/// the daemon's global hotkeys, or an addon that registers a temporary mode
/// or a pre-engine key handler.
///
/// Verified against the Fcitx5 5.1.23 sources: `TempModeManager` offers every
/// key to each temporary mode's `triggerTempMode` at the Default phase of
/// `InputContextKeyEvent`, which runs for an inactive input context too (an
/// inactive context only has the plain keyboard engine, which accepts
/// nothing); `Instance` and `IMSelector` filter their hotkeys at the
/// PreInputMethod phase. Defaults are the `KeyListOption`s of
/// `modules/{quickphrase,clipboard,unicode,imselector}` and of
/// `instance.cpp`'s global config, and the defaults are compared with the
/// daemon's own (`lab/defaults/fcitx5-*.txt`, `tools/dump_defaults.py`). The Chinese addons' hotkeys (chttrans
/// `Hotkey`, fullwidth `Hotkey`, punctuation `Hotkey`) are not listed: their
/// handlers only act while an engine that registered their status-area
/// action is active, and that state is already reported as "may transform".
struct Fcitx5Component {
    /// What the layer calls this component's hotkeys.
    label: &'static str,
    /// The summary's subject: "Fcitx5 hotkey", "Fcitx5 QuickPhrase".
    subject: &'static str,
    /// Addon id for `[Behavior] DisabledAddons`; `None` for the daemon.
    addon: Option<&'static str>,
    /// File under `fcitx5/` (`$XDG_CONFIG_HOME/fcitx5`).
    file: &'static str,
    /// Prefix of each list's section in that file.
    prefix: &'static str,
    options: &'static [HotkeyOption],
}

const GLOBAL_FILE: &str = "config";

const FCITX5_COMPONENTS: &[Fcitx5Component] = &[
    Fcitx5Component {
        label: "global hotkeys",
        subject: "Fcitx5 hotkey",
        addon: None,
        file: GLOBAL_FILE,
        prefix: "Hotkey/",
        options: &[
            HotkeyOption {
                name: "TriggerKeys",
                defaults: &["Control+space", "Zenkaku_Hankaku", "Hangul"],
                action: "toggles the input method",
            },
            HotkeyOption {
                name: "AltTriggerKeys",
                defaults: &["Shift_L"],
                action: "temporarily toggles the input method",
            },
            HotkeyOption {
                name: "ActivateKeys",
                defaults: &["Hangul_Hanja"],
                action: "activates the input method",
            },
            HotkeyOption {
                name: "DeactivateKeys",
                defaults: &["Hangul_Romaja"],
                action: "deactivates the input method",
            },
            HotkeyOption {
                name: "EnumerateForwardKeys",
                defaults: &[],
                action: "switches to the next input method",
            },
            HotkeyOption {
                name: "EnumerateBackwardKeys",
                defaults: &[],
                action: "switches to the previous input method",
            },
            HotkeyOption {
                name: "EnumerateGroupForwardKeys",
                defaults: &["Super+space"],
                action: "switches to the next input method group",
            },
            HotkeyOption {
                name: "EnumerateGroupBackwardKeys",
                defaults: &["Shift+Super+space"],
                action: "switches to the previous input method group",
            },
            HotkeyOption {
                name: "TogglePreedit",
                defaults: &["Control+Alt+P"],
                action: "toggles embedded preedit",
            },
        ],
    },
    Fcitx5Component {
        label: "input method selector",
        subject: "Fcitx5 input method selector",
        addon: Some("imselector"),
        file: "conf/imselector.conf",
        prefix: "",
        options: &[
            HotkeyOption {
                name: "SwitchKey",
                defaults: &[],
                action: "switches to the n-th input method",
            },
            HotkeyOption {
                name: "SwitchKeyLocal",
                defaults: &[],
                action: "switches the n-th input method for this input context",
            },
            HotkeyOption {
                name: "TriggerKey",
                defaults: &[],
                action: "opens the input method selector",
            },
            HotkeyOption {
                name: "TriggerKeyLocal",
                defaults: &[],
                action: "opens the input method selector for this input context",
            },
        ],
    },
    Fcitx5Component {
        label: "QuickPhrase",
        subject: "Fcitx5 QuickPhrase",
        addon: Some("quickphrase"),
        file: "conf/quickphrase.conf",
        prefix: "",
        options: &[HotkeyOption {
            name: "TriggerKey",
            defaults: &["Super+grave", "Super+semicolon"],
            action: "opens quick phrase input",
        }],
    },
    Fcitx5Component {
        label: "clipboard",
        subject: "Fcitx5 clipboard",
        addon: Some("clipboard"),
        file: "conf/clipboard.conf",
        prefix: "",
        options: &[
            HotkeyOption {
                name: "TriggerKey",
                defaults: &["Control+semicolon"],
                action: "opens the clipboard history",
            },
            HotkeyOption {
                name: "PastePrimaryKey",
                defaults: &[],
                action: "pastes the primary selection",
            },
        ],
    },
    Fcitx5Component {
        label: "Unicode",
        subject: "Fcitx5 Unicode",
        addon: Some("unicode"),
        file: "conf/unicode.conf",
        prefix: "",
        options: &[
            HotkeyOption {
                name: "TriggerKey",
                defaults: &["Control+Alt+Shift+U"],
                action: "opens Unicode search",
            },
            HotkeyOption {
                name: "DirectUnicodeMode",
                defaults: &["Control+Shift+U"],
                action: "starts typing a Unicode code point in hex",
            },
        ],
    },
];

/// Hotkeys whose effect depends on the current state (activating an
/// already-active input method does nothing).
const FCITX5_STATE_DEPENDENT: &[&str] = &["ActivateKeys", "DeactivateKeys"];

/// The IBus panel process that binds `triggers`; it is a key grab, so it
/// takes the key whatever the engine state.
const IBUS_PANEL_PROCESS: &str = "ibus-ui-gtk3";
/// IBus's own default for `org.freedesktop.ibus.general.hotkey triggers`.
const IBUS_DEFAULT_TRIGGERS: &[&str] = &["<Super>space"];
const IBUS_SCHEMA: &str = "org.freedesktop.ibus.general.hotkey";
const IBUS_TRIGGERS_KEY: &str = "org.freedesktop.ibus.general.hotkey triggers";

/// A hotkey of Fcitx5 or IBus bound to the queried key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImeHotkey {
    /// "Fcitx5" or "IBus": the layer name.
    layer: &'static str,
    /// "Fcitx5 QuickPhrase", "Fcitx5 hotkey", "IBus hotkey".
    subject: &'static str,
    /// Section (Fcitx5) or key (IBus) holding the binding.
    section: String,
    name: &'static str,
    binding: String,
    action: &'static str,
    /// File that holds the binding; `None` for a built-in default or a
    /// settings-store value.
    file: Option<String>,
    /// Line in `file`.
    line: Option<u32>,
    tier: EvidenceTier,
}

/// Fcitx5's lists in one file: every `[Section]` with its numbered values
/// and their line numbers, keyed by the section path (`Hotkey/TriggerKeys`,
/// `TriggerKey`). A present but empty list (an empty section, or `Key=` with
/// no value, which Fcitx5 writes for an emptied list) clears the hotkey; an
/// absent one keeps the default.
fn fcitx5_lists(config: &str) -> BTreeMap<String, Vec<(String, u32)>> {
    let mut lists: BTreeMap<String, Vec<(String, u32)>> = BTreeMap::new();
    let mut section: Option<String> = None;
    for (number, line) in (1..).zip(config.lines()) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            lists.entry(name.to_owned()).or_default();
            section = Some(name.to_owned());
            continue;
        }
        let Some((index, value)) = line.split_once('=') else {
            continue;
        };
        let (index, value) = (index.trim(), value.trim().trim_matches('"'));
        if let (Some(section), Ok(_)) = (&section, index.parse::<u32>()) {
            if !value.is_empty() {
                lists
                    .entry(section.clone())
                    .or_default()
                    .push((value.to_owned(), number));
            }
        } else if value.is_empty() {
            let path = section
                .as_ref()
                .map_or_else(|| index.to_owned(), |section| format!("{section}/{index}"));
            lists.entry(path).or_default();
        }
    }
    lists
}

fn fcitx5_binding_matches(binding: &str, key: &KeyCombo) -> bool {
    if key.is_alone() {
        return key
            .alone_keysyms()
            .iter()
            .any(|keysym| keysym.eq_ignore_ascii_case(binding));
    }
    binding
        .parse::<KeyCombo>()
        .is_ok_and(|parsed| parsed == *key)
}

/// The contents of Fcitx5's configuration files (`None` for an unreadable
/// one), by their path under `fcitx5/`, and where that directory is.
#[derive(Debug, Clone, Default)]
struct Fcitx5Files {
    dir: String,
    contents: BTreeMap<&'static str, Option<String>>,
}

impl Fcitx5Files {
    fn read(dir: &std::path::Path) -> Self {
        Self {
            dir: dir.display().to_string(),
            contents: FCITX5_COMPONENTS
                .iter()
                .map(|component| {
                    let text = crate::recording::read_to_string(dir.join(component.file)).ok();
                    (component.file, text)
                })
                .collect(),
        }
    }

    fn path(&self, file: &str) -> String {
        format!("{}/{file}", self.dir)
    }

    fn text(&self, file: &str) -> Option<&str> {
        self.contents.get(file)?.as_deref()
    }
}

/// Whether Fcitx5 loads `addon`: `[Behavior] DisabledAddons` in
/// `fcitx5/config` wins over `EnabledAddons`, and `all` stands for every
/// addon (`InstancePrivate::overrideAddons`, `AddonManager::load`). `Err`
/// carries the line that disables it.
fn fcitx5_addon_enabled(
    global: &BTreeMap<String, Vec<(String, u32)>>,
    addon: &str,
) -> Result<(), u32> {
    let names = |list: &str| -> Vec<(&str, u32)> {
        global
            .get(list)
            .map(|values| {
                values
                    .iter()
                    .map(|(name, line)| (name.as_str(), *line))
                    .collect()
            })
            .unwrap_or_default()
    };
    let disabled = names("Behavior/DisabledAddons");
    let enabled = names("Behavior/EnabledAddons");
    let listed = |list: &[(&str, u32)], name: &str| list.iter().any(|(entry, _)| *entry == name);
    let force_enabled = |name: &str| {
        enabled
            .iter()
            .any(|(entry, _)| *entry == name && !listed(&disabled, entry))
    };
    if force_enabled("all") || force_enabled(addon) {
        return Ok(());
    }
    match disabled
        .iter()
        .find(|(entry, _)| *entry == "all" || *entry == addon)
    {
        Some((_, line)) => Err(*line),
        None => Ok(()),
    }
}

/// How one component answered for a key.
enum ComponentCheck {
    Match(ImeHotkey),
    /// Nothing matched; the string says what was read for the answer.
    NoMatch {
        source: String,
        tier: EvidenceTier,
    },
    /// The addon is disabled in `fcitx5/config`, so its hotkeys do nothing.
    Disabled {
        source: String,
    },
}

fn fcitx5_check(
    component: &Fcitx5Component,
    key: &KeyCombo,
    files: &Fcitx5Files,
    global: &BTreeMap<String, Vec<(String, u32)>>,
) -> ComponentCheck {
    let path = files.path(component.file);
    if let Some(addon) = component.addon {
        if let Err(line) = fcitx5_addon_enabled(global, addon) {
            return ComponentCheck::Disabled {
                source: format!(
                    "{} ({addon} is disabled by {}:{line})",
                    component.label,
                    files.path(GLOBAL_FILE)
                ),
            };
        }
    }
    let text = files.text(component.file);
    let configured = text.map(fcitx5_lists).unwrap_or_default();
    for option in component.options {
        let section = format!("{}{}", component.prefix, option.name);
        let bindings: Vec<(String, Option<u32>)> = match configured.get(&section) {
            Some(bindings) => bindings
                .iter()
                .map(|(binding, line)| (binding.clone(), Some(*line)))
                .collect(),
            None => option
                .defaults
                .iter()
                .map(|value| ((*value).to_owned(), None))
                .collect(),
        };
        if let Some((binding, line)) = bindings
            .into_iter()
            .find(|(binding, _)| fcitx5_binding_matches(binding, key))
        {
            return ComponentCheck::Match(ImeHotkey {
                layer: "Fcitx5",
                subject: component.subject,
                section,
                name: option.name,
                binding,
                action: option.action,
                file: Some(path),
                line,
                tier: if line.is_some() {
                    EvidenceTier::ConfigFile
                } else {
                    EvidenceTier::Default
                },
            });
        }
    }
    if text.is_some() {
        ComponentCheck::NoMatch {
            source: format!("{} (read {path})", component.label),
            tier: EvidenceTier::ConfigFile,
        }
    } else {
        ComponentCheck::NoMatch {
            source: format!("{} (built-in defaults; {path} not read)", component.label),
            tier: EvidenceTier::Default,
        }
    }
}

/// The IBus `triggers` accelerators: what `gsettings` reported, or IBus's
/// built-in default with the reason gsettings could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IbusTriggers {
    Live(Vec<String>),
    Default(String),
}

fn query_ibus_triggers() -> IbusTriggers {
    match crate::layers::gsettings::run_list(&["get", IBUS_SCHEMA, "triggers"]) {
        Ok(output) => IbusTriggers::Live(crate::layers::gsettings::quoted_values(&output)),
        Err(error) => IbusTriggers::Default(error),
    }
}

/// The IBus `triggers` binding for `key`. Current IBus binds `triggers`
/// (GTK accelerator syntax) in its panel (`ui/gtk3/panel.vala`,
/// `bind_switch_shortcut`) and reads none of the schema's other hotkey keys
/// (`trigger`, `next-engine`, `enable-unconditional`, ...).
fn ibus_hotkey(key: &KeyCombo, triggers: &IbusTriggers) -> Option<ImeHotkey> {
    let (accelerators, tier) = match triggers {
        IbusTriggers::Live(values) => (values.clone(), EvidenceTier::LiveQuery),
        IbusTriggers::Default(_) => (
            IBUS_DEFAULT_TRIGGERS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            EvidenceTier::Default,
        ),
    };
    accelerators
        .into_iter()
        .find(|accelerator| {
            crate::layers::gsettings::parse_accelerator(accelerator)
                .is_some_and(|parsed| parsed == *key)
        })
        .map(|binding| ImeHotkey {
            layer: "IBus",
            subject: "IBus hotkey",
            section: "triggers".into(),
            name: "triggers",
            binding,
            action: "switches the input method",
            file: None,
            line: None,
            tier,
        })
}

/// Describe the input-method stage for `key`: the daemon's runtime state
/// plus the hotkeys Fcitx5 (global and addon) and IBus take whatever that
/// state is.
pub fn inspect_key(key: &KeyCombo, detections: &[Detection]) -> LayerResult {
    let fcitx5 = detections
        .iter()
        .any(|detection| detection.engine == "fcitx5")
        .then(|| crate::util::config_base().map(|base| Fcitx5Files::read(&base.join("fcitx5"))))
        .flatten();
    let ibus = detections
        .iter()
        .any(|detection| detection.engine == "ibus")
        .then(query_ibus_triggers);
    let x11 = crate::recording::env_var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "x11");
    inspect_with_hotkey_sources(key, detections, fcitx5.as_ref(), ibus.as_ref(), x11)
}

/// As [`inspect_key`], with the Fcitx5 files and IBus triggers already read;
/// `None` when that input method was not detected, and whether the session is X11.
fn inspect_with_hotkey_sources(
    key: &KeyCombo,
    detections: &[Detection],
    fcitx5: Option<&Fcitx5Files>,
    ibus: Option<&IbusTriggers>,
    x11: bool,
) -> LayerResult {
    let (mut result, forwards) = state_verdict(detections);
    let mut checked = Vec::new();
    let mut tiers = Vec::new();
    let mut sources = Vec::new();
    if let Some(files) = fcitx5 {
        let global = files
            .text(GLOBAL_FILE)
            .map(fcitx5_lists)
            .unwrap_or_default();
        let mut component_sources = Vec::new();
        for component in FCITX5_COMPONENTS {
            match fcitx5_check(component, key, files, &global) {
                ComponentCheck::Match(hotkey) => {
                    return hotkey_result(hotkey, detections, result);
                }
                ComponentCheck::NoMatch { source, tier } => {
                    component_sources.push(source);
                    tiers.push(tier);
                }
                ComponentCheck::Disabled { source } => component_sources.push(source),
            }
        }
        checked.push(format!(
            "no Fcitx5 hotkey matches; checked {}",
            component_sources.join(", ")
        ));
        sources.extend(component_sources);
    }
    if let Some(triggers) = ibus {
        let panel = detections
            .iter()
            .any(|detection| detection.engine == "ibus" && detection.panel);
        if let Some(hotkey) = ibus_hotkey(key, triggers) {
            return ibus_result(hotkey, panel, x11, triggers, result);
        }
        match triggers {
            IbusTriggers::Live(_) => {
                checked.push(format!("no IBus hotkey matches (read {IBUS_TRIGGERS_KEY})"));
                sources.push(format!("IBus {IBUS_TRIGGERS_KEY}"));
                tiers.push(EvidenceTier::LiveQuery);
            }
            IbusTriggers::Default(error) => {
                checked.push(format!(
                    "no IBus hotkey matches (built-in default; {IBUS_TRIGGERS_KEY} unreadable: {error})"
                ));
                sources.push(format!("IBus built-in default {IBUS_TRIGGERS_KEY}"));
                tiers.push(EvidenceTier::Default);
            }
        }
    }
    result.details.extend(checked);
    // A pass is only as strong as the weakest thing it rests on: the daemon
    // answered about its state, but the hotkeys came from files or defaults.
    // Every detected daemon's hotkeys must have been read for it to be one.
    if let Some(state) = forwards {
        let covered = detections
            .iter()
            .all(|detection| match detection.engine.as_str() {
                "fcitx5" => fcitx5.is_some(),
                "ibus" => ibus.is_some(),
                _ => false,
            });
        if covered {
            let tier = tiers
                .into_iter()
                .chain([state.tier])
                .max_by_key(tier_rank)
                .unwrap_or(state.tier);
            let mut all = state.sources;
            all.extend(sources);
            result = result.confirmed_pass(Checked::new(tier, all));
        } else {
            result
                .details
                .push("the hotkeys of a detected input method could not be read".into());
        }
    }
    result
}

fn tier_rank(tier: &EvidenceTier) -> u8 {
    match tier {
        EvidenceTier::Observed => 0,
        EvidenceTier::LiveQuery => 1,
        EvidenceTier::ConfigFile => 2,
        EvidenceTier::Default => 3,
    }
}

fn binding_detail(hotkey: &ImeHotkey) -> String {
    let binding = format!("binding: [{}] {}", hotkey.section, hotkey.binding);
    match (&hotkey.file, hotkey.line) {
        (Some(file), Some(line)) => format!("{binding} ({file}:{line})"),
        (Some(file), None) => format!("{binding} (Fcitx5 default; {file} does not set it)"),
        (None, _) if hotkey.layer == "IBus" => format!(
            "{binding} ({IBUS_TRIGGERS_KEY}{})",
            if hotkey.tier == EvidenceTier::Default {
                "; IBus default"
            } else {
                ""
            }
        ),
        (None, _) => binding,
    }
}

/// The result for a hotkey Fcitx5 binds to the key. The daemon's state is
/// that of its most recently focused input context, which need not be the
/// terminal's, so the key is taken only if the focused window talks to Fcitx5:
/// uncertain, never consumed.
fn hotkey_result(hotkey: ImeHotkey, detections: &[Detection], base: LayerResult) -> LayerResult {
    let mut details = vec![binding_detail(&hotkey)];
    details.extend(base.details);
    if FCITX5_STATE_DEPENDENT.contains(&hotkey.name) {
        details.push(
            "this hotkey does nothing when the input method is already in the state it sets".into(),
        );
    }
    details.push(match fcitx5_focus_note(detections) {
        Some(note) => note,
        None => {
            "Fcitx5 takes this key only if the focused window is one of its input contexts".into()
        }
    });
    let summary = format!(
        "{} {} {}, even while the input method is inactive",
        hotkey.subject, hotkey.name, hotkey.action
    );
    let source = match (&hotkey.file, hotkey.line) {
        (Some(file), Some(line)) => {
            Some(crate::layers::SourceLocation::new(file.clone(), Some(line)))
        }
        _ => None,
    };
    LayerResult::new(
        hotkey.layer,
        LayerId::Ime,
        Outcome::HandledUncertain,
        summary,
        details,
    )
    .with_binding(crate::layers::BindingEvidence::action(
        format!("{} ({})", hotkey.name, hotkey.action),
        crate::layers::BindingScope::Unknown,
        source,
    ))
    .with_tier(hotkey.tier)
}

/// What the daemon's state says about whether the focused window is attached
/// to it. `State` answers for the most recently focused input context.
fn fcitx5_focus_note(detections: &[Detection]) -> Option<String> {
    let detection = detections
        .iter()
        .find(|detection| detection.engine == "fcitx5")?;
    Some(match detection.state.as_deref() {
        Some("closed") if detection.active_engine.is_none() => {
            "Fcitx5 reports no focused input context, so the focused window may not use it".into()
        }
        Some(state) => format!(
            "Fcitx5 takes this key only if the focused window is one of its input contexts; its reported state ({state}) is that of the most recently focused one, which may belong to another window"
        ),
        None => "Fcitx5 takes this key only if the focused window is one of its input contexts; its state could not be read".into(),
    })
}

fn ibus_result(
    hotkey: ImeHotkey,
    panel: bool,
    x11: bool,
    triggers: &IbusTriggers,
    base: LayerResult,
) -> LayerResult {
    let mut details = vec![binding_detail(&hotkey)];
    details.extend(base.details);
    if let IbusTriggers::Default(error) = triggers {
        details.push(format!("{IBUS_TRIGGERS_KEY} unreadable: {error}"));
    }
    // `triggers` is a key grab made by the IBus panel on X11; on Wayland it
    // needs the compositor to honor IBus's global shortcuts, and another
    // panel (GNOME Shell's, for one) does not bind it at all.
    let certain = panel && x11 && !matches!(triggers, IbusTriggers::Default(_));
    details.push(if !panel {
        format!(
            "{IBUS_PANEL_PROCESS}, the panel that binds this key, was not seen running; another panel may or may not bind it"
        )
    } else if !x11 {
        format!(
            "{IBUS_PANEL_PROCESS} binds this key as a global shortcut that only an X11 session or a compositor honoring IBus's global shortcuts delivers"
        )
    } else {
        format!("{IBUS_PANEL_PROCESS} grabs this key, so IBus takes it whatever its engine state")
    });
    let summary = format!(
        "{} {} {}, even while no engine is active",
        hotkey.subject, hotkey.name, hotkey.action
    );
    LayerResult::new(
        hotkey.layer,
        LayerId::Ime,
        if certain {
            Outcome::Consumed
        } else {
            Outcome::HandledUncertain
        },
        summary,
        details,
    )
    .with_binding(crate::layers::BindingEvidence::action(
        format!("{} ({})", hotkey.name, hotkey.action),
        crate::layers::BindingScope::Unknown,
        None,
    ))
    .with_tier(hotkey.tier)
}

/// Describe a previously collected input-method snapshot without rerunning
/// process scans or runtime queries. Hotkeys are not considered; see
/// [`inspect_key`]. A pass here has no evidence tier: the runtime state does
/// not show that no hotkey takes the key.
pub fn inspect_with_detections(detections: &[Detection]) -> LayerResult {
    let (mut result, forwards) = state_verdict(detections);
    if forwards.is_some() {
        result
            .details
            .push("input-method hotkeys were not checked".into());
    }
    result
}

/// The runtime state's verdict. A state that forwards keys is not a pass
/// yet: the daemon takes its own hotkeys in any state. It comes back as an
/// uncertain result with the [`Checked`] evidence the state contributes, and
/// [`inspect_with_hotkey_sources`] confirms the pass once the hotkeys of
/// every detected daemon were read.
fn state_verdict(detections: &[Detection]) -> (LayerResult, Option<Checked>) {
    if detections.is_empty() {
        return (
            LayerResult::pass(
                "Input method",
                LayerId::Ime,
                "no supported input method was detected",
                Vec::new(),
                Checked::new(
                    EvidenceTier::LiveQuery,
                    [
                        "GTK_IM_MODULE, QT_IM_MODULE and XMODIFIERS",
                        "running processes (Fcitx5, IBus)",
                    ],
                ),
            ),
            None,
        );
    }

    let mut details = Vec::new();
    let mut transforming = false;
    let mut unknown = false;
    let mut forwarding_reasons = Vec::new();
    for detection in detections {
        details.push(format!("detected: {}", detection.engine));
        details.extend(
            detection
                .sources
                .iter()
                .map(|source| format!("source: {source}")),
        );
        details.extend(
            detection
                .processes
                .iter()
                .map(|process| format!("process: {process}")),
        );
        if let Some(active_engine) = &detection.active_engine {
            details.push(format!("active engine: {active_engine}"));
        }
        if detection.no_engine {
            details.push("active engine: none (the daemon reports no engine is set)".into());
        }
        if let Some(state) = &detection.state {
            if state == "closed" && detection.active_engine.is_none() {
                details.push("runtime state: closed (no focused input context)".into());
            } else {
                details.push(format!("runtime state: {state}"));
            }
        }
        if let Some(error) = &detection.query_error {
            details.push(format!("runtime query unavailable: {error}"));
        }
        match runtime_mode(detection) {
            RuntimeMode::Forwarding(reason) => forwarding_reasons.push(reason),
            RuntimeMode::Transforming => transforming = true,
            RuntimeMode::Unknown => unknown = true,
        }
    }
    details.push(
        detections
            .iter()
            .map(|detection| detection.text_reconstruction.detail())
            .collect::<Vec<_>>()
            .join("; "),
    );
    // Only a daemon's runtime state answers for it; environment variables
    // and process names merely show that one is present.
    let tier = detections
        .iter()
        .any(|detection| detection.state.is_some())
        .then_some(EvidenceTier::LiveQuery);
    if transforming {
        return (
            LayerResult::new(
                "Input method",
                LayerId::Ime,
                Outcome::UncertainContinues,
                "an active input method may transform this input before text is committed",
                details,
            )
            .with_optional_tier(tier),
            None,
        );
    }
    if unknown {
        return (
            LayerResult::new(
            "Input method",
            LayerId::Ime,
            Outcome::UncertainContinues,
            if detections
                .iter()
                .any(|detection| detection.query_error.is_some())
            {
                "input method was detected but its runtime API is unreachable; transformation state remains unknown"
            } else {
                "an input-method context may transform this input before text is committed"
            },
            details,
        )
        .with_optional_tier(tier),
            None,
        );
    }
    // Every detected daemon reported a runtime state that forwards keys
    // unchanged. The state says nothing about the daemon's own hotkeys
    // (the trigger key, addon keys), which it takes in any state, so this is
    // not a pass yet: `inspect_with_hotkey_sources` adds the hotkey checks
    // and confirms it with the weakest tier they used.
    let state_checked = Checked::new(
        EvidenceTier::LiveQuery,
        detections
            .iter()
            .map(|detection| format!("{} runtime state", detection.engine)),
    );
    (
        LayerResult::new(
            "Input method",
            LayerId::Ime,
            Outcome::UncertainContinues,
            format!(
                "input method was detected but {}; keys pass through unchanged",
                forwarding_reasons.join(" and ")
            ),
            details,
        ),
        Some(state_checked),
    )
}

enum RuntimeMode {
    /// Runtime state proves the daemon forwards keys; the payload says why.
    Forwarding(String),
    /// An input method is engaged and may compose or transform keys.
    Transforming,
    /// The runtime state could not be read.
    Unknown,
}

fn runtime_mode(detection: &Detection) -> RuntimeMode {
    if detection.no_engine {
        return RuntimeMode::Forwarding("no engine is set".into());
    }
    if let Some(state) = detection
        .state
        .as_deref()
        .filter(|state| matches!(*state, "inactive" | "closed"))
    {
        return RuntimeMode::Forwarding(format!("is {state}"));
    }
    match detection.active_engine.as_deref() {
        Some(engine) if is_keyboard_engine(engine) => RuntimeMode::Forwarding(format!(
            "its active engine {engine} is a plain keyboard layout"
        )),
        Some(_)
            if detection.state.as_deref() != Some("active") && detection.query_error.is_some() =>
        {
            RuntimeMode::Unknown
        }
        Some(_) => RuntimeMode::Transforming,
        None if detection.state.as_deref() == Some("active") => RuntimeMode::Transforming,
        None => RuntimeMode::Unknown,
    }
}

/// Engines that only apply an XKB layout: Fcitx5's `keyboard-<layout>` and
/// IBus's `xkb:<layout>:<variant>:<language>`.
fn is_keyboard_engine(engine: &str) -> bool {
    engine.starts_with("keyboard-") || engine.starts_with("xkb:")
}

#[derive(Default)]
struct RuntimeQuery {
    active_engine: Option<String>,
    state: Option<String>,
    error: Option<String>,
    no_engine: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Query documented read-only input-method APIs only after an input-method
/// process or session variable identified the daemon. These queries never
/// switch, activate, or deactivate an input method.
fn query_runtime(engine: &str) -> RuntimeQuery {
    match engine {
        "ibus" => query_ibus(),
        "fcitx5" => query_fcitx5(),
        _ => RuntimeQuery::default(),
    }
}

fn query_ibus() -> RuntimeQuery {
    let mut command = Command::new("ibus");
    // Untranslated replies, so "No engine is set" below is recognized.
    command.arg("engine").env("LC_ALL", "C");
    let output = match command::output(&mut command) {
        Ok(output) => output,
        Err(error) => {
            return RuntimeQuery {
                error: Some(format!("ibus: {error}")),
                ..RuntimeQuery::default()
            };
        }
    };
    if output.status.success() {
        return RuntimeQuery {
            active_engine: nonempty_line(&String::from_utf8_lossy(&output.stdout)),
            ..RuntimeQuery::default()
        };
    }
    // The daemon answered and selected nothing: `ibus engine` exits 1 with
    // "No engine is set." (stdout) and "No global engine" (stderr). That is a
    // reply, not an unreachable API.
    if ibus_reports_no_engine(&output) {
        return RuntimeQuery {
            no_engine: true,
            ..RuntimeQuery::default()
        };
    }
    RuntimeQuery {
        error: Some(command_failure("ibus", &output)),
        ..RuntimeQuery::default()
    }
}

fn ibus_reports_no_engine(output: &std::process::Output) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("No engine is set") || stderr.contains("No global engine")
}

fn query_fcitx5() -> RuntimeQuery {
    // Some distro builds of fcitx5-remote throw an uncaught C++ exception
    // when the session bus cannot be created. Probe the bus and query the
    // documented controller methods directly so a diagnostic never needs to
    // launch that crash-prone helper.
    if let Err(error) = fcitx5_service_available() {
        return RuntimeQuery {
            error: Some(error),
            ..RuntimeQuery::default()
        };
    }

    let active_engine = match run_fcitx5_method("CurrentInputMethod") {
        // Fcitx5 answers "" when no input context is focused; State then
        // reports 0 (closed), so keep querying instead of giving up.
        Ok(output) => match parse_dbus_string(&output.stdout) {
            Some(engine) => (!engine.is_empty()).then_some(engine),
            None => {
                return RuntimeQuery {
                    error: Some("Fcitx5 CurrentInputMethod returned no D-Bus string value".into()),
                    ..RuntimeQuery::default()
                };
            }
        },
        Err(error) => {
            return RuntimeQuery {
                error: Some(error),
                ..RuntimeQuery::default()
            };
        }
    };

    match run_fcitx5_method("State") {
        Ok(output) => {
            let state =
                parse_dbus_integer(&output.stdout).and_then(|code| fcitx5_state(Some(code)));
            let error = if state.is_none() {
                Some(command_failure("dbus-send", &output))
            } else {
                None
            };
            RuntimeQuery {
                active_engine,
                state,
                error,
                ..RuntimeQuery::default()
            }
        }
        Err(error) => RuntimeQuery {
            active_engine,
            error: Some(error),
            ..RuntimeQuery::default()
        },
    }
}

fn run_fcitx5_method(method: &str) -> Result<std::process::Output, String> {
    let mut command = Command::new("dbus-send");
    command.args([
        "--session",
        "--print-reply=literal",
        "--dest=org.fcitx.Fcitx5",
        "/controller",
    ]);
    command.arg(format!("org.fcitx.Fcitx.Controller1.{method}"));
    let output = command::output(&mut command)
        .map_err(|error| format!("dbus-send Fcitx5 {method}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_failure("dbus-send", &output))
    }
}

fn fcitx5_service_available() -> Result<(), String> {
    if crate::recording::env_var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return Err("D-Bus session address is unset; skipped Fcitx5 runtime query".into());
    }

    let mut command = Command::new("dbus-send");
    command.args([
        "--session",
        "--print-reply=literal",
        "--dest=org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.ListNames",
    ]);
    let output = command::output(&mut command).map_err(|error| {
        format!("D-Bus preflight failed; skipped Fcitx5 runtime query: {error}")
    })?;
    if !output.status.success() {
        return Err(format!(
            "D-Bus session is unavailable; skipped Fcitx5 runtime query ({})",
            command_failure("dbus-send", &output)
        ));
    }

    let names = String::from_utf8_lossy(&output.stdout);
    if !has_fcitx5_service(&names) {
        return Err(
            "org.fcitx.Fcitx5 is not registered on the session bus; skipped Fcitx5 runtime query"
                .into(),
        );
    }
    Ok(())
}

fn has_fcitx5_service(names: &str) -> bool {
    names
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        })
        .any(|name| name == "org.fcitx.Fcitx5")
}

fn command_failure(program: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!("{program} exited with {}", output.status)
    } else {
        format!("{program} exited with {}: {detail}", output.status)
    }
}

fn nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_dbus_string(value: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(value);
    // `--print-reply=literal` prints an empty string as blank output.
    let Some(line) = value.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return Some(String::new());
    };
    let payload = if let Some(payload) = line.strip_prefix("string ") {
        payload.trim()
    } else {
        // `--print-reply=literal` emits a bare string value (for example
        // `keyboard-us`) but keeps scalar non-string replies typed. Reject
        // those typed values so a malformed State reply cannot be mistaken
        // for an input-method name.
        let type_name = line.split_whitespace().next()?;
        if matches!(
            type_name,
            "array"
                | "boolean"
                | "byte"
                | "double"
                | "int16"
                | "int32"
                | "int64"
                | "object"
                | "signature"
                | "uint16"
                | "uint32"
                | "uint64"
                | "variant"
        ) {
            return None;
        }
        line
    };
    let inner = payload
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(payload);
    Some(inner.replace("\\\\", "\\").replace("\\\"", "\""))
}

fn parse_dbus_integer(value: &[u8]) -> Option<i32> {
    let text = String::from_utf8_lossy(value);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut fields = line.split_whitespace();
    let first = fields.next()?;
    let payload = match first {
        "byte" | "int16" | "int32" | "int64" | "uint16" | "uint32" | "uint64" => fields.next()?,
        _ if fields.next().is_none() => first,
        _ => return None,
    };
    fields
        .next()
        .is_none()
        .then(|| payload.parse().ok())
        .flatten()
}

fn fcitx5_state(code: Option<i32>) -> Option<String> {
    match code {
        Some(0) => Some("closed".into()),
        Some(1) => Some("inactive".into()),
        Some(2) => Some("active".into()),
        _ => None,
    }
}

fn process_engine(process: &str) -> Option<&'static str> {
    if process.eq_ignore_ascii_case("fcitx5") {
        Some("fcitx5")
    } else if process.eq_ignore_ascii_case("ibus-daemon") {
        Some("ibus")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::defaults_fixtures;

    /// Fcitx5's key-list options with a candidate window or an input method
    /// engine in the loop: they act only while one is shown, so they are not
    /// modelled (see [`Fcitx5Component`]).
    const FCITX5_UNMODELLED: &[&str] = &[
        "global/PrevPage",
        "global/NextPage",
        "global/PrevCandidate",
        "global/NextCandidate",
    ];

    /// The hotkey defaults are the ones the Fcitx5 daemon itself reports
    /// (`lab/defaults/fcitx5-*.txt`, read over its D-Bus controller), and a
    /// key-list option the daemon has that is neither embedded nor listed in
    /// `FCITX5_UNMODELLED` is a new hotkey whykey does not know.
    #[test]
    fn fcitx5_hotkey_defaults_equal_the_daemons() {
        let Some(fixture) = defaults_fixtures::load("fcitx5") else {
            return;
        };
        assert_eq!(fixture.version, "5.1.23");
        let mut embedded = Vec::new();
        for component in FCITX5_COMPONENTS {
            let config = component.addon.unwrap_or("global");
            for option in component.options {
                embedded.push((
                    format!("{config}/{}", option.name),
                    option.defaults.join(" "),
                ));
            }
        }
        let modelled = defaults_fixtures::Fixture {
            version: fixture.version.clone(),
            rows: fixture
                .rows
                .iter()
                .filter(|(name, _)| !FCITX5_UNMODELLED.contains(&name.as_str()))
                .cloned()
                .collect(),
        };
        defaults_fixtures::assert_same_rows("Fcitx5 hotkey defaults", &embedded, &modelled);
    }

    use super::*;
    use crate::layers::EvidenceTier;

    #[test]
    fn detection_shape_is_stable_and_serializable() {
        let detection = Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: Vec::new(),
            active_engine: Some("xkb:us::eng".into()),
            state: None,
            query_error: None,
            no_engine: false,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        };
        let value = serde_json::to_value(&detection).unwrap();
        assert_eq!(value["engine"], "ibus");
        assert!(value["sources"].is_array());
        assert_eq!(value["active_engine"], "xkb:us::eng");
        assert!(value.get("state").is_none());
        assert_eq!(
            value["text_reconstruction"]["committed_text"],
            "conditional"
        );
        assert_eq!(
            value["text_reconstruction"]["compose_dead_key"],
            "unobserved"
        );
        assert_eq!(
            value["text_reconstruction"]["application_preedit"],
            "unobserved"
        );
    }

    #[test]
    fn only_daemon_processes_trigger_runtime_queries() {
        assert_eq!(process_engine("fcitx5"), Some("fcitx5"));
        assert_eq!(process_engine("FCITX5"), Some("fcitx5"));
        assert_eq!(process_engine("fcitx5-remote"), None);
        assert_eq!(process_engine("ibus-daemon"), Some("ibus"));
        assert_eq!(process_engine("ibus-ui-gtk3"), None);
    }

    #[test]
    fn fcitx5_exit_codes_are_read_only_state_observations() {
        assert_eq!(fcitx5_state(Some(0)).as_deref(), Some("closed"));
        assert_eq!(fcitx5_state(Some(1)).as_deref(), Some("inactive"));
        assert_eq!(fcitx5_state(Some(2)).as_deref(), Some("active"));
        assert_eq!(fcitx5_state(Some(3)), None);
    }

    fn fcitx5(active_engine: &str, state: &str) -> Detection {
        Detection {
            engine: "fcitx5".into(),
            sources: vec!["GTK_IM_MODULE=fcitx".into()],
            processes: vec!["fcitx5".into()],
            active_engine: Some(active_engine.into()),
            state: Some(state.into()),
            query_error: None,
            no_engine: false,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        }
    }

    /// The Fcitx5 files a test host has: `global` is `fcitx5/config`,
    /// `addons` are `(addon, contents)` for `fcitx5/conf/<addon>.conf`.
    fn files(global: Option<&str>, addons: &[(&str, &str)]) -> Fcitx5Files {
        let mut files = Fcitx5Files {
            dir: "/cfg/fcitx5".into(),
            ..Fcitx5Files::default()
        };
        for component in FCITX5_COMPONENTS {
            let text = if component.addon.is_none() {
                global.map(str::to_owned)
            } else {
                addons
                    .iter()
                    .find(|(addon, _)| Some(*addon) == component.addon)
                    .map(|(_, text)| (*text).to_owned())
            };
            files.contents.insert(component.file, text);
        }
        files
    }

    fn parse_key(key: &str) -> KeyCombo {
        if let Some(modifier) = key.strip_suffix(" --alone") {
            KeyCombo::parse_alone(modifier).unwrap()
        } else {
            key.parse().unwrap()
        }
    }

    fn with_files(key: &str, detection: Detection, files: &Fcitx5Files) -> LayerResult {
        inspect_with_hotkey_sources(&parse_key(key), &[detection], Some(files), None, false)
    }

    fn with_config(key: &str, detection: Detection, config: Option<&str>) -> LayerResult {
        with_files(key, detection, &files(config, &[]))
    }

    #[test]
    fn inactive_fcitx_still_takes_its_default_toggle_preedit_hotkey() {
        let config =
            "[Hotkey/TriggerKeys]\n0=Zenkaku_Hankaku\n\n[Behavior]\nActiveByDefault=False\n";
        let result = with_config(
            "ctrl+alt+p",
            fcitx5("keyboard-us", "inactive"),
            Some(config),
        );

        assert_eq!(result.layer, "Fcitx5");
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::Default));
        assert!(result.summary.contains("TogglePreedit"));
        assert!(result.details[0].contains("Fcitx5 default"));
        // An unreadable config falls back to the same defaults.
        let unread = with_config("ctrl+alt+p", fcitx5("keyboard-us", "inactive"), None);
        assert_eq!(unread.outcome, Outcome::HandledUncertain);
        assert_eq!(unread.summary, result.summary);
    }

    #[test]
    fn configured_fcitx5_hotkeys_replace_defaults_and_cite_the_line() {
        let config =
            "[Hotkey/TriggerKeys]\n0=Control+Shift+space\n\n[Hotkey/EnumerateGroupForwardKeys]\n";
        let result = with_config("ctrl+shift+space", fcitx5("pinyin", "active"), Some(config));
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::ConfigFile));
        let source = result.binding.as_ref().unwrap().source.as_ref().unwrap();
        assert_eq!(
            (source.file.as_str(), source.line),
            ("/cfg/fcitx5/config", Some(2))
        );

        // The configured list replaces Control+space; the empty section
        // clears Super+space.
        for key in ["ctrl+space", "super+space"] {
            let result = with_config(key, fcitx5("keyboard-us", "inactive"), Some(config));
            assert_eq!(result.outcome, Outcome::Pass, "{key}");
            assert!(result.details.iter().any(|detail| detail.starts_with(
                "no Fcitx5 hotkey matches; checked global hotkeys (read /cfg/fcitx5/config)"
            )));
        }
    }

    #[test]
    fn a_fcitx5_hotkey_is_never_certain_because_the_state_is_another_windows() {
        // `State` answers for the most recently focused input context, which
        // need not be the terminal's.
        let mut closed = fcitx5("keyboard-us", "closed");
        closed.active_engine = None;
        let result = with_config("ctrl+alt+p", closed, None);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("no focused input context"))
        );
        let inactive = with_config("ctrl+alt+p", fcitx5("keyboard-us", "inactive"), None);
        assert_eq!(inactive.outcome, Outcome::HandledUncertain);
        assert!(inactive.details.iter().any(|detail| {
            detail.contains("(inactive) is that of the most recently focused one")
        }));
        let config = "[Hotkey/ActivateKeys]\n0=Henkan\n";
        let activate = with_config("henkan", fcitx5("keyboard-us", "inactive"), Some(config));
        assert_eq!(activate.outcome, Outcome::HandledUncertain);
        assert!(
            activate
                .details
                .iter()
                .any(|detail| detail.contains("already in the state it sets"))
        );
    }

    #[test]
    fn modifier_only_fcitx5_hotkeys_match_a_lone_modifier() {
        let result = with_config("shift --alone", fcitx5("keyboard-us", "inactive"), None);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert!(result.summary.contains("AltTriggerKeys"));
        assert_eq!(
            with_config("shift+a", fcitx5("keyboard-us", "inactive"), None).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn fcitx5_quickphrase_trigger_is_read_from_its_addon_file() {
        // The user's own host: quickphrase.conf replaces both defaults with
        // Super+semicolon.
        let quickphrase = "[TriggerKey]\n0=Super+semicolon\n";
        let files = files(
            Some("[Behavior]\nActiveByDefault=False\n"),
            &[("quickphrase", quickphrase)],
        );
        let result = with_files("super+semicolon", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(result.layer, "Fcitx5");
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::ConfigFile));
        assert!(
            result
                .summary
                .starts_with("Fcitx5 QuickPhrase TriggerKey opens quick phrase input")
        );
        assert_eq!(
            result.details[0],
            "binding: [TriggerKey] Super+semicolon (/cfg/fcitx5/conf/quickphrase.conf:2)"
        );
        let source = result.binding.as_ref().unwrap().source.as_ref().unwrap();
        assert_eq!(
            (source.file.as_str(), source.line),
            ("/cfg/fcitx5/conf/quickphrase.conf", Some(2))
        );

        // The configured list replaces the built-in Super+grave.
        let grave = with_files("super+grave", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(grave.outcome, Outcome::Pass);
    }

    #[test]
    fn fcitx5_addon_defaults_apply_without_the_addon_file() {
        for (key, name) in [
            ("super+grave", "TriggerKey"),
            ("ctrl+semicolon", "TriggerKey"),
            ("ctrl+alt+shift+u", "TriggerKey"),
            ("ctrl+shift+u", "DirectUnicodeMode"),
        ] {
            let result = with_config(key, fcitx5("keyboard-us", "inactive"), None);
            assert_eq!(result.outcome, Outcome::HandledUncertain, "{key}");
            assert_eq!(result.tier, Some(EvidenceTier::Default), "{key}");
            assert!(result.summary.contains(name), "{key}: {}", result.summary);
            assert!(
                result.details[0].contains("Fcitx5 default; /cfg/fcitx5/conf/"),
                "{key}: {}",
                result.details[0]
            );
            assert!(result.binding.as_ref().unwrap().source.is_none());
        }
    }

    #[test]
    fn an_emptied_addon_list_clears_the_hotkey() {
        // Fcitx5 writes an emptied list as `TriggerKey=` at the top level
        // (the clipboard.conf of a real host), and a bare section for the
        // same thing in a hand-written file.
        for text in ["TriggerKey=\nPastePrimaryKey=\n", "[TriggerKey]\n"] {
            let files = files(None, &[("clipboard", text)]);
            let result = with_files("ctrl+semicolon", fcitx5("keyboard-us", "inactive"), &files);
            assert_eq!(result.outcome, Outcome::Pass, "{text:?}");
        }
        let primary = files(
            None,
            &[("clipboard", "[PastePrimaryKey]\n0=Control+Shift+v\n")],
        );
        let result = with_files("ctrl+shift+v", fcitx5("keyboard-us", "inactive"), &primary);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert!(
            result
                .summary
                .contains("PastePrimaryKey pastes the primary selection")
        );
    }

    #[test]
    fn imselector_hotkeys_have_no_default_and_are_read_when_set() {
        let none = with_config("f9", fcitx5("keyboard-us", "inactive"), None);
        assert_eq!(none.outcome, Outcome::Pass);
        let files = files(
            None,
            &[(
                "imselector",
                "[SwitchKey]\n0=F9\n1=F10\n\n[TriggerKeyLocal]\n0=Control+Alt+i\n",
            )],
        );
        let switch = with_files("f10", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(switch.outcome, Outcome::HandledUncertain);
        assert!(
            switch
                .summary
                .contains("SwitchKey switches to the n-th input method")
        );
        let local = with_files("ctrl+alt+i", fcitx5("keyboard-us", "inactive"), &files);
        assert!(local.summary.contains("TriggerKeyLocal"));
    }

    #[test]
    fn disabled_fcitx5_addons_do_not_take_their_hotkeys() {
        let config = "[Behavior]\n\n[Behavior/DisabledAddons]\n0=quickphrase\n1=unicode\n";
        let files = files(Some(config), &[]);
        let quickphrase = with_files("super+grave", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(quickphrase.outcome, Outcome::Pass);
        assert!(quickphrase.details.iter().any(|detail| {
            detail.contains("QuickPhrase (quickphrase is disabled by /cfg/fcitx5/config:4)")
        }));
        let unicode = with_files("ctrl+shift+u", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(unicode.outcome, Outcome::Pass);
        // An addon that is not disabled keeps its hotkey.
        let clipboard = with_files("ctrl+semicolon", fcitx5("keyboard-us", "inactive"), &files);
        assert_eq!(clipboard.outcome, Outcome::HandledUncertain);
    }

    #[test]
    fn fcitx5_addon_override_lists_follow_the_daemon_rules() {
        let parse = |text: &str| fcitx5_lists(text);
        let enabled = |text: &str, addon: &str| fcitx5_addon_enabled(&parse(text), addon);
        assert_eq!(enabled("", "quickphrase"), Ok(()));
        // `all` disables every addon, and EnabledAddons brings one back.
        let all = "[Behavior/DisabledAddons]\n0=all\n";
        assert_eq!(enabled(all, "clipboard"), Err(2));
        let back = "[Behavior/DisabledAddons]\n0=all\n\n[Behavior/EnabledAddons]\n0=clipboard\n";
        assert_eq!(enabled(back, "clipboard"), Ok(()));
        assert_eq!(enabled(back, "unicode"), Err(2));
        // An addon in both lists is disabled.
        let both = "[Behavior/DisabledAddons]\n0=unicode\n\n[Behavior/EnabledAddons]\n0=unicode\n";
        assert_eq!(enabled(both, "unicode"), Err(2));
        // An emptied `DisabledAddons=` disables nothing.
        assert_eq!(enabled("[Behavior]\nDisabledAddons=\n", "unicode"), Ok(()));
    }

    #[test]
    fn a_pass_carries_the_weakest_tier_of_the_hotkey_sources() {
        // Every file read: the hotkeys came from config files, the state
        // from the daemon, so the weakest is the config file.
        let read = |text: &'static str| {
            let mut files = files(
                Some(text),
                &[
                    ("quickphrase", ""),
                    ("clipboard", ""),
                    ("unicode", ""),
                    ("imselector", ""),
                ],
            );
            files.dir = "/cfg/fcitx5".into();
            files
        };
        let result = with_files(
            "a",
            fcitx5("keyboard-us", "inactive"),
            &read("[Behavior]\n"),
        );
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.tier, Some(EvidenceTier::ConfigFile));
        // An unreadable addon file falls back to its defaults.
        let unread = with_config("a", fcitx5("keyboard-us", "inactive"), Some("[Behavior]\n"));
        assert_eq!(unread.tier, Some(EvidenceTier::Default));
        // The state alone is never a pass: without the hotkeys it is
        // uncertain and has no tier.
        assert_eq!(
            inspect_with_detections(&[fcitx5("pinyin", "inactive")]).tier,
            None
        );
        // An answer about an active engine is the daemon's own: unchanged.
        let active = with_config("a", fcitx5("pinyin", "active"), None);
        assert_eq!(active.outcome, Outcome::UncertainContinues);
        assert_eq!(active.tier, Some(EvidenceTier::LiveQuery));
    }

    #[test]
    fn hotkeys_are_skipped_when_no_engine_was_detected() {
        let key = parse_key("ctrl+alt+p");
        let result = inspect_with_hotkey_sources(
            &key,
            &[fcitx5("keyboard-us", "inactive")],
            None,
            None,
            false,
        );
        // The daemon's hotkeys cannot be read, so nothing proves that none
        // takes the key: a forwarding state alone is not a pass.
        assert_eq!(result.outcome, Outcome::UncertainContinues);
        assert!(result.checked.is_none());
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("hotkeys of a detected input method"))
        );
    }

    /// The full key-aware verdict for `a` with every detected daemon's
    /// hotkey sources readable and empty: the state alone decides.
    fn inspected(detections: &[Detection]) -> LayerResult {
        let fcitx5 = detections
            .iter()
            .any(|detection| detection.engine == "fcitx5")
            .then(|| files(None, &[]));
        let ibus = detections
            .iter()
            .any(|detection| detection.engine == "ibus")
            .then(|| IbusTriggers::Live(Vec::new()));
        inspect_with_hotkey_sources(
            &parse_key("a"),
            detections,
            fcitx5.as_ref(),
            ibus.as_ref(),
            false,
        )
    }

    fn ibus_detection(panel: bool) -> Detection {
        Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec!["ibus-daemon".into()],
            active_engine: None,
            state: None,
            query_error: None,
            no_engine: true,
            panel,
            text_reconstruction: TextReconstruction::default(),
        }
    }

    fn with_ibus(key: &str, panel: bool, triggers: IbusTriggers) -> LayerResult {
        ibus_on(key, panel, false, triggers)
    }

    fn ibus_on(key: &str, panel: bool, x11: bool, triggers: IbusTriggers) -> LayerResult {
        inspect_with_hotkey_sources(
            &parse_key(key),
            &[ibus_detection(panel)],
            None,
            Some(&triggers),
            x11,
        )
    }

    #[test]
    fn ibus_triggers_are_read_from_gsettings_output() {
        let live = IbusTriggers::Live(vec!["<Super>space".into(), "<Control><Alt>x".into()]);
        for key in ["super+space", "ctrl+alt+x"] {
            let result = with_ibus(key, true, live.clone());
            assert_eq!(result.layer, "IBus", "{key}");
            assert_eq!(result.tier, Some(EvidenceTier::LiveQuery), "{key}");
            assert!(result.summary.contains("IBus hotkey triggers"), "{key}");
            assert!(result.details[0].contains("(org.freedesktop.ibus.general.hotkey triggers)"));
        }
        // An emptied list (`@as []`) binds nothing, even the default key.
        let empty = IbusTriggers::Live(crate::layers::gsettings::quoted_values("@as []\n"));
        assert_eq!(with_ibus("super+space", true, empty).outcome, Outcome::Pass);
        assert_eq!(
            crate::layers::gsettings::quoted_values("['<Super>space']\n"),
            vec!["<Super>space".to_owned()]
        );
    }

    #[test]
    fn ibus_defaults_apply_when_gsettings_cannot_answer() {
        let missing = IbusTriggers::Default("No such schema".into());
        let result = with_ibus("super+space", true, missing.clone());
        assert_eq!(result.tier, Some(EvidenceTier::Default));
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert!(result.details[0].contains("IBus default"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("No such schema"))
        );
        // The Pass of an unrelated key rests on the default too.
        let pass = with_ibus("a", true, missing);
        assert_eq!(pass.outcome, Outcome::Pass);
        assert_eq!(pass.tier, Some(EvidenceTier::Default));
        assert!(
            pass.details
                .iter()
                .any(|detail| detail.contains("built-in default"))
        );
    }

    #[test]
    fn an_ibus_trigger_is_certain_only_with_its_panel_on_x11() {
        let live = IbusTriggers::Live(vec!["<Super>space".into()]);
        // The panel must be running to bind the key at all.
        let no_panel = with_ibus("super+space", false, live.clone());
        assert_eq!(no_panel.outcome, Outcome::HandledUncertain);
        assert!(
            no_panel
                .details
                .iter()
                .any(|detail| detail.contains("was not seen running"))
        );
        // With the panel it is a key grab on X11 and uncertain elsewhere.
        for (x11, outcome) in [
            (true, Outcome::Consumed),
            (false, Outcome::HandledUncertain),
        ] {
            let result = ibus_on("super+space", true, x11, live.clone());
            assert_eq!(result.outcome, outcome, "x11 {x11}");
        }
        // A default read in place of gsettings is never certain.
        let default = ibus_on(
            "super+space",
            true,
            true,
            IbusTriggers::Default("no gsettings".into()),
        );
        assert_eq!(default.outcome, Outcome::HandledUncertain);
    }

    #[test]
    fn a_pass_covers_both_engines_when_both_are_detected() {
        let mut both = [fcitx5("keyboard-us", "inactive"), ibus_detection(false)];
        both[1].no_engine = true;
        let files = files(Some("[Behavior]\n"), &[]);
        let live = IbusTriggers::Live(vec!["<Super>space".into()]);
        let result =
            inspect_with_hotkey_sources(&parse_key("a"), &both, Some(&files), Some(&live), false);
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.tier, Some(EvidenceTier::Default));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.starts_with("no Fcitx5 hotkey matches"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.starts_with("no IBus hotkey matches"))
        );
    }

    #[test]
    fn inactive_fcitx_forwards_keys_with_evidence() {
        let result = inspected(&[fcitx5("pinyin", "inactive")]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("is inactive"));
        assert!(result.summary.contains("keys pass through unchanged"));
        assert!(!result.summary.contains("active input method"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "runtime state: inactive")
        );
    }

    #[test]
    fn plain_keyboard_engine_forwards_keys_even_when_active() {
        let result = inspected(&[fcitx5("keyboard-us", "active")]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(
            result
                .summary
                .contains("keyboard-us is a plain keyboard layout")
        );
    }

    #[test]
    fn unreachable_runtime_api_is_distinguished_from_inactive_state() {
        let result = inspect_with_detections(&[Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec![],
            active_engine: None,
            state: None,
            query_error: Some("session bus unavailable".into()),
            no_engine: false,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        }]);
        assert!(result.summary.contains("runtime API is unreachable"));
        // The environment shows an input method exists; nothing answered.
        assert_eq!(result.tier, None);
        assert_eq!(
            inspect_with_detections(&[fcitx5("pinyin", "inactive")]).tier,
            None
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("session bus"))
        );
    }

    #[test]
    fn renders_the_fcitx5_state_matrix_without_stringly_typed_evidence() {
        for (state, outcome) in [
            ("active", Outcome::UncertainContinues),
            ("inactive", Outcome::Pass),
            ("closed", Outcome::Pass),
        ] {
            let result = inspected(&[fcitx5("pinyin", state)]);
            assert_eq!(result.outcome, outcome, "state {state}");
            assert!(result.details.iter().any(|detail| {
                detail.contains("text reconstruction:") && detail.contains("conditional")
            }));
        }
    }

    #[test]
    fn renders_ibus_active_and_unreachable_as_distinct_evidence() {
        let ibus = |active_engine: &str| Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec!["ibus-daemon".into()],
            active_engine: Some(active_engine.into()),
            state: None,
            query_error: None,
            no_engine: false,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        };
        let active = inspect_with_detections(&[ibus("libpinyin")]);
        assert!(active.summary.contains("active input method"));
        assert_eq!(active.outcome, Outcome::UncertainContinues);

        let layout = inspected(&[ibus("xkb:us::eng")]);
        assert_eq!(layout.outcome, Outcome::Pass);

        let unreachable = inspect_with_detections(&[Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec![],
            active_engine: None,
            state: None,
            query_error: Some("session bus unavailable".into()),
            no_engine: false,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        }]);
        assert!(unreachable.summary.contains("runtime API is unreachable"));
        assert_eq!(unreachable.outcome, Outcome::UncertainContinues);
        assert!(
            unreachable
                .details
                .iter()
                .any(|detail| { detail.contains("application preedit unobserved") })
        );
    }

    fn ibus_no_engine(no_engine: bool) -> Detection {
        Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec!["ibus-daemon".into()],
            active_engine: None,
            state: None,
            query_error: None,
            no_engine,
            panel: false,
            text_reconstruction: TextReconstruction::default(),
        }
    }

    #[test]
    fn ibus_without_a_global_engine_forwards_keys() {
        // `ibus engine` answers "No global engine": the daemon is reachable
        // and no engine is selected, so keys pass unchanged. It is not an
        // unreachable runtime API.
        let result = inspected(&[ibus_no_engine(true)]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("no engine is set"));
        assert!(!result.summary.contains("unreachable"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.starts_with("active engine: none"))
        );
    }

    #[test]
    fn ibus_engine_query_reads_no_engine_as_an_answer() {
        use std::os::unix::process::ExitStatusExt;

        let no_engine = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: b"No engine is set.\n".to_vec(),
            stderr: b"(process): IBUS-WARNING: Properties.Get: No global engine.\n".to_vec(),
        };
        assert!(ibus_reports_no_engine(&no_engine));

        let unreachable = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"Cannot connect to ibus-daemon\n".to_vec(),
        };
        assert!(!ibus_reports_no_engine(&unreachable));
    }

    #[test]
    fn ignores_blank_runtime_query_output() {
        assert_eq!(nonempty_line(" \n\n "), None);
        assert_eq!(
            nonempty_line("\n keyboard-us \n"),
            Some("keyboard-us".into())
        );
    }

    #[test]
    fn matches_only_the_exact_fcitx5_service_name() {
        assert!(has_fcitx5_service(
            "array [ string \"org.fcitx.Fcitx5\", string \":1.7\" ]"
        ));
        assert!(!has_fcitx5_service("org.fcitx.Fcitx5-helper"));
        assert!(!has_fcitx5_service("org.example.org.fcitx.Fcitx5"));
    }

    #[test]
    fn parses_read_only_fcitx5_dbus_replies() {
        assert_eq!(
            parse_dbus_string(b"string \"keyboard-us\"\n"),
            Some("keyboard-us".into())
        );
        assert_eq!(
            parse_dbus_string(b"   keyboard-us\n"),
            Some("keyboard-us".into())
        );
        assert_eq!(parse_dbus_string(b"int32 2\n"), None);
        assert_eq!(parse_dbus_string(b"string \"\"\n"), Some(String::new()));
        assert_eq!(parse_dbus_string(b"   "), Some(String::new()));
        assert_eq!(parse_dbus_integer(b"int32 2\n"), Some(2));
        assert_eq!(parse_dbus_integer(b"2\n"), Some(2));
        assert_eq!(parse_dbus_integer(b"reply 2\n"), None);
        assert_eq!(parse_dbus_integer(b"string \"2\"\n"), None);
        assert_eq!(
            fcitx5_state(parse_dbus_integer(b"int32 2\n")),
            Some("active".into())
        );
    }
}
