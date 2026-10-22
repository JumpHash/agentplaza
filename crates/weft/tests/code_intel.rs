//! The code-intelligence fallback (PLAN 15.3): where no language server answers, Weft's tree-sitter symbols do. The
//! W40 fixture (apz-lsp's testdata/repo, one file per language) is committed to a lane, and for each language with a
//! pack the definition and the call of its symbol are found in the revision's tree.

mod common;

use std::fs;

use common::plaza;
use weft::derived::Derived;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../apz-lsp/testdata/repo");

struct Case {
    path: String,
    language: String,
    symbol: String,
    definition_line: usize,
    call_line: Option<usize>,
}

fn cases() -> Vec<Case> {
    fs::read_to_string(format!("{FIXTURE}/cases.tsv"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|row| {
            let cols: Vec<&str> = row.split('\t').collect();
            let path = cols[0].to_string();
            Case {
                language: language_of(&path),
                path,
                symbol: cols[1].to_string(),
                definition_line: cols[2].parse().unwrap(),
                call_line: cols[3].parse().ok(),
            }
        })
        .collect()
}

/// The pack a fixture file uses, by its language's name (`csharp` is the `c_sharp` pack).
fn language_of(path: &str) -> String {
    let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "ts" => "typescript",
        "js" => "javascript",
        "swift" => "swift",
        "kt" => "kotlin",
        "java" => "java",
        "cs" => "csharp",
        "html" => "html",
        "css" => "css",
        "sh" => "bash",
        "ps1" => "powershell",
        "yaml" => "yaml",
        "toml" => "toml",
        "json" => "json",
        other => panic!("no language for .{other}"),
    }
    .to_string()
}

#[test]
fn weft_finds_each_fixture_symbol_and_its_call_for_every_language_with_a_pack() {
    let cases = cases();
    assert_eq!(cases.len(), 16, "one case per W40 language");
    let p = plaza("code-intel", &["repo"]);
    let packs: Vec<&str> = cases
        .iter()
        .filter(|c| c.language != "swift")
        .map(|c| {
            if c.language == "csharp" {
                "c_sharp"
            } else {
                c.language.as_str()
            }
        })
        .collect();
    p.add_packs(&packs);

    let mut drafts = Vec::new();
    for case in &cases {
        let text = fs::read_to_string(format!("{FIXTURE}/{}", case.path)).unwrap();
        drafts.push(p.put(&case.path, &text));
    }
    let change = p.lane("repo", 107, "w40-fixture", "W40 fixture", &[drafts]);
    let rev = format!("{change}.r1");

    let derived = Derived::new(p.dir.join("derived"));
    let mut tested = Vec::new();
    for case in &cases {
        if case.language == "swift" {
            // No Swift pack yet: the grammar publishes no WebAssembly (see the PR's report).
            eprintln!(
                "{}: no tree-sitter pack for swift; the fallback has nothing to answer",
                case.path
            );
            continue;
        }
        let symbol = case.symbol.clone();
        let (defs, refs) = derived
            .find_names(&p.store, &rev, |n| n.trim_start_matches('.') == symbol)
            .unwrap();
        let text = fs::read_to_string(format!("{FIXTURE}/{}", case.path)).unwrap();
        let parsed = derived.parse_bytes(&p.store, &case.path, text.as_bytes(), None);
        let pack = if case.language == "csharp" {
            "c_sharp"
        } else {
            case.language.as_str()
        };
        assert_eq!(
            parsed.pack.as_deref(),
            Some(pack),
            "{}: its pack",
            case.path
        );
        assert!(
            defs.iter()
                .any(|d| d.path == case.path && d.start_line == case.definition_line),
            "{}: `{}` is defined on line {} (got {defs:?})",
            case.path,
            case.symbol,
            case.definition_line
        );
        if let Some(call) = case.call_line {
            assert!(
                refs.iter().any(|r| r.path == case.path && r.line == call),
                "{}: `{}` is called on line {call} (got {refs:?})",
                case.path,
                case.symbol
            );
        }
        tested.push(case.language.clone());
    }
    eprintln!(
        "the fallback answered for {} languages: {tested:?}",
        tested.len()
    );
    assert_eq!(
        tested.len(),
        15,
        "every language but Swift, which has no pack yet"
    );
    std::fs::remove_dir_all(&p.dir).ok();
}
