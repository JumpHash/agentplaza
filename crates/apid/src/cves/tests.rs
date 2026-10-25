//! The CVE poll against fixtures (PLAN 24.3): replayed advisories (OSV's documented schema, GitHub's advisories, RustSec's
//! Markdown, a govulncheck stream, an audit's hits) matched against a project's lock and a release's SBOM. The network is a
//! fake server behind the glue's `Outbound`, the call graph and the fixer are fakes of their ports: no real network, no
//! real Weft, no real upgrade run.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;
use crate::glue::{Outgoing, Reply};

/// A fake server: the first route whose URL is the request's URL answers its status and body; any other URL is a 404.
struct Server {
    routes: Vec<(&'static str, u16, String)>,
    sent: Mutex<Vec<String>>,
}

impl Server {
    fn new(routes: Vec<(&'static str, u16, String)>) -> Server {
        Server {
            routes,
            sent: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self, prefix: &str) -> usize {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|u| u.starts_with(prefix))
            .count()
    }
}

impl Outbound for Server {
    fn send(&self, request: &Outgoing) -> Result<Reply, String> {
        self.sent.lock().unwrap().push(request.url.clone());
        let (status, body) = self
            .routes
            .iter()
            .find(|(url, _, _)| *url == request.url)
            .map(|(_, status, body)| (*status, body.clone()))
            .unwrap_or((404, String::new()));
        Ok(Reply {
            status,
            body: body.clone(),
            bytes: body.into_bytes(),
        })
    }
}

/// The call graph of main: how many call references each name has (names not listed have none).
struct Calls(BTreeMap<String, usize>);

impl super::Calls for Calls {
    fn calls(&self, name: &str) -> Option<usize> {
        Some(self.0.get(name).copied().unwrap_or(0))
    }
}

/// A call graph that is not there.
struct NoCalls;

impl super::Calls for NoCalls {
    fn calls(&self, _name: &str) -> Option<usize> {
        None
    }
}

/// The fixer: records the advisories it was asked to fix, and refuses when told to.
#[derive(Default)]
struct Fixer {
    asked: Mutex<Vec<crate::upgrades::Advisory>>,
    refuse: bool,
}

impl super::Fixer for Fixer {
    fn start(&self, advisory: &crate::upgrades::Advisory) -> Result<(), String> {
        self.asked.lock().unwrap().push(advisory.clone());
        if self.refuse {
            Err("Weft is not running: the fix cannot be opened".into())
        } else {
            Ok(())
        }
    }
}

const CARGO_LOCK_MAIN: &str = r#"
version = 3

[[package]]
name = "h2"
version = "0.3.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bb"
"#;

fn tree(files: &[(&str, &str)]) -> Tree {
    files
        .iter()
        .map(|(p, text)| (p.to_string(), text.as_bytes().to_vec()))
        .collect()
}

/// A CycloneDX SBOM of one release: its components as the release's artifacts carry them.
fn sbom(components: &[(&str, &str, &str)]) -> String {
    let comps: Vec<serde_json::Value> = components
        .iter()
        .map(|(name, version, purl)| serde_json::json!({"type": "library", "name": name, "version": version, "purl": purl}))
        .collect();
    serde_json::json!({"bomFormat": "CycloneDX", "specVersion": "1.5", "components": comps})
        .to_string()
}

/// The feeds the fake server serves for h2: OSV's query, GitHub's advisories, and RustSec's directory and file.
fn h2_feeds() -> Server {
    let osv = include_str!("fixtures/osv_h2.json");
    let github = include_str!("fixtures/github_advisories.json");
    let rustsec_md = include_str!("fixtures/rustsec_h2.md");
    Server::new(vec![
        ("https://api.osv.dev/v1/query", 200, format!(r#"{{"vulns":[{osv}]}}"#)),
        (
            "https://api.github.com/advisories?ecosystem=rust&affects=h2&per_page=100",
            200,
            github.to_string(),
        ),
        (
            "https://api.github.com/repos/RustSec/advisory-db/contents/crates/h2",
            200,
            r#"[{"name":"RUSTSEC-2024-0003.md","download_url":"https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/h2/RUSTSEC-2024-0003.md"}]"#
                .to_string(),
        ),
        (
            "https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/h2/RUSTSEC-2024-0003.md",
            200,
            rustsec_md.to_string(),
        ),
    ])
}

/// A release 0.9.0 that ships h2 0.2.0, deployed to production.
fn old_release() -> Vec<ReleaseIn> {
    vec![ReleaseIn {
        id: "rel-1".into(),
        version: "0.9.0".into(),
        sboms: vec![sbom(&[("h2", "0.2.0", "pkg:cargo/h2@0.2.0")])],
    }]
}

fn envs() -> Vec<EnvIn> {
    vec![
        EnvIn {
            name: "production".into(),
            track: "releases".into(),
            version: "0.9.0".into(),
        },
        EnvIn {
            name: "staging".into(),
            track: "main".into(),
            version: String::new(),
        },
    ]
}

fn run_poll(
    main: &Tree,
    releases: &[ReleaseIn],
    outbound: &Server,
    calls: &dyn super::Calls,
    fixer: &Fixer,
    previous: &[Finding],
) -> Report {
    poll(&Inputs {
        main,
        releases,
        environments: &envs(),
        outbound,
        calls,
        fixer,
        audits: &[],
        govulncheck: None,
        previous,
        now: 1_000,
    })
}

#[test]
fn a_known_advisory_against_an_old_release_is_in_the_report_with_its_fix_task() {
    let main = tree(&[("Cargo.lock", CARGO_LOCK_MAIN)]);
    let server = h2_feeds();
    let fixer = Fixer::default();
    let calls = Calls(BTreeMap::from([("read_frame".to_string(), 2)]));
    let report = run_poll(&main, &old_release(), &server, &calls, &fixer, &[]);

    // One OSV query per package the project has (h2 and serde), and the GitHub and RustSec reads for each.
    assert_eq!(server.requests("https://api.osv.dev/"), 2);
    assert_eq!(server.requests("https://api.github.com/advisories"), 2);
    // The three feeds agree on one advisory: OSV's primary id, with the CVE and GHSA as aliases.
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let f = &report.findings[0];
    assert_eq!(f.id, "cargo:h2:RUSTSEC-2024-0003");
    assert!(f.ids.contains(&"CVE-2024-27316".to_string()));
    assert!(f.ids.contains(&"GHSA-qx2v-8332-m7x4".to_string()));
    assert_eq!(f.source, "osv, github, rustsec");
    assert_eq!(f.severity, "high");
    // main is affected at 0.3.9 and the fix is 0.3.26; the old release is affected too and runs in production.
    assert_eq!(f.main_version, "0.3.9");
    assert_eq!(f.fixed, "0.3.26");
    assert_eq!(f.main_environments, vec!["staging".to_string()]);
    assert_eq!(f.releases.len(), 1);
    assert_eq!(f.releases[0].release, "rel-1");
    assert_eq!(f.releases[0].version, "0.2.0");
    assert_eq!(f.releases[0].environments, vec!["production".to_string()]);
    // The Rust function is called from main's code: reachable, at the advisory's own priority.
    assert_eq!(f.reachability, "reachable");
    assert_eq!(f.priority, "high");
    // The fix is an upgrade task asked for once, with the advisory's priority and fixed release.
    assert_eq!(f.fix_state, "started");
    let asked = fixer.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].package, "h2");
    assert_eq!(asked[0].fixed, "0.3.26");
    assert_eq!(asked[0].priority, "high");
    assert_eq!(report.new, vec![f.id.clone()]);
    // The feeds' records: each answered, with a count.
    assert!(report.feeds.iter().all(|feed| feed.error.is_empty()));
    assert_eq!(
        report
            .feeds
            .iter()
            .find(|f| f.feed == "rustsec")
            .unwrap()
            .count,
        1
    );

    // The next poll keeps the fix as it is: the fixer is not asked again.
    let again = run_poll(
        &main,
        &old_release(),
        &server,
        &calls,
        &fixer,
        &report.findings,
    );
    assert_eq!(again.findings[0].fix_state, "started");
    assert_eq!(fixer.asked.lock().unwrap().len(), 1);
    assert!(again.new.is_empty());
}

#[test]
fn a_range_that_excludes_the_version_does_not_match() {
    // h2 0.4.5 is past the affected 0.4.0..0.4.3 range and the 0.3.x one: no finding for it, in main or a release.
    let main = tree(&[("Cargo.lock", &CARGO_LOCK_MAIN.replace("0.3.9", "0.4.5"))]);
    let releases = vec![ReleaseIn {
        id: "rel-2".into(),
        version: "1.0.0".into(),
        sboms: vec![sbom(&[("h2", "0.4.5", "pkg:cargo/h2@0.4.5")])],
    }];
    let fixer = Fixer::default();
    let report = run_poll(&main, &releases, &h2_feeds(), &NoCalls, &fixer, &[]);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(fixer.asked.lock().unwrap().is_empty());
}

#[test]
fn a_rust_advisory_the_call_graph_never_reaches_is_unreachable_and_one_level_lower() {
    let main = tree(&[("Cargo.lock", CARGO_LOCK_MAIN)]);
    let fixer = Fixer::default();
    let silent = Calls(BTreeMap::new());
    let report = run_poll(&main, &[], &h2_feeds(), &silent, &fixer, &[]);
    let f = &report.findings[0];
    assert_eq!(f.reachability, "unreachable");
    assert_eq!(f.severity, "high");
    assert_eq!(
        f.priority, "medium",
        "the code does not reach it: one band lower"
    );
    // An unreachable finding is still fixed: the report says what the fix is.
    assert_eq!(f.fix_state, "started");

    // Without a call graph the reachability is unknown, and the priority is the severity.
    let unknown = run_poll(&main, &[], &h2_feeds(), &NoCalls, &Fixer::default(), &[]);
    assert_eq!(unknown.findings[0].reachability, "unknown");
    assert_eq!(unknown.findings[0].priority, "high");
}

#[test]
fn the_report_exports_as_csv_with_quoted_cells_and_as_json() {
    let main = tree(&[("Cargo.lock", CARGO_LOCK_MAIN)]);
    let report = run_poll(
        &main,
        &old_release(),
        &h2_feeds(),
        &Calls(BTreeMap::from([("read_frame".to_string(), 1)])),
        &Fixer::default(),
        &[],
    );
    let csv = export::csv(&report.findings);
    let mut lines = csv.lines();
    assert_eq!(lines.next().unwrap(), export::COLUMNS.join(","));
    let row = lines.next().expect("one finding, one row");
    assert!(
        row.starts_with("cargo:h2:RUSTSEC-2024-0003,RUSTSEC-2024-0003;CVE-2024-27316;"),
        "{row}"
    );
    assert!(row.contains(",rel-1@0.2.0,production@0.2.0,"), "{row}");
    // A title with a comma and a quote is one quoted cell.
    let mut odd = report.findings[0].clone();
    odd.title = "a, \"quoted\" title".into();
    let quoted = export::csv(&[odd]);
    assert!(quoted.contains("\"a, \"\"quoted\"\" title\""), "{quoted}");
    let json: serde_json::Value = serde_json::from_str(&export::json(&report.findings)).unwrap();
    assert_eq!(json["findings"][0]["id"], "cargo:h2:RUSTSEC-2024-0003");
}

#[test]
fn a_go_module_is_reachable_by_govulncheck_and_its_fix_is_not_built() {
    let osv_go = r#"{"id":"GO-2024-2611","aliases":["CVE-2023-45288"],"summary":"HTTP/2 CONTINUATION flood in net/http",
        "database_specific":{"severity":"HIGH"},
        "affected":[{"package":{"ecosystem":"Go","name":"golang.org/x/net"},
            "ranges":[{"type":"SEMVER","events":[{"introduced":"0"},{"fixed":"0.23.0"}]}]}]}"#;
    let server = Server::new(vec![(
        "https://api.osv.dev/v1/query",
        200,
        format!(r#"{{"vulns":[{osv_go}]}}"#),
    )]);
    let main = tree(&[
        ("go.mod", "module example.com/api\n"),
        (
            "go.sum",
            "golang.org/x/net v0.20.0 h1:abc=\ngolang.org/x/net v0.20.0/go.mod h1:def=\n",
        ),
    ]);
    let govulncheck = include_str!("fixtures/govulncheck.json");
    let report = poll(&Inputs {
        main: &main,
        releases: &[],
        environments: &[],
        outbound: &server,
        calls: &NoCalls,
        fixer: &Fixer::default(),
        audits: &[],
        govulncheck: Some(govulncheck),
        previous: &[],
        now: 5,
    });
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let f = &report.findings[0];
    assert_eq!(f.id, "go:golang.org/x/net:GO-2024-2611");
    assert_eq!(f.reachability, "reachable");
    assert_eq!(f.main_version, "v0.20.0");
    assert_eq!(f.fixed, "0.23.0");
    assert_eq!(f.fix_state, "not_built", "only Cargo has fixes built");
    assert!(f.fix_reason.contains("only Cargo"));
}

#[test]
fn an_audit_observation_is_matched_to_main_only_and_a_failed_feed_keeps_the_record() {
    let main = tree(&[("Cargo.lock", CARGO_LOCK_MAIN)]);
    // Every feed answers 500: the poll says so, and the findings from the audit remain.
    let failing = Server::new(vec![
        ("https://api.osv.dev/v1/query", 500, String::new()),
        (
            "https://api.github.com/advisories?ecosystem=rust&affects=h2&per_page=100",
            500,
            String::new(),
        ),
        (
            "https://api.github.com/repos/RustSec/advisory-db/contents/crates/h2",
            500,
            String::new(),
        ),
        (
            "https://api.github.com/repos/RustSec/advisory-db/contents/crates/serde",
            500,
            String::new(),
        ),
    ]);
    let hits = vec![apz_topology::security::Hit {
        id: "RUSTSEC-2024-0003".into(),
        severity: "high".into(),
        subject: "h2".into(),
        detail: "a flaw".into(),
    }];
    let audits = vec![("cargo-audit".to_string(), hits)];
    let fixer = Fixer::default();
    let report = poll(&Inputs {
        main: &main,
        releases: &old_release(),
        environments: &envs(),
        outbound: &failing,
        calls: &NoCalls,
        fixer: &fixer,
        audits: &audits,
        govulncheck: None,
        previous: &[],
        now: 9,
    });
    assert!(
        report
            .feeds
            .iter()
            .filter(|f| f.feed != "main")
            .all(|f| !f.error.is_empty())
    );
    assert!(report.notes.iter().any(|n| n.starts_with("osv: ")));
    // The audit saw h2 in main's lock with no version named: it is main's finding alone, not the release's.
    let f = report
        .findings
        .iter()
        .find(|f| f.id == "cargo:h2:RUSTSEC-2024-0003")
        .unwrap();
    assert_eq!(f.main_version, "0.3.9");
    assert!(f.releases.is_empty());
    assert_eq!(f.source, "audit:cargo-audit");
    // No fix is named by an audit's observation: the report says so rather than inventing one.
    assert_eq!(f.fix_state, "no_fix");
    assert!(fixer.asked.lock().unwrap().is_empty());
    // A failed feed keeps what was there before: a previous finding is not dropped by a poll that could not read.
    let previous = vec![Finding {
        id: "cargo:h2:CVE-2000-0001".into(),
        ..Finding::default()
    }];
    let kept = poll(&Inputs {
        main: &main,
        releases: &[],
        environments: &[],
        outbound: &failing,
        calls: &NoCalls,
        fixer: &Fixer::default(),
        audits: &[],
        govulncheck: None,
        previous: &previous,
        now: 10,
    });
    assert!(
        kept.findings
            .iter()
            .any(|f| f.id == "cargo:h2:CVE-2000-0001")
    );
}

#[test]
fn a_pip_advisory_orders_its_versions_by_pep_440() {
    let osv_pip = r#"{"id":"PYSEC-2023-74","summary":"requests leaks credentials",
        "affected":[{"package":{"ecosystem":"PyPI","name":"requests"},
            "ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"2.31.0rc1"}]}]}]}"#;
    let server = Server::new(vec![(
        "https://api.osv.dev/v1/query",
        200,
        format!(r#"{{"vulns":[{osv_pip}]}}"#),
    )]);
    let main = tree(&[("requirements.txt", "requests==2.31.0.dev1\nflask==2.0\n")]);
    let report = poll(&Inputs {
        main: &main,
        releases: &[],
        environments: &[],
        outbound: &server,
        calls: &NoCalls,
        fixer: &Fixer::default(),
        audits: &[],
        govulncheck: None,
        previous: &[],
        now: 3,
    });
    // 2.31.0.dev1 is before 2.31.0rc1 (a development release sorts before a pre-release): affected, fixed in 2.31.0rc1.
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].main_version, "2.31.0.dev1");
    assert_eq!(report.findings[0].fixed, "2.31.0rc1");
    assert_eq!(report.findings[0].fix_state, "not_built");
}

#[test]
fn a_fix_settled_by_the_upgrade_flow_names_its_task_or_why_it_stopped() {
    let batch = crate::upgrades::UpgradeBatch {
        state: "task".into(),
        task: "t7".into(),
        reason: "RUSTSEC-2024-0003 (high, h2)".into(),
        ..Default::default()
    };
    let mut f = Finding::default();
    let report = crate::upgrades::Report {
        batches: vec![batch],
        ..Default::default()
    };
    record_fix(&mut f, &report);
    assert_eq!((f.fix_state.as_str(), f.fix_task.as_str()), ("task", "t7"));
    assert!(f.fix_reason.contains("high"));

    // A batch whose tests failed says so, with no task.
    let failed = crate::upgrades::Report {
        batches: vec![crate::upgrades::UpgradeBatch {
            state: "tests_failed".into(),
            reason: "the suite fails".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    record_fix(&mut f, &failed);
    assert_eq!(f.fix_state, "tests_failed");
    assert!(f.fix_task.is_empty());

    // An ecosystem with no fix flow is not built, and says why.
    let skipped = crate::upgrades::Report {
        skipped: Some("npm fixes of an advisory are not built: only Cargo has them".into()),
        ..Default::default()
    };
    record_fix(&mut f, &skipped);
    assert_eq!(f.fix_state, "not_built");
}
