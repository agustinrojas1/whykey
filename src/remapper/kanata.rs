//! Static evaluation of a kanata configuration: the `defsrc` key list, the
//! first (default) layer, aliases that name plain keys, and v1
//! `defoverrides`. Tap-hold, layers, and other stateful actions stay
//! conditional.

use std::path::Path;

use super::keys::{self, ALT, ALTGR, CTRL, Press, SHIFT, SUPER};
use super::{Effect, Finding, Rule};

const MAX_ALIAS_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Sexp {
    Atom(String, u32),
    List(Vec<Sexp>, u32),
}

impl Sexp {
    fn line(&self) -> u32 {
        match self {
            Self::Atom(_, line) | Self::List(_, line) => *line,
        }
    }

    fn atom(&self) -> Option<&str> {
        match self {
            Self::Atom(text, _) => Some(text),
            Self::List(..) => None,
        }
    }

    fn head(&self) -> Option<&str> {
        match self {
            Self::List(items, _) => items.first().and_then(Sexp::atom),
            Self::Atom(..) => None,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Atom(text, _) => text.clone(),
            Self::List(items, _) => format!(
                "({})",
                items.iter().map(Sexp::render).collect::<Vec<_>>().join(" ")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Override {
    input: Vec<u16>,
    output: Vec<u16>,
    /// `defoverridesv2` adds modifier and layer exclusions.
    excluding: bool,
    rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    path: String,
    layer: String,
    /// `defsrc` keys paired with the default layer's action for each.
    base: Vec<(u16, Sexp)>,
    /// `deflayermap` fallback (`_`, `__`, `___`) for unlisted keys.
    layer_fallback: Option<Sexp>,
    aliases: Vec<(String, Sexp)>,
    overrides: Vec<Override>,
    process_unmapped: bool,
    /// `defcfg` restricts the managed devices.
    device_scoped: bool,
}

/// Parse one kanata configuration. `Err` is a reason the file cannot be
/// evaluated; unreadable includes are appended to `unreadable`.
pub(crate) fn load(
    path: &Path,
    content: &str,
    unreadable: &mut Vec<String>,
) -> Result<Config, String> {
    let display = path.display().to_string();
    let mut forms = parse(content).map_err(|error| format!("{display}: {error}"))?;
    let mut included = Vec::new();
    for form in &forms {
        if form.head() == Some("include")
            && let Sexp::List(items, _) = form
            && let Some(name) = items.get(1).and_then(Sexp::atom)
        {
            let target = path
                .parent()
                .map_or_else(|| Path::new(name).to_path_buf(), |dir| dir.join(name));
            match super::read_config(&target) {
                Ok(content) => included.extend(
                    parse(&content).map_err(|error| format!("{}: {error}", target.display()))?,
                ),
                Err(reason) => unreadable.push(reason),
            }
        }
    }
    forms.extend(included);

    let mut source: Option<Vec<Option<u16>>> = None;
    let mut layer: Option<(String, Vec<Sexp>, bool)> = None;
    let mut config = Config {
        path: display.clone(),
        layer: String::new(),
        base: Vec::new(),
        layer_fallback: None,
        aliases: Vec::new(),
        overrides: Vec::new(),
        process_unmapped: false,
        device_scoped: false,
    };
    for form in forms {
        let Sexp::List(items, _) = form else {
            continue;
        };
        let Some(head) = items.first().and_then(Sexp::atom) else {
            continue;
        };
        match head {
            "defsrc" if source.is_none() => {
                source = Some(
                    items[1..]
                        .iter()
                        .map(|item| item.atom().and_then(key))
                        .collect(),
                );
            }
            "deflayer" | "deflayermap" if layer.is_none() => {
                let name = items.get(1).map(|name| match name {
                    Sexp::List(parts, _) => parts.first().map(Sexp::render).unwrap_or_default(),
                    atom => atom.render(),
                });
                layer = Some((
                    name.unwrap_or_default(),
                    items[2.min(items.len())..].to_vec(),
                    head == "deflayermap",
                ));
            }
            "defalias" => {
                for pair in items[1..].chunks(2) {
                    if let [Sexp::Atom(name, _), action] = pair {
                        config.aliases.push((name.clone(), action.clone()));
                    }
                }
            }
            "defcfg" => {
                for pair in items[1..].chunks(2) {
                    let [Sexp::Atom(name, _), value] = pair else {
                        continue;
                    };
                    match name.as_str() {
                        "process-unmapped-keys" => {
                            config.process_unmapped = !matches!(value.atom(), Some("no" | "false"));
                        }
                        "linux-dev" | "linux-dev-names-include" | "linux-dev-names-exclude" => {
                            config.device_scoped = true;
                        }
                        _ => {}
                    }
                }
            }
            "defoverrides" | "defoverridesv2" => {
                let width = if head == "defoverrides" { 2 } else { 4 };
                for entry in items[1..].chunks(width) {
                    let (Some(input), Some(output)) = (entry.first(), entry.get(1)) else {
                        continue;
                    };
                    let excluding = entry[2..]
                        .iter()
                        .any(|list| matches!(list, Sexp::List(items, _) if !items.is_empty()));
                    let codes = |list: &Sexp| match list {
                        Sexp::List(items, _) => items
                            .iter()
                            .map(|item| item.atom().and_then(key))
                            .collect::<Option<Vec<_>>>(),
                        Sexp::Atom(..) => None,
                    };
                    if let (Some(input_codes), Some(output_codes)) = (codes(input), codes(output)) {
                        config.overrides.push(Override {
                            input: input_codes,
                            output: output_codes,
                            excluding,
                            rule: Rule::new(
                                &display,
                                input.line(),
                                &format!("{head} {} {}", input.render(), output.render()),
                            ),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    let source = source.ok_or_else(|| format!("{display}: no defsrc"))?;
    let (name, actions, is_map) = layer.ok_or_else(|| format!("{display}: no deflayer"))?;
    config.layer = name;
    if is_map {
        for pair in actions.chunks(2) {
            let [input, action] = pair else {
                continue;
            };
            match input.atom() {
                Some("_" | "__" | "___") => config.layer_fallback = Some(action.clone()),
                Some(name) => {
                    if let Some(code) = key(name) {
                        config.base.push((code, action.clone()));
                    }
                }
                None => {}
            }
        }
        // Keys in defsrc but not in the map are transparent.
        for code in source.iter().flatten() {
            if !config.base.iter().any(|(mapped, _)| mapped == code) {
                config.base.push((*code, Sexp::Atom("_".into(), 0)));
            }
        }
    } else {
        config.base = source
            .iter()
            .zip(actions)
            .filter_map(|(code, action)| code.map(|code| (code, action)))
            .collect();
    }
    Ok(config)
}

impl Config {
    /// kanata's effect on `press` in the default layer.
    pub(crate) fn evaluate(&self, press: Press) -> Vec<Finding> {
        let mut findings = vec![self.evaluate_key(press)];
        if self.device_scoped {
            findings.push(Finding::untouched_because(
                "kanata's defcfg restricts the managed keyboards (linux-dev)",
            ));
        }
        findings
    }

    fn evaluate_key(&self, press: Press) -> Finding {
        if let Some(rule) = self.held_modifier_remapped(press.mods) {
            return Finding {
                effect: Effect::Conditional(
                    "kanata remaps the held modifier keys; the modifiers that reach kanata's output are not evaluated"
                        .into(),
                ),
                rule: Some(rule),
            };
        }
        let mapped = self.base.iter().find(|(code, _)| *code == press.code);
        let (action, rule) = match mapped {
            Some((_, action)) => (action, self.rule_for(action, press.code)),
            None if self.process_unmapped => match &self.layer_fallback {
                Some(action) => (action, self.rule_for(action, press.code)),
                None => return self.apply_overrides(press, press, None),
            },
            None => return Finding::untouched(),
        };
        match self.resolve(action, press, 0) {
            Resolved::Emit(output) => self.apply_overrides(press, output, Some(rule)),
            Resolved::Effect(effect) => Finding {
                effect,
                rule: Some(rule),
            },
        }
    }

    fn rule_for(&self, action: &Sexp, code: u16) -> Rule {
        Rule::new(
            &self.path,
            action.line(),
            &format!(
                "deflayer {}: {} -> {}",
                self.layer,
                keys::evdev_name(code),
                action.render()
            ),
        )
    }

    fn held_modifier_remapped(&self, mods: u32) -> Option<Rule> {
        for bit in keys::modifier_bits(mods) {
            let remapped: Vec<Rule> = keys::modifier_codes(bit)
                .iter()
                .filter_map(|code| {
                    let (_, action) = self.base.iter().find(|(mapped, _)| mapped == code)?;
                    let identity = Press::new(0, *code);
                    match self.resolve(action, identity, 0) {
                        Resolved::Emit(output) if output == identity => None,
                        _ => Some(self.rule_for(action, *code)),
                    }
                })
                .collect();
            if remapped.len() == keys::modifier_codes(bit).len() {
                return remapped.into_iter().next();
            }
        }
        None
    }

    fn resolve(&self, action: &Sexp, press: Press, depth: usize) -> Resolved {
        match action {
            Sexp::Atom(text, _) => {
                let text = text.as_str();
                if matches!(text, "_" | "‗" | "≝") {
                    return Resolved::Emit(press);
                }
                if matches!(text, "XX" | "✗" | "∅" | "•") {
                    return Resolved::Effect(Effect::Consume("disabled by kanata (XX)".into()));
                }
                if let Some(name) = text.strip_prefix('@') {
                    let Some((_, aliased)) = self.aliases.iter().find(|(alias, _)| alias == name)
                    else {
                        return Resolved::Effect(Effect::Conditional(format!(
                            "kanata alias @{name} is not defined in the evaluated files"
                        )));
                    };
                    if depth >= MAX_ALIAS_DEPTH {
                        return Resolved::Effect(Effect::Conditional(format!(
                            "kanata alias @{name} nests too deeply to evaluate"
                        )));
                    }
                    return self.resolve(aliased, press, depth + 1);
                }
                match combo(text) {
                    Some(emitted) => {
                        Resolved::Emit(Press::new(press.mods | emitted.mods, emitted.code))
                    }
                    None => Resolved::Effect(Effect::Conditional(format!(
                        "kanata action {text} is not evaluated"
                    ))),
                }
            }
            Sexp::List(items, _) => {
                let head = action.head().unwrap_or_default();
                let target = items.get(1).map(Sexp::render).unwrap_or_default();
                let effect = match head {
                    "multi" => {
                        let mut mods = press.mods;
                        let mut code = None;
                        for item in &items[1..] {
                            let Some(emitted) = item.atom().and_then(combo) else {
                                return Resolved::Effect(Effect::Consume(format!(
                                    "kanata presses {}",
                                    action.render()
                                )));
                            };
                            match keys::modifier_bit(emitted.code) {
                                Some(bit) if emitted.mods == 0 => mods |= bit,
                                _ if code.is_none() => {
                                    mods |= emitted.mods;
                                    code = Some(emitted.code);
                                }
                                _ => {
                                    return Resolved::Effect(Effect::Consume(format!(
                                        "kanata presses {}",
                                        action.render()
                                    )));
                                }
                            }
                        }
                        return match code {
                            Some(code) => Resolved::Emit(Press::new(mods, code)),
                            None => Resolved::Effect(Effect::Consume(format!(
                                "kanata holds modifiers {}",
                                action.render()
                            ))),
                        };
                    }
                    "layer-switch" => {
                        Effect::Consume(format!("switches the kanata base layer to {target}"))
                    }
                    "layer-while-held" | "layer-toggle" => {
                        Effect::Consume(format!("activates kanata layer {target} while held"))
                    }
                    "macro"
                    | "macro-release-cancel"
                    | "macro-repeat"
                    | "macro-repeat-release-cancel"
                    | "unicode" => Effect::Consume(format!("kanata emits {}", action.render())),
                    "cmd" | "cmd-output-keys" | "cmd-log" => {
                        Effect::Consume("runs a kanata command".into())
                    }
                    head if head.starts_with("tap-hold")
                        || head.starts_with("tap-dance")
                        || head.starts_with("one-shot")
                        || head.starts_with("caps-word")
                        || matches!(
                            head,
                            "fork" | "switch" | "chord" | "release-key" | "on-press" | "on-release"
                        ) =>
                    {
                        Effect::Conditional(format!(
                            "kanata action {} depends on timing or other keys",
                            action.render()
                        ))
                    }
                    _ => Effect::Conditional(format!(
                        "kanata action {} is not evaluated",
                        action.render()
                    )),
                };
                Resolved::Effect(effect)
            }
        }
    }

    fn apply_overrides(&self, press: Press, output: Press, rule: Option<Rule>) -> Finding {
        let matched = self.overrides.iter().find(|entry| {
            entry
                .input
                .iter()
                .any(|code| *code == output.code && keys::modifier_bit(*code).is_none())
        });
        let Some(entry) = matched else {
            return Finding {
                effect: if output == press {
                    Effect::Untouched
                } else {
                    Effect::Remap(output)
                },
                rule: rule.filter(|_| output != press),
            };
        };
        let input_mods = entry
            .input
            .iter()
            .any(|code| keys::modifier_bit(*code).is_some());
        if entry.excluding || input_mods || output.mods != 0 {
            return Finding {
                effect: Effect::Conditional(
                    "a kanata override with modifier conditions may replace this key".into(),
                ),
                rule: Some(entry.rule.clone()),
            };
        }
        let mut mods = 0;
        let mut code = None;
        for emitted in &entry.output {
            match keys::modifier_bit(*emitted) {
                Some(bit) => mods |= bit,
                None => code = Some(*emitted),
            }
        }
        let effect = match code {
            Some(code) => Effect::Remap(Press::new(mods, code)),
            None => Effect::Conditional("a kanata override emits only modifiers".into()),
        };
        Finding {
            effect,
            rule: Some(entry.rule.clone()),
        }
    }
}

enum Resolved {
    Emit(Press),
    Effect(Effect),
}

/// Modifier prefixes of a kanata output key, right-side variants first.
const PREFIXES: &[(&str, u32)] = &[
    ("RC-", CTRL),
    ("RS-", SHIFT),
    ("RA-", ALTGR),
    ("AG-", ALTGR),
    ("RM-", SUPER),
    ("C-", CTRL),
    ("S-", SHIFT),
    ("A-", ALT),
    ("M-", SUPER),
];

/// A kanata key with optional `C-`/`S-`/`A-`/`M-` (and right-side) prefixes.
fn combo(text: &str) -> Option<Press> {
    let mut mods = 0;
    let mut rest = text;
    while let Some((bit, tail)) = PREFIXES.iter().find_map(|(prefix, bit)| {
        rest.strip_prefix(prefix)
            .filter(|tail| !tail.is_empty())
            .map(|tail| (*bit, tail))
    }) {
        mods |= bit;
        rest = tail;
    }
    key(rest).map(|code| Press::new(mods, code))
}

/// kanata's Linux key names, falling back to evdev names.
fn key(name: &str) -> Option<u16> {
    let code = match name {
        "grv" | "grave" | "`" => 41,
        "min" | "-" => 12,
        "eql" | "=" => 13,
        "bspc" | "bks" => 14,
        "lbrc" | "[" | "{" => 26,
        "rbrc" | "]" | "}" => 27,
        "caps" => 58,
        "scln" | ";" => 39,
        "apo" | "apos" | "quot" | "'" => 40,
        "ret" | "return" | "ent" | "enter" => 28,
        "lshift" | "lshft" | "lsft" | "shft" | "sft" => 42,
        "rshift" | "rshft" | "rsft" => 54,
        "lctrl" | "lctl" | "ctl" => 29,
        "rctrl" | "rctl" => 97,
        "lalt" | "alt" => 56,
        "ralt" | "altgr" => 100,
        "lmeta" | "lmet" | "met" => 125,
        "rmeta" | "rmet" => 126,
        "spc" => 57,
        "comm" | "," => 51,
        "." => 52,
        "/" => 53,
        "bksl" | "\\" | "yen" => 43,
        "del" => 111,
        "ins" => 110,
        "pgup" => 104,
        "pgdn" => 109,
        "lft" | "left" => 105,
        "rght" | "right" => 106,
        "nlck" | "nlk" => 69,
        "slck" | "scrlck" => 70,
        "brk" | "pause" => 119,
        "prtsc" | "prnt" | "sys" | "ssrq" => 99,
        "comp" | "cmps" | "cmp" | "menu" | "apps" => 127,
        "kprt" => 96,
        "kp/" => 98,
        "kp+" => 78,
        "kp*" => 55,
        "kp-" => 74,
        "kp." => 83,
        "volu" => 115,
        "vold" | "voldwn" => 114,
        "102d" | "lsgt" | "nubs" => 86,
        "pp" => 164,
        "next" => 163,
        "prev" => 165,
        "brup" | "bru" => 225,
        "brdown" | "brdwn" | "brdn" => 224,
        _ => return keys::evdev_code(name),
    };
    Some(code)
}

/// Maximum list nesting; bounds recursion in evaluation and `Drop`.
const MAX_NESTING: usize = 64;

/// Parse kanata s-expressions: `;;` line comments, `#| |#` block comments,
/// and double-quoted strings.
fn parse(content: &str) -> Result<Vec<Sexp>, String> {
    let mut stack: Vec<(Vec<Sexp>, u32)> = vec![(Vec::new(), 0)];
    let mut line = 1u32;
    let mut chars = content.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\n' => line += 1,
            c if c.is_whitespace() => {}
            ';' if chars.peek() == Some(&';') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '#' if chars.peek() == Some(&'|') => {
                chars.next();
                let mut previous = ' ';
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                    }
                    if previous == '|' && next == '#' {
                        closed = true;
                        break;
                    }
                    previous = next;
                }
                if !closed {
                    return Err("unterminated block comment".into());
                }
            }
            '(' => {
                if stack.len() > MAX_NESTING {
                    return Err(format!("line {line}: nesting deeper than {MAX_NESTING}"));
                }
                stack.push((Vec::new(), line));
            }
            ')' => {
                let (items, start) = stack
                    .pop()
                    .filter(|_| !stack.is_empty())
                    .ok_or_else(|| format!("line {line}: unbalanced ')'"))?;
                stack
                    .last_mut()
                    .ok_or_else(|| format!("line {line}: unbalanced ')'"))?
                    .0
                    .push(Sexp::List(items, start));
            }
            '"' => {
                let mut text = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '"' {
                        closed = true;
                        break;
                    }
                    if next == '\n' {
                        line += 1;
                    }
                    text.push(next);
                }
                if !closed {
                    return Err(format!("line {line}: unterminated string"));
                }
                push_atom(&mut stack, text, line)?;
            }
            first => {
                let mut text = String::from(first);
                while let Some(next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '(' | ')') {
                        break;
                    }
                    text.push(*next);
                    chars.next();
                }
                push_atom(&mut stack, text, line)?;
            }
        }
    }
    if stack.len() != 1 {
        return Err("unbalanced '('".into());
    }
    Ok(stack.pop().map(|(items, _)| items).unwrap_or_default())
}

fn push_atom(stack: &mut [(Vec<Sexp>, u32)], text: String, line: u32) -> Result<(), String> {
    stack
        .last_mut()
        .ok_or_else(|| format!("line {line}: unexpected atom"))?
        .0
        .push(Sexp::Atom(text, line));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(content: &str) -> Config {
        let mut unreadable = Vec::new();
        load(
            Path::new("/home/u/.config/kanata/kanata.kbd"),
            content,
            &mut unreadable,
        )
        .unwrap()
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let content = format!("{}{}", "(".repeat(200_000), ")".repeat(200_000));
        let mut unreadable = Vec::new();
        let Err(error) = load(Path::new("/tmp/deep.kbd"), &content, &mut unreadable) else {
            panic!("deep nesting must be a parse error");
        };
        assert!(error.contains("nesting"), "{error}");
        let shallow = format!("{}{}", "(".repeat(32), ")".repeat(32));
        assert!(parse(&shallow).is_ok());
    }

    fn effect(config: &Config, press: Press) -> Effect {
        let findings = config.evaluate(press);
        assert_eq!(findings.len(), 1, "{findings:?}");
        findings.into_iter().next().unwrap().effect
    }

    #[test]
    fn default_layer_maps_defsrc_positions() {
        let config = config(
            ";; caps to escape\n(defsrc caps a s lctl)\n(defalias nav (layer-while-held nav) th (tap-hold 200 200 s lctl))\n(deflayer base esc _ @th lctl)\n(deflayer nav left down up right)\n",
        );
        assert_eq!(
            effect(&config, Press::new(0, 58)),
            Effect::Remap(Press::new(0, 1))
        );
        assert_eq!(
            effect(&config, Press::new(CTRL, 58)),
            Effect::Remap(Press::new(CTRL, 1))
        );
        assert_eq!(effect(&config, Press::new(0, 30)), Effect::Untouched);
        assert!(matches!(
            effect(&config, Press::new(0, 31)),
            Effect::Conditional(_)
        ));
        // Not in defsrc and process-unmapped-keys is off.
        assert_eq!(effect(&config, Press::new(0, 59)), Effect::Untouched);
    }

    #[test]
    fn layer_actions_consume_and_overrides_replace() {
        let config = config(
            "(defcfg process-unmapped-keys yes)\n(defsrc caps 1)\n(defalias nav (layer-while-held nav))\n(deflayer base @nav 1)\n(deflayer nav XX XX)\n(defoverrides (1) (lsft 1))\n",
        );
        assert!(matches!(
            effect(&config, Press::new(0, 58)),
            Effect::Consume(_)
        ));
        assert_eq!(
            effect(&config, Press::new(0, 2)),
            Effect::Remap(Press::new(SHIFT, 2))
        );
    }

    #[test]
    fn missing_defsrc_is_not_evaluated() {
        let mut unreadable = Vec::new();
        assert!(
            load(
                Path::new("x.kbd"),
                "(deflayer base @caps a)",
                &mut unreadable
            )
            .is_err()
        );
    }
}
