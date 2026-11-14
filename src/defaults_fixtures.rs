//! Test support: the default-binding fixtures under `lab/defaults/`, dumped
//! from the real binaries by `tools/dump_defaults.py`.
//!
//! The embedded tables (kitty, foot, Ghostty, Fcitx5, Readline) are copies of
//! upstream data, and nothing at run time tells whether upstream changed. Each
//! table is compared with the fixture of the same program, so refreshing a
//! fixture from a newer binary turns drift into a failing test and a diff.
//! Only the refresh needs Docker; the comparison is a plain unit test.

use std::path::Path;

/// One fixture file: the version the binary reported and its rows
/// (`key<TAB>action`, header lines starting with `#` removed).
pub(crate) struct Fixture {
    pub version: String,
    pub rows: Vec<(String, String)>,
}

/// The fixture for `program`. `lab/` is not part of the published crate, so a
/// source tree without it skips the comparison instead of failing: `None`.
pub(crate) fn load(program: &str) -> Option<Fixture> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("lab/defaults");
    if !dir.is_dir() {
        eprintln!("lab/defaults is absent; not comparing the {program} defaults");
        return None;
    }
    let prefix = format!("{program}-");
    let mut found = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix).is_some_and(|rest| {
                rest.ends_with(".txt") && rest.starts_with(|c: char| c.is_ascii_digit())
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one lab/defaults/{program}-<version>.txt, found {}; \
         refresh with `python3 tools/dump_defaults.py dump {program}`",
        found.len()
    );
    let path = found.remove(0).path();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut version = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("# version: ") {
            version = Some(v.to_owned());
        } else if !line.starts_with('#') {
            let (key, action) = line.split_once('\t').unwrap_or((line, ""));
            rows.push((key.to_owned(), action.to_owned()));
        }
    }
    Some(Fixture {
        version: version.unwrap_or_else(|| panic!("{} has no `# version:` header", path.display())),
        rows,
    })
}

/// Panics with the rows only one side has, so a failure reads as a diff.
pub(crate) fn assert_same_rows(what: &str, embedded: &[(String, String)], fixture: &Fixture) {
    let mut only_embedded: Vec<_> = embedded
        .iter()
        .filter(|row| !fixture.rows.contains(row))
        .collect();
    let mut only_fixture: Vec<_> = fixture
        .rows
        .iter()
        .filter(|row| !embedded.contains(row))
        .collect();
    only_embedded.sort();
    only_fixture.sort();
    assert!(
        only_embedded.is_empty() && only_fixture.is_empty(),
        "{what} differ from the binary's defaults (fixture version {}).\n\
         only in the embedded table: {only_embedded:#?}\n\
         only in the fixture: {only_fixture:#?}",
        fixture.version
    );
}
