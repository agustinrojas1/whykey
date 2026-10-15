//! Replays the reality lab's recordings (`lab/recordings/*.json`) through
//! whykey and checks that each still gives the verdict the lab accepted.
//!
//! A recording holds every answer whykey took from its environment (commands,
//! files, environment variables, directory listings) while the lab checked
//! it against a real program. The replay runs the same whykey arguments with
//! `WHYKEY_REPLAY` set and an empty environment, so nothing outside the
//! recording is read and nothing is started. No Docker is needed and no input
//! is sent. See `docs/lab-replay.md`.
//!
//! The verdict rules are the ones `lab/run.py` applies (`layer_verdict` and
//! `evaluate`): the layer's verdict must be in the accepted set, and the
//! scenario's text checks must hold. Refreshing a recording needs a real lab
//! run: `python3 lab/run.py --record lab/recordings --only <scenario>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// Exit status of a replay whose recording could not be loaded (src/recording.rs).
const SETUP_EXIT: i32 = 96;
/// A replay asked a question the recording holds no answer to.
const UNRECORDED_EXIT: i32 = 97;
/// A replay's recording held answers the run never asked for.
const UNCONSUMED_EXIT: i32 = 98;
/// Set to `1` to make recorded answers that a run never asked for a failure.
const STRICT_VARIABLE: &str = "WHYKEY_REPLAY_STRICT";

fn recordings_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("lab")
        .join("recordings")
}

fn recordings() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(recordings_dir())
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", recordings_dir().display()))
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
}

fn load(path: &Path) -> Value {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
}

fn argv_of(document: &Value) -> Vec<String> {
    document["whykey"]["argv"]
        .as_array()
        .expect("whykey.argv is an array")
        .iter()
        .map(|argument| {
            argument
                .as_str()
                .expect("argv entries are strings")
                .to_owned()
        })
        .collect()
}

/// Run whykey from `recording` with the given arguments, in an empty environment.
fn replay_with(argv: &[String], recording: &Path, strict: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_whykey"));
    command
        .args(argv)
        .env_clear()
        .env("WHYKEY_REPLAY", recording)
        .stdin(Stdio::null());
    if strict {
        command.env(STRICT_VARIABLE, "1");
    }
    command.output().expect("whykey runs")
}

/// Run whykey from `recording`. Strict mode follows the test's own
/// `WHYKEY_REPLAY_STRICT`, so `WHYKEY_REPLAY_STRICT=1 cargo test` fails on
/// recorded answers that a run never asked for.
fn replay(argv: &[String], recording: &Path) -> Output {
    let strict = std::env::var(STRICT_VARIABLE).is_ok_and(|value| value == "1");
    replay_with(argv, recording, strict)
}

/// The path entries of a report: the extension's layer, the `path` of a
/// single-key report, or the last key's path of a key sequence.
fn report_path(report: &Value) -> Vec<&Value> {
    if let Some(layer) = report.pointer("/extension/layer") {
        return vec![layer];
    }
    if let Some(path) = report.get("path").and_then(Value::as_array) {
        return path.iter().collect();
    }
    report
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.last())
        .and_then(|step| step.get("path"))
        .and_then(Value::as_array)
        .map(|path| path.iter().collect())
        .unwrap_or_default()
}

/// The verdict of the first layer named in `layers` (`lab/run.py`:
/// `layer_verdict`), with that layer's entry.
fn layer_verdict<'a>(report: &'a Value, layers: &[String]) -> (String, Option<&'a Value>) {
    for entry in report_path(report) {
        let layer = entry.get("layer").and_then(Value::as_str);
        if !layers.iter().any(|name| Some(name.as_str()) == layer) {
            continue;
        }
        let status = entry.get("status").and_then(Value::as_str);
        let propagation = entry.get("propagation").and_then(Value::as_str);
        let verdict = match (status, propagation) {
            (Some("Handled"), Some("Stops")) => "consumed",
            (Some("Handled"), Some("Continues")) => "handled-passed",
            (Some("NotHandled"), Some("Continues")) => "pass",
            (Some("Handled"), Some("Indeterminate")) => "handled-uncertain",
            (Some("Indeterminate"), Some("Continues")) => "uncertain-continues",
            _ if propagation == Some("Redirected") => "redirected",
            _ if status == Some("Unavailable") => "unavailable",
            _ => "indeterminate",
        };
        return (verdict.to_owned(), Some(entry));
    }
    ("absent".to_owned(), None)
}

/// The summary and evidence texts of a layer entry, summary first.
fn entry_texts(entry: Option<&Value>) -> Vec<String> {
    let Some(entry) = entry else {
        return Vec::new();
    };
    let mut texts = vec![
        entry
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    ];
    for item in entry
        .get("evidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        texts.push(
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        );
    }
    texts
}

/// The text checks of `lab/run.py`'s `evaluate`: summary, detail, binding,
/// stderr and confidence, each only when the scenario states it.
fn text_checks(
    expect: &Value,
    report: &Value,
    stderr: &str,
    entry: Option<&Value>,
) -> Result<(), String> {
    let texts = entry_texts(entry);
    if let Some(summary) = expect["summary"].as_str() {
        if !texts.first().is_some_and(|text| text.contains(summary)) {
            return Err(format!("the layer's summary must contain {summary:?}"));
        }
    }
    if let Some(detail) = expect["detail"].as_str() {
        if !texts.iter().any(|text| text.contains(detail)) {
            return Err(format!("the layer's evidence must contain {detail:?}"));
        }
    }
    if let Some(binding) = expect["binding"].as_str() {
        let action = entry
            .and_then(|entry| entry.pointer("/binding/action"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !action.contains(binding) {
            return Err(format!(
                "the binding action must contain {binding:?} (reported {action:?})"
            ));
        }
    }
    if let Some(text) = expect["stderr"].as_str() {
        if !stderr.contains(text) {
            return Err(format!("stderr must contain {text:?}"));
        }
    }
    if let Some(confidence) = expect["confidence"].as_str() {
        let reported = report
            .pointer("/assessment/confidence")
            .and_then(Value::as_str);
        if reported != Some(confidence) {
            return Err(format!(
                "confidence must be {confidence:?} (reported {reported:?})"
            ));
        }
    }
    Ok(())
}

/// Why this replay does not reproduce the lab's verdict, or None when it does.
fn mismatch(document: &Value, output: &Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if let Some(code) = output.status.code() {
        if matches!(code, SETUP_EXIT | UNRECORDED_EXIT | UNCONSUMED_EXIT) {
            return Some(format!("replay exit {code}: {}", stderr.trim()));
        }
    }
    let report: Value = match serde_json::from_slice(&output.stdout) {
        Ok(report) => report,
        Err(error) => return Some(format!("stdout is not a report ({error}): {stderr}")),
    };
    let layers: Vec<String> = document["expect"]["layers"]
        .as_array()
        .expect("expect.layers is an array")
        .iter()
        .map(|layer| layer.as_str().expect("layer names are strings").to_owned())
        .collect();
    let (verdict, entry) = layer_verdict(&report, &layers);
    let accepted: Vec<&str> = document["expect"]["verdicts"]
        .as_array()
        .expect("expect.verdicts is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if !accepted.contains(&verdict.as_str()) {
        return Some(format!(
            "whykey says {verdict}, the lab accepted {}",
            accepted.join("|")
        ));
    }
    text_checks(&document["expect"], &report, &stderr, entry).err()
}

#[test]
fn every_recording_replays_to_the_accepted_verdict() {
    let paths = recordings();
    assert!(
        !paths.is_empty(),
        "no recordings in {}; make them with `python3 lab/run.py --record lab/recordings`",
        recordings_dir().display()
    );
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    for path in &paths {
        let document = load(path);
        let scenario = document["scenario"].as_str().unwrap_or("?").to_owned();
        let output = replay(&argv_of(&document), path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(line) = stderr.lines().find(|line| line.contains("never requested")) {
            warnings.push(format!("{scenario}: {}", line.trim()));
        }
        if let Some(reason) = mismatch(&document, &output) {
            failures.push(format!("{scenario}: {reason}"));
        }
    }
    for warning in &warnings {
        eprintln!("warning: recorded answers the run never asked for: {warning}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} recording(s) no longer reproduce the lab's verdict:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}

/// Writes a copy of `document` whose inputs `edit` has changed, for a negative test.
fn variant(name: &str, document: &Value, edit: impl FnOnce(&mut Vec<Value>)) -> PathBuf {
    let mut copy = document.clone();
    let mut inputs = copy["inputs"]
        .as_array()
        .expect("inputs is an array")
        .clone();
    edit(&mut inputs);
    copy["inputs"] = Value::Array(inputs);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.json"));
    fs::write(&path, serde_json::to_vec(&copy).expect("serializes")).expect("writes the variant");
    path
}

#[test]
fn a_question_the_recording_lacks_fails_loudly() {
    let path = recordings().into_iter().next().expect("a recording exists");
    let document = load(&path);
    // Without its first answer, the replay reaches that question unanswered.
    let recording = variant("lab-replay-missing-answer", &document, |inputs| {
        inputs.remove(0);
    });
    let output = replay(&argv_of(&document), &recording);
    assert_eq!(
        output.status.code(),
        Some(UNRECORDED_EXIT),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unrecorded input"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "no verdict may be printed for an unrecorded input"
    );
}

#[test]
fn an_unused_answer_is_a_warning_and_fails_only_when_strict() {
    let path = recordings().into_iter().next().expect("a recording exists");
    let document = load(&path);
    let recording = variant("lab-replay-unused-answer", &document, |inputs| {
        inputs.push(serde_json::json!({
            "kind": "env",
            "key": "WHYKEY_LAB_TEST_NEVER_READ",
            "response": null,
        }));
    });
    // Default: the run reports the unused answer and keeps its own verdict.
    let output = replay_with(&argv_of(&document), &recording, false);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("never requested"), "stderr: {stderr}");
    assert_ne!(
        output.status.code(),
        Some(UNCONSUMED_EXIT),
        "stderr: {stderr}"
    );
    assert_eq!(
        mismatch(&document, &output),
        None,
        "an unused answer must not change the verdict"
    );
    // WHYKEY_REPLAY_STRICT=1: the same recording fails.
    let strict = replay_with(&argv_of(&document), &recording, true);
    assert_eq!(
        strict.status.code(),
        Some(UNCONSUMED_EXIT),
        "stderr: {}",
        String::from_utf8_lossy(&strict.stderr)
    );
}

#[test]
fn a_different_verdict_with_the_same_inputs_is_a_mismatch() {
    // Whykey's answer stays the one the recording holds, but the lab's accepted
    // verdict differs: the replay must report the mismatch, not pass.
    let (path, mut document) = recordings()
        .into_iter()
        .map(|path| {
            let document = load(&path);
            (path, document)
        })
        .find(|(_, document)| {
            document["expect"]["verdicts"]
                .as_array()
                .is_some_and(|verdicts| verdicts.iter().any(|verdict| verdict == "consumed"))
        })
        .expect("a recording the lab accepted as consumed");
    let output = replay(&argv_of(&document), &path);
    assert!(
        mismatch(&document, &output).is_none(),
        "the real expectation must pass"
    );
    document["expect"]["verdicts"] = serde_json::json!(["pass"]);
    let reason = mismatch(&document, &output).expect("a different accepted verdict must fail");
    assert!(reason.contains("whykey says consumed"), "reason: {reason}");
    assert!(reason.contains("the lab accepted pass"), "reason: {reason}");
}

/// Source files that may still call the standard library for inputs, with the
/// reason. Each is live-only (its inputs are never recorded) or not recorded
/// yet. Everything else in `src` reaches its inputs through `src/recording.rs`.
const RAW_INPUT_ALLOWED: &[(&str, &str)] = &[
    (
        "src/listen/",
        "live capture: listen reads input devices and sockets while it runs",
    ),
    (
        "src/probe/",
        "live probes: the focus and key probes read the session",
    ),
    (
        "src/hyprland_capture.rs",
        "live capture: Hyprland socket2, used by listen",
    ),
    (
        "src/sway_capture.rs",
        "live capture: Sway IPC event stream, used by listen",
    ),
    ("src/layers/x11_grab.rs", "live X11 grab check"),
    (
        "src/layers/wayfire.rs",
        "not recorded yet: the Wayfire IPC socket",
    ),
];

/// Calls that read something outside whykey, as they are written in `src`.
const RAW_INPUT_CALLS: &[&str] = &[
    "env::var",
    "fs::read",
    "fs::metadata",
    "fs::symlink_metadata",
    "fs::canonicalize",
    "fs::read_link",
    "fs::read_dir",
    "read_dir(",
    "File::open(",
    "OpenOptions::new(",
    "UnixStream::connect(",
    "process::id()",
    "parent_id()",
    "getuid(",
    ".exists()",
    ".canonicalize()",
    ".is_file()",
    ".is_dir()",
];

fn source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("src is readable") {
        let path = entry.expect("readable entry").path();
        if path.is_dir() {
            source_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// The lines of a source file outside its `#[cfg(test)] mod ... { }` blocks.
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut in_test_module = false;
    let mut after_cfg_test = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if !in_test_module && trimmed == "#[cfg(test)]" {
            after_cfg_test = true;
            continue;
        }
        if after_cfg_test {
            after_cfg_test = false;
            if trimmed.starts_with("mod ") && trimmed.ends_with('{') {
                in_test_module = true;
                continue;
            }
        }
        if in_test_module {
            if line == "}" {
                in_test_module = false;
            }
            continue;
        }
        lines.push((index + 1, line));
    }
    lines
}

#[test]
fn raw_input_calls_go_through_the_recording_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    source_files(&root.join("src"), &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .expect("inside the crate")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "src/recording.rs" || relative == "src/replay.rs" {
            continue;
        }
        let allowed = RAW_INPUT_ALLOWED.iter().any(|(entry, _)| {
            relative == *entry || (entry.ends_with('/') && relative.starts_with(entry))
        });
        if allowed {
            continue;
        }
        let text = fs::read_to_string(&file).expect("source is UTF-8");
        for (number, line) in code_lines(&text) {
            let code = line.trim();
            if code.starts_with("//")
                || line.contains("recording::")
                || line.contains("metadata.is_")
            {
                continue;
            }
            if RAW_INPUT_CALLS.iter().any(|call| line.contains(call)) {
                offenders.push(format!("{relative}:{number}: {code}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "raw input calls bypass src/recording.rs (route them through it, or allow the file with a reason \
         in RAW_INPUT_ALLOWED):\n{}",
        offenders.join("\n")
    );
}
