use serde::Serialize;

pub use crate::layers::BindingRecord;
/// Compatibility alias from the 1.x series: new code uses [`BindingRecord`].
pub type BindingEntry = BindingRecord;

use crate::schema;
use crate::style::RenderOptions;

const MAX_BINDINGS: usize = 8192;

/// Certainty of a runtime record whose key the source does not report (for
/// example Hyprland Lua binds with runtime-built `code:` keys). Such records
/// list an action and modifiers only and are never compared by key.
pub(crate) const UNRESOLVED_KEY_CERTAINTY: &str = "runtime effective binding; key not reported";

/// Versioned output for `whykey bindings`.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema_version: u8,
    /// False means the listed sources are useful evidence but do not prove
    /// every runtime client, plugin, or firmware transformation.
    pub complete: bool,
    pub bindings: Vec<BindingRecord>,
    pub unavailable: Vec<String>,
    pub limitations: Vec<String>,
    /// Records whose key the source does not report but whose modifiers
    /// equal those of a `--key` filter: any of them may be bound to it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub possible_matches: Vec<BindingRecord>,
}

pub fn current() -> Inventory {
    // One discovery pass: applicability comes from the shared Environment
    // snapshot instead of re-running desktop detection per adapter.
    let environment = crate::environment::Environment::collect();
    let mut inventory = Inventory {
        schema_version: 1,
        complete: false,
        bindings: Vec::new(),
        unavailable: Vec::new(),
        limitations: vec![
            "the inventory covers only sources exposed by detected adapters".into(),
            "application, plugin, input-inhibitor, and firmware bindings may remain outside the listing".into(),
        ],
        possible_matches: Vec::new(),
    };

    for entry in crate::registry::DESKTOPS {
        let Some(collect) = entry.inventory.or(entry.bindings) else {
            continue;
        };
        if !inventory_adapter_applies(&environment, entry) {
            continue;
        }
        match collect() {
            Ok(mut records) => inventory.bindings.append(&mut records),
            Err(error) => inventory.unavailable.push(error),
        }
    }

    append_extension_inventory(&environment, &mut inventory);

    if environment.desktop("compositor").applicable
        && !crate::registry::DESKTOPS.iter().any(|entry| {
            entry.inventory.or(entry.bindings).is_some()
                && inventory_adapter_applies(&environment, entry)
        })
    {
        inventory
            .limitations
            .push("the detected desktop has no dedicated binding inventory adapter".into());
    }

    // Stable sort without the action: records of one key keep the order the
    // source reported them in, which is the order a compositor such as
    // Hyprland runs them in (a submap bind stops the ones after it).
    inventory.bindings.sort_by(|left, right| {
        (
            &left.source,
            &left.key,
            &left.context,
            &left.device,
            &left.submap,
        )
            .cmp(&(
                &right.source,
                &right.key,
                &right.context,
                &right.device,
                &right.submap,
            ))
    });
    if inventory.bindings.len() > MAX_BINDINGS {
        inventory.bindings.truncate(MAX_BINDINGS);
        inventory.limitations.push(format!(
            "binding inventory was truncated to {MAX_BINDINGS} entries"
        ));
    }
    inventory
}

fn append_extension_inventory(
    environment: &crate::environment::Environment,
    inventory: &mut Inventory,
) {
    crate::command::with_deadline(crate::command::configured_diagnostic_timeout(), || {
        let mut truncated = false;
        for adapter in &environment.extension_adapters {
            if !crate::extension_adapters::applicable_with_context(
                adapter,
                &environment.compositor_context,
            ) {
                continue;
            }
            if crate::command::deadline_exceeded() {
                truncated = true;
                break;
            }
            let report = environment.extension_inventory(adapter);
            if let Some(error) = report.error {
                inventory.unavailable.push(error);
                continue;
            }
            inventory.bindings.extend(report.records);
            if report.malformed_entries > 0 {
                inventory.unavailable.push(format!(
                    "{}: {} malformed binding entr{} skipped",
                    adapter.display,
                    report.malformed_entries,
                    if report.malformed_entries == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                ));
            }
        }
        if truncated {
            inventory
                .unavailable
                .push("extension inventory truncated: diagnostic budget exceeded".into());
        }
    });
}

/// A `--key` filter. Text that parses like an inspect query (`super+enter`,
/// `super+,`, `code:10`) matches records by normalized key: a bare key
/// matches it under any modifiers, a combination needs exactly its
/// modifiers. Any other text is a case-insensitive substring of the key.
pub(crate) enum KeyFilter<'a> {
    Combo(crate::key::KeyCombo, &'a str),
    Text(&'a str),
}

impl<'a> KeyFilter<'a> {
    pub(crate) fn new(raw: &'a str) -> Self {
        match crate::key::KeyCombo::parse_query(raw) {
            Ok(combo) => Self::Combo(combo, raw),
            Err(_) => Self::Text(raw),
        }
    }

    /// Whether a record key matches; `keycode` is the code of a key shown
    /// as its layout symbol (see [`record_keycode`]).
    pub(crate) fn matches(&self, key: &str, keycode: Option<u32>) -> bool {
        match self {
            Self::Text(needle) => contains(key, Some(needle)),
            Self::Combo(query, raw) => match key.parse::<crate::key::KeyCombo>() {
                Ok(bound) => {
                    let same_key = bound.key() == query.key()
                        || keycode.is_some_and(|code| query.key() == format!("CODE:{code}"));
                    same_key && (query.modmask() == 0 || bound.modmask() == query.modmask())
                }
                // A key the parser does not read (for example a modifier
                // prefix without a key) can still contain the text.
                Err(_) => contains(key, Some(raw)),
            },
        }
    }
}

/// The keycode of a record whose `code:N` key is listed as its symbol in
/// the active layout.
pub(crate) fn record_keycode(record: &BindingRecord) -> Option<u32> {
    crate::layers::hyprland::displayed_keycode(&record.certainty)
}

pub fn filter(
    mut inventory: Inventory,
    key: Option<&str>,
    action: Option<&str>,
    source: Option<&str>,
    device: Option<&str>,
    submap: Option<&str>,
) -> Inventory {
    let key = key.map(KeyFilter::new);
    let matches_key = |binding: &BindingRecord| {
        key.as_ref()
            .is_none_or(|key| key.matches(&binding.key, record_keycode(binding)))
    };
    let matches_other_filters = |binding: &BindingRecord| {
        contains(&binding.action, action)
            && contains(&binding.source, source)
            && contains_optional(binding.device.as_deref(), device)
            && contains_optional(binding.submap.as_deref(), submap)
    };
    if let Some(KeyFilter::Combo(query, _)) = &key {
        inventory.possible_matches = inventory
            .bindings
            .iter()
            .filter(|binding| {
                unresolved_modmask(binding) == Some(query.modmask())
                    && !matches_key(binding)
                    && matches_other_filters(binding)
            })
            .cloned()
            .collect();
    }
    inventory
        .bindings
        .retain(|binding| matches_key(binding) && matches_other_filters(binding));
    inventory
}

/// Modifiers of a record whose key the source does not report, for example
/// `SUPER+<key not reported>`.
fn unresolved_modmask(binding: &BindingRecord) -> Option<u32> {
    if binding.certainty != UNRESOLVED_KEY_CERTAINTY {
        return None;
    }
    binding
        .key
        .strip_suffix("<key not reported>")?
        .split('+')
        .filter(|part| !part.is_empty())
        .try_fold(0, |mask, part| {
            crate::key::modifier_mask(part).map(|bit| mask | bit)
        })
}

fn contains(haystack: &str, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

/// Missing metadata never matches an active filter: filters combine with
/// AND logic, so an entry without a device cannot satisfy `--device`.
fn contains_optional(field: Option<&str>, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        field.is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
    })
}

/// Whether a bindings-capable adapter contributes to this session. The X11
/// and programmable X11 adapters only expose bindings when their
/// configuration is readable, mirroring their original applicability rules.
fn inventory_adapter_applies(
    environment: &crate::environment::Environment,
    entry: &crate::registry::AdapterDescriptor,
) -> bool {
    let status = environment.desktop(entry.id);
    if matches!(entry.id, "x11" | "programmable") {
        status.applicable && status.ipc
    } else {
        status.applicable
    }
}

pub fn render_text(inventory: &Inventory) -> String {
    render_text_with_options(inventory, RenderOptions::plain(false))
}

pub fn render_text_with_options(inventory: &Inventory, options: RenderOptions) -> String {
    let mut output = format!("{}\n\n", options.accent("whykey bindings"));
    let scope = format!(
        "Scope: {} source(s); complete: {}",
        inventory
            .bindings
            .iter()
            .map(|entry| entry.source.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        if inventory.complete { "yes" } else { "no" }
    );
    for line in crate::style::wrap_hanging(&scope, options.width, "", "  ") {
        output.push_str(&line);
        output.push('\n');
    }
    for entry in &inventory.bindings {
        push_entry(&mut output, entry, options);
    }
    if inventory.bindings.is_empty() {
        output.push_str("No matching bindings were enumerated from the detected sources.\n");
    }
    if !inventory.possible_matches.is_empty() {
        let count = inventory.possible_matches.len();
        let note = format!(
            "{count} binding{} with these modifiers {} keys the source does not report; any of them may match:",
            if count == 1 { "" } else { "s" },
            if count == 1 { "has" } else { "have" },
        );
        for line in crate::style::wrap_hanging(&note, options.width, "", "  ") {
            output.push_str(&options.uncertain(line));
            output.push('\n');
        }
        for entry in &inventory.possible_matches {
            push_entry(&mut output, entry, options);
        }
    }
    for unavailable in &inventory.unavailable {
        for line in crate::style::wrap_hanging(
            &format!("UNAVAILABLE: {unavailable}"),
            options.width,
            "",
            "  ",
        ) {
            output.push_str(&options.failure(line));
            output.push('\n');
        }
    }
    for limitation in &inventory.limitations {
        for line in crate::style::wrap_hanging(
            &format!("LIMITATION: {limitation}"),
            options.width,
            "",
            "  ",
        ) {
            output.push_str(&options.uncertain(line));
            output.push('\n');
        }
    }
    output
}

fn push_entry(output: &mut String, entry: &BindingRecord, options: RenderOptions) {
    let line = format!(
        "{}  {}  {} [{}]",
        crate::style::human_key_text(&entry.key),
        entry.action,
        entry.source,
        entry.certainty
    );
    for line in crate::style::wrap_hanging(&line, options.width, "  ", "    ") {
        output.push_str(&line);
        output.push('\n');
    }
    for (label, value) in [
        ("context", &entry.context),
        ("device", &entry.device),
        ("submap", &entry.submap),
    ] {
        if let Some(value) = value {
            for line in crate::style::wrap_hanging(
                &format!("{label}: {value}"),
                options.width,
                "    ",
                "      ",
            ) {
                output.push_str(&line);
                output.push('\n');
            }
        }
    }
}

pub fn render_json(inventory: &Inventory, schema_version: u8) -> String {
    if schema_version == 2 {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 2,
            "operation": "bindings",
            "context": schema::context(),
            "complete": inventory.complete,
            "bindings": inventory.bindings,
            "unavailable": inventory.unavailable,
            "limitations": inventory.limitations,
            "possible_matches": inventory.possible_matches,
        }))
        .expect("bindings are serializable")
            + "\n";
    }
    serde_json::to_string_pretty(inventory).expect("binding inventory is serializable") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_modifier_masks_without_changing_the_raw_key() {
        assert_eq!(crate::layers::combo_display(4 | 64, "c"), "CTRL+SUPER+C");
        assert_eq!(crate::layers::combo_display(0, "code:30"), "CODE:30");
    }

    #[test]
    fn reports_an_explicitly_incomplete_inventory() {
        let inventory = Inventory {
            schema_version: 1,
            complete: false,
            bindings: Vec::new(),
            unavailable: Vec::new(),
            limitations: vec!["runtime clients remain unknown".into()],
            possible_matches: Vec::new(),
        };
        let value: serde_json::Value = serde_json::from_str(&render_json(&inventory, 1)).unwrap();
        assert_eq!(value["complete"], false);
        assert_eq!(value["schema_version"], 1);
    }

    fn filter_fixture() -> Inventory {
        Inventory {
            schema_version: 1,
            complete: false,
            bindings: vec![
                BindingRecord {
                    device: Some("AT Keyboard".into()),
                    submap: Some("default".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec foot".into(),
                    context: Some("default".into()),
                    certainty: "configured".into(),
                },
                BindingRecord {
                    device: Some("Other Keyboard".into()),
                    submap: Some("gaming".into()),
                    source: "Hyprland".into(),
                    key: "CTRL+X".into(),
                    action: "exec kitty".into(),
                    context: Some("gaming".into()),
                    certainty: "configured".into(),
                },
                BindingRecord {
                    device: None,
                    submap: None,
                    source: "GNOME".into(),
                    key: "SUPER+X".into(),
                    action: "Open overview".into(),
                    context: None,
                    certainty: "configured".into(),
                },
            ],
            unavailable: Vec::new(),
            limitations: Vec::new(),
            possible_matches: Vec::new(),
        }
    }

    #[test]
    fn filters_bindings_by_case_insensitive_key_action_and_source() {
        let filtered = filter(
            filter_fixture(),
            Some("ctrl+x"),
            Some("foot"),
            Some("hypr"),
            None,
            None,
        );

        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].source, "Hyprland");
    }

    #[test]
    fn filters_bindings_by_device_only() {
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            None,
            Some("at KEYBOARD"),
            None,
        );
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec foot");
    }

    #[test]
    fn filters_bindings_by_submap_only() {
        let filtered = filter(filter_fixture(), None, None, None, None, Some("GAMING"));
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec kitty");
    }

    #[test]
    fn filters_bindings_by_combined_key_device_and_submap() {
        let filtered = filter(
            filter_fixture(),
            Some("ctrl+x"),
            None,
            None,
            Some("other"),
            Some("gaming"),
        );
        assert_eq!(filtered.bindings.len(), 1);
        assert_eq!(filtered.bindings[0].action, "exec kitty");
        let empty = filter(
            filter_fixture(),
            Some("ctrl+x"),
            None,
            None,
            Some("other"),
            Some("default"),
        );
        assert!(empty.bindings.is_empty());
    }

    #[test]
    fn missing_metadata_never_matches_an_active_filter() {
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            Some("gnome"),
            Some("keyboard"),
            None,
        );
        assert!(filtered.bindings.is_empty());
        let filtered = filter(
            filter_fixture(),
            None,
            None,
            Some("gnome"),
            None,
            Some("default"),
        );
        assert!(filtered.bindings.is_empty());
    }

    #[test]
    fn text_and_json_return_the_same_filtered_entries() {
        let filtered = filter(filter_fixture(), Some("ctrl+x"), None, None, None, None);
        let text = render_text(&filtered);
        assert!(text.contains("exec foot"));
        assert!(text.contains("exec kitty"));
        assert!(text.contains("submap: default"));
        assert!(text.contains("submap: gaming"));
        for version in [1, 2] {
            let value: serde_json::Value =
                serde_json::from_str(&render_json(&filtered, version)).unwrap();
            let entries = value["bindings"].as_array().unwrap();
            assert_eq!(entries.len(), 2);
            assert!(entries.iter().any(|entry| entry["submap"] == "default"));
        }
        let device_only = filter(filter_fixture(), None, None, None, Some("other"), None);
        let text = render_text(&device_only);
        assert!(text.contains("exec kitty"));
        assert!(!text.contains("exec foot"));
        let value: serde_json::Value = serde_json::from_str(&render_json(&device_only, 1)).unwrap();
        assert_eq!(value["bindings"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn key_filter_lists_same_modifier_bindings_with_unreported_keys() {
        let mut inventory = filter_fixture();
        for workspace in 1..=2 {
            inventory.bindings.push(BindingRecord {
                device: None,
                submap: Some("default".into()),
                source: "Hyprland".into(),
                key: "SUPER+<key not reported>".into(),
                action: format!("__lua {workspace}; Switch to workspace {workspace}"),
                context: None,
                certainty: UNRESOLVED_KEY_CERTAINTY.into(),
            });
        }
        inventory.bindings.push(BindingRecord {
            device: None,
            submap: Some("default".into()),
            source: "Hyprland".into(),
            key: "ALT+SUPER+<key not reported>".into(),
            action: "__lua 9; Switch to group window 1".into(),
            context: None,
            certainty: UNRESOLVED_KEY_CERTAINTY.into(),
        });

        let filtered = filter(inventory.clone(), Some("super+1"), None, None, None, None);

        assert!(filtered.bindings.is_empty());
        assert_eq!(filtered.possible_matches.len(), 2);
        let text = render_text_with_options(
            &filtered,
            RenderOptions {
                color: crate::style::ColorChoice::Never,
                verbose: false,
                width: 400,
            },
        );
        assert!(text.contains("No matching bindings were enumerated"));
        assert!(text.contains(
            "2 bindings with these modifiers have keys the source does not report; any of them may match:"
        ));
        assert!(text.contains("Super+<key not reported> __lua 2; Switch to workspace 2"));
        assert!(!text.contains("Switch to group window"));
        for version in [1, 2] {
            let value: serde_json::Value =
                serde_json::from_str(&render_json(&filtered, version)).unwrap();
            assert_eq!(value["possible_matches"].as_array().unwrap().len(), 2);
        }

        // Without a key filter nothing is a "possible" match.
        assert!(
            filter(inventory, None, None, None, None, None)
                .possible_matches
                .is_empty()
        );
    }

    #[test]
    fn extension_inventory_stops_at_the_shared_diagnostic_budget() {
        let mut environment = crate::environment::Environment::collect();
        environment.compositor_context.desktop = Some("budget".into());
        environment.extension_adapters = (1..=2)
            .map(|number| crate::extension_adapters::ExtensionAdapter {
                id: format!("budget-{number}"),
                display: "budget".into(),
                tier: "extension".into(),
                manifest_path: format!("budget-{number}.toml").into(),
                applicable_env: Vec::new(),
                applicable_desktop: vec!["budget".into()],
                bindings_cmd: vec!["sh".into(), "-c".into(), "sleep 1; printf '[]'".into()],
                focused_cmd: None,
                reload_gen_cmd: None,
            })
            .collect();
        let mut inventory = Inventory {
            schema_version: 1,
            complete: false,
            bindings: Vec::new(),
            unavailable: Vec::new(),
            limitations: Vec::new(),
            possible_matches: Vec::new(),
        };
        crate::command::with_deadline(std::time::Duration::ZERO, || {
            append_extension_inventory(&environment, &mut inventory);
        });

        assert!(
            inventory
                .unavailable
                .iter()
                .any(|entry| entry == "extension inventory truncated: diagnostic budget exceeded")
        );
    }

    #[test]
    fn key_filter_parses_queries_like_inspect() {
        let record = |key: &str, certainty: &str| BindingRecord {
            device: None,
            submap: Some("default".into()),
            source: "Hyprland".into(),
            key: key.into(),
            action: format!("action {key}"),
            context: Some("default".into()),
            certainty: certainty.into(),
        };
        let shown = "runtime effective binding; code:10 shown as its key in the active XKB layout";
        let mut inventory = filter_fixture();
        inventory.bindings = vec![
            record("SUPER+RETURN", "runtime effective binding"),
            record("SUPER+KP_ENTER", "runtime effective binding"),
            record("SUPER+BACKSPACE", "runtime effective binding"),
            record("SUPER+B", "runtime effective binding"),
            record("SUPER+/", "runtime effective binding"),
            record("SUPER+,", "runtime effective binding"),
            record("SUPER+1", shown),
            record("SHIFT+SUPER+CODE:201", "runtime effective binding"),
            record("RETURN", "runtime effective binding"),
        ];
        let keys = |query: &str| {
            filter(inventory.clone(), Some(query), None, None, None, None)
                .bindings
                .into_iter()
                .map(|binding| binding.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys("super+enter"), vec!["SUPER+RETURN"]);
        assert_eq!(keys("super+KP_Enter"), vec!["SUPER+KP_ENTER"]);
        assert_eq!(keys("super+b"), vec!["SUPER+B"]);
        assert_eq!(keys("super+/"), vec!["SUPER+/"]);
        assert_eq!(keys("super+slash"), vec!["SUPER+/"]);
        assert_eq!(keys("super+,"), vec!["SUPER+,"]);
        assert_eq!(keys("super+code:10"), vec!["SUPER+1"]);
        assert_eq!(keys("code:10"), vec!["SUPER+1"]);
        assert_eq!(keys("shift+super+code:201"), vec!["SHIFT+SUPER+CODE:201"]);
        // A bare key matches it under any modifiers.
        assert_eq!(keys("enter"), vec!["SUPER+RETURN", "RETURN"]);
        // Text that is not a key stays a substring filter.
        assert_eq!(keys("shift"), vec!["SHIFT+SUPER+CODE:201"]);
    }
}
