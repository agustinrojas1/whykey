//! GNOME global shortcuts from live GSettings.
//!
//! mutter, GNOME Shell and gnome-settings-daemon read their keybindings from
//! GSettings and rebind as soon as a value changes, so `gsettings
//! list-recursively` reports the configuration the running session uses.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::command;
use crate::key::{KeyCombo, modifier_mask};
use crate::layers::gsettings::quoted_values;
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, Checked, EvidenceTier, LayerId, LayerResult,
    Outcome, SourceLocation,
};

const LAYER: &str = "GNOME";
const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";

/// Well-known bus name of gnome-settings-daemon's media-keys plugin, which
/// grabs the custom shortcuts. The lab's `gnome-shell` scenarios show that a
/// configured custom shortcut is not grabbed while this name has no owner.
const MEDIA_KEYS_OWNER: &str = "org.gnome.SettingsDaemon.MediaKeys";
/// Bus name GNOME Shell owns; it grabs the `org.gnome.shell.keybindings` keys.
const SHELL_OWNER: &str = "org.gnome.Shell";
/// GNOME Shell's own settings: which extensions are enabled.
const SHELL_SETTINGS: &str = "org.gnome.shell";
/// Prefix of the component name of a binding an extension registered.
const EXTENSION_COMPONENT: &str = "GNOME Shell extension";
/// The xdg-desktop-portal GlobalShortcuts API only lets an application
/// register its own shortcuts; no call lists them.
const PORTAL_NOTE: &str = "shortcuts applications register through the xdg-desktop-portal GlobalShortcuts API cannot be listed, so they are not checked";

/// A keybinding schema, the component that grabs its keys, and the bus name
/// that component owns while it runs (`None` for mutter, which is the
/// compositor this layer only runs under).
struct Source {
    schema: &'static str,
    component: &'static str,
    owner: Option<&'static str>,
}

/// Keybinding schemas and the component that grabs their keys.
const SCHEMAS: &[Source] = &[
    Source {
        schema: "org.gnome.desktop.wm.keybindings",
        component: "mutter",
        owner: None,
    },
    Source {
        schema: "org.gnome.mutter.keybindings",
        component: "mutter",
        owner: None,
    },
    Source {
        schema: "org.gnome.mutter.wayland.keybindings",
        component: "mutter, Wayland sessions",
        owner: None,
    },
    Source {
        schema: "org.gnome.shell.keybindings",
        component: "GNOME Shell",
        owner: Some(SHELL_OWNER),
    },
    Source {
        schema: MEDIA_KEYS_SCHEMA,
        component: "gnome-settings-daemon",
        owner: Some(MEDIA_KEYS_OWNER),
    },
];

/// XKB keycode of the key above Tab (`TLDE`) on evdev keyboards; mutter
/// resolves its `Above_Tab` pseudo-keysym to this key.
const ABOVE_TAB_KEYCODE: u32 = 49;

/// Read-only GNOME global shortcut adapter.
pub struct Gnome;

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
                .any(|name| name.trim().eq_ignore_ascii_case("gnome"))
        })
        || crate::recording::env_var_os("GNOME_DESKTOP_SESSION_ID").is_some()
}

pub fn ipc_available() -> bool {
    runtime_reachable().is_ok()
}

/// Whether GSettings serves the session's keybindings; the error explains
/// why not.
pub(crate) fn runtime_reachable() -> Result<(), String> {
    match list_recursively(SCHEMAS[0].schema) {
        Ok(_) => Ok(()),
        Err(ListError::NoSchema) => Err(format!("schema {} is not installed", SCHEMAS[0].schema)),
        Err(ListError::Failed(error)) => Err(error),
    }
}

/// Return GNOME bindings as normalized values for the global inventory.
pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let loaded = load().map_err(|error| format!("GNOME: {error}"))?;
    Ok(loaded
        .bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "GNOME",
                binding.combo.compact_display(),
                binding.command.map_or_else(
                    || binding.action.clone(),
                    |command| format!("{}: {command}", binding.action),
                ),
                "live gsettings",
            )
            .with_context(format!("{} ({})", binding.schema, binding.component))
        })
        .collect())
}

impl Gnome {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        if key.is_alone() {
            return inspect_alone(key, read_overlay_key());
        }
        inspect(load(), key, owner_running)
    }
}

/// `org.gnome.mutter overlay-key`: the modifier that opens the Activities
/// overview when it is pressed and released on its own. `Ok(None)` when the
/// schema is not installed.
fn read_overlay_key() -> Result<Option<String>, String> {
    let mut gsettings = Command::new("gsettings");
    gsettings.args(["get", "org.gnome.mutter", "overlay-key"]);
    let output = command::output(&mut gsettings).map_err(|error| format!("gsettings: {error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if stderr.contains("No such schema") || stderr.contains("No such key") {
            return Ok(None);
        }
        return Err(super::gsettings::command_error(
            "gsettings",
            output.status,
            &output.stderr,
        ));
    }
    if stderr.contains("'memory' GSettings backend") {
        return Err(
            "gsettings fell back to the memory backend (dconf unreachable), so it reports schema defaults, not the session's overlay key".into(),
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(quoted_values(&text).into_iter().next())
}

/// A lone modifier on GNOME: mutter's `overlay-key` is the only gesture of
/// this kind, and it is read live from GSettings.
fn inspect_alone(key: &KeyCombo, overlay: Result<Option<String>, String>) -> LayerResult {
    let shown = crate::style::human_key(key);
    let overlay = match overlay {
        Ok(overlay) => overlay,
        Err(error) => {
            return LayerResult::unavailable(
                LAYER,
                LayerId::Compositor,
                "could not read GNOME's overlay-key from GSettings",
                vec![error],
            );
        }
    };
    let mut details =
        vec!["source: live GSettings (`gsettings get org.gnome.mutter overlay-key`)".to_owned()];
    let Some(overlay) = overlay else {
        details.push("the org.gnome.mutter schema or its overlay-key is not installed".into());
        return LayerResult::new(
            LAYER,
            LayerId::Compositor,
            Outcome::Unknown,
            "GNOME's overlay-key could not be read",
            details,
        );
    };
    let names = key.alone_keysyms();
    let bound = !overlay.is_empty()
        && overlay != "disabled"
        && names.contains(&crate::key::normalize_key_str(&overlay).as_str());
    if !bound {
        details.push(if overlay.is_empty() || overlay == "disabled" {
            "overlay-key is disabled".to_owned()
        } else {
            format!("overlay-key is '{overlay}'")
        });
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            format!("no GNOME lone-key shortcut uses {shown}"),
            details,
            Checked::new(
                EvidenceTier::LiveQuery,
                ["gsettings org.gnome.mutter overlay-key"],
            ),
        );
    }
    details.push(format!(
        "binding: org.gnome.mutter overlay-key = '{overlay}' (mutter opens the Activities overview when it is pressed and released alone)"
    ));
    details.push(
        "mutter acts on the release when no other key was pressed in between; whether the press itself also reaches the focused window is not verified"
            .into(),
    );
    let evidence = BindingEvidence::action(
        "open the Activities overview".to_owned(),
        BindingScope::Universal,
        Some(SourceLocation::new("gsettings org.gnome.mutter", None)),
    )
    .with_dispatcher("mutter")
    .with_description("overlay-key".to_owned());
    LayerResult::new(
        LAYER,
        LayerId::Compositor,
        Outcome::HandledUncertain,
        format!("GNOME binds {shown} to the Activities overview on release"),
        details,
    )
    .with_binding(evidence)
    .with_tier(EvidenceTier::LiveQuery)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    /// Accelerator as stored in GSettings.
    accelerator: String,
    schema: String,
    component: String,
    /// Bus name whose owner grabs this key, when the component is a daemon.
    owner: Option<&'static str>,
    action: String,
    command: Option<String>,
    /// Registered by a GNOME Shell extension through its own settings. The
    /// extension's code decides when it grabs the key, so a match is not
    /// proof the key is consumed.
    extension: bool,
}

#[derive(Debug, Default)]
struct Loaded {
    bindings: Vec<Binding>,
    /// Facts about bindings that could not be compared or schemas that are
    /// not installed.
    notes: Vec<String>,
    /// Enabled GNOME Shell extensions whose keybindings could not be read;
    /// while any exist, "no binding matched" is not an answer.
    unread_extensions: Vec<String>,
    /// Enabled extensions whose settings were read, for the details.
    read_extensions: Vec<String>,
}

/// What a GNOME pass rests on: every keybinding schema mutter, GNOME Shell
/// and gnome-settings-daemon read, and the settings of each enabled
/// extension that registers keys.
fn gnome_checked(read_extensions: &[String]) -> Checked {
    Checked::new(
        EvidenceTier::LiveQuery,
        SCHEMAS
            .iter()
            .map(|source| format!("gsettings {}", source.schema))
            .chain(
                read_extensions
                    .iter()
                    .map(|extension| format!("GNOME Shell extension {extension}")),
            ),
    )
}

fn inspect(
    loaded: Result<Loaded, String>,
    key: &KeyCombo,
    owner_running: impl Fn(&str) -> Result<bool, String>,
) -> LayerResult {
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            return LayerResult::unavailable(
                LAYER,
                LayerId::Compositor,
                "could not read GNOME keybindings from GSettings",
                vec![error],
            );
        }
    };
    let mut details = vec![
        "source: live GSettings (`gsettings list-recursively`), read by mutter, GNOME Shell and gnome-settings-daemon".to_owned(),
    ];
    let matches = loaded
        .bindings
        .iter()
        .filter(|binding| binding.combo == *key)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        if !loaded.read_extensions.is_empty() {
            details.push(format!(
                "GNOME Shell extensions read: {}",
                loaded.read_extensions.join(", ")
            ));
        }
        details.extend(loaded.notes);
        details.push(PORTAL_NOTE.into());
        if !loaded.unread_extensions.is_empty() {
            details.extend(loaded.unread_extensions);
            details.push(
                "extensions register keys through their own settings or code, so an unread extension may still take this key"
                    .into(),
            );
            return LayerResult::uncertain_continues(
                LAYER,
                LayerId::Compositor,
                "no readable GNOME global shortcut uses this key; some enabled extensions could not be read",
                details,
            )
            .with_tier(EvidenceTier::LiveQuery);
        }
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            "no GNOME global shortcut uses this key",
            details,
            gnome_checked(&loaded.read_extensions),
        );
    }
    // A daemon-owned binding grabs the key only while its daemon owns its bus
    // name; a configured shortcut whose daemon is not running does nothing.
    let mut grabbed = Vec::new();
    let mut idle_owners = Vec::new();
    for binding in &matches {
        let Some(owner) = binding.owner else {
            grabbed.push(*binding);
            continue;
        };
        match owner_running(owner) {
            Ok(true) => grabbed.push(*binding),
            Ok(false) => {
                if !idle_owners.contains(&owner) {
                    idle_owners.push(owner);
                }
            }
            Err(error) => {
                details.push(format!(
                    "could not check whether {owner} runs ({error}); assuming it grabs the key"
                ));
                grabbed.push(*binding);
            }
        }
    }
    for binding in &matches {
        let idle = binding
            .owner
            .filter(|owner| idle_owners.contains(owner))
            .map(|owner| format!(" — {owner} is not running, so nothing grabs this key"))
            .unwrap_or_default();
        details.push(format!(
            "binding: {} {} = '{}' ({}){}{}",
            binding.schema,
            binding.action,
            binding.accelerator,
            binding.component,
            binding
                .command
                .as_deref()
                .map(|command| format!(" — command: {command}"))
                .unwrap_or_default(),
            idle
        ));
        if binding.accelerator.contains("Above_Tab") {
            details.push(
                "Above_Tab is the key above Tab, whatever symbol the layout puts there".into(),
            );
        }
    }
    let Some(first) = grabbed
        .iter()
        .find(|binding| !binding.extension)
        .or_else(|| grabbed.first())
    else {
        details.extend(loaded.notes);
        if !loaded.unread_extensions.is_empty() {
            details.extend(loaded.unread_extensions);
            return LayerResult::uncertain_continues(
                LAYER,
                LayerId::Compositor,
                "GNOME shortcut is configured but its daemon is not running; some enabled extensions could not be read",
                details,
            )
            .with_tier(EvidenceTier::LiveQuery);
        }
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            "GNOME shortcut is configured but its daemon is not running",
            details,
            gnome_checked(&loaded.read_extensions),
        );
    };
    details.push(
        "not active while the focused window holds a keyboard-shortcuts inhibitor that the user allowed"
            .into(),
    );
    // An extension grabs its keys from its own code: it may register them
    // only when one of its options is on, or only in some shell modes.
    let (outcome, summary) = if first.extension {
        details.push(
            "the extension decides when it grabs this key (its own options and the shell mode); whykey cannot read that"
                .into(),
        );
        (
            Outcome::HandledUncertain,
            "a GNOME Shell extension binds this key",
        )
    } else {
        (Outcome::Consumed, "GNOME global shortcut consumes the key")
    };
    details.extend(loaded.notes);
    details.extend(loaded.unread_extensions);
    let evidence = BindingEvidence::action(
        first
            .command
            .clone()
            .unwrap_or_else(|| first.action.clone()),
        BindingScope::Universal,
        Some(SourceLocation::new(
            format!("gsettings {}", first.schema),
            None,
        )),
    )
    .with_dispatcher(first.component.clone())
    .with_description(first.action.clone());
    LayerResult::new(LAYER, LayerId::Compositor, outcome, summary, details)
        .with_binding(evidence)
        .with_tier(EvidenceTier::LiveQuery)
}

/// Whether a bus name has an owner on the session bus.
fn owner_running(name: &str) -> Result<bool, String> {
    let mut gdbus = Command::new("gdbus");
    gdbus.args([
        "call",
        "--session",
        "--dest",
        "org.freedesktop.DBus",
        "--object-path",
        "/org/freedesktop/DBus",
        "--method",
        "org.freedesktop.DBus.NameHasOwner",
        name,
    ]);
    let output = command::output(&mut gdbus).map_err(|error| format!("gdbus: {error}"))?;
    if !output.status.success() {
        return Err(super::gsettings::command_error(
            "gdbus",
            output.status,
            &output.stderr,
        ));
    }
    let reply = String::from_utf8_lossy(&output.stdout);
    match reply.trim() {
        "(true,)" => Ok(true),
        "(false,)" => Ok(false),
        other => Err(format!("unexpected NameHasOwner reply: {other}")),
    }
}

fn load() -> Result<Loaded, String> {
    let mut loaded = Loaded::default();
    let mut custom_paths = Vec::new();
    let mut read_any = false;
    for source in SCHEMAS {
        let (schema, component, owner) = (source.schema, source.component, source.owner);
        let output = match list_recursively(schema) {
            Ok(output) => output,
            Err(ListError::NoSchema) => {
                loaded.notes.push(format!(
                    "schema {schema} is not installed, so {component} defines none of its keybindings"
                ));
                continue;
            }
            Err(ListError::Failed(error)) => return Err(error),
        };
        read_any = true;
        for (key, value) in schema_entries(&output, schema) {
            if schema == MEDIA_KEYS_SCHEMA && key == "custom-keybindings" {
                custom_paths.extend(
                    string_values(value)
                        .into_iter()
                        .filter(|path| path.starts_with('/')),
                );
                continue;
            }
            for accelerator in string_values(value) {
                push_accelerator(
                    &mut loaded,
                    accelerator,
                    schema.to_string(),
                    component,
                    owner,
                    key.to_owned(),
                    None,
                );
            }
        }
    }
    if !read_any {
        return Err("none of the GNOME keybinding schemas is installed".into());
    }
    for path in custom_paths {
        let schema = format!("{CUSTOM_SCHEMA}:{path}");
        let output = match list_recursively(&schema) {
            Ok(output) => output,
            Err(ListError::NoSchema) => {
                loaded
                    .notes
                    .push(format!("custom keybinding {path} could not be read"));
                continue;
            }
            Err(ListError::Failed(error)) => return Err(error),
        };
        let mut binding = None;
        let mut name = None;
        let mut command = None;
        for (key, value) in schema_entries(&output, CUSTOM_SCHEMA) {
            let value = string_values(value).into_iter().next();
            match key {
                "binding" => binding = value,
                "name" => name = value.filter(|value| !value.is_empty()),
                "command" => command = value.filter(|value| !value.is_empty()),
                _ => {}
            }
        }
        if let Some(binding) = binding {
            push_accelerator(
                &mut loaded,
                binding,
                schema,
                "gnome-settings-daemon custom shortcut",
                Some(MEDIA_KEYS_OWNER),
                name.unwrap_or_else(|| "custom shortcut".into()),
                command,
            );
        }
    }
    load_extensions(&mut loaded);
    Ok(loaded)
}

fn push_accelerator(
    loaded: &mut Loaded,
    accelerator: String,
    schema: String,
    component: impl Into<String>,
    owner: Option<&'static str>,
    action: String,
    command: Option<String>,
) {
    push_binding(
        loaded,
        accelerator,
        schema,
        component.into(),
        owner,
        action,
        command,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_binding(
    loaded: &mut Loaded,
    accelerator: String,
    schema: String,
    component: String,
    owner: Option<&'static str>,
    action: String,
    command: Option<String>,
    extension: bool,
) {
    let combos = match parse_accelerator(&accelerator) {
        Accelerator::Disabled => return,
        Accelerator::Unmappable(reason) => {
            loaded.notes.push(format!(
                "not compared: {schema} {action} = '{accelerator}' ({reason})"
            ));
            return;
        }
        Accelerator::Combos(combos) => combos,
    };
    for combo in combos {
        loaded.bindings.push(Binding {
            combo,
            accelerator: accelerator.clone(),
            schema: schema.clone(),
            component: component.clone(),
            owner,
            action: action.clone(),
            command: command.clone(),
            extension,
        });
    }
}

/// `key value` pairs of one schema in `gsettings list-recursively` output.
fn schema_entries<'a>(
    output: &'a str,
    schema: &'a str,
) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
    output.lines().filter_map(move |line| {
        let rest = line.strip_prefix(schema)?.strip_prefix(' ')?;
        let (key, value) = rest.split_once(' ')?;
        Some((key, value))
    })
}

/// Strings of a GVariant `s` or `as` value in GSettings text form; values
/// of any other type (`volume-step 6`) yield nothing.
fn string_values(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value.strip_prefix("@as").map_or(value, str::trim_start);
    if value.starts_with(['\'', '"', '[']) {
        quoted_values(value)
    } else {
        Vec::new()
    }
}

enum ListError {
    NoSchema,
    Failed(String),
}

fn list_recursively(schema: &str) -> Result<String, ListError> {
    list_recursively_in(schema, None)
}

/// `gsettings list-recursively`, reading `schema` from `schema_dir` (an
/// extension's compiled schemas) as well as the installed ones when given.
fn list_recursively_in(schema: &str, schema_dir: Option<&Path>) -> Result<String, ListError> {
    let mut gsettings = Command::new("gsettings");
    if let Some(dir) = schema_dir {
        gsettings.arg("--schemadir").arg(dir);
    }
    gsettings.args(["list-recursively", schema]);
    let output = command::output(&mut gsettings)
        .map_err(|error| ListError::Failed(format!("gsettings: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if stderr.contains("No such schema") || stderr.contains("No such path") {
            return Err(ListError::NoSchema);
        }
        return Err(ListError::Failed(super::gsettings::command_error(
            "gsettings",
            output.status,
            &output.stderr,
        )));
    }
    // Without dconf GLib falls back to an in-memory backend that only knows
    // schema defaults, not what the session uses.
    if stderr.contains("'memory' GSettings backend") {
        return Err(ListError::Failed(
            "gsettings fell back to the memory backend (dconf unreachable), so it reports schema defaults, not the session's keybindings".into(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| ListError::Failed(format!("gsettings returned invalid UTF-8: {error}")))
}

/// A GNOME Shell extension that is enabled and installed.
#[derive(Debug, PartialEq, Eq)]
struct Extension {
    uuid: String,
    name: String,
    /// `settings-schema` of its `metadata.json`.
    schema: Option<String>,
    /// Directory holding compiled schemas, when the extension ships them.
    schema_dir: Option<PathBuf>,
    /// Installed under the user's data directory, which
    /// `disable-user-extensions` switches off.
    user: bool,
}

/// Directories GNOME Shell searches for extensions, user directory first,
/// each with whether it is the user's.
fn extension_roots() -> Vec<(PathBuf, bool)> {
    let mut roots = Vec::new();
    if let Some(base) = crate::util::data_base() {
        roots.push((base.join("gnome-shell/extensions"), true));
    }
    let dirs = crate::recording::env_var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in dirs.split(':').filter(|dir| !dir.is_empty()) {
        roots.push((Path::new(dir).join("gnome-shell/extensions"), false));
    }
    roots
}

/// Find an installed extension by UUID in the first root that has it.
fn find_extension(uuid: &str, roots: &[(PathBuf, bool)]) -> Option<Extension> {
    for (root, user) in roots {
        let dir = root.join(uuid);
        let Ok(metadata) = crate::recording::read_to_string(dir.join("metadata.json")) else {
            continue;
        };
        let metadata = serde_json::from_str::<serde_json::Value>(&metadata).ok();
        let field = |name: &str| {
            metadata
                .as_ref()
                .and_then(|value| value.get(name))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let compiled = dir.join("schemas");
        let has_schemas = crate::recording::metadata(compiled.join("gschemas.compiled")).is_ok();
        return Some(Extension {
            uuid: uuid.to_owned(),
            name: field("name").unwrap_or_else(|| uuid.to_owned()),
            schema: field("settings-schema"),
            schema_dir: has_schemas.then_some(compiled),
            user: *user,
        });
    }
    None
}

/// Strings of a GVariant `as` value (`['a', 'b']` or `@as []`); `None` when
/// the text is not an array of strings.
fn string_array(value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    let value = value.strip_prefix("@as").map_or(value, str::trim_start);
    (value.starts_with('[') && value.ends_with(']')).then(|| quoted_values(value))
}

/// Whether a string reads like a keybinding accelerator rather than any
/// other string in an `as` setting (an app list, a layout name).
fn looks_like_accelerator(value: &str) -> bool {
    value.contains('<')
        || value.starts_with("XF86")
        || value.strip_prefix('F').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Add the keybindings in `output` (an extension schema's
/// `list-recursively`) to `loaded`: every `as` key holding accelerators.
fn add_extension_bindings(loaded: &mut Loaded, extension: &Extension, schema: &str, output: &str) {
    let component = format!("{EXTENSION_COMPONENT} {}", extension.name);
    for (key, value) in schema_entries(output, schema) {
        let Some(values) = string_array(value) else {
            continue;
        };
        for accelerator in values
            .into_iter()
            .filter(|value| looks_like_accelerator(value))
        {
            push_binding(
                loaded,
                accelerator,
                schema.to_owned(),
                component.clone(),
                Some(SHELL_OWNER),
                key.to_owned(),
                None,
                true,
            );
        }
    }
}

/// Read the keybindings of the enabled GNOME Shell extensions. Extensions
/// register keys through `Main.wm.addKeybinding` from their own schemas,
/// which none of [`SCHEMAS`] covers.
fn load_extensions(loaded: &mut Loaded) {
    let enabled = match gsettings_get(SHELL_SETTINGS, "enabled-extensions") {
        Ok(Some(value)) => value,
        // No GNOME Shell settings: no extensions to ask.
        Ok(None) => return,
        Err(error) => {
            loaded.unread_extensions.push(format!(
                "could not read the enabled GNOME Shell extensions: {error}"
            ));
            return;
        }
    };
    let Some(uuids) = string_array(&enabled) else {
        loaded.unread_extensions.push(format!(
            "unexpected value for org.gnome.shell enabled-extensions: {}",
            enabled.trim()
        ));
        return;
    };
    if uuids.is_empty() {
        return;
    }
    let disable_user = match gsettings_get(SHELL_SETTINGS, "disable-user-extensions") {
        Ok(value) => value.is_some_and(|value| value.trim() == "true"),
        Err(error) => {
            loaded.notes.push(format!(
                "could not read disable-user-extensions ({error}); assuming user extensions are on"
            ));
            false
        }
    };
    load_extensions_from(
        loaded,
        &uuids,
        disable_user,
        &extension_roots(),
        list_recursively_in,
    );
}

fn load_extensions_from(
    loaded: &mut Loaded,
    uuids: &[String],
    disable_user: bool,
    roots: &[(PathBuf, bool)],
    list: impl Fn(&str, Option<&Path>) -> Result<String, ListError>,
) {
    for uuid in uuids {
        let Some(extension) = find_extension(uuid, roots) else {
            loaded.notes.push(format!(
                "enabled extension {uuid} is not installed, so it grabs nothing"
            ));
            continue;
        };
        if disable_user && extension.user {
            loaded.notes.push(format!(
                "extension {} is skipped: disable-user-extensions is on",
                extension.name
            ));
            continue;
        }
        let Some(schema) = extension.schema.as_deref() else {
            loaded.notes.push(format!(
                "extension {} declares no settings schema, so it has no settings-backed keybindings",
                extension.name
            ));
            loaded.read_extensions.push(extension.name.clone());
            continue;
        };
        match list(schema, extension.schema_dir.as_deref()) {
            Ok(output) => {
                add_extension_bindings(loaded, &extension, schema, &output);
                loaded.read_extensions.push(extension.name.clone());
            }
            Err(ListError::NoSchema) => loaded.unread_extensions.push(format!(
                "extension {} ({}): schema {schema} could not be found, so its keybindings are unread",
                extension.name, extension.uuid
            )),
            Err(ListError::Failed(error)) => loaded.unread_extensions.push(format!(
                "extension {} ({}): could not read schema {schema}: {error}",
                extension.name, extension.uuid
            )),
        }
    }
}

/// `gsettings get`; `Ok(None)` when the schema or key is not installed.
fn gsettings_get(schema: &str, key: &str) -> Result<Option<String>, String> {
    let mut gsettings = Command::new("gsettings");
    gsettings.args(["get", schema, key]);
    let output = command::output(&mut gsettings).map_err(|error| format!("gsettings: {error}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if stderr.contains("No such schema") || stderr.contains("No such key") {
            return Ok(None);
        }
        return Err(super::gsettings::command_error(
            "gsettings",
            output.status,
            &output.stderr,
        ));
    }
    if stderr.contains("'memory' GSettings backend") {
        return Err(
            "gsettings fell back to the memory backend (dconf unreachable), so it reports schema defaults, not the session's values".into(),
        );
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

#[derive(Debug, PartialEq, Eq)]
enum Accelerator {
    /// Empty or `disabled`: mutter binds nothing.
    Disabled,
    /// Keys this accelerator can match (one, or two for `Above_Tab`).
    Combos(Vec<KeyCombo>),
    Unmappable(&'static str),
}

/// Parse an accelerator the way mutter's `meta_parse_accelerator`
/// (src/core/meta-accel-parse.c) does: modifier tokens are case-insensitive,
/// unknown `<...>` tokens are skipped, `0xNN` is a keycode and `Above_Tab`
/// names the key above Tab.
fn parse_accelerator(accelerator: &str) -> Accelerator {
    if accelerator.is_empty() || accelerator == "disabled" {
        return Accelerator::Disabled;
    }
    let mut mask = 0;
    let mut rest = accelerator;
    while let Some(after) = rest.strip_prefix('<') {
        let Some(end) = after.find('>') else {
            // mutter skips to the end of the string: no key remains.
            return Accelerator::Unmappable("unterminated modifier");
        };
        let name = after[..end].to_ascii_lowercase();
        let modifier = match name.as_str() {
            "primary" | "control" | "ctrl" | "ctl" => "ctrl",
            "shift" | "shft" => "shift",
            "alt" | "mod1" => "alt",
            "super" | "mod4" => "super",
            "hyper" | "mod3" => "hyper",
            "mod2" => "mod2",
            "mod5" => "mod5",
            "meta" => {
                return Accelerator::Unmappable(
                    "<Meta> is a virtual modifier whose physical key depends on the keymap",
                );
            }
            // mutter ignores any other token, `<Release>` included.
            _ => "",
        };
        if !modifier.is_empty() {
            mask |= modifier_mask(modifier).unwrap_or(0);
        }
        rest = &after[end + 1..];
    }
    if rest.is_empty() {
        return Accelerator::Unmappable("no key after the modifiers");
    }
    if let Some(hex) = rest.strip_prefix("0x") {
        let digits = hex
            .find(|character: char| !character.is_ascii_hexdigit())
            .map_or(hex, |end| &hex[..end]);
        return match u32::from_str_radix(digits, 16) {
            Ok(code) if digits.len() >= 2 => {
                Accelerator::Combos(vec![KeyCombo::from_parts(mask, format!("CODE:{code}"))])
            }
            _ => Accelerator::Unmappable("malformed keycode"),
        };
    }
    if rest == "Above_Tab" {
        return Accelerator::Combos(vec![
            KeyCombo::from_parts(mask, format!("CODE:{ABOVE_TAB_KEYCODE}")),
            KeyCombo::from_parts(mask, "grave"),
        ]);
    }
    if rest.chars().any(char::is_whitespace) {
        return Accelerator::Unmappable("key name contains whitespace");
    }
    Accelerator::Combos(vec![KeyCombo::from_parts(mask, rest)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> Accelerator {
        Accelerator::Combos(vec![text.parse().unwrap()])
    }

    #[test]
    fn parses_accelerators_like_mutter() {
        assert_eq!(parse_accelerator("<Super>Return"), combo("super+return"));
        assert_eq!(parse_accelerator("<Primary><Alt>t"), combo("ctrl+alt+t"));
        assert_eq!(
            parse_accelerator("<Shift><Super>Page_Up"),
            combo("shift+super+page_up")
        );
        assert_eq!(parse_accelerator("<shft><CTL>x"), combo("ctrl+shift+x"));
        assert_eq!(parse_accelerator("<Mod4>x"), combo("super+x"));
        assert_eq!(parse_accelerator("XF86AudioMute"), combo("xf86audiomute"));
        // mutter skips tokens it does not know, `<Release>` included.
        assert_eq!(parse_accelerator("<Release><Super>l"), combo("super+l"));
        assert_eq!(parse_accelerator("<Win>t"), combo("t"));
        assert_eq!(parse_accelerator("0x31"), combo("code:49"));
        assert_eq!(
            parse_accelerator("<Super>Above_Tab"),
            Accelerator::Combos(vec![
                "super+code:49".parse().unwrap(),
                "super+`".parse().unwrap()
            ])
        );
        assert_eq!(parse_accelerator(""), Accelerator::Disabled);
        assert_eq!(parse_accelerator("disabled"), Accelerator::Disabled);
        assert!(matches!(
            parse_accelerator("<Meta>x"),
            Accelerator::Unmappable(_)
        ));
        assert!(matches!(
            parse_accelerator("<Super>"),
            Accelerator::Unmappable(_)
        ));
    }

    #[test]
    fn reads_only_string_values() {
        assert_eq!(
            string_values("['<Super>l', '<Control><Alt>t']"),
            vec!["<Super>l", "<Control><Alt>t"]
        );
        assert_eq!(string_values("['', '<Super>F1']"), vec!["", "<Super>F1"]);
        assert_eq!(string_values("'<Super>t'"), vec!["<Super>t"]);
        assert!(string_values("@as []").is_empty());
        assert!(string_values("[]").is_empty());
        // `volume-step 6` is an integer, not a key named 6.
        assert!(string_values("6").is_empty());
        assert!(string_values("uint32 30").is_empty());
    }

    #[test]
    fn matches_live_listing_entries() {
        let output = "org.gnome.desktop.wm.keybindings close ['<Super>q']\n\
                      org.gnome.desktop.wm.keybindings minimize @as []\n\
                      org.gnome.desktop.wm.keybindings-other close ['<Super>z']\n";
        let entries =
            schema_entries(output, "org.gnome.desktop.wm.keybindings").collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![("close", "['<Super>q']"), ("minimize", "@as []")]
        );
        let mut loaded = Loaded::default();
        for (key, value) in entries {
            for accelerator in string_values(value) {
                push_accelerator(
                    &mut loaded,
                    accelerator,
                    "org.gnome.desktop.wm.keybindings".into(),
                    "mutter",
                    None,
                    key.into(),
                    None,
                );
            }
        }
        let hit = inspect(Ok(loaded), &"super+q".parse().unwrap(), |_| Ok(true));
        assert_eq!(hit.outcome, Outcome::Consumed);
        assert!(
            hit.details
                .iter()
                .any(|line| line.contains("close = '<Super>q'"))
        );
        let miss = inspect(Ok(Loaded::default()), &"super+q".parse().unwrap(), |_| {
            Ok(true)
        });
        assert_eq!(miss.outcome, Outcome::Pass);
        let unavailable = inspect(
            Err("gsettings: not found".into()),
            &"super+q".parse().unwrap(),
            |_| Ok(true),
        );
        assert_eq!(unavailable.outcome, Outcome::Unavailable);
        // Live GSettings answers both ways; an unreachable store answers
        // nothing.
        assert_eq!(hit.tier, Some(EvidenceTier::LiveQuery));
        assert_eq!(miss.tier, Some(EvidenceTier::LiveQuery));
        assert_eq!(unavailable.tier, None);
    }
    /// Loaded bindings for one schema entry owned by `owner`.
    fn owned(
        schema: &str,
        owner: Option<&'static str>,
        component: &'static str,
        accelerator: &str,
        action: &str,
    ) -> Loaded {
        let mut loaded = Loaded::default();
        push_accelerator(
            &mut loaded,
            accelerator.into(),
            schema.into(),
            component,
            owner,
            action.into(),
            Some("touch /tmp/fired".into()),
        );
        loaded
    }

    #[test]
    fn custom_shortcut_is_not_grabbed_while_its_daemon_is_down() {
        let key = "super+t".parse().unwrap();
        let loaded = || {
            owned(
                "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/custom0/",
                Some(MEDIA_KEYS_OWNER),
                "gnome-settings-daemon custom shortcut",
                "<Super>t",
                "Lab fire",
            )
        };
        // Owner absent: nothing grabs the key, so the key goes on.
        let idle = inspect(Ok(loaded()), &key, |name| {
            assert_eq!(name, MEDIA_KEYS_OWNER);
            Ok(false)
        });
        assert_eq!(idle.outcome, Outcome::Pass);
        assert_eq!(
            idle.summary,
            "GNOME shortcut is configured but its daemon is not running"
        );
        assert!(
            idle.details
                .iter()
                .any(|line| line.contains("org.gnome.SettingsDaemon.MediaKeys is not running"))
        );
        // Owner present: the shortcut consumes the key as before.
        let running = inspect(Ok(loaded()), &key, |_| Ok(true));
        assert_eq!(running.outcome, Outcome::Consumed);
        // The owner cannot be asked: keep the configured verdict, say so.
        let unknown = inspect(Ok(loaded()), &key, |_| Err("gdbus: not found".into()));
        assert_eq!(unknown.outcome, Outcome::Consumed);
        assert!(unknown.details.iter().any(|line| {
            line.contains("could not check whether org.gnome.SettingsDaemon.MediaKeys runs")
        }));
    }

    #[test]
    fn shell_binding_is_not_grabbed_without_gnome_shell() {
        let key = "super+q".parse().unwrap();
        let shell = owned(
            "org.gnome.shell.keybindings",
            Some(SHELL_OWNER),
            "GNOME Shell",
            "<Super>q",
            "toggle-overview",
        );
        let idle = inspect(Ok(shell), &key, |_| Ok(false));
        assert_eq!(idle.outcome, Outcome::Pass);
        // mutter owns its own bindings whatever the shell's state.
        let mutter = owned(
            "org.gnome.desktop.wm.keybindings",
            None,
            "mutter",
            "<Super>q",
            "close",
        );
        let kept = inspect(Ok(mutter), &key, |_| Ok(false));
        assert_eq!(kept.outcome, Outcome::Consumed);
    }

    #[test]
    fn overlay_key_is_the_super_gesture() {
        let key = KeyCombo::parse_alone("super").unwrap();
        let result = inspect_alone(&key, Ok(Some("Super_L".into())));
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
        assert!(result.summary.contains("Activities"), "{}", result.summary);
        let evidence = result.binding.expect("overlay-key is the binding");
        assert_eq!(evidence.description.as_deref(), Some("overlay-key"));
    }

    #[test]
    fn a_disabled_or_other_overlay_key_leaves_super_free() {
        let key = KeyCombo::parse_alone("super").unwrap();
        for value in ["", "disabled", "Alt_L"] {
            let result = inspect_alone(&key, Ok(Some(value.into())));
            assert_eq!(result.outcome, Outcome::Pass, "{value}");
        }
        let ctrl = KeyCombo::parse_alone("ctrl").unwrap();
        let result = inspect_alone(&ctrl, Ok(Some("Super_L".into())));
        assert_eq!(result.outcome, Outcome::Pass);
    }

    #[test]
    fn an_unreadable_overlay_key_is_unavailable() {
        let key = KeyCombo::parse_alone("super").unwrap();
        let result = inspect_alone(&key, Err("gsettings: not found".into()));
        assert_eq!(result.outcome, Outcome::Unavailable);
    }

    const DOCK_SCHEMA: &str = "org.gnome.shell.extensions.dash-to-dock";
    /// `gsettings list-recursively` of Dash to Dock, trimmed: keybinding
    /// arrays, an empty one, and values of other types.
    const DOCK_OUTPUT: &str = "org.gnome.shell.extensions.dash-to-dock app-hotkey-1 ['<Super>1']\n\
        org.gnome.shell.extensions.dash-to-dock app-hotkey-2 ['<Super>2']\n\
        org.gnome.shell.extensions.dash-to-dock shortcut ['<Super>q']\n\
        org.gnome.shell.extensions.dash-to-dock shortcut-text '<Super>q'\n\
        org.gnome.shell.extensions.dash-to-dock app-hotkey-3 @as []\n\
        org.gnome.shell.extensions.dash-to-dock preferred-monitor-by-connector ['DP-1']\n\
        org.gnome.shell.extensions.dash-to-dock dock-position 'BOTTOM'\n\
        org.gnome.shell.extensions.dash-to-dock dash-max-icon-size 48\n";

    fn dock() -> Extension {
        Extension {
            uuid: "dash-to-dock@micxgx.gmail.com".into(),
            name: "Dash to Dock".into(),
            schema: Some(DOCK_SCHEMA.into()),
            schema_dir: None,
            user: true,
        }
    }

    fn loaded_with_dock() -> Loaded {
        let mut loaded = Loaded::default();
        add_extension_bindings(&mut loaded, &dock(), DOCK_SCHEMA, DOCK_OUTPUT);
        loaded.read_extensions.push("Dash to Dock".into());
        loaded
    }

    #[test]
    fn reads_extension_keybindings_and_ignores_other_settings() {
        let loaded = loaded_with_dock();
        let actions = loaded
            .bindings
            .iter()
            .map(|binding| (binding.action.as_str(), binding.accelerator.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![
                ("app-hotkey-1", "<Super>1"),
                ("app-hotkey-2", "<Super>2"),
                ("shortcut", "<Super>q")
            ]
        );
        assert!(loaded.bindings.iter().all(|binding| binding.extension));
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
    }

    #[test]
    fn an_extension_keybinding_is_handled_not_a_pass() {
        let result = inspect(
            Ok(loaded_with_dock()),
            &"super+1".parse().unwrap(),
            |name| {
                assert_eq!(name, SHELL_OWNER);
                Ok(true)
            },
        );
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.tier, Some(EvidenceTier::LiveQuery));
        let binding = result.binding.expect("the extension key is the binding");
        assert_eq!(
            binding.dispatcher.as_deref(),
            Some("GNOME Shell extension Dash to Dock")
        );
        assert_eq!(binding.description.as_deref(), Some("app-hotkey-1"));
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains(DOCK_SCHEMA) && line.contains("'<Super>1'"))
        );
    }

    #[test]
    fn an_extension_does_not_grab_while_gnome_shell_is_down() {
        let result = inspect(Ok(loaded_with_dock()), &"super+1".parse().unwrap(), |_| {
            Ok(false)
        });
        assert_eq!(result.outcome, Outcome::Pass);
    }

    #[test]
    fn a_mutter_binding_outranks_an_extension_one() {
        let mut loaded = loaded_with_dock();
        push_accelerator(
            &mut loaded,
            "<Super>q".into(),
            "org.gnome.desktop.wm.keybindings".into(),
            "mutter",
            None,
            "close".into(),
            None,
        );
        let result = inspect(Ok(loaded), &"super+q".parse().unwrap(), |_| Ok(true));
        assert_eq!(result.outcome, Outcome::Consumed);
        let binding = result.binding.unwrap();
        assert_eq!(binding.dispatcher.as_deref(), Some("mutter"));
    }

    #[test]
    fn a_key_no_readable_extension_binds_still_passes() {
        let result = inspect(Ok(loaded_with_dock()), &"super+9".parse().unwrap(), |_| {
            Ok(true)
        });
        assert_eq!(result.outcome, Outcome::Pass);
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("extensions read: Dash to Dock"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("GlobalShortcuts"))
        );
    }

    #[test]
    fn unread_extensions_make_a_miss_uncertain() {
        let mut loaded = Loaded::default();
        loaded.unread_extensions.push(
            "extension Pop Shell (pop-shell@system76.com): schema org.gnome.shell.extensions.pop-shell could not be found, so its keybindings are unread".into(),
        );
        let result = inspect(Ok(loaded), &"super+1".parse().unwrap(), |_| Ok(true));
        assert_eq!(result.outcome, Outcome::UncertainContinues);
        assert!(result.summary.contains("could not be read"));
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("pop-shell@system76.com"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("GlobalShortcuts"))
        );
    }

    /// A temporary extensions root holding the named extensions, each with
    /// a `metadata.json` body.
    fn extension_root(extensions: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "whykey-gnome-ext-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        for (uuid, metadata) in extensions {
            let dir = root.join(uuid);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("metadata.json"), metadata).unwrap();
        }
        root
    }

    fn uuids(list: &[&str]) -> Vec<String> {
        list.iter().map(|uuid| (*uuid).to_owned()).collect()
    }

    #[test]
    fn enabled_extensions_are_found_by_uuid_and_read_by_schema() {
        let user = extension_root(&[(
            "dash-to-dock@micxgx.gmail.com",
            r#"{"name": "Dash to Dock", "settings-schema": "org.gnome.shell.extensions.dash-to-dock"}"#,
        )]);
        let system = extension_root(&[
            (
                "tiling-assistant@leleat-on-github",
                r#"{"name": "Tiling Assistant", "settings-schema": "org.gnome.shell.extensions.tiling-assistant"}"#,
            ),
            ("plain@example.org", r#"{"name": "Plain"}"#),
        ]);
        let roots = vec![(user.clone(), true), (system.clone(), false)];
        let mut loaded = Loaded::default();
        load_extensions_from(
            &mut loaded,
            &uuids(&[
                "dash-to-dock@micxgx.gmail.com",
                "tiling-assistant@leleat-on-github",
                "plain@example.org",
                "gone@example.org",
            ]),
            false,
            &roots,
            |schema, _| match schema {
                DOCK_SCHEMA => Ok(DOCK_OUTPUT.to_owned()),
                "org.gnome.shell.extensions.tiling-assistant" => Ok(
                    "org.gnome.shell.extensions.tiling-assistant tile-maximize ['<Super>Up']\n"
                        .to_owned(),
                ),
                other => panic!("unexpected schema {other}"),
            },
        );
        assert!(loaded.unread_extensions.is_empty());
        assert_eq!(
            loaded.read_extensions,
            vec!["Dash to Dock", "Tiling Assistant", "Plain"]
        );
        assert!(loaded.bindings.iter().any(|binding| {
            binding.action == "tile-maximize" && binding.component.ends_with("Tiling Assistant")
        }));
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.contains("gone@example.org is not installed"))
        );
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.contains("Plain declares no settings schema"))
        );
        let _ = std::fs::remove_dir_all(user);
        let _ = std::fs::remove_dir_all(system);
    }

    #[test]
    fn user_extensions_are_skipped_when_user_extensions_are_disabled() {
        let user = extension_root(&[(
            "dash-to-dock@micxgx.gmail.com",
            r#"{"name": "Dash to Dock", "settings-schema": "org.gnome.shell.extensions.dash-to-dock"}"#,
        )]);
        let mut loaded = Loaded::default();
        load_extensions_from(
            &mut loaded,
            &uuids(&["dash-to-dock@micxgx.gmail.com"]),
            true,
            &[(user.clone(), true)],
            |_, _| panic!("a skipped extension must not be read"),
        );
        assert!(loaded.bindings.is_empty());
        assert!(loaded.unread_extensions.is_empty());
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.contains("disable-user-extensions"))
        );
        let _ = std::fs::remove_dir_all(user);
    }

    #[test]
    fn an_extension_whose_schema_cannot_be_read_is_unread() {
        let user = extension_root(&[
            (
                "pop-shell@system76.com",
                r#"{"name": "Pop Shell", "settings-schema": "org.gnome.shell.extensions.pop-shell"}"#,
            ),
            (
                "forge@jmmaranan.com",
                r#"{"name": "Forge", "settings-schema": "org.gnome.shell.extensions.forge"}"#,
            ),
        ]);
        let mut loaded = Loaded::default();
        load_extensions_from(
            &mut loaded,
            &uuids(&["pop-shell@system76.com", "forge@jmmaranan.com"]),
            false,
            &[(user.clone(), true)],
            |schema, _| {
                if schema.ends_with("pop-shell") {
                    Err(ListError::NoSchema)
                } else {
                    Err(ListError::Failed("dconf unreachable".into()))
                }
            },
        );
        assert_eq!(loaded.unread_extensions.len(), 2);
        assert!(loaded.unread_extensions[0].contains("Pop Shell"));
        assert!(loaded.unread_extensions[1].contains("dconf unreachable"));
        assert!(loaded.read_extensions.is_empty());
        let _ = std::fs::remove_dir_all(user);
    }

    #[test]
    fn parses_enabled_extension_lists() {
        assert_eq!(
            string_array("['a@b', 'c@d']\n"),
            Some(vec!["a@b".to_owned(), "c@d".to_owned()])
        );
        assert_eq!(string_array("@as []"), Some(Vec::new()));
        assert_eq!(string_array("true"), None);
        assert_eq!(string_array("'x'"), None);
    }

    #[test]
    fn only_accelerator_shaped_strings_are_keybindings() {
        for accelerator in ["<Super>1", "<Primary><Alt>t", "XF86AudioMute", "F12"] {
            assert!(looks_like_accelerator(accelerator), "{accelerator}");
        }
        for other in ["", "DP-1", "firefox.desktop", "Foo", "F"] {
            assert!(!looks_like_accelerator(other), "{other}");
        }
    }
}
