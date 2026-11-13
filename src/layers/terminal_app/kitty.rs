//! kitty.conf: `map` lines, `kitty_mod`, `include`/`globinclude` and
//! `clear_all_shortcuts`, resolved on top of kitty's default shortcuts.
//!
//! kitty resolves `kitty_mod` after reading the whole configuration, so it
//! applies to every `map` line and to the defaults wherever the option
//! appears. `clear_all_shortcuts yes` drops every mapping read up to that
//! point (defaults included). Includes are read in place, relative to the
//! directory of the including file (kitty `conf/utils.py`, `parse_line`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{Binding, decode_escape, defaults};
use crate::key::{KeyCombo, modifier_mask};

const DEFAULT_KITTY_MOD: &str = "ctrl+shift";

/// One effective-configuration step, in file order.
enum Step {
    ClearAll,
    Map {
        keys: String,
        action: String,
        options: Vec<String>,
    },
}

#[derive(Default)]
struct Parsed {
    steps: Vec<Step>,
    kitty_mod: Option<String>,
    files: Vec<PathBuf>,
    /// Why an unmatched key cannot be reported as unbound.
    caveats: Vec<String>,
    notes: Vec<String>,
    seen: HashSet<PathBuf>,
}

/// The effective kitty bindings, the files that contributed, and what the
/// reader could not follow.
pub(super) struct Loaded {
    pub bindings: Vec<Binding>,
    pub files: Vec<PathBuf>,
    pub caveats: Vec<String>,
    pub notes: Vec<String>,
}

/// Read `path` and everything it includes. An unreadable main file is an
/// error; an unreadable include is ignored, as kitty ignores it.
pub(super) fn load(path: &Path) -> Result<Loaded, String> {
    let content = crate::recording::read_to_string(path)
        .map_err(|error| format!("config: {}: {error}", path.display()))?;
    Ok(resolve(&content, path))
}

fn resolve(content: &str, path: &Path) -> Loaded {
    let mut parsed = Parsed::default();
    parsed.seen.insert(normalize(path));
    parsed.files.push(path.to_owned());
    parse_file(content, path, &mut parsed, 0);
    finish(parsed)
}

/// Parse an in-memory `kitty.conf` (relative includes resolve against `base`).
#[cfg(test)]
pub(super) fn parse_str(content: &str, base: &Path) -> Loaded {
    resolve(content, &base.join("kitty.conf"))
}

fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

fn parse_file(content: &str, path: &Path, parsed: &mut Parsed, depth: usize) {
    let base = path.parent().unwrap_or(Path::new("."));
    for line in logical_lines(content) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = match line.split_once(char::is_whitespace) {
            Some((name, value)) => (name, value.trim()),
            None => (line, ""),
        };
        match name {
            "kitty_mod" => {
                if valid_modifiers(value) {
                    parsed.kitty_mod = Some(value.to_ascii_lowercase());
                } else {
                    parsed
                        .caveats
                        .push(format!("kitty_mod {value} is not a modifier list"));
                }
            }
            "clear_all_shortcuts" => {
                if matches!(value.to_ascii_lowercase().as_str(), "yes" | "true" | "y") {
                    parsed.steps.push(Step::ClearAll);
                }
            }
            "map" => {
                if let Some(step) = parse_map(value) {
                    parsed.steps.push(step);
                }
            }
            "include" | "globinclude" => include(name, value, base, parsed, depth),
            "envinclude" | "geninclude" => parsed.caveats.push(format!(
                "{name} {value} is not followed (it depends on the environment or runs a program)"
            )),
            _ => {}
        }
    }
}

/// Lines with kitty's `\` continuation joined: a line starting with `\`
/// continues the previous one.
fn logical_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in content.lines() {
        let trimmed = raw.trim_start();
        match (trimmed.strip_prefix('\\'), lines.last_mut()) {
            (Some(rest), Some(previous)) => previous.push_str(rest),
            _ => lines.push(trimmed.to_owned()),
        }
    }
    lines
}

fn include(directive: &str, value: &str, base: &Path, parsed: &mut Parsed, depth: usize) {
    // kitty's own limit is the "already included" check; keep a hard bound
    // too so a pathological tree cannot run away.
    if depth >= 32 {
        parsed
            .caveats
            .push(format!("{directive} {value}: includes nest too deeply"));
        return;
    }
    let expanded = PathBuf::from(crate::layers::tiling::expand_shell_path(value));
    let target = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    let paths = if directive == "globinclude" {
        match crate::layers::tiling::expand_glob(&target) {
            Ok(paths) => paths,
            Err(error) => {
                parsed
                    .caveats
                    .push(format!("globinclude {value} is not followed: {error}"));
                return;
            }
        }
    } else {
        vec![target]
    };
    for path in paths {
        if !parsed.seen.insert(normalize(&path)) {
            continue;
        }
        match crate::recording::read_to_string(&path) {
            Ok(content) => {
                parsed.files.push(path.clone());
                parse_file(&content, &path, parsed, depth + 1);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parsed
                    .notes
                    .push(format!("included file not found: {}", path.display()));
            }
            Err(error) => parsed.caveats.push(format!(
                "included file {} could not be read: {error}",
                path.display()
            )),
        }
    }
}

/// `map [--option[=value]]... keys [action]`.
fn parse_map(value: &str) -> Option<Step> {
    let mut rest = value;
    let mut options = Vec::new();
    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with("--") {
            rest = trimmed;
            break;
        }
        let (option, after) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        options.push(option.to_owned());
        // `--mode name` takes its value as the next word.
        rest = after;
        if !option.contains('=') && option_takes_value(option) {
            let (word, after) = rest
                .trim_start()
                .split_once(char::is_whitespace)
                .unwrap_or((rest.trim_start(), ""));
            options.push(word.to_owned());
            rest = after;
        }
    }
    let (keys, action) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(keys, action)| (keys, action.trim()));
    if keys.is_empty() {
        return None;
    }
    Some(Step::Map {
        keys: keys.to_owned(),
        action: action.to_owned(),
        options,
    })
}

fn option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "--mode" | "--new-mode" | "--on-unknown" | "--on-action" | "--timeout" | "--when-focus-on"
    )
}

/// True when every `+`-separated token is a modifier.
fn valid_modifiers(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('+')
            .all(|token| modifier_mask(token.trim()).is_some())
}

/// Replace the `kitty_mod` token of a key spec by `modifiers`.
pub(super) fn substitute_kitty_mod(keys: &str, modifiers: &str) -> String {
    keys.split('+')
        .map(|token| {
            if token.eq_ignore_ascii_case("kitty_mod") {
                modifiers
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn finish(parsed: Parsed) -> Loaded {
    let Parsed {
        steps,
        kitty_mod,
        files,
        mut caveats,
        mut notes,
        ..
    } = parsed;
    let modifiers = kitty_mod.as_deref().unwrap_or(DEFAULT_KITTY_MOD);
    if modifiers != DEFAULT_KITTY_MOD {
        notes.push(format!("kitty_mod is {modifiers}"));
    }
    let mut bindings = defaults::kitty(modifiers);
    for step in steps {
        match step {
            Step::ClearAll => {
                bindings.clear();
                notes.push("clear_all_shortcuts removed the default shortcuts".into());
            }
            Step::Map {
                keys,
                action,
                options,
            } => {
                match map_binding(&keys, &action, &options, modifiers) {
                    Ok(Some(binding)) => bindings.push(binding),
                    // A mode-scoped map is not active at a shell prompt.
                    Ok(None) => {}
                    Err(()) => caveats.push(format!("map {keys}: key is not understood")),
                }
            }
        }
    }
    Loaded {
        bindings,
        files,
        caveats,
        notes,
    }
}

/// `Ok(None)` for a map that exists only inside a keyboard mode, `Err` for a
/// key whykey cannot parse.
fn map_binding(
    keys: &str,
    action: &str,
    options: &[String],
    modifiers: &str,
) -> Result<Option<Binding>, ()> {
    let spec = substitute_kitty_mod(keys, modifiers);
    // `a>b` is a multi-key mapping: the first key starts the sequence.
    let (first, is_sequence) = match spec.split_once('>') {
        Some((first, _)) => (first, true),
        None => (spec.as_str(), false),
    };
    let trigger: KeyCombo = first.parse().map_err(|_| ())?;
    let in_mode = options
        .iter()
        .any(|option| option.starts_with("--mode") && !option.starts_with("--new-mode"));
    if in_mode {
        return Ok(None);
    }
    let entering_mode = options
        .iter()
        .any(|option| option.starts_with("--new-mode"));
    let action_name = action.split_whitespace().next().unwrap_or_default();
    let unmaps = !is_sequence
        && !entering_mode
        && matches!(
            action_name.to_ascii_lowercase().as_str(),
            "" | "noop" | "no-op" | "no_op"
        );
    let (stored_action, output) = if is_sequence {
        ("key_sequence_prefix".to_owned(), None)
    } else if entering_mode {
        ("push_keyboard_mode".to_owned(), None)
    } else {
        let output = action.strip_prefix("send_text ").and_then(|value| {
            value
                .split_once(' ')
                .map(|(_, payload)| decode_escape(payload))
        });
        (action.to_owned(), output)
    };
    let description = if unmaps {
        format!("map {keys} (unmapped)")
    } else {
        format!("{trigger} {action}")
    };
    let mut binding = Binding::new(trigger, stored_action, output, description);
    binding.unmaps = unmaps;
    if options
        .iter()
        .any(|option| option.starts_with("--when-focus-on"))
    {
        binding.uncertain =
            Some("the mapping applies only to windows matching its focus condition".into());
    }
    Ok(Some(binding))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(loaded: &'a Loaded, key: &str) -> Option<&'a Binding> {
        let key: KeyCombo = key.parse().unwrap();
        loaded.bindings.iter().rev().find(|b| b.trigger == key)
    }

    #[test]
    fn kitty_mod_defaults_to_ctrl_shift_and_resolves_in_map_lines() {
        let loaded = parse_str("map kitty_mod+f12 launch ls\n", Path::new("/nowhere"));
        assert_eq!(find(&loaded, "ctrl+shift+f12").unwrap().action, "launch ls");
    }

    #[test]
    fn kitty_mod_override_moves_the_defaults_too() {
        let loaded = parse_str("kitty_mod ctrl+alt\n", Path::new("/nowhere"));
        // The default copy shortcut now sits on ctrl+alt+c, and ctrl+shift+c is free.
        assert_eq!(
            find(&loaded, "ctrl+alt+c").unwrap().action,
            "copy_to_clipboard"
        );
        assert!(find(&loaded, "ctrl+shift+c").is_none());
        assert!(loaded.notes.iter().any(|note| note.contains("ctrl+alt")));
        // kitty_mod set after a map line still applies to it.
        let loaded = parse_str(
            "map kitty_mod+f12 launch ls\nkitty_mod ctrl+alt\n",
            Path::new("/nowhere"),
        );
        assert!(find(&loaded, "ctrl+alt+f12").is_some());
        assert!(find(&loaded, "ctrl+shift+f12").is_none());
    }

    #[test]
    fn empty_map_and_no_op_unmap_a_default() {
        for line in [
            "map kitty_mod+c",
            "map kitty_mod+c no_op",
            "map kitty_mod+c noop",
        ] {
            let loaded = parse_str(line, Path::new("/nowhere"));
            assert!(find(&loaded, "ctrl+shift+c").unwrap().unmaps, "{line}");
        }
    }

    #[test]
    fn discard_event_is_not_an_unmap() {
        let loaded = parse_str("map kitty_mod+c discard_event\n", Path::new("/nowhere"));
        assert!(!find(&loaded, "ctrl+shift+c").unwrap().unmaps);
    }

    #[test]
    fn clear_all_shortcuts_drops_earlier_mappings_and_defaults() {
        let loaded = parse_str(
            "map ctrl+f1 launch a\nclear_all_shortcuts yes\nmap ctrl+f2 launch b\n",
            Path::new("/nowhere"),
        );
        assert!(find(&loaded, "ctrl+shift+c").is_none());
        assert!(find(&loaded, "ctrl+f1").is_none());
        assert!(find(&loaded, "ctrl+f2").is_some());
        let off = parse_str("clear_all_shortcuts no\n", Path::new("/nowhere"));
        assert!(find(&off, "ctrl+shift+c").is_some());
    }

    #[test]
    fn multi_key_maps_make_their_first_key_a_prefix() {
        let loaded = parse_str("map ctrl+a>c launch x\n", Path::new("/nowhere"));
        assert_eq!(
            find(&loaded, "ctrl+a").unwrap().action,
            "key_sequence_prefix"
        );
    }

    #[test]
    fn mode_scoped_maps_are_not_active_at_a_prompt() {
        let loaded = parse_str(
            "map --mode resize ctrl+f9 launch x\nmap --new-mode resize ctrl+f8\n",
            Path::new("/nowhere"),
        );
        assert!(find(&loaded, "ctrl+f9").is_none());
        assert_eq!(
            find(&loaded, "ctrl+f8").unwrap().action,
            "push_keyboard_mode"
        );
    }

    #[test]
    fn comments_only_start_at_the_beginning_of_a_line() {
        let loaded = parse_str(
            "map ctrl+f1 send_text all hi # not a comment\n# map ctrl+f2 launch no\n",
            Path::new("/nowhere"),
        );
        let binding = find(&loaded, "ctrl+f1").unwrap();
        assert!(
            binding
                .output
                .as_ref()
                .unwrap()
                .ends_with(b"# not a comment")
        );
        assert!(find(&loaded, "ctrl+f2").is_none());
    }

    #[test]
    fn unfollowed_includes_are_reported_as_caveats() {
        let loaded = parse_str(
            "envinclude KITTY_*\ngeninclude gen.py\n",
            Path::new("/nowhere"),
        );
        assert_eq!(loaded.caveats.len(), 2);
    }
}
