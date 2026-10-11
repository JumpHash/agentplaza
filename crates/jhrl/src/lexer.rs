//! Lexer (docs/jhrl.md section 4): identifiers, primes, strings (interpolations kept raw),
//! numbers with units converted to base units, requirement ids, Unicode operator aliases.

use crate::num::{unit, unit_expr, Dim, Num};
use crate::span::{Diag, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    /// A `'` written directly after an identifier.
    Prime,
    /// String contents with escapes resolved; `{…}` interpolations are left as text.
    Str(String),
    /// A number, already multiplied into base units. `text` is the literal as written.
    Num { value: Num, dim: Dim, text: String },
    ReqId(String),
    Sym(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// A line break separates this token from the previous one.
    pub nl_before: bool,
}

pub const RESERVED: &[&str] = &[
    "for", "in", "where", "let", "require", "if", "then", "else", "and", "or", "not", "is", "true", "false",
    "yield", "when", "always", "eventually", "leads_to",
];

const SYMS: &[&str] = &[
    "..", "==", "!=", "<=", ">=", "=>", "->", "??", "||", "~>", "(", ")", "[", "]", "{", "}", ",", ";", ":", ".",
    "=", "<", ">", "+", "-", "*", "/", "^", "|", "?",
];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub fn lex(file: u32, text: &str) -> Result<Vec<Token>, Diag> {
    let mut out: Vec<Token> = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    let mut nl = false;
    let sp = |lo: usize, hi: usize| Span::new(file, lo, hi);
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        // whitespace and comments
        if c == '\n' {
            nl = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if text[i..].starts_with("//") {
            while i < text.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if text[i..].starts_with("/*") {
            match text[i + 2..].find("*/") {
                Some(j) => {
                    if text[i..i + 2 + j].contains('\n') {
                        nl = true;
                    }
                    i += j + 4;
                }
                None => return Err(Diag::error(sp(i, i + 2), "unterminated comment")),
            }
            continue;
        }
        let start = i;
        let tok;
        if is_ident_start(c) {
            let mut j = i;
            while j < text.len() && is_ident_char(b[j] as char) {
                j += 1;
            }
            let word = &text[i..j];
            // Requirement id: AP-NNN or AP-NNN.Rn
            if word == "AP" && text[j..].starts_with('-') && text[j + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                let mut k = j + 1;
                while k < text.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k - (j + 1) != 3 {
                    return Err(Diag::error(sp(i, k), "a requirement id has three digits: AP-NNN.Rn"));
                }
                if text[k..].starts_with(".R") && text[k + 2..].starts_with(|c: char| c.is_ascii_digit()) {
                    k += 2;
                    while k < text.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                }
                tok = Tok::ReqId(text[i..k].to_string());
                i = k;
            } else {
                tok = Tok::Ident(word.to_string());
                i = j;
            }
        } else if c.is_ascii_digit() {
            let (t, j) = lex_number(file, text, i)?;
            tok = t;
            i = j;
        } else if c == '"' {
            let mut s = String::new();
            let mut j = i + 1;
            loop {
                let Some(ch) = text[j..].chars().next() else {
                    return Err(Diag::error(sp(i, j), "unterminated string"));
                };
                match ch {
                    '"' => {
                        j += 1;
                        break;
                    }
                    '\\' => {
                        let e = text[j + 1..].chars().next();
                        s.push(match e {
                            Some('"') => '"',
                            Some('\\') => '\\',
                            Some('n') => '\n',
                            Some('t') => '\t',
                            _ => return Err(Diag::error(sp(j, j + 2), "unknown escape; use \\\" \\\\ \\n or \\t")),
                        });
                        j += 2;
                    }
                    '\n' => return Err(Diag::error(sp(i, j), "unterminated string (strings are one line)")),
                    _ => {
                        s.push(ch);
                        j += ch.len_utf8();
                    }
                }
            }
            tok = Tok::Str(s);
            i = j;
        } else if c == '\'' {
            let adjacent = out.last().is_some_and(|t| matches!(t.tok, Tok::Ident(_)) && t.span.hi as usize == i);
            if !adjacent {
                return Err(Diag::error(sp(i, i + 1), "a prime `'` must follow a variable name directly"));
            }
            tok = Tok::Prime;
            i += 1;
        } else {
            let alias = match c {
                '≤' => Some("<="),
                '≥' => Some(">="),
                '≠' => Some("!="),
                '·' | '×' => Some("*"),
                '→' => Some("->"),
                '±' => Some("±"),
                '□' => Some("□"),
                '◇' => Some("◇"),
                _ => None,
            };
            if let Some(a) = alias {
                tok = match a {
                    "□" => Tok::Ident("always".into()),
                    "◇" => Tok::Ident("eventually".into()),
                    _ => Tok::Sym(a),
                };
                i += c.len_utf8();
            } else if let Some(s) = SYMS.iter().find(|s| text[i..].starts_with(**s)) {
                tok = if *s == "~>" { Tok::Ident("leads_to".into()) } else { Tok::Sym(s) };
                i += s.len();
            } else {
                return Err(Diag::error(sp(i, i + c.len_utf8()), format!("unexpected character `{c}`")));
            }
        }
        out.push(Token { tok, span: sp(start, i), nl_before: nl });
        nl = false;
    }
    out.push(Token { tok: Tok::Eof, span: sp(text.len(), text.len()), nl_before: true });
    Ok(out)
}

/// Length of a unit atom (`cr`, `KiB`, `%`) at the start of `s`, if one is there.
fn unit_atom(s: &str) -> Option<usize> {
    if s.starts_with('%') {
        return Some(1);
    }
    let n = s.bytes().take_while(|c| c.is_ascii_alphabetic()).count();
    if n == 0 || s[n..].starts_with(|c: char| is_ident_char(c)) {
        return None;
    }
    unit(&s[..n]).map(|_| n)
}

/// Matches a unit expression (`cr`, `cr/d`, `/min`, `cr/Mtok`) at the start of `s`; returns its length.
fn unit_len(s: &str) -> Option<usize> {
    let mut k = 0;
    if let Some(n) = unit_atom(s) {
        k = n;
    } else if !s.starts_with('/') {
        return None;
    }
    while s[k..].starts_with('/') {
        match unit_atom(&s[k + 1..]) {
            Some(n) => k += 1 + n,
            None => break,
        }
    }
    if k == 0 {
        None
    } else {
        Some(k)
    }
}

fn lex_number(file: u32, text: &str, i: usize) -> Result<(Tok, usize), Diag> {
    let b = text.as_bytes();
    let mut j = i;
    let digits = |j: &mut usize| {
        while *j < text.len() && (b[*j].is_ascii_digit() || b[*j] == b'_') {
            *j += 1;
        }
    };
    digits(&mut j);
    if j + 1 < text.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
        j += 1;
        digits(&mut j);
    }
    if j < text.len() && (b[j] == b'e' || b[j] == b'E') {
        let mut k = j + 1;
        if k < text.len() && (b[k] == b'+' || b[k] == b'-') {
            k += 1;
        }
        if k < text.len() && b[k].is_ascii_digit() {
            j = k;
            digits(&mut j);
        }
    }
    let lit = &text[i..j];
    let mut value = Num::parse_decimal(lit).map_err(|e| Diag::error(Span::new(file, i, j), e))?;
    let mut dim = Dim::NONE;
    let mut end = j;
    // A unit directly after the number, or after exactly one space.
    let ustart = if text[j..].starts_with(' ') && !text[j + 1..].starts_with(' ') { j + 1 } else { j };
    if let Some(n) = unit_len(&text[ustart..]) {
        let (f, d) = unit_expr(&text[ustart..ustart + n]).map_err(|e| Diag::error(Span::new(file, ustart, ustart + n), e))?;
        value = value.mul(f).map_err(|e| Diag::error(Span::new(file, i, ustart + n), e))?;
        dim = d;
        end = ustart + n;
    }
    Ok((Tok::Num { value, dim, text: text[i..end].to_string() }, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(0, s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn units_convert_to_base() {
        match &toks("16 KiB")[0] {
            Tok::Num { value, dim, .. } => {
                assert_eq!(*value, Num::int(16384));
                assert_eq!(*dim, Dim::INFO);
            }
            t => panic!("{t:?}"),
        }
        match &toks("30 /min")[0] {
            Tok::Num { value, dim, .. } => {
                assert_eq!(*value, Num::rat(1, 2).unwrap());
                assert_eq!(*dim, Dim::TIME.inv());
            }
            t => panic!("{t:?}"),
        }
        match &toks("1000 cr/d")[0] {
            Tok::Num { dim, .. } => assert_eq!(*dim, Dim::CREDITS.div(Dim::TIME)),
            t => panic!("{t:?}"),
        }
        match &toks("20 %")[0] {
            Tok::Num { value, dim, .. } => assert_eq!((*value, *dim), (Num::rat(1, 5).unwrap(), Dim::NONE)),
            t => panic!("{t:?}"),
        }
        for (src, d) in [("5 ms", Dim::TIME), ("2 wk", Dim::TIME), ("3 Mtok", Dim::TOKENS), ("1 GB", Dim::INFO), ("7 kcr", Dim::CREDITS)] {
            match &toks(src)[0] {
                Tok::Num { dim, .. } => assert_eq!(*dim, d, "{src}"),
                t => panic!("{t:?}"),
            }
        }
    }

    #[test]
    fn a_unit_needs_a_word_boundary_and_at_most_one_space() {
        // `in` is not a unit; two spaces end the literal; `x / s` with spaces is division.
        assert!(matches!(toks("1 in")[1], Tok::Ident(_)));
        assert!(matches!(toks("1  cr")[1], Tok::Ident(_)));
        assert_eq!(toks("n / s").len(), 4);
        assert!(matches!(toks("2 crs")[1], Tok::Ident(_)));
        assert!(matches!(toks("0 cr .. 3 cr")[1], Tok::Sym("..")));
    }

    #[test]
    fn reqids_primes_aliases_strings() {
        assert_eq!(toks("AP-073.R6")[0], Tok::ReqId("AP-073.R6".into()));
        assert_eq!(toks("AP-073")[0], Tok::ReqId("AP-073".into()));
        assert_eq!(toks("used'")[1], Tok::Prime);
        assert!(lex(0, "used '").is_err());
        assert_eq!(toks("a ≤ b ≠ c → d"), toks("a <= b != c -> d"));
        assert_eq!(toks("a · b × c"), toks("a * b * c"));
        assert_eq!(toks("□ p ~> q"), toks("always p leads_to q"));
        assert_eq!(toks(r#""a {x} \"q\"""#)[0], Tok::Str("a {x} \"q\"".into()));
        assert_eq!(toks("1 /* c */ // d\n2").len(), 3);
        let t = lex(0, "a\nb").unwrap();
        assert!(!t[0].nl_before || t[1].nl_before);
        assert!(t[1].nl_before);
    }
}
