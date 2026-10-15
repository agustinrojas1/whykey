//! Static evaluation of xremap configurations: `modmap` and `keymap` lists
//! whose `remap` maps use scalar keys. The YAML is read with yaml-rust2;
//! aliases, several documents, complex mapping keys, and nesting deeper than
//! [`MAX_NESTING`] make the whole file unevaluated rather than half-read.

use std::path::Path;

use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::TScalarStyle;

use super::keys::{self, ALT, ALTGR, CTRL, Press, SHIFT, SUPER};
use super::{Effect, Finding, Rule};

/// Entry keys that restrict a remap to some windows, devices, or modes.
const CONDITIONS: &[&str] = &["application", "window", "device", "mode"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Yaml {
    Null,
    Scalar(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml, u32)>),
}

impl Yaml {
    fn get(&self, name: &str) -> Option<&Yaml> {
        match self {
            Self::Map(entries) => entries
                .iter()
                .find(|(key, ..)| key == name)
                .map(|(_, value, _)| value),
            _ => None,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Scalar(text) => text.clone(),
            Self::Seq(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Yaml::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, value, _)| format!("{key}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    conditional: bool,
    exact_match: bool,
    remap: Vec<(String, Yaml, Rule)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Config {
    modmap: Vec<Entry>,
    keymap: Vec<Entry>,
    /// `--device` limits the keyboards xremap grabs.
    device_scoped: bool,
}

impl Config {
    pub(crate) fn with_device_scope(mut self, scoped: bool) -> Self {
        self.device_scoped = scoped;
        self
    }

    /// Append one file; xremap merges every configuration it is given.
    pub(crate) fn extend(&mut self, path: &Path, content: &str) -> Result<(), String> {
        let display = path.display().to_string();
        let document = parse(content).map_err(|error| format!("{display}: {error}"))?;
        let Yaml::Map(_) = document else {
            return Err(format!("{display}: the top level is not a mapping"));
        };
        for (name, target) in [("modmap", &mut self.modmap), ("keymap", &mut self.keymap)] {
            match document.get(name) {
                None | Some(Yaml::Null) => {}
                Some(Yaml::Seq(items)) => {
                    for item in items {
                        target.push(entry(item, &display, name)?);
                    }
                }
                Some(_) => return Err(format!("{display}: {name} is not a list")),
            }
        }
        Ok(())
    }

    pub(crate) fn evaluate(&self, press: Press) -> Vec<Finding> {
        let mut findings = vec![self.evaluate_key(press)];
        if self.device_scoped {
            findings.push(Finding::untouched_because(
                "xremap was started with --device and may not grab this keyboard",
            ));
        }
        findings
    }

    fn evaluate_key(&self, press: Press) -> Finding {
        for bit in keys::modifier_bits(press.mods) {
            let remapped: Vec<Rule> = keys::modifier_codes(bit)
                .iter()
                .filter_map(|code| match self.modmap_for(*code) {
                    Some((Effect::Remap(output), _)) if output.code == *code => None,
                    Some((_, rule)) => Some(rule),
                    None => None,
                })
                .collect();
            if remapped.len() == keys::modifier_codes(bit).len() {
                return Finding {
                    effect: Effect::Conditional(
                        "xremap's modmap remaps the held modifier keys".into(),
                    ),
                    rule: remapped.into_iter().next(),
                };
            }
        }
        let (code, modmap_rule) = match self.modmap_for(press.code) {
            Some((Effect::Remap(output), rule)) => (output.code, Some(rule)),
            Some((effect, rule)) => {
                return Finding {
                    effect,
                    rule: Some(rule),
                };
            }
            None => (press.code, None),
        };
        let modmapped = Press::new(press.mods, code);
        if let Some(finding) = self.keymap_for(modmapped) {
            return finding;
        }
        if modmapped == press {
            Finding::untouched()
        } else {
            Finding {
                effect: Effect::Remap(modmapped),
                rule: modmap_rule,
            }
        }
    }

    /// The first modmap entry that names `code`.
    fn modmap_for(&self, code: u16) -> Option<(Effect, Rule)> {
        for entry in &self.modmap {
            let Some((_, value, rule)) = entry
                .remap
                .iter()
                .find(|(from, ..)| parse_key(from) == Some(code))
            else {
                continue;
            };
            let effect = if entry.conditional {
                Effect::Conditional(
                    "the xremap modmap applies only to some windows, devices, or modes".into(),
                )
            } else {
                match value {
                    Yaml::Scalar(to) => match parse_key(to) {
                        Some(to) => Effect::Remap(Press::new(0, to)),
                        None => Effect::Conditional(format!("xremap key {to} is not recognized")),
                    },
                    Yaml::Seq(items) => Effect::Consume(format!(
                        "xremap presses several keys: {}",
                        Yaml::Seq(items.clone()).render()
                    )),
                    Yaml::Map(_) => {
                        let held = value.get("held").map(Yaml::render);
                        let alone = value.get("alone").map(Yaml::render);
                        match (alone, held) {
                            (Some(alone), Some(held)) => Effect::Conditional(format!(
                                "multi-purpose xremap key: alone emits {alone}, held acts as \
                                 {held}; timing decides"
                            )),
                            _ => Effect::Conditional(format!(
                                "multi-purpose xremap key {}: the result depends on timing",
                                value.render()
                            )),
                        }
                    }
                    Yaml::Null => Effect::Consume("disabled by xremap".into()),
                }
            };
            return Some((effect, rule.clone()));
        }
        None
    }

    /// xremap prefers a remap whose modifiers match exactly, then an
    /// inexact one that carries the extra held modifiers through.
    fn keymap_for(&self, press: Press) -> Option<Finding> {
        let held = classes(press.mods);
        let mut inexact: Option<Finding> = None;
        for entry in &self.keymap {
            for (from, value, rule) in &entry.remap {
                let Some(trigger) = parse_combo(from) else {
                    continue;
                };
                if trigger.code != press.code {
                    continue;
                }
                if trigger.side_specific & held != 0 {
                    return Some(Finding {
                        effect: Effect::Conditional(format!(
                            "xremap binds {from} to one side's modifier; the held side is not known"
                        )),
                        rule: Some(rule.clone()),
                    });
                }
                let trigger_classes = classes(trigger.mods);
                let exact = trigger_classes == held;
                if !exact
                    && (entry.exact_match || trigger_classes & !held != 0 || inexact.is_some())
                {
                    continue;
                }
                let carried = press.mods & !class_bits(trigger_classes);
                let finding = Finding {
                    effect: if entry.conditional {
                        Effect::Conditional(
                            "the xremap keymap applies only to some windows, devices, or modes"
                                .into(),
                        )
                    } else {
                        action_effect(value, carried)
                    },
                    rule: Some(rule.clone()),
                };
                if exact {
                    return Some(finding);
                }
                inexact = Some(finding);
            }
        }
        inexact
    }
}

fn action_effect(value: &Yaml, carried: u32) -> Effect {
    match value {
        Yaml::Scalar(text) => match parse_combo(text) {
            Some(combo) if combo.side_specific == 0 => {
                Effect::Remap(Press::new(carried | combo.mods, combo.code))
            }
            // The emitted keys are known, but a modifier mask cannot name the side.
            Some(_) => {
                Effect::Conditional(format!("xremap emits {text} with a side-specific modifier"))
            }
            None => Effect::Conditional(format!("xremap action {text} is not recognized")),
        },
        Yaml::Seq(items) if items.len() == 1 => action_effect(&items[0], carried),
        Yaml::Seq(items) if items.is_empty() => Effect::Consume("disabled by xremap".into()),
        Yaml::Seq(_) => Effect::Consume(format!("xremap emits {}", value.render())),
        Yaml::Null => Effect::Consume("disabled by xremap".into()),
        Yaml::Map(_) => {
            if value.get("remap").is_some() {
                if value.get("timeout_key").is_some() {
                    Effect::Conditional("starts an xremap key sequence with a timeout key".into())
                } else {
                    Effect::Consume(
                        "starts an xremap key sequence; xremap waits for the next key".into(),
                    )
                }
            } else if value.get("launch").is_some() {
                Effect::Consume("launches a command through xremap".into())
            } else if value.get("set_mode").is_some() {
                Effect::Consume("switches the xremap mode".into())
            } else if value.get("escape_next_key").is_some() {
                Effect::Consume("makes xremap pass the next key through".into())
            } else {
                Effect::Conditional(format!("xremap action {} is not evaluated", value.render()))
            }
        }
    }
}

/// A parsed `MOD-...-KEY` combination. Side-agnostic modifiers are bits in
/// `mods`; modifiers named by a specific key set the class bit in
/// `side_specific`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Combo {
    mods: u32,
    side_specific: u32,
    code: u16,
}

fn parse_combo(text: &str) -> Option<Combo> {
    let mut parts: Vec<&str> = text.split('-').collect();
    let key_name = parts.pop()?;
    let code = parse_key(key_name)?;
    let mut mods = 0;
    let mut side_specific = 0;
    for part in parts {
        match part.to_ascii_uppercase().as_str() {
            "S" | "SHIFT" => mods |= SHIFT,
            "C" | "CTRL" | "CONTROL" => mods |= CTRL,
            "A" | "M" | "ALT" => mods |= ALT,
            "SUPER" | "W" | "WIN" | "WINDOWS" => mods |= SUPER,
            _ => {
                let modifier = parse_key(part)?;
                side_specific |= classes(keys::modifier_bit(modifier)?);
            }
        }
    }
    Some(Combo {
        mods,
        side_specific,
        code,
    })
}

/// Collapse AltGr into Alt: xremap's Alt matches either Alt key.
fn classes(mods: u32) -> u32 {
    let mut classes = mods & (SHIFT | CTRL | SUPER | ALT);
    if mods & ALTGR != 0 {
        classes |= ALT;
    }
    classes
}

fn class_bits(classes: u32) -> u32 {
    if classes & ALT != 0 {
        classes | ALTGR
    } else {
        classes
    }
}

/// xremap key names: evdev names with optional `KEY_`, plus its aliases.
fn parse_key(name: &str) -> Option<u16> {
    let upper = name.trim().to_ascii_uppercase();
    if let Some(code) = keys::evdev_code(&upper) {
        return Some(code);
    }
    if let Some(code) = upper.strip_prefix("CODE_") {
        return code.parse().ok();
    }
    Some(match upper.as_str() {
        "SHIFT_R" | "S_R" => 54,
        "SHIFT_L" | "S_L" => 42,
        "CONTROL_R" | "CTRL_R" | "C_R" => 97,
        "CONTROL_L" | "CTRL_L" | "C_L" => 29,
        "ALT_R" | "A_R" | "M_R" => 100,
        "ALT_L" | "A_L" | "M_L" => 56,
        "SUPER_R" | "WINDOWS_R" | "WIN_R" | "W_R" => 126,
        "SUPER_L" | "WINDOWS_L" | "WIN_L" | "W_L" => 125,
        _ => return None,
    })
}

fn entry(item: &Yaml, path: &str, section: &str) -> Result<Entry, String> {
    let Yaml::Map(fields) = item else {
        return Err(format!("{path}: a {section} entry is not a mapping"));
    };
    let mut entry = Entry {
        conditional: false,
        exact_match: false,
        remap: Vec::new(),
    };
    for (name, value, _) in fields {
        match name.as_str() {
            "remap" => {
                let Yaml::Map(pairs) = value else {
                    return Err(format!("{path}: {section} remap is not a mapping"));
                };
                entry.remap = pairs
                    .iter()
                    .map(|(from, to, line)| {
                        (
                            from.clone(),
                            to.clone(),
                            Rule::new(path, *line, &format!("{section}: {from}: {}", to.render())),
                        )
                    })
                    .collect();
            }
            "exact_match" => {
                entry.exact_match = matches!(value, Yaml::Scalar(text) if text == "true");
            }
            condition if CONDITIONS.contains(&condition) => entry.conditional = true,
            _ => {}
        }
    }
    Ok(entry)
}

/// Maximum mapping/sequence nesting; bounds recursion in evaluation and `Drop`.
const MAX_NESTING: usize = 64;

/// A collection whose end event has not arrived yet.
enum Open {
    Seq(Vec<Yaml>),
    /// Entries so far, plus the key (and its line) awaiting a value.
    Map(Vec<(String, Yaml, u32)>, Option<(String, u32)>),
}

/// Parse one YAML document into the [`Yaml`] tree, recording each mapping
/// key's line.
fn parse(content: &str) -> Result<Yaml, String> {
    let mut parser = Parser::new_from_str(content);
    let mut stack: Vec<Open> = Vec::new();
    let mut document = None;
    let mut documents = 0;
    loop {
        let (event, marker) = parser.next_token().map_err(|error| error.to_string())?;
        let line = u32::try_from(marker.line()).unwrap_or(u32::MAX);
        let awaiting_key = matches!(stack.last(), Some(Open::Map(_, None)));
        let value = match event {
            Event::StreamEnd => break,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return Err(format!("line {line}: several YAML documents"));
                }
                continue;
            }
            Event::Nothing | Event::StreamStart | Event::DocumentEnd => continue,
            Event::Alias(_) => {
                return Err(format!("line {line}: YAML aliases are not evaluated"));
            }
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                if awaiting_key {
                    return Err(format!(
                        "line {line}: complex mapping keys are not evaluated"
                    ));
                }
                if stack.len() >= MAX_NESTING {
                    return Err(format!("line {line}: nesting deeper than {MAX_NESTING}"));
                }
                stack.push(match event {
                    Event::SequenceStart(..) => Open::Seq(Vec::new()),
                    _ => Open::Map(Vec::new(), None),
                });
                continue;
            }
            Event::SequenceEnd | Event::MappingEnd => match stack.pop() {
                Some(Open::Seq(items)) => Yaml::Seq(items),
                Some(Open::Map(entries, _)) => Yaml::Map(entries),
                None => return Err(format!("line {line}: unbalanced YAML collection")),
            },
            Event::Scalar(text, style, ..) => {
                if let Some(Open::Map(_, key @ None)) = stack.last_mut() {
                    *key = Some((text, line));
                    continue;
                }
                let null = style == TScalarStyle::Plain
                    && matches!(text.as_str(), "" | "~" | "null" | "Null" | "NULL");
                if null { Yaml::Null } else { Yaml::Scalar(text) }
            }
        };
        match stack.last_mut() {
            None => document = Some(value),
            Some(Open::Seq(items)) => items.push(value),
            Some(Open::Map(entries, key)) => {
                if let Some((key, key_line)) = key.take() {
                    entries.push((key, value, key_line));
                }
            }
        }
    }
    Ok(document.unwrap_or(Yaml::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(content: &str) -> Config {
        let mut config = Config::default();
        config
            .extend(Path::new("/home/u/.config/xremap/config.yml"), content)
            .unwrap();
        config
    }

    fn effect(config: &Config, press: Press) -> Effect {
        let findings = config.evaluate(press);
        assert_eq!(findings.len(), 1, "{findings:?}");
        findings.into_iter().next().unwrap().effect
    }

    #[test]
    fn keymap_combo_redirects_and_carries_extra_modifiers() {
        let config = config(
            "# emacs-ish\nkeymap:\n  - name: Emacs\n    remap:\n      C-a: C-b\n      C-e: [end]\n      C-x:\n        remap:\n          C-s: C-s\n",
        );
        assert_eq!(
            effect(&config, Press::new(CTRL, 30)),
            Effect::Remap(Press::new(CTRL, 48))
        );
        assert_eq!(
            effect(&config, Press::new(CTRL | SHIFT, 30)),
            Effect::Remap(Press::new(CTRL | SHIFT, 48))
        );
        assert_eq!(
            effect(&config, Press::new(CTRL, 18)),
            Effect::Remap(Press::new(0, 107))
        );
        assert!(matches!(
            effect(&config, Press::new(CTRL, 45)),
            Effect::Consume(_)
        ));
        assert_eq!(effect(&config, Press::new(0, 30)), Effect::Untouched);
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let path = Path::new("/home/u/.config/xremap/config.yml");
        let flow = format!("keymap: {}{}\n", "[".repeat(100_000), "]".repeat(100_000));
        let dashes = format!("{}x\n", "- ".repeat(100_000));
        let mut indented = String::new();
        for depth in 0..2_000 {
            indented.push_str(&" ".repeat(depth));
            indented.push_str("k:\n");
        }
        for content in [flow, dashes, indented] {
            let error = Config::default().extend(path, &content).unwrap_err();
            // Flow nesting hits the scanner's own limit first.
            assert!(
                error.contains("nesting") || error.contains("recursion limit"),
                "{error}"
            );
        }
        // Moderate nesting still parses.
        let shallow = format!("x: {}{}\n", "[".repeat(10), "]".repeat(10));
        assert!(Config::default().extend(path, &shallow).is_ok());
    }

    #[test]
    fn rules_keep_their_key_lines() {
        let config = config("# header\n\nkeymap:\n  - remap:\n      # note\n      C-a: C-b\n");
        let finding = config.evaluate(Press::new(CTRL, 30)).remove(0);
        assert_eq!(finding.rule.map(|rule| rule.line), Some(6));
    }

    #[test]
    fn modmap_feeds_keymap_and_conditions_stay_conditional() {
        let config = config(
            "modmap:\n  - name: caps\n    remap:\n      CapsLock: Esc\n  - application:\n      only: firefox\n    remap:\n      F1: F2\nkeymap:\n  - exact_match: true\n    remap:\n      Ctrl-Esc: Ctrl-Tab\n",
        );
        assert_eq!(
            effect(&config, Press::new(0, 58)),
            Effect::Remap(Press::new(0, 1))
        );
        assert_eq!(
            effect(&config, Press::new(CTRL, 58)),
            Effect::Remap(Press::new(CTRL, 15))
        );
        assert_eq!(
            effect(&config, Press::new(CTRL | SHIFT, 58)),
            Effect::Remap(Press::new(CTRL | SHIFT, 1))
        );
        assert!(matches!(
            effect(&config, Press::new(0, 59)),
            Effect::Conditional(_)
        ));
    }

    #[test]
    fn unsupported_yaml_is_rejected() {
        let mut config = Config::default();
        assert!(
            config
                .extend(
                    Path::new("x.yml"),
                    "shared:\n  a: &a\n    C-a: C-b\nkeymap:\n  - remap: *a\n"
                )
                .is_err()
        );
    }

    #[test]
    fn held_and_alone_on_a_modifier_name_both_outputs() {
        let config = config(
            "modmap:\n  - name: tap super\n    remap:\n      Super_L:\n        held: Super_L\n        alone: Esc\n",
        );
        let Effect::Conditional(text) = effect(&config, Press::new(0, 125)) else {
            panic!("a multi-purpose key is conditional");
        };
        assert!(
            text.contains("alone emits Esc, held acts as Super_L"),
            "{text}"
        );
    }
}
