use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::layers::{LayerId, LayerResult, Outcome, PhysicalInput};

const MAX_PROCESS_ENTRIES: usize = 4096;
const MAX_CONFIG_FILES: usize = 64;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Detection {
    pub name: String,
    pub processes: Vec<String>,
    pub configurations: Vec<String>,
    pub transformations: Vec<String>,
}

/// Detect common Linux remappers without opening or changing input devices.
/// A process/configuration hit is evidence that a transformation may happen
/// before the compositor; it is not proof of the active runtime graph.
pub fn detect() -> Vec<Detection> {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        return Vec::new();
    }
    let mut detections: BTreeMap<String, Detection> = BTreeMap::new();
    for (name, process_names, config_env, defaults) in known_remappers() {
        let mut detection = Detection {
            name: name.to_owned(),
            processes: Vec::new(),
            configurations: Vec::new(),
            transformations: Vec::new(),
        };
        collect_processes(process_names, &mut detection.processes);
        let config_paths = config_candidates(config_env, defaults);
        for path in config_paths {
            if !path.is_file() {
                continue;
            }
            detection.configurations.push(path.display().to_string());
            detection.transformations.extend(match name {
                "keyd" => parse_keyd_transformations(&path),
                "kanata" | "kmonad" => parse_lisp_transformations(&path),
                "input-remapper" => parse_input_remapper_transformations(&path),
                "xremap" => parse_xremap_transformations(&path),
                _ => Vec::new(),
            });
        }
        if !detection.processes.is_empty() || !detection.configurations.is_empty() {
            detection.processes.sort();
            detection.configurations.sort();
            detection.transformations.sort();
            detection.transformations.dedup();
            detections.insert(name.to_owned(), detection);
        }
    }
    detections.into_values().collect()
}

pub fn inspect(physical_input: Option<&PhysicalInput>) -> LayerResult {
    let detections = detect();
    inspect_with_detections(physical_input, &detections)
}

/// Describe a previously collected remapper snapshot without rediscovering
/// processes or configuration files.
pub fn inspect_with_detections(
    physical_input: Option<&PhysicalInput>,
    detections: &[Detection],
) -> LayerResult {
    if detections.is_empty() {
        return LayerResult::new(
            "Input remapper",
            LayerId::Remapper,
            Outcome::Pass,
            "no supported pre-compositor remapper detected",
            Vec::new(),
        );
    }
    let mut details = Vec::new();
    for detection in detections {
        details.push(format!("detected: {}", detection.name));
        details.extend(
            detection
                .processes
                .iter()
                .map(|process| format!("process: {process}")),
        );
        details.extend(
            detection
                .configurations
                .iter()
                .map(|configuration| format!("config: {configuration}")),
        );
        details.extend(
            detection
                .transformations
                .iter()
                .map(|transformation| format!("static transformation: {transformation}")),
        );
    }
    if let Some(input) = physical_input {
        let source_desc = input.device.as_deref().unwrap_or("compositor capture");
        details.push(format!(
            "captured evdev input {} from {source_desc}; remapper routing remains unverified",
            input.keycode
        ));
    }
    details.push(
        "virtual-device routing, timing-dependent transforms, and firmware changes remain unknown"
            .into(),
    );
    LayerResult::new(
        "Input remapper",
        LayerId::Remapper,
        Outcome::UncertainContinues,
        "a pre-compositor remapper may transform this input",
        details,
    )
}

fn known_remappers() -> [(
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static [&'static str],
); 5] {
    [
        (
            "keyd",
            &["keyd"],
            "WHYKEY_KEYD_CONFIG",
            &["/etc/keyd", "~/.config/keyd"],
        ),
        (
            "kanata",
            &["kanata"],
            "KANATA_CONFIG",
            &["~/.config/kanata"],
        ),
        (
            "kmonad",
            &["kmonad"],
            "KMONAD_CONFIG",
            &["~/.config/kmonad"],
        ),
        (
            "input-remapper",
            &["input-remapper", "input-remapper-service"],
            "INPUT_REMAPPER_CONFIG",
            &["~/.config/input-remapper-2"],
        ),
        (
            "xremap",
            &["xremap"],
            "XREMAP_CONFIG",
            &["~/.config/xremap"],
        ),
    ]
}

fn collect_processes(names: &[&str], processes: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten().take(MAX_PROCESS_ENTRIES) {
        let file_name = entry.file_name();
        let Some(pid) = file_name
            .to_str()
            .filter(|value| value.parse::<u32>().is_ok())
        else {
            continue;
        };
        let comm = fs::read_to_string(entry.path().join("comm"))
            .ok()
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        let cmdline_name = fs::read(entry.path().join("cmdline"))
            .ok()
            .and_then(|value| {
                value
                    .split(|byte| *byte == 0)
                    .next()
                    .map(|part| part.to_vec())
            })
            .and_then(|value| String::from_utf8(value).ok())
            .and_then(|value| {
                Path::new(&value)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        if names
            .iter()
            .any(|name| comm.eq_ignore_ascii_case(name) || cmdline_name.eq_ignore_ascii_case(name))
        {
            processes.push(format!(
                "pid {pid}: {}",
                if comm.is_empty() { cmdline_name } else { comm }
            ));
        }
    }
}

fn config_candidates(env_name: &str, defaults: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os(env_name) {
        candidates.push(PathBuf::from(path));
    }
    for raw in defaults {
        let expanded = if let Some(rest) = raw.strip_prefix("~/") {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(rest))
        } else {
            Some(PathBuf::from(raw))
        };
        let Some(path) = expanded else { continue };
        if path.is_dir() {
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            candidates.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .take(MAX_CONFIG_FILES),
            );
        } else {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn parse_keyd_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('['))
        .filter_map(|line| {
            let (from, to) = line.split_once('=')?;
            let from = from.trim();
            let to = to.trim();
            (!from.is_empty() && !to.is_empty()).then(|| format!("{from} -> {to}"))
        })
        .take(MAX_CONFIG_FILES)
        .collect::<Vec<_>>();
    if content.lines().map(str::trim).any(|line| {
        line.split_once('=').is_some_and(|(_, to)| {
            let to = to.trim().to_ascii_lowercase();
            to.contains("overload") || to.contains("oneshot") || to.contains("one-shot")
        })
    }) {
        transformations.push(
            "timing-dependent keyd overload/one-shot behavior detected; tap-versus-hold remains conditional"
                .into(),
        );
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn parse_lisp_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    for line in content.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("(defalias") else {
            continue;
        };
        let tokens = rest
            .trim_matches(|character| character == '(' || character == ')')
            .split_whitespace()
            .collect::<Vec<_>>();
        if tokens.len() >= 2
            && tokens[0].chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '@')
            })
        {
            transformations.push(format!(
                "{} -> {} (conditional alias)",
                tokens[0],
                tokens[1..].join(" ")
            ));
        }
    }
    if content.contains("tap-hold") || content.contains("tap-hold-next-release") {
        transformations.push(
            "tap-hold/layer behavior detected; timing and active layer remain conditional".into(),
        );
    }
    if content.contains("deflayer") {
        transformations.push(
            "layered keyboard definitions detected; active layer and precedence remain conditional"
                .into(),
        );
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn parse_input_remapper_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    collect_json_pairs(&value, &mut transformations);
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

fn collect_json_pairs(value: &serde_json::Value, transformations: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        if let Some(values) = value.as_array() {
            for value in values {
                collect_json_pairs(value, transformations);
            }
        }
        return;
    };
    if let (Some(input), Some(output)) = (object.get("input"), object.get("output")) {
        let input = display_json_value(input);
        let output = display_json_value(output);
        if !input.is_empty() && !output.is_empty() {
            transformations.push(format!("{input} -> {output}"));
        }
    }
    for value in object.values() {
        collect_json_pairs(value, transformations);
    }
}

fn display_json_value(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn parse_xremap_transformations(path: &Path) -> Vec<String> {
    let Some(content) = crate::util::read_bounded(path, MAX_CONFIG_BYTES) else {
        return Vec::new();
    };
    let mut transformations = Vec::new();
    let mut in_remap = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("remap:") {
            in_remap = true;
            continue;
        }
        if trimmed.ends_with(':') && !trimmed.starts_with('-') {
            let key = trimmed.trim_end_matches(':').trim();
            if !matches!(key, "modmap" | "keymap" | "remap") {
                in_remap = false;
            }
            continue;
        }
        if !in_remap || trimmed.contains('{') || trimmed.contains('}') {
            continue;
        }
        let Some((from, to)) = trimmed.split_once(':') else {
            continue;
        };
        let from = from.trim().trim_matches(['"', '\'']);
        let to = to.trim().trim_matches(['"', '\'']);
        if !from.is_empty() && !to.is_empty() && !to.starts_with('[') {
            transformations.push(format!("{from} -> {to}"));
        }
    }
    transformations.truncate(MAX_CONFIG_FILES);
    transformations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_literal_keyd_mappings() {
        let path = env::temp_dir().join(format!(
            "whykey-keyd-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "[main]\ncapslock = overload(control, esc)\na = b\n# ignored\n",
        )
        .unwrap();
        let transformations = parse_keyd_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value == "capslock -> overload(control, esc)")
        );
        assert!(transformations.iter().any(|value| value == "a -> b"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_literal_caps_to_ctrl_from_fixture() {
        let path = fixture_path("keyd-caps-to-ctrl.conf");
        let transformations = parse_keyd_transformations(&path);
        assert_eq!(transformations, vec!["capslock -> leftcontrol"]);
    }

    #[test]
    fn keeps_keyd_tap_hold_timing_conditional() {
        let path = fixture_path("keyd-overload-tap-hold.conf");
        let transformations = parse_keyd_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value == "capslock -> overload(control, esc)")
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("timing-dependent") && value.contains("conditional"))
        );
    }

    #[test]
    fn catalogs_common_remappers() {
        assert!(known_remappers().iter().any(|(name, ..)| *name == "keyd"));
    }

    #[test]
    fn parses_literal_input_remapper_json_pairs() {
        let path = env::temp_dir().join(format!(
            "whykey-input-remapper-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, r#"[{"input":"CapsLock","output":"Ctrl_L"}]"#).unwrap();
        let transformations = parse_input_remapper_transformations(&path);
        assert_eq!(transformations, vec!["CapsLock -> Ctrl_L"]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_literal_caps_to_ctrl_across_json_and_yaml_fixtures() {
        let input_remapper =
            parse_input_remapper_transformations(&fixture_path("input-remapper-caps-to-ctrl.json"));
        assert_eq!(input_remapper, vec!["CapsLock -> Ctrl_L"]);

        let xremap = parse_xremap_transformations(&fixture_path("xremap-caps-to-ctrl.yml"));
        assert_eq!(xremap, vec!["CapsLock -> Ctrl_L"]);
    }

    #[test]
    fn keeps_lisp_tap_hold_and_layers_conditional() {
        let path = env::temp_dir().join(format!(
            "whykey-kanata-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "(defalias caps (tap-hold 200 200 esc lctl))\n(deflayer base @caps a)\n",
        )
        .unwrap();
        let transformations = parse_lisp_transformations(&path);
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("conditional alias"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("tap-hold"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("layered"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reproduces_fixture_tap_hold_without_flattening_it_to_a_keypress() {
        let transformations = parse_lisp_transformations(&fixture_path("kanata-tap-hold.kbd"));
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("caps -> (tap-hold") && value.contains("lctl"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("timing and active layer remain conditional"))
        );
        assert!(
            transformations
                .iter()
                .any(|value| value.contains("active layer and precedence remain conditional"))
        );
    }

    #[test]
    fn parses_xremap_literal_remap_entries() {
        let path = env::temp_dir().join(format!(
            "whykey-xremap-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &path,
            "modmap:\n  - name: caps\n    remap:\n      CapsLock: Ctrl_L\n",
        )
        .unwrap();
        let transformations = parse_xremap_transformations(&path);
        assert_eq!(transformations, vec!["CapsLock -> Ctrl_L"]);
        let _ = fs::remove_file(path);
    }

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/remappers")
            .join(name)
    }
}
