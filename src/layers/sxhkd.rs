use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, EvidenceTier, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// Read-only sxhkd binding adapter for bspwm-style sessions.
pub struct Sxhkd;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let config = load_config().ok_or_else(|| {
        "sxhkd: sxhkdrc was not found; runtime sxhkd state remains unknown".to_owned()
    })?;
    if config.files.is_empty() {
        return Err(format!("sxhkd: config: {}", config.unreadable.join("; ")));
    }
    Ok(config
        .bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "sxhkd",
                binding.combo.compact_display(),
                binding.command,
                "configured; runtime activation conditional",
            )
            .with_context("sxhkdrc static binding")
        })
        .collect())
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("bspwm"))
        })
        || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

impl Sxhkd {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(config) = load_config() else {
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unavailable,
                "sxhkd configuration is unavailable",
                vec!["set SXHKD_CONFIG or provide ~/.config/sxhkd/sxhkdrc".into()],
            );
        };
        let mut details: Vec<String> = config
            .files
            .iter()
            .map(|path| format!("config: {}", path.display()))
            .chain(
                config
                    .unreadable
                    .iter()
                    .map(|error| format!("config: {error}")),
            )
            .chain(config.skipped.iter().cloned())
            .collect();
        if config.files.is_empty() {
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unavailable,
                "could not read sxhkd configuration",
                details,
            );
        }
        let matches = config
            .bindings
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            details.push(
                "no matching static sxhkd binding found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unknown,
                "sxhkd shortcut state is conditional",
                details,
            )
            .with_tier(EvidenceTier::ConfigFile);
        }
        matched_result(&matches, details)
    }
}

/// The finding for the bindings matching the queried key, in config order.
/// sxhkd grabs every bound key synchronously and, for each event, replays
/// it to the focused client only when the matched chord is `~`-prefixed; a
/// press that matches only `@` (release) bindings is not replayed
/// (sxhkd.c `key_button_event`, types.c `find_hotkey`).
fn matched_result(matches: &[Binding], mut details: Vec<String>) -> LayerResult {
    for binding in matches {
        details.push(format!("binding: {}", binding.command));
    }
    let replayed = match matches.iter().find(|binding| !binding.release) {
        Some(binding) if binding.replay => {
            details
                .push("~: sxhkd runs the command and replays the key to the focused client".into());
            true
        }
        Some(_) => {
            details.push(
                "sxhkd takes the key with its grab and does not replay it (no ~ prefix)".into(),
            );
            false
        }
        None => {
            details.push(
                "@: the command runs on key release; sxhkd takes the press with its grab and does not replay it"
                    .into(),
            );
            false
        }
    };
    let (outcome, summary) = if replayed {
        (
            Outcome::HandledAndPassed,
            "matching sxhkd binding replays the key to the focused client (~)",
        )
    } else {
        (
            Outcome::HandledUncertain,
            "matching sxhkd shortcut configured; runtime activation is conditional",
        )
    };
    LayerResult::new("sxhkd", LayerId::Compositor, outcome, summary, details)
        .with_tier(EvidenceTier::ConfigFile)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    command: String,
    /// `~`: sxhkd replays the event to the focused client after matching.
    replay: bool,
    /// `@`: the command runs on the key release instead of the press.
    release: bool,
}

/// Bindings from every file sxhkd loads, in its load order.
struct LoadedConfig {
    files: Vec<PathBuf>,
    unreadable: Vec<String>,
    bindings: Vec<Binding>,
    /// Hotkey/command pairs not evaluated, as detail lines.
    skipped: Vec<String>,
}

/// sxhkd has no include directive: it reads its main config and then each
/// EXTRA_CONFIG operand, each file parsed on its own. `None` when no config
/// file is known.
fn load_config() -> Option<LoadedConfig> {
    let files = config_files();
    (!files.is_empty()).then(|| load_files(files))
}

fn load_files(files: Vec<PathBuf>) -> LoadedConfig {
    let mut config = LoadedConfig {
        files: Vec::new(),
        unreadable: Vec::new(),
        bindings: Vec::new(),
        skipped: Vec::new(),
    };
    for path in files {
        match crate::util::read_bounded(&path, MAX_CONFIG_BYTES) {
            Some(content) => {
                let bindings = parse_bindings(&content, &mut config.skipped);
                config.bindings.extend(bindings);
                config.files.push(path);
            }
            None => config
                .unreadable
                .push(format!("{}: unreadable or too large", path.display())),
        }
    }
    config
}

/// The files to read: `SXHKD_CONFIG` when set, else the files a running
/// sxhkd was started with, else the default sxhkdrc.
fn config_files() -> Vec<PathBuf> {
    if let Some(path) = override_path() {
        return vec![path];
    }
    crate::util::ProcessSnapshot::collect()
        .matching_pids(&["sxhkd"])
        .into_iter()
        .find_map(running_config_files)
        .unwrap_or_else(|| config_path().into_iter().collect())
}

/// The configured sxhkdrc, without looking at running processes.
fn config_path() -> Option<PathBuf> {
    override_path().or_else(|| default_config_path().filter(|path| path.is_file()))
}

fn override_path() -> Option<PathBuf> {
    env::var_os("SXHKD_CONFIG")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn default_config_path() -> Option<PathBuf> {
    crate::util::config_base().map(|base| base.join("sxhkd/sxhkdrc"))
}

/// Config files of the sxhkd process `pid`, read from its command line.
/// Relative paths are resolved against that process's working directory.
fn running_config_files(pid: u32) -> Option<Vec<PathBuf>> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmdline = String::from_utf8_lossy(&cmdline);
    let arguments: Vec<&str> = cmdline
        .strip_suffix('\0')
        .unwrap_or(&cmdline)
        .split('\0')
        .skip(1)
        .collect();
    let (config, extras) = sxhkd_config_arguments(&arguments);
    let cwd = fs::read_link(format!("/proc/{pid}/cwd")).ok();
    let resolve = |path: &str| match &cwd {
        Some(cwd) if Path::new(path).is_relative() => cwd.join(path),
        _ => PathBuf::from(path),
    };
    let main = match config {
        Some(path) => Some(resolve(path)),
        None => default_config_path(),
    };
    Some(
        main.into_iter()
            .chain(extras.into_iter().map(resolve))
            .collect(),
    )
}

/// Split sxhkd's arguments (without `argv[0]`) as its getopt string
/// `hvm:t:c:r:s:a:` does: returns the `-c CONFIG_FILE` value and the
/// EXTRA_CONFIG operands. Like glibc getopt, operands may appear anywhere
/// and `--` ends option parsing.
fn sxhkd_config_arguments<'a>(arguments: &[&'a str]) -> (Option<&'a str>, Vec<&'a str>) {
    let mut config = None;
    let mut extras = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        index += 1;
        if argument == "--" {
            extras.extend_from_slice(&arguments[index..]);
            break;
        }
        let Some(cluster) = argument
            .strip_prefix('-')
            .filter(|cluster| !cluster.is_empty())
        else {
            extras.push(argument);
            continue;
        };
        for (offset, option) in cluster.char_indices() {
            if !"mtcrsa".contains(option) {
                continue;
            }
            let attached = &cluster[offset + option.len_utf8()..];
            let value = if attached.is_empty() {
                index += 1;
                arguments.get(index - 1).copied()
            } else {
                Some(attached)
            };
            if option == 'c' && value.is_some() {
                config = value;
            }
            break;
        }
    }
    (config, extras)
}

/// Hotkey/command pairs as sxhkd 0.6 `load_config` (src/parse.c) reads them:
/// a line starting with a printable character is a hotkey, a line starting
/// with whitespace is a command, a `#` in the first column comments the line
/// out, and a trailing `\` continues it. A command completes the pair with
/// the latest hotkey. Lines are trimmed of every byte that is not ASCII
/// graphic (`lgraph`/`rgraph` with `isgraph` in the C locale, so non-ASCII
/// text at a line's edge goes too).
fn parse_bindings(content: &str, skipped: &mut Vec<String>) -> Vec<Binding> {
    let mut bindings = Vec::new();
    let mut hotkey = LineBuffer::new(SXHKD_MAXLEN);
    let mut command = LineBuffer::new(2 * SXHKD_MAXLEN);
    // One continuation offset shared by both buffers, as in sxhkd: a
    // continued command moves where the next hotkey line is written.
    let mut offset = 0;
    for line in content.lines() {
        let Some(first) = line.chars().next() else {
            continue;
        };
        if first == '#' {
            continue;
        }
        let text = line.trim_matches(|character: char| !character.is_ascii_graphic());
        if text.is_empty() {
            continue;
        }
        let target = if first.is_ascii_graphic() {
            &mut hotkey
        } else {
            &mut command
        };
        target.write_at(offset, text.as_bytes());
        if text.ends_with('\\') {
            offset += text.len() - 1;
            continue;
        }
        offset = 0;
        if first.is_ascii_whitespace() && !hotkey.text().is_empty() && !command.text().is_empty() {
            let hotkey_text = String::from_utf8_lossy(hotkey.text());
            match process_hotkey(&hotkey_text, &String::from_utf8_lossy(command.text())) {
                Some(expanded) => bindings.extend(expanded),
                None => skipped.push(format!(
                    "limitation: skipped `{hotkey_text}`: it expands to more than {MAX_RENDERINGS} bindings"
                )),
            }
            hotkey.clear();
            command.clear();
        }
    }
    bindings
}

/// sxhkd's `MAXLEN`: its hotkey buffer holds this many bytes, its command
/// buffer twice as many, each including the terminating NUL.
const SXHKD_MAXLEN: usize = 256;

/// A fixed-size C string buffer as `load_config` uses one: `snprintf` at an
/// offset writes what fits plus a NUL, `[0] = '\0'` clears it, and the text
/// ends at the first NUL. Writing past the current text leaves the text
/// unchanged, and an offset may split a multi-byte character, so the
/// buffer holds bytes.
struct LineBuffer {
    bytes: Vec<u8>,
}

impl LineBuffer {
    fn new(size: usize) -> Self {
        Self {
            bytes: vec![0; size],
        }
    }

    fn write_at(&mut self, offset: usize, text: &[u8]) {
        let Some(room) = self.bytes.len().checked_sub(offset + 1) else {
            return;
        };
        let count = text.len().min(room);
        self.bytes[offset..offset + count].copy_from_slice(&text[..count]);
        self.bytes[offset + count] = 0;
    }

    fn text(&self) -> &[u8] {
        let end = self
            .bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.bytes.len());
        &self.bytes[..end]
    }

    fn clear(&mut self) {
        self.bytes[0] = 0;
    }
}

/// One hotkey/command pair expanded as sxhkd `process_hotkey` does: the
/// `{…}` sequences of the hotkey and of the command are rendered in lockstep,
/// the n-th hotkey with the n-th command, until either runs out; text
/// without a sequence stays fixed.
fn process_hotkey(hotkey: &str, command: &str) -> Option<Vec<Binding>> {
    let (hotkeys, hotkey_fixed) = expand(hotkey);
    let (commands, command_fixed) = expand(command);
    let (Some(hotkeys), Some(commands)) = (hotkeys, commands) else {
        return None;
    };
    let count = match (hotkey_fixed, command_fixed) {
        (true, true) => 1,
        (true, false) => commands.len(),
        (false, true) => hotkeys.len(),
        (false, false) => hotkeys.len().min(commands.len()),
    };
    let bindings = (0..count)
        .filter_map(|index| {
            let hotkey = &hotkeys[if hotkey_fixed { 0 } else { index }];
            let command = &commands[if command_fixed { 0 } else { index }];
            parse_chord(hotkey).map(|(combo, replay, release)| Binding {
                combo,
                command: command.clone(),
                replay,
                release,
            })
        })
        .collect();
    Some(bindings)
}

/// Most renderings one hotkey or command line may expand to; a line beyond
/// it is skipped rather than materialized.
const MAX_RENDERINGS: usize = 4096;

/// Every rendering of `text` in sxhkd's order (`extract_chunks` and
/// `render_next`): each `{a,b,…}` sequence takes its elements in turn, the
/// first sequence varying fastest; an element `x-y` of three characters is
/// the range x..=y, `_` is the empty element, and `\` makes the next brace
/// or comma literal. The flag is true when `text` has no sequence. `None`
/// when the renderings would exceed [`MAX_RENDERINGS`]; sxhkd renders them
/// lazily, whykey would materialize them all.
fn expand(text: &str) -> (Option<Vec<String>>, bool) {
    enum Chunk {
        Text(String),
        Sequence(Vec<String>),
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut in_sequence = false;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let next = characters.next();
                // sxhkd keeps the backslash for get_token's own escaping
                // inside a sequence, and outside one unless it escapes a
                // brace or another backslash.
                if next
                    .is_none_or(|next| (next != '\\' || in_sequence) && next != '{' && next != '}')
                {
                    current.push('\\');
                }
                current.extend(next);
            }
            '{' => {
                if !current.is_empty() {
                    let text = std::mem::take(&mut current);
                    chunks.push(if in_sequence {
                        Chunk::Sequence(sequence_elements(&text))
                    } else {
                        Chunk::Text(text)
                    });
                }
                in_sequence = true;
            }
            '}' => {
                // An empty `{}` leaves no sequence behind (extract_chunks).
                let text = std::mem::take(&mut current);
                if in_sequence && !text.is_empty() {
                    chunks.push(Chunk::Sequence(sequence_elements(&text)));
                } else if !text.is_empty() {
                    chunks.push(Chunk::Text(text));
                }
                in_sequence = false;
            }
            _ => current.push(character),
        }
    }
    if in_sequence {
        chunks.push(Chunk::Sequence(sequence_elements(&current)));
    } else if !current.is_empty() {
        chunks.push(Chunk::Text(current));
    }
    let fixed = !chunks
        .iter()
        .any(|chunk| matches!(chunk, Chunk::Sequence(_)));
    let product = chunks
        .iter()
        .try_fold(1usize, |product, chunk| match chunk {
            Chunk::Text(_) => Some(product),
            Chunk::Sequence(elements) => product
                .checked_mul(elements.len())
                .filter(|&product| product <= MAX_RENDERINGS),
        });
    if product.is_none() {
        return (None, fixed);
    }
    let mut renderings = vec![String::new()];
    // Build from the last chunk backwards so the first sequence varies
    // fastest in the final order.
    for chunk in chunks.iter().rev() {
        renderings = match chunk {
            Chunk::Text(text) => renderings
                .into_iter()
                .map(|rest| format!("{text}{rest}"))
                .collect(),
            Chunk::Sequence(elements) => renderings
                .iter()
                .flat_map(|rest| {
                    elements
                        .iter()
                        .map(move |element| format!("{element}{rest}"))
                })
                .collect(),
        };
    }
    (Some(renderings), fixed)
}

/// The elements of one `{…}` sequence: comma-separated (empty elements are
/// skipped, as get_token does), `\,` is a literal comma, `_` renders as
/// nothing, and a three-character `x-y` with x <= y is the range x..=y.
fn sequence_elements(text: &str) -> Vec<String> {
    let mut elements = Vec::new();
    for element in tokens(text, &[',']) {
        let bytes = element.as_bytes();
        if bytes.len() == 3 && bytes[1] == b'-' && bytes[0] <= bytes[2] {
            elements.extend((bytes[0]..=bytes[2]).map(|byte| char::from(byte).to_string()));
        } else if element == "_" {
            elements.push(String::new());
        } else {
            elements.push(element);
        }
    }
    if elements.is_empty() {
        elements.push(String::new());
    }
    elements
}

/// sxhkd `get_token`: tokens between runs of `separators`, where a backslash
/// makes the next character literal.
fn tokens(text: &str, separators: &[char]) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(next) if next == '\\' || separators.contains(&next) => current.push(next),
                Some(next) => {
                    current.push('\\');
                    current.push(next);
                }
                None => current.push('\\'),
            }
        } else if separators.contains(&character) {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// One rendered hotkey as sxhkd `parse_chain` reads it: `+` or spaces
/// separate modifiers and the keysym, each token may start with `~` (replay)
/// and then `@` (release). Chains of several chords (`;`, `:`), pointer
/// buttons and the `any` modifier are not modelled and yield `None`.
fn parse_chord(hotkey: &str) -> Option<(KeyCombo, bool, bool)> {
    let chords = tokens(hotkey, &[';', ':']);
    let [chord] = chords.as_slice() else {
        return None;
    };
    let mut modifiers: Vec<&str> = Vec::new();
    let mut key = None;
    let mut replay = false;
    let mut release = false;
    for token in tokens(chord, &['+', ' ']) {
        let mut name = token.as_str();
        if let Some(rest) = name.strip_prefix('~') {
            replay = true;
            name = rest;
        }
        if let Some(rest) = name.strip_prefix('@') {
            release = true;
            name = rest;
        }
        // sxhkd parse_modifier, with names as X maps them by default:
        // Meta_L shares Mod1 with Alt, Mode_switch shares Mod5 with AltGr.
        let modifier = match name {
            "shift" => Some("shift"),
            "control" | "ctrl" => Some("ctrl"),
            "alt" | "meta" | "mod1" => Some("alt"),
            "super" | "mod4" => Some("super"),
            "hyper" | "mod3" => Some("hyper"),
            "mode_switch" | "mod5" => Some("mod5"),
            "lock" => Some("lock"),
            "mod2" => Some("mod2"),
            "any" => return None,
            _ => None,
        };
        match modifier {
            Some(modifier) => {
                if !modifiers.contains(&modifier) {
                    modifiers.push(modifier);
                }
            }
            None if is_button(name) || key.is_some() => return None,
            None => key = Some(name.to_owned()),
        }
    }
    let mut key = key?;
    // An uppercase letter keysym sits on the Shift level of its key, so
    // sxhkd binds it as Shift plus the base keysym (types.c make_chord).
    if key.len() == 1 && key.as_bytes()[0].is_ascii_uppercase() {
        key.make_ascii_lowercase();
        if !modifiers.contains(&"shift") {
            modifiers.push("shift");
        }
    }
    let combo = modifiers
        .iter()
        .copied()
        .chain(std::iter::once(key.as_str()))
        .collect::<Vec<_>>()
        .join("+")
        .parse::<KeyCombo>()
        .ok()?;
    Some((combo, replay, release))
}

fn is_button(name: &str) -> bool {
    name.strip_prefix("button").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn parses_simple_sxhkd_key_command_pairs() {
        let bindings = parse_bindings(
            "# comment\nsuper + Return\n    alacritty\n\nctrl + alt + t\n    notify-send terminal\n",
            &mut Vec::new(),
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert_eq!(bindings[0].command, "alacritty");
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    fn pairs(content: &str) -> Vec<(String, String, bool, bool)> {
        parse_bindings(content, &mut Vec::new())
            .into_iter()
            .map(|binding| {
                (
                    binding.combo.compact_display(),
                    binding.command,
                    binding.replay,
                    binding.release,
                )
            })
            .collect()
    }

    fn combo(text: &str) -> String {
        text.parse::<KeyCombo>().unwrap().compact_display()
    }

    #[test]
    fn continuation_offset_is_shared_and_byte_based() {
        // sxhkd's one offset (2, from the continued command) cuts "aéb" at
        // byte 2, inside `é`: the hotkey becomes "a\xc3q", which names no
        // key sxhkd knows. This used to panic in String::truncate.
        assert_eq!(
            pairs("aé\\\nb\n  yz\\\nq\n  cmd\nsuper + t\n  echo\n"),
            vec![
                (combo("a\u{fffd}q"), "cmd".into(), false, false),
                (combo("super+t"), "echo".into(), false, false),
            ]
        );
    }

    #[test]
    fn continuation_past_the_buffer_text_is_invisible() {
        // The command's offset (11) lands past the empty hotkey's NUL, so
        // sxhkd's hotkey stays empty and nothing binds.
        assert_eq!(pairs("  touch /tmp/\\\nsuper + t\n  x\n"), Vec::new());
    }

    #[test]
    fn trims_non_ascii_line_edges_like_sxhkd() {
        // sxhkd runs in the C locale, where isgraph is false for every byte
        // of a non-ASCII character.
        assert_eq!(
            pairs("super + t\n  notify-send Done ✓\nsuper + y\n\témacs\n"),
            vec![
                (combo("super+t"), "notify-send Done".into(), false, false),
                (combo("super+y"), "macs".into(), false, false),
            ]
        );
    }

    #[test]
    fn skips_brace_expansions_beyond_the_cap() {
        let started = std::time::Instant::now();
        let mut skipped = Vec::new();
        let bindings = parse_bindings(
            "super + {a-z}{a-z}{a-z}{a-z}{a-z}\n  echo\nsuper + {a-z}{a-z}\n  echo {a-z}{a-z}\n",
            &mut skipped,
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(bindings.len(), 26 * 26);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("more than 4096"), "{skipped:?}");
    }

    #[test]
    fn expands_sequences_in_lockstep_with_the_command() {
        assert_eq!(
            pairs("super + {t,y}\n  echo {one,two}\n"),
            vec![
                (combo("super+t"), "echo one".into(), false, false),
                (combo("super+y"), "echo two".into(), false, false),
            ]
        );
        // A command without a sequence is shared by every expansion.
        assert_eq!(
            pairs("super + {a,b}\n  echo fixed\n")
                .into_iter()
                .map(|(key, command, ..)| (key, command))
                .collect::<Vec<_>>(),
            vec![
                (combo("super+a"), "echo fixed".into()),
                (combo("super+b"), "echo fixed".into()),
            ]
        );
    }

    #[test]
    fn expands_ranges_and_empty_elements_first_sequence_fastest() {
        let bindings =
            pairs("super + {_,shift + }{1-3,0}\n  bspc {desktop -f,node -d} '^{1-3,10}'\n");
        assert_eq!(
            bindings
                .iter()
                .map(|(key, command, ..)| (key.as_str(), command.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (combo("super+1").as_str(), "bspc desktop -f '^1'"),
                (combo("super+shift+1").as_str(), "bspc node -d '^1'"),
                (combo("super+2").as_str(), "bspc desktop -f '^2'"),
                (combo("super+shift+2").as_str(), "bspc node -d '^2'"),
                (combo("super+3").as_str(), "bspc desktop -f '^3'"),
                (combo("super+shift+3").as_str(), "bspc node -d '^3'"),
                (combo("super+0").as_str(), "bspc desktop -f '^10'"),
                (combo("super+shift+0").as_str(), "bspc node -d '^10'"),
            ]
        );
        // Lockstep stops at the shorter side; a reversed range is literal.
        assert_eq!(pairs("{a,b,c}\n  echo {1,2}\n").len(), 2);
        assert_eq!(sequence_elements("9-1"), vec!["9-1"]);
        assert_eq!(sequence_elements("a-c,_,\\,"), vec!["a", "b", "c", "", ","]);
    }

    #[test]
    fn reads_replay_and_release_prefixes_and_plain_keys() {
        assert_eq!(
            pairs("~super + t\n  a\n@super + y\n  b\nsuper + ~@u\n  c\nF5\n  d\n~t\n  e\n"),
            vec![
                (combo("super+t"), "a".into(), true, false),
                (combo("super+y"), "b".into(), false, true),
                (combo("super+u"), "c".into(), true, true),
                (combo("f5"), "d".into(), false, false),
                (combo("t"), "e".into(), true, false),
            ]
        );
        // sxhkd reads `~` before `@`, so `@~t` names the unknown keysym `~t`.
        assert!(
            pairs("@~t\n  x\n")
                .iter()
                .all(|(key, ..)| *key != combo("t"))
        );
    }

    #[test]
    fn skips_what_sxhkd_does_not_bind_to_one_key() {
        // Chord chains, pointer buttons and `any` are not single key bindings.
        assert!(pairs("super + a ; b\n  x\nsuper + button1\n  y\nany + t\n  z\n").is_empty());
        // A comment counts only in the first column; an indented `#` line is
        // a command, and a hotkey line replaces an unpaired one.
        assert_eq!(
            pairs("ctrl + q\nsuper + t\n# note\n  # run\n"),
            vec![(combo("super+t"), "# run".into(), false, false)]
        );
        // A trailing backslash continues the line; a hotkey continues on a
        // line that starts in the first column.
        assert_eq!(
            pairs("super + \\\nt\n  echo \\\n  done\n"),
            vec![(combo("super+t"), "echo done".into(), false, false)]
        );
        // sxhkd's meta is Mod1 (Alt) and mode_switch Mod5 in the default map.
        assert_eq!(pairs("meta + t\n  x\n")[0].0, combo("alt+t"));
        // An uppercase letter is its key's Shift level.
        assert_eq!(pairs("super + T\n  x\n")[0].0, combo("super+shift+t"));
    }

    fn binding(key: &str, replay: bool, release: bool) -> Binding {
        Binding {
            combo: key.parse().unwrap(),
            command: "touch /tmp/fired".into(),
            replay,
            release,
        }
    }

    #[test]
    fn replay_decides_whether_the_press_is_passed_on() {
        let replayed = matched_result(&[binding("super+t", true, false)], Vec::new());
        assert_eq!(replayed.outcome, Outcome::HandledAndPassed);
        assert!(replayed.summary.contains("replays the key"));

        let grabbed = matched_result(&[binding("super+t", false, false)], Vec::new());
        assert_eq!(grabbed.outcome, Outcome::HandledUncertain);

        // A release binding (even `~@`) never replays the press.
        let release = matched_result(&[binding("super+t", true, true)], Vec::new());
        assert_eq!(release.outcome, Outcome::HandledUncertain);
        assert!(
            release
                .details
                .iter()
                .any(|detail| detail.starts_with("@:"))
        );

        // The first press binding decides, as sxhkd's find_hotkey returns it.
        let first = matched_result(
            &[
                binding("super+t", false, true),
                binding("super+t", true, false),
                binding("super+t", false, false),
            ],
            Vec::new(),
        );
        assert_eq!(first.outcome, Outcome::HandledAndPassed);
    }

    #[test]
    fn reads_the_config_and_extra_configs_from_sxhkd_arguments() {
        assert_eq!(
            sxhkd_config_arguments(&["-m", "1", "-c", "main", "extra1", "-t5", "extra2"]),
            (Some("main"), vec!["extra1", "extra2"])
        );
        assert_eq!(
            sxhkd_config_arguments(&["-vcmain", "--", "-extra"]),
            (Some("main"), vec!["-extra"])
        );
        assert_eq!(sxhkd_config_arguments(&[]), (None, Vec::new()));
    }

    #[test]
    fn loads_each_file_on_its_own_and_never_follows_include_lines() {
        let root = std::env::temp_dir().join(format!("whykey-sxhkd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // sxhkd has no include directive, so `include child` is not followed.
        // A chain left dangling at the end of one file must not borrow the
        // first command of the next one.
        fs::write(
            root.join("main"),
            "include child\nsuper + Return\n  root\nctrl + q\n",
        )
        .unwrap();
        fs::write(root.join("child"), "alt + c\n  included\n").unwrap();
        fs::write(root.join("extra"), "\n  stray\nctrl + x\n  extra\n").unwrap();
        let config = load_files(vec![
            root.join("main"),
            root.join("missing"),
            root.join("extra"),
        ]);
        assert_eq!(
            config
                .bindings
                .iter()
                .map(|binding| binding.command.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "extra"]
        );
        assert_eq!(config.files.len(), 2);
        assert_eq!(config.unreadable.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn non_regular_config_operands_are_reported_unreadable() {
        let root = std::env::temp_dir().join(format!("whykey-sxhkd-fifo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fifo = root.join("fifo");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let config = load_files(vec![fifo, root.clone()]);
        assert!(config.files.is_empty());
        assert_eq!(config.unreadable.len(), 2);
        let _ = fs::remove_dir_all(root);
    }
}
