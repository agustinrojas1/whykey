pub(crate) fn analyze(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[LuaBindHint],
    config_ignore_mods: bool,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    inspect_json_with_keycode(
        key,
        bindings_json,
        active_submap,
        keycodes,
        lua_hints,
        config_ignore_mods,
        physical_input,
    )
}

#[cfg(test)]
fn inspect_json(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
) -> Result<LayerResult, String> {
    inspect_json_with_keycode(key, bindings_json, active_submap, &[], &[], false, None)
}

pub(crate) fn inspect_json_with_keycode(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[LuaBindHint],
    config_ignore_mods: bool,
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    let (bindings, skipped_bindings) = parse_bindings(bindings_json)?;
    let active_submap = normalize_submap(active_submap);

    let active_bindings: Vec<_> = bindings
        .iter()
        .filter(|binding| binding_is_active(binding, active_submap))
        .collect();

    // Keep same-key bindings from other submaps visible. They do not handle
    // the current event, but hiding them makes mode-dependent configurations
    // look as if the key is unbound everywhere.
    let inactive_bindings: Vec<_> = bindings
        .iter()
        .filter(|binding| !binding_is_active(binding, active_submap))
        .filter(|binding| binding_matches_key(binding, key, keycodes, physical_input))
        .filter(|binding| {
            physical_device_match(&binding.device, physical_input) != DeviceMatch::NoMatch
        })
        .collect();

    let matches: Vec<_> = active_bindings
        .iter()
        .copied()
        .filter_map(|binding| {
            if !binding_matches_key(binding, key, keycodes, physical_input) {
                None
            } else {
                let device_scoped = has_device_scope(&binding.device);
                let device_match = physical_device_match(&binding.device, physical_input);
                if device_match == DeviceMatch::NoMatch {
                    return None;
                }
                let certainty = if binding.modmask == key.modmask() {
                    MatchCertainty::Exact
                } else if boolish_value(&binding.ignore_mods) == Some(true) {
                    MatchCertainty::ModifierInsensitive
                } else if boolish_value(&binding.ignore_mods) == Some(false) {
                    return None;
                } else if let Some(ignore_mods) = lua_ignore_mods_for_binding(binding, lua_hints) {
                    if ignore_mods {
                        MatchCertainty::ModifierInsensitive
                    } else {
                        return None;
                    }
                } else if config_ignore_mods {
                    MatchCertainty::ModifierInsensitive
                } else {
                    MatchCertainty::PossibleIgnoreMods
                };
                Some(BindingMatch {
                    binding,
                    // Without a physical capture we cannot know whether a
                    // per-device binding applies. A captured evdev device has
                    // already passed that filter, so retain modifier certainty.
                    certainty: if device_scoped
                        && (physical_input.is_none() || device_match == DeviceMatch::Unknown)
                    {
                        MatchCertainty::PossibleDevice
                    } else {
                        certainty
                    },
                })
            }
        })
        .collect();

    if matches.is_empty() {
        if skipped_bindings > 0 {
            return Ok(LayerResult::new(
                "Hyprland",
                LayerId::Compositor,
                Outcome::Unknown,
                "effective bindings are incomplete; no definitive match",
                details_with_physical_input(
                    vec![
                        format!("active submap: {active_submap}"),
                        format!(
                            "could not parse {skipped_bindings} binding entr{}",
                            if skipped_bindings == 1 { "y" } else { "ies" }
                        ),
                    ],
                    physical_input,
                ),
            )
            .with_verbose_details(inactive_submap_details(&inactive_bindings)));
        }
        return Ok(LayerResult::new(
            "Hyprland",
            LayerId::Compositor,
            Outcome::Pass,
            "no active binding found",
            details_with_physical_input(
                vec![format!("active submap: {active_submap}")],
                physical_input,
            ),
        )
        .with_verbose_details(inactive_submap_details(&inactive_bindings)));
    }

    let mut details = details_with_physical_input(
        vec![format!("active submap: {active_submap}")],
        physical_input,
    );
    // Same-key bindings from other submaps explain mode-dependent setups;
    // they stay one `--verbose` away in normal reports.
    let mut verbose_details: Vec<String> = Vec::new();
    if skipped_bindings > 0 {
        details.push(format!(
            "could not parse {skipped_bindings} effective binding entr{}; other bindings may be missing",
            if skipped_bindings == 1 { "y" } else { "ies" }
        ));
    }
    let has_exact_match = matches.iter().any(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        )
    });
    let has_possible_match = matches
        .iter()
        .any(|matched| matched.certainty == MatchCertainty::PossibleIgnoreMods);
    let has_possible_device = matches
        .iter()
        .any(|matched| matched.certainty == MatchCertainty::PossibleDevice);

    if has_possible_match {
        details.push("Hyprland does not expose the ignore_mods flag through hyprctl.".into());
    }
    if config_ignore_mods
        && matches
            .iter()
            .any(|matched| matched.certainty == MatchCertainty::ModifierInsensitive)
    {
        details.push(
            "the loaded declarative config contains an ignore_mods binding for this key".into(),
        );
    }
    if has_possible_device {
        if physical_input.and_then(|i| i.device.as_ref()).is_some() {
            details.push(
                "device-specific binding could not be matched by name with certainty; it remains possible.".into(),
            );
        } else {
            details.push(
                "device-specific binding found; the active keyboard device is unknown.".into(),
            );
        }
    } else if let Some(input) = physical_input {
        if input.device.is_some() {
            details.push(
                "device-specific bindings were checked against the captured evdev device.".into(),
            );
        } else {
            details.push(
                "The physical keycode was captured, but the source keyboard is unavailable.".into(),
            );
            details.push("Device-specific binding matching remains uncertain.".into());
        }
    }
    if !inactive_bindings.is_empty() {
        verbose_details.push(format!(
            "{} matching binding(s) exist in inactive submap(s); current submap is {active_submap}",
            inactive_bindings.len()
        ));
        verbose_details.extend(inactive_submap_details(&inactive_bindings));
    }
    if !has_exact_match {
        details.push(format!("no exact binding for {key}"));
    }

    for matched in &matches {
        let label = match matched.certainty {
            MatchCertainty::Exact => "binding",
            MatchCertainty::PossibleIgnoreMods => "possible modifier-insensitive binding",
            MatchCertainty::ModifierInsensitive => "modifier-insensitive binding",
            MatchCertainty::PossibleDevice => "possible device-specific binding",
        };
        details.push(format!(
            "{label}: {} ({})",
            binding_action(matched.binding),
            binding_scope(matched.binding, active_submap)
        ));
    }

    let exact_matches = matches.iter().filter(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        )
    });
    if exact_matches.clone().any(|matched| {
        boolish_value(&matched.binding.locked) == Some(true)
            || boolish_value(&matched.binding.dont_inhibit) == Some(true)
    }) {
        details.push(
            "matching binding is allowed while an input inhibitor is active (locked/dont_inhibit)"
                .into(),
        );
    }
    if exact_matches
        .clone()
        .any(|matched| boolish_value(&matched.binding.allow_input_capture) == Some(true))
    {
        details
            .push("matching binding remains active while client input capture is enabled".into());
    }

    if matches.iter().any(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        ) && dispatcher_is_opaque(matched.binding)
            && !matched.binding.non_consuming
    }) {
        details
            .push("hyprctl does not expose the forwarding result of an opaque dispatcher.".into());
    }
    if matches
        .iter()
        .any(|matched| matched.binding.dispatcher == "__lua")
    {
        details.push(
            "Lua/plugin dispatcher is active; its runtime action is not executed by whykey.".into(),
        );
    }

    let propagation = if skipped_bindings > 0 {
        Propagation::Indeterminate
    } else {
        binding_propagation(&matches)
    };
    let status = if skipped_bindings > 0 {
        LayerStatus::Indeterminate
    } else if has_exact_match {
        LayerStatus::Handled
    } else {
        LayerStatus::Indeterminate
    };
    let outcome = Outcome::from_parts(&status, &propagation);
    let summary = if skipped_bindings > 0 {
        "effective bindings are incomplete; handling cannot be proven"
    } else {
        match (&status, &propagation) {
            (LayerStatus::Handled, Propagation::Continues) => {
                "active binding found; event is forwarded"
            }
            (LayerStatus::Handled, Propagation::Stops) => "active binding found; event is consumed",
            (LayerStatus::Handled, Propagation::Redirected) => {
                "active binding found; event is redirected to another window"
            }
            (LayerStatus::Handled, Propagation::Indeterminate) => {
                "active binding found; forwarding cannot be determined"
            }
            (LayerStatus::Indeterminate, Propagation::Continues) if has_possible_device => {
                "no device-independent exact binding found; the active keyboard is unknown"
            }
            (LayerStatus::Indeterminate, Propagation::Continues) => {
                "no exact binding found; a same-key binding may ignore modifiers"
            }
            (LayerStatus::Indeterminate, Propagation::Indeterminate) => {
                "no exact binding found; forwarding cannot be proven"
            }
            _ => unreachable!("binding matches cannot produce this state"),
        }
    };

    let has_universal_match = matches
        .iter()
        .any(|matched| matched.binding.submap_universal);
    let binding = primary_match(&matches).map(|matched| {
        let mut evidence = binding_evidence(matched.binding, lua_hints, has_universal_match);
        if evidence.uncertainty.is_none() && matched.certainty == MatchCertainty::PossibleIgnoreMods
        {
            evidence.uncertainty = Some(crate::layers::UncertaintyReason::ModifierAmbiguity);
        }
        evidence
    });

    Ok({
        let mut result =
            LayerResult::new("Hyprland", LayerId::Compositor, outcome, summary, details)
                .with_verbose_details(verbose_details);
        result.binding = binding;
        result
    })
}

/// The match the conclusion describes: an exact binding when one exists,
/// otherwise the first candidate. Evidence never invents a stronger match.
fn primary_match<'a>(matches: &'a [BindingMatch<'a>]) -> Option<BindingMatch<'a>> {
    matches
        .iter()
        .find(|matched| {
            matches!(
                matched.certainty,
                MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
            )
        })
        .or_else(|| matches.first())
        .copied()
}

/// Typed binding evidence for the renderer and schema v2. The human-readable
/// `details` stay untouched; decisions must read these fields instead.
fn binding_evidence(
    binding: &Binding,
    lua_hints: &[LuaBindHint],
    has_universal_match: bool,
) -> BindingEvidence {
    let action = format!("{} {}", binding.dispatcher, binding.arg);
    BindingEvidence {
        dispatcher: none_if_empty(&binding.dispatcher),
        action: none_if_empty(&action),
        description: none_if_empty(&binding.description),
        submap: Some(normalize_submap(&binding.submap).to_string()),
        scope: if binding.submap_universal {
            BindingScope::Universal
        } else {
            BindingScope::Submap(normalize_submap(&binding.submap).to_string())
        },
        source: hint_source(binding, lua_hints),
        has_universal_match,
        uncertainty: if BindingEvidence::dispatcher_is_opaque(&binding.dispatcher) {
            Some(crate::layers::UncertaintyReason::OpaqueDispatcher)
        } else {
            None
        },
    }
}

fn none_if_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Link a runtime binding to its static config hint only on an exact
/// description match. Anything weaker would be a guessed file hint.
fn hint_source(binding: &Binding, lua_hints: &[LuaBindHint]) -> Option<SourceLocation> {
    let description = none_if_empty(&binding.description)?;
    let hint = lua_hints
        .iter()
        .find(|hint| hint.description.as_deref() == Some(&description))?;
    Some(SourceLocation {
        file: hint.source.display().to_string(),
        line: u32::try_from(hint.line_number).ok(),
    })
}

fn parse_bindings(bindings_json: &str) -> Result<(Vec<Binding>, usize), String> {
    let value: serde_json::Value = serde_json::from_str(bindings_json)
        .map_err(|error| format!("hyprctl returned invalid binding data: {error}"))?;
    let Some(entries) = value.as_array() else {
        return Err("hyprctl returned invalid binding data: expected an array".into());
    };
    let mut bindings = Vec::with_capacity(entries.len());
    let mut skipped = 0;
    for entry in entries {
        match serde_json::from_value::<Binding>(entry.clone()) {
            Ok(binding) => bindings.push(binding),
            Err(_) => skipped += 1,
        }
    }
    Ok((bindings, skipped))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchCertainty {
    Exact,
    PossibleIgnoreMods,
    ModifierInsensitive,
    PossibleDevice,
}

#[derive(Debug, Clone, Copy)]
struct BindingMatch<'a> {
    binding: &'a Binding,
    certainty: MatchCertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlowState {
    found: bool,
    dispatcher_passes: bool,
    dispatcher_redirects: bool,
    processing: bool,
}

#[derive(Debug, Clone, Copy)]
struct DispatcherOutcome {
    success: bool,
    passes: bool,
    redirects: bool,
}

fn normalize_submap(submap: &str) -> &str {
    match submap {
        "" | "reset" => "default",
        other => other,
    }
}

fn binding_is_active(binding: &Binding, active_submap: &str) -> bool {
    binding.submap_universal || normalize_submap(&binding.submap) == active_submap
}

fn inactive_submap_details(bindings: &[&Binding]) -> Vec<String> {
    bindings
        .iter()
        .map(|binding| {
            let submap = normalize_submap(&binding.submap);
            format!(
                "inactive submap binding: {} ({submap})",
                binding_action(binding)
            )
        })
        .collect()
}

fn binding_matches_key(
    binding: &Binding,
    combo: &KeyCombo,
    keycodes: &[crate::xkb::XkbKeycode],
    physical_input: Option<&PhysicalInput>,
) -> bool {
    if let Some(keycode) = combo.key().strip_prefix("CODE:") {
        // An explicit numeric query is already an assertion about the
        // binding's code; do not let a separately captured physical event
        // broaden it to another code.
        return keycode
            .parse::<u32>()
            .ok()
            .map(crate::xkb::XkbKeycode::from)
            == Some(binding.keycode);
    }
    if binding.keycode != crate::xkb::XkbKeycode::from(0) {
        return keycodes.contains(&binding.keycode)
            || physical_input
                .is_some_and(|input| physical_keycode_matches(binding.keycode, input.keycode));
    }
    binding.catch_all || binding.key.eq_ignore_ascii_case(combo.key())
}

fn physical_keycode_matches(
    binding_keycode: crate::xkb::XkbKeycode,
    evdev_keycode: crate::xkb::EvdevKeycode,
) -> bool {
    // `Binding.keycode` is an XKB keycode per the compositor. Physical
    // capture carries a Linux evdev code, so convert it once at this
    // boundary instead of accepting both numeric namespaces.
    evdev_keycode
        .to_xkb()
        .is_some_and(|xkb_keycode| binding_keycode == xkb_keycode)
}

fn binding_propagation(matches: &[BindingMatch<'_>]) -> Propagation {
    let mut states = vec![FlowState {
        found: false,
        dispatcher_passes: false,
        dispatcher_redirects: false,
        processing: true,
    }];

    for matched in matches {
        let previous = states;
        let mut next = Vec::new();

        if matches!(
            matched.certainty,
            MatchCertainty::PossibleIgnoreMods | MatchCertainty::PossibleDevice
        ) {
            next.extend(previous.iter().copied());
        }

        for state in previous {
            if !state.processing {
                next.push(state);
                continue;
            }

            for outcome in dispatcher_outcomes(matched.binding) {
                let found = if matched.binding.release
                    && !matched.binding.non_consuming
                    && !matched.binding.auto_consuming
                    && !dispatcher_is_special(matched.binding)
                {
                    true
                } else if matched.binding.long_press || matched.binding.non_consuming {
                    state.found
                } else if matched.binding.auto_consuming {
                    state.found || outcome.success
                } else {
                    true
                };
                let next_state = FlowState {
                    found,
                    dispatcher_passes: outcome.passes,
                    dispatcher_redirects: outcome.redirects,
                    processing: matched.binding.dispatcher != "submap",
                };
                if !next.contains(&next_state) {
                    next.push(next_state);
                }
            }
        }

        states = next;
    }

    let outcomes: Vec<_> = states
        .iter()
        .map(|state| {
            if state.dispatcher_redirects {
                Propagation::Redirected
            } else if state.dispatcher_passes || !state.found {
                Propagation::Continues
            } else {
                Propagation::Stops
            }
        })
        .collect();
    if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Continues)
    {
        Propagation::Continues
    } else if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Stops)
    {
        Propagation::Stops
    } else if outcomes
        .iter()
        .all(|outcome| *outcome == Propagation::Redirected)
    {
        Propagation::Redirected
    } else {
        Propagation::Indeterminate
    }
}

fn dispatcher_outcomes(binding: &Binding) -> Vec<DispatcherOutcome> {
    if binding.dispatcher == "pass" {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: true,
        }];
    }

    if binding.release
        && !binding.non_consuming
        && !binding.auto_consuming
        && !dispatcher_is_special(binding)
    {
        return vec![DispatcherOutcome {
            success: false,
            passes: false,
            redirects: false,
        }];
    }

    if binding.release && binding.auto_consuming {
        return vec![DispatcherOutcome {
            success: false,
            passes: true,
            redirects: false,
        }];
    }

    if binding.long_press {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: false,
        }];
    }

    if binding.non_consuming {
        return vec![DispatcherOutcome {
            success: true,
            passes: true,
            redirects: false,
        }];
    }

    if dispatcher_is_opaque(binding) {
        return vec![
            DispatcherOutcome {
                success: true,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: true,
                passes: true,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: true,
                redirects: false,
            },
        ];
    }

    if binding.auto_consuming {
        return vec![
            DispatcherOutcome {
                success: true,
                passes: false,
                redirects: false,
            },
            DispatcherOutcome {
                success: false,
                passes: false,
                redirects: false,
            },
        ];
    }

    vec![DispatcherOutcome {
        success: true,
        passes: false,
        redirects: false,
    }]
}
fn dispatcher_is_opaque(binding: &Binding) -> bool {
    BindingEvidence::dispatcher_is_opaque(&binding.dispatcher)
}

fn dispatcher_is_special(binding: &Binding) -> bool {
    matches!(
        binding.dispatcher.as_str(),
        "global" | "pass" | "sendshortcut" | "mouse"
    )
}

fn binding_scope<'a>(binding: &Binding, active_submap: &'a str) -> &'a str {
    if binding.submap_universal {
        "all submaps"
    } else {
        active_submap
    }
}

fn binding_action(binding: &Binding) -> String {
    let mut action = binding.dispatcher.clone();
    if !binding.arg.is_empty() {
        action.push(' ');
        action.push_str(&binding.arg);
    }
    if !binding.description.is_empty() {
        action.push_str("; ");
        action.push_str(&binding.description);
    }
    let mut flags = Vec::new();
    for (name, value) in [
        ("locked", &binding.locked),
        ("repeat", &binding.repeat),
        ("transparent", &binding.transparent),
        ("dont_inhibit", &binding.dont_inhibit),
        ("allow_input_capture", &binding.allow_input_capture),
    ] {
        if boolish_value(value) == Some(true) {
            flags.push(name);
        }
    }
    if has_device_scope(&binding.device) {
        flags.push("device-specific");
    }
    if !flags.is_empty() {
        action.push_str(" [");
        action.push_str(&flags.join(", "));
        action.push(']');
    }
    action
}

fn boolish_value(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn filter_whykey_capture_bindings(bindings_json: &str) -> String {
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(bindings_json) {
        if let Some(arr) = val.as_array_mut() {
            arr.retain(|b| b.get("submap").and_then(|s| s.as_str()) != Some("__whykey_capture"));
            if let Ok(filtered) = serde_json::to_string(&val) {
                return filtered;
            }
        }
    }
    bindings_json.to_owned()
}

fn has_device_scope(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => true,
    }
}

pub(crate) fn main_keyboard_lock_state(devices_json: &str) -> (bool, bool) {
    let Ok(devices) = serde_json::from_str::<serde_json::Value>(devices_json) else {
        return (false, false);
    };
    let keyboards = devices
        .get("keyboards")
        .and_then(serde_json::Value::as_array);
    let Some(keyboards) = keyboards else {
        return (false, false);
    };
    let keyboard = keyboards
        .iter()
        .find(|keyboard| keyboard.get("main").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| keyboards.first());
    let Some(keyboard) = keyboard else {
        return (false, false);
    };
    let caps = keyboard
        .get("capsLock")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let num = keyboard
        .get("numLock")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (caps, num)
}

fn details_with_physical_input(
    mut details: Vec<String>,
    physical_input: Option<&PhysicalInput>,
) -> Vec<String> {
    if let Some(input) = physical_input {
        match &input.device {
            Some(device) => details.push(format!("captured evdev device: {device}")),
            None => details.push("captured keyboard device: unavailable".into()),
        }
        let xkb_candidate = input
            .keycode
            .to_xkb()
            .map_or("unknown".into(), |code| code.get().to_string());
        details.push(format!(
            "physical keycode: {} (XKB keycode {})",
            input.keycode.get(),
            xkb_candidate
        ));
    }
    details
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceMatch {
    Match,
    NoMatch,
    Unknown,
}

fn physical_device_match(
    scope: &serde_json::Value,
    physical_input: Option<&PhysicalInput>,
) -> DeviceMatch {
    let Some(physical_input) = physical_input else {
        return DeviceMatch::Unknown;
    };
    let Some(device_name) = physical_input.device.as_deref() else {
        return DeviceMatch::Unknown;
    };
    match scope {
        serde_json::Value::String(name) => device_name_match(name, device_name),
        serde_json::Value::Object(object) => {
            let inclusive = object
                .get("inclusive")
                .and_then(boolish_value)
                .unwrap_or(true);
            let names = object
                .get("list")
                .and_then(serde_json::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if names.is_empty() {
                return DeviceMatch::Unknown;
            }
            let listed = names
                .iter()
                .map(|name| device_name_match(name, device_name))
                .fold(DeviceMatch::Unknown, |state, current| match current {
                    DeviceMatch::Match => DeviceMatch::Match,
                    DeviceMatch::NoMatch => state,
                    DeviceMatch::Unknown => {
                        if state == DeviceMatch::Match {
                            state
                        } else {
                            DeviceMatch::Unknown
                        }
                    }
                });
            match (inclusive, listed) {
                (true, DeviceMatch::Match) => DeviceMatch::Match,
                (true, DeviceMatch::NoMatch | DeviceMatch::Unknown) => DeviceMatch::Unknown,
                (false, DeviceMatch::Match) => DeviceMatch::NoMatch,
                (false, DeviceMatch::NoMatch | DeviceMatch::Unknown) => DeviceMatch::Unknown,
            }
        }
        _ => DeviceMatch::Unknown,
    }
}

fn device_name_match(configured: &str, actual: &str) -> DeviceMatch {
    let configured = configured.trim();
    if configured.is_empty() || configured.contains('*') || configured.contains('?') {
        return DeviceMatch::Unknown;
    }
    if configured.eq_ignore_ascii_case(actual.trim())
        || normalize_device_name(configured) == normalize_device_name(actual)
    {
        DeviceMatch::Match
    } else {
        // libinput/Hyprland and evdev can use different names for the same
        // composite keyboard. A mismatch is therefore not proof of exclusion.
        DeviceMatch::Unknown
    }
}

#[cfg(test)]
mod matching_tests {
    use super::*;

    #[test]
    fn analyzes_a_snapshot_without_ipc_or_config_access() {
        let key: KeyCombo = "super+return".parse().unwrap();
        let result = analyze(
            &key,
            r#"[{"modmask":64,"key":"Return","dispatcher":"spawn","arg":"foot","submap":""}]"#,
            "default",
            &[],
            &[],
            false,
            None,
        )
        .unwrap();
        assert!(result.summary.contains("active binding found"));
    }

    #[test]
    fn matching_source_has_no_ipc_runner() {
        let source = include_str!("matching.rs");
        let ipc_call = ["run_", "hyprctl("].concat();
        assert!(!source.contains(&ipc_call));
        let command_constructor = ["Command", "::new"].concat();
        assert!(!source.contains(&command_constructor));
    }
}

fn normalize_device_name(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect()
}

fn deserialize_boolish<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Boolish {
        Bool(bool),
        String(String),
    }

    match Boolish::deserialize(deserializer)? {
        Boolish::Bool(value) => Ok(value),
        Boolish::String(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Boolish::String(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Boolish::String(value) => Err(serde::de::Error::custom(format!(
            "expected true or false, got '{value}'"
        ))),
    }
}
