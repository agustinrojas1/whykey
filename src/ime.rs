use std::collections::BTreeMap;
use std::env;
use std::process::Command;

use serde::Serialize;

use crate::command;
use crate::layers::{EvidenceTier, LayerId, LayerResult, Outcome};

/// Read-only input-method context discovered from the current session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Observed,
    Conditional,
    Unobserved,
}

impl EvidenceStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Conditional => "conditional",
            Self::Unobserved => "unobserved",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TextReconstruction {
    /// Text committed by the application cannot be attributed to one key
    /// from a passive terminal/compositor observation.
    pub committed_text: EvidenceStatus,
    /// Compose/dead-key state belongs to the input-method and terminal state
    /// machines, not to the daemon-level engine query.
    pub compose_dead_key: EvidenceStatus,
    /// Application-side preedit is exposed by the client toolkit, not by the
    /// read-only IBus/Fcitx5 controller methods used here.
    pub application_preedit: EvidenceStatus,
}

impl Default for TextReconstruction {
    fn default() -> Self {
        Self {
            committed_text: EvidenceStatus::Conditional,
            compose_dead_key: EvidenceStatus::Unobserved,
            application_preedit: EvidenceStatus::Unobserved,
        }
    }
}

impl TextReconstruction {
    fn detail(&self) -> String {
        format!(
            "text reconstruction: committed text {}, Compose/dead-key history {}, application preedit {}",
            self.committed_text.label(),
            self.compose_dead_key.label(),
            self.application_preedit.label()
        )
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Detection {
    pub engine: String,
    pub sources: Vec<String>,
    pub processes: Vec<String>,
    /// Engine selected by the input-method daemon at observation time, when
    /// its documented read-only D-Bus controller is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_engine: Option<String>,
    /// Fcitx5's daemon state. IBus does not expose an equivalent state in its
    /// small command-line query, so this remains absent for IBus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// A failed runtime query is evidence about observability, not evidence
    /// that the input method is inactive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_error: Option<String>,
    /// The daemon answered that no engine is selected (IBus reports "No
    /// global engine"). Keys then reach the client unchanged; this is an
    /// answer, not a failed query.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_engine: bool,
    /// Typed limits of what a passive diagnostic can reconstruct. Keeping
    /// these separate from `query_error` prevents an unreachable daemon from
    /// being mistaken for an inactive IME or for observed committed text.
    pub text_reconstruction: TextReconstruction,
}

/// Detect IBus/Fcitx5 without opening or changing an input-method connection.
pub fn detect() -> Vec<Detection> {
    let snapshot = crate::util::ProcessSnapshot::collect();
    detect_with_snapshot(&snapshot)
}

pub fn detect_with_snapshot(snapshot: &crate::util::ProcessSnapshot) -> Vec<Detection> {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return Vec::new();
    }
    let mut found: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (variable, value) in [
        ("GTK_IM_MODULE", env::var("GTK_IM_MODULE").ok()),
        ("QT_IM_MODULE", env::var("QT_IM_MODULE").ok()),
        ("XMODIFIERS", env::var("XMODIFIERS").ok()),
    ] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let value_lower = value.to_ascii_lowercase();
        let engine = if value_lower.contains("fcitx") {
            "fcitx5"
        } else if value_lower.contains("ibus") {
            "ibus"
        } else {
            continue;
        };
        found
            .entry(engine.into())
            .or_default()
            .0
            .push(format!("{variable}={value}"));
    }
    for process in snapshot.comm_names() {
        let engine = process_engine(&process);
        if let Some(engine) = engine {
            found.entry(engine.into()).or_default().1.push(process);
        }
    }
    found
        .into_iter()
        .map(|(engine, (mut sources, mut processes))| {
            sources.sort();
            sources.dedup();
            processes.sort();
            processes.dedup();
            let runtime = query_runtime(&engine);
            Detection {
                engine,
                sources,
                processes,
                active_engine: runtime.active_engine,
                state: runtime.state,
                query_error: runtime.error,
                no_engine: runtime.no_engine,
                text_reconstruction: TextReconstruction::default(),
            }
        })
        .collect()
}

/// Describe the input-method stage without claiming it consumed a key. An IME
/// can turn several keys into committed text, but a terminal-side capture
/// cannot reconstruct the original Compose/dead-key sequence from that text.
pub fn inspect() -> LayerResult {
    let detections = detect();
    inspect_with_detections(&detections)
}

/// Describe a previously collected input-method snapshot without rerunning
/// process scans or runtime queries.
pub fn inspect_with_detections(detections: &[Detection]) -> LayerResult {
    if detections.is_empty() {
        return LayerResult::new(
            "Input method",
            LayerId::Ime,
            Outcome::Pass,
            "no supported input method was detected",
            Vec::new(),
        );
    }

    let mut details = Vec::new();
    let mut transforming = false;
    let mut unknown = false;
    let mut forwarding_reasons = Vec::new();
    for detection in detections {
        details.push(format!("detected: {}", detection.engine));
        details.extend(
            detection
                .sources
                .iter()
                .map(|source| format!("source: {source}")),
        );
        details.extend(
            detection
                .processes
                .iter()
                .map(|process| format!("process: {process}")),
        );
        if let Some(active_engine) = &detection.active_engine {
            details.push(format!("active engine: {active_engine}"));
        }
        if detection.no_engine {
            details.push("active engine: none (the daemon reports no engine is set)".into());
        }
        if let Some(state) = &detection.state {
            if state == "closed" && detection.active_engine.is_none() {
                details.push("runtime state: closed (no focused input context)".into());
            } else {
                details.push(format!("runtime state: {state}"));
            }
        }
        if let Some(error) = &detection.query_error {
            details.push(format!("runtime query unavailable: {error}"));
        }
        match runtime_mode(detection) {
            RuntimeMode::Forwarding(reason) => forwarding_reasons.push(reason),
            RuntimeMode::Transforming => transforming = true,
            RuntimeMode::Unknown => unknown = true,
        }
    }
    details.push(
        detections
            .iter()
            .map(|detection| detection.text_reconstruction.detail())
            .collect::<Vec<_>>()
            .join("; "),
    );
    // Only a daemon's runtime state answers for it; environment variables
    // and process names merely show that one is present.
    let tier = detections
        .iter()
        .any(|detection| detection.state.is_some())
        .then_some(EvidenceTier::LiveQuery);
    if transforming {
        return LayerResult::new(
            "Input method",
            LayerId::Ime,
            Outcome::UncertainContinues,
            "an active input method may transform this input before text is committed",
            details,
        )
        .with_optional_tier(tier);
    }
    if unknown {
        return LayerResult::new(
            "Input method",
            LayerId::Ime,
            Outcome::UncertainContinues,
            if detections
                .iter()
                .any(|detection| detection.query_error.is_some())
            {
                "input method was detected but its runtime API is unreachable; transformation state remains unknown"
            } else {
                "an input-method context may transform this input before text is committed"
            },
            details,
        )
        .with_optional_tier(tier);
    }
    // Every detected daemon reported a runtime state that forwards keys
    // unchanged. Its own global hotkeys (for example the trigger key) are
    // not read, so say so instead of implying they were checked.
    details.push("input-method hotkeys (trigger/switch keys) were not inspected".into());
    LayerResult::new(
        "Input method",
        LayerId::Ime,
        Outcome::Pass,
        format!(
            "input method was detected but {}; keys pass through unchanged",
            forwarding_reasons.join(" and ")
        ),
        details,
    )
    .with_optional_tier(tier)
}

enum RuntimeMode {
    /// Runtime state proves the daemon forwards keys; the payload says why.
    Forwarding(String),
    /// An input method is engaged and may compose or transform keys.
    Transforming,
    /// The runtime state could not be read.
    Unknown,
}

fn runtime_mode(detection: &Detection) -> RuntimeMode {
    if detection.no_engine {
        return RuntimeMode::Forwarding("no engine is set".into());
    }
    if let Some(state) = detection
        .state
        .as_deref()
        .filter(|state| matches!(*state, "inactive" | "closed"))
    {
        return RuntimeMode::Forwarding(format!("is {state}"));
    }
    match detection.active_engine.as_deref() {
        Some(engine) if is_keyboard_engine(engine) => RuntimeMode::Forwarding(format!(
            "its active engine {engine} is a plain keyboard layout"
        )),
        Some(_)
            if detection.state.as_deref() != Some("active") && detection.query_error.is_some() =>
        {
            RuntimeMode::Unknown
        }
        Some(_) => RuntimeMode::Transforming,
        None if detection.state.as_deref() == Some("active") => RuntimeMode::Transforming,
        None => RuntimeMode::Unknown,
    }
}

/// Engines that only apply an XKB layout: Fcitx5's `keyboard-<layout>` and
/// IBus's `xkb:<layout>:<variant>:<language>`.
fn is_keyboard_engine(engine: &str) -> bool {
    engine.starts_with("keyboard-") || engine.starts_with("xkb:")
}

#[derive(Default)]
struct RuntimeQuery {
    active_engine: Option<String>,
    state: Option<String>,
    error: Option<String>,
    no_engine: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Query documented read-only input-method APIs only after an input-method
/// process or session variable identified the daemon. These queries never
/// switch, activate, or deactivate an input method.
fn query_runtime(engine: &str) -> RuntimeQuery {
    match engine {
        "ibus" => query_ibus(),
        "fcitx5" => query_fcitx5(),
        _ => RuntimeQuery::default(),
    }
}

fn query_ibus() -> RuntimeQuery {
    let mut command = Command::new("ibus");
    // Untranslated replies, so "No engine is set" below is recognized.
    command.arg("engine").env("LC_ALL", "C");
    let output = match command::output(&mut command) {
        Ok(output) => output,
        Err(error) => {
            return RuntimeQuery {
                error: Some(format!("ibus: {error}")),
                ..RuntimeQuery::default()
            };
        }
    };
    if output.status.success() {
        return RuntimeQuery {
            active_engine: nonempty_line(&String::from_utf8_lossy(&output.stdout)),
            ..RuntimeQuery::default()
        };
    }
    // The daemon answered and selected nothing: `ibus engine` exits 1 with
    // "No engine is set." (stdout) and "No global engine" (stderr). That is a
    // reply, not an unreachable API.
    if ibus_reports_no_engine(&output) {
        return RuntimeQuery {
            no_engine: true,
            ..RuntimeQuery::default()
        };
    }
    RuntimeQuery {
        error: Some(command_failure("ibus", &output)),
        ..RuntimeQuery::default()
    }
}

fn ibus_reports_no_engine(output: &std::process::Output) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("No engine is set") || stderr.contains("No global engine")
}

fn query_fcitx5() -> RuntimeQuery {
    // Some distro builds of fcitx5-remote throw an uncaught C++ exception
    // when the session bus cannot be created. Probe the bus and query the
    // documented controller methods directly so a diagnostic never needs to
    // launch that crash-prone helper.
    if let Err(error) = fcitx5_service_available() {
        return RuntimeQuery {
            error: Some(error),
            ..RuntimeQuery::default()
        };
    }

    let active_engine = match run_fcitx5_method("CurrentInputMethod") {
        // Fcitx5 answers "" when no input context is focused; State then
        // reports 0 (closed), so keep querying instead of giving up.
        Ok(output) => match parse_dbus_string(&output.stdout) {
            Some(engine) => (!engine.is_empty()).then_some(engine),
            None => {
                return RuntimeQuery {
                    error: Some("Fcitx5 CurrentInputMethod returned no D-Bus string value".into()),
                    ..RuntimeQuery::default()
                };
            }
        },
        Err(error) => {
            return RuntimeQuery {
                error: Some(error),
                ..RuntimeQuery::default()
            };
        }
    };

    match run_fcitx5_method("State") {
        Ok(output) => {
            let state =
                parse_dbus_integer(&output.stdout).and_then(|code| fcitx5_state(Some(code)));
            let error = if state.is_none() {
                Some(command_failure("dbus-send", &output))
            } else {
                None
            };
            RuntimeQuery {
                active_engine,
                state,
                error,
                ..RuntimeQuery::default()
            }
        }
        Err(error) => RuntimeQuery {
            active_engine,
            error: Some(error),
            ..RuntimeQuery::default()
        },
    }
}

fn run_fcitx5_method(method: &str) -> Result<std::process::Output, String> {
    let mut command = Command::new("dbus-send");
    command.args([
        "--session",
        "--print-reply=literal",
        "--dest=org.fcitx.Fcitx5",
        "/controller",
    ]);
    command.arg(format!("org.fcitx.Fcitx.Controller1.{method}"));
    let output = command::output(&mut command)
        .map_err(|error| format!("dbus-send Fcitx5 {method}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_failure("dbus-send", &output))
    }
}

fn fcitx5_service_available() -> Result<(), String> {
    if env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return Err("D-Bus session address is unset; skipped Fcitx5 runtime query".into());
    }

    let mut command = Command::new("dbus-send");
    command.args([
        "--session",
        "--print-reply=literal",
        "--dest=org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.ListNames",
    ]);
    let output = command::output(&mut command).map_err(|error| {
        format!("D-Bus preflight failed; skipped Fcitx5 runtime query: {error}")
    })?;
    if !output.status.success() {
        return Err(format!(
            "D-Bus session is unavailable; skipped Fcitx5 runtime query ({})",
            command_failure("dbus-send", &output)
        ));
    }

    let names = String::from_utf8_lossy(&output.stdout);
    if !has_fcitx5_service(&names) {
        return Err(
            "org.fcitx.Fcitx5 is not registered on the session bus; skipped Fcitx5 runtime query"
                .into(),
        );
    }
    Ok(())
}

fn has_fcitx5_service(names: &str) -> bool {
    names
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        })
        .any(|name| name == "org.fcitx.Fcitx5")
}

fn command_failure(program: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!("{program} exited with {}", output.status)
    } else {
        format!("{program} exited with {}: {detail}", output.status)
    }
}

fn nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_dbus_string(value: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(value);
    // `--print-reply=literal` prints an empty string as blank output.
    let Some(line) = value.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return Some(String::new());
    };
    let payload = if let Some(payload) = line.strip_prefix("string ") {
        payload.trim()
    } else {
        // `--print-reply=literal` emits a bare string value (for example
        // `keyboard-us`) but keeps scalar non-string replies typed. Reject
        // those typed values so a malformed State reply cannot be mistaken
        // for an input-method name.
        let type_name = line.split_whitespace().next()?;
        if matches!(
            type_name,
            "array"
                | "boolean"
                | "byte"
                | "double"
                | "int16"
                | "int32"
                | "int64"
                | "object"
                | "signature"
                | "uint16"
                | "uint32"
                | "uint64"
                | "variant"
        ) {
            return None;
        }
        line
    };
    let inner = payload
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(payload);
    Some(inner.replace("\\\\", "\\").replace("\\\"", "\""))
}

fn parse_dbus_integer(value: &[u8]) -> Option<i32> {
    let text = String::from_utf8_lossy(value);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut fields = line.split_whitespace();
    let first = fields.next()?;
    let payload = match first {
        "byte" | "int16" | "int32" | "int64" | "uint16" | "uint32" | "uint64" => fields.next()?,
        _ if fields.next().is_none() => first,
        _ => return None,
    };
    fields
        .next()
        .is_none()
        .then(|| payload.parse().ok())
        .flatten()
}

fn fcitx5_state(code: Option<i32>) -> Option<String> {
    match code {
        Some(0) => Some("closed".into()),
        Some(1) => Some("inactive".into()),
        Some(2) => Some("active".into()),
        _ => None,
    }
}

fn process_engine(process: &str) -> Option<&'static str> {
    if process.eq_ignore_ascii_case("fcitx5") {
        Some("fcitx5")
    } else if process.eq_ignore_ascii_case("ibus-daemon") {
        Some("ibus")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_shape_is_stable_and_serializable() {
        let detection = Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: Vec::new(),
            active_engine: Some("xkb:us::eng".into()),
            state: None,
            query_error: None,
            no_engine: false,
            text_reconstruction: TextReconstruction::default(),
        };
        let value = serde_json::to_value(&detection).unwrap();
        assert_eq!(value["engine"], "ibus");
        assert!(value["sources"].is_array());
        assert_eq!(value["active_engine"], "xkb:us::eng");
        assert!(value.get("state").is_none());
        assert_eq!(
            value["text_reconstruction"]["committed_text"],
            "conditional"
        );
        assert_eq!(
            value["text_reconstruction"]["compose_dead_key"],
            "unobserved"
        );
        assert_eq!(
            value["text_reconstruction"]["application_preedit"],
            "unobserved"
        );
    }

    #[test]
    fn only_daemon_processes_trigger_runtime_queries() {
        assert_eq!(process_engine("fcitx5"), Some("fcitx5"));
        assert_eq!(process_engine("FCITX5"), Some("fcitx5"));
        assert_eq!(process_engine("fcitx5-remote"), None);
        assert_eq!(process_engine("ibus-daemon"), Some("ibus"));
        assert_eq!(process_engine("ibus-ui-gtk3"), None);
    }

    #[test]
    fn fcitx5_exit_codes_are_read_only_state_observations() {
        assert_eq!(fcitx5_state(Some(0)).as_deref(), Some("closed"));
        assert_eq!(fcitx5_state(Some(1)).as_deref(), Some("inactive"));
        assert_eq!(fcitx5_state(Some(2)).as_deref(), Some("active"));
        assert_eq!(fcitx5_state(Some(3)), None);
    }

    fn fcitx5(active_engine: &str, state: &str) -> Detection {
        Detection {
            engine: "fcitx5".into(),
            sources: vec!["GTK_IM_MODULE=fcitx".into()],
            processes: vec!["fcitx5".into()],
            active_engine: Some(active_engine.into()),
            state: Some(state.into()),
            query_error: None,
            no_engine: false,
            text_reconstruction: TextReconstruction::default(),
        }
    }

    #[test]
    fn inactive_fcitx_forwards_keys_with_evidence() {
        let result = inspect_with_detections(&[fcitx5("pinyin", "inactive")]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("is inactive"));
        assert!(result.summary.contains("keys pass through unchanged"));
        assert!(!result.summary.contains("active input method"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail == "runtime state: inactive")
        );
    }

    #[test]
    fn plain_keyboard_engine_forwards_keys_even_when_active() {
        let result = inspect_with_detections(&[fcitx5("keyboard-us", "active")]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(
            result
                .summary
                .contains("keyboard-us is a plain keyboard layout")
        );
    }

    #[test]
    fn unreachable_runtime_api_is_distinguished_from_inactive_state() {
        let result = inspect_with_detections(&[Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec![],
            active_engine: None,
            state: None,
            query_error: Some("session bus unavailable".into()),
            no_engine: false,
            text_reconstruction: TextReconstruction::default(),
        }]);
        assert!(result.summary.contains("runtime API is unreachable"));
        // The environment shows an input method exists; nothing answered.
        assert_eq!(result.tier, None);
        assert_eq!(
            inspect_with_detections(&[fcitx5("pinyin", "inactive")]).tier,
            Some(crate::layers::EvidenceTier::LiveQuery)
        );
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("session bus"))
        );
    }

    #[test]
    fn renders_the_fcitx5_state_matrix_without_stringly_typed_evidence() {
        for (state, outcome) in [
            ("active", Outcome::UncertainContinues),
            ("inactive", Outcome::Pass),
            ("closed", Outcome::Pass),
        ] {
            let result = inspect_with_detections(&[fcitx5("pinyin", state)]);
            assert_eq!(result.outcome, outcome, "state {state}");
            assert!(result.details.iter().any(|detail| {
                detail.contains("text reconstruction:") && detail.contains("conditional")
            }));
        }
    }

    #[test]
    fn renders_ibus_active_and_unreachable_as_distinct_evidence() {
        let ibus = |active_engine: &str| Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec!["ibus-daemon".into()],
            active_engine: Some(active_engine.into()),
            state: None,
            query_error: None,
            no_engine: false,
            text_reconstruction: TextReconstruction::default(),
        };
        let active = inspect_with_detections(&[ibus("libpinyin")]);
        assert!(active.summary.contains("active input method"));
        assert_eq!(active.outcome, Outcome::UncertainContinues);

        let layout = inspect_with_detections(&[ibus("xkb:us::eng")]);
        assert_eq!(layout.outcome, Outcome::Pass);

        let unreachable = inspect_with_detections(&[Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec![],
            active_engine: None,
            state: None,
            query_error: Some("session bus unavailable".into()),
            no_engine: false,
            text_reconstruction: TextReconstruction::default(),
        }]);
        assert!(unreachable.summary.contains("runtime API is unreachable"));
        assert_eq!(unreachable.outcome, Outcome::UncertainContinues);
        assert!(
            unreachable
                .details
                .iter()
                .any(|detail| { detail.contains("application preedit unobserved") })
        );
    }

    fn ibus_no_engine(no_engine: bool) -> Detection {
        Detection {
            engine: "ibus".into(),
            sources: vec!["GTK_IM_MODULE=ibus".into()],
            processes: vec!["ibus-daemon".into()],
            active_engine: None,
            state: None,
            query_error: None,
            no_engine,
            text_reconstruction: TextReconstruction::default(),
        }
    }

    #[test]
    fn ibus_without_a_global_engine_forwards_keys() {
        // `ibus engine` answers "No global engine": the daemon is reachable
        // and no engine is selected, so keys pass unchanged. It is not an
        // unreachable runtime API.
        let result = inspect_with_detections(&[ibus_no_engine(true)]);

        assert_eq!(result.outcome, Outcome::Pass);
        assert!(result.summary.contains("no engine is set"));
        assert!(!result.summary.contains("unreachable"));
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.starts_with("active engine: none"))
        );
    }

    #[test]
    fn ibus_engine_query_reads_no_engine_as_an_answer() {
        use std::os::unix::process::ExitStatusExt;

        let no_engine = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: b"No engine is set.\n".to_vec(),
            stderr: b"(process): IBUS-WARNING: Properties.Get: No global engine.\n".to_vec(),
        };
        assert!(ibus_reports_no_engine(&no_engine));

        let unreachable = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"Cannot connect to ibus-daemon\n".to_vec(),
        };
        assert!(!ibus_reports_no_engine(&unreachable));
    }

    #[test]
    fn ignores_blank_runtime_query_output() {
        assert_eq!(nonempty_line(" \n\n "), None);
        assert_eq!(
            nonempty_line("\n keyboard-us \n"),
            Some("keyboard-us".into())
        );
    }

    #[test]
    fn matches_only_the_exact_fcitx5_service_name() {
        assert!(has_fcitx5_service(
            "array [ string \"org.fcitx.Fcitx5\", string \":1.7\" ]"
        ));
        assert!(!has_fcitx5_service("org.fcitx.Fcitx5-helper"));
        assert!(!has_fcitx5_service("org.example.org.fcitx.Fcitx5"));
    }

    #[test]
    fn parses_read_only_fcitx5_dbus_replies() {
        assert_eq!(
            parse_dbus_string(b"string \"keyboard-us\"\n"),
            Some("keyboard-us".into())
        );
        assert_eq!(
            parse_dbus_string(b"   keyboard-us\n"),
            Some("keyboard-us".into())
        );
        assert_eq!(parse_dbus_string(b"int32 2\n"), None);
        assert_eq!(parse_dbus_string(b"string \"\"\n"), Some(String::new()));
        assert_eq!(parse_dbus_string(b"   "), Some(String::new()));
        assert_eq!(parse_dbus_integer(b"int32 2\n"), Some(2));
        assert_eq!(parse_dbus_integer(b"2\n"), Some(2));
        assert_eq!(parse_dbus_integer(b"reply 2\n"), None);
        assert_eq!(parse_dbus_integer(b"string \"2\"\n"), None);
        assert_eq!(
            fcitx5_state(parse_dbus_integer(b"int32 2\n")),
            Some("active".into())
        );
    }
}
