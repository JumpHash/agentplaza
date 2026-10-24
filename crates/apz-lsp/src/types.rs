//! Protocol values: positions, locations, diagnostics and symbols, read from and written to JSON.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// A zero-based line and a character offset in UTF-16 code units, as the protocol counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

impl Location {
    /// The file this location is in, when it names one.
    pub fn path(&self) -> Option<PathBuf> {
        uri_to_path(&self.uri)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint; absent when the server does not say.
    pub severity: Option<u8>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    /// The kind in words: `function`, `method`, `class`, …
    pub kind: &'static str,
    pub detail: Option<String>,
    pub range: Range,
    pub children: Vec<DocumentSymbol>,
}

/// The `file://` URI of an absolute path, with the characters a URI cannot carry percent-encoded.
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/');
    }
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The path a `file://` URI names; `None` for another scheme or a malformed escape.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = rest.get(i + 1..i + 3)?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let mut text = String::from_utf8(bytes).ok()?;
    // `file:///C:/x` names `C:/x` on Windows.
    if text.len() >= 3 && text.starts_with('/') && text.as_bytes()[2] == b':' {
        text.remove(0);
    }
    Some(PathBuf::from(text))
}

/// The protocol position of 1-based `line` and `column` (counted in characters) in `text`.
pub fn position_at(text: &str, line: usize, column: usize) -> Option<Position> {
    let line_text = text.split('\n').nth(line.checked_sub(1)?)?;
    let units: u32 = line_text
        .chars()
        .take(column.saturating_sub(1))
        .map(|ch| ch.len_utf16() as u32)
        .sum();
    Some(Position {
        line: u32::try_from(line - 1).ok()?,
        character: units,
    })
}

/// The name of an LSP `SymbolKind`.
pub fn symbol_kind(n: u64) -> &'static str {
    match n {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type parameter",
        _ => "symbol",
    }
}

/// The `SymbolKind` number of a kind name (the reverse of [`symbol_kind`]); 13 (variable) for an unknown name.
pub fn symbol_kind_number(name: &str) -> u64 {
    (1..=26).find(|n| symbol_kind(*n) == name).unwrap_or(13)
}

fn position(v: &Value) -> Option<Position> {
    Some(Position {
        line: u32::try_from(v.get("line")?.as_u64()?).ok()?,
        character: u32::try_from(v.get("character")?.as_u64()?).ok()?,
    })
}

fn range(v: &Value) -> Option<Range> {
    Some(Range {
        start: position(v.get("start")?)?,
        end: position(v.get("end")?)?,
    })
}

/// A definition's or a reference's answer: `Location`, `Location[]`, `LocationLink[]`, or null.
pub fn parse_locations(v: &Value) -> Vec<Location> {
    let items: Vec<&Value> = match v {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    };
    items
        .into_iter()
        .filter_map(|item| match item.get("targetUri").and_then(Value::as_str) {
            Some(uri) => {
                let at = item
                    .get("targetSelectionRange")
                    .or_else(|| item.get("targetRange"))
                    .and_then(range)?;
                Some(Location {
                    uri: uri.to_string(),
                    range: at,
                })
            }
            None => Some(Location {
                uri: item.get("uri")?.as_str()?.to_string(),
                range: range(item.get("range")?)?,
            }),
        })
        .collect()
}

pub fn parse_diagnostics(v: &Value) -> Vec<Diagnostic> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Diagnostic {
                range: range(d.get("range")?)?,
                severity: d
                    .get("severity")
                    .and_then(Value::as_u64)
                    .and_then(|s| u8::try_from(s).ok()),
                source: d.get("source").and_then(Value::as_str).map(str::to_string),
                message: d.get("message")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// `DocumentSymbol[]` (hierarchical) or `SymbolInformation[]` (flat, which a server may answer anyway).
pub fn parse_document_symbols(v: &Value) -> Vec<DocumentSymbol> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let kind = symbol_kind(s.get("kind").and_then(Value::as_u64).unwrap_or(0));
            let (at, children) = match (s.get("range"), s.get("location")) {
                (Some(r), _) => {
                    let children = match s.get("children") {
                        Some(c) => parse_document_symbols(c),
                        None => Vec::new(),
                    };
                    (range(r)?, children)
                }
                (None, Some(loc)) => (range(loc.get("range")?)?, Vec::new()),
                _ => return None,
            };
            Some(DocumentSymbol {
                name,
                kind,
                detail: s.get("detail").and_then(Value::as_str).map(str::to_string),
                range: at,
                children,
            })
        })
        .collect()
}

pub fn position_json(p: Position) -> Value {
    json!({ "line": p.line, "character": p.character })
}

pub fn range_json(r: Range) -> Value {
    json!({ "start": position_json(r.start), "end": position_json(r.end) })
}

pub fn location_json(l: &Location) -> Value {
    json!({ "uri": l.uri, "range": range_json(l.range) })
}

pub fn diagnostic_json(d: &Diagnostic) -> Value {
    let mut v = json!({ "range": range_json(d.range), "message": d.message });
    if let Some(severity) = d.severity {
        v["severity"] = json!(severity);
    }
    if let Some(source) = &d.source {
        v["source"] = json!(source);
    }
    v
}

pub fn document_symbol_json(s: &DocumentSymbol) -> Value {
    json!({
        "name": s.name,
        "kind": symbol_kind_number(s.kind),
        "detail": s.detail,
        "range": range_json(s.range),
        "selectionRange": range_json(s.range),
        "children": s.children.iter().map(document_symbol_json).collect::<Vec<Value>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_escapes() {
        let path = PathBuf::from("/work/my repo/src/día.rs");
        let uri = path_to_uri(&path);
        assert_eq!(uri, "file:///work/my%20repo/src/d%C3%ADa.rs");
        assert_eq!(uri_to_path(&uri), Some(path));
        assert_eq!(uri_to_path("https://example.com/x"), None);
    }

    #[test]
    fn columns_are_counted_in_utf16_units() {
        // "é" is one character and one unit; "𝄞" (U+1D11E) is one character and two units.
        let text = "let a = \"𝄞\";\nlet b = 1;\n";
        assert_eq!(
            position_at(text, 1, 11),
            Some(Position {
                line: 0,
                character: 11
            })
        );
        assert_eq!(
            position_at(text, 2, 5),
            Some(Position {
                line: 1,
                character: 4
            })
        );
        assert_eq!(position_at(text, 9, 1), None);
    }

    #[test]
    fn locations_come_from_any_of_the_three_shapes() {
        let loc = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}});
        let link = json!({"targetUri": "file:///b.rs", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 0}}, "targetSelectionRange": {"start": {"line": 2, "character": 3}, "end": {"line": 2, "character": 6}}});
        assert_eq!(parse_locations(&Value::Null), vec![]);
        assert_eq!(parse_locations(&loc).len(), 1);
        let links = parse_locations(&json!([loc.clone(), link]));
        assert_eq!(links.len(), 2);
        assert_eq!(
            links[1].range.start.line, 2,
            "the selection range names the definition"
        );
    }

    #[test]
    fn document_symbols_nest_and_flat_ones_still_parse() {
        let nested = json!([{"name": "Plaza", "kind": 5, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}, "selectionRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}, "children": [{"name": "open", "kind": 6, "range": {"start": {"line": 1, "character": 2}, "end": {"line": 2, "character": 2}}, "selectionRange": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 6}}}]}]);
        let symbols = parse_document_symbols(&nested);
        assert_eq!(symbols[0].kind, "class");
        assert_eq!(symbols[0].children[0].name, "open");
        assert_eq!(symbols[0].children[0].kind, "method");
        let flat = json!([{"name": "f", "kind": 12, "location": {"uri": "file:///a.rs", "range": {"start": {"line": 4, "character": 0}, "end": {"line": 5, "character": 0}}}}]);
        let flat = parse_document_symbols(&flat);
        assert_eq!((flat[0].kind, flat[0].range.start.line), ("function", 4));
        assert_eq!(symbol_kind_number("function"), 12);
    }
}
