use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, LayerStatus, Outcome, Propagation};

/// Describe session layers that sit between the terminal and the shell.
///
/// tmux, GNU Screen, and Zellij are intermediate layers between the terminal and shell.
/// Their results are kept separate from the compositor and terminal layers so
/// a report can show where a key was consumed before reaching Readline.
pub fn inspect(key: &KeyCombo) -> LayerResult {
    let env = crate::environment::Environment::collect();
    inspect_with_env(key, true, &env)
}

pub fn inspect_with_byte_status(key: &KeyCombo, bytes_verified: bool) -> LayerResult {
    let env = crate::environment::Environment::collect();
    inspect_with_env(key, bytes_verified, &env)
}

pub fn inspect_with_env(
    key: &KeyCombo,
    bytes_verified: bool,
    env: &crate::environment::Environment,
) -> LayerResult {
    let mut multiplexers = Vec::new();
    if env.tmux {
        multiplexers.push("tmux");
    }
    if env.screen {
        multiplexers.push("screen");
    }
    if env.zellij {
        multiplexers.push("Zellij");
    }

    let mut details = Vec::new();
    if let Some(term_program) = &env.term_program {
        details.push(format!("terminal program: {term_program}"));
    }
    if !env.shell.is_empty() {
        details.push(format!("shell: {}", env.shell));
    }
    if let Some(pane) = &env.tmux_pane {
        details.push(format!("tmux pane: {pane}"));
    }
    if let Some(session) = &env.screen_session {
        details.push(format!("screen session: {session}"));
    }
    if let Some(pane) = &env.zellij_pane {
        details.push(format!("Zellij pane: {pane}"));
    }
    if let Some(session) = &env.zellij_session {
        details.push(format!("Zellij session: {session}"));
    }
    if env.ssh {
        details.push("SSH session detected".into());
    }

    match multiplexers.as_slice() {
        [] => {}
        ["tmux"] => {
            let mut result = super::tmux::inspect_with_byte_status(key, bytes_verified);
            result.details.splice(0..0, details);
            return result;
        }
        ["screen"] => {
            let mut result = super::screen::inspect(key);
            result.details.splice(0..0, details);
            return result;
        }
        ["Zellij"] => {
            let mut result = super::zellij::inspect(key);
            result.details.splice(0..0, details);
            return result;
        }
        _ => {
            let results = multiplexers
                .iter()
                .map(|multiplexer| match *multiplexer {
                    "tmux" => super::tmux::inspect_with_byte_status(key, bytes_verified),
                    "screen" => super::screen::inspect(key),
                    "Zellij" => super::zellij::inspect(key),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            details.push(format!(
                "nested multiplexers detected: {}",
                multiplexers.join(" + ")
            ));
            details.push(
                "environment variables do not prove nesting order; each session layer was inspected independently".into(),
            );
            for result in &results {
                details.push(format!(
                    "{}: {:?} / {:?} — {}",
                    result.layer,
                    result.status(),
                    result.propagation(),
                    result.summary
                ));
                details.extend(
                    result
                        .details
                        .iter()
                        .map(|detail| format!("{} detail: {detail}", result.layer)),
                );
            }
            let status = if results
                .iter()
                .any(|result| result.status() == LayerStatus::Indeterminate)
            {
                LayerStatus::Indeterminate
            } else if results
                .iter()
                .any(|result| result.status() == LayerStatus::Unavailable)
            {
                LayerStatus::Unavailable
            } else if results
                .iter()
                .any(|result| result.status() == LayerStatus::Handled)
            {
                LayerStatus::Handled
            } else {
                LayerStatus::NotHandled
            };
            let propagation = if results
                .iter()
                .all(|result| result.status() == LayerStatus::NotHandled)
            {
                Propagation::Continues
            } else {
                Propagation::Indeterminate
            };
            return LayerResult {
                verbose_details: Vec::new(),
                binding: None,
                layer: "Session context",
                id: LayerId::Session,
                outcome: Outcome::from_parts(&status, &propagation),
                summary: "nested multiplexer layers detected; handling order is conditional".into(),
                details,
            };
        }
    }

    LayerResult {
        verbose_details: Vec::new(),
        binding: None,
        layer: "Session context",
        id: LayerId::Session,
        outcome: Outcome::Pass,
        summary: "no tmux, GNU Screen, or Zellij session detected".into(),
        details,
    }
}
