//! The client, the pool and the tools' answers against a scripted server: no real server is needed, so these run
//! on every OS.

use std::fs;
use std::io::{PipeReader, PipeWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apz_lsp::fake::{self, Script, Seen};
use apz_lsp::servers::RUST_ANALYZER;
use apz_lsp::types::{path_to_uri, position_at};
use apz_lsp::{
    Diagnostic, DocumentSymbol, Language, Location, LspError, Pool, Position, Range, Timeouts,
    Transport,
};
use serde_json::json;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/repo")
        .join(rel)
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

fn range(start_line: u32, end_line: u32) -> Range {
    Range {
        start: at(start_line, 0),
        end: at(end_line, 1),
    }
}

fn location(file: &Path, start_line: u32) -> Location {
    Location {
        uri: path_to_uri(file),
        range: range(start_line, start_line),
    }
}

/// A pool whose every server is the scripted one; the recorded transports are kept for the assertions.
fn scripted(script: Script, timeouts: Timeouts) -> (Pool, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let pool = Pool::with_launch(timeouts, move |_spec, _root| {
        let (transport, seen) = fake::transport(script.clone());
        record.lock().unwrap().push(seen);
        Ok(transport)
    });
    (pool, seen)
}

/// The fixture copied into a scratch directory, so a test may edit its files.
fn scratch_copy(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-lsp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_tree(&fixture(""), &dir);
    dir
}

/// Copies the files and directories of the fixture. Dot entries are left out: SwiftPM leaves `.build` (a link) there.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn a_server_starts_on_the_first_query_and_stays_up() {
    let root = fixture("");
    let script = Script {
        definition: vec![location(&fixture("typescript/greet.ts"), 0)],
        references: vec![location(&fixture("typescript/greet.ts"), 0)],
        ..Script::default()
    };
    let (pool, seen) = scripted(script, Timeouts::default());
    assert_eq!(pool.starts(), 0, "nothing starts before a query");
    let ts = fixture("typescript/greet.ts");
    pool.definition(&root, &ts, at(5, 9)).unwrap();
    assert_eq!(pool.starts(), 1);
    pool.references(&root, &ts, at(5, 9)).unwrap();
    assert_eq!(
        pool.starts(),
        1,
        "the second query reuses the running server"
    );
    // TypeScript and JavaScript share one server.
    pool.definition(&root, &fixture("javascript/greet.js"), at(5, 9))
        .unwrap();
    assert_eq!(pool.starts(), 1);
    // Another language's server starts once, and so does one under another workspace root.
    pool.definition(&root, &fixture("python/greet.py"), at(5, 11))
        .unwrap();
    assert_eq!(pool.starts(), 2);
    pool.definition(&root, &fixture("python/greet.py"), at(5, 11))
        .unwrap();
    assert_eq!(pool.starts(), 2);
    pool.definition(&root.join("python"), &fixture("python/greet.py"), at(5, 11))
        .unwrap();
    assert_eq!(pool.starts(), 3);
    assert_eq!(pool.running().len(), 3);
    // Each document is opened once, then only queried: the shared TypeScript server holds greet.ts and greet.js.
    let first = seen.lock().unwrap()[0].clone();
    assert_eq!(first.opened.lock().unwrap().len(), 2);
    assert_eq!(first.methods.lock().unwrap()[0], "initialize");
}

#[test]
fn definitions_references_and_symbols_are_the_servers_answers() {
    let file = fixture("src/lib.rs");
    let script = Script {
        definition: vec![location(&file, 0)],
        references: vec![location(&file, 0), location(&file, 5)],
        symbols: vec![DocumentSymbol {
            name: "Plaza".into(),
            kind: "class",
            detail: None,
            range: range(0, 9),
            children: vec![DocumentSymbol {
                name: "open".into(),
                kind: "method",
                detail: Some("fn()".into()),
                range: range(1, 2),
                children: vec![],
            }],
        }],
        ..Script::default()
    };
    let (pool, _) = scripted(script, Timeouts::default());
    let root = fixture("");
    let defs = pool.definition(&root, &file, at(5, 4)).unwrap();
    assert_eq!(defs, vec![location(&file, 0)]);
    assert_eq!(defs[0].path().as_deref(), Some(file.as_path()));
    let refs = pool.references(&root, &file, at(5, 4)).unwrap();
    assert_eq!(
        refs.iter().map(|l| l.range.start.line).collect::<Vec<_>>(),
        vec![0, 5]
    );
    let symbols = pool.document_symbols(&root, &file).unwrap();
    assert_eq!(symbols[0].name, "Plaza");
    assert_eq!(symbols[0].children[0].kind, "method");
    assert_eq!(symbols[0].children[0].detail.as_deref(), Some("fn()"));
}

#[test]
fn rust_analyzer_is_started_with_its_check_on_save_off() {
    let file = fixture("src/lib.rs");
    let answer = Script {
        definition: vec![location(&file, 0)],
        ..Script::default()
    };
    let (pool, seen) = scripted(answer, Timeouts::default());
    assert_eq!(RUST_ANALYZER.id, "rust-analyzer");
    pool.definition(&fixture(""), &file, at(5, 4)).unwrap();
    let options = seen.lock().unwrap()[0].init_options.lock().unwrap().clone();
    assert_eq!(options, Some(json!({"checkOnSave": false})));
}

#[test]
fn diagnostics_are_collected_and_a_change_asks_again() {
    let dir = scratch_copy("diagnostics");
    let file = dir.join("src/lib.rs");
    let warning = Diagnostic {
        range: range(0, 0),
        severity: Some(2),
        source: Some("fake".into()),
        message: "unused".into(),
    };
    let script = Script {
        diagnostics: vec![warning.clone()],
        ..Script::default()
    };
    let (pool, seen) = scripted(script, Timeouts::default());
    let checked = pool.diagnostics(&dir, &file).unwrap();
    assert!(checked.settled);
    assert_eq!(checked.items, vec![warning.clone()]);
    // Unchanged: the answer is the one the server gave, with no new request.
    let again = pool.diagnostics(&dir, &file).unwrap();
    assert_eq!(again.items, vec![warning]);
    assert_eq!(seen.lock().unwrap()[0].changed.lock().unwrap().len(), 0);
    // An edit is sent whole, and the server answers it.
    fs::write(&file, "pub fn greet() {}\n").unwrap();
    pool.diagnostics(&dir, &file).unwrap();
    assert_eq!(seen.lock().unwrap()[0].changed.lock().unwrap().len(), 1);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_refused_request_is_an_error_and_a_silent_server_times_out() {
    let file = fixture("src/lib.rs");
    let refusing = Script {
        refuse: Some("no index yet".into()),
        ..Script::default()
    };
    let (pool, _) = scripted(refusing, Timeouts::default());
    match pool.definition(&fixture(""), &file, at(5, 4)) {
        Err(LspError::Rpc {
            method,
            code,
            message,
        }) => {
            assert_eq!(method, "textDocument/definition");
            assert_eq!(code, -32603);
            assert_eq!(message, "no index yet");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }

    // A server that reads but never answers: the handshake gives up after its timeout.
    let kept: Arc<Mutex<Vec<(PipeReader, PipeWriter)>>> = Arc::new(Mutex::new(Vec::new()));
    let keep = kept.clone();
    let quick = Timeouts {
        start: Duration::from_millis(200),
        request: Duration::from_millis(200),
        diagnostics: Duration::from_millis(100),
        shutdown: Duration::from_millis(100),
        warm_up: Duration::from_secs(5),
        warm_retry: Duration::from_millis(10),
    };
    let silent = Pool::with_launch(quick, move |_spec, _root| {
        let (server_in, to_server) =
            std::io::pipe().map_err(|e| LspError::Protocol(e.to_string()))?;
        let (from_server, server_out) =
            std::io::pipe().map_err(|e| LspError::Protocol(e.to_string()))?;
        keep.lock().unwrap().push((server_in, server_out));
        Ok(Transport {
            reader: Box::new(from_server),
            writer: Box::new(to_server),
            child: None,
            stderr: None,
        })
    });
    let err = silent
        .definition(&fixture(""), &file, at(5, 4))
        .unwrap_err();
    assert_eq!(
        err,
        LspError::Timeout {
            method: "initialize".into()
        }
    );
    assert_eq!(
        silent.starts(),
        0,
        "a server that did not start is not kept"
    );
    drop(kept);
}

#[test]
fn a_language_without_an_installed_server_says_so() {
    let root = fixture("");
    let pool = Pool::new(Timeouts::default());
    let mut checked = 0;
    for language in Language::ALL {
        if apz_lsp::is_installed(language) {
            continue;
        }
        let rel = match language {
            Language::Rust => "src/lib.rs",
            Language::Go => "go/greet.go",
            Language::Python => "python/greet.py",
            Language::TypeScript => "typescript/greet.ts",
            Language::JavaScript => "javascript/greet.js",
            Language::Swift => "swift/Sources/Greet/Greet.swift",
            Language::Kotlin => "kotlin/Greet.kt",
            Language::Java => "java/Greet.java",
            Language::CSharp => "csharp/Greet.cs",
            Language::Html => "html/index.html",
            Language::Css => "css/greet.css",
            Language::Bash => "bash/greet.sh",
            Language::PowerShell => "powershell/Greet.ps1",
            Language::Yaml => "yaml/greet.yaml",
            Language::Toml => "toml/greet.toml",
            Language::Json => "json/greet.json",
        };
        let err = pool.definition(&root, &fixture(rel), at(0, 0)).unwrap_err();
        assert!(
            matches!(&err, LspError::NoServer { language: l, .. } if l == language.name()),
            "{rel}: {err:?}"
        );
        assert!(
            err.to_string()
                .starts_with(&format!("no server for {} here", language.name())),
            "{err}"
        );
        checked += 1;
    }
    assert_eq!(pool.starts(), 0);
    eprintln!("checked {checked} languages whose servers are not installed here");
}

#[test]
fn paths_must_be_absolute_and_have_a_language() {
    let pool = Pool::new(Timeouts::default());
    let err = pool
        .definition(&fixture(""), Path::new("src/lib.rs"), at(0, 0))
        .unwrap_err();
    assert!(matches!(err, LspError::File(_)), "{err:?}");
    let err = pool
        .definition(&fixture(""), &fixture("Cargo.toml.bak"), at(0, 0))
        .unwrap_err();
    assert!(matches!(err, LspError::File(_)), "{err:?}");
    assert_eq!(position_at("abc\n", 1, 2), Some(at(0, 1)));
}

#[test]
fn an_empty_answer_from_a_warming_server_is_asked_again_and_a_later_empty_one_is_final() {
    let file = fixture("src/lib.rs");
    let quick = Timeouts {
        warm_up: Duration::from_secs(5),
        warm_retry: Duration::from_millis(10),
        ..Timeouts::default()
    };
    let warming = Script {
        definition: vec![location(&file, 0)],
        empty_answers: 3,
        ..Script::default()
    };
    let (pool, _) = scripted(warming, quick);
    let defs = pool.definition(&fixture(""), &file, at(5, 4)).unwrap();
    assert_eq!(
        defs,
        vec![location(&file, 0)],
        "the third ask, after three empty ones, has the answer"
    );

    // Once the warm-up window has passed, an empty answer stands (a position that names no symbol).
    let settled = Timeouts {
        warm_up: Duration::ZERO,
        ..quick
    };
    let empty = Script {
        empty_answers: 1,
        ..Script::default()
    };
    let (pool, _) = scripted(empty, settled);
    assert_eq!(
        pool.references(&fixture(""), &file, at(0, 0)).unwrap(),
        vec![]
    );
}
