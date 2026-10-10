use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::key::KeyCombo;
use crate::layers::{BindingRecord, EvidenceTier, LayerId, LayerResult, Outcome};

const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// Read-only sxhkd binding adapter for bspwm-style sessions.
pub struct Sxhkd;

pub fn binding_inventory() -> Result<Vec<BindingRecord>, String> {
    let config = load_config().ok_or_else(|| {
        "sxhkd: sxhkdrc was not found; runtime sxhkd state remains unknown".to_owned()
    })?;
    if config.files.is_empty() {
        return Err(format!("sxhkd: config: {}", config.unreadable.join("; ")));
    }
    Ok(config
        .bindings
        .into_iter()
        .map(|binding| {
            BindingRecord::new(
                "sxhkd",
                binding.combo.compact_display(),
                binding.command,
                "configured; runtime activation conditional",
            )
            .with_context("sxhkdrc static binding")
        })
        .collect())
}

pub fn applicable() -> bool {
    if super::compositor::remote_session() {
        return false;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .into_iter()
        .chain(env::var("XDG_SESSION_DESKTOP").ok())
        .any(|desktop| {
            desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("bspwm"))
        })
        || config_path().is_some()
}

pub fn ipc_available() -> bool {
    config_path().is_some()
}

impl Sxhkd {
    pub fn inspect(&self, key: &KeyCombo) -> LayerResult {
        let Some(config) = load_config() else {
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unavailable,
                "sxhkd configuration is unavailable",
                vec!["set SXHKD_CONFIG or provide ~/.config/sxhkd/sxhkdrc".into()],
            );
        };
        let mut details: Vec<String> = config
            .files
            .iter()
            .map(|path| format!("config: {}", path.display()))
            .chain(
                config
                    .unreadable
                    .iter()
                    .map(|error| format!("config: {error}")),
            )
            .collect();
        if config.files.is_empty() {
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unavailable,
                "could not read sxhkd configuration",
                details,
            );
        }
        let matches = config
            .bindings
            .into_iter()
            .filter(|binding| binding.combo == *key)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            details.push(
                "no matching static sxhkd binding found; runtime reload state remains unknown"
                    .into(),
            );
            return LayerResult::new(
                "sxhkd",
                LayerId::Compositor,
                Outcome::Unknown,
                "sxhkd shortcut state is conditional",
                details,
            )
            .with_tier(EvidenceTier::ConfigFile);
        }
        for binding in &matches {
            details.push(format!("binding: {}", binding.command));
        }
        LayerResult::new(
            "sxhkd",
            LayerId::Compositor,
            Outcome::HandledUncertain,
            "matching sxhkd shortcut configured; runtime activation is conditional",
            details,
        )
        .with_tier(EvidenceTier::ConfigFile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    combo: KeyCombo,
    command: String,
}

/// Bindings from every file sxhkd loads, in its load order.
struct LoadedConfig {
    files: Vec<PathBuf>,
    unreadable: Vec<String>,
    bindings: Vec<Binding>,
}

/// sxhkd has no include directive: it reads its main config and then each
/// EXTRA_CONFIG operand, each file parsed on its own. `None` when no config
/// file is known.
fn load_config() -> Option<LoadedConfig> {
    let files = config_files();
    (!files.is_empty()).then(|| load_files(files))
}

fn load_files(files: Vec<PathBuf>) -> LoadedConfig {
    let mut config = LoadedConfig {
        files: Vec::new(),
        unreadable: Vec::new(),
        bindings: Vec::new(),
    };
    for path in files {
        match crate::util::read_bounded(&path, MAX_CONFIG_BYTES) {
            Some(content) => {
                config.bindings.extend(parse_bindings(&content));
                config.files.push(path);
            }
            None => config
                .unreadable
                .push(format!("{}: unreadable or too large", path.display())),
        }
    }
    config
}

/// The files to read: `SXHKD_CONFIG` when set, else the files a running
/// sxhkd was started with, else the default sxhkdrc.
fn config_files() -> Vec<PathBuf> {
    if let Some(path) = override_path() {
        return vec![path];
    }
    crate::util::ProcessSnapshot::collect()
        .matching_pids(&["sxhkd"])
        .into_iter()
        .find_map(running_config_files)
        .unwrap_or_else(|| config_path().into_iter().collect())
}

/// The configured sxhkdrc, without looking at running processes.
fn config_path() -> Option<PathBuf> {
    override_path().or_else(|| default_config_path().filter(|path| path.is_file()))
}

fn override_path() -> Option<PathBuf> {
    env::var_os("SXHKD_CONFIG")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn default_config_path() -> Option<PathBuf> {
    crate::util::config_base().map(|base| base.join("sxhkd/sxhkdrc"))
}

/// Config files of the sxhkd process `pid`, read from its command line.
/// Relative paths are resolved against that process's working directory.
fn running_config_files(pid: u32) -> Option<Vec<PathBuf>> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmdline = String::from_utf8_lossy(&cmdline);
    let arguments: Vec<&str> = cmdline
        .strip_suffix('\0')
        .unwrap_or(&cmdline)
        .split('\0')
        .skip(1)
        .collect();
    let (config, extras) = sxhkd_config_arguments(&arguments);
    let cwd = fs::read_link(format!("/proc/{pid}/cwd")).ok();
    let resolve = |path: &str| match &cwd {
        Some(cwd) if Path::new(path).is_relative() => cwd.join(path),
        _ => PathBuf::from(path),
    };
    let main = match config {
        Some(path) => Some(resolve(path)),
        None => default_config_path(),
    };
    Some(
        main.into_iter()
            .chain(extras.into_iter().map(resolve))
            .collect(),
    )
}

/// Split sxhkd's arguments (without argv[0]) as its getopt string
/// `hvm:t:c:r:s:a:` does: returns the `-c CONFIG_FILE` value and the
/// EXTRA_CONFIG operands. Like glibc getopt, operands may appear anywhere
/// and `--` ends option parsing.
fn sxhkd_config_arguments<'a>(arguments: &[&'a str]) -> (Option<&'a str>, Vec<&'a str>) {
    let mut config = None;
    let mut extras = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        index += 1;
        if argument == "--" {
            extras.extend_from_slice(&arguments[index..]);
            break;
        }
        let Some(cluster) = argument
            .strip_prefix('-')
            .filter(|cluster| !cluster.is_empty())
        else {
            extras.push(argument);
            continue;
        };
        for (offset, option) in cluster.char_indices() {
            if !"mtcrsa".contains(option) {
                continue;
            }
            let attached = &cluster[offset + option.len_utf8()..];
            let value = if attached.is_empty() {
                index += 1;
                arguments.get(index - 1).copied()
            } else {
                Some(attached)
            };
            if option == 'c' && value.is_some() {
                config = value;
            }
            break;
        }
    }
    (config, extras)
}

fn parse_bindings(content: &str) -> Vec<Binding> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut bindings = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw_key = lines[index].trim();
        index += 1;
        if raw_key.is_empty() || raw_key.starts_with('#') || raw_key.starts_with("^") {
            continue;
        }
        let normalized = raw_key.split_whitespace().collect::<String>();
        if !normalized.contains('+') || normalized.contains(['{', '}', '|']) {
            continue;
        }
        let Ok(combo) = normalized.parse::<KeyCombo>() else {
            continue;
        };
        let Some(command_index) = (index..lines.len()).find(|position| {
            let line = lines[*position];
            line.chars().next().is_some_and(char::is_whitespace)
                && !line.trim().is_empty()
                && !line.trim_start().starts_with('#')
        }) else {
            break;
        };
        bindings.push(Binding {
            combo,
            command: lines[command_index].trim().to_owned(),
        });
        index = command_index + 1;
    }
    bindings
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn parses_simple_sxhkd_key_command_pairs() {
        let bindings = parse_bindings(
            "# comment\nsuper + Return\n    alacritty\n\nctrl + alt + t\n    notify-send terminal\n",
        );
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].combo, "super+return".parse().unwrap());
        assert_eq!(bindings[0].command, "alacritty");
        assert_eq!(bindings[1].combo, "ctrl+alt+t".parse().unwrap());
    }

    #[test]
    fn ignores_dynamic_sxhkd_chords() {
        assert!(parse_bindings("super + {a,b}\n  echo dynamic\n").is_empty());
    }

    #[test]
    fn reads_the_config_and_extra_configs_from_sxhkd_arguments() {
        assert_eq!(
            sxhkd_config_arguments(&["-m", "1", "-c", "main", "extra1", "-t5", "extra2"]),
            (Some("main"), vec!["extra1", "extra2"])
        );
        assert_eq!(
            sxhkd_config_arguments(&["-vcmain", "--", "-extra"]),
            (Some("main"), vec!["-extra"])
        );
        assert_eq!(sxhkd_config_arguments(&[]), (None, Vec::new()));
    }

    #[test]
    fn loads_each_file_on_its_own_and_never_follows_include_lines() {
        let root = std::env::temp_dir().join(format!("whykey-sxhkd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // sxhkd has no include directive, so `include child` is not followed.
        // A chain left dangling at the end of one file must not borrow the
        // first command of the next one.
        fs::write(
            root.join("main"),
            "include child\nsuper + Return\n  root\nctrl + q\n",
        )
        .unwrap();
        fs::write(root.join("child"), "alt + c\n  included\n").unwrap();
        fs::write(root.join("extra"), "\n  stray\nctrl + x\n  extra\n").unwrap();
        let config = load_files(vec![
            root.join("main"),
            root.join("missing"),
            root.join("extra"),
        ]);
        assert_eq!(
            config
                .bindings
                .iter()
                .map(|binding| binding.command.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "extra"]
        );
        assert_eq!(config.files.len(), 2);
        assert_eq!(config.unreadable.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn non_regular_config_operands_are_reported_unreadable() {
        let root = std::env::temp_dir().join(format!("whykey-sxhkd-fifo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fifo = root.join("fifo");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let config = load_files(vec![fifo, root.clone()]);
        assert!(config.files.is_empty());
        assert_eq!(config.unreadable.len(), 2);
        let _ = fs::remove_dir_all(root);
    }
}
