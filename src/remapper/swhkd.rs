//! Static evaluation of swhkd configuration (`swhkdrc`).
//!
//! swhkd is a hotkey daemon, not a remapper, but it works at the same
//! stage: it grabs the keyboards' evdev devices and re-emits every key it
//! does not take through a uinput device, so a bound key never reaches the
//! compositor. Its syntax follows sxhkd's (`man 5 swhkd`), so brace
//! expansion is shared with the sxhkd adapter. What differs is modelled
//! here from the swhkd documentation and daemon source:
//!
//! - modifiers are `ctrl`/`control`, `super`/`mod4`, `alt`/`mod1`,
//!   `altgr`/`mod5` and `shift`; `any` matches any non-empty modifier set,
//!   and all others must equal the held set;
//! - `~` emits the key after the command (it is not swallowed); `@` runs
//!   the command on release;
//! - `include FILE` merges another file, `ignore HOTKEY` unbinds one,
//!   `mode NAME [swallow] [oneoff]` … `endmode` groups bindings that are
//!   active only while the mode is on the mode stack, and a `swallow` mode
//!   emits no key at all.
//!
//! Anything else is reported as not evaluated rather than guessed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::keys::{self, ALT, ALTGR, CTRL, Press, SHIFT, SUPER};
use super::{Effect, Finding, Rule, read_config};
use crate::layers::sxhkd::{render_pairs, tokens};

/// Nested `include` files followed before the rest is left unread.
const MAX_INCLUDE_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mods {
    /// The held modifiers must equal this set.
    Exact(u32),
    /// `any`: at least one modifier is held.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    mods: Mods,
    code: u16,
    /// `~`: the key is emitted as well.
    send: bool,
    /// `@`: the command runs on release.
    release: bool,
    /// The mode this binding belongs to; `None` is normal mode.
    mode: Option<String>,
    command: String,
    rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Config {
    bindings: Vec<Binding>,
    ignored: Vec<(Mods, u16)>,
    /// `swallow` modes with the line that declares them.
    swallowing: Vec<(String, Rule)>,
    /// Hotkeys that were not evaluated, with the reason.
    unevaluated: Vec<String>,
    /// `-D` limits the keyboards swhkd grabs.
    device_scoped: bool,
}

/// Read `path` and the files it includes. A file that cannot be read goes
/// to `unreadable`.
pub(crate) fn load(path: &Path, device_scoped: bool, unreadable: &mut Vec<String>) -> Config {
    let mut config = Config {
        device_scoped,
        ..Config::default()
    };
    let mut seen = BTreeSet::new();
    config.read_file(path, 0, &mut seen, unreadable);
    config
}

impl Config {
    fn read_file(
        &mut self,
        path: &Path,
        depth: usize,
        seen: &mut BTreeSet<PathBuf>,
        unreadable: &mut Vec<String>,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }
        if depth > MAX_INCLUDE_DEPTH {
            unreadable.push(format!(
                "{}: includes nested deeper than {MAX_INCLUDE_DEPTH}",
                path.display()
            ));
            return;
        }
        let content = match read_config(path) {
            Ok(content) => content,
            Err(reason) => {
                unreadable.push(reason);
                return;
            }
        };
        let file = path.display().to_string();
        let mut includes = Vec::new();
        self.parse(&file, &content, &mut includes);
        for include in includes {
            self.read_file(&include, depth + 1, seen, unreadable);
        }
    }

    /// One file's lines. Continued lines (a trailing `\`) are joined; an
    /// unindented line is a hotkey or directive, an indented one the
    /// command of the hotkey above it.
    fn parse(&mut self, file: &str, content: &str, includes: &mut Vec<PathBuf>) {
        let mut mode: Option<String> = None;
        let mut hotkey: Option<(String, u32)> = None;
        for (line, text, indented) in logical_lines(content) {
            let trimmed = text.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if indented {
                if let Some((hotkey, hotkey_line)) = hotkey.take() {
                    self.bind(file, hotkey_line, &hotkey, trimmed, mode.as_deref());
                }
                continue;
            }
            hotkey = None;
            let text = trimmed.split('#').next().unwrap_or_default().trim();
            let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
            match word {
                "include" if !rest.trim().is_empty() => {
                    includes.push(PathBuf::from(rest.trim()));
                }
                "ignore" if !rest.trim().is_empty() => match parse_chord(rest) {
                    Ok(chord) => self.ignored.push((chord.mods, chord.code)),
                    Err(reason) => self.unevaluated.push(format!("ignore {rest}: {reason}")),
                },
                "mode" if !rest.trim().is_empty() => {
                    let mut words = rest.split_whitespace();
                    let name = words.next().unwrap_or_default().to_owned();
                    if words.any(|option| option == "swallow") {
                        let rule = Rule::new(file, line, text);
                        self.swallowing.push((name.clone(), rule));
                    }
                    mode = Some(name);
                }
                "endmode" if rest.trim().is_empty() => mode = None,
                _ if !text.is_empty() => hotkey = Some((text.to_owned(), line)),
                _ => {}
            }
        }
    }

    fn bind(&mut self, file: &str, line: u32, hotkey: &str, command: &str, mode: Option<&str>) {
        let Some(pairs) = render_pairs(hotkey, command) else {
            self.unevaluated
                .push(format!("{hotkey}: expands to too many bindings"));
            return;
        };
        for (hotkey, command) in pairs {
            match parse_chord(&hotkey) {
                Ok(chord) => self.bindings.push(Binding {
                    mods: chord.mods,
                    code: chord.code,
                    send: chord.send,
                    release: chord.release,
                    mode: mode.map(str::to_owned),
                    rule: Rule::new(file, line, &format!("{hotkey}: {command}")),
                    command,
                }),
                Err(reason) => self.unevaluated.push(format!("{hotkey}: {reason}")),
            }
        }
    }

    pub(crate) fn evaluate(&self, press: Press) -> Vec<Finding> {
        let mut findings: Vec<Finding> = self
            .bindings
            .iter()
            .filter(|binding| binding.matches(press))
            .map(|binding| self.finding(binding))
            .collect();
        if findings.is_empty() {
            findings.push(self.unmatched());
        }
        if self.device_scoped {
            findings.push(Finding::untouched_because(
                "swhkd was started with -D and may not grab this keyboard",
            ));
        }
        findings
    }

    fn finding(&self, binding: &Binding) -> Finding {
        let command = &binding.command;
        let when = if binding.release {
            " on key release"
        } else {
            ""
        };
        let effect = if self.ignored.contains(&(binding.mods, binding.code)) {
            Effect::Conditional(format!(
                "swhkd binds this to `{command}` but an `ignore` line names it too; which one wins is not evaluated"
            ))
        } else if let Some(mode) = &binding.mode {
            Effect::Conditional(format!(
                "swhkd runs `{command}`{when} only while its `{mode}` mode is active"
            ))
        } else if binding.send {
            Effect::Conditional(format!(
                "swhkd runs `{command}`{when} and also passes the key on (~)"
            ))
        } else {
            Effect::Consume(format!(
                "swhkd runs `{command}`{when} and does not pass the key on"
            ))
        };
        Finding {
            effect,
            rule: Some(binding.rule.clone()),
        }
    }

    /// No binding names the key: it passes unless something unevaluated or
    /// a swallow mode could take it.
    fn unmatched(&self) -> Finding {
        if let Some((mode, rule)) = self.swallowing.first() {
            return Finding {
                effect: Effect::Conditional(format!(
                    "swhkd's swallow mode `{mode}` emits no key while it is active"
                )),
                rule: Some(rule.clone()),
            };
        }
        if let Some(first) = self.unevaluated.first() {
            return Finding {
                effect: Effect::Conditional(format!(
                    "{} swhkd hotkey(s) were not evaluated, so this key may be bound (first: {first})",
                    self.unevaluated.len()
                )),
                rule: None,
            };
        }
        Finding::untouched()
    }
}

impl Binding {
    fn matches(&self, press: Press) -> bool {
        self.code == press.code
            && match self.mods {
                Mods::Exact(mods) => mods == press.mods,
                Mods::Any => press.mods != 0,
            }
    }
}

/// Lines as `(first line number, text, indented)` with trailing-`\`
/// continuations joined.
fn logical_lines(content: &str) -> Vec<(u32, String, bool)> {
    let mut lines = Vec::new();
    let mut pending: Option<(u32, String, bool)> = None;
    for (index, line) in content.lines().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let (number, mut text, indented) = pending
            .take()
            .unwrap_or_else(|| (number, String::new(), line.starts_with(char::is_whitespace)));
        let piece = if text.is_empty() { line } else { line.trim() };
        match piece.trim_end().strip_suffix('\\') {
            Some(rest) => {
                text.push_str(rest);
                pending = Some((number, text, indented));
            }
            None => {
                text.push_str(piece);
                lines.push((number, text, indented));
            }
        }
    }
    if let Some(line) = pending {
        lines.push(line);
    }
    lines
}

struct Chord {
    mods: Mods,
    code: u16,
    send: bool,
    release: bool,
}

/// One rendered hotkey: `+`-separated modifiers and one key, the first
/// token optionally prefixed with `~` and/or `@`.
fn parse_chord(hotkey: &str) -> Result<Chord, String> {
    let mut mods = 0;
    let mut any = false;
    let mut code = None;
    let (mut send, mut release) = (false, false);
    for token in tokens(hotkey, &['+']) {
        let mut name = token.trim();
        while let Some(rest) = name.strip_prefix(['~', '@']) {
            send |= name.starts_with('~');
            release |= name.starts_with('@');
            name = rest.trim_start();
        }
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "" => {}
            "ctrl" | "control" => mods |= CTRL,
            "super" | "mod4" => mods |= SUPER,
            "alt" | "mod1" => mods |= ALT,
            "altgr" | "mod5" => mods |= ALTGR,
            "shift" => mods |= SHIFT,
            "any" => any = true,
            _ if code.is_some() => return Err("more than one key".into()),
            _ => code = Some(key_code(&name).ok_or_else(|| format!("key `{name}` is not mapped"))?),
        }
    }
    let code = code.ok_or("no key")?;
    Ok(Chord {
        mods: if any { Mods::Any } else { Mods::Exact(mods) },
        code,
        send,
        release,
    })
}

/// The evdev code of a swhkd key name (`man 5 swhkd-keys`), for the names
/// whose key is certain.
fn key_code(name: &str) -> Option<u16> {
    let linux = match name {
        "escape" => "ESC",
        "return" | "enter" => "ENTER",
        "-" | "minus" => "MINUS",
        "=" | "equal" => "EQUAL",
        "`" | "grave" => "GRAVE",
        "," | "comma" => "COMMA",
        "." | "dot" | "period" => "DOT",
        "/" | "slash" => "SLASH",
        ";" | "semicolon" => "SEMICOLON",
        "'" | "apostrophe" => "APOSTROPHE",
        "[" | "bracketleft" | "leftbrace" => "LEFTBRACE",
        "]" | "bracketright" | "rightbrace" => "RIGHTBRACE",
        "prior" => "PAGEUP",
        "next" => "PAGEDOWN",
        other => other,
    };
    keys::evdev_code(linux)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(content: &str) -> Config {
        let mut config = Config::default();
        config.parse("swhkdrc", content, &mut Vec::new());
        config
    }

    fn press(mods: u32, name: &str) -> Press {
        Press::new(mods, keys::evdev_code(name).unwrap())
    }

    fn effects(config: &Config, press: Press) -> Vec<Effect> {
        config
            .evaluate(press)
            .into_iter()
            .map(|finding| finding.effect)
            .collect()
    }

    #[test]
    fn plain_binding_swallows_the_key() {
        let config = config("super + Return\n    alacritty\n");
        let found = effects(&config, press(SUPER, "ENTER"));
        let [Effect::Consume(why)] = found.as_slice() else {
            panic!("expected a consume");
        };
        assert!(why.contains("alacritty") && why.contains("does not pass"));
        assert_eq!(
            effects(&config, press(SUPER | SHIFT, "ENTER")),
            vec![Effect::Untouched]
        );
        assert_eq!(effects(&config, press(0, "ENTER")), vec![Effect::Untouched]);
    }

    #[test]
    fn send_prefix_passes_the_key_on() {
        let config = config("~super + w\n    firefox\n");
        let found = effects(&config, press(SUPER, "W"));
        let [Effect::Conditional(why)] = found.as_slice() else {
            panic!("expected a conditional");
        };
        assert!(why.contains("also passes the key on"));
    }

    #[test]
    fn release_binding_still_takes_the_press() {
        let config = config("@super + shift + f\n    pcmanfm\n");
        let found = effects(&config, press(SUPER | SHIFT, "F"));
        let [Effect::Consume(why)] = found.as_slice() else {
            panic!("expected a consume");
        };
        assert!(why.contains("on key release"));
    }

    #[test]
    fn brace_expansion_and_omission_match_sxhkd() {
        let config = config("super + {_,shift + }{h,j}\n    bspc node -{f,s} {west,south}\n");
        assert_eq!(config.bindings.len(), 4);
        assert!(matches!(
            effects(&config, press(SUPER | SHIFT, "J")).as_slice(),
            [Effect::Consume(_)]
        ));
    }

    #[test]
    fn continued_hotkey_lines_are_joined() {
        let config = config("super + {Left\\\n,Right}\n  echo {l,r}\n");
        assert_eq!(config.bindings.len(), 2);
        assert_eq!(config.bindings[1].code, keys::evdev_code("RIGHT").unwrap());
    }

    #[test]
    fn any_modifier_needs_at_least_one_held() {
        let config = config("any + F5\n    reload\n");
        assert_eq!(effects(&config, press(0, "F5")), vec![Effect::Untouched]);
        assert!(matches!(
            effects(&config, press(CTRL, "F5")).as_slice(),
            [Effect::Consume(_)]
        ));
    }

    #[test]
    fn altgr_is_not_alt() {
        let config = config("altgr + a\n    one\n");
        assert!(matches!(
            effects(&config, press(ALTGR, "A")).as_slice(),
            [Effect::Consume(_)]
        ));
        assert_eq!(effects(&config, press(ALT, "A")), vec![Effect::Untouched]);
    }

    #[test]
    fn mode_bindings_are_conditional_and_endmode_returns_to_normal() {
        let config = config(
            "super + m\n    @enter music\n\nmode music\nq\n    @escape\nendmode\n\nsuper + x\n    lock\n",
        );
        assert!(matches!(
            effects(&config, press(0, "Q")).as_slice(),
            [Effect::Conditional(why)] if why.contains("`music` mode")
        ));
        assert!(matches!(
            effects(&config, press(SUPER, "X")).as_slice(),
            [Effect::Consume(_)]
        ));
    }

    #[test]
    fn swallow_mode_makes_unbound_keys_conditional() {
        let config = config("mode quiet swallow oneoff\na\n    echo\nendmode\n");
        assert!(matches!(
            effects(&config, press(0, "B")).as_slice(),
            [Effect::Conditional(why)] if why.contains("swallow mode `quiet`")
        ));
    }

    #[test]
    fn ignore_and_include_lines_are_not_hotkeys() {
        let mut includes = Vec::new();
        let mut config = Config::default();
        config.parse(
            "swhkdrc",
            "include /etc/swhkd/extra\nignore alt + print # unbound\nsuper + t\n    term\n",
            &mut includes,
        );
        assert_eq!(includes, vec![PathBuf::from("/etc/swhkd/extra")]);
        assert_eq!(config.bindings.len(), 1);
        // `print` has no certain evdev mapping, so only the ignore line is unevaluated.
        assert_eq!(config.unevaluated.len(), 1);
    }

    #[test]
    fn ignored_binding_is_not_a_confident_match() {
        let config = config("super + t\n    term\nignore super + t\n");
        assert!(matches!(
            effects(&config, press(SUPER, "T")).as_slice(),
            [Effect::Conditional(why)] if why.contains("`ignore` line")
        ));
    }

    #[test]
    fn unmapped_key_names_make_unbound_keys_uncertain() {
        let config = config("super + print\n    scrot\n");
        assert_eq!(config.unevaluated.len(), 1);
        assert!(matches!(
            effects(&config, press(0, "A")).as_slice(),
            [Effect::Conditional(why)] if why.contains("not evaluated")
        ));
    }

    #[test]
    fn device_scope_adds_an_untouched_possibility() {
        let mut config = config("super + t\n    term\n");
        config.device_scoped = true;
        assert_eq!(effects(&config, press(SUPER, "T")).len(), 2);
    }

    #[test]
    fn include_files_are_merged_and_cycles_stop() {
        let dir = std::env::temp_dir().join(format!("whykey-swhkd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("swhkdrc");
        let extra = dir.join("extra");
        std::fs::write(
            &main,
            format!("include {}\nsuper + a\n    one\n", extra.display()),
        )
        .unwrap();
        std::fs::write(
            &extra,
            format!("include {}\nsuper + b\n    two\n", main.display()),
        )
        .unwrap();
        let mut unreadable = Vec::new();
        let config = load(&main, false, &mut unreadable);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(unreadable.is_empty(), "{unreadable:?}");
        assert_eq!(config.bindings.len(), 2);
    }

    #[test]
    fn missing_include_is_unreadable() {
        let dir = std::env::temp_dir().join(format!("whykey-swhkd-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("swhkdrc");
        std::fs::write(&main, "include /nonexistent/whykey/extra\n").unwrap();
        let mut unreadable = Vec::new();
        load(&main, false, &mut unreadable);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(unreadable.len(), 1);
    }
}
