//! Pre-compositor remappers: keyd, kanata, kmonad, input-remapper, xremap.
//!
//! Detection finds running daemons and their configuration without opening
//! input devices. Each configuration is evaluated for the queried key where
//! its syntax is static; everything else, and any configuration that cannot
//! be read, stays uncertain with the reason.

mod input_remapper;
mod kanata;
mod keyd;
pub(crate) mod keys;
mod kmonad;
mod sexp;
mod xremap;

use crate::recording::PathInputs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingScope, EvidenceTier, LayerId, LayerResult, Outcome, PhysicalInput,
    SourceLocation,
};
use keys::{Press, QueryPresses};

const LAYER: &str = "Input remapper";
const MAX_CONFIG_FILES: usize = 64;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
/// Substrings of the virtual keyboards remappers create. An event read from
/// one was already transformed.
const VIRTUAL_DEVICE_MARKERS: &[&str] = &["keyd", "kanata", "xremap", "kmonad", "input-remapper"];
/// xremap options that take a separate value argument.
const XREMAP_VALUE_OPTIONS: &[&str] = &[
    "--device",
    "--ignore",
    "--output-device-name",
    "--vendor",
    "--product",
    "--completions",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Detection {
    pub name: String,
    pub processes: Vec<String>,
    pub configurations: Vec<String>,
    pub transformations: Vec<String>,
    /// Key-level model of the configuration, used by inspection only.
    #[serde(skip)]
    pub(crate) analysis: Analysis,
}

/// What a remapper does to one physical press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    Untouched,
    Remap(Press),
    Consume(String),
    Conditional(String),
}

/// The configuration line behind a finding, or a note without a location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    text: String,
    file: Option<String>,
    line: u32,
}

impl Rule {
    pub(crate) fn new(file: &str, line: u32, text: &str) -> Self {
        Self {
            text: text.to_owned(),
            file: Some(file.to_owned()),
            line,
        }
    }

    fn render(&self) -> String {
        match &self.file {
            Some(file) if self.line > 0 => format!("{} ({file}:{})", self.text, self.line),
            Some(file) => format!("{} ({file})", self.text),
            None => self.text.clone(),
        }
    }

    fn source(&self) -> Option<SourceLocation> {
        self.file
            .as_ref()
            .map(|file| SourceLocation::new(file.clone(), (self.line > 0).then_some(self.line)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) effect: Effect,
    pub(crate) rule: Option<Rule>,
}

impl Finding {
    pub(crate) fn untouched() -> Self {
        Self {
            effect: Effect::Untouched,
            rule: None,
        }
    }

    /// A configuration may not apply at all, for the stated reason.
    pub(crate) fn untouched_because(reason: &str) -> Self {
        Self {
            effect: Effect::Untouched,
            rule: Some(Rule {
                text: reason.to_owned(),
                file: None,
                line: 0,
            }),
        }
    }
}

/// The input device a configuration is limited to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceScope {
    /// How the configuration names the device.
    label: String,
    /// Device names that identify the device; empty when unknown.
    names: Vec<String>,
}

impl DeviceScope {
    /// An evdev device file such as `/dev/input/by-id/…`. Its name
    /// identifies it only while no other present device shares the name.
    pub(crate) fn device_file(path: &str) -> Self {
        let name = evdev_device_name(Path::new(path)).filter(|name| {
            present_device_names().is_some_and(|names| {
                names
                    .iter()
                    .filter(|other| same_device_name(other, name))
                    .count()
                    == 1
            })
        });
        Self {
            label: path.to_owned(),
            names: name.into_iter().collect(),
        }
    }

    /// Every input device with this name.
    pub(crate) fn named(name: &str) -> Self {
        Self {
            label: name.to_owned(),
            names: vec![name.to_owned()],
        }
    }

    /// Whether the captured input device is provably this device.
    pub(crate) fn includes(&self, captured: Option<&str>) -> bool {
        captured.is_some_and(|captured| {
            self.names
                .iter()
                .any(|name| same_device_name(name, captured))
        })
    }

    /// The configuration does not apply to keys from other devices.
    pub(crate) fn finding(&self, remapper: &str) -> Finding {
        Finding::untouched_because(&format!(
            "{remapper} reads only {}, and the queried key is not known to come from it",
            self.label
        ))
    }
}

/// Device names compare without case and punctuation, so a compositor's
/// `at-translated-set-2-keyboard` matches `AT Translated Set 2 keyboard`.
fn same_device_name(left: &str, right: &str) -> bool {
    let normalize = |name: &str| {
        name.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|character| character.to_ascii_lowercase())
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

/// The kernel name of the evdev device behind a device file.
fn evdev_device_name(path: &Path) -> Option<String> {
    let target = crate::recording::canonicalize(path).ok()?;
    let node = target.file_name()?.to_str()?;
    if !node.starts_with("event") {
        return None;
    }
    let name =
        crate::recording::read_to_string(format!("/sys/class/input/{node}/device/name")).ok()?;
    Some(name.trim().to_owned())
}

/// Names of the present input devices.
fn present_device_names() -> Option<Vec<String>> {
    let content = crate::recording::read_to_string("/proc/bus/input/devices").ok()?;
    Some(
        content
            .lines()
            .filter_map(|line| {
                let name = line.strip_prefix("N: Name=")?;
                Some(name.trim_matches('"').to_owned())
            })
            .collect(),
    )
}

/// The static model of one detected remapper.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Analysis {
    running: bool,
    engine: Engine,
    /// Configurations that exist or are named but could not be read.
    unreadable: Vec<String>,
    /// Reasons the configuration cannot be evaluated statically.
    unevaluated: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Engine {
    /// No configuration applies, so the daemon changes nothing.
    #[default]
    Idle,
    Keyd(keyd::Keymap),
    /// Each entry is one configuration kanata may be running.
    Kanata(Vec<kanata::Config>),
    /// Each entry is the configuration of one kmonad process.
    Kmonad(Vec<kmonad::Config>),
    /// Each entry is one autoloaded preset.
    InputRemapper(Vec<input_remapper::Preset>),
    /// Each entry is one merged xremap invocation or candidate file.
    Xremap(Vec<xremap::Config>),
}

/// Configuration files found for one remapper.
#[derive(Debug, Default)]
struct Located {
    /// Each group is one configuration the daemon may be running; xremap
    /// merges the files of a group.
    groups: Vec<Group>,
    unreadable: Vec<String>,
    keyd_app_config: Option<PathBuf>,
    /// input-remapper's `xmodmap.json`, the session layout's symbol names.
    layout_symbols: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct Group {
    paths: Vec<PathBuf>,
    device_scoped: bool,
    /// The input devices an input-remapper preset applies to.
    device: Option<DeviceScope>,
    /// Settings from the kmonad command line.
    kmonad: kmonad::Cli,
}

/// Detect common Linux remappers without opening or changing input devices.
/// A process/configuration hit is evidence that a transformation may happen
/// before the compositor; it is not proof of the active runtime graph.
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
    let mut detections: BTreeMap<String, Detection> = BTreeMap::new();
    for (name, process_names, config_env, defaults) in known_remappers() {
        let pids = snapshot.matching_pids(process_names);
        let located = locate(name, &pids, config_env);
        let mut detection = Detection {
            name: name.to_owned(),
            processes: snapshot.find_processes(process_names),
            configurations: Vec::new(),
            transformations: Vec::new(),
            analysis: Analysis::default(),
        };
        let mut config_paths = config_candidates(config_env, defaults);
        config_paths.extend(located.groups.iter().flat_map(|group| group.paths.clone()));
        config_paths.sort();
        config_paths.dedup();
        for path in config_paths {
            if !path.recorded_is_file() {
                continue;
            }
            detection.configurations.push(path.display().to_string());
            detection.transformations.extend(match name {
                "keyd" => parse_keyd_transformations(&path),
                "kanata" | "kmonad" => parse_lisp_transformations(&path),
                "input-remapper" => parse_input_remapper_transformations(&path),
                "xremap" => parse_xremap_transformations(&path),
                _ => Vec::new(),
            });
        }
        if !detection.processes.is_empty()
            || !detection.configurations.is_empty()
            || !located.unreadable.is_empty()
        {
            let running = match name {
                "input-remapper" => input_remapper_service_running(&pids),
                _ => !detection.processes.is_empty(),
            };
            detection.analysis = analyze(name, running, located);
            detection.processes.sort();
            detection.configurations.sort();
            detection.transformations.sort();
            detection.transformations.dedup();
            detections.insert(name.to_owned(), detection);
        }
    }
    detections.into_values().collect()
}

/// Describe a previously collected remapper snapshot for one key without
/// rediscovering processes or configuration files. Only an explicit XKB
/// layout in the environment tells whether printable keys sit at their US
/// positions.
pub fn inspect_with_detections(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    detections: &[Detection],
) -> LayerResult {
    inspect_with_detections_and_layout(key, physical_input, detections, None)
}

/// As [`inspect_with_detections`], with the compositor's own answer to
/// whether the layout is plain US (Hyprland reports its main keyboard's
/// RMLVO); `None` falls back to the environment.
pub fn inspect_with_detections_and_layout(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    detections: &[Detection],
    compositor_layout_us: Option<bool>,
) -> LayerResult {
    let layout_us = compositor_layout_us.unwrap_or_else(keys::layout_is_us);
    inspect_detections(key, physical_input, detections, layout_us)
}

/// Remapper rules come from configuration files; the daemons are never asked.
fn inspect_detections(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    detections: &[Detection],
    layout_us: bool,
) -> LayerResult {
    let tier = detections
        .iter()
        .any(|detection| !detection.configurations.is_empty())
        .then_some(EvidenceTier::ConfigFile);
    resolve_detections(key, physical_input, detections, layout_us).with_optional_tier(tier)
}

fn resolve_detections(
    key: &KeyCombo,
    physical_input: Option<&PhysicalInput>,
    detections: &[Detection],
    layout_us: bool,
) -> LayerResult {
    if detections.is_empty() {
        return LayerResult::pass(
            LAYER,
            LayerId::Remapper,
            "no supported pre-compositor remapper detected",
            Vec::new(),
        );
    }
    let mut details = detection_details(detections);
    let keycode = match physical_input {
        Some(input) => {
            let source = input.device.as_deref().unwrap_or("compositor capture");
            if input.device.as_deref().is_none_or(is_remapper_device) {
                details.push(format!(
                    "captured evdev input {} from {source} after any remapping; the physical key pressed is not known",
                    input.keycode
                ));
                return uncertain(details);
            }
            details.push(format!(
                "captured evdev input {} from {source}",
                input.keycode
            ));
            Some(input.keycode.get())
        }
        None => None,
    };
    let shown = crate::style::human_key(key);
    let Some(query) = keys::query_presses(key, keycode, layout_us) else {
        details.push(format!(
            "{shown} has no known physical key code, so remapper rules cannot be matched"
        ));
        return uncertain(details);
    };

    let device = physical_input.and_then(|input| input.device.as_deref());
    let active: Vec<(&Detection, Verdict)> = detections
        .iter()
        .map(|detection| {
            (
                detection,
                detection.analysis.verdict(&query, &shown, device),
            )
        })
        .filter(|(_, verdict)| !matches!(verdict, Verdict::Pass))
        .collect();
    match active.as_slice() {
        [] => LayerResult::pass(
            LAYER,
            LayerId::Remapper,
            format!("no remapper rule changes {shown}"),
            details,
        ),
        [(detection, Verdict::Definite(finding))] => {
            definite(detection, finding, &shown, layout_us, details)
        }
        _ => combined(&active, &shown, details),
    }
}

/// A single remapper provably remaps or consumes the key.
fn definite(
    detection: &Detection,
    finding: &Finding,
    key: &str,
    layout_us: bool,
    mut details: Vec<String>,
) -> LayerResult {
    let name = &detection.name;
    if let Some(rule) = &finding.rule {
        details.insert(0, format!("binding: {}", rule.render()));
    }
    let source = finding.rule.as_ref().and_then(Rule::source);
    let description = finding.rule.as_ref().map(|rule| rule.text.clone());
    let (outcome, summary, action) = match &finding.effect {
        Effect::Remap(press) => match keys::downstream_combo(*press, layout_us) {
            Ok(target) => (
                Outcome::Redirected,
                format!(
                    "{name} remaps {key} to {}",
                    crate::style::human_key(&target)
                ),
                target.compact_display(),
            ),
            Err(why) => {
                let emitted = keys::describe(*press);
                (
                    Outcome::Redirected,
                    format!("{name} remaps {key} to {emitted}"),
                    format!("{emitted} ({why})"),
                )
            }
        },
        Effect::Consume(why) => (
            Outcome::Consumed,
            format!("{name} consumes {key}: {why}"),
            why.clone(),
        ),
        Effect::Untouched | Effect::Conditional(_) => {
            unreachable!("definite verdicts remap or consume")
        }
    };
    let mut binding = BindingEvidence::action(action, BindingScope::Universal, source)
        .with_dispatcher(name.clone());
    if let Some(description) = description {
        binding = binding.with_description(description);
    }
    LayerResult::new(LAYER, LayerId::Remapper, outcome, summary, details).with_binding(binding)
}

/// Several findings, or findings that are not provable.
fn combined(active: &[(&Detection, Verdict)], key: &str, mut details: Vec<String>) -> LayerResult {
    let mut handled = Vec::new();
    let mut unavailable = Vec::new();
    let mut first_rule = None;
    for (detection, verdict) in active {
        let name = &detection.name;
        match verdict {
            Verdict::Pass => {}
            Verdict::Definite(finding) => {
                let text = effect_text(&finding.effect);
                details.push(format!("{name}: {text}"));
                first_rule = first_rule.or(finding.rule.clone());
                handled.push((name.as_str(), text));
            }
            Verdict::Conditional(why, rule) => {
                details.push(format!("{name}: {why}"));
                if let Some(rule) = rule {
                    details.push(format!("binding: {}", rule.render()));
                }
                first_rule = first_rule.or(rule.clone());
                handled.push((name.as_str(), why.clone()));
            }
            Verdict::Unavailable(reasons) => {
                details.extend(
                    reasons
                        .iter()
                        .map(|reason| format!("{name} configuration unreadable: {reason}")),
                );
                unavailable.push(name.as_str());
            }
            Verdict::Unevaluated(reasons) => {
                details.extend(
                    reasons
                        .iter()
                        .map(|reason| format!("{name} not evaluated: {reason}")),
                );
            }
        }
    }
    details.push(
        "virtual-device routing, timing-dependent transforms, and firmware changes remain unknown"
            .into(),
    );
    if !handled.is_empty() {
        if handled.len() > 1 {
            details.push("several remappers act on this key; their order is not known".into());
        }
        let summary = match handled.as_slice() {
            [(name, text)] => format!("{name} may change {key}: {text}"),
            _ => format!("several remappers may change {key}"),
        };
        let action = handled
            .iter()
            .map(|(name, text)| format!("{name}: {text}"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut binding = BindingEvidence::action(
            action,
            BindingScope::Unknown,
            first_rule.as_ref().and_then(Rule::source),
        );
        if let Some(rule) = &first_rule {
            binding = binding.with_description(rule.text.clone());
        }
        return LayerResult::new(
            LAYER,
            LayerId::Remapper,
            Outcome::HandledUncertain,
            summary,
            details,
        )
        .with_binding(binding);
    }
    if !unavailable.is_empty() {
        return LayerResult::unavailable(
            LAYER,
            LayerId::Remapper,
            format!(
                "{} configuration could not be read; a pre-compositor remapper may transform this input",
                unavailable.join(", ")
            ),
            details,
        );
    }
    uncertain(details)
}

fn uncertain(mut details: Vec<String>) -> LayerResult {
    if !details
        .iter()
        .any(|detail| detail.starts_with("virtual-device routing"))
    {
        details.push(
            "virtual-device routing, timing-dependent transforms, and firmware changes remain unknown"
                .into(),
        );
    }
    LayerResult::uncertain_continues(
        LAYER,
        LayerId::Remapper,
        "a pre-compositor remapper may transform this input",
        details,
    )
}

fn detection_details(detections: &[Detection]) -> Vec<String> {
    let mut details = Vec::new();
    for detection in detections {
        details.push(format!("detected: {}", detection.name));
        details.extend(
            detection
                .processes
                .iter()
                .map(|process| format!("process: {process}")),
        );
        details.extend(
            detection
                .configurations
                .iter()
                .map(|configuration| format!("config: {configuration}")),
        );
        details.extend(
            detection
                .transformations
                .iter()
                .map(|transformation| format!("static transformation: {transformation}")),
        );
    }
    details
}

fn is_remapper_device(device: &str) -> bool {
    let device = device.to_ascii_lowercase();
    VIRTUAL_DEVICE_MARKERS
        .iter()
        .any(|marker| device.contains(marker))
}

fn effect_text(effect: &Effect) -> String {
    match effect {
        Effect::Untouched => "leaves the key unchanged".into(),
        Effect::Remap(press) => format!("remaps it to {}", keys::describe(*press)),
        Effect::Consume(why) | Effect::Conditional(why) => why.clone(),
    }
}

/// One remapper's answer for the queried key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Pass,
    /// Every possible configuration remaps or consumes the key the same way.
    Definite(Finding),
    Conditional(String, Option<Rule>),
    Unavailable(Vec<String>),
    Unevaluated(Vec<String>),
}

impl Analysis {
    fn evaluate(&self, press: Press, device: Option<&str>) -> Vec<Finding> {
        match &self.engine {
            Engine::Idle => vec![Finding::untouched()],
            Engine::Keyd(keymap) => keymap.evaluate(press),
            Engine::Kanata(configs) => configs
                .iter()
                .flat_map(|config| config.evaluate(press))
                .collect(),
            Engine::Kmonad(configs) => configs
                .iter()
                .flat_map(|config| config.evaluate(press, device))
                .collect(),
            Engine::InputRemapper(presets) => presets
                .iter()
                .flat_map(|preset| preset.evaluate(press, device))
                .collect(),
            Engine::Xremap(configs) => configs
                .iter()
                .flat_map(|config| config.evaluate(press))
                .collect(),
        }
    }

    /// `device` is the captured input device of the query, when known.
    fn verdict(&self, query: &QueryPresses, key: &str, device: Option<&str>) -> Verdict {
        if !self.unreadable.is_empty() {
            return Verdict::Unavailable(self.unreadable.clone());
        }
        if !self.unevaluated.is_empty() {
            return Verdict::Unevaluated(self.unevaluated.clone());
        }
        let findings: Vec<Finding> = query
            .presses
            .iter()
            .flat_map(|press| {
                self.evaluate(*press, device)
                    .into_iter()
                    .map(|mut finding| {
                        if finding.effect == Effect::Remap(*press) {
                            finding.effect = Effect::Untouched;
                        }
                        finding
                    })
            })
            .collect();
        let Some(first) = findings.first() else {
            return Verdict::Pass;
        };
        let verdict = if findings
            .iter()
            .all(|finding| finding.effect == first.effect)
        {
            match &first.effect {
                Effect::Untouched => Verdict::Pass,
                Effect::Remap(_) | Effect::Consume(_) => Verdict::Definite(first.clone()),
                Effect::Conditional(why) => Verdict::Conditional(why.clone(), first.rule.clone()),
            }
        } else {
            let mut outcomes: Vec<String> = Vec::new();
            for finding in &findings {
                let text = match &finding.rule {
                    Some(rule) => format!("{} ({})", effect_text(&finding.effect), rule.render()),
                    None => effect_text(&finding.effect),
                };
                if !outcomes.contains(&text) {
                    outcomes.push(text);
                }
            }
            let rule = findings
                .iter()
                .filter(|finding| finding.effect != Effect::Untouched)
                .find_map(|finding| finding.rule.clone());
            Verdict::Conditional(
                format!(
                    "the result depends on which configuration or key applies: {}",
                    outcomes.join("; ")
                ),
                rule,
            )
        };
        match verdict {
            Verdict::Pass if query.layout_assumed => Verdict::Unevaluated(vec![format!(
                "no rule matches the US-layout position of {key}, but the XKB layout is not known, so another physical key may produce it"
            )]),
            Verdict::Definite(finding) if query.layout_assumed => Verdict::Conditional(
                format!(
                    "{} if {key} is at its US-layout position; the XKB layout is not known",
                    effect_text(&finding.effect)
                ),
                finding.rule,
            ),
            Verdict::Definite(finding) if !self.running => Verdict::Conditional(
                format!(
                    "{} when the daemon runs; no running process was seen",
                    effect_text(&finding.effect)
                ),
                finding.rule,
            ),
            verdict => verdict,
        }
    }
}

/// Build the key-level model from located configuration files.
fn analyze(name: &str, running: bool, located: Located) -> Analysis {
    let Located {
        groups,
        mut unreadable,
        keyd_app_config,
        layout_symbols,
    } = located;
    let mut unevaluated = Vec::new();
    let engine = match name {
        "keyd" => {
            let paths: Vec<PathBuf> = groups.into_iter().flat_map(|group| group.paths).collect();
            let (keymap, missing) =
                keyd::Keymap::load(&paths, keyd_app_config.as_deref(), present_input_ids());
            unreadable.extend(missing);
            if keymap.is_empty() {
                Engine::Idle
            } else {
                Engine::Keyd(keymap)
            }
        }
        "kanata" => {
            let mut configs = Vec::new();
            for path in groups.iter().flat_map(|group| &group.paths) {
                match read_config(path) {
                    Ok(content) => match kanata::load(path, &content, &mut unreadable) {
                        Ok(config) => configs.push(config),
                        Err(reason) => unevaluated.push(reason),
                    },
                    Err(reason) => unreadable.push(reason),
                }
            }
            Engine::Kanata(configs)
        }
        "xremap" => {
            let mut configs = Vec::new();
            for group in &groups {
                let mut config = xremap::Config::default().with_device_scope(group.device_scoped);
                let mut complete = true;
                for path in &group.paths {
                    match read_config(path) {
                        Ok(content) => {
                            if let Err(reason) = config.extend(path, &content) {
                                unevaluated.push(reason);
                                complete = false;
                            }
                        }
                        Err(reason) => {
                            unreadable.push(reason);
                            complete = false;
                        }
                    }
                }
                if complete {
                    configs.push(config);
                }
            }
            Engine::Xremap(configs)
        }
        "kmonad" => {
            let mut configs = Vec::new();
            for group in &groups {
                for path in &group.paths {
                    match read_config(path) {
                        Ok(content) => match kmonad::load(path, &content, &group.kmonad) {
                            Ok(config) => configs.push(config),
                            Err(reason) => unevaluated.push(reason),
                        },
                        Err(reason) => unreadable.push(reason),
                    }
                }
            }
            Engine::Kmonad(configs)
        }
        "input-remapper" => {
            let symbols = layout_symbols.as_deref().and_then(|path| {
                let loaded = read_config(path).and_then(|content| {
                    input_remapper::layout_symbols(&content)
                        .map_err(|reason| format!("{}: {reason}", path.display()))
                });
                loaded.map_err(|reason| unreadable.push(reason)).ok()
            });
            let mut presets = Vec::new();
            for group in &groups {
                let device = group
                    .device
                    .clone()
                    .unwrap_or_else(|| DeviceScope::named(""));
                for path in &group.paths {
                    match read_config(path) {
                        Ok(content) => {
                            match input_remapper::load(path, &content, &device, symbols.as_deref())
                            {
                                Ok(preset) => presets.push(preset),
                                Err(reason) => unevaluated.push(reason),
                            }
                        }
                        Err(reason) => unreadable.push(reason),
                    }
                }
            }
            Engine::InputRemapper(presets)
        }
        _ => Engine::Idle,
    };
    let engine = match engine {
        Engine::Kanata(configs) if configs.is_empty() => Engine::Idle,
        Engine::Kmonad(configs) if configs.is_empty() => Engine::Idle,
        Engine::InputRemapper(presets) if presets.is_empty() => Engine::Idle,
        Engine::Xremap(configs) if configs.is_empty() => Engine::Idle,
        engine => engine,
    };
    if engine == Engine::Idle && running && unreadable.is_empty() && unevaluated.is_empty() {
        match name {
            "kanata" | "xremap" | "kmonad" => unevaluated.push(format!(
                "no {name} configuration was found for the running process"
            )),
            "input-remapper" => unevaluated.push(
                "the running input-remapper service autoloads no preset; a preset applied from its window is not known"
                    .into(),
            ),
            _ => {}
        }
    }
    Analysis {
        running,
        engine,
        unreadable,
        unevaluated,
    }
}

/// Find the configuration each remapper is (or would be) running.
fn locate(name: &str, pids: &[u32], config_env: &str) -> Located {
    let mut located = Located::default();
    match name {
        "keyd" => {
            let (dir, system) = match crate::recording::env_var_os(config_env).map(PathBuf::from) {
                Some(path) if path.recorded_is_dir() => (path, false),
                Some(path) => {
                    located.groups.push(Group {
                        paths: vec![path],
                        ..Group::default()
                    });
                    return located;
                }
                None => (PathBuf::from("/etc/keyd"), true),
            };
            match crate::recording::read_dir(&dir) {
                Ok(entries) => {
                    let mut paths: Vec<PathBuf> = entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.extension()
                                .is_some_and(|extension| extension == "conf")
                        })
                        .collect();
                    paths.sort();
                    paths.truncate(MAX_CONFIG_FILES);
                    located.groups.push(Group {
                        paths,
                        ..Group::default()
                    });
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    located
                        .unreadable
                        .push(format!("{}: {error}", dir.display()));
                }
                Err(_) => {}
            }
            if system {
                located.keyd_app_config =
                    expand_home("~/.config/keyd/app.conf").filter(|path| path.recorded_is_file());
            }
        }
        "kanata" => {
            for pid in pids {
                let Some(args) = process_args(*pid) else {
                    continue;
                };
                if let Some(path) = kanata_config_arg(&args) {
                    match resolve_process_path(*pid, &path) {
                        Ok(path) => located.groups.push(Group {
                            paths: vec![path],
                            ..Group::default()
                        }),
                        Err(reason) => located.unreadable.push(reason),
                    }
                }
            }
            if located.groups.is_empty() && located.unreadable.is_empty() {
                let candidates = match crate::recording::env_var_os(config_env) {
                    Some(path) => vec![PathBuf::from(path)],
                    // A running kanata without `-c` reads kanata.kbd.
                    None if !pids.is_empty() => expand_home("~/.config/kanata/kanata.kbd")
                        .into_iter()
                        .collect(),
                    None => files_with_extensions("~/.config/kanata", &["kbd"]),
                };
                located.groups.extend(single_file_groups(candidates));
            }
        }
        "xremap" => {
            for pid in pids {
                let Some(args) = process_args(*pid) else {
                    continue;
                };
                let (paths, device_scoped) = xremap_config_args(&args);
                let mut group = Group {
                    device_scoped,
                    ..Group::default()
                };
                for path in paths {
                    match resolve_process_path(*pid, &path) {
                        Ok(path) => group.paths.push(path),
                        Err(reason) => located.unreadable.push(reason),
                    }
                }
                if !group.paths.is_empty() {
                    located.groups.push(group);
                }
            }
            if located.groups.is_empty() && located.unreadable.is_empty() {
                let candidates = match crate::recording::env_var_os(config_env) {
                    Some(path) => vec![PathBuf::from(path)],
                    None => files_with_extensions("~/.config/xremap", &["yml", "yaml"]),
                };
                located.groups.extend(single_file_groups(candidates));
            }
        }
        "kmonad" => {
            for pid in pids {
                let Some(args) = process_args(*pid) else {
                    continue;
                };
                let (Some(path), cli) = kmonad::command_line(&args) else {
                    continue;
                };
                match resolve_process_path(*pid, &path) {
                    Ok(path) => located.groups.push(Group {
                        paths: vec![path],
                        kmonad: cli,
                        ..Group::default()
                    }),
                    Err(reason) => located.unreadable.push(reason),
                }
            }
            // kmonad has no default configuration; a stopped one may run
            // any of these.
            if pids.is_empty() {
                let candidates = match crate::recording::env_var_os(config_env) {
                    Some(path) => vec![PathBuf::from(path)],
                    None => files_with_extensions("~/.config/kmonad", &["kbd"]),
                };
                located.groups.extend(single_file_groups(candidates));
            }
        }
        "input-remapper" => {
            let Some(dir) = input_remapper_dir(config_env) else {
                return located;
            };
            let config = dir.join("config.json");
            match read_config(&config) {
                Ok(content) => match input_remapper::autoload(&content) {
                    Ok(entries) => {
                        let groups = crate::recording::read_to_string("/proc/bus/input/devices")
                            .ok()
                            .map(|devices| input_remapper::present_groups(&devices));
                        located
                            .groups
                            .extend(entries.into_iter().map(|(key, preset)| {
                                let (name, device) =
                                    input_remapper::resolve_group(&key, groups.as_deref());
                                Group {
                                    paths: vec![input_remapper::preset_path(&dir, &name, &preset)],
                                    device: Some(device),
                                    ..Group::default()
                                }
                            }));
                    }
                    Err(reason) => located
                        .unreadable
                        .push(format!("{}: {reason}", config.display())),
                },
                Err(_)
                    if crate::recording::metadata(&config)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                Err(reason) => located.unreadable.push(reason),
            }
            let symbols = dir.join("xmodmap.json");
            located.layout_symbols = symbols.recorded_exists().then_some(symbols);
        }
        _ => {}
    }
    located
}

fn single_file_groups(paths: Vec<PathBuf>) -> impl Iterator<Item = Group> {
    paths
        .into_iter()
        .filter(|path| path.recorded_exists())
        .map(|path| Group {
            paths: vec![path],
            ..Group::default()
        })
}

/// input-remapper's configuration directory: `config_env` (a directory or
/// its `config.json`), else `$XDG_CONFIG_HOME/input-remapper-2`.
fn input_remapper_dir(config_env: &str) -> Option<PathBuf> {
    if let Some(path) = crate::recording::env_var_os(config_env).map(PathBuf::from) {
        if path.recorded_is_dir() {
            return Some(path);
        }
        return path.parent().map(Path::to_path_buf);
    }
    match crate::recording::env_var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(config) => Some(PathBuf::from(config).join("input-remapper-2")),
        None => expand_home("~/.config/input-remapper-2"),
    }
}

/// Whether one of the matched processes is the input-remapper service, the
/// root daemon that injects presets. The window and the reader service
/// share its truncated process name.
fn input_remapper_service_running(pids: &[u32]) -> bool {
    pids.iter().any(|pid| {
        process_args(*pid).is_some_and(|args| {
            args.iter().take(2).any(|arg| {
                Path::new(arg)
                    .file_name()
                    .is_some_and(|name| name == "input-remapper-service")
            })
        })
    })
}

/// `-c PATH`, `-cPATH`, `--cfg PATH`, or `--cfg=PATH`; kanata runs the first.
fn kanata_config_arg(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "-c" || arg == "--cfg" {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix("--cfg=") {
            return Some(value.to_owned());
        }
        if let Some(value) = arg.strip_prefix("-c").filter(|value| !value.is_empty()) {
            return Some(value.to_owned());
        }
    }
    None
}

/// xremap's positional configuration files and whether `--device` limits
/// the grabbed keyboards.
fn xremap_config_args(args: &[String]) -> (Vec<String>, bool) {
    let mut paths = Vec::new();
    let mut device_scoped = false;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            paths.extend(iter.by_ref().cloned());
            break;
        }
        if let Some(option) = arg.strip_prefix("--") {
            device_scoped |= option.split('=').next() == Some("device");
            if !option.contains('=') && XREMAP_VALUE_OPTIONS.contains(&arg.as_str()) {
                iter.next();
            }
            continue;
        }
        if !arg.starts_with('-') {
            paths.push(arg.clone());
        }
    }
    (paths, device_scoped)
}

fn process_args(pid: u32) -> Option<Vec<String>> {
    let raw = crate::recording::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}

/// Resolve a path argument against the process working directory.
fn resolve_process_path(pid: u32, path: &str) -> Result<PathBuf, String> {
    let path = expand_home(path).unwrap_or_else(|| PathBuf::from(path));
    if path.is_absolute() {
        return Ok(path);
    }
    crate::recording::read_link(format!("/proc/{pid}/cwd"))
        .map(|cwd| cwd.join(&path))
        .map_err(|error| {
            format!(
                "{} (relative to the working directory of pid {pid}): {error}",
                path.display()
            )
        })
}

fn expand_home(raw: &str) -> Option<PathBuf> {
    match raw.strip_prefix("~/") {
        Some(rest) => {
            crate::recording::env_var_os("HOME").map(|home| PathBuf::from(home).join(rest))
        }
        None => Some(PathBuf::from(raw)),
    }
}

fn files_with_extensions(dir: &str, extensions: &[&str]) -> Vec<PathBuf> {
    let Some(dir) = expand_home(dir) else {
        return Vec::new();
    };
    let Ok(entries) = crate::recording::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        })
        .collect();
    paths.sort();
    paths.truncate(MAX_CONFIG_FILES);
    paths
}

/// `vendor:product` ids of the present input devices, as keyd's `[ids]`
/// names them.
fn present_input_ids() -> Option<Vec<String>> {
    let content = crate::recording::read_to_string("/proc/bus/input/devices").ok()?;
    Some(
        content
            .lines()
            .filter_map(|line| {
                let line = line.strip_prefix("I:")?;
                let field = |name: &str| {
                    line.split_whitespace()
                        .find_map(|part| part.strip_prefix(name))
                        .map(str::to_ascii_lowercase)
                };
                Some(format!("{}:{}", field("Vendor=")?, field("Product=")?))
            })
            .collect(),
    )
}

/// Read a configuration file, keeping the reason when it cannot be read.
pub(crate) fn read_config(path: &Path) -> Result<String, String> {
    let bytes = crate::recording::read_prefix(path, MAX_CONFIG_BYTES)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let content = String::from_utf8(bytes)
        .map_err(|_| format!("{}: stream did not contain valid UTF-8", path.display()))?;
    if content.len() > MAX_CONFIG_BYTES {
        return Err(format!("{}: larger than 1 MiB", path.display()));
    }
    Ok(content)
}

fn known_remappers() -> [(
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static [&'static str],
); 5] {
    [
        (
            "keyd",
            &["keyd"],
            "WHYKEY_KEYD_CONFIG",
            &["/etc/keyd", "~/.config/keyd"],
        ),
        (
            "kanata",
            &["kanata"],
            "KANATA_CONFIG",
            &["~/.config/kanata"],
        ),
        (
            "kmonad",
            &["kmonad"],
            "KMONAD_CONFIG",
            &["~/.config/kmonad"],
        ),
        (
            "input-remapper",
            // The kernel truncates the service's process name to 15 bytes.
            &[
                "input-remapper",
                "input-remapper-service",
                "input-remapper-",
            ],
            "INPUT_REMAPPER_CONFIG",
            &["~/.config/input-remapper-2"],
        ),
        (
            "xremap",
            &["xremap"],
            "XREMAP_CONFIG",
            &["~/.config/xremap"],
        ),
    ]
}

fn config_candidates(env_name: &str, defaults: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = crate::recording::env_var_os(env_name) {
        candidates.push(PathBuf::from(path));
    }
    for raw in defaults {
        let expanded = if let Some(rest) = raw.strip_prefix("~/") {
            crate::recording::env_var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(rest))
        } else {
            Some(PathBuf::from(raw))
        };
        let Some(path) = expanded else { continue };
        if path.recorded_is_dir() {
            let Ok(entries) = crate::recording::read_dir(&path) else {
                continue;
            };
            candidates.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.recorded_is_file())
                    .take(MAX_CONFIG_FILES),
            );
        } else {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn parse_keyd_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('['))
        .filter_map(|line| {
            let (from, to) = line.split_once('=')?;
            let from = from.trim();
            let to = to.trim();
            (!from.is_empty() && !to.is_empty()).then(|| format!("{from} -> {to}"))
        })
        .take(MAX_CONFIG_FILES)
        .collect::<Vec<_>>();
    if content.lines().map(str::trim).any(|line| {
        line.split_once('=').is_some_and(|(_, to)| {
            let to = to.trim().to_ascii_lowercase();
            to.contains("overload") || to.contains("oneshot") || to.contains("one-shot")
        })
    }) {
        transformations.push(
            "timing-dependent keyd overload/one-shot behavior detected; tap-versus-hold remains conditional"
                .into(),
        );
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn parse_lisp_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    for line in content.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("(defalias") else {
            continue;
        };
        let tokens = rest
            .trim_matches(|character| character == '(' || character == ')')
            .split_whitespace()
            .collect::<Vec<_>>();
        if tokens.len() >= 2
            && tokens[0].chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '@')
            })
        {
            transformations.push(format!(
                "{} -> {} (conditional alias)",
                tokens[0],
                tokens[1..].join(" ")
            ));
        }
    }
    if content.contains("tap-hold") || content.contains("tap-hold-next-release") {
        transformations.push(
            "tap-hold/layer behavior detected; timing and active layer remain conditional".into(),
        );
    }
    if content.contains("deflayer") {
        transformations.push(
            "layered keyboard definitions detected; active layer and precedence remain conditional"
                .into(),
        );
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn parse_input_remapper_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    collect_json_pairs(&value, &mut transformations);
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn collect_json_pairs(value: &serde_json::Value, transformations: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        if let Some(values) = value.as_array() {
            for value in values {
                collect_json_pairs(value, transformations);
            }
        }
        return;
    };
    if let (Some(input), Some(output)) = (object.get("input"), object.get("output")) {
        let input = display_json_value(input);
        let output = display_json_value(output);
        if !input.is_empty() && !output.is_empty() {
            transformations.push(format!("{input} -> {output}"));
        }
    }
    for value in object.values() {
        collect_json_pairs(value, transformations);
    }
}

fn display_json_value(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn parse_xremap_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    let mut in_remap = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("remap:") {
            in_remap = true;
            continue;
        }
        if trimmed.ends_with(':') && !trimmed.starts_with('-') {
            let key = trimmed.trim_end_matches(':').trim();
            if !matches!(key, "modmap" | "keymap" | "remap") {
                in_remap = false;
            }
            continue;
        }
        if !in_remap || trimmed.contains('{') || trimmed.contains('}') {
            continue;
        }
        let Some((from, to)) = trimmed.split_once(':') else {
            continue;
        };
        let from = from.trim().trim_matches(['"', '\'']);
        let to = to.trim().trim_matches(['"', '\'']);
        if !from.is_empty() && !to.is_empty() && !to.starts_with('[') {
            transformations.push(format!("{from} -> {to}"));
        }
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    #[test]
    fn parses_only_literal_keyd_mappings() {
        let path = env::temp_dir().join(format!(
            "whykey-keyd-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "[main]\ncapslock = overload(control, esc)\na = b\n# ignored\n",
        )
        .unwrap();
        let transformations = parse_keyd_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value == "capslock -> overload(control, esc)")
        );
        assert!(transformations.iter().any(|value| value == "a -> b"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_literal_caps_to_ctrl_from_fixture() {
        let path = fixture_path("keyd-caps-to-ctrl.conf");
        let transformations = parse_keyd_transformations(&path);
        assert_eq!(transformations, vec!["capslock -> leftcontrol"]);
    }

    #[test]
    fn keeps_keyd_tap_hold_timing_conditional() {
        let path = fixture_path("keyd-overload-tap-hold.conf");
        let transformations = parse_keyd_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value == "capslock -> overload(control, esc)")
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("timing-dependent") && value.contains("conditional"))
        );
    }

    #[test]
    fn catalogs_common_remappers() {
        assert!(known_remappers().iter().any(|(name, ..)| *name == "keyd"));
    }

    #[test]
    fn parses_literal_input_remapper_json_pairs() {
        let path = env::temp_dir().join(format!(
            "whykey-input-remapper-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, r#"[{"input":"CapsLock","output":"Ctrl_L"}]"#).unwrap();
        let transformations = parse_input_remapper_transformations(&path);
        assert_eq!(transformations, vec!["CapsLock -> Ctrl_L"]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_literal_caps_to_ctrl_across_json_and_yaml_fixtures() {
        let input_remapper =
            parse_input_remapper_transformations(&fixture_path("input-remapper-caps-to-ctrl.json"));
        assert_eq!(input_remapper, vec!["CapsLock -> Ctrl_L"]);

        let xremap = parse_xremap_transformations(&fixture_path("xremap-caps-to-ctrl.yml"));
        assert_eq!(xremap, vec!["CapsLock -> Ctrl_L"]);
    }

    #[test]
    fn keeps_lisp_tap_hold_and_layers_conditional() {
        let path = env::temp_dir().join(format!(
            "whykey-kanata-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "(defalias caps (tap-hold 200 200 esc lctl))\n(deflayer base @caps a)\n",
        )
        .unwrap();
        let transformations = parse_lisp_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("conditional alias"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("tap-hold"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("layered"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_fixture_tap_hold_without_flattening_it_to_a_keypress() {
        let transformations = parse_lisp_transformations(&fixture_path("kanata-tap-hold.kbd"));
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("caps -> (tap-hold") && value.contains("lctl"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("timing and active layer remain conditional"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("active layer and precedence remain conditional"))
        );
    }

    #[test]
    fn parses_xremap_literal_remap_entries() {
        let path = env::temp_dir().join(format!(
            "whykey-xremap-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "modmap:\n  - name: caps\n    remap:\n      CapsLock: Ctrl_L\n",
        )
        .unwrap();
        let transformations = parse_xremap_transformations(&path);
        assert_eq!(transformations, vec!["CapsLock -> Ctrl_L"]);
        let _ = fs::remove_file(path);
    }

    /// A detection built from configuration files written to a fresh
    /// directory, as `detect` would build it for those paths.
    fn detection(name: &str, running: bool, files: &[(&str, &str)]) -> Detection {
        device_detection(name, running, files, None)
    }

    /// As `detection`, for a configuration that applies to one named input
    /// device.
    fn device_detection(
        name: &str,
        running: bool,
        files: &[(&str, &str)],
        device: Option<&str>,
    ) -> Detection {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("whykey-remapper-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = files
            .iter()
            .map(|(file, content)| {
                let path = dir.join(file);
                fs::write(&path, content).unwrap();
                path
            })
            .collect();
        let located = Located {
            groups: vec![Group {
                paths: paths.clone(),
                device: device.map(DeviceScope::named),
                ..Group::default()
            }],
            ..Located::default()
        };
        Detection {
            name: name.into(),
            processes: if running {
                vec![format!("pid 1: {name}")]
            } else {
                Vec::new()
            },
            configurations: paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            transformations: Vec::new(),
            analysis: analyze(name, running, located),
        }
    }

    fn query(key: &str) -> KeyCombo {
        KeyCombo::parse_query(key).unwrap()
    }

    fn action(result: &LayerResult) -> Option<&str> {
        result
            .binding
            .as_ref()
            .and_then(|binding| binding.action.as_deref())
    }

    #[test]
    fn keyd_main_layer_remap_redirects_to_the_target_key() {
        let keyd = detection(
            "keyd",
            true,
            &[("default.conf", "[ids]\n*\n\n[main]\ncapslock = esc\n")],
        );
        let result = inspect_detections(&query("caps_lock"), None, &[keyd], false);
        assert_eq!(result.outcome, Outcome::Redirected);
        assert_eq!(action(&result), Some("ESCAPE"));
        assert_eq!(
            result
                .binding
                .as_ref()
                .and_then(|binding| binding.source.as_ref())
                .and_then(|source| source.line),
            Some(5)
        );
    }

    #[test]
    fn keyd_rule_on_a_keypad_key_matches_its_query() {
        let keyd = detection(
            "keyd",
            true,
            &[(
                "default.conf",
                "[ids]\n*\n\n[main]\nkpenter = esc\nkp1 = f5\n",
            )],
        );
        let enter =
            inspect_detections(&query("kp_enter"), None, std::slice::from_ref(&keyd), false);
        assert_eq!(enter.outcome, Outcome::Redirected);
        assert_eq!(action(&enter), Some("ESCAPE"));
        let digit = inspect_detections(&query("kp_1"), None, std::slice::from_ref(&keyd), false);
        assert_eq!(digit.outcome, Outcome::Redirected);
        assert_eq!(action(&digit), Some("F5"));
        // The main keys are other keys: the keypad rules leave them alone
        // (US layout, so "1" is known to be the top-row key).
        for main in ["return", "1"] {
            let result = inspect_detections(&query(main), None, std::slice::from_ref(&keyd), true);
            assert_eq!(result.outcome, Outcome::Pass, "{main}");
        }
    }

    #[test]
    fn keyd_layer_key_is_consumed() {
        let keyd = detection(
            "keyd",
            true,
            &[(
                "default.conf",
                "[ids]\n*\n[main]\ncapslock = layer(nav)\n[nav]\nh = left\n",
            )],
        );
        let result = inspect_detections(
            &query("caps_lock"),
            None,
            std::slice::from_ref(&keyd),
            false,
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        // The nav layer is not active without its layer key, and F5 has no rule.
        let unbound = inspect_detections(&query("f5"), None, &[keyd], false);
        assert_eq!(unbound.outcome, Outcome::Pass);
        // Rules come from keyd's configuration, whichever way they answer.
        assert_eq!(result.tier, Some(EvidenceTier::ConfigFile));
        assert_eq!(unbound.tier, Some(EvidenceTier::ConfigFile));
        assert_eq!(
            inspect_detections(&query("f5"), None, &[], false).tier,
            None
        );
    }

    #[test]
    fn kanata_default_layer_position_redirects() {
        let kanata = detection(
            "kanata",
            true,
            &[(
                "kanata.kbd",
                "(defsrc caps a)\n(deflayer base esc _)\n(deflayer other a a)\n",
            )],
        );
        let result = inspect_detections(&query("caps_lock"), None, &[kanata], false);
        assert_eq!(result.outcome, Outcome::Redirected);
        assert_eq!(action(&result), Some("ESCAPE"));
    }

    #[test]
    fn xremap_keymap_combo_redirects_when_the_key_position_is_known() {
        let xremap = detection(
            "xremap",
            true,
            &[(
                "config.yml",
                "keymap:\n  - name: emacs\n    remap:\n      C-a: C-b\n",
            )],
        );
        let result =
            inspect_detections(&query("ctrl+a"), None, std::slice::from_ref(&xremap), true);
        assert_eq!(result.outcome, Outcome::Redirected);
        assert_eq!(action(&result), Some("CTRL+B"));

        // A captured keycode proves the physical key; the emitted keysym
        // still depends on the layout, so the target is not a key name.
        let physical = PhysicalInput {
            device: Some("AT Translated Set 2 keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::new(30),
        };
        let captured = inspect_detections(
            &query("ctrl+a"),
            Some(&physical),
            std::slice::from_ref(&xremap),
            false,
        );
        assert_eq!(captured.outcome, Outcome::Redirected);
        assert!(action(&captured).is_some_and(|target| KeyCombo::parse_query(target).is_err()));

        // Without the layout, a printable key's position is an assumption.
        let assumed = inspect_detections(&query("ctrl+a"), None, &[xremap], false);
        assert_eq!(assumed.outcome, Outcome::HandledUncertain);
    }

    #[test]
    fn unreadable_configuration_is_never_a_pass() {
        let located = Located {
            groups: vec![Group {
                paths: vec![env::temp_dir().join("whykey-remapper-missing/never.kbd")],
                ..Group::default()
            }],
            ..Located::default()
        };
        let kanata = Detection {
            name: "kanata".into(),
            processes: vec!["pid 1: kanata".into()],
            configurations: Vec::new(),
            transformations: Vec::new(),
            analysis: analyze("kanata", true, located),
        };
        let result = inspect_detections(&query("f5"), None, &[kanata], false);
        assert_eq!(result.outcome, Outcome::Unavailable);
    }

    #[test]
    fn unevaluated_and_post_remap_inputs_stay_uncertain() {
        let kmonad = detection("kmonad", true, &[("kmonad.kbd", "(defsrc caps)\n")]);
        let result = inspect_detections(
            &query("caps_lock"),
            None,
            std::slice::from_ref(&kmonad),
            false,
        );
        assert_eq!(result.outcome, Outcome::UncertainContinues);

        let keyd = detection(
            "keyd",
            true,
            &[("default.conf", "[ids]\n*\n[main]\ncapslock = esc\n")],
        );
        let virtual_device = PhysicalInput {
            device: Some("keyd virtual keyboard".into()),
            keycode: crate::xkb::EvdevKeycode::new(1),
        };
        let observed = inspect_detections(&query("escape"), Some(&virtual_device), &[keyd], false);
        assert_eq!(observed.outcome, Outcome::UncertainContinues);
    }

    #[test]
    fn configuration_without_a_running_daemon_is_conditional() {
        let keyd = detection(
            "keyd",
            false,
            &[("default.conf", "[ids]\n*\n[main]\ncapslock = esc\n")],
        );
        let result = inspect_detections(&query("caps_lock"), None, &[keyd], false);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
    }

    fn captured(device: &str, keycode: u16) -> PhysicalInput {
        PhysicalInput {
            device: Some(device.into()),
            keycode: crate::xkb::EvdevKeycode::new(keycode),
        }
    }

    #[test]
    fn kmonad_first_layer_is_evaluated_for_its_keyboard() {
        let mut kmonad = detection(
            "kmonad",
            true,
            &[(
                "laptop.kbd",
                "(defcfg input (device-file \"/dev/input/by-id/laptop-kbd\") output (uinput-sink \"kmonad out\") fallthrough true)\n(defalias nav (layer-toggle nav) th (tap-hold 200 s lctl))\n(defsrc caps a s)\n(deflayer base esc @nav @th)\n(deflayer nav _ _ left)\n",
            )],
        );
        // The device file is not present, so the key is conditional on
        // coming from that keyboard.
        let unscoped =
            inspect_with_detections(&query("caps_lock"), None, std::slice::from_ref(&kmonad));
        assert_eq!(unscoped.outcome, Outcome::HandledUncertain);
        assert!(
            unscoped
                .details
                .iter()
                .any(|detail| detail.contains("reads only /dev/input/by-id/laptop-kbd")),
            "{unscoped:?}"
        );

        let Engine::Kmonad(configs) = &mut kmonad.analysis.engine else {
            panic!("kmonad is evaluated: {:?}", kmonad.analysis);
        };
        for config in configs.iter_mut() {
            *config = config
                .clone()
                .with_device(DeviceScope::named("Laptop Keyboard"));
        }
        let kmonad = std::slice::from_ref(&kmonad);
        let keyboard = |code| captured("laptop-keyboard", code);
        let redirected = inspect_with_detections(&query("caps_lock"), Some(&keyboard(58)), kmonad);
        assert_eq!(redirected.outcome, Outcome::Redirected);
        assert_eq!(action(&redirected), Some("ESCAPE"));
        assert_eq!(
            inspect_with_detections(&query("a"), Some(&keyboard(30)), kmonad).outcome,
            Outcome::Consumed
        );
        assert_eq!(
            inspect_with_detections(&query("s"), Some(&keyboard(31)), kmonad).outcome,
            Outcome::HandledUncertain
        );
        // Fallthrough passes keys outside defsrc.
        assert_eq!(
            inspect_with_detections(&query("f5"), Some(&keyboard(63)), kmonad).outcome,
            Outcome::Pass
        );

        let located = Located {
            groups: vec![Group {
                paths: vec![env::temp_dir().join("whykey-remapper-missing/kmonad.kbd")],
                ..Group::default()
            }],
            ..Located::default()
        };
        let unreadable = Detection {
            analysis: analyze("kmonad", true, located),
            ..kmonad[0].clone()
        };
        assert_eq!(
            inspect_with_detections(&query("caps_lock"), None, &[unreadable]).outcome,
            Outcome::Unavailable
        );
    }

    #[test]
    fn input_remapper_autoloaded_preset_is_evaluated_for_its_device() {
        let preset = r#"[
            {"input_combination": [{"type": 1, "code": 58, "origin_hash": "1a2b"}], "target_uinput": "keyboard", "output_symbol": "Escape"},
            {"input_combination": [{"type": 1, "code": 30}], "target_uinput": "keyboard", "output_symbol": "key(KEY_B).key(KEY_C)"},
            {"input_combination": [{"type": 1, "code": 29}, {"type": 1, "code": 31}], "target_uinput": "keyboard", "output_symbol": "KEY_X"}
        ]"#;
        let remapper = device_detection(
            "input-remapper",
            true,
            &[("work.json", preset)],
            Some("AT Translated Set 2 keyboard"),
        );
        let remapper = std::slice::from_ref(&remapper);
        let keyboard = |code| captured("at-translated-set-2-keyboard", code);
        let redirected =
            inspect_with_detections(&query("caps_lock"), Some(&keyboard(58)), remapper);
        assert_eq!(redirected.outcome, Outcome::Redirected);
        assert_eq!(action(&redirected), Some("ESCAPE"));
        assert_eq!(
            inspect_with_detections(&query("a"), Some(&keyboard(30)), remapper).outcome,
            Outcome::Consumed
        );
        // The combination fires only while Control is held.
        assert_eq!(
            inspect_with_detections(&query("s"), Some(&keyboard(31)), remapper).outcome,
            Outcome::HandledUncertain
        );
        // Another keyboard, or an unknown one, may not use this preset.
        for other in [None, Some(captured("usb-keyboard", 58))] {
            assert_eq!(
                inspect_with_detections(&query("caps_lock"), other.as_ref(), remapper).outcome,
                Outcome::HandledUncertain
            );
        }

        let missing = Located {
            groups: vec![Group {
                paths: vec![env::temp_dir().join("whykey-remapper-missing/preset.json")],
                device: Some(DeviceScope::named("AT Translated Set 2 keyboard")),
                ..Group::default()
            }],
            ..Located::default()
        };
        let unreadable = Detection {
            analysis: analyze("input-remapper", true, missing),
            ..remapper[0].clone()
        };
        assert_eq!(
            inspect_with_detections(&query("caps_lock"), Some(&keyboard(58)), &[unreadable])
                .outcome,
            Outcome::Unavailable
        );
        // A running service with no autoloaded preset may run one applied
        // from its window.
        let idle = Detection {
            analysis: analyze("input-remapper", true, Located::default()),
            ..remapper[0].clone()
        };
        assert_eq!(
            inspect_with_detections(&query("caps_lock"), None, &[idle]).outcome,
            Outcome::UncertainContinues
        );
    }

    #[test]
    fn inspection_routes_through_the_remapped_key() {
        use crate::layers::InspectRequest;

        let mut session = crate::environment::Environment::collect();
        let key = query("caps_lock");

        session.remappers = vec![detection(
            "keyd",
            true,
            &[("default.conf", "[ids]\n*\n[main]\ncapslock = esc\n")],
        )];
        let results = crate::layers::inspection::inspect_with_environment(
            &InspectRequest::new(&key),
            &session,
        );
        assert_eq!(results[0].id, LayerId::Remapper);
        assert_eq!(results[0].outcome, Outcome::Redirected);
        assert_eq!(action(&results[0]), Some("ESCAPE"));
        assert!(
            results.len() > 1,
            "downstream layers inspect the remapped key"
        );

        session.remappers = vec![detection(
            "keyd",
            true,
            &[(
                "default.conf",
                "[ids]\n*\n[main]\ncapslock = layer(nav)\n[nav]\nh = left\n",
            )],
        )];
        let results = crate::layers::inspection::inspect_with_environment(
            &InspectRequest::new(&key),
            &session,
        );
        assert_eq!(
            results.len(),
            1,
            "a consuming remapper stops the chain: {results:?}"
        );
        assert_eq!(results[0].outcome, Outcome::Consumed);
    }

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/remappers")
            .join(name)
    }

    #[test]
    fn a_lone_modifier_reaches_only_the_remapper_and_the_compositor() {
        use crate::layers::InspectRequest;

        let mut session = crate::environment::Environment::collect();
        session.remappers = vec![detection(
            "keyd",
            true,
            &[(
                "default.conf",
                "[ids]\n*\n[main]\nleftmeta = overload(control, esc)\n",
            )],
        )];
        let key = KeyCombo::parse_alone("super").unwrap();
        let results = crate::layers::inspection::inspect_with_environment(
            &InspectRequest::new(&key),
            &session,
        );
        assert_eq!(results[0].id, LayerId::Remapper);
        assert_eq!(results[0].tier, Some(EvidenceTier::ConfigFile));
        let text = format!("{} {:?}", results[0].summary, results[0].details);
        assert!(text.contains("a tap emits esc"), "{text}");
        for layer in &results {
            assert!(
                matches!(layer.id, LayerId::Remapper | LayerId::Compositor),
                "a lone modifier produces no terminal bytes: {:?}",
                layer.layer
            );
        }
    }
}
