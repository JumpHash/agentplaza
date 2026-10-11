//! The specs in drafts/jhrl/specs/ and the examples in docs/jhrl.md, end to end.

use jhrl::ast::Item;
use jhrl::check::check_program;
use jhrl::mc::{self, Found, Options};
use jhrl::parser::parse_module;
use jhrl::scenario::{self, Outcome};
use jhrl::Program;
use std::path::{Path, PathBuf};

fn spec_files() -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let mut es: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()).collect();
        es.sort();
        for p in es {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "jhrl") {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("specs"), &mut v);
    v
}

fn load_specs() -> Program {
    let files = spec_files().into_iter().map(|p| (p.display().to_string(), std::fs::read_to_string(&p).unwrap())).collect();
    let (p, d) = Program::load(files);
    assert!(d.is_empty(), "{}", d.iter().map(|x| x.render(&p.sources)).collect::<Vec<_>>().join("\n"));
    p
}

#[test]
fn every_spec_file_parses() {
    let files = spec_files();
    assert!(files.len() >= 7, "{files:?}");
    for (i, f) in files.iter().enumerate() {
        let text = std::fs::read_to_string(f).unwrap();
        if let Err(d) = parse_module(i as u32, &text) {
            let mut sm = jhrl::span::SourceMap::default();
            for (j, g) in files.iter().enumerate() {
                sm.add(g.display().to_string(), if j == i { text.clone() } else { String::new() });
            }
            panic!("{}", d.render(&sm));
        }
    }
}

#[test]
fn specs_pass_name_and_dimension_checks() {
    let p = load_specs();
    let d = check_program(&p);
    let errs: Vec<String> = d.iter().filter(|x| !x.warning).map(|x| x.render(&p.sources)).collect();
    assert!(errs.is_empty(), "{}", errs.join("\n"));
}

fn model<'a>(p: &'a Program, machine: &str) -> (usize, &'a jhrl::ast::ModelDecl) {
    p.items()
        .find_map(|(pk, _, it)| match it {
            Item::Model(m) if m.machine == machine => Some((pk, m)),
            _ => None,
        })
        .unwrap()
}

/// Parity with TLC on drafts/formal/Approval.tla + Approval.cfg (tla2tools 1.8.0):
/// "23 states generated, 11 distinct states found … depth of the complete state graph search is 3".
#[test]
fn approval_matches_tlc() {
    let p = load_specs();
    let (pk, m) = model(&p, "Approval");
    let r = mc::explore(&p, pk, m, &Options::default()).unwrap();
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    assert_eq!(r.states.len(), 11);
    assert_eq!(r.depth, 3);
    assert_eq!(r.invariants.len(), 4);
    assert_eq!(r.transitions.len(), 1);
    // TLC also counts the 12 stuttering steps of its `Decide` (an out-of-turn answer is
    // UNCHANGED); JHRL's guard `when a == chain[pos]` disables those instead.
    assert_eq!(r.generated, 11);
}

#[test]
fn credit_line_holds() {
    let p = load_specs();
    let (pk, m) = model(&p, "CreditLine");
    let r = mc::explore(&p, pk, m, &Options::default()).unwrap();
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    assert_eq!(r.states.len(), 16);
}

#[test]
#[ignore = "model-checks every machine (minutes); run with --release -- --ignored"]
fn every_mutant_is_caught_the_way_it_says() {
    let p = load_specs();
    let mut n = 0;
    for (pk, _, it) in p.items() {
        if let Item::Mutant(m) = it {
            let o = mc::run_mutant(&p, pk, m);
            assert!(o.caught, "mutant \"{}\": {}", m.name, o.line);
            n += 1;
        }
    }
    assert!(n > 0, "no mutants found");
}

#[test]
fn approval_mutants_are_caught_by_decided_by_the_chain() {
    let p = load_specs();
    for (pk, _, it) in p.items() {
        if let Item::Mutant(m) = it {
            if m.machine == "Approval" {
                let o = mc::run_mutant(&p, pk, m);
                assert!(o.line.contains("invariant \"Decided by the chain\" violated at depth 2"), "{}", o.line);
            }
        }
    }
}

#[test]
#[ignore = "model-checks every machine (minutes); run with --release -- --ignored"]
fn every_witness_is_reachable() {
    let p = load_specs();
    let mut n = 0;
    for (pk, _, it) in p.items() {
        if let Item::Witness(w) = it {
            let mref = p.lookup_decl(pk, &w.machine).unwrap();
            let (mp, _, md) = mc::models_of(&p, mref).into_iter().find(|(_, _, md)| Some(&md.name) == w.model.as_ref()).unwrap();
            let r = mc::explore(&p, mp, md, &Options::default()).unwrap();
            assert!(r.find(&p, pk, &w.expr).unwrap().is_some(), "witness \"{}\" unreachable", w.name);
            n += 1;
        }
    }
    assert!(n > 0, "no witnesses found");
}

#[test]
fn a_mutant_that_is_not_caught_fails() {
    // The same Approval spec with a harmless mutant: replacing the guard by itself.
    let mut p = load_specs();
    let f = p.sources.files.iter().position(|s| s.path.ends_with("approval.jhrl")).unwrap();
    let text = p.sources.files[f].text.clone()
        + "\nmutant \"No-op\" of Approval {\n  replace \"a == chain[pos]\" in Decide with \"chain[pos] == a\"\n  expect violation \"Decided by the chain\"\n}\n";
    p = p.with_source(f as u32, text).unwrap();
    let (pk, m) = p
        .items()
        .find_map(|(pk, _, it)| match it {
            Item::Mutant(m) if m.name == "No-op" => Some((pk, m.clone())),
            _ => None,
        })
        .unwrap();
    let o = mc::run_mutant(&p, pk, &m);
    assert!(!o.caught && o.line.contains("NOT caught"), "{}", o.line);
}

#[test]
fn an_unreachable_witness_fails() {
    let p = load_specs();
    let (pk, m) = model(&p, "Approval");
    let r = mc::explore(&p, pk, m, &Options::default()).unwrap();
    let e = jhrl::parser::parse_expr_at(0, "status == \"approved\" and decided_by == \"builder\"", 0).unwrap();
    assert_eq!(r.find(&p, pk, &e).unwrap(), None);
}

#[test]
fn every_scenario_passes() {
    let p = load_specs();
    let mut n = 0;
    for (pk, _, it) in p.items() {
        if let Item::Scenario(s) = it {
            match scenario::run(&p, pk, s) {
                Outcome::Pass(_) => n += 1,
                other => panic!("scenario {} \"{}\": {other:?}", s.req, s.name),
            }
        }
    }
    assert_eq!(n, 6);
}

#[test]
fn a_counterexample_is_a_shortest_trace() {
    // "Accept any authority" with normal (stop-at-first) checking: one Decide from an initial
    // state already breaks "Decided by the chain", so the trace is init + one step.
    let p = load_specs();
    let (pk, m) = p
        .items()
        .find_map(|(pk, _, it)| match it {
            Item::Mutant(m) if m.name == "Accept any authority" => Some((pk, m.clone())),
            _ => None,
        })
        .unwrap();
    let mp = mc::mutate(&p, pk, &m).unwrap();
    let (mpk, md) = model(&mp, "Approval");
    let r = mc::explore(&mp, mpk, md, &Options::default()).unwrap();
    let v = &r.violations[0];
    assert_eq!(v.what, Found::Invariant("Decided by the chain".into()));
    assert_eq!(r.path_to(v.state).len(), 2);
    let t = r.render_trace(v, "");
    assert!(t.contains(" 1. init") && t.contains(" 2. Decide(a = "), "{t}");
}

/// Every ```jhrl block in docs/jhrl.md parses (the document is the language reference).
#[test]
fn doc_examples_parse() {
    let doc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/jhrl.md");
    let Ok(text) = std::fs::read_to_string(&doc) else {
        eprintln!("docs/jhrl.md not found; skipped");
        return;
    };
    let mut blocks = Vec::new();
    let mut cur: Option<(usize, String)> = None;
    for (i, line) in text.lines().enumerate() {
        match (&mut cur, line.trim_start()) {
            (None, l) if l.starts_with("```jhrl") => cur = Some((i + 2, String::new())),
            (Some(_), l) if l.starts_with("```") => blocks.push(cur.take().unwrap()),
            (Some((_, b)), _) => {
                b.push_str(line);
                b.push('\n');
            }
            _ => {}
        }
    }
    assert!(blocks.len() >= 10, "{}", blocks.len());
    // Declarations the engine doesn't implement yet (laws and their tests, section 9.13–9.14).
    const NOT_YET: &[&str] = &["type", "operation", "law", "fixture", "authz"];
    for (line, b) in &blocks {
        let first_words: Vec<&str> = b.lines().filter_map(|l| l.split_whitespace().next()).collect();
        if first_words.iter().any(|w| NOT_YET.contains(w)) {
            eprintln!("docs/jhrl.md block at line {line}: uses declarations not in this engine yet; skipped");
            continue;
        }
        if let Err(d) = parse_module(0, b) {
            let mut sm = jhrl::span::SourceMap::default();
            sm.add(format!("docs/jhrl.md block at line {line}"), b.clone());
            panic!("{}", d.render(&sm));
        }
    }
}

#[test]
fn every_authz_case_passes() {
    let results = jhrl::authz::run_authz(&load_specs());
    assert!(!results.is_empty(), "no authz cases found");
    let failed: Vec<String> = results.iter().filter(|r| !r.1).map(|r| format!("{}: {}", r.0, r.2)).collect();
    assert!(failed.is_empty(), "{} of {} cases failed:\n{}", failed.len(), results.len(), failed.join("\n"));
}
