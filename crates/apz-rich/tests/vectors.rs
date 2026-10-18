//! The shared vectors: `vectors.json` is an array of `{name, text, ctx, spans}`. The web's TypeScript port runs the
//! same file, so the two classifiers must agree on every case.

use std::collections::BTreeSet;

use apz_rich::{Ctx, Span, classify};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vector {
    name: String,
    text: String,
    ctx: Ctx,
    spans: Vec<Span>,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("vectors.json")).expect("vectors.json parses")
}

fn raw_of(span: &Span) -> &str {
    match span {
        Span::Text(text) | Span::Code(text) => text,
        Span::Ref { raw, .. } => raw,
    }
}

#[test]
fn every_vector_classifies_to_its_spans() {
    for case in vectors() {
        assert_eq!(
            classify(&case.text, &case.ctx),
            case.spans,
            "case {}",
            case.name
        );
    }
}

#[test]
fn spans_join_back_to_the_text() {
    for case in vectors() {
        let joined: String = case.spans.iter().map(raw_of).collect();
        assert_eq!(joined, case.text, "case {}", case.name);
    }
}

#[test]
fn the_vectors_cover_at_least_sixty_cases_and_every_reference_kind() {
    let cases = vectors();
    assert!(cases.len() >= 60, "{} cases", cases.len());
    let mut kinds = BTreeSet::new();
    for case in &cases {
        for span in &case.spans {
            if let Span::Ref { r, .. } = span {
                let json = serde_json::to_value(r).expect("a ref serialises");
                kinds.insert(json["kind"].as_str().expect("a kind").to_string());
            }
        }
    }
    let every = [
        "github_pull",
        "github_issue",
        "github_commit",
        "github_repo",
        "github_file",
        "github_release",
        "github_run",
        "gist",
        "agent",
        "mention_all",
        "task",
        "path",
        "library",
        "library_tool",
        "image",
        "video",
        "html",
        "link",
    ];
    let missing: Vec<&str> = every.into_iter().filter(|k| !kinds.contains(*k)).collect();
    assert!(missing.is_empty(), "no vector for {missing:?}");
}
