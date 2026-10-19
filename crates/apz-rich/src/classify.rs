//! The text scanner. Four passes, each splitting the plain-text spans the earlier ones left: inline code, markdown
//! links and images, bare URLs, then the words (`#528`, `@Name`, `task #528`, workspace paths, hub library
//! routes). Code is never split. Pure.

use crate::url::{classify_url, hub_route, strip_scheme};
use crate::{Ctx, Ref, Span};

/// Splits `text` into spans. See the crate docs for the rules.
pub fn classify(text: &str, ctx: &Ctx) -> Vec<Span> {
    let spans = code_spans(text);
    let spans = map_text(spans, |t| markdown_spans(t, ctx));
    let spans = map_text(spans, |t| url_spans(t, ctx));
    let spans = map_text(spans, |t| word_spans(t, ctx));
    merge(spans)
}

/// Applies `f` to every plain-text span; the other spans pass through.
fn map_text(spans: Vec<Span>, f: impl Fn(&str) -> Vec<Span>) -> Vec<Span> {
    spans
        .into_iter()
        .flat_map(|span| match span {
            Span::Text(text) => f(&text),
            other => vec![other],
        })
        .collect()
}

/// Adjacent text spans become one; empty text spans go.
fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match span {
            Span::Text(text) if text.is_empty() => {}
            Span::Text(text) => match out.last_mut() {
                Some(Span::Text(prev)) => prev.push_str(&text),
                _ => out.push(Span::Text(text)),
            },
            other => out.push(other),
        }
    }
    out
}

fn push_text(out: &mut Vec<Span>, text: &str) {
    if !text.is_empty() {
        out.push(Span::Text(text.to_string()));
    }
}

/// Inline code: a backtick, one or more characters that are not a backtick or a newline, and a backtick.
fn code_spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while let Some(off) = text[i..].find('`') {
        let open = i + off;
        let body = open + 1;
        let line_end = text[body..].find('\n').map_or(text.len(), |k| body + k);
        match text[body..line_end].find('`') {
            Some(k) if k > 0 => {
                let close = body + k;
                push_text(&mut out, &text[plain_from..open]);
                out.push(Span::Code(text[open..=close].to_string()));
                plain_from = close + 1;
                i = close + 1;
            }
            _ => i = body,
        }
    }
    push_text(&mut out, &text[plain_from..]);
    out
}

/// `[label](target)` and `![alt](target)`, where the target is an http(s) URL, a hub route or a workspace path.
/// Anything else stays text.
fn markdown_spans(text: &str, ctx: &Ctx) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while let Some(off) = text[i..].find('[') {
        let open = i + off;
        if let Some((end, label, target)) = parse_link(text, open) {
            let bang = open > 0 && text.as_bytes()[open - 1] == b'!';
            let start = if bang { open - 1 } else { open };
            if let Some(r) = target_ref(target, ctx) {
                push_text(&mut out, &text[plain_from..start]);
                out.push(Span::Ref {
                    raw: text[start..end].to_string(),
                    r,
                    text: Some(label.to_string()),
                });
                plain_from = end;
                i = end;
                continue;
            }
        }
        i = open + 1;
    }
    push_text(&mut out, &text[plain_from..]);
    out
}

/// Parses `[label](target)` at `open` (the `[`). The label has no `]` or newline; the target has no whitespace and
/// its parentheses are balanced at one level. Returns the end offset, the label and the target.
fn parse_link(text: &str, open: usize) -> Option<(usize, &str, &str)> {
    let bytes = text.as_bytes();
    let close = open + 1 + text[open + 1..].find(']')?;
    let label = &text[open + 1..close];
    if label.is_empty() || label.contains('\n') || bytes.get(close + 1) != Some(&b'(') {
        return None;
    }
    let target_start = close + 2;
    let mut p = target_start;
    loop {
        match *bytes.get(p)? {
            b')' => break,
            b'(' => {
                let mut q = p + 1;
                while let Some(&b) = bytes.get(q) {
                    if b == b')' || b == b'(' || b.is_ascii_whitespace() {
                        break;
                    }
                    q += 1;
                }
                if bytes.get(q) != Some(&b')') {
                    return None;
                }
                p = q + 1;
            }
            b if b.is_ascii_whitespace() => return None,
            _ => p += 1,
        }
    }
    Some((p + 1, label, &text[target_start..p]))
}

/// The reference a markdown target names: an http(s) URL, a hub route or a workspace path.
fn target_ref(target: &str, ctx: &Ctx) -> Option<Ref> {
    if strip_scheme(target).is_some() {
        return classify_url(target, ctx);
    }
    local_ref(target, ctx)
}

/// Bare http(s) URLs. Trailing punctuation is text (the web renderer's rule).
fn url_spans(text: &str, ctx: &Ctx) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while let Some(start) = find_scheme(text, i) {
        let end = text[start..]
            .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
            .map_or(text.len(), |k| start + k);
        let mut stop = end;
        while stop > start
            && matches!(
                text.as_bytes()[stop - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'"' | b'*' | b'_' | b')' | b']'
            )
        {
            stop -= 1;
        }
        if let Some(r) = classify_url(&text[start..stop], ctx) {
            push_text(&mut out, &text[plain_from..start]);
            out.push(Span::Ref {
                raw: text[start..stop].to_string(),
                r,
                text: None,
            });
            plain_from = stop;
        }
        i = end;
    }
    push_text(&mut out, &text[plain_from..]);
    out
}

/// The next `http://` or `https://` at or after `from`, any case.
fn find_scheme(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    (from..bytes.len()).find(|&p| {
        (bytes[p] | 0x20) == b'h'
            && (text
                .get(p..p + 7)
                .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
                || text
                    .get(p..p + 8)
                    .is_some_and(|s| s.eq_ignore_ascii_case("https://")))
    })
}

/// Words: whitespace-separated tokens, with the punctuation around a word left as text.
fn word_spans(text: &str, ctx: &Ctx) -> Vec<Span> {
    let tokens = tokens(text);
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while i < tokens.len() {
        let (raw_a, raw_b) = tokens[i];
        let (a, b) = trim(text, raw_a, raw_b);
        if a >= b {
            i += 1;
            continue;
        }
        // Two words: `task #528`. `PR #12` and `pull #12` are text, and so is their `#12`. The gap is spaces only,
        // and the first word has no trailing punctuation.
        if let Some(&(next_a, next_b)) = tokens.get(i + 1) {
            let gap = &text[raw_b..next_a];
            if b == raw_b && !gap.is_empty() && gap.bytes().all(|c| c == b' ' || c == b'\t') {
                let (c, d) = trim(text, next_a, next_b);
                let word = &text[a..b];
                if let (true, Some(number)) =
                    (word.eq_ignore_ascii_case("task"), hash_number(&text[c..d]))
                {
                    push_text(&mut out, &text[plain_from..a]);
                    out.push(Span::Ref {
                        raw: text[a..d].to_string(),
                        r: Ref::Task { number },
                        text: None,
                    });
                    plain_from = d;
                    i += 2;
                    continue;
                }
                let is_pr = word.eq_ignore_ascii_case("pr") || word.eq_ignore_ascii_case("pull");
                if is_pr && hash_number(&text[c..d]).is_some() {
                    i += 2;
                    continue;
                }
            }
        }
        if let Some(r) = single_word(&text[a..b], ctx) {
            push_text(&mut out, &text[plain_from..a]);
            out.push(Span::Ref {
                raw: text[a..b].to_string(),
                r,
                text: None,
            });
            plain_from = b;
        }
        i += 1;
    }
    push_text(&mut out, &text[plain_from..]);
    out
}

/// One word: `#N` (a task, in a project), `@all`, `@Name`, `agent:<cid>`, a hub route, or a workspace path.
fn single_word(word: &str, ctx: &Ctx) -> Option<Ref> {
    if let Some(number) = hash_number(word) {
        return ctx.project.as_ref().map(|_| Ref::Task { number });
    }
    if let Some(name) = word.strip_prefix('@') {
        if name.eq_ignore_ascii_case("all") {
            return Some(Ref::MentionAll);
        }
        return ctx
            .agents
            .iter()
            .any(|a| a.name == name)
            .then(|| Ref::Agent {
                name: Some(name.to_string()),
                cid: None,
            });
    }
    if let Some(digits) = word.strip_prefix("agent:") {
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return digits.parse().ok().map(|cid| Ref::Agent {
                name: None,
                cid: Some(cid),
            });
        }
        return None;
    }
    local_ref(word.strip_prefix("file:").unwrap_or(word), ctx)
}

/// A relative target: a hub route (`/v1/files/<id>`), or a workspace path with an optional line.
fn local_ref(target: &str, ctx: &Ctx) -> Option<Ref> {
    if target.starts_with("/v1/") {
        let end = target.find(['?', '#']).unwrap_or(target.len());
        let segments: Vec<&str> = target[..end].split('/').filter(|s| !s.is_empty()).collect();
        return hub_route(&segments);
    }
    let (path, line) = split_line(target);
    if path.is_empty() {
        return None;
    }
    if ctx.paths.contains(path) {
        // A file as given, or a folder given with its `/`. A folder has no line.
        let dir = path.ends_with('/');
        return match (dir, line) {
            (true, Some(_)) => None,
            _ => Some(Ref::Path {
                path: path.to_string(),
                line,
                dir,
            }),
        };
    }
    // A folder given without its `/`.
    let folder = format!("{path}/");
    (line.is_none() && !path.ends_with('/') && ctx.paths.contains(&folder)).then_some(Ref::Path {
        path: folder,
        line: None,
        dir: true,
    })
}

/// `path:12` and `path#L12` give the line; anything else is the path itself.
fn split_line(target: &str) -> (&str, Option<u32>) {
    let hash = target
        .rfind("#L")
        .and_then(|i| all_digits(&target[i + 2..]).map(|line| (&target[..i], line)));
    let colon = target
        .rfind(':')
        .and_then(|i| all_digits(&target[i + 1..]).map(|line| (&target[..i], line)));
    match hash.or(colon) {
        Some((path, line)) => (path, Some(line)),
        None => (target, None),
    }
}

fn all_digits(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// `#528` gives 528; anything else None.
fn hash_number(word: &str) -> Option<u64> {
    let digits = word.strip_prefix('#')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Maximal runs of non-whitespace, as byte ranges.
fn tokens(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

/// A token without the brackets and punctuation around it: `(src/a.rs),` gives `src/a.rs`.
fn trim(text: &str, mut a: usize, mut b: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    while a < b && matches!(bytes[a], b'(' | b'[' | b'{' | b'"' | b'\'' | b'<') {
        a += 1;
    }
    while b > a
        && matches!(
            bytes[b - 1],
            b'.' | b','
                | b';'
                | b':'
                | b'!'
                | b'?'
                | b'\''
                | b'"'
                | b'*'
                | b'_'
                | b')'
                | b']'
                | b'}'
                | b'>'
        )
    {
        b -= 1;
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_kept_whole_and_an_unclosed_backtick_is_text() {
        let spans = classify(
            "see `https://github.com/o/r/pull/1` and `unclosed",
            &Ctx::default(),
        );
        assert_eq!(
            spans,
            vec![
                Span::Text("see ".into()),
                Span::Code("`https://github.com/o/r/pull/1`".into()),
                Span::Text(" and `unclosed".into()),
            ]
        );
    }

    #[test]
    fn spans_join_back_to_the_input() {
        let ctx = Ctx {
            project: Some("pr1".into()),
            ..Ctx::default()
        };
        let text =
            "x `c` [a](https://example.com/a) #3 task #4, https://github.com/o/r/pull/5. end";
        let joined: String = classify(text, &ctx)
            .iter()
            .map(|s| match s {
                Span::Text(t) | Span::Code(t) => t.as_str(),
                Span::Ref { raw, .. } => raw.as_str(),
            })
            .collect();
        assert_eq!(joined, text);
    }
}
