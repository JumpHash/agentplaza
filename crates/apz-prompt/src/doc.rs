//! A line-level view of a markdown body that every rule reads: which lines are inside code fences, which are
//! headings, list items or blank, and the sections between headings. Not a full CommonMark parser; it reads the
//! subset prompts use (ATX headings, ``` fences, `-`/`*`/`+`/`1.` lists, paragraphs).

/// What one line is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Blank,
    /// `#`..`######` and the heading text, trimmed (closing `#`s removed).
    Heading {
        level: u8,
        text: String,
    },
    /// A fence line (opening or closing). `lang` is the info string on an opening fence, empty if none.
    Fence {
        opening: bool,
        lang: String,
    },
    /// A line inside a fenced block.
    Code,
    /// A list item: its indent (spaces) and its text after the marker.
    Item {
        indent: u32,
        text: String,
    },
    /// Any other line.
    Text,
}

#[derive(Debug, Clone)]
pub struct Line<'a> {
    /// 1-based.
    pub number: u32,
    pub raw: &'a str,
    pub block: Block,
}

/// A section: a heading and the lines under it until the next heading of the same or a higher level.
/// The lines before the first heading form a section with `level` 0 and an empty title.
#[derive(Debug, Clone)]
pub struct Section {
    pub level: u8,
    pub title: String,
    /// The heading's line (0 for the preamble).
    pub line: u32,
    /// The line numbers under the heading, up to the next heading of any level (its own body, not subsections).
    pub body: Vec<u32>,
    /// The line numbers under the heading up to the next heading of the same or a higher level (subsections
    /// included).
    pub span: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Doc<'a> {
    pub lines: Vec<Line<'a>>,
    /// Whether the last fence was left open at the end of the body.
    pub open_fence: bool,
}

impl<'a> Doc<'a> {
    pub fn parse(body: &'a str) -> Doc<'a> {
        let mut lines = Vec::new();
        let mut in_fence = false;
        for (i, raw) in body.lines().enumerate() {
            let number = i as u32 + 1;
            let trimmed = raw.trim_start();
            let block = if let Some(rest) = trimmed.strip_prefix("```") {
                in_fence = !in_fence;
                Block::Fence {
                    opening: in_fence,
                    lang: if in_fence {
                        rest.trim().to_string()
                    } else {
                        String::new()
                    },
                }
            } else if in_fence {
                Block::Code
            } else if raw.trim().is_empty() {
                Block::Blank
            } else if let Some((level, text)) = heading(raw) {
                Block::Heading { level, text }
            } else if let Some((indent, text)) = item(raw) {
                Block::Item { indent, text }
            } else {
                Block::Text
            };
            lines.push(Line { number, raw, block });
        }
        Doc {
            lines,
            open_fence: in_fence,
        }
    }

    pub fn line(&self, number: u32) -> Option<&Line<'a>> {
        self.lines.get(number.checked_sub(1)? as usize)
    }

    /// The headings: (line, level, text).
    pub fn headings(&self) -> impl Iterator<Item = (u32, u8, &str)> {
        self.lines.iter().filter_map(|l| match &l.block {
            Block::Heading { level, text } => Some((l.number, *level, text.as_str())),
            _ => None,
        })
    }

    /// Lines outside code fences that are prose (text, list items, headings), with their numbers.
    pub fn prose(&self) -> impl Iterator<Item = &Line<'a>> {
        self.lines.iter().filter(|l| {
            matches!(
                l.block,
                Block::Text | Block::Item { .. } | Block::Heading { .. }
            )
        })
    }

    /// The sections, in order, the preamble first when there are lines before the first heading.
    pub fn sections(&self) -> Vec<Section> {
        let heads: Vec<(u32, u8, String)> = self
            .headings()
            .map(|(n, l, t)| (n, l, t.to_string()))
            .collect();
        let last = self.lines.len() as u32;
        let mut out = Vec::new();
        let first = heads.first().map_or(last + 1, |h| h.0);
        if first > 1 {
            let body: Vec<u32> = (1..first).collect();
            out.push(Section {
                level: 0,
                title: String::new(),
                line: 0,
                body: body.clone(),
                span: body,
            });
        }
        for (i, (line, level, title)) in heads.iter().enumerate() {
            let next_any = heads.get(i + 1).map_or(last + 1, |h| h.0);
            let next_same = heads[i + 1..]
                .iter()
                .find(|h| h.1 <= *level)
                .map_or(last + 1, |h| h.0);
            out.push(Section {
                level: *level,
                title: title.clone(),
                line: *line,
                body: (line + 1..next_any).collect(),
                span: (line + 1..next_same).collect(),
            });
        }
        out
    }
}

fn heading(raw: &str) -> Option<(u8, String)> {
    let lead = raw.len() - raw.trim_start_matches(' ').len();
    if lead > 3 {
        return None;
    }
    let s = &raw[lead..];
    let hashes = s.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end().to_string();
    Some((hashes as u8, text))
}

fn item(raw: &str) -> Option<(u32, String)> {
    let indent = (raw.len() - raw.trim_start().len()) as u32;
    let s = raw.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(marker) {
            return Some((indent, rest.trim().to_string()));
        }
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &s[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((indent, rest.trim().to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_parse_classifies_lines_and_code_fences() {
        // classifies_lines: each line gets its block kind
        {
            let doc = Doc::parse(
                "# Title\n\nText\n- item\n  2. sub\n```rust\nlet x = 1;\n```\n#nospace\n",
            );
            let kinds: Vec<_> = doc.lines.iter().map(|l| &l.block).collect();
            assert_eq!(
                kinds[0],
                &Block::Heading {
                    level: 1,
                    text: "Title".into()
                }
            );
            assert_eq!(kinds[1], &Block::Blank);
            assert_eq!(kinds[2], &Block::Text);
            assert_eq!(
                kinds[3],
                &Block::Item {
                    indent: 0,
                    text: "item".into()
                }
            );
            assert_eq!(
                kinds[4],
                &Block::Item {
                    indent: 2,
                    text: "sub".into()
                }
            );
            assert_eq!(
                kinds[5],
                &Block::Fence {
                    opening: true,
                    lang: "rust".into()
                }
            );
            assert_eq!(kinds[6], &Block::Code);
            assert_eq!(
                kinds[7],
                &Block::Fence {
                    opening: false,
                    lang: String::new()
                }
            );
            assert_eq!(kinds[8], &Block::Text);
            assert!(!doc.open_fence);
        }
        // a_hash_inside_a_fence_is_code: a # inside an open fence is not a heading
        {
            let doc = Doc::parse("```\n# not a heading\n");
            assert_eq!(doc.headings().count(), 0);
            assert!(doc.open_fence);
        }
    }

    #[test]
    fn sections_split_body_and_span() {
        let doc = Doc::parse("pre\n# A\na\n## B\nb\n# C\n");
        let s = doc.sections();
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].level, s[0].body.clone()), (0, vec![1]));
        assert_eq!(
            (s[1].title.as_str(), s[1].body.clone(), s[1].span.clone()),
            ("A", vec![3], vec![3, 4, 5])
        );
        assert_eq!((s[2].title.as_str(), s[2].body.clone()), ("B", vec![5]));
        assert!(s[3].body.is_empty());
    }
}
