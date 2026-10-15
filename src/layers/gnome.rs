//! GNOME global shortcuts from live GSettings.
//!
//! mutter, GNOME Shell and gnome-settings-daemon read their keybindings from
//! GSettings and rebind as soon as a value changes, so `gsettings
//! list-recursively` reports the configuration the running session uses.

use std::env;
use std::process::Command;

use crate::command;
use crate::key::{KeyCombo, modifier_mask};
use crate::layers::gsettings::quoted_values;
use crate::layers::{
    BindingEvidence, BindingRecord, BindingScope, EvidenceTier, LayerId, LayerResult, Outcome,
    SourceLocation,
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
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("gnome"))
        })
        || env::var_os("GNOME_DESKTOP_SESSION_ID").is_some()
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
        inspect(load(), key, owner_running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    /// Accelerator as stored in GSettings.
    accelerator: String,
    schema: String,
    component: &'static str,
    /// Bus name whose owner grabs this key, when the component is a daemon.
    owner: Option<&'static str>,
    action: String,
    command: Option<String>,
}

#[derive(Debug, Default)]
struct Loaded {
    bindings: Vec<Binding>,
    /// Facts about bindings that could not be compared or schemas that are
    /// not installed.
    notes: Vec<String>,
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
        details.extend(loaded.notes);
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            "no GNOME global shortcut uses this key",
            details,
        )
        .with_tier(EvidenceTier::LiveQuery);
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
    let Some(first) = grabbed.first() else {
        details.extend(loaded.notes);
        return LayerResult::pass(
            LAYER,
            LayerId::Compositor,
            "GNOME shortcut is configured but its daemon is not running",
            details,
        )
        .with_tier(EvidenceTier::LiveQuery);
    };
    details.push(
        "not active while the focused window holds a keyboard-shortcuts inhibitor that the user allowed"
            .into(),
    );
    details.extend(loaded.notes);
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
    .with_dispatcher(first.component)
    .with_description(first.action.clone());
    LayerResult::new(
        LAYER,
        LayerId::Compositor,
        Outcome::Consumed,
        "GNOME global shortcut consumes the key",
        details,
    )
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
    Ok(loaded)
}

fn push_accelerator(
    loaded: &mut Loaded,
    accelerator: String,
    schema: String,
    component: &'static str,
    owner: Option<&'static str>,
    action: String,
    command: Option<String>,
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
            component,
            owner,
            action: action.clone(),
            command: command.clone(),
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
    let mut gsettings = Command::new("gsettings");
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
}
