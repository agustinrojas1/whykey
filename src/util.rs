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

/// Names of currently running processes, read-only from `/proc`.
pub fn process_names() -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return names;
    };
    for entry in entries.flatten().take(512) {
        let Ok(process) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let process = process.trim();
        if !process.is_empty() {
            names.push(process.to_owned());
        }
    }
    names.sort();
    names.dedup();
    names
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
    fn ancestor_walk_stays_bounded() {
        let pids = ancestor_pids(1);
        assert!(!pids.is_empty());
        assert!(pids.len() <= 16);
    }
}
