//! Bounded, read-only include-chain resolution for literal configuration.
//!
//! The resolver follows only explicit `include`/`source` directives. It never
//! expands globs or executes config logic; dynamic paths become warnings and
//! remain conditional evidence. Every retained line carries its file and line
//! origin so adapters can report provenance without claiming runtime truth.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const DEFAULT_MAX_DEPTH: usize = 32;
const DEFAULT_MAX_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_FILES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLine {
    pub text: String,
    pub source: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub lines: Vec<ResolvedLine>,
    pub warnings: Vec<String>,
}

impl ResolvedConfig {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + if self.lines.is_empty() { "" } else { "\n" }
    }
}

pub fn resolve(root: &Path, directives: &[&str]) -> io::Result<ResolvedConfig> {
    let mut state = State {
        directives,
        visited: HashSet::new(),
        files: 0,
        output_bytes: 0,
        lines: Vec::new(),
        warnings: Vec::new(),
    };
    state.visit(root, 0)?;
    Ok(ResolvedConfig {
        lines: state.lines,
        warnings: state.warnings,
    })
}

struct State<'a> {
    directives: &'a [&'a str],
    visited: HashSet<PathBuf>,
    files: usize,
    output_bytes: usize,
    lines: Vec<ResolvedLine>,
    warnings: Vec<String>,
}

impl State<'_> {
    fn visit(&mut self, path: &Path, depth: usize) -> io::Result<()> {
        if depth > DEFAULT_MAX_DEPTH || self.files >= DEFAULT_MAX_FILES {
            self.warnings
                .push(format!("include limit reached at {}", path.display()));
            return Ok(());
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if !self.visited.insert(canonical.clone()) {
            self.warnings
                .push(format!("include cycle skipped at {}", canonical.display()));
            return Ok(());
        }
        let content =
            crate::util::read_bounded(&canonical, DEFAULT_MAX_BYTES).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is unreadable or too large", canonical.display()),
                )
            })?;
        self.files += 1;
        for (index, raw_line) in content.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = raw_line.trim();
            let Some((directive, argument)) = trimmed.split_once(char::is_whitespace) else {
                self.push_line(raw_line, &canonical, line_number);
                continue;
            };
            if !self.directives.contains(&directive) {
                self.push_line(raw_line, &canonical, line_number);
                continue;
            }
            let argument = argument.trim().trim_matches(['"', '\'']);
            if argument.is_empty() || argument.contains(['*', '?', '[', ']', '$', '`', '{', '}']) {
                self.warnings.push(format!(
                    "dynamic {directive} at {}:{line_number} remains unresolved",
                    canonical.display()
                ));
                continue;
            }
            let include = Path::new(argument);
            let include = if include.is_absolute() {
                include.to_owned()
            } else {
                canonical
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(include)
            };
            if !include.is_file() {
                self.warnings.push(format!(
                    "unresolved {directive} at {}:{line_number}: {}",
                    canonical.display(),
                    include.display()
                ));
                continue;
            }
            self.visit(&include, depth + 1)?;
        }
        Ok(())
    }

    fn push_line(&mut self, text: &str, source: &Path, line: usize) {
        if self.output_bytes + text.len() + 1 > DEFAULT_MAX_BYTES {
            self.warnings.push(format!(
                "resolved output limit reached at {}:{line}",
                source.display()
            ));
            return;
        }
        self.output_bytes += text.len() + 1;
        self.lines.push(ResolvedLine {
            text: text.to_owned(),
            source: source.to_owned(),
            line,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_nested_sources_with_cycle_and_provenance() {
        let root = std::env::temp_dir().join(format!("whykey-resolver-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("root"), "include child\nroot\n").unwrap();
        fs::write(root.join("child"), "source root\nchild\n").unwrap();
        let resolved = resolve(&root.join("root"), &["include", "source"]).unwrap();
        assert_eq!(resolved.text(), "child\nroot\n");
        assert!(resolved.lines[0].source.ends_with("child"));
        assert_eq!(resolved.lines[0].line, 2);
        assert!(
            resolved
                .warnings
                .iter()
                .any(|warning| warning.contains("cycle"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn leaves_dynamic_and_missing_paths_as_warnings() {
        let root =
            std::env::temp_dir().join(format!("whykey-resolver-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("root"),
            "include $HOME/generated\nsource absent\nvalue\n",
        )
        .unwrap();
        let resolved = resolve(&root.join("root"), &["include", "source"]).unwrap();
        assert_eq!(resolved.text(), "value\n");
        assert_eq!(resolved.warnings.len(), 2);
        let _ = fs::remove_dir_all(root);
    }
}
