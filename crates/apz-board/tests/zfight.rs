//! The z-fight lint over every board kind and every variant the board fixtures use (src/zfight.rs). A ratchet: the
//! finding count per board may only go down. Run with `--nocapture` to read the findings.

use std::collections::BTreeMap;

use apz_board::zfight::{describe, zfight_report};
use apz_board::Board;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/boards.json");

/// The current finding counts, keyed by the case label (a board's kind, or "kind k=v ..." for a variant). Lower a
/// count when its findings are fixed; never raise one.
const BASELINE: &[(&str, usize)] = &[
    ("apple-soc", 166),
    ("edge", 168),
    ("laptop", 85),
    ("phone", 113),
    ("server", 484),
    ("workstation", 1006),
    ("workstation amd=1 gpus=4", 1020),
    ("workstation gpus=2 studio=1", 1019),
    ("edge cooler=1", 181),
    ("apple-soc e_cores=2 gpu_cores=12 p_cores=6 studio=1", 196),
    ("server gpus=3", 564),
];

fn baseline(label: &str) -> Option<usize> {
    BASELINE.iter().find(|(l, _)| *l == label).map(|(_, n)| *n)
}

#[test]
fn zfight_ratchet() {
    let fx: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut cases: Vec<(String, Board)> = Vec::new();
    for kind in fx["boards"].as_object().expect("boards").keys() {
        cases.push((kind.clone(), Board::from_kind(kind, &BTreeMap::new())));
    }
    for v in fx["variants"].as_array().expect("variants") {
        let kind = v["kind"].as_str().expect("kind");
        let params: BTreeMap<String, f64> =
            v["params"].as_object().expect("params").iter().map(|(k, x)| (k.clone(), x.as_f64().expect("number"))).collect();
        let label = params.iter().fold(kind.to_string(), |acc, (k, v)| format!("{acc} {k}={v}"));
        cases.push((label, Board::from_kind(kind, &params)));
    }

    let mut failures = Vec::new();
    let mut total = 0;
    for (label, board) in &cases {
        let report = zfight_report(board);
        println!("{label}: {} faces, {} findings, {} skipped", report.faces, report.findings.len(), report.skipped.len());
        for f in &report.findings {
            println!("  {}", describe(f));
        }
        total += report.findings.len();
        match baseline(label) {
            Some(b) if report.findings.len() <= b => {}
            Some(b) => failures.push(format!("{label}: {} findings, baseline {b}", report.findings.len())),
            None => failures.push(format!("{label}: {} findings, no baseline", report.findings.len())),
        }
    }
    println!("total findings {total} over {} cases", cases.len());
    assert!(failures.is_empty(), "z-fight ratchet:\n{}", failures.join("\n"));
}
