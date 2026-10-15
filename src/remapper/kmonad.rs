//! Static evaluation of a kmonad configuration: the `defcfg` input device
//! and `fallthrough`, `defsrc` (named sources too), the first `deflayer`
//! (the layer kmonad starts in), aliases, `_`, `XX`, shifted names, and
//! `C-`/`S-`/`A-`/`M-` prefixes. Layer, macro, and command buttons consume
//! the key; tap-hold and other timing-dependent buttons stay conditional.

use std::path::Path;

use super::keys::{self, ALT, ALTGR, CTRL, Press, SHIFT, SUPER};
use super::sexp::{self, Dialect, Sexp};
use super::{DeviceScope, Effect, Finding, Rule};

const MAX_ALIAS_DEPTH: usize = 8;
/// kmonad options that take a value; `-x VALUE`, `-xVALUE`, `--x VALUE`,
/// and `--x=VALUE` all work.
const VALUE_OPTIONS: &[&str] = &[
    "--log-level",
    "--cmp-seq",
    "--cmp-seq-delay",
    "--key-seq-delay",
    "--implicit-around",
    "--ia",
    "--output",
    "--input",
    "--start-delay",
];
const SHORT_VALUE_OPTIONS: &[char] = &['l', 's', 'o', 'i', 'w'];

/// `defcfg` settings given on the kmonad command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cli {
    fallthrough: bool,
    input: Option<String>,
    implicit_around: Option<String>,
}

/// The configuration file and the command-line settings of one kmonad
/// process.
pub(crate) fn command_line(args: &[String]) -> (Option<String>, Cli) {
    let mut path = None;
    let mut cli = Cli::default();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            path = path.or_else(|| iter.next().cloned());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (long, None),
            };
            if name == "fallthrough" {
                cli.fallthrough = true;
            }
            if VALUE_OPTIONS.contains(&arg.split('=').next().unwrap_or_default()) {
                let value = inline.or_else(|| iter.next().cloned());
                cli.set(name, value);
            }
            continue;
        }
        if let Some(short) = arg.strip_prefix('-').filter(|short| !short.is_empty()) {
            for (index, flag) in short.char_indices() {
                if flag == 'f' {
                    cli.fallthrough = true;
                }
                if SHORT_VALUE_OPTIONS.contains(&flag) {
                    let rest = &short[index + flag.len_utf8()..];
                    let value = if rest.is_empty() {
                        iter.next().cloned()
                    } else {
                        Some(rest.to_owned())
                    };
                    let name = match flag {
                        'i' => "input",
                        _ => "",
                    };
                    cli.set(name, value);
                    break;
                }
            }
            continue;
        }
        path = path.or_else(|| Some(arg.clone()));
    }
    (path, cli)
}

impl Cli {
    fn set(&mut self, name: &str, value: Option<String>) {
        match name {
            "input" => self.input = value,
            "implicit-around" | "ia" => self.implicit_around = value,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    path: String,
    layer: String,
    /// Source keys paired with the first layer's button for each.
    base: Vec<(u16, Sexp)>,
    aliases: Vec<(String, Sexp)>,
    fallthrough: bool,
    /// `implicit-around disabled` rejects shifted names and prefixes.
    implicit_around: bool,
    device: DeviceScope,
}

/// The first `deflayer`, with its `:source` and `:implicit-around`.
struct Layer {
    name: String,
    buttons: Vec<Sexp>,
    source: Option<String>,
    implicit_around: Option<String>,
}

impl Layer {
    fn parse(items: &[Sexp]) -> Self {
        let mut layer = Self {
            name: items.get(1).map(Sexp::render).unwrap_or_default(),
            buttons: Vec::new(),
            source: None,
            implicit_around: None,
        };
        let mut rest = items.get(2..).unwrap_or_default().iter();
        while let Some(item) = rest.next() {
            match item.atom() {
                Some(":source") => layer.source = rest.next().map(Sexp::render),
                Some(":implicit-around") => layer.implicit_around = rest.next().map(Sexp::render),
                _ => layer.buttons.push(item.clone()),
            }
        }
        layer
    }
}

/// Parse one kmonad configuration with the command-line settings of the
/// process running it. `Err` is a reason kmonad would reject the file or
/// whykey cannot evaluate it.
pub(crate) fn load(path: &Path, content: &str, cli: &Cli) -> Result<Config, String> {
    let display = path.display().to_string();
    let forms =
        sexp::parse(content, Dialect::Kmonad).map_err(|error| format!("{display}: {error}"))?;
    let mut sources: Vec<(Option<String>, Vec<Sexp>)> = Vec::new();
    let mut layer: Option<Layer> = None;
    let mut aliases = Vec::new();
    let mut input = None;
    let mut fallthrough = false;
    let mut implicit_around = None;
    for form in forms {
        let Sexp::List(items, _) = form else {
            continue;
        };
        let Some(head) = items.first().and_then(Sexp::atom) else {
            continue;
        };
        match head {
            "defcfg" => {
                for pair in items[1..].chunks(2) {
                    let [Sexp::Atom(name, _), value] = pair else {
                        continue;
                    };
                    match name.as_str() {
                        "input" => input = Some(value.clone()),
                        "fallthrough" => fallthrough = value.atom() == Some("true"),
                        "implicit-around" => implicit_around = value.atom().map(str::to_owned),
                        _ => {}
                    }
                }
            }
            "defsrc" => {
                let (name, keys) = match items.get(1).and_then(Sexp::atom) {
                    Some(":name") => (
                        items.get(2).and_then(Sexp::atom).map(str::to_owned),
                        items.get(3..).unwrap_or_default(),
                    ),
                    _ => (None, &items[1..]),
                };
                sources.push((name, keys.to_vec()));
            }
            "deflayer" if layer.is_none() => layer = Some(Layer::parse(&items)),
            "defalias" => {
                for pair in items[1..].chunks(2) {
                    if let [Sexp::Atom(name, _), button] = pair {
                        aliases.push((name.clone(), button.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    if sources.is_empty() {
        return Err(format!("{display}: no defsrc"));
    }
    let Layer {
        name: layer,
        buttons,
        source,
        implicit_around: layer_around,
    } = layer.ok_or_else(|| format!("{display}: no deflayer"))?;
    let keys = sources
        .iter()
        .find(|(name, _)| *name == source)
        .map(|(_, keys)| keys)
        .ok_or_else(|| match &source {
            Some(name) => format!("{display}: deflayer {layer} names a missing defsrc {name}"),
            None => format!("{display}: deflayer {layer} needs a defsrc without :name"),
        })?;
    if keys.len() != buttons.len() {
        return Err(format!(
            "{display}: deflayer {layer} has {} buttons for {} defsrc keys",
            buttons.len(),
            keys.len()
        ));
    }
    let mut base = Vec::new();
    for (key, button) in keys.iter().zip(buttons) {
        let mut key = key.clone();
        for _ in 0..MAX_ALIAS_DEPTH {
            let Some(name) = key.atom().and_then(|atom| atom.strip_prefix('@')) else {
                break;
            };
            match aliases.iter().find(|(alias, _)| alias == name) {
                Some((_, aliased)) => key = aliased.clone(),
                None => break,
            }
        }
        match key.atom() {
            Some("XX") => {}
            Some(name) => match keycode(name) {
                Some(code) => base.push((code, button)),
                None => return Err(format!("{display}: defsrc key {name} is not a keycode")),
            },
            None => {
                return Err(format!(
                    "{display}: defsrc entry {} is not a keycode",
                    key.render()
                ));
            }
        }
    }

    let device = match cli.input.as_deref() {
        Some(token) => sexp::parse(token, Dialect::Kmonad)
            .ok()
            .map(|items| Sexp::List(items, 0)),
        None => input,
    };
    let device_file = match &device {
        Some(list @ Sexp::List(items, _)) if list.head() == Some("device-file") => {
            items.get(1).and_then(Sexp::atom).map(str::to_owned)
        }
        _ => None,
    };
    let Some(device_file) = device_file else {
        return Err(format!(
            "{display}: kmonad reads no Linux device-file input ({})",
            device
                .as_ref()
                .map_or_else(|| "no input".into(), Sexp::render)
        ));
    };
    let around = layer_around
        .or_else(|| cli.implicit_around.clone())
        .or(implicit_around);
    Ok(Config {
        path: display,
        layer,
        base,
        aliases,
        fallthrough: fallthrough || cli.fallthrough,
        implicit_around: around.as_deref() != Some("disabled"),
        device: DeviceScope::device_file(&device_file),
    })
}

enum Resolved {
    Emit(Press),
    /// Transparent: the event falls to the layer below, which the first
    /// layer does not have.
    Unhandled,
    Effect(Effect),
}

impl Config {
    /// The same configuration read from another device.
    #[cfg(test)]
    pub(crate) fn with_device(mut self, device: DeviceScope) -> Self {
        self.device = device;
        self
    }

    /// kmonad's effect on `press` in the layer it starts in. `device` is the
    /// captured input device, when known.
    pub(crate) fn evaluate(&self, press: Press, device: Option<&str>) -> Vec<Finding> {
        let mut findings = vec![self.evaluate_key(press)];
        if !self.device.includes(device) {
            findings.push(self.device.finding("kmonad"));
        }
        findings
    }

    fn evaluate_key(&self, press: Press) -> Finding {
        for bit in keys::modifier_bits(press.mods) {
            let changed: Vec<Finding> = keys::modifier_codes(bit)
                .iter()
                .map(|code| self.key_finding(Press::new(0, *code)))
                .filter(|finding| finding.effect != Effect::Untouched)
                .collect();
            if changed.len() == keys::modifier_codes(bit).len() {
                return Finding {
                    effect: Effect::Conditional(
                        "kmonad changes the held modifier keys; the modifiers that reach its output are not evaluated"
                            .into(),
                    ),
                    rule: changed.into_iter().find_map(|finding| finding.rule),
                };
            }
        }
        self.key_finding(press)
    }

    fn key_finding(&self, press: Press) -> Finding {
        let mapped = self.base.iter().find(|(code, _)| *code == press.code);
        let (resolved, rule) = match mapped {
            Some((code, button)) => (self.resolve(button, press, 0), self.rule_for(button, *code)),
            None => (
                Resolved::Unhandled,
                Rule::new(
                    &self.path,
                    0,
                    &format!(
                        "kmonad defsrc does not list {}",
                        keys::evdev_name(press.code)
                    ),
                ),
            ),
        };
        let effect = match resolved {
            Resolved::Emit(output) if output == press => return Finding::untouched(),
            Resolved::Emit(output) => Effect::Remap(output),
            Resolved::Unhandled if self.fallthrough => return Finding::untouched(),
            Resolved::Unhandled => Effect::Consume(
                "kmonad drops keys its active layer does not handle (fallthrough is off)".into(),
            ),
            Resolved::Effect(effect) => effect,
        };
        Finding {
            effect,
            rule: Some(rule),
        }
    }

    fn rule_for(&self, button: &Sexp, code: u16) -> Rule {
        Rule::new(
            &self.path,
            button.line(),
            &format!(
                "deflayer {}: {} -> {}",
                self.layer,
                keys::evdev_name(code),
                button.render()
            ),
        )
    }

    fn resolve(&self, button: &Sexp, press: Press, depth: usize) -> Resolved {
        if depth >= MAX_ALIAS_DEPTH {
            return Resolved::Effect(Effect::Conditional(format!(
                "kmonad button {} nests too deeply to evaluate",
                button.render()
            )));
        }
        let items = match button {
            Sexp::Atom(text, _) => return self.resolve_name(text, press, depth),
            Sexp::List(items, _) => items,
        };
        let head = button.head().unwrap_or_default();
        let target = items.get(1).map(Sexp::render).unwrap_or_default();
        let effect = match head {
            "around" | "around-only" | "around-when-alone" | "around-implicit" => {
                if head == "around-implicit" && !self.implicit_around {
                    return disabled_around(button);
                }
                let (Some(outer), Some(inner)) = (items.get(1), items.get(2)) else {
                    return not_evaluated(button);
                };
                return self.around(
                    self.resolve(outer, press, depth + 1),
                    self.resolve(inner, press, depth + 1),
                    press,
                    button,
                );
            }
            "layer-toggle" | "momentary-layer" => {
                Effect::Consume(format!("activates kmonad layer {target} while held"))
            }
            "layer-switch" | "permanent-layer" => {
                Effect::Consume(format!("switches the kmonad base layer to {target}"))
            }
            "layer-add" => Effect::Consume(format!("adds kmonad layer {target}")),
            "layer-rem" => Effect::Consume(format!("removes kmonad layer {target}")),
            "layer-delay" | "layer-next" => {
                Effect::Consume(format!("activates kmonad layer for {}", button.render()))
            }
            "tap-macro" | "tap-macro-release" => {
                Effect::Consume(format!("kmonad types {}", button.render()))
            }
            "cmd-button" => Effect::Consume("runs a kmonad command".into()),
            "pause" => Effect::Consume("kmonad pauses".into()),
            "press-only" | "release-only" => {
                Effect::Consume(format!("kmonad emits only {}", button.render()))
            }
            "around-next" | "around-next-single" | "before-after-next" => {
                Effect::Consume(format!("kmonad wraps the next key: {}", button.render()))
            }
            head if head.starts_with("tap-")
                || matches!(
                    head,
                    "multi-tap" | "sticky-key" | "stepped" | "around-next-timeout"
                ) =>
            {
                Effect::Conditional(tap_hold_text(head, items, button))
            }
            _ => return not_evaluated(button),
        };
        Resolved::Effect(effect)
    }

    fn resolve_name(&self, text: &str, press: Press, depth: usize) -> Resolved {
        match text {
            "_" => return Resolved::Unhandled,
            "XX" => return Resolved::Effect(Effect::Consume("blocked by kmonad (XX)".into())),
            _ => {}
        }
        if let Some(name) = text.strip_prefix('@').filter(|name| !name.is_empty()) {
            let Some((_, aliased)) = self.aliases.iter().find(|(alias, _)| alias == name) else {
                return Resolved::Effect(Effect::Conditional(format!(
                    "kmonad alias @{name} is not defined"
                )));
            };
            return self.resolve(aliased, press, depth + 1);
        }
        if let Some(code) = shifted(text) {
            if !self.implicit_around {
                return disabled_around(&Sexp::Atom(text.into(), 0));
            }
            return Resolved::Emit(Press::new(press.mods | SHIFT, code));
        }
        if let Some((bit, rest)) = PREFIXES.iter().find_map(|(prefix, bit)| {
            text.strip_prefix(prefix)
                .filter(|rest| !rest.is_empty())
                .map(|rest| (*bit, rest))
        }) {
            if !self.implicit_around {
                return disabled_around(&Sexp::Atom(text.into(), 0));
            }
            return match self.resolve_name(rest, press, depth + 1) {
                Resolved::Emit(inner) => Resolved::Emit(Press::new(inner.mods | bit, inner.code)),
                Resolved::Unhandled => not_evaluated(&Sexp::Atom(text.into(), 0)),
                effect => effect,
            };
        }
        if let Some(code) = keycode(text) {
            return Resolved::Emit(Press::new(press.mods, code));
        }
        let mut characters = text.chars();
        let single = characters.next().filter(|_| characters.next().is_none());
        if text
            .strip_prefix('P')
            .is_some_and(|ms| ms.parse::<u32>().is_ok())
        {
            Resolved::Effect(Effect::Consume("kmonad pauses".into()))
        } else if (text.len() == 2 && text.starts_with('+'))
            || single.is_some_and(|character| !character.is_ascii())
        {
            Resolved::Effect(Effect::Consume(format!(
                "kmonad types {text} as a compose sequence"
            )))
        } else {
            not_evaluated(&Sexp::Atom(text.into(), 0))
        }
    }

    /// `(around outer inner)` holds `outer` while `inner` is pressed.
    fn around(&self, outer: Resolved, inner: Resolved, press: Press, button: &Sexp) -> Resolved {
        match (outer, inner) {
            (Resolved::Emit(outer), Resolved::Emit(inner)) => {
                match keys::modifier_bit(outer.code).filter(|_| outer.mods == press.mods) {
                    Some(bit) => Resolved::Emit(Press::new(inner.mods | bit, inner.code)),
                    None => Resolved::Effect(Effect::Consume(format!(
                        "kmonad presses {}",
                        button.render()
                    ))),
                }
            }
            (Resolved::Effect(effect), _) | (_, Resolved::Effect(effect)) => {
                Resolved::Effect(effect)
            }
            _ => not_evaluated(button),
        }
    }
}

fn not_evaluated(button: &Sexp) -> Resolved {
    Resolved::Effect(Effect::Conditional(format!(
        "kmonad button {} is not evaluated",
        button.render()
    )))
}

fn disabled_around(button: &Sexp) -> Resolved {
    Resolved::Effect(Effect::Conditional(format!(
        "kmonad rejects {} because implicit-around is disabled",
        button.render()
    )))
}

/// Modifier prefixes of a kmonad button.
const PREFIXES: &[(&str, u32)] = &[
    ("RS-", SHIFT),
    ("RC-", CTRL),
    ("RA-", ALTGR),
    ("RM-", SUPER),
    ("S-", SHIFT),
    ("C-", CTRL),
    ("A-", ALT),
    ("M-", SUPER),
];

/// kmonad's names for shifted keys, which press Shift around the key.
fn shifted(name: &str) -> Option<u16> {
    let code = match name {
        "!" => 2,
        "@" => 3,
        "#" => 4,
        "$" => 5,
        "%" => 6,
        "^" => 7,
        "&" => 8,
        "*" => 9,
        "\\(" | "lprn" => 10,
        "\\)" | "rprn" => 11,
        "\\_" | "udrs" => 12,
        "+" => 13,
        "{" => 26,
        "}" => 27,
        ":" | "cln" => 39,
        "\"" | "quot" => 40,
        "~" | "tild" => 41,
        "|" | "pipe" => 43,
        "<" => 51,
        ">" => 52,
        "?" => 53,
        letter if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_uppercase()) => {
            return keys::evdev_code(letter);
        }
        _ => return None,
    };
    Some(code)
}

/// kmonad's keycode names: its aliases, then the Linux names without
/// `KEY_` (kmonad accepts them as written in its keycode list or in lower
/// case).
fn keycode(name: &str) -> Option<u16> {
    let code = match name {
        "ret" | "return" | "ent" => 28,
        "min" | "-" => 12,
        "eql" | "=" => 13,
        "zzz" => 142,
        "spc" => 57,
        "pgup" => 104,
        "pgdn" => 109,
        "ins" => 110,
        "del" => 111,
        "volu" | "volumeincrement" => 115,
        "voldwn" | "vold" | "volumedecrement" => 114,
        "brup" | "bru" => 225,
        "brdown" | "brdwn" | "brdn" => 224,
        "lalt" | "alt" => 56,
        "ralt" => 100,
        "comp" | "cmps" | "cmp" | "app" | "application" => 127,
        "lshift" | "lshft" | "lsft" | "shft" | "sft" => 42,
        "rshift" | "rshft" | "rsft" => 54,
        "lctrl" | "lctl" | "ctl" => 29,
        "rctrl" | "rctl" => 97,
        "lmeta" | "lmet" | "met" => 125,
        "rmeta" | "rmet" => 126,
        "bks" | "bspc" => 14,
        "caps" => 58,
        "102d" | "lsgt" | "nubs" => 86,
        "fwd" => 159,
        "scrlck" | "slck" => 70,
        "prnt" => 210,
        "lft" => 105,
        "rght" => 106,
        "lbrc" | "[" => 26,
        "rbrc" | "]" => 27,
        "scln" | ";" => 39,
        "apos" | "'" | "apo" => 40,
        "grv" | "`" => 41,
        "bksl" | "\\" | "\\\\" | "nonuspound" => 43,
        "comm" | "," => 51,
        "." => 52,
        "/" => 53,
        "nlck" => 69,
        "kp/" => 98,
        "kprt" => 96,
        "kp+" => 78,
        "kp*" => 55,
        "kp-" => 74,
        "kp." => 83,
        "ssrq" | "sys" => 99,
        "next" | "nexttrack" => 163,
        "pp" => 164,
        "prev" | "previoustrack" => 165,
        "mininteresting" => 113,
        _ => {
            let bare = name
                .strip_prefix("Key")
                .or_else(|| name.strip_prefix("key"))
                .unwrap_or(name);
            if bare.is_empty() || bare.contains('_') {
                return None;
            }
            return keys::evdev_code(bare);
        }
    };
    Some(code)
}

/// A timing-dependent kmonad button, naming its tap and hold outputs for
/// the `tap-hold` family (`tap-hold MS TAP HOLD`) and the `tap-next` family
/// (`tap-next TAP HOLD`).
fn tap_hold_text(head: &str, items: &[Sexp], button: &Sexp) -> String {
    let operands = if head.starts_with("tap-hold") {
        items.get(2).zip(items.get(3))
    } else if head.starts_with("tap-next") {
        items.get(1).zip(items.get(2))
    } else {
        None
    };
    match operands {
        Some((tap, hold)) => format!(
            "kmonad {}: a tap emits {}, holding emits {}; timing decides",
            button.render(),
            tap.render(),
            hold.render()
        ),
        None => format!(
            "kmonad button {} depends on timing or other keys",
            button.render()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(content: &str) -> Config {
        load(
            Path::new("/home/u/.config/kmonad/laptop.kbd"),
            content,
            &Cli::default(),
        )
        .unwrap()
    }

    fn effect(config: &Config, press: Press) -> Effect {
        config.evaluate_key(press).effect
    }

    const CFG: &str = "(defcfg input (device-file \"/dev/input/by-id/kbd\") output (uinput-sink \"kmonad\") fallthrough true)\n";

    #[test]
    fn first_layer_buttons_map_source_positions() {
        let config = config(&format!(
            "{CFG}(defalias cpy C-c nav (layer-toggle nav) th (tap-hold 200 a lctl) mc #(K M) par \\()\n(defsrc caps a s d f g h j)\n(deflayer base esc _ @th @nav @cpy XX @mc @par)\n(deflayer nav left _ _ _ _ _ _ _)\n"
        ));
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
        assert!(matches!(
            effect(&config, Press::new(0, 32)),
            Effect::Consume(_)
        ));
        assert_eq!(
            effect(&config, Press::new(0, 33)),
            Effect::Remap(Press::new(CTRL, 46))
        );
        assert!(matches!(
            effect(&config, Press::new(0, 34)),
            Effect::Consume(_)
        ));
        assert!(matches!(
            effect(&config, Press::new(0, 35)),
            Effect::Consume(_)
        ));
        assert_eq!(
            effect(&config, Press::new(0, 36)),
            Effect::Remap(Press::new(SHIFT, 10))
        );
        // Not in defsrc, and fallthrough re-emits it.
        assert_eq!(effect(&config, Press::new(0, 59)), Effect::Untouched);
    }

    #[test]
    fn without_fallthrough_unhandled_keys_are_dropped() {
        let config = config(
            "(defcfg input (device-file \"/dev/input/event3\") output (uinput-sink \"k\"))\n(defsrc caps a)\n(deflayer base esc _)\n",
        );
        assert!(matches!(
            effect(&config, Press::new(0, 30)),
            Effect::Consume(_)
        ));
        assert!(matches!(
            effect(&config, Press::new(0, 59)),
            Effect::Consume(_)
        ));
        // Ctrl never reaches the output, so Ctrl+Caps is not Ctrl+Esc.
        assert!(matches!(
            effect(&config, Press::new(CTRL, 58)),
            Effect::Conditional(_)
        ));
        // The command line can turn fallthrough on.
        let (path, cli) = command_line(&[
            "kmonad".into(),
            "-ldebug".into(),
            "--fallthrough".into(),
            "laptop.kbd".into(),
        ]);
        assert_eq!(path.as_deref(), Some("laptop.kbd"));
        let config = load(
            Path::new("laptop.kbd"),
            "(defcfg input (device-file \"/dev/input/event3\") output (uinput-sink \"k\"))\n(defsrc caps a)\n(deflayer base esc _)\n",
            &cli,
        )
        .unwrap();
        assert_eq!(effect(&config, Press::new(0, 30)), Effect::Untouched);
    }

    #[test]
    fn named_sources_and_modifier_arounds() {
        let config = config(&format!(
            "{CFG}(defsrc :name pad kp1 kp2)\n(defalias ca (around lctl (around lalt del)) ab (around a b))\n(deflayer pad :source pad @ca @ab)\n"
        ));
        assert_eq!(
            effect(&config, Press::new(0, 79)),
            Effect::Remap(Press::new(CTRL | ALT, 111))
        );
        assert!(matches!(
            effect(&config, Press::new(0, 80)),
            Effect::Consume(_)
        ));
    }

    #[test]
    fn bare_at_is_shifted_two() {
        let config = config(&format!("{CFG}(defsrc 1 2 3 4)\n(deflayer sym ! @ # $)\n"));
        assert_eq!(
            effect(&config, Press::new(0, 3)),
            Effect::Remap(Press::new(SHIFT, 3))
        );
        assert_eq!(
            effect(&config, Press::new(0, 4)),
            Effect::Remap(Press::new(SHIFT, 4))
        );
    }

    #[test]
    fn rejected_configurations_are_errors() {
        let cli = Cli::default();
        for content in [
            "(defsrc caps)",
            "(defcfg input (device-file \"/dev/x\"))(defsrc caps a)(deflayer base esc)",
            "(defcfg input (low-level-hook))(defsrc caps)(deflayer base esc)",
            "(defcfg input (device-file \"/dev/x\"))(deflayer base :source missing esc)(defsrc caps)",
        ] {
            assert!(
                load(Path::new("x.kbd"), content, &cli).is_err(),
                "{content}"
            );
        }
        let (_, cli) = command_line(&[
            "kmonad".into(),
            "-i".into(),
            "device-file \"/dev/input/event9\"".into(),
            "x.kbd".into(),
        ]);
        assert!(load(Path::new("x.kbd"), "(defsrc caps)(deflayer base esc)", &cli).is_ok());
    }

    #[test]
    fn tap_hold_on_a_modifier_names_tap_and_hold() {
        let config = config(&format!(
            "{CFG}(defalias m (tap-hold 200 esc lmet))\n(defsrc lmet a)\n(deflayer base @m _)\n"
        ));
        let Effect::Conditional(text) = effect(&config, Press::new(0, 125)) else {
            panic!("tap-hold is conditional");
        };
        assert!(
            text.contains("a tap emits esc, holding emits lmet"),
            "{text}"
        );
    }
}
