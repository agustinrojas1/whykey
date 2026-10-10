//! S-expression reader shared by the kanata and kmonad configurations:
//! `;;` line comments, `#| |#` block comments, and double-quoted strings,
//! with a nesting cap.

/// Maximum list nesting; bounds recursion in evaluation and `Drop`.
const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    Kanata,
    /// kmonad also reads `\(`/`\)` inside tokens, `#(...)` as a
    /// `tap-macro`, backslash escapes in strings, and `"` as a delimiter.
    Kmonad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sexp {
    Atom(String, u32),
    List(Vec<Sexp>, u32),
}

impl Sexp {
    pub(crate) fn line(&self) -> u32 {
        match self {
            Self::Atom(_, line) | Self::List(_, line) => *line,
        }
    }

    pub(crate) fn atom(&self) -> Option<&str> {
        match self {
            Self::Atom(text, _) => Some(text),
            Self::List(..) => None,
        }
    }

    pub(crate) fn head(&self) -> Option<&str> {
        match self {
            Self::List(items, _) => items.first().and_then(Sexp::atom),
            Self::Atom(..) => None,
        }
    }

    pub(crate) fn render(&self) -> String {
        match self {
            Self::Atom(text, _) => text.clone(),
            Self::List(items, _) => format!(
                "({})",
                items.iter().map(Sexp::render).collect::<Vec<_>>().join(" ")
            ),
        }
    }
}

pub(crate) fn parse(content: &str, dialect: Dialect) -> Result<Vec<Sexp>, String> {
    let kmonad = dialect == Dialect::Kmonad;
    let mut stack: Vec<(Vec<Sexp>, u32)> = vec![(Vec::new(), 0)];
    let mut line = 1u32;
    let mut chars = content.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\n' => line += 1,
            c if c.is_whitespace() => {}
            ';' if chars.peek() == Some(&';') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '#' if chars.peek() == Some(&'|') => {
                chars.next();
                let mut previous = ' ';
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '\n' {
                        line += 1;
                    }
                    if previous == '|' && next == '#' {
                        closed = true;
                        break;
                    }
                    previous = next;
                }
                if !closed {
                    return Err("unterminated block comment".into());
                }
            }
            '#' if kmonad && chars.peek() == Some(&'(') => {
                chars.next();
                open(&mut stack, line)?;
                push_atom(&mut stack, "tap-macro".into(), line)?;
            }
            '(' => open(&mut stack, line)?,
            ')' => {
                let (items, start) = stack
                    .pop()
                    .filter(|_| !stack.is_empty())
                    .ok_or_else(|| format!("line {line}: unbalanced ')'"))?;
                stack
                    .last_mut()
                    .ok_or_else(|| format!("line {line}: unbalanced ')'"))?
                    .0
                    .push(Sexp::List(items, start));
            }
            '"' => {
                let mut text = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '"' {
                        closed = true;
                        break;
                    }
                    if next == '\n' {
                        line += 1;
                    }
                    if kmonad && next == '\\' {
                        if let Some(escaped) = chars.next() {
                            if escaped == '\n' {
                                line += 1;
                            }
                            text.push(escaped);
                        }
                        continue;
                    }
                    text.push(next);
                }
                if !closed {
                    return Err(format!("line {line}: unterminated string"));
                }
                push_atom(&mut stack, text, line)?;
            }
            first => {
                let mut text = String::from(first);
                if kmonad && first == '\\' {
                    if let Some(escaped) = chars.next_if(|next| matches!(next, '(' | ')')) {
                        text.push(escaped);
                    }
                }
                while let Some(next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '(' | ')') || (kmonad && *next == '"')
                    {
                        break;
                    }
                    let next = *next;
                    chars.next();
                    text.push(next);
                    if kmonad && next == '\\' {
                        if let Some(escaped) = chars.next_if(|next| matches!(next, '(' | ')')) {
                            text.push(escaped);
                        }
                    }
                }
                push_atom(&mut stack, text, line)?;
            }
        }
    }
    if stack.len() != 1 {
        return Err("unbalanced '('".into());
    }
    Ok(stack.pop().map(|(items, _)| items).unwrap_or_default())
}

fn open(stack: &mut Vec<(Vec<Sexp>, u32)>, line: u32) -> Result<(), String> {
    if stack.len() > MAX_NESTING {
        return Err(format!("line {line}: nesting deeper than {MAX_NESTING}"));
    }
    stack.push((Vec::new(), line));
    Ok(())
}

fn push_atom(stack: &mut [(Vec<Sexp>, u32)], text: String, line: u32) -> Result<(), String> {
    stack
        .last_mut()
        .ok_or_else(|| format!("line {line}: unexpected atom"))?
        .0
        .push(Sexp::Atom(text, line));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        for dialect in [Dialect::Kanata, Dialect::Kmonad] {
            let content = format!("{}{}", "(".repeat(200_000), ")".repeat(200_000));
            let Err(error) = parse(&content, dialect) else {
                panic!("deep nesting must be a parse error");
            };
            assert!(error.contains("nesting"), "{error}");
            let shallow = format!("{}{}", "(".repeat(32), ")".repeat(32));
            assert!(parse(&shallow, dialect).is_ok());
            let macros = format!("{}{}", "#(".repeat(200_000), ")".repeat(200_000));
            assert!(parse(&macros, dialect).is_err());
        }
    }

    #[test]
    fn kmonad_reads_escaped_parens_and_hash_macros() {
        let forms = parse(
            r#"(defalias lp \( mc #(K \) "x\"y") s C-\()"#,
            Dialect::Kmonad,
        )
        .unwrap();
        assert_eq!(
            forms[0].render(),
            r#"(defalias lp \( mc (tap-macro K \) x"y) s C-\()"#
        );
        assert!(parse(r"(a \( b)", Dialect::Kanata).is_err());
    }
}
