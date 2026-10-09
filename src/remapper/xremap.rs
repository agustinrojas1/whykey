//! Static evaluation of the simple xremap YAML subset: `modmap` and
//! `keymap` lists whose `remap` maps use scalar keys. YAML features outside
//! that subset (anchors, block scalars, tags) make the whole file
//! unevaluated rather than half-read.

use std::path::Path;

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
                    Yaml::Map(_) => Effect::Conditional(format!(
                        "multi-purpose xremap key {}: the result depends on timing",
                        value.render()
                    )),
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

struct Line<'a> {
    indent: usize,
    text: &'a str,
    number: u32,
}

/// Maximum combined block/flow nesting; bounds recursion in parsing and `Drop`.
const MAX_NESTING: usize = 64;

/// Parse the block-style YAML subset xremap examples use.
fn parse(content: &str) -> Result<Yaml, String> {
    let mut lines = Vec::new();
    for (index, raw) in content.lines().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let without_comment = strip_comment(raw);
        let text = without_comment.trim_end();
        if text.trim().is_empty() {
            continue;
        }
        if text.trim_start() == "---" && lines.is_empty() {
            continue;
        }
        let indent = text.len() - text.trim_start().len();
        if text[..indent].contains('\t') {
            return Err(format!("line {number}: tab indentation"));
        }
        lines.push(Line {
            indent,
            text: text.trim_start(),
            number,
        });
    }
    if lines.is_empty() {
        return Ok(Yaml::Null);
    }
    let mut index = 0;
    let value = block(&lines, &mut index, lines[0].indent, 0)?;
    if index != lines.len() {
        return Err(format!(
            "line {}: unexpected indentation",
            lines[index].number
        ));
    }
    Ok(value)
}

fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    for (index, character) in line.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), c) if c == open => quote = None,
            (None, '#') if previous.is_whitespace() => return &line[..index],
            _ => {}
        }
        previous = character;
    }
    line
}

fn block(
    lines: &[Line<'_>],
    index: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Yaml, String> {
    if depth >= MAX_NESTING {
        return Err(format!(
            "line {}: nesting deeper than {MAX_NESTING}",
            lines[*index].number
        ));
    }
    let depth = depth + 1;
    if lines[*index].text == "-" || lines[*index].text.starts_with("- ") {
        sequence(lines, index, indent, depth)
    } else {
        mapping(lines, index, indent, None, depth)
    }
}

fn sequence(
    lines: &[Line<'_>],
    index: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Yaml, String> {
    let mut items = Vec::new();
    while *index < lines.len() && lines[*index].indent == indent {
        let line = &lines[*index];
        let Some(rest) = line.text.strip_prefix('-') else {
            break;
        };
        if !(rest.is_empty() || rest.starts_with(' ')) {
            break;
        }
        let item = rest.trim_start();
        if item.is_empty() {
            *index += 1;
            items.push(nested(lines, index, indent, depth)?);
        } else if split_key(item).is_some() && !item.starts_with(['[', '{', '"', '\'']) {
            // `- key: value` opens a mapping at the item's column.
            let column = indent + (line.text.len() - item.len());
            items.push(mapping(
                lines,
                index,
                column,
                Some((item, line.number)),
                depth,
            )?);
        } else {
            items.push(scalar(item, line.number, depth)?);
            *index += 1;
        }
    }
    Ok(Yaml::Seq(items))
}

fn mapping(
    lines: &[Line<'_>],
    index: &mut usize,
    indent: usize,
    first: Option<(&str, u32)>,
    depth: usize,
) -> Result<Yaml, String> {
    let mut entries = Vec::new();
    let mut pending = first;
    loop {
        let (text, number) = match pending.take() {
            Some(first) => first,
            None => {
                if *index >= lines.len() || lines[*index].indent != indent {
                    break;
                }
                let line = &lines[*index];
                if line.text.starts_with("- ") || line.text == "-" {
                    break;
                }
                (line.text, line.number)
            }
        };
        let (key, value) =
            split_key(text).ok_or_else(|| format!("line {number}: expected `key: value`"))?;
        *index += 1;
        let value = if value.is_empty() {
            nested(lines, index, indent, depth)?
        } else {
            scalar(value, number, depth)?
        };
        entries.push((unquote(key), value, number));
    }
    Ok(Yaml::Map(entries))
}

/// The block under `key:` or `-`: deeper lines, or a sequence at the same
/// indentation under a mapping key.
fn nested(
    lines: &[Line<'_>],
    index: &mut usize,
    indent: usize,
    depth: usize,
) -> Result<Yaml, String> {
    match lines.get(*index) {
        Some(next) if next.indent > indent => block(lines, index, next.indent, depth),
        Some(next)
            if next.indent == indent && (next.text.starts_with("- ") || next.text == "-") =>
        {
            block(lines, index, indent, depth)
        }
        _ => Ok(Yaml::Null),
    }
}

/// Split `key: value` at the first `: ` (or a trailing `:`) outside quotes.
fn split_key(text: &str) -> Option<(&str, &str)> {
    let mut quote = None;
    let bytes = text.as_bytes();
    for (index, character) in text.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') if index == 0 => quote = Some(character),
            (Some(open), c) if c == open => quote = None,
            (None, ':') if bytes.get(index + 1).is_none_or(|next| *next == b' ') => {
                let key = text[..index].trim();
                return (!key.is_empty()).then(|| (key, text[index + 1..].trim()));
            }
            (None, '{' | '[') if index == 0 => return None,
            _ => {}
        }
    }
    None
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    text.to_owned()
}

fn scalar(text: &str, number: u32, depth: usize) -> Result<Yaml, String> {
    let text = text.trim();
    if text.starts_with(['&', '*', '!', '|', '>', '%', '@', '`']) {
        return Err(format!(
            "line {number}: YAML feature `{text}` is not evaluated"
        ));
    }
    if text.starts_with(['[', '{']) {
        let mut chars = text.chars().peekable();
        let value = flow(&mut chars, number, depth)?;
        if chars.any(|character| !character.is_whitespace()) {
            return Err(format!("line {number}: trailing text after a flow value"));
        }
        return Ok(value);
    }
    Ok(match text {
        "null" | "~" => Yaml::Null,
        _ => Yaml::Scalar(unquote(text)),
    })
}

fn flow(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    number: u32,
    depth: usize,
) -> Result<Yaml, String> {
    if depth >= MAX_NESTING {
        return Err(format!("line {number}: nesting deeper than {MAX_NESTING}"));
    }
    let depth = depth + 1;
    skip_spaces(chars);
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            let mut items = Vec::new();
            loop {
                skip_spaces(chars);
                if chars.peek() == Some(&']') {
                    chars.next();
                    return Ok(Yaml::Seq(items));
                }
                items.push(flow(chars, number, depth)?);
                skip_spaces(chars);
                match chars.next() {
                    Some(',') => {}
                    Some(']') => return Ok(Yaml::Seq(items)),
                    _ => return Err(format!("line {number}: malformed flow sequence")),
                }
            }
        }
        Some('{') => {
            chars.next();
            let mut entries = Vec::new();
            loop {
                skip_spaces(chars);
                if chars.peek() == Some(&'}') {
                    chars.next();
                    return Ok(Yaml::Map(entries));
                }
                let key = flow_scalar(chars, &[':']);
                if chars.next() != Some(':') {
                    return Err(format!("line {number}: malformed flow mapping"));
                }
                let value = flow(chars, number, depth)?;
                entries.push((key, value, number));
                skip_spaces(chars);
                match chars.next() {
                    Some(',') => {}
                    Some('}') => return Ok(Yaml::Map(entries)),
                    _ => return Err(format!("line {number}: malformed flow mapping")),
                }
            }
        }
        _ => {
            let text = flow_scalar(chars, &[',', ']', '}']);
            scalar(&text, number, depth)
        }
    }
}

fn flow_scalar(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, stops: &[char]) -> String {
    skip_spaces(chars);
    let mut text = String::new();
    let quote = chars.peek().copied().filter(|c| matches!(c, '"' | '\''));
    if let Some(quote) = quote {
        chars.next();
        for character in chars.by_ref() {
            if character == quote {
                break;
            }
            text.push(character);
        }
        skip_spaces(chars);
        return text;
    }
    while let Some(character) = chars.peek() {
        if stops.contains(character) {
            break;
        }
        text.push(*character);
        chars.next();
    }
    text.trim().to_owned()
}

fn skip_spaces(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while chars
        .peek()
        .is_some_and(|character| character.is_whitespace())
    {
        chars.next();
    }
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
        let dashes = "-\n".repeat(100_000);
        let mut indented = String::new();
        for depth in 0..2_000 {
            indented.push_str(&" ".repeat(depth));
            indented.push_str("k:\n");
        }
        for content in [flow, dashes, indented] {
            let error = Config::default().extend(path, &content).unwrap_err();
            assert!(error.contains("nesting"), "{error}");
        }
        // Moderate nesting still parses.
        let shallow = format!("x: {}{}\n", "[".repeat(10), "]".repeat(10));
        assert!(Config::default().extend(path, &shallow).is_ok());
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
}
