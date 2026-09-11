use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, Outcome, Propagation};

/// Normalized Sway/i3 IPC binding: both compositors expose the same
/// event-mask/key JSON shape, so both front ends convert their typed
/// bindings here instead of duplicating the match.
pub(crate) struct IpcBinding {
    pub command: String,
    pub keys: Vec<String>,
    pub input_code: Option<u32>,
    pub keycodes: Vec<u32>,
    pub mask: Option<u32>,
    pub release: bool,
    pub exact: bool,
}

/// Per-desktop strings and display quirks for [`inspect`].
pub(crate) struct TilingMeta {
    pub layer: &'static str,
    pub error_prefix: &'static str,
    pub unavailable_summary: &'static str,
    pub no_match_source: &'static str,
    pub match_source: &'static str,
    pub possible_note: &'static str,
    pub show_release: bool,
}

/// Partition bindings into exact-modmask matches and conditional
/// (possible) matches, skipping release bindings.
pub(crate) fn split_matches<'a>(
    bindings: &'a [IpcBinding],
    key: &KeyCombo,
) -> (Vec<&'a IpcBinding>, Vec<&'a IpcBinding>) {
    let mut exact_matches = Vec::new();
    let mut possible_matches = Vec::new();
    for binding in bindings {
        if binding.release {
            continue;
        }
        if !binding_matches_key(binding, key) {
            continue;
        }
        match binding.mask {
            Some(mask) if mask == key.modmask() => exact_matches.push(binding),
            Some(_) if !binding.exact => possible_matches.push(binding),
            None => possible_matches.push(binding),
            Some(_) => {}
        }
    }
    (exact_matches, possible_matches)
}

/// Outcome from the dispatcher classification of matched bindings.
pub(crate) fn decide_outcome(exact: &[&IpcBinding], possible: &[&IpcBinding]) -> Outcome {
    if exact.is_empty() {
        if possible
            .iter()
            .all(|binding| command_propagation(&binding.command) == Some(Propagation::Continues))
        {
            Outcome::UncertainContinues
        } else {
            Outcome::Unknown
        }
    } else {
        match exact
            .iter()
            .find_map(|binding| command_propagation(&binding.command))
        {
            Some(Propagation::Continues) => Outcome::HandledAndPassed,
            Some(Propagation::Stops) => Outcome::Consumed,
            Some(Propagation::Redirected) => Outcome::Redirected,
            Some(Propagation::Indeterminate) | None => Outcome::HandledUncertain,
        }
    }
}

pub(crate) fn inspect(
    meta: &TilingMeta,
    loaded: Result<Vec<IpcBinding>, String>,
    key: &KeyCombo,
) -> LayerResult {
    let bindings = match loaded {
        Ok(bindings) => bindings,
        Err(error) => {
            return LayerResult::new(
                meta.layer,
                LayerId::Compositor,
                Outcome::Unavailable,
                meta.unavailable_summary,
                vec![error],
            );
        }
    };

    let (exact_matches, possible_matches) = split_matches(&bindings, key);

    if exact_matches.is_empty() && possible_matches.is_empty() {
        return LayerResult::new(
            meta.layer,
            LayerId::Compositor,
            Outcome::Pass,
            "no active binding found",
            vec![meta.no_match_source.to_owned()],
        );
    }

    let mut details = vec![meta.match_source.to_owned()];
    for binding in exact_matches.iter().chain(possible_matches.iter()) {
        details.push(format!(
            "binding: {}{}",
            binding.command,
            if meta.show_release && binding.release {
                " (release)"
            } else {
                ""
            }
        ));
    }
    if !possible_matches.is_empty() {
        details.push(meta.possible_note.to_owned());
    }

    let outcome = decide_outcome(&exact_matches, &possible_matches);
    LayerResult::new(
        meta.layer,
        LayerId::Compositor,
        outcome,
        if exact_matches.is_empty() {
            "possible binding found; modifier matching is conditional".to_owned()
        } else {
            "active binding found".to_owned()
        },
        details,
    )
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

pub(crate) fn command_propagation(command: &str) -> Option<Propagation> {
    let dispatcher = command.split_whitespace().next()?.to_ascii_lowercase();
    Some(match dispatcher.as_str() {
        "nop" => Propagation::Continues,
        "pass" => Propagation::Redirected,
        _ => Propagation::Stops,
    })
}

pub(crate) fn string_values(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) => vec![value.clone()],
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn binding_matches_key(binding: &IpcBinding, key: &KeyCombo) -> bool {
    if let Some(query_code) = key
        .key()
        .strip_prefix("CODE:")
        .and_then(|value| value.parse::<u32>().ok())
    {
        return binding.input_code == Some(query_code) || binding.keycodes.contains(&query_code);
    }
    binding.keys.iter().any(|candidate| {
        candidate
            .parse::<KeyCombo>()
            .map(|combo| combo.key().eq_ignore_ascii_case(key.key()))
            .unwrap_or_else(|_| candidate.eq_ignore_ascii_case(key.key()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: TilingMeta = TilingMeta {
        layer: "test",
        error_prefix: "test: ",
        unavailable_summary: "could not inspect effective test bindings",
        no_match_source: "testmsg -t get_bindings",
        match_source: "source: testmsg -t get_bindings",
        possible_note: "test binding may accept additional modifiers because exact matching is not enabled",
        show_release: true,
    };

    #[test]
    fn reads_keys_from_string_and_array_fields() {
        let keysym = serde_json::json!("Return");
        let symbols = serde_json::json!(["KP_Enter"]);
        let mut keys = string_values(&keysym);
        keys.extend(string_values(&symbols));
        assert_eq!(keys, vec!["Return", "KP_Enter"]);
        assert_eq!(string_values(&serde_json::json!(42)), Vec::<String>::new());
    }

    #[test]
    fn classifies_dispatchers() {
        assert_eq!(command_propagation("nop"), Some(Propagation::Continues));
        assert_eq!(command_propagation("pass"), Some(Propagation::Redirected));
        assert_eq!(command_propagation("exec foo"), Some(Propagation::Stops));
        assert_eq!(
            command_propagation(""),
            None,
            "an empty dispatcher leaves propagation conditional instead of guessing"
        );
    }

    #[test]
    fn partitions_exact_and_possible_matches() {
        let bindings = vec![
            IpcBinding {
                command: "exec exact".into(),
                keys: vec!["c".into()],
                input_code: None,
                keycodes: vec![],
                mask: Some(0),
                release: false,
                exact: true,
            },
            IpcBinding {
                command: "exec loose".into(),
                keys: vec!["c".into()],
                input_code: None,
                keycodes: vec![],
                mask: Some(4),
                release: false,
                exact: false,
            },
            IpcBinding {
                command: "exec unknown-mask".into(),
                keys: vec!["c".into()],
                input_code: None,
                keycodes: vec![],
                mask: None,
                release: false,
                exact: true,
            },
            IpcBinding {
                command: "exec skipped-release".into(),
                keys: vec!["c".into()],
                input_code: None,
                keycodes: vec![],
                mask: Some(0),
                release: true,
                exact: true,
            },
        ];
        let key: KeyCombo = "c".parse().unwrap();
        let (exact, possible) = split_matches(&bindings, &key);
        assert_eq!(
            exact
                .iter()
                .map(|binding| binding.command.as_str())
                .collect::<Vec<_>>(),
            vec!["exec exact"]
        );
        assert_eq!(
            possible
                .iter()
                .map(|binding| binding.command.as_str())
                .collect::<Vec<_>>(),
            vec!["exec loose", "exec unknown-mask"]
        );
    }

    #[test]
    fn matches_input_codes() {
        let bindings = vec![IpcBinding {
            command: "exec coded".into(),
            keys: vec![],
            input_code: Some(36),
            keycodes: vec![36],
            mask: Some(0),
            release: false,
            exact: true,
        }];
        let code: KeyCombo = "code:36".parse().unwrap();
        let (exact, _) = split_matches(&bindings, &code);
        assert_eq!(exact.len(), 1);
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

    #[test]
    fn reports_unavailable_and_pass_through_meta() {
        let key: KeyCombo = "c".parse().unwrap();
        let unavailable = inspect(&META, Err("boom".into()), &key);
        assert_eq!(unavailable.outcome, Outcome::Unavailable);
        assert_eq!(unavailable.details, vec!["boom"]);

        let pass = inspect(&META, Ok(Vec::new()), &key);
        assert_eq!(pass.outcome, Outcome::Pass);
        assert_eq!(pass.details, vec![META.no_match_source]);
    }
}
