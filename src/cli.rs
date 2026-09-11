use std::process::ExitCode;

use whykey::layers::{LayerId, LayerResult, LayerStatus, Outcome, Propagation};
use whykey::schema;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobalArguments {
    pub arguments: Vec<String>,
    pub json: bool,
    pub ndjson: bool,
    pub verbose: bool,
    pub schema_version: u8,
}

/// Remove global presentation flags while preserving command-local arguments.
/// Validation is centralized here so every dispatch path sees the same schema
/// and output-mode rules.
pub(crate) fn parse_global_arguments(raw: Vec<String>) -> Result<GlobalArguments, String> {
    let json = raw
        .iter()
        .any(|argument| matches!(argument.as_str(), "--json" | "--json-v2" | "--ndjson"));
    let ndjson = raw.iter().any(|argument| argument == "--ndjson");
    let verbose = raw
        .iter()
        .any(|argument| matches!(argument.as_str(), "--verbose" | "-v"));
    let mut schema_version = schema::DEFAULT_VERSION;
    let mut arguments = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--json" | "--ndjson" | "--verbose" | "-v" => {}
            "--json-v2" => schema_version = 2,
            "--schema-version" => {
                let Some(value) = raw.get(index + 1) else {
                    return Err("--schema-version requires 1 or 2".into());
                };
                schema_version = match value.parse::<u8>() {
                    Ok(version @ (1 | 2)) => version,
                    _ => return Err("--schema-version must be 1 or 2".into()),
                };
                index += 1;
            }
            argument => arguments.push(argument.to_owned()),
        }
        index += 1;
    }
    Ok(GlobalArguments {
        arguments,
        json,
        ndjson,
        verbose,
        schema_version,
    })
}

pub(crate) fn inspection_failed(layers: &[LayerResult], has_explicit_target: bool) -> bool {
    let target_unavailable = layers.iter().any(|layer| {
        layer.id == LayerId::Application && layer.status() == LayerStatus::Unavailable
    });
    if has_explicit_target && target_unavailable {
        return true;
    }
    if layers
        .iter()
        .any(|layer| layer.id == LayerId::Diagnostic && layer.status() == LayerStatus::Unavailable)
    {
        return true;
    }
    let has_handled = layers.iter().any(|layer| {
        layer.status() == LayerStatus::Handled
            || layer.propagation() == Propagation::Stops
            || layer.propagation() == Propagation::Redirected
    });
    if has_handled {
        return false;
    }
    !layers.is_empty()
        && layers
            .iter()
            .all(|layer| matches!(layer.outcome, Outcome::Unavailable))
}

pub(crate) fn exit_for_inspection(layers: &[LayerResult], has_explicit_target: bool) -> ExitCode {
    if inspection_failed(layers, has_explicit_target) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_flags_without_reordering_command_arguments() {
        let parsed = parse_global_arguments(vec![
            "--json-v2".into(),
            "inspect".into(),
            "ctrl+x".into(),
            "--verbose".into(),
        ])
        .unwrap();
        assert_eq!(parsed.arguments, vec!["inspect", "ctrl+x"]);
        assert!(parsed.json);
        assert!(parsed.verbose);
        assert_eq!(parsed.schema_version, 2);
    }

    #[test]
    fn rejects_invalid_schema_versions_at_the_parser_boundary() {
        assert_eq!(
            parse_global_arguments(vec!["--schema-version".into()]).unwrap_err(),
            "--schema-version requires 1 or 2"
        );
        assert_eq!(
            parse_global_arguments(vec!["--schema-version".into(), "3".into()]).unwrap_err(),
            "--schema-version must be 1 or 2"
        );
    }
}
