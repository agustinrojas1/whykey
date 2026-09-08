use std::env;

use crate::key::KeyCombo;
use crate::layers::{LayerId, LayerResult, LayerStatus, Outcome, Propagation};

/// Describe session layers that sit between the terminal and the shell.
///
/// tmux, GNU Screen, and Zellij are intermediate layers between the terminal and shell.
/// Their results are kept separate from the compositor and terminal layers so
/// a report can show where a key was consumed before reaching Readline.
pub fn inspect(key: &KeyCombo) -> LayerResult {
    let mut multiplexers = Vec::new();
    if env::var_os("TMUX").is_some() {
        multiplexers.push("tmux");
    }
    if env::var_os("STY").is_some() {
        multiplexers.push("screen");
    }
    if env::var_os("ZELLIJ").is_some() || env::var_os("ZELLIJ_SESSION_NAME").is_some() {
        multiplexers.push("Zellij");
    }

    let mut details = Vec::new();
    if let Some(term_program) = env::var_os("TERM_PROGRAM") {
        details.push(format!(
            "terminal program: {}",
            term_program.to_string_lossy()
        ));
    }
    if let Some(shell) = env::var_os("SHELL") {
        details.push(format!("shell: {}", shell.to_string_lossy()));
    }
    if let Some(pane) = env::var_os("TMUX_PANE") {
        details.push(format!("tmux pane: {}", pane.to_string_lossy()));
    }
    if let Some(session) = env::var_os("STY") {
        details.push(format!("screen session: {}", session.to_string_lossy()));
    }
    if let Some(pane) = env::var_os("ZELLIJ_PANE_ID") {
        details.push(format!("Zellij pane: {}", pane.to_string_lossy()));
    }
    if let Some(session) = env::var_os("ZELLIJ_SESSION_NAME") {
        details.push(format!("Zellij session: {}", session.to_string_lossy()));
    }
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        details.push("SSH session detected".into());
    }

    match multiplexers.as_slice() {
        [] => {}
        ["tmux"] => {
            let mut result = super::tmux::inspect(key);
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
                    "tmux" => super::tmux::inspect(key),
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
                layer: "Session context",
                id: LayerId::Session,
                outcome: Outcome::from_parts(&status, &propagation),
                summary: "nested multiplexer layers detected; handling order is conditional".into(),
                details,
            };
        }
    }

    LayerResult {
        layer: "Session context",
        id: LayerId::Session,
        outcome: Outcome::Pass,
        summary: "no tmux, GNU Screen, or Zellij session detected".into(),
        details,
    }
}
