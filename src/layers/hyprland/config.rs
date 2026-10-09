#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigBindHint {
    combo: KeyCombo,
    ignore_mods: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaBindHint {
    combo: KeyCombo,
    description: Option<String>,
    action: Option<String>,
    ignore_mods: Option<bool>,
    source: PathBuf,
    line_number: usize,
}

/// Static configuration evidence. Config files do not change during one
/// process, so they are read once instead of on every inspected key.
#[derive(Debug)]
struct ConfigHints {
    lua: Vec<LuaBindHint>,
    declarative: Vec<ConfigBindHint>,
}

/// Upper bound on a single Hyprland config or Lua file read.
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

static CONFIG_HINTS: LazyLock<ConfigHints> = LazyLock::new(|| ConfigHints {
    lua: lua_config_bindings(),
    declarative: declarative_config_hints(),
});

/// Read only the declarative bind syntax needed to recover `ignore_mods`.
///
/// `hyprctl binds -j` intentionally omits that field on several Hyprland
/// versions. This parser is deliberately conservative: unresolved variables,
/// globs, and Lua expressions are skipped, so it can only add evidence and
/// never turns an unknown binding into a false exact match.
fn declarative_config_hints() -> Vec<ConfigBindHint> {
    let Some(root) = hyprland_config_paths()
        .into_iter()
        .find(|path| path.is_file())
    else {
        return Vec::new();
    };
    let mut visited = HashSet::new();
    let mut variables = HashMap::new();
    let mut hints = Vec::new();
    collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
    hints
}

fn hyprland_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("HYPRLAND_CONFIG") {
        paths.push(PathBuf::from(path));
    }
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("hypr/hyprland.conf"));
    }
    paths.push(PathBuf::from("/etc/xdg/hypr/hyprland.conf"));
    paths
}

/// Scan the small, literal subset of Lua used by common Hyprland helpers
/// (`o.bind`, `hl.bind`, and `hl.unbind`). Lua is executable code, so this
/// never evaluates it; dynamic key expressions remain intentionally unknown.
fn lua_config_bindings() -> Vec<LuaBindHint> {
    let mut roots = Vec::new();
    // Load packaged defaults first and user configuration last, matching the
    // normal Omarchy require order so the final matching hint is the useful
    // one when a user overrides a default binding.
    roots.push(PathBuf::from("/usr/share/omarchy/default/hypr"));
    if let Some(omarchy) = env::var_os("OMARCHY_PATH") {
        roots.push(PathBuf::from(omarchy).join("default/hypr"));
    }
    if let Some(base) = crate::util::config_base() {
        roots.push(base.join("hypr"));
    }

    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        collect_lua_files(&root, &mut seen, &mut files, 0);
    }

    let mut hints = Vec::new();
    for path in files {
        let Some(content) = crate::util::read_bounded(&path, MAX_CONFIG_BYTES) else {
            continue;
        };
        extend_lua_bind_hints(&mut hints, &content, &path);
    }
    hints
}

#[cfg(test)]
fn parse_lua_bind_hints(content: &str, source: &Path) -> Vec<LuaBindHint> {
    let mut hints = Vec::new();
    extend_lua_bind_hints(&mut hints, content, source);
    hints
}

/// Apply one file's literal bind/unbind calls, in order, on top of the hints
/// collected from earlier files.
fn extend_lua_bind_hints(hints: &mut Vec<LuaBindHint>, content: &str, source: &Path) {
    const MAX_CALL_LENGTH: usize = 64 * 1024;
    let mut pending = String::new();
    let mut start_line = 0;
    let mut skipped = None;

    for (index, raw_line) in content.lines().enumerate() {
        let sanitized = strip_lua_comments_and_long_strings(raw_line, &mut skipped);
        let line = sanitized.trim();
        if pending.is_empty() {
            if find_lua_bind_call(line).is_none() {
                continue;
            }
            start_line = index + 1;
        } else {
            pending.push('\n');
        }
        pending.push_str(line);

        let complete = find_lua_bind_call(&pending)
            .and_then(|(_, open)| lua_call_arguments(&pending[open + 1..]))
            .is_some();
        if complete {
            apply_lua_bind_call(hints, &pending, source, start_line);
            pending.clear();
        } else if pending.len() > MAX_CALL_LENGTH {
            pending.clear();
        }
    }
}

fn apply_lua_bind_call(hints: &mut Vec<LuaBindHint>, call: &str, source: &Path, line: usize) {
    if find_lua_bind_call(call).is_some_and(|(prefix, _)| prefix == "hl.unbind") {
        apply_lua_unbind(hints, call);
    } else if let Some(hint) = parse_lua_bind_hint(call, source, line) {
        hints.push(hint);
    }
}

/// Model `hl.unbind`: a literal combo removes every earlier binding for that
/// combo (a later bind re-adds it), and `hl.unbind("all")` removes them all.
/// A dynamic argument cannot be attributed, so it leaves the hints as they are.
fn apply_lua_unbind(hints: &mut Vec<LuaBindHint>, call: &str) {
    let Some(arguments) =
        find_lua_bind_call(call).and_then(|(_, open)| lua_call_arguments(&call[open + 1..]))
    else {
        return;
    };
    let Some(target) = arguments
        .first()
        .and_then(|argument| lua_literal_argument(argument))
    else {
        return;
    };
    if target == "all" && arguments.len() == 1 {
        hints.clear();
    } else if let Ok(combo) = target.parse::<KeyCombo>() {
        hints.retain(|hint| hint.combo != combo);
    }
}

/// Recover the key of a binding that hyprctl reports without one: Lua binds
/// with `code:` keys expose neither `key` nor `keycode`. Only literal hints
/// with the same modifiers and description count, and only when they all
/// agree on the key.
fn lua_hint_key<'a>(modmask: u32, description: &str, hints: &'a [LuaBindHint]) -> Option<&'a str> {
    if description.trim().is_empty() {
        return None;
    }
    let mut keys = hints
        .iter()
        .filter(|hint| {
            hint.combo.modmask() == modmask && hint.description.as_deref() == Some(description)
        })
        .map(|hint| hint.combo.key());
    let first = keys.next()?;
    keys.all(|key| key == first).then_some(first)
}

/// A Lua long-bracket comment or string that continues onto a later line.
/// The closing delimiter is retained rather than a nesting counter because
/// Lua long brackets do not nest.
type LuaSkippedDelimiter = Option<String>;

fn strip_lua_comments_and_long_strings(line: &str, skipped: &mut LuaSkippedDelimiter) -> String {
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        if let Some(delimiter) = skipped.as_deref() {
            if let Some(offset) = line[index..].find(delimiter) {
                index += offset + delimiter.len();
                *skipped = None;
            } else {
                break;
            }
            continue;
        }

        let character = line[index..]
            .chars()
            .next()
            .expect("index stays on a UTF-8 character boundary");
        if matches!(character, '"' | '\'') {
            let quote = character;
            output.push(character);
            index += character.len_utf8();
            let mut escaped = false;
            while index < line.len() {
                let next = line[index..]
                    .chars()
                    .next()
                    .expect("index stays on a UTF-8 character boundary");
                output.push(next);
                index += next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            continue;
        }

        if character == '-' && line[index..].starts_with("--") {
            let comment_start = index + 2;
            if let Some((open_len, delimiter)) = lua_long_bracket(line, comment_start) {
                *skipped = Some(delimiter);
                index = comment_start + open_len;
                continue;
            }
            break;
        }

        if character == '[' {
            if let Some((open_len, delimiter)) = lua_long_bracket(line, index) {
                *skipped = Some(delimiter);
                index += open_len;
                continue;
            }
        }

        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn lua_long_bracket(line: &str, start: usize) -> Option<(usize, String)> {
    let bytes = line.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut cursor = start + 1;
    while bytes.get(cursor) == Some(&b'=') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'[') {
        return None;
    }
    let equals = &line[start + 1..cursor];
    Some((cursor + 1 - start, format!("]{equals}]")))
}

fn collect_lua_files(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    depth: usize,
) {
    const MAX_LUA_DEPTH: usize = 5;
    const MAX_LUA_FILES: usize = 512;
    if depth > MAX_LUA_DEPTH || files.len() >= MAX_LUA_FILES {
        return;
    }
    // Follow a symlink only for the explicitly selected root. A symlinked
    // child could otherwise make a config scan unexpectedly walk an unrelated
    // tree (or a whole filesystem hierarchy).
    if depth > 0
        && fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        return;
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !seen.insert(canonical.clone()) {
        return;
    }
    let Ok(metadata) = fs::metadata(&canonical) else {
        return;
    };
    if metadata.is_file() {
        if canonical.extension().and_then(|value| value.to_str()) == Some("lua") {
            files.push(canonical);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(&canonical) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_lua_files(&entry.path(), seen, files, depth + 1);
        if files.len() >= MAX_LUA_FILES {
            break;
        }
    }
}

fn parse_lua_bind_hint(line: &str, source: &Path, line_number: usize) -> Option<LuaBindHint> {
    let (prefix, open) = find_lua_bind_call(line)?;
    if prefix == "hl.unbind" {
        return None;
    }
    let arguments = lua_call_arguments(&line[open + 1..])?;
    let combo = lua_literal_argument(arguments.first()?)?.parse().ok()?;
    let (description, action, options_index) = if prefix == "o.bind" {
        (
            arguments
                .get(1)
                .and_then(|value| lua_literal_argument(value)),
            arguments
                .get(2)
                .and_then(|value| lua_literal_argument(value)),
            3,
        )
    } else {
        (
            None,
            arguments
                .get(1)
                .and_then(|value| lua_literal_argument(value)),
            2,
        )
    };
    let ignore_mods = lua_ignore_mods_option(arguments.get(options_index).copied());
    Some(LuaBindHint {
        combo,
        description,
        action,
        ignore_mods,
        source: source.to_owned(),
        line_number,
    })
}

fn find_lua_bind_call(line: &str) -> Option<(&'static str, usize)> {
    let prefixes = ["o.bind", "hl.bind", "hl.unbind", "bind"];
    let mut index = 0;
    while index < line.len() {
        let character = line[index..]
            .chars()
            .next()
            .expect("index stays on a UTF-8 character boundary");
        if matches!(character, '"' | '\'') {
            let quote = character;
            index += character.len_utf8();
            let mut escaped = false;
            while index < line.len() {
                let next = line[index..]
                    .chars()
                    .next()
                    .expect("index stays on a UTF-8 character boundary");
                index += next.len_utf8();
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            continue;
        }
        if character == '-' && line[index..].starts_with("--") {
            break;
        }
        for prefix in prefixes {
            if !line[index..].starts_with(prefix) {
                continue;
            }
            let has_boundary = line[..index]
                .chars()
                .next_back()
                .is_none_or(|value| !value.is_alphanumeric() && value != '_');
            let is_qualified_bare_bind = prefix == "bind"
                && line[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|value| value == '.' || value == ':');
            if !has_boundary || is_qualified_bare_bind {
                continue;
            }
            let after_name = &line[index + prefix.len()..];
            let whitespace = after_name.len() - after_name.trim_start().len();
            if after_name[whitespace..].starts_with('(') {
                return Some((prefix, index + prefix.len() + whitespace));
            }
        }
        index += character.len_utf8();
    }
    None
}

fn lua_call_arguments(input: &str) -> Option<Vec<&str>> {
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut parentheses = 0_u32;
    let mut braces = 0_u32;
    let mut brackets = 0_u32;
    let mut quote = None;
    let mut escaped = false;

    for (index, character) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '(' => parentheses += 1,
            ')' if parentheses > 0 => parentheses -= 1,
            ')' if braces == 0 && brackets == 0 => {
                let argument = input[start..index].trim();
                if !argument.is_empty() {
                    arguments.push(argument);
                }
                return Some(arguments);
            }
            '{' => braces += 1,
            '}' if braces > 0 => braces -= 1,
            '[' => brackets += 1,
            ']' if brackets > 0 => brackets -= 1,
            ',' if parentheses == 0 && braces == 0 && brackets == 0 => {
                arguments.push(input[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    None
}

fn lua_literal_argument(argument: &str) -> Option<String> {
    let argument = argument.trim();
    let quote = matches!(argument.chars().next(), Some('"' | '\''));
    if !quote {
        return None;
    }

    // Only accept a complete quoted Lua string. In particular, do not turn
    // `"SUPER+C" .. suffix` (or a helper call containing a string) into a
    // literal binding: doing so could make a dynamic declaration look like
    // authoritative runtime evidence.
    let mut characters = argument.char_indices();
    let (_, delimiter) = characters.next()?;
    let mut value = String::new();
    let mut escaped = false;
    let mut closing_byte = None;
    for (index, character) in characters {
        if escaped {
            value.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == delimiter {
            closing_byte = Some(index + character.len_utf8());
            break;
        } else {
            value.push(character);
        }
    }
    let closing_byte = closing_byte?;
    if escaped || !argument[closing_byte..].trim().is_empty() {
        return None;
    }
    Some(value)
}

fn lua_ignore_mods_option(options: Option<&str>) -> Option<bool> {
    let Some(options) = options else {
        return Some(false);
    };
    let options = options.trim();
    if !options.starts_with('{') || !options.ends_with('}') {
        return None;
    }
    let normalized = options
        .split_whitespace()
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.contains("ignore_mods=true") {
        Some(true)
    } else {
        Some(false)
    }
}

/// `ignore_mods` from the last literal Lua declaration of exactly this
/// binding (same modifiers and key). A declaration of the same key with other
/// modifiers says nothing about this binding.
fn lua_ignore_mods_for_binding(
    binding: &Binding,
    key: &BindingKey,
    hints: &[LuaBindHint],
) -> Option<bool> {
    hints
        .iter()
        .rev()
        .find(|hint| {
            hint.combo.modmask() == binding.modmask
                && BindingKey::from_name(hint.combo.key()) == *key
        })
        .and_then(|hint| hint.ignore_mods)
}

/// The `.conf` counterpart of [`lua_ignore_mods_for_binding`].
fn config_ignore_mods_for_binding(
    binding: &Binding,
    key: &BindingKey,
    hints: &[ConfigBindHint],
) -> Option<bool> {
    hints
        .iter()
        .rev()
        .find(|hint| {
            hint.combo.modmask() == binding.modmask
                && BindingKey::from_name(hint.combo.key()) == *key
        })
        .map(|hint| hint.ignore_mods)
}

fn collect_config_hints(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    variables: &mut HashMap<String, String>,
    hints: &mut Vec<ConfigBindHint>,
    depth: usize,
) {
    const MAX_CONFIG_DEPTH: usize = 32;
    if depth > MAX_CONFIG_DEPTH {
        return;
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !visited.insert(canonical.clone()) {
        return;
    }
    let Some(content) = crate::util::read_bounded(&canonical, MAX_CONFIG_BYTES) else {
        return;
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = parse_config_variable(line) {
            // hyprlang expands references in a value against variables defined
            // earlier; one pass against the current map cannot loop, and a
            // self-reference resolves to the previous value (or stays literal).
            let value = resolve_inline_variables(&value, variables);
            variables.insert(name, value);
        }
    }

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(include) = parse_config_include(line) {
            let include = resolve_config_path(&include, &canonical, variables);
            // Bracket classes are not expanded; skip rather than misread them.
            if !include.to_string_lossy().contains('[') {
                for include in expand_config_glob(&include) {
                    collect_config_hints(&include, visited, variables, hints, depth + 1);
                }
            }
        }
        if let Some(hint) = parse_config_bind_hint(line, variables) {
            hints.push(hint);
        } else if let Some(combo) = parse_config_unbind(line, variables) {
            hints.retain(|hint| hint.combo != combo);
        }
    }
}

fn parse_config_variable(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once('=')?;
    let name = name.trim().strip_prefix('$')?;
    if name.is_empty() || name.chars().any(|character| character.is_whitespace()) {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then(|| (name.to_owned(), value.to_owned()))
}

fn parse_config_include(line: &str) -> Option<String> {
    let (directive, value) = line.split_once('=')?;
    if !matches!(directive.trim(), "source" | "include") {
        return None;
    }
    let value = value.trim().trim_matches(['"', '\'']);
    (!value.is_empty()).then(|| value.to_owned())
}

/// Expand `*` and `?` in each path component the way hyprlang's `source`
/// does through glob(3): a leading dot only matches explicitly, and matches
/// are returned in sorted order. A path without wildcards is returned as is.
fn expand_config_glob(path: &Path) -> Vec<PathBuf> {
    let mut matches = vec![PathBuf::new()];
    for component in path.components() {
        let pattern: Vec<char> = component.as_os_str().to_string_lossy().chars().collect();
        if !pattern.contains(&'*') && !pattern.contains(&'?') {
            for path in &mut matches {
                path.push(component);
            }
            continue;
        }
        let mut expanded = Vec::new();
        for directory in &matches {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            expanded.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name())
                    .filter(|name| {
                        let name: Vec<char> = name.to_string_lossy().chars().collect();
                        (name.first() != Some(&'.') || pattern.first() == Some(&'.'))
                            && wildcard_matches(&pattern, &name)
                    })
                    .map(|name| directory.join(name)),
            );
        }
        matches = expanded;
    }
    matches.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    matches
}

/// `*` matches any run of characters and `?` exactly one. Linear-time
/// two-pointer match: only the most recent `*` is ever backtracked to.
fn wildcard_matches(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

fn resolve_config_path(
    raw: &str,
    relative_to: &Path,
    variables: &HashMap<String, String>,
) -> PathBuf {
    let mut value = raw.to_owned();
    for _ in 0..8 {
        let Some(start) = value.find('$') else {
            break;
        };
        let end = value[start + 1..]
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map(|offset| start + 1 + offset)
            .unwrap_or(value.len());
        let name = &value[start + 1..end];
        let replacement = variables.get(name).cloned().or_else(|| env::var(name).ok());
        let Some(replacement) = replacement else {
            break;
        };
        value.replace_range(start..end, &replacement);
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        if value == "~" {
            return home;
        }
        if let Some(rest) = value.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        relative_to
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn parse_config_bind_hint(
    line: &str,
    variables: &HashMap<String, String>,
) -> Option<ConfigBindHint> {
    let (directive, values) = line.split_once('=')?;
    // Hyprland reads every character after `bind` as one flag (`bindel`,
    // `bindd`, `bindrl`, ...); any other suffix is a different keyword or an
    // invalid bind that Hyprland rejects.
    let flags = directive.trim().strip_prefix("bind")?;
    if !flags.chars().all(|flag| "lremnatisodpcgukx".contains(flag)) {
        return None;
    }
    // Multi-key binds (`s`) list several keys joined by `&`; they never
    // describe a single combo.
    if flags.contains('s') {
        return None;
    }
    let mut fields = values.split(',');
    let combo = parse_config_combo(fields.next()?, fields.next()?, variables)?;
    Some(ConfigBindHint {
        combo,
        ignore_mods: flags.contains('i'),
    })
}

/// `unbind = MODS, KEY` removes the earlier declarations of that combo.
fn parse_config_unbind(line: &str, variables: &HashMap<String, String>) -> Option<KeyCombo> {
    let (directive, values) = line.split_once('=')?;
    if directive.trim() != "unbind" {
        return None;
    }
    let mut fields = values.split(',');
    parse_config_combo(fields.next()?, fields.next()?, variables)
}

fn parse_config_combo(
    modifiers: &str,
    key: &str,
    variables: &HashMap<String, String>,
) -> Option<KeyCombo> {
    let modifiers = resolve_inline_variables(modifiers, variables);
    let key = resolve_inline_variables(key, variables);
    if key.is_empty() || key.split_whitespace().count() > 1 {
        return None;
    }
    let combo_text = if modifiers.is_empty() {
        key
    } else {
        format!(
            "{}+{key}",
            modifiers.split_whitespace().collect::<Vec<_>>().join("+")
        )
    };
    combo_text.parse().ok()
}

/// Substitute `$name` variables the way hyprlang does: at each `$`, the
/// longest defined name wins, so `$mod` never clobbers `$mainMod`. Unknown
/// names stay literal and make the bind unparsable instead of guessed.
fn resolve_inline_variables(raw: &str, variables: &HashMap<String, String>) -> String {
    let mut value = String::with_capacity(raw.len());
    let mut rest = raw.trim();
    while let Some(start) = rest.find('$') {
        value.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let longest = variables
            .iter()
            .filter(|(name, _)| after.starts_with(name.as_str()))
            .max_by_key(|(name, _)| name.len());
        match longest {
            Some((name, replacement)) => {
                value.push_str(replacement);
                rest = &after[name.len()..];
            }
            None => {
                value.push('$');
                rest = after;
            }
        }
    }
    value.push_str(rest);
    value.trim_matches(['"', '\'']).trim().to_owned()
}

#[cfg(test)]
mod config_variable_tests {
    use super::*;

    #[test]
    fn variable_values_expand_earlier_variables_for_bind_and_unbind() {
        let base = std::env::temp_dir().join(format!(
            "whykey-hypr-alias-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&base).unwrap();
        let root = base.join("hyprland.conf");
        fs::write(
            &root,
            "$mainMod = SUPER\n$mod = $mainMod\n$loop = $loop\n\
             bindi = $mod, Q, exec, a\nbindi = $mod, W, exec, b\nunbind = $mod, W\n",
        )
        .unwrap();

        let mut visited = HashSet::new();
        let mut variables = HashMap::new();
        let mut hints = Vec::new();
        collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
        let _ = fs::remove_dir_all(&base);

        assert_eq!(variables.get("mod").map(String::as_str), Some("SUPER"));
        assert_eq!(variables.get("loop").map(String::as_str), Some("$loop"));
        let super_q: KeyCombo = "super+q".parse().unwrap();
        assert!(hints.iter().any(|hint| hint.combo == super_q));
        let super_w: KeyCombo = "super+w".parse().unwrap();
        assert!(hints.iter().all(|hint| hint.combo != super_w));
    }
}

#[cfg(test)]
mod wildcard_tests {
    use super::wildcard_matches;

    fn matches(pattern: &str, name: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let name: Vec<char> = name.chars().collect();
        wildcard_matches(&pattern, &name)
    }

    #[test]
    fn wildcard_semantics() {
        assert!(matches("*.conf", "binds.conf"));
        assert!(matches("b?nds*", "binds.conf"));
        assert!(matches("*", ""));
        assert!(matches("a*b*c", "aXbYbZc"));
        assert!(!matches("*.conf", "binds.lua"));
        assert!(!matches("?", ""));
        assert!(!matches("a*b", "acbc"));
    }

    #[test]
    fn pathological_wildcard_is_linear() {
        let pattern = format!("{}b", "a*".repeat(64));
        let name = "a".repeat(4096);
        let started = std::time::Instant::now();
        assert!(!matches(&pattern, &name));
        assert!(matches(&pattern, &format!("{name}b")));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
