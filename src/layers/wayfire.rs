use crate::recording::PathInputs;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, EvidenceTier, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_BINDINGS: usize = 4096;
const MAX_METADATA_BYTES: usize = 1024 * 1024;
const MAX_IPC_REPLY_BYTES: usize = 1024 * 1024;
const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_METADATA_DIR: &str = "/usr/share/wayfire/metadata";
const LOCAL_METADATA_DIR: &str = "/usr/local/share/wayfire/metadata";
/// Plugin name of the options compiled into the compositor itself. It is
/// always loaded and never appears in `core/plugins`.
const CORE: &str = "core";

/// Read-only Wayfire adapter.
///
/// Two sources, strongest first:
///
/// 1. The running compositor, through the `ipc-rules` plugin's
///    `wayfire/get-config-option` and `wayfire/configuration` methods. Every
///    activator option that the plugin metadata declares (and every entry of
///    the dynamic binding lists) is read as the compositor holds it. An
///    option counts only when its plugin is loaded, which is checked by
///    looking for the plugin's shared object in the compositor's memory map.
/// 2. The config file, with the plugin metadata defaults, when the IPC socket
///    cannot be reached. Without the running state every answer is tolerated
///    uncertainty, except that a key no file or default binds is a pass.
pub struct Wayfire;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    action: String,
    section: String,
}

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let config = load_file().map_err(|error| format!("Wayfire: {error}"))?;
    Ok(config
        .bindings
        .into_iter()
        .map(|binding| {
            let context: String = if binding.section.is_empty() {
                "wayfire binding".into()
            } else {
                format!("Wayfire [{}]", binding.section)
            };
            BindingRecord::new(
                "Wayfire",
                binding.combo.compact_display(),
                binding.action,
                "configured; runtime activation conditional",
            )
            .with_context(context)
        })
        .collect())
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    crate::recording::env_var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(crate::recording::env_var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("wayfire"))
        })
        || config_path().is_some_and(|path| path.recorded_is_file())
}

/// The config file is readable. This keeps the file-based meaning that the
/// status and capability reports have always used; the live query is only
/// used by [`Wayfire::inspect`].
pub fn ipc_available() -> bool {
    config_path().is_some_and(|path| path.recorded_is_file())
}

impl Wayfire {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        if let Some(socket) = live_socket() {
            let loaded = wayfire_pid().and_then(|pid| loaded_plugins(pid).ok());
            match inspect_live(&socket, key, loaded) {
                Ok(result) => return result,
                Err(error) => {
                    // The socket answered badly: fall back to the file, and
                    // say so.
                    let mut result = inspect_file(key);
                    result.details.push(format!(
                        "live query failed ({error}); answer from the config file"
                    ));
                    return result;
                }
            }
        }
        inspect_file(key)
    }
}

// ---------------------------------------------------------------------------
// Config file (fallback and inventory)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct FileConfig {
    bindings: Vec<Binding>,
    /// Activator values whose key names could not be mapped.
    unparsed: Vec<String>,
    /// `section/option` pairs the file sets, so metadata defaults they
    /// replace are ignored.
    declared: BTreeSet<String>,
}

fn load_file() -> Result<FileConfig, String> {
    let path = config_path().ok_or_else(|| "Wayfire config was not found".to_owned())?;
    let bytes =
        crate::recording::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_CONFIG_BYTES}-byte config limit",
            path.display()
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|error| format!("{} is not valid UTF-8: {error}", path.display()))?;
    let activators = metadata_activators(&metadata_decls_or_empty());
    Ok(parse_file(&content, &activators))
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = crate::recording::env_var_os("WAYFIRE_CONFIG_FILE") {
        return Some(PathBuf::from(path));
    }
    let base = crate::util::config_base()?;
    Some(base.join("wayfire.ini"))
}

fn config_description() -> String {
    config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Wayfire config".into())
}

/// Parse a wayfire.ini. A key is a binding when it starts with `binding_`
/// (or holds `_binding_`, as the dynamic lists' `release_binding_` do), or
/// when the plugin metadata declares it as an activator or key option of
/// that section (`[core] close_top_view = ...`).
fn parse_file(content: &str, activators: &BTreeSet<(String, String)>) -> FileConfig {
    let mut config = FileConfig::default();
    let mut section = String::new();
    for raw_line in content.lines() {
        let line = raw_line.split_once('#').map_or(raw_line, |(line, _)| line);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_owned();
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let Some(action) = binding_action(name, &section, activators) else {
            continue;
        };
        config.declared.insert(format!("{section}/{name}"));
        let activation = activation(value.trim());
        config.unparsed.extend(activation.unparsed);
        for combo in activation.combos {
            config.bindings.push(Binding {
                combo,
                action: action.clone(),
                section: section.clone(),
            });
            if config.bindings.len() >= MAX_BINDINGS {
                return config;
            }
        }
    }
    config
}

/// The action name of an option that is a binding, or `None`.
fn binding_action(
    name: &str,
    section: &str,
    activators: &BTreeSet<(String, String)>,
) -> Option<String> {
    if let Some(action) = name.strip_prefix("binding_") {
        return (!action.is_empty()).then(|| action.to_owned());
    }
    if let Some((_, action)) = name.split_once("_binding_") {
        return (!action.is_empty()).then(|| action.to_owned());
    }
    activators
        .contains(&(section.to_owned(), name.to_owned()))
        .then(|| name.to_owned())
}

/// Static answer from the file and the metadata defaults.
fn inspect_file(key: &KeyCombo) -> LayerResult {
    let config = match load_file() {
        Ok(config) => config,
        Err(error) => {
            return LayerResult::new(
                "Wayfire",
                LayerId::Compositor,
                Outcome::Unavailable,
                "could not inspect Wayfire shortcuts",
                vec![error],
            );
        }
    };
    let decls = metadata_decls_or_empty();
    let metadata_found = !decls.is_empty();
    let file_matches = config
        .bindings
        .iter()
        .filter(|binding| binding.combo == *key)
        .collect::<Vec<_>>();
    let default_matches = metadata_defaults(&decls, &config.declared)
        .into_iter()
        .filter(|default| default.combos.contains(key))
        .collect::<Vec<_>>();

    let mut details = vec![format!("source: {}", config_description())];
    for binding in &file_matches {
        details.push(format!("binding: {} [{}]", binding.action, binding.section));
    }
    for default in &default_matches {
        details.push(format!(
            "default binding: {} [{}] (metadata default, active when plugin {} is loaded)",
            default.option, default.plugin, default.plugin
        ));
    }
    details.extend(
        config
            .unparsed
            .iter()
            .map(|value| format!("not inspected: '{value}' has a key name whykey cannot map")),
    );
    details.push(
        "live state (plugin loading, runtime precedence) needs the Wayfire IPC socket, which was not reachable"
            .into(),
    );

    if !file_matches.is_empty() || !default_matches.is_empty() {
        return LayerResult::new(
            "Wayfire",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "Wayfire config contains a matching binding",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile);
    }
    if !config.unparsed.is_empty() {
        return LayerResult::new(
            "Wayfire",
            LayerId::Compositor,
            Outcome::Unknown,
            "a Wayfire binding could not be parsed",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile);
    }
    if !metadata_found {
        details
            .push("plugin metadata was not found, so default bindings were not inspected".into());
        return LayerResult::new(
            "Wayfire",
            LayerId::Compositor,
            Outcome::UncertainContinues,
            "no matching Wayfire binding found in the config file",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile);
    }
    LayerResult::new(
        "Wayfire",
        LayerId::Compositor,
        Outcome::Pass,
        "no matching Wayfire binding found",
        details,
    )
    .with_tier(EvidenceTier::ConfigFile)
}

// ---------------------------------------------------------------------------
// Plugin metadata: the option names and defaults Wayfire declares
// ---------------------------------------------------------------------------

/// One option that holds a key binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OptionDecl {
    plugin: String,
    name: String,
    /// For a dynamic list, the position of its activator entry among the
    /// list's entries; `None` for a plain activator or key option.
    list_entry: Option<usize>,
    default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefaultBinding {
    plugin: String,
    option: String,
    combos: Vec<KeyCombo>,
}

fn metadata_decls_or_empty() -> Vec<OptionDecl> {
    metadata_decls(&[
        PathBuf::from(DEFAULT_METADATA_DIR),
        PathBuf::from(LOCAL_METADATA_DIR),
    ])
    .unwrap_or_default()
}

/// Read every metadata file in the first directory that has any. `None`
/// when no directory is readable.
fn metadata_decls(dirs: &[PathBuf]) -> Option<Vec<OptionDecl>> {
    for dir in dirs {
        let Ok(entries) = crate::recording::read_dir(dir) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
            .collect::<Vec<_>>();
        paths.sort();
        let mut decls = Vec::new();
        let mut read_any = false;
        for path in paths {
            let Ok(text) = read_limited(&path, MAX_METADATA_BYTES) else {
                continue;
            };
            read_any = true;
            decls.extend(parse_metadata(&text));
        }
        if read_any {
            return Some(decls);
        }
    }
    None
}

fn read_limited(path: &Path, limit: usize) -> Result<String, String> {
    let bytes =
        crate::recording::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > limit {
        return Err(format!("{} exceeds {limit} bytes", path.display()));
    }
    String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// The binding options of a Wayfire metadata document: `<plugin>` blocks
/// holding `<option>` elements of type `activator` or `key`, and
/// `dynamic-list` options whose entries include such an activator.
fn parse_metadata(xml: &str) -> Vec<OptionDecl> {
    let mut decls = Vec::new();
    for plugin_block in blocks(xml, "<plugin ", "</plugin>") {
        let Some(plugin) = attr(first_tag(plugin_block), "name") else {
            continue;
        };
        for option_block in blocks(plugin_block, "<option ", "</option>") {
            let head = first_tag(option_block);
            let (Some(name), Some(kind)) = (attr(head, "name"), attr(head, "type")) else {
                continue;
            };
            match kind {
                "activator" | "key" => decls.push(OptionDecl {
                    plugin: plugin.to_owned(),
                    name: name.to_owned(),
                    list_entry: None,
                    default: default_text(option_block),
                }),
                "dynamic-list" => {
                    for (index, entry) in tags(option_block, "<entry").into_iter().enumerate() {
                        if matches!(attr(entry, "type"), Some("activator" | "key")) {
                            decls.push(OptionDecl {
                                plugin: plugin.to_owned(),
                                name: name.to_owned(),
                                list_entry: Some(index),
                                default: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    decls
}

fn metadata_activators(decls: &[OptionDecl]) -> BTreeSet<(String, String)> {
    decls
        .iter()
        .filter(|decl| decl.list_entry.is_none())
        .map(|decl| (decl.plugin.clone(), decl.name.clone()))
        .collect()
}

/// Metadata defaults that bind a key and that the file does not replace.
fn metadata_defaults(decls: &[OptionDecl], declared: &BTreeSet<String>) -> Vec<DefaultBinding> {
    decls
        .iter()
        .filter(|decl| decl.list_entry.is_none())
        .filter(|decl| !declared.contains(&format!("{}/{}", decl.plugin, decl.name)))
        .filter_map(|decl| {
            let activation = activation(decl.default.as_deref()?);
            (!activation.combos.is_empty()).then(|| DefaultBinding {
                plugin: decl.plugin.clone(),
                option: decl.name.clone(),
                combos: activation.combos,
            })
        })
        .collect()
}

fn blocks<'a>(text: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = &rest[start..];
        let Some(end) = after.find(close) else {
            break;
        };
        found.push(&after[..end + close.len()]);
        rest = &after[end + close.len()..];
    }
    found
}

fn tags<'a>(text: &'a str, open: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            break;
        };
        found.push(&after[..=end]);
        rest = &after[end + 1..];
    }
    found
}

fn first_tag(block: &str) -> &str {
    block.find('>').map_or(block, |end| &block[..=end])
}

/// The value of attribute `name` in a start tag. Only whole attribute names
/// match, so `name` does not match inside `prefix="name_"`.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let mut search = 0;
    while let Some(found) = tag[search..].find(&needle) {
        let at = search + found;
        let preceded_ok = tag[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
        if preceded_ok {
            let value_start = at + needle.len();
            let value_end = tag[value_start..].find('"')? + value_start;
            return Some(&tag[value_start..value_end]);
        }
        search = at + needle.len();
    }
    None
}

fn default_text(block: &str) -> Option<String> {
    let start = block.find("<default>")? + "<default>".len();
    let end = block[start..].find("</default>")? + start;
    let text = unescape(block[start..end].trim());
    (!text.is_empty()).then_some(text)
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

// ---------------------------------------------------------------------------
// Activator values
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Activation {
    combos: Vec<KeyCombo>,
    /// Alternatives whose key name whykey cannot map.
    unparsed: Vec<String>,
}

/// Parse an activator value such as `<super> KEY_T | <alt> KEY_F4`. `none`,
/// `disabled`, buttons and modifier-only activators bind no key.
fn activation(value: &str) -> Activation {
    let mut activation = Activation::default();
    let value = value.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("disabled")
    {
        return activation;
    }
    for alternative in value.split('|') {
        match alternative_combo(alternative) {
            Ok(Some(combo)) => activation.combos.push(combo),
            Ok(None) => {}
            Err(text) => activation.unparsed.push(text),
        }
    }
    activation
}

/// One `<mod> ... KEY` alternative. `Ok(None)` for a button or a
/// modifier-only alternative; `Err` with the text when the key name is not
/// one `KeyCombo` accepts.
fn alternative_combo(alternative: &str) -> Result<Option<KeyCombo>, String> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for raw in alternative.split_whitespace() {
        let token = raw.trim_matches(['<', '>']);
        if token.is_empty() {
            continue;
        }
        if token.starts_with("BTN_") {
            return Ok(None);
        }
        if let Some(modifier) = parse_modifier(token) {
            modifiers.push(modifier);
        } else if key.is_none() {
            key = Some(token);
        } else {
            return Err(alternative.trim().to_owned());
        }
    }
    let Some(key) = key else {
        return Ok(None);
    };
    let input = if modifiers.is_empty() {
        wayfire_key_name(key)
    } else {
        format!("{}+{}", modifiers.join("+"), wayfire_key_name(key))
    };
    KeyCombo::parse_query(&input)
        .map(Some)
        .map_err(|_| alternative.trim().to_owned())
}

/// Wayfire names keys by evdev code (`KEY_T`, `KEY_KP1`, `KEY_ESC`) or by
/// keysym name. Map the evdev names whose keysym spelling differs. Bare names
/// pass through and `KeyCombo` normalizes them.
fn wayfire_key_name(token: &str) -> String {
    let Some(name) = token
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("KEY_"))
        .and_then(|_| token.get(4..))
    else {
        return token.to_owned();
    };
    let upper = name.to_ascii_uppercase();
    let mapped = match upper.as_str() {
        "KPENTER" => "KP_Enter",
        "KPPLUS" => "KP_Add",
        "KPMINUS" => "KP_Subtract",
        "KPASTERISK" => "KP_Multiply",
        "KPSLASH" => "KP_Divide",
        "KPDOT" => "KP_Decimal",
        "DOT" => "period",
        "SLASH" => "slash",
        "BACKSLASH" => "backslash",
        "APOSTROPHE" => "apostrophe",
        "GRAVE" => "grave",
        "LEFTBRACE" => "bracketleft",
        "RIGHTBRACE" => "bracketright",
        "ENTER" => "Return",
        "ESC" => "Escape",
        "PAGEUP" => "Page_Up",
        "PAGEDOWN" => "Page_Down",
        _ => {
            return match upper.strip_prefix("KP") {
                Some(digit) if digit.len() == 1 && digit.bytes().all(|b| b.is_ascii_digit()) => {
                    format!("KP_{digit}")
                }
                _ => name.to_owned(),
            };
        }
    };
    mapped.to_owned()
}

fn parse_modifier(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "super" | "mod4" | "meta" | "win" => Some("super"),
        "ctrl" | "control" | "primary" => Some("ctrl"),
        "alt" | "mod1" => Some("alt"),
        "shift" => Some("shift"),
        "caps" | "capslock" => Some("caps"),
        "num" | "numlock" | "mod2" => Some("mod2"),
        "hyper" | "mod3" => Some("mod3"),
        "mod5" => Some("mod5"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Live state over the IPC socket
// ---------------------------------------------------------------------------

/// The IPC socket of a running Wayfire: `_WAYFIRE_SOCKET` when the session
/// exports it, otherwise the one `wayfire-*.socket` in `XDG_RUNTIME_DIR`
/// (preferring the one named after `WAYLAND_DISPLAY`).
fn live_socket() -> Option<PathBuf> {
    if let Some(path) =
        crate::recording::env_var_os("_WAYFIRE_SOCKET").filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    let runtime = crate::recording::env_var_os("XDG_RUNTIME_DIR")?;
    let mut sockets = crate::recording::read_dir(runtime)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("wayfire-") && name.ends_with(".socket"))
        })
        .collect::<Vec<_>>();
    sockets.sort();
    if let Ok(display) = crate::recording::env_var("WAYLAND_DISPLAY") {
        let matching = sockets
            .iter()
            .filter(|path| path.to_string_lossy().contains(display.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if matching.len() == 1 {
            return matching.into_iter().next();
        }
    }
    (sockets.len() == 1).then(|| sockets.remove(0))
}

/// One request: a native-endian `u32` length and the JSON, then the reply in
/// the same framing.
fn ipc_request(socket: &Path, method: &str, data: Value) -> Result<Value, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|error| format!("{}: {error}", socket.display()))?;
    stream
        .set_read_timeout(Some(IPC_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IPC_TIMEOUT)))
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_vec(&json!({"method": method, "data": data}))
        .map_err(|error| error.to_string())?;
    let length = u32::try_from(body.len()).map_err(|_| "request too large".to_owned())?;
    stream
        .write_all(&length.to_ne_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|error| format!("writing {method}: {error}"))?;
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("reading {method}: {error}"))?;
    let length = u32::from_ne_bytes(header) as usize;
    if length > MAX_IPC_REPLY_BYTES {
        return Err(format!("{method} reply is {length} bytes"));
    }
    let mut reply = vec![0_u8; length];
    stream
        .read_exact(&mut reply)
        .map_err(|error| format!("reading {method}: {error}"))?;
    serde_json::from_slice(&reply).map_err(|error| format!("{method} reply: {error}"))
}

/// `wayfire/get-config-option`: the live value, or `None` when Wayfire has no
/// such option.
fn live_option(socket: &Path, option: &str) -> Result<Option<Value>, String> {
    let reply = ipc_request(
        socket,
        "wayfire/get-config-option",
        json!({"option": option}),
    )?;
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        return if error == "Option not found!" {
            Ok(None)
        } else {
            Err(format!("{option}: {error}"))
        };
    }
    reply
        .get("value")
        .cloned()
        .map(Some)
        .ok_or_else(|| format!("{option}: reply has no value"))
}

/// Load state of a plugin, read from the compositor's memory map: a plugin is
/// loaded when its `lib<plugin>.so` is mapped into the Wayfire process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    Loaded,
    NotLoaded,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveHit {
    plugin: String,
    option: String,
    value: String,
    load: Load,
}

/// The live answer for `key`. `loaded` is the set of plugin names mapped into
/// the compositor, or `None` when that could not be read.
fn inspect_live(
    socket: &Path,
    key: &KeyCombo,
    loaded: Option<BTreeSet<String>>,
) -> Result<LayerResult, String> {
    let configuration = ipc_request(socket, "wayfire/configuration", json!({}))?;
    // The directory of the running compositor's metadata; the installed
    // directories only when it does not report one.
    let running_dir = configuration
        .get("plugin-xml-dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .and_then(|dir| metadata_decls(&[dir]));
    let Some(decls) = running_dir.or_else(|| {
        metadata_decls(&[
            PathBuf::from(DEFAULT_METADATA_DIR),
            PathBuf::from(LOCAL_METADATA_DIR),
        ])
    }) else {
        return Err("plugin metadata could not be read".into());
    };

    let plugins_value =
        live_option(socket, "core/plugins")?.ok_or_else(|| "core/plugins is not set".to_owned())?;
    let plugins = plugins_value
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let load_of = |plugin: &str| -> Load {
        if plugin == CORE {
            return Load::Loaded;
        }
        if !plugins.iter().any(|name| name == plugin) {
            return Load::NotLoaded;
        }
        match &loaded {
            Some(set) if set.contains(plugin) => Load::Loaded,
            Some(_) => Load::NotLoaded,
            None => Load::Unknown,
        }
    };

    let mut hits = Vec::new();
    let mut unparsed = Vec::new();
    let mut options_read = 0_usize;
    for decl in &decls {
        if decl.plugin != CORE && !plugins.contains(&decl.plugin) {
            // Not configured, so never loaded: its options cannot bind.
            continue;
        }
        let Some(value) = live_option(socket, &format!("{}/{}", decl.plugin, decl.name))? else {
            continue;
        };
        options_read += 1;
        match decl.list_entry {
            None => {
                let Some(text) = value.as_str() else {
                    unparsed.push(format!("{}/{}", decl.plugin, decl.name));
                    continue;
                };
                record_hit(
                    &mut hits,
                    &mut unparsed,
                    key,
                    text,
                    &decl.plugin,
                    &decl.name,
                    load_of(&decl.plugin),
                );
            }
            Some(index) => {
                let Some(rows) = value.as_array() else {
                    unparsed.push(format!("{}/{}", decl.plugin, decl.name));
                    continue;
                };
                for row in rows {
                    let cells = row.as_array();
                    let entry = cells
                        .and_then(|cells| cells.first())
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let text = cells
                        .and_then(|cells| cells.get(index + 1))
                        .and_then(Value::as_str);
                    match text {
                        Some(text) => record_hit(
                            &mut hits,
                            &mut unparsed,
                            key,
                            text,
                            &decl.plugin,
                            &format!("{}[{entry}]", decl.name),
                            load_of(&decl.plugin),
                        ),
                        None => unparsed.push(format!("{}/{}[{entry}]", decl.plugin, decl.name)),
                    }
                }
            }
        }
    }

    let mut details = vec![
        format!(
            "source: live options via wayfire/get-config-option ({})",
            socket.display()
        ),
        format!("configured plugins (core/plugins): {}", plugins.join(" ")),
        format!("options read: {options_read}"),
    ];
    match &loaded {
        Some(set) => details.push(format!(
            "plugin libraries mapped into the compositor: {}",
            set.iter().cloned().collect::<Vec<_>>().join(" ")
        )),
        None => details.push(
            "plugin loading not confirmed: the compositor's memory map is not readable by whykey"
                .into(),
        ),
    }
    for hit in &hits {
        details.push(format!(
            "binding: {} [{}/{}] ({})",
            hit.value,
            hit.plugin,
            hit.option,
            match hit.load {
                Load::Loaded => "plugin loaded",
                Load::NotLoaded => "plugin not loaded: inert",
                Load::Unknown => "plugin load state unknown",
            }
        ));
    }
    for text in &unparsed {
        details.push(format!(
            "not inspected: {text} has a key name whykey cannot map"
        ));
    }

    let active = hits.iter().filter(|hit| hit.load == Load::Loaded).count();
    let unknown = hits.iter().filter(|hit| hit.load == Load::Unknown).count();
    let (outcome, summary) = if active == 1 && unknown == 0 {
        (
            Outcome::Consumed,
            "active Wayfire binding found in the running config",
        )
    } else if active > 1 {
        (
            Outcome::HandledUncertain,
            "several active Wayfire bindings match this key in the running config",
        )
    } else if unknown > 0 {
        (
            Outcome::HandledUncertain,
            "a Wayfire binding matches this key, but its plugin's load state is unknown",
        )
    } else if !unparsed.is_empty() {
        (
            Outcome::Unknown,
            "a Wayfire binding could not be parsed, so no match can be ruled out",
        )
    } else {
        (
            Outcome::Pass,
            "no active Wayfire binding for this key in the running config",
        )
    };
    Ok(
        LayerResult::new("Wayfire", LayerId::Compositor, outcome, summary, details)
            .with_tier(EvidenceTier::LiveQuery),
    )
}

fn record_hit(
    hits: &mut Vec<LiveHit>,
    unparsed: &mut Vec<String>,
    key: &KeyCombo,
    text: &str,
    plugin: &str,
    option: &str,
    load: Load,
) {
    let activation = activation(text);
    unparsed.extend(
        activation
            .unparsed
            .iter()
            .map(|alternative| format!("{plugin}/{option} '{alternative}'")),
    );
    if activation.combos.contains(key) {
        hits.push(LiveHit {
            plugin: plugin.to_owned(),
            option: option.to_owned(),
            value: text.to_owned(),
            load,
        });
    }
}

/// The compositor process: the only `wayfire` process, when there is exactly
/// one.
fn wayfire_pid() -> Option<u32> {
    let mut found = Vec::new();
    for entry in crate::recording::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if crate::recording::read_to_string(entry.path().join("comm"))
            .is_ok_and(|comm| comm.trim_end() == "wayfire")
        {
            found.push(pid);
        }
    }
    (found.len() == 1).then(|| found[0])
}

/// Plugin names whose shared object is mapped into `pid`.
fn loaded_plugins(pid: u32) -> Result<BTreeSet<String>, String> {
    let maps = crate::recording::read_to_string(format!("/proc/{pid}/maps"))
        .map_err(|error| format!("/proc/{pid}/maps: {error}"))?;
    Ok(maps
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter_map(|path| path.rsplit('/').next())
        .filter_map(|name| name.strip_prefix("lib")?.strip_suffix(".so"))
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    #[test]
    fn parses_wayfire_shortcut_values() {
        let config = parse_file(
            "[command]\nbinding_terminal = <super> <shift> t\ncommand_terminal = foot\n[shortcuts]\nbinding_next_output = <alt> Right\n",
            &BTreeSet::new(),
        );
        assert_eq!(config.bindings.len(), 2);
        assert_eq!(config.bindings[0].combo, combo("super+shift+t"));
        assert_eq!(config.bindings[0].action, "terminal");
        assert_eq!(config.bindings[1].combo, combo("alt+right"));
    }

    #[test]
    fn disabled_and_modifier_only_values_bind_no_key() {
        assert!(activation("none").combos.is_empty());
        assert!(activation("disabled").combos.is_empty());
        assert!(activation("<super>").combos.is_empty());
        assert!(activation("BTN_LEFT").combos.is_empty());
        assert!(activation("<super> <alt>").combos.is_empty());
        assert_eq!(
            activation("<super> <ctrl> t").combos,
            vec![combo("super+ctrl+t")]
        );
    }

    #[test]
    fn every_alternative_of_a_value_binds() {
        let config = parse_file(
            "[core]\nbinding_next = <super> KEY_T | <ctrl> KEY_Y\n",
            &BTreeSet::new(),
        );
        assert_eq!(config.bindings.len(), 2);
        assert_eq!(config.bindings[0].combo, combo("super+t"));
        assert_eq!(config.bindings[1].combo, combo("ctrl+y"));
    }

    #[test]
    fn parses_wayfire_evdev_key_names() {
        let config = parse_file(
            "[command]\nbinding_terminal = <super> KEY_T\nbinding_kp = <ctrl> KEY_KP1\nbinding_enter = <super> KEY_KPENTER\n",
            &BTreeSet::new(),
        );
        assert_eq!(config.bindings.len(), 3);
        assert_eq!(config.bindings[0].combo, combo("super+t"));
        assert_eq!(config.bindings[1].combo, combo("ctrl+KP_1"));
        assert_eq!(config.bindings[2].combo, combo("super+KP_Enter"));
    }

    #[test]
    fn maps_evdev_names_whose_keysym_spelling_differs() {
        let cases = [
            ("<super> KEY_ESC", "super+escape"),
            ("<super> KEY_ENTER", "super+return"),
            ("<alt> KEY_TAB", "alt+tab"),
            ("<super> KEY_LEFT", "super+left"),
            ("<super> KEY_SPACE", "super+space"),
            ("<super> KEY_BACKSPACE", "super+backspace"),
            ("<super> KEY_PAGEUP", "super+page_up"),
            ("<super> KEY_DOT", "super+period"),
            ("<super> KEY_SLASH", "super+slash"),
            ("<super> KEY_LEFTBRACE", "super+bracketleft"),
            ("<super> KEY_F4", "super+f4"),
            ("<super> KEY_1", "super+1"),
            ("<super> KEY_KPPLUS", "super+KP_Add"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                activation(value).combos,
                vec![combo(expected)],
                "{value} should be {expected}"
            );
        }
    }

    #[test]
    fn unmappable_key_names_are_reported_not_dropped() {
        let activation = activation("<super> KEY_NOSUCHKEY");
        assert!(activation.combos.is_empty());
        assert_eq!(activation.unparsed.len(), 1);
    }

    #[test]
    fn metadata_declares_static_and_dynamic_binding_options() {
        let xml = r#"<wayfire>
  <plugin name="command">
    <option name="bindings" type="dynamic-list" type-hint="dict">
      <entry prefix="command_" type="string" name="command"/>
      <entry prefix="binding_" type="activator" name="binding"/>
    </option>
    <option name="flag" type="bool"><default>true</default></option>
  </plugin>
  <plugin name="core">
    <option name="close_top_view" type="activator">
      <default>&lt;super&gt; KEY_Q | &lt;alt&gt; KEY_F4</default>
    </option>
    <option name="focus_buttons" type="activator">
      <default>BTN_LEFT | BTN_MIDDLE</default>
    </option>
  </plugin>
</wayfire>"#;
        let decls = parse_metadata(xml);
        assert_eq!(
            decls,
            vec![
                OptionDecl {
                    plugin: "command".into(),
                    name: "bindings".into(),
                    list_entry: Some(1),
                    default: None,
                },
                OptionDecl {
                    plugin: "core".into(),
                    name: "close_top_view".into(),
                    list_entry: None,
                    default: Some("<super> KEY_Q | <alt> KEY_F4".into()),
                },
                OptionDecl {
                    plugin: "core".into(),
                    name: "focus_buttons".into(),
                    list_entry: None,
                    default: Some("BTN_LEFT | BTN_MIDDLE".into()),
                },
            ]
        );
        let defaults = metadata_defaults(&decls, &BTreeSet::new());
        assert_eq!(defaults.len(), 1, "button defaults bind no key");
        assert!(defaults[0].combos.contains(&combo("super+q")));
        assert!(defaults[0].combos.contains(&combo("alt+F4")));
        let overridden =
            metadata_defaults(&decls, &BTreeSet::from(["core/close_top_view".to_owned()]));
        assert!(overridden.is_empty(), "a file setting replaces the default");
    }

    #[test]
    fn file_sets_metadata_activators_outside_binding_prefix() {
        let activators = BTreeSet::from([("core".to_owned(), "close_top_view".to_owned())]);
        let config = parse_file("[core]\nclose_top_view = none\n", &activators);
        assert!(config.bindings.is_empty());
        assert!(config.declared.contains("core/close_top_view"));
        let config = parse_file("[core]\nclose_top_view = <super> KEY_X\n", &activators);
        assert_eq!(config.bindings[0].action, "close_top_view");
        assert_eq!(config.bindings[0].combo, combo("super+x"));
    }

    #[test]
    fn xml_attributes_match_whole_names_only() {
        let tag = r#"<entry prefix="name_" type="activator" name="binding">"#;
        assert_eq!(attr(tag, "name"), Some("binding"));
        assert_eq!(attr(tag, "prefix"), Some("name_"));
        assert_eq!(attr(tag, "missing"), None);
    }

    #[test]
    fn rejects_wayfire_dynamic_or_disabled_values() {
        assert!(activation("none").combos.is_empty());
        assert!(activation("<super> {key}").combos.is_empty());
        assert_eq!(
            activation("<super> <ctrl> t").combos,
            vec![combo("super+ctrl+t")]
        );
    }

    /// Option answer as `wayfire/get-config-option` gives it.
    fn option(value: Value) -> Value {
        json!({"result": "ok", "value": value})
    }

    /// A stand-in for the compositor's IPC socket. It answers each request
    /// from `answers`: keyed by method, or by `option <name>` for
    /// `wayfire/get-config-option`; an absent key is "Option not found!".
    fn fake_compositor(answers: Vec<(&'static str, Value)>, dir: &Path) -> PathBuf {
        use std::collections::HashMap;
        use std::os::unix::net::UnixListener;

        fs::create_dir_all(dir).unwrap();
        let socket = dir.join("wayfire-test.socket");
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let answers: HashMap<String, Value> = answers
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut header = [0_u8; 4];
                let mut body = Vec::new();
                if stream.read_exact(&mut header).is_err() {
                    continue;
                }
                body.resize(u32::from_ne_bytes(header) as usize, 0);
                if stream.read_exact(&mut body).is_err() {
                    continue;
                }
                let request: Value = serde_json::from_slice(&body).unwrap();
                let method = request["method"].as_str().unwrap_or_default();
                let key = if method == "wayfire/get-config-option" {
                    format!(
                        "option {}",
                        request["data"]["option"].as_str().unwrap_or_default()
                    )
                } else {
                    method.to_owned()
                };
                let reply = answers
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| json!({"error": "Option not found!"}));
                let text = serde_json::to_vec(&reply).unwrap();
                let _ = stream.write_all(&(text.len() as u32).to_ne_bytes());
                let _ = stream.write_all(&text);
            }
        });
        socket
    }

    /// Metadata of a compositor with `core` and `command` (plus `vswitch`,
    /// which the test configuration does not load).
    fn test_metadata(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("core.xml"),
            r#"<wayfire><plugin name="core">
                 <option name="close_top_view" type="activator">
                   <default>&lt;super&gt; KEY_Q | &lt;alt&gt; KEY_F4</default>
                 </option>
               </plugin></wayfire>"#,
        )
        .unwrap();
        fs::write(
            dir.join("command.xml"),
            r#"<wayfire><plugin name="command">
                 <option name="bindings" type="dynamic-list">
                   <entry prefix="command_" type="string" name="command"/>
                   <entry prefix="binding_" type="activator" name="binding"/>
                 </option>
               </plugin></wayfire>"#,
        )
        .unwrap();
        fs::write(
            dir.join("vswitch.xml"),
            r#"<wayfire><plugin name="vswitch">
                 <option name="binding_left" type="activator">
                   <default>&lt;super&gt; KEY_LEFT</default>
                 </option>
               </plugin></wayfire>"#,
        )
        .unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "whykey-wayfire-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    /// The answers of a compositor that loads `core` and `command`, binds
    /// Super+T in `command` and keeps the core default close binding.
    fn running_compositor(metadata: &Path, plugins: &str) -> Vec<(&'static str, Value)> {
        vec![
            (
                "wayfire/configuration",
                json!({"api-version": 1, "plugin-xml-dir": metadata.display().to_string()}),
            ),
            ("option core/plugins", option(json!(plugins))),
            (
                "option command/bindings",
                option(json!([["terminal", "touch /tmp/fired", "<super> KEY_T"]])),
            ),
            (
                "option core/close_top_view",
                option(json!("<super> KEY_Q | <alt> KEY_F4")),
            ),
            (
                "option vswitch/binding_left",
                option(json!("<super> KEY_LEFT")),
            ),
        ]
    }

    fn live(socket: &Path, key: &str, loaded: Option<&[&str]>) -> LayerResult {
        let loaded = loaded.map(|names| names.iter().map(|name| (*name).to_owned()).collect());
        inspect_live(socket, &combo(key), loaded).unwrap()
    }

    #[test]
    fn live_consumes_a_key_bound_by_a_loaded_default() {
        let root = scratch("core-default");
        let metadata = root.join("metadata");
        test_metadata(&metadata);
        let socket = fake_compositor(
            running_compositor(&metadata, "ipc ipc-rules command"),
            &root,
        );

        let result = live(&socket, "super+q", Some(&[]));
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("core/close_top_view"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_binding_of_a_plugin_is_active_only_when_it_is_mapped() {
        let root = scratch("plugin-load");
        let metadata = root.join("metadata");
        test_metadata(&metadata);
        let socket = fake_compositor(
            running_compositor(&metadata, "ipc ipc-rules command"),
            &root,
        );

        assert_eq!(
            live(&socket, "super+t", Some(&["command"])).outcome,
            Outcome::Consumed
        );
        assert_eq!(live(&socket, "super+t", Some(&[])).outcome, Outcome::Pass);
        assert_eq!(
            live(&socket, "super+t", None).outcome,
            Outcome::HandledUncertain
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_plugin_not_in_core_plugins_binds_nothing() {
        let root = scratch("not-configured");
        let metadata = root.join("metadata");
        test_metadata(&metadata);
        // `command` is mapped in the memory map but not configured, so the
        // answer must not count it.
        let socket = fake_compositor(running_compositor(&metadata, "ipc ipc-rules"), &root);

        let result = live(&socket, "super+t", Some(&["command"]));
        assert_eq!(result.outcome, Outcome::Pass);
        // An unconfigured plugin's default (vswitch) binds nothing either.
        assert_eq!(
            live(&socket, "super+left", Some(&["command"])).outcome,
            Outcome::Pass
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_disabled_default_passes_and_several_active_bindings_are_uncertain() {
        let root = scratch("disabled-default");
        let metadata = root.join("metadata");
        test_metadata(&metadata);
        let mut answers = running_compositor(&metadata, "ipc ipc-rules command");
        answers.retain(|(key, _)| *key != "option core/close_top_view");
        answers.push(("option core/close_top_view", option(json!("none"))));
        let socket = fake_compositor(answers, &root);
        assert_eq!(live(&socket, "super+q", Some(&[])).outcome, Outcome::Pass);

        let mut answers = running_compositor(&metadata, "ipc ipc-rules command");
        answers.retain(|(key, _)| *key != "option command/bindings");
        answers.push((
            "option command/bindings",
            option(json!([["quit", "pkill wayfire", "<super> KEY_Q"]])),
        ));
        let socket = fake_compositor(answers, &root.join("second"));
        let result = live(&socket, "super+q", Some(&["command"]));
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        let _ = fs::remove_dir_all(root);
    }
}
