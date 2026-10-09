//! Shared i3-family (Sway and i3) adapter support.
//!
//! Neither compositor exposes a binding inventory over IPC, so bindings are
//! parsed from the running configuration returned by `GET_CONFIG`, plus the
//! files it includes, and matched against the mode reported by
//! `GET_BINDING_STATE`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::key::{KeyCombo, normalize_key_str};
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, LayerId, LayerResult, Outcome, SourceLocation,
    UncertaintyReason,
};

const DEFAULT_MODE: &str = "default";
const MAX_INCLUDE_DEPTH: usize = 16;

/// Per-compositor differences between Sway and i3.
pub(crate) struct Family {
    pub layer: &'static str,
    /// Inventory source label.
    pub source: &'static str,
    pub msg: &'static str,
    /// Arguments placed after `-t <type>`.
    pub msg_args: &'static [&'static str],
    /// Whether a `--release` binding forwards the key press to the focused
    /// client. `None` when the compositor's behavior is not verified.
    pub release_press_forwarded: Option<bool>,
    /// Configuration paths the compositor searches, in order.
    pub config_candidates: fn() -> Vec<PathBuf>,
}

/// Bit for a modifier name in the i3/Sway vocabulary, using the X11 layout
/// shared with [`KeyCombo`]. Names are those Sway/i3 accept in `bindsym` and
/// report in `event_state_mask`.
fn modifier_bit(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "shift" => 1,
        "lock" => 2,
        "control" | "ctrl" => 4,
        "mod1" | "alt" => 8,
        "mod2" => 16,
        "mod3" => 32,
        "mod4" | "super" => 64,
        "mod5" => 128,
        _ => return None,
    })
}

/// Layout group a binding is restricted to (`Group1`..`Group4`; i3's
/// `Mode_switch` selects the second group).
fn group_index(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if lower == "mode_switch" {
        return Some(2);
    }
    lower
        .strip_prefix("group")
        .and_then(|index| index.parse::<u8>().ok())
        .filter(|index| (1..=4).contains(index))
}

/// Decode an IPC `event_state_mask`. i3/Sway send an array of modifier names
/// (`["Shift", "Mod4"]`); a numeric X11 mask is accepted too. Group names
/// carry no modifier bit. `None` when the value is neither shape.
pub(crate) fn event_state_mask(value: &serde_json::Value) -> Option<u32> {
    if let Some(mask) = value.as_u64() {
        return u32::try_from(mask).ok();
    }
    let names = value.as_array()?;
    Some(
        names
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(modifier_bit)
            .fold(0, |mask, bit| mask | bit),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindKey {
    /// Normalized keysym name (see [`normalize_key_str`]).
    Sym(String),
    /// XKB keycode from `bindcode`.
    Code(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigBinding {
    pub mode: String,
    pub key: BindKey,
    /// `None` when the combination names a modifier whykey cannot map.
    pub mask: Option<u32>,
    pub group: Option<u8>,
    pub release: bool,
    pub locked: bool,
    pub inhibited: bool,
    pub input_device: Option<String>,
    /// Binding as written, after variable substitution.
    pub declaration: String,
    pub command: String,
    pub location: SourceLocation,
}

impl ConfigBinding {
    fn same_trigger(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.key == other.key
            && self.mask == other.mask
            && self.group == other.group
            && self.release == other.release
            && self.input_device == other.input_device
    }

    /// The binding applies only under a condition whykey cannot observe.
    fn condition(&self) -> Option<String> {
        if self.mask.is_none() {
            return Some("its modifiers include a name whykey cannot map".into());
        }
        if let Some(group) = self.group {
            return Some(format!(
                "it applies only while layout group {group} is active"
            ));
        }
        if let Some(device) = &self.input_device {
            return Some(format!("it applies only to input device {device}"));
        }
        None
    }

    fn key_display(&self) -> String {
        let key = match &self.key {
            BindKey::Sym(name) => name.clone(),
            BindKey::Code(code) => format!("code:{code}"),
        };
        match self.mask {
            Some(mask) => super::combo_display(mask, &key),
            None => key.to_ascii_uppercase(),
        }
    }
}

/// One config file: the main config from `GET_CONFIG` or an included file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigSource {
    pub path: Option<PathBuf>,
    pub contents: String,
}

#[derive(Debug, Default)]
pub(crate) struct ParsedConfig {
    pub bindings: Vec<ConfigBinding>,
    /// Config that could not be inspected; a missing binding may live there.
    pub gaps: Vec<String>,
    /// Evidence about how the config was assembled.
    pub notes: Vec<String>,
}

/// Split a `GET_CONFIG` reply into the main config and, for i3 ≥ 4.20, the
/// files i3 loaded (`included_configs`, main file first).
pub(crate) fn config_reply(
    value: &serde_json::Value,
) -> Result<(ConfigSource, Vec<ConfigSource>), String> {
    let main = value
        .get("config")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "get_config reply has no config text".to_owned())?;
    let mut loaded = value
        .get("included_configs")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(ConfigSource {
                path: entry
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(PathBuf::from),
                contents: entry.get("raw_contents")?.as_str()?.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let main_path = if loaded.first().is_some_and(|first| first.contents == main) {
        loaded.remove(0).path
    } else {
        None
    };
    Ok((
        ConfigSource {
            path: main_path,
            contents: main.to_owned(),
        },
        loaded,
    ))
}

/// Parse the main config, expanding `include` from the compositor-provided
/// files first and the filesystem second.
pub(crate) fn parse_config(main: &ConfigSource, provided: Vec<ConfigSource>) -> ParsedConfig {
    let mut parser = Parser {
        vars: Vec::new(),
        out: ParsedConfig::default(),
        used: vec![false; provided.len()],
        provided,
        visiting: Vec::new(),
    };
    parser.parse_source(main.path.as_deref(), &main.contents, 0);
    let leftovers = parser
        .used
        .iter()
        .enumerate()
        .filter(|(_, used)| !**used)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in leftovers {
        let source = parser.provided[index].clone();
        parser.out.notes.push(format!(
            "{} was loaded by the compositor but not reached from an include line; parsed after the main config",
            display_path(source.path.as_deref())
        ));
        parser.parse_source(source.path.as_deref(), &source.contents, 1);
    }
    parser.out
}

struct Parser {
    /// Variables with their `$` prefix, in definition order.
    vars: Vec<(String, String)>,
    out: ParsedConfig,
    provided: Vec<ConfigSource>,
    used: Vec<bool>,
    visiting: Vec<PathBuf>,
}

impl Parser {
    fn parse_source(&mut self, path: Option<&Path>, contents: &str, depth: usize) {
        let file = display_path(path);
        let lines = logical_lines(contents);
        let mut headers: Vec<String> = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let (line_no, text) = &lines[index];
            index += 1;
            if text == "}" {
                headers.pop();
                continue;
            }
            if let Some(header) = text.strip_suffix('{') {
                headers.push(header.trim().to_owned());
                continue;
            }
            // A block may open on the line after its header.
            if lines.get(index).is_some_and(|(_, next)| next == "{") {
                headers.push(text.clone());
                index += 1;
                continue;
            }
            let statement = if headers.is_empty() {
                text.clone()
            } else {
                format!("{} {text}", headers.join(" "))
            };
            let location = SourceLocation::new(file.clone(), u32::try_from(*line_no).ok());
            self.statement(&statement, path, location, depth);
        }
    }

    fn statement(
        &mut self,
        statement: &str,
        path: Option<&Path>,
        location: SourceLocation,
        depth: usize,
    ) {
        let raw = tokenize(statement);
        let Some(first) = raw.first() else {
            return;
        };
        match first.text.to_ascii_lowercase().as_str() {
            "set" => {
                if let (Some(name), Some(value_start)) =
                    (raw.get(1), raw.get(2).map(|token| token.start))
                {
                    let value = self.substitute(statement[value_start..].trim());
                    self.define(&name.text, unquote(&value));
                }
                return;
            }
            "set_from_resource" => {
                if let (Some(name), Some(fallback)) = (raw.get(1), raw.get(3)) {
                    let value = self.substitute(statement[fallback.start..].trim());
                    self.out.notes.push(format!(
                        "{} uses its set_from_resource fallback; X resources are not read",
                        name.text
                    ));
                    self.define(&name.text, unquote(&value));
                }
                return;
            }
            _ => {}
        }
        let statement = self.substitute(statement);
        let tokens = tokenize(&statement);
        let Some(keyword) = tokens.first().map(|token| token.text.to_ascii_lowercase()) else {
            return;
        };
        match keyword.as_str() {
            "include" => {
                if let Some(target) = tokens.get(1) {
                    let target = statement[target.start..].trim().to_owned();
                    self.include(&target, path, depth);
                }
            }
            "mode" => {
                let mut rest = tokens[1..].iter();
                let name = rest.find(|token| !token.text.starts_with("--"));
                let Some(name) = name else {
                    return;
                };
                let tail = rest.as_slice();
                if let Some(bind) = tail.first() {
                    self.binding(&statement, tail, &bind.text, &name.text, location);
                }
            }
            _ => self.binding(&statement, &tokens, &tokens[0].text, DEFAULT_MODE, location),
        }
    }

    fn define(&mut self, name: &str, value: String) {
        if !name.starts_with('$') {
            return;
        }
        self.vars.retain(|(existing, _)| existing != name);
        self.vars.push((name.to_owned(), value));
    }

    /// Replace `$name` references, preferring the longest defined name at
    /// each position (`$mod` must not shadow `$mod_alt`).
    fn substitute(&self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(position) = rest.find('$') {
            output.push_str(&rest[..position]);
            rest = &rest[position..];
            let longest = self
                .vars
                .iter()
                .filter(|(name, _)| rest.starts_with(name.as_str()))
                .max_by_key(|(name, _)| name.len());
            match longest {
                Some((name, value)) => {
                    output.push_str(value);
                    rest = &rest[name.len()..];
                }
                None => {
                    output.push('$');
                    rest = &rest[1..];
                }
            }
        }
        output.push_str(rest);
        output
    }

    fn binding(
        &mut self,
        statement: &str,
        tokens: &[Token],
        keyword: &str,
        mode: &str,
        location: SourceLocation,
    ) {
        let keyword = keyword.to_ascii_lowercase();
        let (code, unbind) = match keyword.as_str() {
            "bindsym" => (false, false),
            "bindcode" => (true, false),
            "unbindsym" => (false, true),
            "unbindcode" => (true, true),
            _ => return,
        };
        let mut release = false;
        let mut locked = false;
        let mut inhibited = false;
        let mut input_device = None;
        let mut position = 1;
        while let Some(flag) = tokens
            .get(position)
            .filter(|token| token.text.starts_with("--"))
        {
            match flag.text.split_once('=') {
                Some(("--input-device", device)) if device != "*" => {
                    input_device = Some(device.to_owned())
                }
                _ => match flag.text.as_str() {
                    "--release" => release = true,
                    "--locked" => locked = true,
                    "--inhibited" => inhibited = true,
                    _ => {}
                },
            }
            position += 1;
        }
        let Some(combo) = tokens.get(position) else {
            return;
        };
        let command = tokens
            .get(position + 1)
            .map_or("", |token| statement[token.start..].trim())
            .to_owned();
        let Some((key, mask, group)) = parse_trigger(&combo.text, code) else {
            return;
        };
        let binding = ConfigBinding {
            mode: unquote(mode),
            key,
            mask,
            group,
            release,
            locked,
            inhibited,
            input_device,
            declaration: statement[tokens[0].start..combo.end].to_owned(),
            command,
            location,
        };
        self.out
            .bindings
            .retain(|existing| !existing.same_trigger(&binding));
        if !unbind {
            self.out.bindings.push(binding);
        }
    }

    fn include(&mut self, target: &str, from: Option<&Path>, depth: usize) {
        if depth >= MAX_INCLUDE_DEPTH {
            self.out.gaps.push(format!(
                "include {target}: nested deeper than {MAX_INCLUDE_DEPTH} levels"
            ));
            return;
        }
        let expanded = expand_shell_path(&unquote(target));
        let path = PathBuf::from(&expanded);
        let path = if path.is_absolute() {
            path
        } else if let Some(base) = from.and_then(Path::parent) {
            base.join(path)
        } else {
            self.out.gaps.push(format!(
                "include {target}: relative path, but the including file's location is unknown"
            ));
            return;
        };
        let paths = match expand_glob(&path) {
            Ok(paths) => paths,
            Err(error) => {
                self.out.gaps.push(format!("include {target}: {error}"));
                return;
            }
        };
        if paths.is_empty() {
            self.out
                .notes
                .push(format!("include {target} matched no files"));
        }
        for path in paths {
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
            if self.visiting.contains(&identity) {
                continue;
            }
            let provided = self.provided.iter().position(|source| {
                source.path.as_deref().is_some_and(|candidate| {
                    candidate == path
                        || candidate.canonicalize().ok().as_deref() == Some(identity.as_path())
                })
            });
            let contents = match provided {
                Some(index) => {
                    self.used[index] = true;
                    self.provided[index].contents.clone()
                }
                None => match std::fs::read_to_string(&path) {
                    Ok(contents) => {
                        self.out.notes.push(format!(
                            "{} read from disk; it may have changed since the compositor loaded it",
                            path.display()
                        ));
                        contents
                    }
                    Err(error) => {
                        self.out
                            .gaps
                            .push(format!("include {}: {error}", path.display()));
                        continue;
                    }
                },
            };
            self.visiting.push(identity);
            self.parse_source(Some(&path), &contents, depth + 1);
            self.visiting.pop();
        }
    }
}

/// `(key, mask, group)` for a `bindsym`/`bindcode` trigger. `None` for
/// pointer buttons and unparsable keycodes, which never match a key query.
fn parse_trigger(combo: &str, code: bool) -> Option<(BindKey, Option<u32>, Option<u8>)> {
    let mut parts = combo.split('+').collect::<Vec<_>>();
    let key = parts.pop().filter(|key| !key.is_empty())?;
    let mut mask = Some(0);
    let mut group = None;
    for part in parts {
        if let Some(index) = group_index(part) {
            group = Some(index);
        } else {
            mask = mask.zip(modifier_bit(part)).map(|(mask, bit)| mask | bit);
        }
    }
    let key = if code {
        BindKey::Code(key.parse().ok()?)
    } else {
        let lower = key.to_ascii_lowercase();
        if lower.starts_with("button") || lower.starts_with("btn_") {
            return None;
        }
        BindKey::Sym(normalize_key_str(key))
    };
    Some((key, mask, group))
}

/// Physical lines joined across trailing `\`, without blank lines or
/// comments, each with its first line number.
fn logical_lines(contents: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (index, line) in contents.lines().enumerate() {
        let (start, mut text) = match pending.take() {
            Some((start, mut text)) => {
                text.push_str(line);
                (start, text)
            }
            None => (index + 1, line.to_owned()),
        };
        if text.ends_with('\\') {
            text.pop();
            pending = Some((start, text));
            continue;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        lines.push((start, trimmed.to_owned()));
    }
    if let Some((start, text)) = pending {
        let trimmed = text.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            lines.push((start, trimmed.to_owned()));
        }
    }
    lines
}

struct Token {
    text: String,
    start: usize,
    end: usize,
}

/// Whitespace-separated words; quoted words keep their spaces and lose the
/// quotes. Byte offsets let callers recover raw command text.
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, first)) = chars.peek() {
        if first.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        let mut quote = None;
        let mut end = start;
        while let Some(&(index, ch)) = chars.peek() {
            match quote {
                Some(open) if ch == open => quote = None,
                Some(_) => word.push(ch),
                None if ch == '"' || ch == '\'' => quote = Some(ch),
                None if ch.is_whitespace() => break,
                None => word.push(ch),
            }
            end = index + ch.len_utf8();
            chars.next();
        }
        tokens.push(Token {
            text: word,
            start,
            end,
        });
    }
    tokens
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = trimmed
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    trimmed.to_owned()
}

/// The shell expansion Sway/i3 apply to include paths that matters for
/// locating files: `~` and environment variables.
fn expand_shell_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_owned(),
    };
    let mut output = String::with_capacity(path.len());
    let mut rest = path.as_str();
    while let Some(position) = rest.find('$') {
        output.push_str(&rest[..position]);
        rest = &rest[position + 1..];
        let (name, after) = if let Some(braced) = rest.strip_prefix('{') {
            match braced.split_once('}') {
                Some((name, after)) => (name, after),
                None => ("", rest),
            }
        } else {
            let end = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        if name.is_empty() {
            output.push('$');
        } else {
            output.push_str(&std::env::var(name).unwrap_or_default());
            rest = after;
        }
    }
    output.push_str(rest);
    output
}

/// Expand `*`/`?` wildcards in the final path component, sorted like the
/// shell does. Wildcards in directory components are not supported.
fn expand_glob(path: &Path) -> Result<Vec<PathBuf>, String> {
    let is_pattern = |text: &str| text.contains(['*', '?', '[']);
    let parent = path.parent().unwrap_or(Path::new("/"));
    if is_pattern(&parent.to_string_lossy()) {
        return Err("wildcards in directory names are not expanded".into());
    }
    let Some(pattern) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(vec![path.to_owned()]);
    };
    if !is_pattern(&pattern) {
        return Ok(vec![path.to_owned()]);
    }
    if pattern.contains('[') {
        return Err("bracket wildcards are not expanded".into());
    }
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", parent.display())),
    };
    let mut matches = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !name.starts_with('.') && wildcard_match(&pattern, &name)
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    matches.sort();
    Ok(matches)
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let name = name.chars().collect::<Vec<_>>();
    let (mut p, mut n) = (0, 0);
    let mut backtrack = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p, n));
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(&ch) if ch == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match backtrack {
                Some((star, matched)) => {
                    p = star + 1;
                    n = matched + 1;
                    backtrack = Some((star, matched + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|ch| *ch == '*')
}

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(
        || "running config".to_owned(),
        |path| path.display().to_string(),
    )
}

/// Running configuration plus the binding mode it is evaluated in.
#[derive(Debug)]
pub(crate) struct LoadedConfig {
    pub parsed: ParsedConfig,
    /// Mode from `GET_BINDING_STATE`; `None` when it could not be read.
    pub active_mode: Option<String>,
}

fn run_msg(family: &Family, message: &str) -> Result<serde_json::Value, String> {
    let mut command = Command::new(family.msg);
    command.args(["-t", message]).args(family.msg_args);
    let output = crate::command::output(&mut command).map_err(|error| error.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            format!("{} exited with {}", family.msg, output.status)
        } else {
            format!("{}: {message}", family.msg)
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{} returned invalid {message} data: {error}", family.msg))
}

pub(crate) fn version_available(family: &Family) -> bool {
    run_msg(family, "get_version").is_ok()
}

pub(crate) fn focused_pid(family: &Family) -> Result<u32, String> {
    let tree = run_msg(family, "get_tree")?;
    focused_pid_in_tree(&tree).ok_or_else(|| {
        format!(
            "{} focused tree node does not expose a valid PID",
            family.layer
        )
    })
}

/// Read and parse the running configuration.
pub(crate) fn load(family: &Family) -> Result<LoadedConfig, String> {
    let reply = run_msg(family, "get_config")?;
    let (mut main, provided) = config_reply(&reply)?;
    if main.path.is_none() {
        // Sway does not report the config path; identify it by content so
        // relative includes resolve the way the compositor resolved them.
        main.path = (family.config_candidates)().into_iter().find(|candidate| {
            std::fs::read_to_string(candidate).is_ok_and(|contents| contents == main.contents)
        });
    }
    let parsed = parse_config(&main, provided);
    let active_mode = run_msg(family, "get_binding_state").ok().and_then(|state| {
        state
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    Ok(LoadedConfig {
        parsed,
        active_mode,
    })
}

fn source_line(family: &Family) -> String {
    format!(
        "source: parsed from running config via `{} -t get_config`",
        family.msg
    )
}

/// Bindings as inventory records, from every mode.
pub(crate) fn inventory(family: &Family) -> Result<Vec<BindingRecord>, String> {
    let loaded = load(family)?;
    Ok(loaded
        .parsed
        .bindings
        .iter()
        .map(|binding| {
            let mut record = BindingRecord::new(
                family.source,
                binding.key_display(),
                binding.command.clone(),
                if binding.condition().is_some() {
                    "parsed from running config; conditional binding"
                } else {
                    "parsed from running config"
                },
            )
            .with_context(format!(
                "{}{}",
                binding.location.file,
                binding
                    .location
                    .line
                    .map_or_else(String::new, |line| format!(":{line}"))
            ));
            if binding.mode != DEFAULT_MODE {
                record.submap = Some(binding.mode.clone());
            }
            record.device = binding.input_device.clone();
            record
        })
        .collect())
}

pub(crate) fn inspect(
    family: &Family,
    loaded: Result<LoadedConfig, String>,
    key: &KeyCombo,
) -> LayerResult {
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            return LayerResult::new(
                family.layer,
                LayerId::Compositor,
                Outcome::Unavailable,
                format!("could not read the running {} config", family.layer),
                vec![error],
            );
        }
    };
    let parsed = &loaded.parsed;
    let mode = loaded.active_mode.as_deref().unwrap_or(DEFAULT_MODE);
    let mut details = vec![
        source_line(family),
        match &loaded.active_mode {
            Some(mode) => format!("binding mode: {mode} (get_binding_state)"),
            None => "binding mode unknown (get_binding_state failed); assuming default".into(),
        },
    ];
    let query_code = key
        .key()
        .strip_prefix("CODE:")
        .and_then(|value| value.parse::<u32>().ok());
    let in_mode = parsed
        .bindings
        .iter()
        .filter(|binding| binding.mode == mode)
        .collect::<Vec<_>>();
    let matches_key = |binding: &&ConfigBinding| {
        (match (&binding.key, query_code) {
            (BindKey::Code(code), Some(query)) => *code == query,
            (BindKey::Sym(name), None) => name.eq_ignore_ascii_case(key.key()),
            _ => false,
        }) && binding.mask.is_none_or(|mask| mask == key.modmask())
    };
    let candidates = in_mode
        .iter()
        .copied()
        .filter(|binding| matches_key(binding))
        .collect::<Vec<_>>();
    let uncompared = in_mode
        .iter()
        .filter(|binding| matches!(binding.key, BindKey::Code(_)) != query_code.is_some())
        .count();
    if uncompared > 0 {
        details.push(format!(
            "{uncompared} {} binding(s) in this mode were not compared: matching them needs the keymap",
            if query_code.is_some() { "bindsym" } else { "bindcode" }
        ));
    }

    let definite = candidates
        .iter()
        .copied()
        .filter(|binding| binding.condition().is_none())
        .collect::<Vec<_>>();
    let press = definite.iter().find(|binding| !binding.release).copied();
    let release = definite.iter().find(|binding| binding.release).copied();
    let conditional = candidates
        .iter()
        .copied()
        .filter(|binding| binding.condition().is_some())
        .collect::<Vec<_>>();

    for binding in press.iter().chain(release.iter()).chain(conditional.iter()) {
        details.push(format!(
            "binding: {} {} ({}{})",
            binding.declaration,
            binding.command,
            binding.location.file,
            binding
                .location
                .line
                .map_or_else(String::new, |line| format!(":{line}"))
        ));
        if let Some(condition) = binding.condition() {
            details.push(format!("conditional: {condition}"));
        }
    }
    details.extend(
        parsed
            .gaps
            .iter()
            .map(|gap| format!("not inspected: {gap}")),
    );
    let mode_uncertainty = loaded.active_mode.is_none();
    // With the mode unknown, a key bound only in another mode may still be
    // captured: that mode could be the active one.
    let other_modes = if mode_uncertainty {
        parsed
            .bindings
            .iter()
            .filter(|binding| binding.mode != mode && !binding.release)
            .filter(|binding| matches_key(binding))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let (outcome, summary, primary, uncertainty) = if let Some(binding) = press {
        if !binding.inhibited {
            details.push(
                "not active while the focused surface inhibits keyboard shortcuts (no --inhibited)"
                    .into(),
            );
        }
        (
            Outcome::Consumed,
            "active binding found".to_owned(),
            Some(binding),
            mode_uncertainty.then_some(UncertaintyReason::UnresolvedMode),
        )
    } else if let Some(binding) = release {
        match family.release_press_forwarded {
            Some(true) => {
                details.push(format!(
                    "--release: {} forwards the key press to the focused client and runs the command on release",
                    family.layer
                ));
                (
                    Outcome::HandledAndPassed,
                    "active --release binding found".to_owned(),
                    Some(binding),
                    mode_uncertainty.then_some(UncertaintyReason::UnresolvedMode),
                )
            }
            Some(false) => {
                details.push(format!(
                    "--release: {} grabs the key, so the focused client receives neither the press nor the release; the command runs on release",
                    family.layer
                ));
                (
                    Outcome::Consumed,
                    "active --release binding found".to_owned(),
                    Some(binding),
                    mode_uncertainty.then_some(UncertaintyReason::UnresolvedMode),
                )
            }
            None => {
                details.push(format!(
                    "--release: the command runs on release; whether {} withholds the press from the focused client is not verified",
                    family.layer
                ));
                (
                    Outcome::HandledUncertain,
                    "active --release binding found".to_owned(),
                    Some(binding),
                    Some(UncertaintyReason::OpaqueDispatcher),
                )
            }
        }
    } else if let Some(binding) = conditional.first().copied() {
        (
            Outcome::Unknown,
            "possible binding found; it applies only under conditions whykey cannot verify"
                .to_owned(),
            Some(binding),
            Some(UncertaintyReason::ModifierAmbiguity),
        )
    } else if mode_uncertainty {
        let mut modes = other_modes
            .iter()
            .map(|binding| binding.mode.as_str())
            .collect::<Vec<_>>();
        modes.sort_unstable();
        modes.dedup();
        if modes.is_empty() {
            details.push("no mode in the inspected config binds this key".into());
        } else {
            details.push(format!("bound in other mode(s): {}", modes.join(", ")));
        }
        (
            if parsed.gaps.is_empty() {
                Outcome::UncertainContinues
            } else {
                Outcome::Unknown
            },
            format!("no binding found in the assumed {mode} mode; the active mode is unknown"),
            other_modes.first().copied(),
            Some(UncertaintyReason::UnresolvedMode),
        )
    } else if !parsed.gaps.is_empty() {
        (
            Outcome::Unknown,
            "no binding found in the inspected config, but part of the config could not be read"
                .to_owned(),
            None,
            None,
        )
    } else {
        (
            Outcome::Pass,
            "no active binding found".to_owned(),
            None,
            None,
        )
    };

    let result = LayerResult::new(family.layer, LayerId::Compositor, outcome, summary, details)
        .with_verbose_details(parsed.notes.clone());
    match primary {
        Some(binding) => {
            let mut evidence = BindingEvidence::action(
                binding.command.clone(),
                BindingScope::Submap(binding.mode.clone()),
                Some(binding.location.clone()),
            )
            .with_submap(binding.mode.clone());
            if let Some(dispatcher) = binding.command.split_whitespace().next() {
                evidence = evidence.with_dispatcher(dispatcher);
            }
            if let Some(reason) = uncertainty {
                evidence = evidence.with_uncertainty(reason);
            }
            result.with_binding(evidence)
        }
        None => result,
    }
}

pub(crate) fn focused_pid_in_tree(value: &serde_json::Value) -> Option<u32> {
    if value.get("focused").and_then(serde_json::Value::as_bool) == Some(true) {
        if let Some(pid) = value
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            return Some(pid);
        }
    }
    value
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find_map(focused_pid_in_tree)
        .or_else(|| {
            value
                .get("floating_nodes")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .find_map(focused_pid_in_tree)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWAY: Family = Family {
        layer: "Sway",
        source: "Sway",
        msg: "swaymsg",
        msg_args: &["-r"],
        release_press_forwarded: Some(true),
        config_candidates: Vec::new,
    };

    /// The production i3 family, so the test pins the shipped constant.
    const I3: Family = super::super::i3::FAMILY;

    fn main_config(contents: &str) -> ConfigSource {
        ConfigSource {
            path: None,
            contents: contents.into(),
        }
    }

    fn loaded(contents: &str, mode: Option<&str>) -> Result<LoadedConfig, String> {
        Ok(LoadedConfig {
            parsed: parse_config(&main_config(contents), Vec::new()),
            active_mode: mode.map(str::to_owned),
        })
    }

    fn query(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    const SWAY_CONFIG: &str = r#"# Default config for sway
set $mod Mod4
set $mod_alt Mod1
set $left h
set $term foot

bindsym $mod+Return exec $term
bindsym $mod+Shift+q kill
bindsym $mod_alt+Tab focus next
bindsym Ctrl+c exec copy
bindsym $mod+$left focus left
bindsym --to-code $mod+comma \
    layout tabbed
bindsym --release Print exec grim
bindsym --input-device=1:1:AT_Translated_Set_2_keyboard $mod+x exec only-laptop
bindsym $mod+Group2+y exec greek-only
bindsym $mod+Hyper+z exec unmapped-modifier
bindcode $mod+38 exec by-code
bindsym $mod+button3 floating toggle
bindsym $mod+p exec old
unbindsym $mod+p

mode "resize" {
    bindsym $left resize shrink width 10px
    bindsym Escape mode "default"
}
bindsym $mod+r mode "resize"
bindsym {
    $mod+1 workspace number 1
}
bar {
    bindsym button4 nop
}
"#;

    #[test]
    fn decodes_ipc_event_state_masks_from_names_and_numbers() {
        assert_eq!(
            event_state_mask(&serde_json::json!(["Shift", "Mod4", "Group2"])),
            Some(1 | 64)
        );
        assert_eq!(
            event_state_mask(&serde_json::json!(["Control", "Mod1", "Lock"])),
            Some(4 | 8 | 2)
        );
        assert_eq!(event_state_mask(&serde_json::json!(64)), Some(64));
        assert_eq!(event_state_mask(&serde_json::Value::Null), None);
    }

    #[test]
    fn parses_variables_modes_blocks_continuations_and_unbinds() {
        let parsed = parse_config(&main_config(SWAY_CONFIG), Vec::new());
        let find = |declaration: &str| {
            parsed
                .bindings
                .iter()
                .find(|binding| binding.declaration == declaration)
                .unwrap_or_else(|| panic!("missing {declaration}"))
        };
        let terminal = find("bindsym Mod4+Return");
        assert_eq!(terminal.command, "exec foot");
        assert_eq!(terminal.mask, Some(64));
        assert_eq!(terminal.key, BindKey::Sym("RETURN".into()));
        assert_eq!(terminal.location.line, Some(7));
        assert_eq!(
            find("bindsym Mod1+Tab").mask,
            Some(8),
            "longest variable wins"
        );
        assert_eq!(
            find("bindsym --to-code Mod4+comma").command,
            "layout tabbed"
        );
        assert_eq!(
            find("bindsym --to-code Mod4+comma").key,
            BindKey::Sym(",".into())
        );
        assert!(find("bindsym --release Print").release);
        assert_eq!(find("bindsym Mod4+Group2+y").group, Some(2));
        assert_eq!(find("bindsym Mod4+Hyper+z").mask, None);
        assert_eq!(find("bindcode Mod4+38").key, BindKey::Code(38));
        assert_eq!(find("bindsym Mod4+1").command, "workspace number 1");
        let resize = find("bindsym h");
        assert_eq!(resize.mode, "resize");
        assert_eq!(resize.command, "resize shrink width 10px");
        assert!(
            parsed
                .bindings
                .iter()
                .all(|binding| !binding.command.contains("exec old")),
            "unbindsym removes the earlier binding"
        );
        assert!(
            parsed
                .bindings
                .iter()
                .all(|binding| !binding.command.contains("floating") && binding.command != "nop"),
            "pointer button bindings are not keyboard bindings"
        );
    }

    #[test]
    fn matching_binding_in_the_active_mode_is_consumed() {
        let result = inspect(
            &SWAY,
            loaded(SWAY_CONFIG, Some("default")),
            &query("super+return"),
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.summary, "active binding found");
        assert!(
            result.details[0].contains("parsed from running config via `swaymsg -t get_config`")
        );
        assert!(result.details.iter().any(|line| line.contains("exec foot")));
        let binding = result.binding.unwrap();
        assert_eq!(binding.scope, BindingScope::Submap("default".into()));
        assert_eq!(binding.dispatcher.as_deref(), Some("exec"));

        let nop = inspect(
            &SWAY,
            loaded("bindsym Ctrl+k nop", Some("default")),
            &query("ctrl+k"),
        );
        assert_eq!(
            nop.outcome,
            Outcome::Consumed,
            "a nop binding still grabs the key"
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let result = inspect(
            &SWAY,
            loaded(SWAY_CONFIG, Some("default")),
            &query("return"),
        );
        assert_eq!(result.outcome, Outcome::Pass);
        let result = inspect(
            &SWAY,
            loaded(SWAY_CONFIG, Some("default")),
            &query("super+shift+return"),
        );
        assert_eq!(result.outcome, Outcome::Pass);
    }

    #[test]
    fn only_the_active_mode_applies() {
        let in_default = inspect(&SWAY, loaded(SWAY_CONFIG, Some("default")), &query("h"));
        assert_eq!(in_default.outcome, Outcome::Pass);
        let in_resize = inspect(&SWAY, loaded(SWAY_CONFIG, Some("resize")), &query("h"));
        assert_eq!(in_resize.outcome, Outcome::Consumed);
        assert!(
            in_resize
                .details
                .iter()
                .any(|line| line == "binding mode: resize (get_binding_state)")
        );

        let unknown = inspect(&SWAY, loaded(SWAY_CONFIG, None), &query("super+return"));
        assert_eq!(unknown.outcome, Outcome::Consumed);
        assert!(unknown.details[1].contains("assuming default"));
        assert_eq!(
            unknown.binding.unwrap().uncertainty,
            Some(UncertaintyReason::UnresolvedMode)
        );
    }

    #[test]
    fn unknown_mode_miss_is_uncertain_and_names_other_modes() {
        let resize_only = inspect(&SWAY, loaded(SWAY_CONFIG, None), &query("h"));
        assert_eq!(resize_only.outcome, Outcome::UncertainContinues);
        assert!(
            resize_only
                .details
                .iter()
                .any(|line| line.contains("resize"))
        );
        let evidence = resize_only.binding.expect("other-mode binding evidence");
        assert_eq!(
            evidence.uncertainty,
            Some(UncertaintyReason::UnresolvedMode)
        );
        assert_eq!(evidence.submap.as_deref(), Some("resize"));

        let unbound = inspect(&SWAY, loaded(SWAY_CONFIG, None), &query("ctrl+alt+f12"));
        assert_eq!(unbound.outcome, Outcome::UncertainContinues);
        assert!(unbound.binding.is_none());
    }

    #[test]
    fn release_bindings_follow_verified_compositor_semantics() {
        let sway = inspect(&SWAY, loaded(SWAY_CONFIG, Some("default")), &query("print"));
        assert_eq!(sway.outcome, Outcome::HandledAndPassed);
        assert!(
            sway.details
                .iter()
                .any(|line| line.contains("forwards the key press"))
        );
        let i3 = inspect(&I3, loaded(SWAY_CONFIG, Some("default")), &query("print"));
        assert_eq!(i3.outcome, Outcome::Consumed);
        assert_eq!(i3.binding.and_then(|binding| binding.uncertainty), None);
        assert!(!i3.details.iter().any(|line| line.contains("not verified")));
    }

    #[test]
    fn conditional_bindings_stay_unknown() {
        for combo in ["super+x", "super+y", "super+z"] {
            let result = inspect(&SWAY, loaded(SWAY_CONFIG, Some("default")), &query(combo));
            assert_eq!(result.outcome, Outcome::Unknown, "{combo}");
            assert!(
                result
                    .details
                    .iter()
                    .any(|line| line.starts_with("conditional:"))
            );
        }
    }

    #[test]
    fn keycode_queries_match_bindcode() {
        let result = inspect(
            &SWAY,
            loaded(SWAY_CONFIG, Some("default")),
            &query("super+code:38"),
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("exec by-code"))
        );
    }

    #[test]
    fn unreadable_includes_make_a_miss_unknown() {
        let result = inspect(
            &SWAY,
            loaded(
                "include /nonexistent/whykey/config.d/main\nbindsym Ctrl+c exec copy",
                Some("default"),
            ),
            &query("ctrl+q"),
        );
        assert_eq!(result.outcome, Outcome::Unknown);
        assert!(
            result
                .details
                .iter()
                .any(|line| line.starts_with("not inspected: include"))
        );

        let relative = parse_config(&main_config("include config.d/*"), Vec::new());
        assert!(relative.gaps[0].contains("relative path"));
    }

    #[test]
    fn includes_expand_globs_relative_to_the_including_file() {
        let base = std::env::temp_dir().join(format!("whykey-tiling-{}", std::process::id()));
        let conf = base.join("config.d");
        std::fs::create_dir_all(&conf).unwrap();
        std::fs::write(conf.join("10-apps.conf"), "bindsym $mod+b exec browser\n").unwrap();
        std::fs::write(conf.join("README"), "bindsym $mod+n exec ignored\n").unwrap();
        let main = ConfigSource {
            path: Some(base.join("config")),
            contents: "set $mod Mod4\ninclude config.d/*.conf\n".into(),
        };
        let parsed = parse_config(&main, Vec::new());
        let _ = std::fs::remove_dir_all(&base);
        assert!(parsed.gaps.is_empty(), "{:?}", parsed.gaps);
        assert_eq!(parsed.bindings.len(), 1);
        assert_eq!(parsed.bindings[0].command, "exec browser");
        assert_eq!(parsed.bindings[0].mask, Some(64));
        assert!(parsed.bindings[0].location.file.ends_with("10-apps.conf"));
    }

    #[test]
    fn i3_reply_uses_included_configs_before_the_filesystem() {
        let reply = serde_json::json!({
            "config": "set $mod Mod4\ninclude /home/user/.config/i3/keys.conf\n",
            "included_configs": [
                {"path": "/home/user/.config/i3/config", "raw_contents": "set $mod Mod4\ninclude /home/user/.config/i3/keys.conf\n", "variable_replaced_contents": ""},
                {"path": "/home/user/.config/i3/keys.conf", "raw_contents": "bindsym $mod+d exec dmenu_run\n", "variable_replaced_contents": ""}
            ]
        });
        let (main, provided) = config_reply(&reply).unwrap();
        assert_eq!(
            main.path.as_deref(),
            Some(Path::new("/home/user/.config/i3/config"))
        );
        let parsed = parse_config(&main, provided);
        assert_eq!(parsed.bindings.len(), 1);
        assert_eq!(parsed.bindings[0].command, "exec dmenu_run");
        assert_eq!(parsed.bindings[0].mask, Some(64));
        assert!(
            parsed.gaps.is_empty() && parsed.notes.is_empty(),
            "{parsed:?}"
        );
        assert_eq!(
            parsed.bindings[0].location.file,
            "/home/user/.config/i3/keys.conf"
        );

        let sway_reply = serde_json::json!({"config": "bindsym Ctrl+c exec copy\n"});
        let (main, provided) = config_reply(&sway_reply).unwrap();
        assert!(main.path.is_none() && provided.is_empty());
        assert!(config_reply(&serde_json::json!({})).is_err());
    }

    #[test]
    fn reports_unavailable_with_the_ipc_error() {
        let result = inspect(&SWAY, Err("swaymsg: not running".into()), &query("c"));
        assert_eq!(result.outcome, Outcome::Unavailable);
        assert_eq!(result.details, vec!["swaymsg: not running"]);
    }

    #[test]
    fn finds_a_focused_pid_in_nested_nodes() {
        let tree = serde_json::json!({
            "nodes": [{
                "focused": false,
                "nodes": [{"focused": true, "pid": 4242}]
            }]
        });
        assert_eq!(focused_pid_in_tree(&tree), Some(4242));
    }

    #[test]
    fn finds_a_focused_pid_in_floating_nodes() {
        let tree = serde_json::json!({
            "nodes": [],
            "floating_nodes": [{"focused": true, "pid": 31337}]
        });
        assert_eq!(focused_pid_in_tree(&tree), Some(31337));
    }
}
