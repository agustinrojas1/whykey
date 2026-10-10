//! Observed X11 passive key grabs on the root window.
//!
//! X11 window managers and hotkey daemons claim a global shortcut with a
//! passive grab on the root window, and the X server refuses a second
//! client's grab on the same window, key and modifiers with `BadAccess`.
//! Asking the server to grab the combination on the root window therefore
//! observes whether some client owns it there, independent of which program
//! that is or where its configuration lives. A grab an application holds on
//! its own window is invisible to this probe. In an X11 session that
//! observation decides the compositor-tier outcome; the selected desktop
//! adapter keeps its identity (binding, command, mode) when it names the
//! grab's owner, and a configured binding without a root-window grab is
//! reported as inactive.
//!
//! Under XWayland the grab covers X11 clients only: the Wayland compositor's
//! own bindings are not X grabs, and an X client's grab fires only while an
//! X11 window has keyboard focus.

use std::env;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{EvidenceTier, LayerId, LayerResult, Outcome};
use crate::probe::x11::{Connection, GrabCheck, GrabStatus};

const LAYER: &str = "X11 key grab";
const GRABBED: &str =
    "an X11 client holds a grab on this shortcut on the root window (WM or hotkey daemon)";
const FREE: &str = "no grab on the root window (WMs and hotkey daemons grab there)";

/// Which display server the grab probe would talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Session {
    /// No local X server to ask.
    None,
    /// A native X11 session: the X server routes every key.
    X11,
    /// XWayland inside a Wayland session: grabs affect X11 clients only.
    XWayland,
}

fn session() -> Session {
    // Unit tests that inspect a whole chain must never grab keys on the
    // developer's live X server; the lab exercises the real probe.
    if cfg!(test) {
        return Session::None;
    }
    let display = env_value("DISPLAY").is_some();
    let remote = super::compositor::remote_session();
    session_from(
        display,
        env_value("XDG_SESSION_TYPE").as_deref(),
        remote,
        wayland_live,
    )
}

/// Whether the X server behind `DISPLAY` is XWayland inside a Wayland
/// session, by the same rule the grab probe uses.
pub fn xwayland_session() -> bool {
    session_from(
        true,
        env_value("XDG_SESSION_TYPE").as_deref(),
        false,
        wayland_live,
    ) == Session::XWayland
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

/// Whether the Wayland socket accepts connections. Only consulted when the
/// session type does not decide, so a usual session never connects to it.
fn wayland_live() -> bool {
    crate::probe::wayland::socket_path().is_ok_and(|path| UnixStream::connect(path).is_ok())
}

/// The logind session type decides when it names X11 or Wayland: an
/// inherited `WAYLAND_DISPLAY` (sudo -E, a previous login) must not turn a
/// native X11 session into XWayland. Otherwise a Wayland socket that accepts
/// connections makes `DISPLAY` XWayland, whose grabs cover X11 clients only.
fn session_from(
    display: bool,
    session_type: Option<&str>,
    remote: bool,
    wayland_live: impl FnOnce() -> bool,
) -> Session {
    if !display || remote {
        return Session::None;
    }
    match session_type {
        Some(kind) if kind.eq_ignore_ascii_case("x11") => Session::X11,
        Some(kind) if kind.eq_ignore_ascii_case("wayland") => Session::XWayland,
        _ if wayland_live() => Session::XWayland,
        _ => Session::X11,
    }
}

/// What the X server reported for one key combination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observation {
    /// One check per keycode producing the key.
    Checked(Vec<GrabCheck>),
    /// No keycode in the server keymap produces the key.
    NoKeycode,
    Failed(String),
}

/// Asks the X server under the diagnostic budget. The blocking X protocol
/// round trips run on a helper thread so a server that accepts the
/// connection but stalls cannot overrun the deadline; a late answer is
/// discarded and its connection (with any probe grab) closes when it ends.
fn observe(key: &KeyCombo) -> Observation {
    let key = key.clone();
    bounded(move || probe(&key))
}

fn bounded(probe: impl FnOnce() -> Observation + Send + 'static) -> Observation {
    let Some(budget) = command::query_budget() else {
        return Observation::Failed("the diagnostic time budget was spent".into());
    };
    let (sender, receiver) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("whykey-x11-grab".into())
        .spawn(move || {
            let _ = sender.send(probe());
        });
    if let Err(error) = spawned {
        return Observation::Failed(format!("could not start the probe: {error}"));
    }
    receiver.recv_timeout(budget).unwrap_or_else(|_| {
        Observation::Failed(format!(
            "the X server did not answer within {} ms",
            budget.as_millis()
        ))
    })
}

fn probe(key: &KeyCombo) -> Observation {
    let run = || -> std::io::Result<Observation> {
        let mut connection = Connection::connect_from_env()?;
        let targets = connection.key_targets(key)?;
        if targets.is_empty() {
            return Ok(Observation::NoKeycode);
        }
        let modmask = u16::try_from(key.modmask() & 0xff).unwrap_or_default();
        let mut checks = Vec::with_capacity(targets.len());
        for target in targets {
            checks.push(connection.check_grab(target.keycode, modmask | target.level_modifiers)?);
        }
        Ok(Observation::Checked(checks))
    };
    run().unwrap_or_else(|error| Observation::Failed(error.to_string()))
}

/// Refines the compositor-tier finding with the X server's grab state.
pub fn refine(key: &KeyCombo, adapter: LayerResult) -> LayerResult {
    match session() {
        Session::None => adapter,
        Session::X11 => x11_result(adapter, observe(key)),
        Session::XWayland if adapter.outcome == Outcome::Consumed => adapter,
        Session::XWayland => xwayland_result(adapter, observe(key)),
    }
}

fn modifier_names(mask: u16) -> String {
    const NAMES: [&str; 8] = [
        "Shift", "Lock", "Control", "Mod1", "Mod2", "Mod3", "Mod4", "Mod5",
    ];
    let names = NAMES
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "no modifiers".into()
    } else {
        names.join("+")
    }
}

fn masks(masks: &[u16]) -> String {
    masks
        .iter()
        .map(|mask| modifier_names(*mask))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Observed evidence lines for each checked keycode.
fn evidence(checks: &[GrabCheck]) -> Vec<String> {
    let mut details = Vec::new();
    for check in checks {
        let target = format!(
            "keycode {} with {}",
            check.keycode,
            modifier_names(check.modmask)
        );
        if !check.core_conflicts.is_empty() {
            details.push(format!(
                "observed: the X server refused a core GrabKey on the root window for {target} (BadAccess) for {}",
                masks(&check.core_conflicts)
            ));
        }
        match &check.xi2_conflicts {
            Some(conflicts) if !conflicts.is_empty() => details.push(format!(
                "observed: the X server refused an XInput2 passive grab on the root window for {target} (BadAccess) for {}",
                masks(conflicts)
            )),
            Some(_) => {}
            None => details.push(
                "XInput2 is unavailable on this server; only core-protocol grabs were checked"
                    .into(),
            ),
        }
        if check.status() == GrabStatus::Free {
            details.push(format!(
                "observed: root-window grabs on {target} (also with Lock/NumLock) succeeded and were released; a grab an application holds on its own window is not visible here"
            ));
        }
    }
    details
}

fn claims(adapter: &LayerResult) -> bool {
    matches!(
        adapter.outcome,
        Outcome::Consumed | Outcome::HandledAndPassed | Outcome::HandledUncertain
    )
}

/// Combines the selected desktop adapter's finding with the observed grab
/// state. The grab decides the outcome; the adapter keeps its identity (and
/// binding, command, mode) whenever it names the owner of an observed grab
/// or the layer of a free key. A grab no adapter claims is reported under
/// the neutral "X11 key grab" layer instead of being attributed to the WM.
fn x11_result(mut adapter: LayerResult, observation: Observation) -> LayerResult {
    let checks = match observation {
        Observation::Checked(checks) => checks,
        Observation::NoKeycode => {
            return with_detail(
                adapter,
                "X11 grab probe: no keycode in the X server keymap produces this key; grab state unknown",
            );
        }
        Observation::Failed(error) => {
            return with_detail(adapter, &format!("X11 grab probe unavailable: {error}"));
        }
    };
    let grabbed = checks
        .iter()
        .any(|check| check.status() == GrabStatus::GrabbedByOtherClient);
    let mut details = evidence(&checks);
    match (grabbed, claims(&adapter)) {
        (true, true) => {
            if adapter.outcome != Outcome::Consumed {
                details.push(format!(
                    "{} configuration: {}",
                    adapter.layer, adapter.summary
                ));
                adapter.summary = GRABBED.into();
            }
            adapter.outcome = Outcome::Consumed;
            details.append(&mut adapter.details);
            adapter.details = details;
            adapter.with_tier(EvidenceTier::LiveQuery)
        }
        (true, false) => {
            let mut verbose = vec![format!(
                "{} adapter: {} ({:?})",
                adapter.layer, adapter.summary, adapter.outcome
            )];
            verbose.extend(adapter.details);
            verbose.extend(adapter.verbose_details);
            LayerResult::consumed(LAYER, LayerId::Compositor, GRABBED, details)
                .with_verbose_details(verbose)
                .with_tier(EvidenceTier::LiveQuery)
        }
        (false, claimed) => {
            if claimed {
                details.push(format!(
                    "{} reported '{}', but no X client holds a grab on the root window, so that binding is not active",
                    adapter.layer, adapter.summary
                ));
                details.extend(
                    adapter
                        .details
                        .iter()
                        .map(|detail| format!("{} (inactive): {detail}", adapter.layer)),
                );
            } else {
                adapter
                    .verbose_details
                    .push(format!("{} adapter: {}", adapter.layer, adapter.summary));
                adapter
                    .verbose_details
                    .extend(std::mem::take(&mut adapter.details));
            }
            LayerResult::pass(adapter.layer, LayerId::Compositor, FREE, details)
                .with_verbose_details(adapter.verbose_details)
                .with_tier(EvidenceTier::LiveQuery)
        }
    }
}

/// Under XWayland a grab says nothing about the Wayland compositor, so only
/// an observed grab is worth a note; the compositor finding stays as is.
fn xwayland_result(mut adapter: LayerResult, observation: Observation) -> LayerResult {
    if let Observation::Checked(checks) = observation {
        if checks
            .iter()
            .any(|check| check.status() == GrabStatus::GrabbedByOtherClient)
        {
            adapter.details.push(
                "an XWayland client grabs this shortcut; it fires only while an X11 window has keyboard focus"
                    .into(),
            );
            adapter.verbose_details.extend(evidence(&checks));
        }
    }
    adapter
}

fn with_detail(mut adapter: LayerResult, detail: &str) -> LayerResult {
    adapter.details.push(detail.to_owned());
    adapter
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(core: Vec<u16>, xi2: Option<Vec<u16>>) -> GrabCheck {
        GrabCheck {
            keycode: 28,
            modmask: 64,
            core_conflicts: core,
            xi2_conflicts: xi2,
        }
    }

    fn i3_claim() -> LayerResult {
        LayerResult::consumed(
            "i3",
            LayerId::Compositor,
            "i3 binding consumes the key",
            vec!["binding: exec touch /tmp/fired".into()],
        )
    }

    #[test]
    fn observed_grab_confirms_the_claiming_adapter() {
        let result = x11_result(
            i3_claim(),
            Observation::Checked(vec![check(vec![64, 66], Some(Vec::new()))]),
        );
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.layer, "i3");
        assert_eq!(result.summary, "i3 binding consumes the key");
        assert!(result.details[0].contains("keycode 28 with Mod4"));
        assert!(result.details[0].contains("Mod4, Lock+Mod4"));
        assert!(
            result
                .details
                .contains(&"binding: exec touch /tmp/fired".to_owned())
        );

        // A static "conditional" claim is settled by the observed grab.
        let xbindkeys = LayerResult::new(
            "X11 xbindkeys",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching xbindkeys shortcut configured",
            vec!["binding: touch /tmp/fired".into()],
        );
        let result = x11_result(xbindkeys, Observation::Checked(vec![check(vec![64], None)]));
        assert_eq!(result.layer, "X11 xbindkeys");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.summary, GRABBED);
    }

    #[test]
    fn free_grab_passes_even_when_config_claims_the_key() {
        let result = x11_result(
            i3_claim(),
            Observation::Checked(vec![check(Vec::new(), None)]),
        );
        assert_eq!(result.layer, "i3");
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(result.summary, FREE);
        assert!(result.binding.is_none());
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("only core-protocol grabs"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("binding is not active"))
        );
    }

    #[test]
    fn unclaimed_grab_is_not_attributed_to_the_adapter() {
        let result = x11_result(
            LayerResult::pass("i3", LayerId::Compositor, "no binding", Vec::new()),
            Observation::Checked(vec![check(Vec::new(), Some(vec![64]))]),
        );
        assert_eq!(result.layer, LAYER);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.details[0].contains("XInput2"));
    }

    #[test]
    fn unobservable_keeps_adapter_finding() {
        let result = x11_result(i3_claim(), Observation::Failed("DISPLAY is not set".into()));
        assert_eq!(result.layer, "i3");
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result
                .details
                .last()
                .unwrap()
                .contains("DISPLAY is not set")
        );
        let result = x11_result(i3_claim(), Observation::NoKeycode);
        assert!(
            result
                .details
                .last()
                .unwrap()
                .contains("grab state unknown")
        );
    }

    #[test]
    fn xwayland_grab_is_only_a_conditional_note() {
        let sway = || LayerResult::pass("Sway", LayerId::Compositor, "no binding", Vec::new());
        let result = xwayland_result(sway(), Observation::Checked(vec![check(vec![64], None)]));
        assert_eq!(result.layer, "Sway");
        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.details[0].starts_with("an XWayland client grabs"));

        // A free grab or an unreachable XWayland says nothing about the
        // compositor: the finding is left untouched.
        for observation in [
            Observation::Checked(vec![check(Vec::new(), Some(Vec::new()))]),
            Observation::NoKeycode,
            Observation::Failed("XWayland is not running".into()),
        ] {
            assert_eq!(xwayland_result(sway(), observation), sway());
        }
    }

    #[test]
    fn session_type_decides_before_the_wayland_socket() {
        let live = || true;
        let dead = || false;
        // An inherited WAYLAND_DISPLAY never overrides an X11 session type,
        // and the socket is not even consulted.
        assert_eq!(
            session_from(true, Some("x11"), false, || panic!("socket consulted")),
            Session::X11
        );
        assert_eq!(
            session_from(true, Some("wayland"), false, dead),
            Session::XWayland
        );
        // Without a decisive session type, only a live Wayland socket makes
        // DISPLAY XWayland.
        assert_eq!(session_from(true, None, false, live), Session::XWayland);
        assert_eq!(
            session_from(true, Some("tty"), false, live),
            Session::XWayland
        );
        assert_eq!(session_from(true, None, false, dead), Session::X11);
        assert_eq!(session_from(false, Some("x11"), false, live), Session::None);
        assert_eq!(session_from(true, Some("x11"), true, live), Session::None);
    }

    #[test]
    fn probe_runs_under_the_diagnostic_deadline() {
        let spent = command::with_deadline(std::time::Duration::ZERO, || {
            bounded(|| panic!("probe started after the budget was spent"))
        });
        assert!(matches!(spent, Observation::Failed(error) if error.contains("budget")));

        let started = std::time::Instant::now();
        let stalled = command::with_deadline(std::time::Duration::from_millis(50), || {
            bounded(|| {
                thread::sleep(std::time::Duration::from_secs(2));
                Observation::NoKeycode
            })
        });
        assert!(matches!(stalled, Observation::Failed(error) if error.contains("did not answer")));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn grab_state_is_a_live_query_and_failures_keep_the_adapter_tier() {
        let configured = || i3_claim().with_tier(EvidenceTier::ConfigFile);
        for observation in [
            Observation::Checked(vec![check(vec![64], None)]),
            Observation::Checked(vec![check(Vec::new(), Some(Vec::new()))]),
        ] {
            assert_eq!(
                x11_result(configured(), observation).tier,
                Some(EvidenceTier::LiveQuery)
            );
        }
        let unclaimed = x11_result(
            LayerResult::pass("i3", LayerId::Compositor, "no binding", Vec::new()),
            Observation::Checked(vec![check(vec![64], None)]),
        );
        assert_eq!(unclaimed.tier, Some(EvidenceTier::LiveQuery));
        assert_eq!(
            x11_result(configured(), Observation::Failed("refused".into())).tier,
            Some(EvidenceTier::ConfigFile)
        );
    }

    #[test]
    fn free_verdict_names_the_root_window_scope() {
        let result = x11_result(
            i3_claim(),
            Observation::Checked(vec![check(Vec::new(), Some(Vec::new()))]),
        );
        assert!(result.summary.contains("root window"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("its own window is not visible"))
        );
    }
}
