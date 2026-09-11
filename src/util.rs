//! Shared bounded helpers for literal configuration readers.
//!
//! Adapters parse small documented literal subsets of configuration files.
//! These helpers keep that parsing in one place: XDG base paths, bounded
//! text reads, literal include traversal, unquoting, and process ancestry.
//! Dynamic configuration languages are never evaluated; unsupported forms
//! produce conditional results in the calling adapter.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Base directory for user configuration (`XDG_CONFIG_HOME` or
/// `HOME/.config`).
pub fn config_base() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// Base directory for user data (`XDG_DATA_HOME` or `HOME/.local/share`).
pub fn data_base() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}
/// User configuration file for a path relative to the config base.
pub fn config_file(relative: &str) -> Option<PathBuf> {
    config_base().map(|base| base.join(relative))
}

/// Bounded text read for a configuration file. Files larger than `limit`
/// bytes are rejected so a runaway file cannot fill memory.
pub fn read_bounded(path: &std::path::Path, limit: usize) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > limit as u64 {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Default bounded text read (64 KiB, matching adapter expectations).
pub fn read_config(path: &std::path::Path) -> Option<String> {
    read_bounded(path, 64 * 1024)
}

/// Strip one pair of surrounding double or single quotes.
pub fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

const MAX_PROCESS_ENTRIES: usize = 4096;

pub struct ProcessEntry {
    pub pid: u32,
    pub comm: String,
    pub cmdline_name: String,
}

pub struct ProcessSnapshot {
    entries: Vec<ProcessEntry>,
}

impl ProcessSnapshot {
    /// One `/proc` walk. Reads comm + cmdline for each PID.
    pub fn collect() -> Self {
        let mut entries = Vec::new();
        let Ok(dir_entries) = fs::read_dir("/proc") else {
            return Self { entries };
        };
        for entry in dir_entries.flatten() {
            if entries.len() >= MAX_PROCESS_ENTRIES {
                break;
            }
            let file_name = entry.file_name();
            let Some(pid) = file_name
                .to_str()
                .and_then(|value| value.parse::<u32>().ok())
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
                    std::path::Path::new(&value)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default();

            entries.push(ProcessEntry {
                pid,
                comm,
                cmdline_name,
            });
        }
        Self { entries }
    }

    /// Filter entries matching any of the given names (case-insensitive
    /// against comm OR cmdline_name). Returns "pid N: name" strings
    /// matching the existing collect_processes output format.
    pub fn find_processes(&self, names: &[&str]) -> Vec<String> {
        let mut result = Vec::new();
        for entry in &self.entries {
            if names.iter().any(|name| {
                entry.comm.eq_ignore_ascii_case(name)
                    || entry.cmdline_name.eq_ignore_ascii_case(name)
            }) {
                let name = if entry.comm.is_empty() {
                    &entry.cmdline_name
                } else {
                    &entry.comm
                };
                result.push(format!("pid {}: {}", entry.pid, name));
            }
        }
        result
    }

    /// Deduplicated sorted comm names. Replaces `util::process_names()`.
    pub fn comm_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for entry in &self.entries {
            if !entry.comm.is_empty() {
                names.push(entry.comm.clone());
            }
        }
        names.sort();
        names.dedup();
        names
    }
}

/// Names of currently running processes, read-only from `/proc`.
pub fn process_names() -> Vec<String> {
    ProcessSnapshot::collect().comm_names()
}

/// Ancestor process IDs starting at `start`, bounded to 16 generations.
pub fn ancestor_pids(start: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    let mut pid = start;
    for _ in 0..16 {
        pids.push(pid);
        let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
            break;
        };
        let Some(ppid) = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:")?.trim().parse::<u32>().ok())
        else {
            break;
        };
        if ppid == 0 || ppid == pid {
            break;
        }
        pid = ppid;
    }
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquotes_one_double_quoted_pair() {
        assert_eq!(unquote("\"key\""), "key");
        assert_eq!(unquote("key"), "key");
        assert_eq!(unquote("\"partial"), "\"partial");
    }

    #[test]
    fn config_file_follows_xdg_base() {
        let path = config_file("whykey-test/config.toml").unwrap();
        assert!(path.ends_with("whykey-test/config.toml"));
    }

    #[test]
    fn bounded_read_rejects_oversized_files() {
        let dir = std::env::temp_dir().join("whykey-util-bound");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.conf");
        std::fs::write(&path, vec![b'x'; 128]).unwrap();
        assert!(read_bounded(&path, 64).is_none());
        assert!(read_bounded(&path, 128).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_snapshot_collects_and_filters() {
        let snapshot = ProcessSnapshot::collect();
        assert!(!snapshot.entries.is_empty());
        let names = snapshot.comm_names();
        assert!(!names.is_empty());
    }

    #[test]
    fn ancestor_walk_stays_bounded() {
        let pids = ancestor_pids(1);
        assert!(!pids.is_empty());
        assert!(pids.len() <= 16);
    }
}
