//! Reads a `.env` file for the vault's import. Lines are `NAME=value`, with an optional `export `
//! prefix (any spacing), `#` comments and blank lines. Values may be bare (an inline ` # comment`
//! ends them), `'single quoted'` (literal), or `"double quoted"` (the escapes `\n \r \t \\ \"`; a
//! quoted value may run over several lines). Whitespace around the value is trimmed. A UTF-8 BOM is
//! skipped. An error names a line number and never repeats a value.

use zeroize::Zeroizing;

/// One `NAME=value` pair. The value is wiped when dropped.
pub struct Line {
    pub name: String,
    pub value: Zeroizing<String>,
}

/// Every pair in `text`, in order. Duplicates are kept: the caller decides.
pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let number = i + 1;
        let trimmed = lines[i].trim();
        i += 1;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let body = match trimmed.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
            _ => trimmed,
        };
        let Some((name, value_text)) = body.split_once('=') else {
            return Err(format!("line {number} is not NAME=value"));
        };
        let value_text = value_text.trim_start();
        let value = match value_text.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let (value, next) = read_quoted(&lines, i, &value_text[1..], quote == '"', number)?;
                i = next;
                value
            }
            _ => unquoted(value_text),
        };
        out.push(Line {
            name: name.trim().to_string(),
            value: Zeroizing::new(value),
        });
    }
    Ok(out)
}

/// The value of a bare `text`: up to an inline comment, trimmed.
fn unquoted(text: &str) -> String {
    if text.starts_with('#') {
        return String::new();
    }
    let mut end = text.len();
    for (index, c) in text.char_indices() {
        if c == '#' && text[..index].ends_with([' ', '\t']) {
            end = index;
            break;
        }
    }
    text[..end].trim().to_string()
}

/// A quoted value that starts in `first` (the line after the opening quote) and may continue on the
/// lines from `next` on. Returns the value and the index of the next unread line.
fn read_quoted(
    lines: &[&str],
    next: usize,
    first: &str,
    double: bool,
    number: usize,
) -> Result<(String, usize), String> {
    let mut value = Zeroizing::new(String::new());
    let mut current = Zeroizing::new(first.to_string());
    let mut next = next;
    loop {
        let mut chars = current.chars();
        let mut tail = None;
        while let Some(c) = chars.next() {
            if double && c == '\\' {
                match chars.next() {
                    Some('n') => value.push('\n'),
                    Some('r') => value.push('\r'),
                    Some('t') => value.push('\t'),
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some(other) => {
                        value.push('\\');
                        value.push(other);
                    }
                    None => value.push('\\'),
                }
            } else if (double && c == '"') || (!double && c == '\'') {
                tail = Some(chars.as_str().to_string());
                break;
            } else {
                value.push(c);
            }
        }
        if let Some(tail) = tail {
            let tail = tail.trim();
            if !tail.is_empty() && !tail.starts_with('#') {
                return Err(format!("line {number}: text after the closing quote"));
            }
            return Ok((value.to_string(), next));
        }
        let Some(line) = lines.get(next) else {
            return Err(format!("line {number}: the quote is never closed"));
        };
        value.push('\n');
        current = Zeroizing::new(line.to_string());
        next += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(text: &str) -> Vec<(String, String)> {
        parse(text)
            .expect("parsed")
            .into_iter()
            .map(|l| (l.name, l.value.to_string()))
            .collect()
    }

    #[test]
    fn bare_quoted_and_exported_lines_parse() {
        let text = "\u{feff}# a comment\n\nexport ANTHROPIC_API_KEY=sk-ant-1\nHF_TOKEN = hf_x  # trailing comment\nTITLE=\"two words\"\nSINGLE='a \\n literal'\nEMPTY=\nHASH=#not-a-value\n";
        assert_eq!(
            pairs(text),
            vec![
                ("ANTHROPIC_API_KEY".into(), "sk-ant-1".into()),
                ("HF_TOKEN".into(), "hf_x".into()),
                ("TITLE".into(), "two words".into()),
                ("SINGLE".into(), "a \\n literal".into()),
                ("EMPTY".into(), String::new()),
                ("HASH".into(), String::new()),
            ]
        );
    }

    #[test]
    fn double_quotes_take_escapes_and_may_span_lines() {
        let text = "KEY=\"line one\\nline \\\"two\\\"\nline three\" # done\nNEXT=1\n";
        assert_eq!(
            pairs(text),
            vec![
                ("KEY".into(), "line one\nline \"two\"\nline three".into()),
                ("NEXT".into(), "1".into())
            ]
        );
    }

    #[test]
    fn errors_name_a_line_and_never_a_value() {
        let secret = "SENTINEL-dotenv-7b1e";
        let unclosed = format!("OK=1\nBAD=\"{secret}\nmore");
        let err = parse(&unclosed).err().expect("refused");
        assert!(err.contains("line 2") && !err.contains(secret), "{err}");
        let not_a_pair = format!("{secret}\n");
        let err = parse(&not_a_pair).err().expect("refused");
        assert!(!err.contains(secret), "{err}");
        let trailing = format!("X=\"{secret}\" junk\n");
        let err = parse(&trailing).err().expect("refused");
        assert!(!err.contains(secret), "{err}");
    }
}
