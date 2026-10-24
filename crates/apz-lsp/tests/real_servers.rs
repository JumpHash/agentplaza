//! The servers installed on this machine, against the fixture repository: each must answer where the definition and
//! the references are. A language whose server is not installed must say so, which sends its tools to Weft.

use std::fs;
use std::path::{Path, PathBuf};

use apz_lsp::types::position_at;
use apz_lsp::{Language, Pool, Position, Timeouts, is_installed};

struct Case {
    path: String,
    definition_line: usize,
    call: Option<(usize, usize)>,
}

/// The fixture's manifest: one row per language, the symbol's definition line, and a call of it.
fn cases() -> Vec<Case> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/repo/cases.tsv");
    fs::read_to_string(manifest)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|row| {
            let cols: Vec<&str> = row.split('\t').collect();
            let call = match (cols[3], cols[4]) {
                ("-", _) => None,
                (line, col) => Some((line.parse().unwrap(), col.parse().unwrap())),
            };
            Case {
                path: cols[0].to_string(),
                definition_line: cols[2].parse().unwrap(),
                call,
            }
        })
        .collect()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/repo")
}

#[test]
fn installed_servers_answer_the_fixture_and_the_others_say_they_are_absent() {
    let root = repo();
    let pool = Pool::new(Timeouts::default());
    let mut answered = Vec::new();
    let mut servers = std::collections::BTreeSet::new();
    for case in cases() {
        let language = Language::from_path(&case.path).expect("every fixture file has a language");
        let file = root.join(&case.path);
        let text = fs::read_to_string(&file).unwrap();
        let Some((line, column)) = case.call else {
            continue;
        };
        if !is_installed(language) {
            let err = pool
                .definition(&root, &file, Position::default())
                .expect_err("no server here");
            assert!(
                err.to_string()
                    .starts_with(&format!("no server for {} here", language.name())),
                "{err}"
            );
            eprintln!(
                "{}: no {} server installed; the tools answer from Weft",
                case.path,
                language.name()
            );
            continue;
        }
        let at = position_at(&text, line, column).unwrap();
        let defs = match pool.definition(&root, &file, at) {
            Ok(defs) => defs,
            // On PATH but not runnable (a stub or a proxy with no binary behind it): not installed here.
            Err(apz_lsp::LspError::Closed { detail }) => {
                eprintln!(
                    "{}: {} is on PATH but does not run here ({detail}); skipped",
                    case.path,
                    language.server().command
                );
                continue;
            }
            Err(e) => panic!("{}: definition: {e}", case.path),
        };
        let wanted = fs::canonicalize(&file).unwrap();
        assert!(
            defs.iter().any(|d| {
                d.path().and_then(|p| fs::canonicalize(p).ok()).as_deref() == Some(wanted.as_path())
                    && d.range.start.line as usize + 1 == case.definition_line
            }),
            "{}: the definition is on line {} (got {defs:?})",
            case.path,
            case.definition_line
        );
        // A server answers references from its index, which it builds in the background (sourcekit-lsp does): on a
        // busy machine the first answers can come before it is built, so the test asks again for a while.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let refs = loop {
            let refs = pool
                .references(&root, &file, at)
                .unwrap_or_else(|e| panic!("{}: references: {e}", case.path));
            if refs.iter().any(|r| r.range.start.line as usize + 1 == line)
                || std::time::Instant::now() >= deadline
            {
                break refs;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        };
        // A sourcekit-lsp too old to index in the background (before Swift 6.0) has no index without a build, so it
        // finds no references at all; its definition was checked above.
        if refs.is_empty() && language.server().id == "sourcekit-lsp" {
            eprintln!(
                "{}: sourcekit-lsp built no index here (a toolchain before Swift 6.0); references skipped",
                case.path
            );
            answered.push(language.name());
            servers.insert(language.server().id);
            continue;
        }
        assert!(
            refs.iter().any(|r| r.range.start.line as usize + 1 == line),
            "{}: the call on line {line} is a reference (got {refs:?})",
            case.path
        );
        answered.push(language.name());
        servers.insert(language.server().id);
    }
    // Servers start on the first query for their language and stay up: one per installed server.
    assert_eq!(pool.starts() as usize, servers.len());
    assert_eq!(pool.running().len(), servers.len());
    eprintln!("answered by a language server here: {answered:?}");
}
