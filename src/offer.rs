//! After an uncertain static report, `whykey KEY` offers to capture the key:
//! the answer to "where does it stop?" is to watch it, not to guess.

use std::io::IsTerminal as _;

use whykey::layers::LayerResult;
use whykey::listen;
use whykey::report;
use whykey::style::{ColorChoice, RenderOptions};

/// Why `whykey KEY` must not stop to ask a question.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CaptureOffer {
    /// `--no-prompt`, or a target (`--pid`, `--focused`, `--instance`) that
    /// a capture in this terminal would not describe.
    declined: bool,
    json: bool,
    /// stdin and stdout are both a terminal.
    interactive: bool,
    disabled_by_environment: bool,
}

impl CaptureOffer {
    /// The situation of this run. The environment is read only when stdin and
    /// stdout are both terminals, so a recorded or piped run never touches it.
    pub(crate) fn detect(declined: bool, json: bool) -> Self {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        Self {
            declined,
            json,
            interactive,
            disabled_by_environment: interactive && prompt_disabled_by_environment(),
        }
    }
}

/// `WHYKEY_NO_PROMPT` (any non-empty value) and `CI` switch the offer off,
/// the same way `NO_COLOR` switches color off.
fn prompt_disabled_by_environment() -> bool {
    ["WHYKEY_NO_PROMPT", "CI"]
        .iter()
        .any(|name| whykey::recording::env_var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Whether to stop and offer a capture after the static report.
fn capture_offer_applies(offer: CaptureOffer, steps: usize, report_is_unsure: bool) -> bool {
    steps == 1
        && report_is_unsure
        && offer.interactive
        && !offer.json
        && !offer.declined
        && !offer.disabled_by_environment
}

/// An uncertain static answer is where capture earns its keep: offer to watch
/// the key on its way instead of leaving the user to guess.
pub(crate) fn offer_capture(
    sequence: &whykey::key::KeySequence,
    reports: &[Vec<LayerResult>],
    offer: CaptureOffer,
    color: ColorChoice,
    verbose: bool,
    schema_version: u8,
) {
    let (Some(key), Some(layers)) = (sequence.as_slice().first(), reports.first()) else {
        return;
    };
    // A modifier pressed alone has no key to wait for.
    let unsure = !key.is_alone() && report::needs_confirmation(key, layers);
    if !capture_offer_applies(offer, reports.len(), unsure) {
        return;
    }
    let text = RenderOptions::cli(color, false);
    println!(
        "{}",
        text.heading(format!(
            "Press {} now to see where it stops (Esc to skip).",
            whykey::style::human_key(key)
        ))
    );
    let options = listen::Options {
        timeout: Some(std::time::Duration::from_secs(60)),
        verbose,
        color,
        schema_version,
        ..listen::Options::default()
    };
    if let Err(error) = listen::run(options) {
        eprintln!("whykey listen: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interactive() -> CaptureOffer {
        CaptureOffer {
            interactive: true,
            ..CaptureOffer::default()
        }
    }

    #[test]
    fn an_unsure_single_key_on_a_terminal_is_offered_a_capture() {
        assert!(capture_offer_applies(interactive(), 1, true));
    }

    #[test]
    fn the_offer_never_interrupts_scripts_json_or_a_decline() {
        let unsure = true;
        let piped = CaptureOffer::default();
        assert!(!capture_offer_applies(piped, 1, unsure), "not a terminal");
        let json = CaptureOffer {
            json: true,
            ..interactive()
        };
        assert!(!capture_offer_applies(json, 1, unsure));
        let declined = CaptureOffer {
            declined: true,
            ..interactive()
        };
        assert!(!capture_offer_applies(declined, 1, unsure));
        let environment = CaptureOffer {
            disabled_by_environment: true,
            ..interactive()
        };
        assert!(!capture_offer_applies(environment, 1, unsure));
    }

    #[test]
    fn a_settled_answer_or_a_sequence_is_not_offered_a_capture() {
        assert!(!capture_offer_applies(interactive(), 1, false));
        assert!(!capture_offer_applies(interactive(), 2, true));
    }
}
