#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigBindHint {
    key: String,
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

/// Read only the declarative bind syntax needed to recover `ignore_mods`.
///
/// `hyprctl binds -j` intentionally omits that field on several Hyprland
/// versions. This parser is deliberately conservative: unresolved variables,
/// globs, and Lua expressions are skipped, so it can only add evidence and
/// never turns an unknown binding into a false exact match.
fn config_has_ignore_mods(key: &KeyCombo, lua_hints: &[LuaBindHint]) -> bool {
    if lua_hints
        .iter()
        .any(|hint| hint.ignore_mods == Some(true) && hint.combo.key() == key.key())
    {
        return true;
    }
    let Some(root) = hyprland_config_paths()
        .into_iter()
        .find(|path| path.is_file())
    else {
        return false;
    };
    let mut visited = HashSet::new();
    let mut variables = HashMap::new();
    let mut hints = Vec::new();
    collect_config_hints(&root, &mut visited, &mut variables, &mut hints, 0);
    hints
        .iter()
        .any(|hint| hint.ignore_mods && hint.key.eq_ignore_ascii_case(key.key()))
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
/// (`o.bind` and `hl.bind`). Lua is executable code, so this never evaluates
/// it; dynamic key expressions remain intentionally unknown.
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
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        hints.extend(parse_lua_bind_hints(&content, &path));
    }
    hints
}

fn parse_lua_bind_hints(content: &str, source: &Path) -> Vec<LuaBindHint> {
    const MAX_CALL_LENGTH: usize = 64 * 1024;
    let mut hints = Vec::new();
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
            if let Some(hint) = parse_lua_bind_hint(&pending, source, start_line) {
                hints.push(hint);
            }
            pending.clear();
        } else if pending.len() > MAX_CALL_LENGTH {
            pending.clear();
        }
    }
    hints
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
    let prefixes = ["o.bind", "hl.bind", "bind"];
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

fn lua_ignore_mods_for_binding(binding: &Binding, hints: &[LuaBindHint]) -> Option<bool> {
    hints
        .iter()
        .rev()
        .find(|hint| {
            hint.combo.modmask() == binding.modmask
                && hint.combo.key().eq_ignore_ascii_case(&binding.key)
        })
        .and_then(|hint| hint.ignore_mods)
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
    let Ok(content) = fs::read_to_string(&canonical) else {
        return;
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = parse_config_variable(line) {
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
            if !include.to_string_lossy().contains('*')
                && !include.to_string_lossy().contains('?')
                && !include.to_string_lossy().contains('[')
            {
                collect_config_hints(&include, visited, variables, hints, depth + 1);
            }
        }
        if let Some(hint) = parse_config_bind_hint(line, variables) {
            hints.push(hint);
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
    let directive = directive.trim();
    let flags = directive
        .strip_prefix("bind[")
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_default();
    if !directive.starts_with("bind") {
        return None;
    }
    let mut fields = values.split(',').map(str::trim);
    let modifiers = resolve_inline_variables(fields.next()?, variables);
    let key = resolve_inline_variables(fields.next()?, variables);
    if key.is_empty() || key.contains(' ') && key.split_whitespace().count() > 1 {
        return None;
    }
    let combo_text = if modifiers.is_empty() {
        key.clone()
    } else {
        format!("{}+{key}", modifiers.replace(' ', "+"))
    };
    let combo: KeyCombo = combo_text.parse().ok()?;
    Some(ConfigBindHint {
        key: combo.key().to_owned(),
        ignore_mods: flags.contains('i'),
    })
}

fn resolve_inline_variables(raw: &str, variables: &HashMap<String, String>) -> String {
    let mut value = raw.trim().to_owned();
    for (name, replacement) in variables {
        value = value.replace(&format!("${name}"), replacement);
    }
    value.trim_matches(['"', '\'']).trim().to_owned()
}
