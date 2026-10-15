//! Static evaluation of keyd configuration (`/etc/keyd/*.conf`).
//!
//! Only the layers that apply without a held layer key are evaluated:
//! `[main]`, the built-in modifier layers for the queried modifiers
//! (`[control]`, `[shift]`, `[alt]`, `[meta]`, `[altgr]`), their composite
//! layers, and a `default_layout`. Bindings in other layers are reported
//! only when their activation would change the answer.

use crate::recording::PathInputs;
use std::path::{Path, PathBuf};

use super::keys::{self, ALT, ALTGR, CTRL, Press, SHIFT, SUPER};
use super::{Effect, Finding, Rule};

/// Built-in modifier layers and the modifier each one emulates.
const MODIFIER_LAYERS: &[(&str, u32)] = &[
    ("control", CTRL),
    ("shift", SHIFT),
    ("alt", ALT),
    ("meta", SUPER),
    ("altgr", ALTGR),
];

/// Default aliases: modifier names that stand for both physical keys.
const DEFAULT_ALIASES: &[(&str, &[u16])] = &[
    ("control", &[29, 97]),
    ("shift", &[42, 54]),
    ("meta", &[125, 126]),
    ("alt", &[56]),
    ("altgr", &[100]),
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Keymap {
    files: Vec<File>,
    /// Keys `keyd-application-mapper` rebinds for some windows.
    app_bindings: Vec<(u16, Rule)>,
    /// `vendor:product` ids of present input devices, when readable.
    present_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct File {
    path: String,
    scope: Scope,
    default_layout: Option<String>,
    layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    All,
    AllExcept(Vec<String>),
    Only(Vec<String>),
    /// No `[ids]` section: keyd does not match the file to a keyboard.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Layer {
    name: String,
    mods: u32,
    bindings: Vec<Binding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    codes: Vec<u16>,
    action: String,
    rule: Rule,
}

impl Keymap {
    /// Parse the daemon's `.conf` files. Unreadable files are returned as
    /// `path: reason` so the caller never treats them as absent.
    pub(crate) fn load(
        paths: &[PathBuf],
        app_config: Option<&Path>,
        present_ids: Option<Vec<String>>,
    ) -> (Self, Vec<String>) {
        let mut keymap = Self {
            present_ids,
            ..Self::default()
        };
        let mut unreadable = Vec::new();
        for path in paths {
            match super::read_config(path) {
                Ok(content) => keymap
                    .files
                    .push(parse_file(path, &content, &mut unreadable)),
                Err(reason) => unreadable.push(reason),
            }
        }
        if let Some(path) = app_config {
            if let Ok(content) = super::read_config(path) {
                keymap.app_bindings = parse_app_bindings(path, &content);
            }
        }
        (keymap, unreadable)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every configuration that may apply to the keyboard, with keyd's
    /// effect on `press` under each.
    pub(crate) fn evaluate(&self, press: Press) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut covered = false;
        for file in &self.files {
            let applies = match &file.scope {
                Scope::All => {
                    covered = true;
                    true
                }
                Scope::AllExcept(excluded) => {
                    if !self.any_present(excluded) {
                        covered = true;
                    }
                    true
                }
                Scope::Only(ids) => self.present_ids.is_none() || self.any_present(ids),
                Scope::Missing => true,
            };
            if applies {
                findings.push(file.evaluate(press));
            }
        }
        if !covered {
            findings.push(Finding::untouched_because(
                "no keyd configuration matches every keyboard ([ids] lists specific devices or exclusions)",
            ));
        }
        if let Some((_, rule)) = self
            .app_bindings
            .iter()
            .find(|(code, _)| *code == press.code)
        {
            findings.push(Finding {
                effect: Effect::Conditional(format!(
                    "keyd-application-mapper rebinds {} for some windows",
                    keys::evdev_name(press.code)
                )),
                rule: Some(rule.clone()),
            });
        }
        findings
    }

    fn any_present(&self, ids: &[String]) -> bool {
        let Some(present) = &self.present_ids else {
            return true;
        };
        ids.iter().any(|id| {
            let id = id
                .strip_prefix("k:")
                .or_else(|| id.strip_prefix("m:"))
                .unwrap_or(id)
                .to_ascii_lowercase();
            present
                .iter()
                .any(|candidate| id.starts_with(candidate.as_str()))
        })
    }
}

impl File {
    fn layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.name == name)
    }

    fn layer_mods(&self, name: &str) -> u32 {
        MODIFIER_LAYERS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, mods)| *mods)
            .or_else(|| self.layer(name).map(|layer| layer.mods))
            .unwrap_or(0)
    }

    fn evaluate(&self, press: Press) -> Finding {
        let held: Vec<&str> = keys::modifier_bits(press.mods)
            .filter_map(|bit| {
                MODIFIER_LAYERS
                    .iter()
                    .find(|(_, mods)| *mods == bit)
                    .map(|(name, _)| *name)
            })
            .collect();
        match self.custom_modifier_layer_rule(press, &held) {
            Some(rule) => Finding {
                effect: Effect::Conditional(
                    "a custom keyd layer with the held modifiers binds this key; the result depends on which key holds the modifier"
                        .into(),
                ),
                rule: Some(rule),
            },
            None => self.lookup(press, &held),
        }
    }

    fn lookup(&self, press: Press, held: &[&str]) -> Finding {
        if held.len() > 1 {
            let chord = self.layers.iter().find(|layer| {
                let parts: Vec<&str> = layer.name.split('+').collect();
                parts.len() == held.len() && held.iter().all(|name| parts.contains(name))
            });
            if let Some(layer) = chord {
                if let Some(binding) = layer.binding(press.code) {
                    return self.binding_finding(binding, 0, &layer.name);
                }
            }
        }
        let mut layered: Vec<Finding> = Vec::new();
        for name in held {
            if let Some(binding) = self.layer(name).and_then(|layer| layer.binding(press.code)) {
                let carried = press.mods & !self.layer_mods(name);
                layered.push(self.binding_finding(binding, carried, name));
            }
        }
        match layered.len() {
            0 => {}
            1 => return layered.remove(0),
            _ => {
                let first = layered.remove(0);
                if layered.iter().all(|other| other.effect == first.effect) {
                    return first;
                }
                return Finding {
                    effect: Effect::Conditional(
                        "several held modifier layers bind this key; keyd uses the most recently activated one"
                            .into(),
                    ),
                    rule: first.rule,
                };
            }
        }
        for name in self
            .default_layout
            .iter()
            .map(String::as_str)
            .chain(["main"])
        {
            if let Some(binding) = self.layer(name).and_then(|layer| layer.binding(press.code)) {
                return self.binding_finding(binding, press.mods, name);
            }
        }
        Finding::untouched()
    }

    /// A non-built-in layer whose modifiers are all held binds the key, and
    /// some key activates it: the modifier may come from that layer.
    fn custom_modifier_layer_rule(&self, press: Press, held: &[&str]) -> Option<Rule> {
        self.layers
            .iter()
            .filter(|layer| {
                layer.mods != 0
                    && press.mods & layer.mods == layer.mods
                    && !held.contains(&layer.name.as_str())
                    && !MODIFIER_LAYERS.iter().any(|(name, _)| *name == layer.name)
                    && self.activates(&layer.name)
            })
            .find_map(|layer| layer.binding(press.code))
            .map(|binding| binding.rule.clone())
    }

    fn activates(&self, layer: &str) -> bool {
        self.layers.iter().flat_map(|l| &l.bindings).any(|binding| {
            action_call(&binding.action)
                .is_some_and(|(_, args)| args.split(',').next().map(str::trim) == Some(layer))
        })
    }

    fn binding_finding(&self, binding: &Binding, carried: u32, layer: &str) -> Finding {
        Finding {
            effect: self.action_effect(&binding.action, carried, layer),
            rule: Some(binding.rule.clone()),
        }
    }

    fn action_effect(&self, action: &str, carried: u32, layer: &str) -> Effect {
        if action == "noop" {
            return Effect::Consume("disabled by keyd (noop)".into());
        }
        if let Some((name, args)) = action_call(action) {
            let target = args.split(',').next().unwrap_or("").trim();
            return match name {
                "layer" => {
                    let mods = self.layer_mods(target);
                    let mut bits = keys::modifier_bits(mods);
                    match (bits.next(), bits.next()) {
                        (Some(bit), None) => {
                            Effect::Remap(Press::new(carried, keys::modifier_codes(bit)[0]))
                        }
                        _ => Effect::Consume(format!("activates keyd layer {target} while held")),
                    }
                }
                "toggle" | "togglem" => Effect::Consume(format!("toggles keyd layer {target}")),
                "swap" | "swapm" => Effect::Consume(format!("swaps in keyd layer {target}")),
                "layerm" => Effect::Consume(format!(
                    "runs a macro and activates keyd layer {target} while held"
                )),
                "setlayout" => Effect::Consume(format!("sets keyd layout {target}")),
                "clear" | "clearm" => Effect::Consume("clears keyd layers".into()),
                "macro" | "macro2" => Effect::Consume(format!("runs keyd macro {action}")),
                "command" => Effect::Consume("runs a keyd shell command".into()),
                "repeat" => Effect::Consume("repeats the last keyd key or macro".into()),
                "oneshot" | "oneshotm" | "oneshotk" => Effect::Conditional(format!(
                    "one-shot layer {target}: the effect depends on the next key and timing"
                )),
                "overload" | "overloadt" | "overloadt2" | "overloadi" | "lettermod" | "timeout" => {
                    Effect::Conditional(format!(
                        "tap-versus-hold action {action}: the result depends on timing"
                    ))
                }
                _ => Effect::Conditional(format!("keyd action {action} is not evaluated")),
            };
        }
        if !action.is_ascii() {
            return Effect::Consume("types text through keyd's compose macro".into());
        }
        if action.split_whitespace().nth(1).is_some() {
            return Effect::Consume(format!("runs keyd macro {action}"));
        }
        match parse_combo(action) {
            Some(emitted) => Effect::Remap(Press::new(carried | emitted.mods, emitted.code)),
            None => {
                Effect::Conditional(format!("keyd {layer} action {action} names an unknown key"))
            }
        }
    }
}

impl Layer {
    /// The effective binding: keyd lets the most recent binding win.
    fn binding(&self, code: u16) -> Option<&Binding> {
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.codes.contains(&code))
    }
}

fn action_call(action: &str) -> Option<(&str, &str)> {
    let (name, rest) = action.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    name.chars()
        .all(|character| character.is_ascii_alphanumeric())
        .then_some((name, args))
}

/// A keyd key name or `C-`/`M-`/`A-`/`S-`/`G-` modifier expression.
fn parse_combo(expression: &str) -> Option<Press> {
    if let Some(press) = key(expression) {
        return Some(press);
    }
    let (prefix, name) = if let Some(prefix) = expression.strip_suffix("--") {
        (prefix, "-")
    } else {
        expression.rsplit_once('-')?
    };
    let mut mods = 0;
    for modifier in prefix.split('-') {
        mods |= match modifier {
            "C" => CTRL,
            "M" => SUPER,
            "A" => ALT,
            "S" => SHIFT,
            "G" => ALTGR,
            _ => return None,
        };
    }
    let base = key(name)?;
    Some(Press::new(mods | base.mods, base.code))
}

/// A keyd key name, including its shifted-symbol names (`!`, `A`, ...).
fn key(name: &str) -> Option<Press> {
    let code = match name {
        "esc" | "escape" => Some(1),
        "leftcontrol" => Some(29),
        "rightcontrol" => Some(97),
        "-" => Some(12),
        "=" => Some(13),
        "[" => Some(26),
        "]" => Some(27),
        ";" => Some(39),
        "'" => Some(40),
        "`" => Some(41),
        "\\" => Some(43),
        "," => Some(51),
        "." => Some(52),
        "/" => Some(53),
        _ => None,
    };
    if let Some(code) = code {
        return Some(Press::new(0, code));
    }
    let shifted = match name {
        "!" => Some(2),
        "@" => Some(3),
        "#" => Some(4),
        "$" => Some(5),
        "%" => Some(6),
        "^" => Some(7),
        "&" => Some(8),
        "*" => Some(9),
        "(" => Some(10),
        ")" => Some(11),
        "_" => Some(12),
        "+" => Some(13),
        "{" => Some(26),
        "}" => Some(27),
        ":" => Some(39),
        "\"" => Some(40),
        "~" => Some(41),
        "|" => Some(43),
        "<" => Some(51),
        ">" => Some(52),
        "?" => Some(53),
        upper if upper.len() == 1 && upper.chars().all(|c| c.is_ascii_uppercase()) => {
            keys::evdev_code(upper)
        }
        _ => None,
    };
    if let Some(code) = shifted {
        return Some(Press::new(SHIFT, code));
    }
    if name.chars().any(|character| character.is_ascii_uppercase()) {
        return None;
    }
    keys::evdev_code(name).map(|code| Press::new(0, code))
}

/// Left-hand side key codes: user aliases, default aliases, or a key name.
/// Chords (`j+k`) are timing-dependent and never match a single key.
fn lhs_codes(lhs: &str, aliases: &[(String, u16)]) -> Vec<u16> {
    if lhs.len() > 1 && lhs.contains('+') {
        return Vec::new();
    }
    let user: Vec<u16> = aliases
        .iter()
        .filter(|(name, _)| name == lhs)
        .map(|(_, code)| *code)
        .collect();
    if !user.is_empty() {
        return user;
    }
    if let Some((_, codes)) = DEFAULT_ALIASES.iter().find(|(name, _)| *name == lhs) {
        return codes.to_vec();
    }
    key(lhs)
        .filter(|press| press.mods == 0)
        .map(|press| vec![press.code])
        .unwrap_or_default()
}

/// Split `lhs = rhs`, allowing `=` itself as the left-hand key.
fn split_binding(line: &str) -> Option<(&str, &str)> {
    let offset = usize::from(line.starts_with('='));
    let index = line[offset..].find('=')? + offset;
    let lhs = line[..index].trim();
    let rhs = line[index + 1..].trim();
    (!lhs.is_empty() && !rhs.is_empty()).then_some((lhs, rhs))
}

fn parse_file(path: &Path, content: &str, unreadable: &mut Vec<String>) -> File {
    let display = path.display().to_string();
    let mut lines: Vec<(String, u32, String)> = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("include ") {
            match include_path(path, name.trim()).map(|included| {
                let content = super::read_config(&included);
                (included, content)
            }) {
                Some((included, Ok(content))) => {
                    let included = included.display().to_string();
                    lines.extend(content.lines().enumerate().map(|(index, line)| {
                        (
                            included.clone(),
                            u32::try_from(index + 1).unwrap_or(u32::MAX),
                            line.trim().to_owned(),
                        )
                    }));
                }
                Some((_, Err(reason))) => unreadable.push(reason),
                None => unreadable.push(format!(
                    "{display}:{number}: include {} was not found",
                    name.trim()
                )),
            }
            continue;
        }
        lines.push((display.clone(), number, trimmed.to_owned()));
    }

    let mut file = File {
        path: display,
        scope: Scope::Missing,
        default_layout: None,
        layers: Vec::new(),
    };
    let mut wildcard = false;
    let mut excluded = Vec::new();
    let mut explicit = Vec::new();
    let mut saw_ids = false;
    let mut aliases: Vec<(String, u16)> = Vec::new();
    let mut section = String::new();
    // Bindings are resolved after aliases, which may follow them.
    let mut pending: Vec<(String, String, String, Rule)> = Vec::new();
    for (source, number, line) in &lines {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let (name, mods) = header.split_once(':').unwrap_or((header, ""));
            section = name.trim().to_owned();
            saw_ids |= section == "ids";
            if !matches!(section.as_str(), "ids" | "global" | "aliases")
                && file.layer(&section).is_none()
            {
                file.layers.push(Layer {
                    name: section.clone(),
                    mods: layer_modifiers(mods),
                    bindings: Vec::new(),
                });
            }
            continue;
        }
        match section.as_str() {
            "ids" => {
                if line == "*" {
                    wildcard = true;
                } else if let Some(id) = line.strip_prefix('-') {
                    excluded.push(id.trim().to_owned());
                } else {
                    explicit.push(line.clone());
                }
            }
            "global" => {
                if let Some((name, value)) = split_binding(line) {
                    if name == "default_layout" {
                        file.default_layout = Some(value.to_owned());
                    }
                }
            }
            "aliases" => {
                if let Some((name, alias)) = split_binding(line) {
                    if let Some(press) = key(name) {
                        aliases.push((alias.to_owned(), press.code));
                        // An alias that is itself a key name remaps the key.
                        if key(alias).is_some() {
                            pending.push((
                                "main".into(),
                                name.to_owned(),
                                alias.to_owned(),
                                Rule::new(source, *number, line),
                            ));
                        }
                    }
                }
            }
            "" => {}
            layer => {
                if let Some((lhs, rhs)) = split_binding(line) {
                    pending.push((
                        layer.to_owned(),
                        lhs.to_owned(),
                        rhs.to_owned(),
                        Rule::new(source, *number, line),
                    ));
                }
            }
        }
    }
    for (layer, lhs, rhs, rule) in pending {
        let codes = lhs_codes(&lhs, &aliases);
        if codes.is_empty() {
            continue;
        }
        if file.layer(&layer).is_none() {
            file.layers.push(Layer {
                name: layer.clone(),
                mods: 0,
                bindings: Vec::new(),
            });
        }
        if let Some(target) = file.layers.iter_mut().find(|l| l.name == layer) {
            target.bindings.push(Binding {
                codes,
                action: rhs,
                rule,
            });
        }
    }
    file.scope = if !saw_ids {
        Scope::Missing
    } else if wildcard && excluded.is_empty() {
        Scope::All
    } else if wildcard {
        Scope::AllExcept(excluded)
    } else {
        Scope::Only(explicit)
    };
    file
}

fn layer_modifiers(spec: &str) -> u32 {
    spec.split('-')
        .map(|modifier| match modifier.trim() {
            "C" => CTRL,
            "M" => SUPER,
            "A" => ALT,
            "S" => SHIFT,
            "G" => ALTGR,
            _ => 0,
        })
        .fold(0, |mods, bit| mods | bit)
}

/// keyd resolves includes in the config directory, then `/usr/share/keyd`.
fn include_path(config: &Path, name: &str) -> Option<PathBuf> {
    if name.contains("..") {
        return None;
    }
    config
        .parent()
        .map(|dir| dir.join(name))
        .into_iter()
        .chain([Path::new("/usr/share/keyd").join(name)])
        .find(|candidate| candidate.recorded_is_file())
}

/// `keyd-application-mapper` bindings: `[class]` sections of
/// `[layer.]key = action` lines.
fn parse_app_bindings(path: &Path, content: &str) -> Vec<(u16, Rule)> {
    let display = path.display().to_string();
    let mut bindings = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((lhs, _)) = split_binding(line) else {
            continue;
        };
        let key_name = lhs.rsplit_once('.').map_or(lhs, |(_, key)| key);
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        for code in lhs_codes(key_name, &[]) {
            bindings.push((code, Rule::new(&display, number, line)));
        }
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap(content: &str) -> Keymap {
        let mut unreadable = Vec::new();
        let file = parse_file(
            Path::new("/etc/keyd/default.conf"),
            content,
            &mut unreadable,
        );
        assert!(unreadable.is_empty());
        Keymap {
            files: vec![file],
            app_bindings: Vec::new(),
            present_ids: Some(Vec::new()),
        }
    }

    fn only(findings: Vec<Finding>) -> Effect {
        assert_eq!(findings.len(), 1, "{findings:?}");
        findings.into_iter().next().unwrap().effect
    }

    #[test]
    fn main_layer_key_is_remapped_and_modifiers_carry() {
        let keymap = keymap("[ids]\n*\n\n[main]\ncapslock = esc\na = C-b\n");
        assert_eq!(
            only(keymap.evaluate(Press::new(0, 58))),
            Effect::Remap(Press::new(0, 1))
        );
        assert_eq!(
            only(keymap.evaluate(Press::new(SHIFT, 58))),
            Effect::Remap(Press::new(SHIFT, 1))
        );
        assert_eq!(
            only(keymap.evaluate(Press::new(0, 30))),
            Effect::Remap(Press::new(CTRL, 48))
        );
        assert_eq!(only(keymap.evaluate(Press::new(0, 59))), Effect::Untouched);
    }

    #[test]
    fn layer_actions_consume_and_modifier_layers_apply_to_held_modifiers() {
        let keymap = keymap(
            "[ids]\n*\n[main]\ncapslock = layer(nav)\nesc = noop\n[nav]\nh = left\n[control]\nj = down\n",
        );
        assert!(matches!(
            only(keymap.evaluate(Press::new(0, 58))),
            Effect::Consume(_)
        ));
        assert!(matches!(
            only(keymap.evaluate(Press::new(0, 1))),
            Effect::Consume(_)
        ));
        // [nav] is not active without its layer key.
        assert_eq!(only(keymap.evaluate(Press::new(0, 35))), Effect::Untouched);
        // Ctrl+j emits an unmodified Down.
        assert_eq!(
            only(keymap.evaluate(Press::new(CTRL, 36))),
            Effect::Remap(Press::new(0, 108))
        );
    }

    #[test]
    fn overload_is_conditional_and_modifier_layer_acts_as_modifier() {
        let keymap =
            keymap("[ids]\n*\n[main]\ncapslock = overload(control, esc)\ntab = layer(control)\n");
        assert!(matches!(
            only(keymap.evaluate(Press::new(0, 58))),
            Effect::Conditional(_)
        ));
        assert_eq!(
            only(keymap.evaluate(Press::new(0, 15))),
            Effect::Remap(Press::new(0, 29))
        );
    }

    #[test]
    fn device_scoped_files_keep_the_unmanaged_keyboard_possible() {
        let mut keymap = keymap("[ids]\n046d:c52b\n[main]\ncapslock = esc\n");
        keymap.present_ids = None;
        let findings = keymap.evaluate(Press::new(0, 58));
        assert!(findings.iter().any(|f| f.effect == Effect::Untouched));
        assert!(
            findings
                .iter()
                .any(|f| f.effect == Effect::Remap(Press::new(0, 1)))
        );
        // A listed device that is not present does not apply.
        keymap.present_ids = Some(vec!["1234:5678".into()]);
        assert_eq!(only(keymap.evaluate(Press::new(0, 58))), Effect::Untouched);
    }
}
