//! The syntax highlighter both consoles share.
//!
//! The rules are data in `apps/shared/highlight.json`, which the web console reads too. [`spans`] scans text with
//! a fixed order of rules and returns the coloured runs as character spans, and [`token`] names the theme token
//! each class paints with. The TypeScript twin follows the same scan, and `tests/highlight_golden.json` pins the
//! spans that both must produce.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// The class of a highlighted run, and so its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Keyword,
    String,
    Type,
    Number,
    Comment,
    Function,
}

/// A coloured run of text. `start` and `end` are char (Unicode scalar) indices, and `end` is exclusive.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub class: Class,
}

/// One language's rules, as `apps/shared/highlight.json` writes them.
///
/// Markers match exactly. `ident_extra` lists the characters that continue an identifier beyond ASCII letters,
/// digits, `_` and `$`. `type_case` makes every capitalised identifier a type, and `ignore_case` matches keywords,
/// constants and types without regard to case.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct Language {
    pub aliases: Vec<String>,
    pub keywords: Vec<String>,
    pub types: Vec<String>,
    pub constants: Vec<String>,
    pub line_comments: Vec<String>,
    pub block_comments: Vec<(String, String)>,
    pub strings: Vec<String>,
    pub multiline_strings: Vec<String>,
    pub ident_extra: String,
    pub type_case: bool,
    pub ignore_case: bool,
}

#[derive(serde::Deserialize)]
struct Rules {
    languages: BTreeMap<String, Language>,
}

struct Registry {
    languages: BTreeMap<String, Language>,
    /// Lowercased names and aliases, each mapped to its key in `languages`.
    index: HashMap<String, String>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let rules: Rules =
            serde_json::from_str(include_str!("../../../apps/shared/highlight.json"))
                .expect("apps/shared/highlight.json is valid highlight rules");
        let mut index = HashMap::new();
        for (name, language) in &rules.languages {
            for key in std::iter::once(name).chain(language.aliases.iter()) {
                let previous = index.insert(key.to_lowercase(), name.clone());
                assert!(
                    previous.is_none(),
                    "highlight name or alias `{key}` is listed twice"
                );
            }
        }
        Registry {
            languages: rules.languages,
            index,
        }
    })
}

/// Returns the rules for a language by name or alias, ignoring case. Unknown names give `None`.
pub fn language(name: &str) -> Option<&'static Language> {
    let registry = registry();
    let key = registry.index.get(&name.to_lowercase())?;
    registry.languages.get(key)
}

/// Scans `text` as `language` and returns its coloured runs in order. An unknown language yields no spans.
///
/// At each char index the scan tries these rules in order, and the first one that applies consumes its run.
///
/// 1. A line comment runs to the end of the line, excluding the newline.
/// 2. A block comment runs through its end marker, or to the end of the text.
/// 3. A string runs from its delimiter to the same delimiter, unless that delimiter is preceded by an odd number
///    of backslashes. A delimiter from `strings` also ends at a newline, which is excluded. A delimiter from
///    `multiline_strings` runs to the end of the text when unclosed.
/// 4. A number starts at a digit, or at a `.` followed by a digit, when the previous char is not an identifier
///    char. It runs over alphanumerics, `_` and `.`, and over a `+` or `-` directly after `e` or `E`.
/// 5. An identifier starts at `_`, `$` or an alphabetic char and runs over identifier chars. It is a keyword, or
///    a constant (classed as a number), or a type. It is a function when the next char after spaces and tabs is
///    `(`, and otherwise it gets no span.
/// 6. Any other char is skipped.
///
/// Where several markers start at the same index, the longest one is used. Keyword, constant and type matching is
/// case-sensitive unless the language sets `ignore_case`.
pub fn spans(language: &str, text: &str) -> Vec<Span> {
    let Some(lang) = self::language(language) else {
        return Vec::new();
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if lang.line_comments.iter().any(|m| starts_with(&chars, i, m)) {
            let end = line_end(&chars, i);
            out.push(Span {
                start: i,
                end,
                class: Class::Comment,
            });
            i = end;
            continue;
        }
        let block = lang
            .block_comments
            .iter()
            .filter(|(open, _)| starts_with(&chars, i, open))
            .max_by_key(|(open, _)| open.chars().count());
        if let Some((open, close)) = block {
            let end = block_end(&chars, i, open, close);
            out.push(Span {
                start: i,
                end,
                class: Class::Comment,
            });
            i = end;
            continue;
        }
        let string = lang
            .strings
            .iter()
            .map(|d| (d.as_str(), false))
            .chain(lang.multiline_strings.iter().map(|d| (d.as_str(), true)))
            .filter(|(d, _)| starts_with(&chars, i, d))
            .max_by_key(|(d, _)| d.chars().count());
        if let Some((delimiter, multiline)) = string {
            let end = string_end(&chars, i, delimiter, multiline);
            out.push(Span {
                start: i,
                end,
                class: Class::String,
            });
            i = end;
            continue;
        }
        let after_ident = i > 0 && is_ident_char(chars[i - 1], lang);
        let dot_then_digit = c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit());
        if !after_ident && (c.is_ascii_digit() || dot_then_digit) {
            let mut j = i;
            while j < chars.len() {
                let d = chars[j];
                let exponent_sign =
                    j > i && (d == '+' || d == '-') && matches!(chars[j - 1], 'e' | 'E');
                if d.is_alphanumeric() || d == '_' || d == '.' || exponent_sign {
                    j += 1;
                } else {
                    break;
                }
            }
            out.push(Span {
                start: i,
                end: j,
                class: Class::Number,
            });
            i = j;
            continue;
        }
        if c == '_' || c == '$' || c.is_alphabetic() {
            let mut j = i;
            while j < chars.len() && is_ident_char(chars[j], lang) {
                j += 1;
            }
            if let Some(class) = classify(lang, &chars, i, j) {
                out.push(Span {
                    start: i,
                    end: j,
                    class,
                });
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// The theme token a class paints with. These names are the `syn-*` tokens in the theme stylesheets.
pub fn token(class: Class) -> &'static str {
    match class {
        Class::Keyword => "syn-keyword",
        Class::String => "syn-string",
        Class::Type => "syn-type",
        Class::Number => "syn-number",
        Class::Comment => "syn-comment",
        Class::Function => "syn-function",
    }
}

/// Whether a char continues an identifier in this language.
fn is_ident_char(c: char, lang: &Language) -> bool {
    c.is_ascii_alphanumeric()
        || c.is_alphabetic()
        || c == '_'
        || c == '$'
        || lang.ident_extra.contains(c)
}

/// Whether `marker` occurs in `chars` at `at`. An empty marker never occurs.
fn starts_with(chars: &[char], at: usize, marker: &str) -> bool {
    !marker.is_empty()
        && marker
            .chars()
            .enumerate()
            .all(|(k, m)| chars.get(at + k) == Some(&m))
}

/// The index of the newline ending the line that holds `from`, or the end of the text.
fn line_end(chars: &[char], from: usize) -> usize {
    chars[from..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |p| from + p)
}

/// The exclusive end of a block comment that opens at `at`.
fn block_end(chars: &[char], at: usize, open: &str, close: &str) -> usize {
    let mut j = at + open.chars().count();
    while j < chars.len() {
        if starts_with(chars, j, close) {
            return j + close.chars().count();
        }
        j += 1;
    }
    chars.len()
}

/// The exclusive end of a string that opens at `at` with `delimiter`.
fn string_end(chars: &[char], at: usize, delimiter: &str, multiline: bool) -> usize {
    let width = delimiter.chars().count();
    let mut j = at + width;
    while j < chars.len() {
        if starts_with(chars, j, delimiter) {
            let backslashes = chars[..j].iter().rev().take_while(|&&c| c == '\\').count();
            if backslashes % 2 == 0 {
                return j + width;
            }
        } else if !multiline && chars[j] == '\n' {
            return j;
        }
        j += 1;
    }
    chars.len()
}

/// Whether a word is listed, matching case only when the language asks for it to be ignored.
fn listed(words: &[String], word: &str, ignore_case: bool) -> bool {
    words.iter().any(|w| {
        if ignore_case {
            w.eq_ignore_ascii_case(word)
        } else {
            w.as_str() == word
        }
    })
}

/// The class of the identifier at `start..end`, or `None` when it gets no span.
fn classify(lang: &Language, chars: &[char], start: usize, end: usize) -> Option<Class> {
    let word: String = chars[start..end].iter().collect();
    let ci = lang.ignore_case;
    if listed(&lang.keywords, &word, ci) {
        return Some(Class::Keyword);
    }
    if listed(&lang.constants, &word, ci) {
        return Some(Class::Number);
    }
    if listed(&lang.types, &word, ci) || (lang.type_case && chars[start].is_uppercase()) {
        return Some(Class::Type);
    }
    let next = chars[end..].iter().find(|&&c| c != ' ' && c != '\t');
    (next == Some(&'(')).then_some(Class::Function)
}
