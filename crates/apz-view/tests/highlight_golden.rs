//! Pins the highlighter to `tests/highlight_golden.json`, the vectors the TypeScript twin is checked against.
//! Run with `APZ_REGEN=1` to rewrite the expected spans from the current implementation, and read the diff before
//! committing one.

use apz_view::highlight::{Span, spans};

#[derive(serde::Serialize, serde::Deserialize)]
struct Case {
    language: String,
    text: String,
    spans: Vec<Span>,
}

#[test]
fn golden_vectors_hold() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/highlight_golden.json");
    let raw = std::fs::read_to_string(path).expect("the golden file reads");
    let mut cases: Vec<Case> = serde_json::from_str(&raw).expect("the golden file parses");
    if std::env::var_os("APZ_REGEN").is_some() {
        for case in &mut cases {
            case.spans = spans(&case.language, &case.text);
        }
        let mut out = serde_json::to_string_pretty(&cases).expect("the cases serialise");
        out.push('\n');
        std::fs::write(path, out).expect("the golden file writes");
        return;
    }
    for case in &cases {
        assert_eq!(
            spans(&case.language, &case.text),
            case.spans,
            "{}: {:?}",
            case.language,
            case.text
        );
    }
}
