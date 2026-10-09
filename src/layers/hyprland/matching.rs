pub(crate) fn analyze(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[LuaBindHint],
    config_hints: &[ConfigBindHint],
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    inspect_json_with_keycode(
        key,
        bindings_json,
        active_submap,
        keycodes,
        lua_hints,
        config_hints,
        physical_input,
    )
}

#[cfg(test)]
fn inspect_json(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
) -> Result<LayerResult, String> {
    inspect_json_with_keycode(key, bindings_json, active_submap, &[], &[], &[], None)
}

pub(crate) fn inspect_json_with_keycode(
    key: &KeyCombo,
    bindings_json: &str,
    active_submap: &str,
    keycodes: &[crate::xkb::XkbKeycode],
    lua_hints: &[LuaBindHint],
    config_hints: &[ConfigBindHint],
    physical_input: Option<&PhysicalInput>,
) -> Result<LayerResult, String> {
    let (bindings, skipped_bindings) = parse_bindings(bindings_json)?;
    let active_submap = normalize_submap(active_submap);
    let keyed_bindings: Vec<_> = bindings
        .iter()
        .map(|binding| (binding, BindingKey::of(binding, lua_hints)))
        .collect();

    // Keep same-key bindings from other submaps visible. They do not handle
    // the current event, but hiding them makes mode-dependent configurations
    // look as if the key is unbound everywhere.
    let inactive_bindings: Vec<_> = keyed_bindings
        .iter()
        .filter(|(binding, _)| !binding_is_active(binding, active_submap))
        .filter(|(_, binding_key)| binding_key.matches(key, keycodes, physical_input) == Some(true))
        .filter(|(binding, _)| {
            physical_device_match(&binding.device, physical_input) != DeviceMatch::NoMatch
        })
        .map(|(binding, _)| *binding)
        .collect();

    let matches: Vec<_> = keyed_bindings
        .iter()
        .filter(|(binding, _)| binding_is_active(binding, active_submap))
        .filter_map(|(binding, binding_key)| {
            let device_match = physical_device_match(&binding.device, physical_input);
            if device_match == DeviceMatch::NoMatch {
                return None;
            }
            let certainty = match binding_key.matches(key, keycodes, physical_input) {
                Some(false) => return None,
                // Only the modifiers of an unresolved-key binding are known,
                // so it remains a candidate for exactly those modifiers.
                None if binding.modmask == key.modmask() => MatchCertainty::PossibleUnresolvedKey,
                None => return None,
                Some(true) if binding.modmask == key.modmask() => MatchCertainty::Exact,
                Some(true) => {
                    match binding_ignore_mods(binding, binding_key, lua_hints, config_hints) {
                        Some(true) => MatchCertainty::ModifierInsensitive,
                        Some(false) => return None,
                        None => MatchCertainty::PossibleIgnoreMods,
                    }
                }
            };
            // Without a physical capture we cannot know whether a per-device
            // binding applies. A captured evdev device has already passed
            // that filter, so retain modifier certainty.
            let device_unproven = has_device_scope(&binding.device)
                && (physical_input.is_none() || device_match == DeviceMatch::Unknown);
            Some(BindingMatch {
                binding,
                certainty: if device_unproven && certainty != MatchCertainty::PossibleUnresolvedKey
                {
                    MatchCertainty::PossibleDevice
                } else {
                    certainty
                },
            })
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
    if matches.iter().any(|matched| {
        matched.certainty == MatchCertainty::ModifierInsensitive
            && boolish_value(&matched.binding.ignore_mods).is_none()
    }) {
        details.push(
            "the static config declares ignore_mods for this exact binding; hyprctl does not report it"
                .into(),
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

    let resolved_matches = || {
        matches
            .iter()
            .filter(|matched| matched.certainty != MatchCertainty::PossibleUnresolvedKey)
    };
    for matched in resolved_matches() {
        let label = match matched.certainty {
            MatchCertainty::Exact => "binding",
            MatchCertainty::PossibleIgnoreMods => "possible modifier-insensitive binding",
            MatchCertainty::ModifierInsensitive => "modifier-insensitive binding",
            MatchCertainty::PossibleDevice => "possible device-specific binding",
            MatchCertainty::PossibleUnresolvedKey => continue,
        };
        details.push(format!(
            "{label}: {} ({})",
            binding_action(matched.binding),
            binding_scope(matched.binding, active_submap)
        ));
    }
    let unresolved: Vec<_> = matches
        .iter()
        .filter(|matched| matched.certainty == MatchCertainty::PossibleUnresolvedKey)
        .map(|matched| matched.binding)
        .collect();
    let only_unresolved = resolved_matches().next().is_none();

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

    if resolved_matches().any(|matched| {
        matches!(
            matched.certainty,
            MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
        ) && dispatcher_is_opaque(matched.binding)
            && !matched.binding.non_consuming
            && !matched.binding.auto_consuming
    }) {
        // Hyprland suppresses the key for every matching bind that is not
        // non-consuming; only a callback returning `pass_event` forwards it.
        details.push(
            "Hyprland consumes the key for this binding unless its callback explicitly returns pass_event."
                .into(),
        );
    }
    if resolved_matches().any(|matched| dispatcher_is_opaque(matched.binding)) {
        details.push(
            "Lua/plugin dispatcher is active; its runtime action is not executed by whykey.".into(),
        );
    }

    let propagation = if skipped_bindings > 0 {
        Propagation::Indeterminate
    } else {
        binding_propagation(&matches)
    };
    if !unresolved.is_empty() {
        let example = unresolved
            .iter()
            .find_map(|binding| none_if_empty(&binding.description))
            .map(|description| format!(" (e.g. '{description}')"))
            .unwrap_or_default();
        let count = format!(
            "Hyprland reports {} binding{} with unresolved keys for these modifiers{example}",
            unresolved.len(),
            if unresolved.len() == 1 { "" } else { "s" }
        );
        // A consuming exact match already decides the route; the unresolved
        // bindings then only matter as possible extra actions.
        if has_exact_match && propagation == Propagation::Stops {
            verbose_details.push(format!("{count}; they may also fire"));
        } else {
            details.push(count);
            details.push(
                "hyprctl reports neither a key name nor a keycode for them (for example Lua binds whose key is built at runtime), so whykey cannot tell which key they use"
                    .into(),
            );
        }
        verbose_details.extend(unresolved.iter().map(|binding| {
            format!(
                "binding with unresolved key: {} ({})",
                binding_action(binding),
                binding_scope(binding, active_submap)
            )
        }));
    }
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
            (LayerStatus::Indeterminate, _) if only_unresolved => {
                "bindings with unresolved keys may handle this key"
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
    let binding = match primary_match(&matches) {
        Some(matched) => {
            let mut evidence = binding_evidence(matched.binding, lua_hints, has_universal_match);
            if evidence.uncertainty.is_none()
                && matched.certainty == MatchCertainty::PossibleIgnoreMods
            {
                evidence.uncertainty = Some(crate::layers::UncertaintyReason::ModifierAmbiguity);
            }
            Some(evidence)
        }
        // Name no single unresolved binding: any of them may be the one
        // bound to this key, so the evidence only states the candidate count.
        None => {
            let consumes_if_matched = unresolved.iter().all(|binding| {
                binding_propagation(&[BindingMatch {
                    binding,
                    certainty: MatchCertainty::Exact,
                }]) == Propagation::Stops
            });
            Some(
                BindingEvidence::action(
                    format!(
                        "one of {} binding{} whose key Hyprland does not report",
                        unresolved.len(),
                        if unresolved.len() == 1 { "" } else { "s" }
                    ),
                    BindingScope::Submap(active_submap.to_owned()),
                    None,
                )
                .with_uncertainty(
                    crate::layers::UncertaintyReason::UnresolvedBindingKey {
                        candidates: unresolved.len(),
                        consumes_if_matched,
                    },
                ),
            )
        }
    };
    // With only unresolved candidates, the summary states what is unknown:
    // which of these bindings (if any) uses the requested key.
    let summary = if only_unresolved && skipped_bindings == 0 {
        let modifiers = crate::style::human_modifiers(key.modmask());
        let example = unresolved
            .iter()
            .find_map(|binding| none_if_empty(&binding.description))
            .map(|description| format!(" (e.g. '{description}')"))
            .unwrap_or_default();
        format!(
            "{} {}binding{} {} keys hyprctl does not report{example}",
            unresolved.len(),
            if modifiers.is_empty() {
                String::new()
            } else {
                format!("{} ", modifiers.join("+"))
            },
            if unresolved.len() == 1 { "" } else { "s" },
            if unresolved.len() == 1 { "has" } else { "have" },
        )
    } else {
        summary.to_owned()
    };

    Ok({
        let mut result =
            LayerResult::new("Hyprland", LayerId::Compositor, outcome, summary, details)
                .with_verbose_details(verbose_details);
        result.binding = binding;
        result
    })
}

/// The match the conclusion describes: an exact binding when one exists,
/// otherwise the first candidate whose key is known. Evidence never invents
/// a stronger match, so an unresolved-key binding is never named alone.
fn primary_match<'a>(matches: &'a [BindingMatch<'a>]) -> Option<BindingMatch<'a>> {
    matches
        .iter()
        .find(|matched| {
            matches!(
                matched.certainty,
                MatchCertainty::Exact | MatchCertainty::ModifierInsensitive
            )
        })
        .or_else(|| {
            matches
                .iter()
                .find(|matched| matched.certainty != MatchCertainty::PossibleUnresolvedKey)
        })
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
/// description and modifier match. Anything weaker would be a guessed file
/// hint; the last declaration wins, as it does in the config itself.
fn hint_source(binding: &Binding, lua_hints: &[LuaBindHint]) -> Option<SourceLocation> {
    let description = none_if_empty(&binding.description)?;
    let hint = lua_hints.iter().rev().find(|hint| {
        hint.combo.modmask() == binding.modmask
            && hint.description.as_deref() == Some(description.as_str())
    })?;
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
    /// Same modifiers, but hyprctl reports no key and no config hint names it.
    PossibleUnresolvedKey,
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

/// The key a runtime binding listens for, normalized like a parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BindingKey {
    Code(crate::xkb::XkbKeycode),
    /// Normalized key name: hyprctl reports XKB keysym names such as
    /// `comma` or `SLASH`, which must compare equal to `,` and `/`.
    Name(String),
    CatchAll,
    /// hyprctl reports neither a key name nor a keycode (Lua binds whose
    /// `code:` key is built at runtime) and no literal config hint names it.
    Unresolved,
}

impl BindingKey {
    fn of(binding: &Binding, lua_hints: &[LuaBindHint]) -> Self {
        if binding.keycode != crate::xkb::XkbKeycode::from(0) {
            Self::Code(binding.keycode)
        } else if binding.catch_all {
            Self::CatchAll
        } else if !binding.key.is_empty() {
            Self::from_name(&binding.key)
        } else {
            lua_hint_key(binding.modmask, &binding.description, lua_hints)
                .map_or(Self::Unresolved, Self::from_name)
        }
    }

    fn from_name(name: &str) -> Self {
        let normalized = crate::key::normalize_key_str(name);
        match normalized
            .strip_prefix("CODE:")
            .and_then(|code| code.parse::<u32>().ok())
        {
            Some(code) => Self::Code(crate::xkb::XkbKeycode::from(code)),
            None => Self::Name(normalized),
        }
    }

    /// Whether this binding fires for `combo`; `None` when the binding's key
    /// is unknown.
    fn matches(
        &self,
        combo: &KeyCombo,
        keycodes: &[crate::xkb::XkbKeycode],
        physical_input: Option<&PhysicalInput>,
    ) -> Option<bool> {
        let query_code = combo
            .key()
            .strip_prefix("CODE:")
            .and_then(|code| code.parse::<u32>().ok())
            .map(crate::xkb::XkbKeycode::from);
        Some(match (self, query_code) {
            (Self::Unresolved, _) => return None,
            (Self::CatchAll, _) => true,
            // An explicit numeric query is already an assertion about the
            // binding's code; do not let a separately captured physical
            // event broaden it to another code.
            (Self::Code(code), Some(query)) => *code == query,
            (Self::Name(_), Some(_)) => false,
            (Self::Code(code), None) => {
                keycodes.contains(code)
                    || physical_input
                        .is_some_and(|input| physical_keycode_matches(*code, input.keycode))
            }
            (Self::Name(name), None) => name == combo.key(),
        })
    }
}

/// `ignore_mods` for one binding: the runtime value when hyprctl reports it,
/// otherwise the static declaration of exactly this binding.
fn binding_ignore_mods(
    binding: &Binding,
    key: &BindingKey,
    lua_hints: &[LuaBindHint],
    config_hints: &[ConfigBindHint],
) -> Option<bool> {
    boolish_value(&binding.ignore_mods)
        .or_else(|| lua_ignore_mods_for_binding(binding, key, lua_hints))
        .or_else(|| config_ignore_mods_for_binding(binding, key, config_hints))
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
            MatchCertainty::PossibleIgnoreMods
                | MatchCertainty::PossibleDevice
                | MatchCertainty::PossibleUnresolvedKey
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

    // Opaque Lua/plugin dispatchers included: Hyprland suppresses the key for
    // every consuming bind; only a callback returning `pass_event` (never
    // executed by whykey) would forward it.
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
            &[],
            None,
        )
        .unwrap();
        assert!(result.summary.contains("active binding found"));
    }

    fn inspect(key: &str, bindings: &str, keycodes: &[u32], hints: &[LuaBindHint]) -> LayerResult {
        let key: KeyCombo = key.parse().unwrap();
        let keycodes: Vec<_> = keycodes
            .iter()
            .copied()
            .map(crate::xkb::XkbKeycode::from)
            .collect();
        inspect_json_with_keycode(&key, bindings, "default", &keycodes, hints, &[], None).unwrap()
    }

    #[test]
    fn consuming_lua_binding_consumes_the_key() {
        let consuming = r#"[{"modmask": 64, "key": "RETURN", "dispatcher": "__lua",
            "arg": "268", "description": "Terminal", "non_consuming": false}]"#;
        let result = inspect("super+return", consuming, &[], &[]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            result
                .binding
                .as_ref()
                .and_then(|binding| binding.description.as_deref()),
            Some("Terminal")
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("not executed by whykey"))
        );

        let non_consuming =
            consuming.replace(r#""non_consuming": false"#, r#""non_consuming": true"#);
        assert_eq!(
            inspect("super+return", &non_consuming, &[], &[]).outcome,
            Outcome::HandledAndPassed
        );
        // Auto-consuming depends on the callback's success, which whykey
        // cannot observe.
        let auto = consuming.replace(r#""non_consuming": false"#, r#""auto_consuming": true"#);
        assert_eq!(
            inspect("super+return", &auto, &[], &[]).outcome,
            Outcome::HandledUncertain
        );
    }

    #[test]
    fn keysym_named_binding_keys_match_normalized_queries() {
        let bindings = r#"[
            {"modmask": 64, "key": "comma", "dispatcher": "__lua", "arg": "1",
             "description": "Dismiss last notification"},
            {"modmask": 64, "key": "SLASH", "dispatcher": "__lua", "arg": "2",
             "description": "Monitor scaling up"},
            {"modmask": 64, "key": "minus", "dispatcher": "__lua", "arg": "3",
             "description": "Shrink"},
            {"modmask": 0, "key": "Prior", "dispatcher": "__lua", "arg": "4",
             "description": "Page"}
        ]"#;
        for (key, description) in [
            ("super+comma", "Dismiss last notification"),
            ("super+,", "Dismiss last notification"),
            ("super+slash", "Monitor scaling up"),
            ("super+minus", "Shrink"),
            ("pageup", "Page"),
        ] {
            let result = inspect(key, bindings, &[], &[]);
            assert_eq!(result.outcome, Outcome::Consumed, "{key}");
            assert_eq!(
                result.binding.and_then(|binding| binding.description),
                Some(description.to_owned()),
                "{key}"
            );
        }
    }

    const WORKSPACE_BINDS: &str = r#"[
        {"modmask": 64, "key": "", "keycode": 0, "dispatcher": "__lua", "arg": "71",
         "description": "Switch to workspace 1"},
        {"modmask": 64, "key": "", "keycode": 0, "dispatcher": "__lua", "arg": "77",
         "description": "Switch to workspace 2"},
        {"modmask": 64, "key": "", "keycode": 0, "dispatcher": "__lua", "arg": "169",
         "description": "Expand window left"}
    ]"#;

    fn tiling_hints() -> Vec<LuaBindHint> {
        parse_lua_bind_hints(
            "o.bind(\"SUPER + code:20\", \"Expand window left\", hl.dsp.window.resize({ x = -100 }))\n\
             for workspace = 1, 10 do\n\
               local key = \"code:\" .. tostring(workspace + 9)\n\
               o.bind(\"SUPER + \" .. key, \"Switch to workspace \" .. workspace, hl.dsp.focus({}))\n\
             end\n",
            Path::new("/usr/share/omarchy/default/hypr/bindings/tiling.lua"),
        )
    }

    #[test]
    fn empty_key_binding_uses_a_literal_lua_code_hint() {
        // XKB keycode 20 is the minus key on the active layout.
        let result = inspect("super+minus", WORKSPACE_BINDS, &[20], &tiling_hints());
        assert_eq!(result.outcome, Outcome::Consumed);
        let binding = result
            .binding
            .expect("the resolved binding is the evidence");
        assert_eq!(binding.description.as_deref(), Some("Expand window left"));
        assert!(binding.source.is_some());
    }

    #[test]
    fn unresolved_empty_key_bindings_keep_the_compositor_uncertain() {
        // Keycode 10 is `1`: the resolved `code:20` bind does not match, the
        // dynamic workspace binds cannot be ruled out.
        let result = inspect("super+1", WORKSPACE_BINDS, &[10], &tiling_hints());
        assert_eq!(result.outcome, Outcome::Unknown);
        assert_eq!(
            result.summary,
            "2 Super bindings have keys hyprctl does not report (e.g. 'Switch to workspace 1')"
        );
        assert!(result.details.iter().any(|detail| detail
            == "Hyprland reports 2 bindings with unresolved keys for these modifiers (e.g. 'Switch to workspace 1')"));
        let evidence = result
            .binding
            .expect("the candidate is reported so the route stops being blamed downstream");
        assert!(evidence.description.is_none(), "no single binding is named");
        assert_eq!(
            evidence.uncertainty,
            Some(crate::layers::UncertaintyReason::UnresolvedBindingKey {
                candidates: 2,
                consumes_if_matched: true,
            })
        );

        // Other modifiers cannot be one of them.
        assert_eq!(
            inspect("alt+1", WORKSPACE_BINDS, &[10], &tiling_hints()).outcome,
            Outcome::Pass
        );

        // A consuming exact bind still decides the route.
        let mut with_exact: Vec<serde_json::Value> = serde_json::from_str(WORKSPACE_BINDS).unwrap();
        with_exact.push(serde_json::json!({"modmask": 64, "key": "RETURN",
            "dispatcher": "__lua", "arg": "268", "description": "Terminal"}));
        let result = inspect(
            "super+return",
            &serde_json::to_string(&with_exact).unwrap(),
            &[36],
            &tiling_hints(),
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(
            result.binding.and_then(|binding| binding.description),
            Some("Terminal".to_owned())
        );
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
