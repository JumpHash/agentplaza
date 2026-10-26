//! The UI and UX checks of a qualification run (PLAN 22.8, 22.9). A candidate's pages (`.plaza/ux.toml`, apz-ux's spec) are
//! read in the lane's test fork through the web target (`user_test::ux_reads`), and judged here:
//!
//! - the screenshot against the previous qualified stable release's, by page, size, theme and locale: a pixel diff past the
//!   tolerance is a `visual` finding, its diff picture the evidence; with no previous release the run records the baseline
//!   and passes with that note;
//! - the accessibility tree and the layout: each failing rule is an `accessibility` or `layout` finding, its evidence the
//!   candidate's screenshot.
//!
//! Each finding is filed on the run's change, as 10.5's `performance` and 10.6's and 10.7's findings are. The candidate's
//! screenshots are kept as its release's artifacts (`ux/<page>/<device>/<theme>/<locale>`, their bytes in the object store),
//! so the next release is judged against them. Those rows are not builds: verify, sync and the registries leave them out.
//!
//! Routes:
//!   POST /v1/test-lab/runs/{id}/ux   the run's UI checks against its release's baseline: its body names the lane's preview
//!                                    address (`base`) and the UI checks' TOML (`ux`, else the project's `.plaza/ux.toml`)

use std::collections::BTreeMap;

use apz_blobs::{Blobs, Hash};
use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_ux::check::Report;
use apz_ux::spec::{Check, Spec};
use apz_ux::visual;
use apz_ux::{Issue, Kind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::lab_performance;
use crate::releases::entity::{Release, ReleaseArtifact, Store as Releases};
use crate::review_graph::{By, Finding, Reviews, Verdict as FindingVerdict};
use crate::state::App;
use crate::test_lab::LabRun;

/// The repository file a project declares its UI checks in.
pub const FILE: &str = ".plaza/ux.toml";

/// The release-artifact target of a check's screenshot.
pub fn target_of(check: &Check) -> String {
    format!(
        "ux/{}/{}/{}/{}",
        check.page, check.device.name, check.theme, check.locale
    )
}

/// The baseline a candidate is judged against: the previous qualified stable release's screenshots, by target, as their
/// object-store digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub release: String,
    pub shots: BTreeMap<String, String>,
}

/// The newest qualified stable release other than the candidate that kept screenshots (`releases` newest first), or None.
pub fn baseline_of(releases: &[Release], candidate: &Release) -> Option<Baseline> {
    releases
        .iter()
        .filter(|r| r.project == candidate.project && r.id != candidate.id)
        .filter(|r| lab_performance::qualified(r))
        .find_map(|r| {
            let shots: BTreeMap<String, String> = r
                .artifacts
                .iter()
                .filter(|a| a.is_ux())
                .map(|a| (a.target.clone(), a.digest.clone()))
                .collect();
            (!shots.is_empty()).then(|| Baseline {
                release: r.id.clone(),
                shots,
            })
        })
}

/// One judgment of a run's UI checks: what was judged, its kind, whether it passed, what it found, and the finding it
/// became (`<review>/<f<n>>`, empty when it passed or the run names no change).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub id: String,
    pub kind: String,
    pub passed: bool,
    pub detail: String,
    pub finding: String,
}

/// Parses a stored digest back into an object-store hash.
fn hash_of(digest: &str) -> Result<Hash, String> {
    digest
        .parse::<Hash>()
        .map_err(|e| format!("{digest} is not a digest: {e}"))
}

/// The finding a failing issue becomes on the run's change: a confirmed finding of the issue's kind, its proof the issue and
/// its evidence. Empty when the run names no change.
fn file(reviews: &Reviews, run: &LabRun, issue: &Issue, evidence: &str) -> String {
    if run.change.is_empty() {
        return String::new();
    }
    let by = By {
        seat: "ui-checks".into(),
        model: "web-target".into(),
        session: run.id.clone(),
    };
    let finding = Finding {
        symbol: FILE.into(),
        lineage: None,
        text: format!(
            "{} {} on {}: {}",
            issue.kind.as_str(),
            issue.rule,
            issue.element,
            issue.message
        ),
        severity: "major".into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!("lab run {} read the UI checks of its candidate", run.id),
        kind: issue.kind.as_str().into(),
        verdict: FindingVerdict::Confirmed,
        proof: format!(
            "lab run {}\n{}\nevidence: {evidence}",
            run.id, issue.message
        ),
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&run.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

/// Judges the UI checks of `run`, whose candidate is `candidate`, over the reads of its pages (`reads`). `prior` are the
/// project's releases, newest first (the baseline is taken from them). Stores the candidate's screenshots as its artifacts
/// and files each failing judgment as a finding.
pub fn stage(
    blobs: &Blobs,
    reviews: &Reviews,
    releases: &Releases,
    prior: &[Release],
    run: &LabRun,
    candidate: &Release,
    spec: &Spec,
    reads: &[(Check, Report)],
) -> Result<Vec<Verdict>, String> {
    let baseline = baseline_of(prior, candidate);
    let tolerance = spec.tolerance();
    let mut rows = Vec::new();
    let mut out = Vec::new();
    for (check, report) in reads {
        let target = target_of(check);
        let label = check.label();
        let shot = blobs.put(&report.screenshot)?;
        rows.push(ReleaseArtifact {
            target: target.clone(),
            artifact: String::new(),
            digest: shot.hex(),
            size: report.screenshot.len() as u64,
            signatures: String::new(),
            sbom: String::new(),
            provenance: String::new(),
            pool: "ux".into(),
            base: String::new(),
            run: run.id.clone(),
        });
        let mut issues = report.issues.clone();
        let mut detail = String::new();
        let mut diff_evidence = String::new();
        match baseline
            .as_ref()
            .and_then(|b| b.shots.get(&target).map(|d| (b, d)))
        {
            Some((base, digest)) => {
                let reference = visual::decode(&blobs.get(&hash_of(digest)?)?)?;
                let picture = visual::decode(&report.screenshot)?;
                let diff = visual::compare(&reference, &picture, tolerance, &spec.visual.mask);
                if diff.passed {
                    detail = format!(
                        "{} of {} pixels changed ({:.4}) against {}; the tolerance is {:.4}",
                        diff.changed, diff.compared, diff.ratio, base.release, tolerance.ratio
                    );
                } else {
                    let reason = format!("{} against {}", diff.reason, base.release);
                    let image =
                        visual::diff_image(&reference, &picture, tolerance, &spec.visual.mask)?;
                    let picture_digest = blobs.put(&image)?.hex();
                    diff_evidence = format!("diff picture {picture_digest}");
                    issues.push(Issue::new(
                        Kind::Visual,
                        "pixel-diff",
                        label.clone(),
                        reason,
                    ));
                }
            }
            None => {
                detail = match &baseline {
                    None => {
                        "no previous qualified stable release: this run records the baseline".into()
                    }
                    Some(base) => format!(
                        "no picture of {label} in {}: this run records it",
                        base.release
                    ),
                };
            }
        }
        let shot_evidence = format!("screenshot {}", shot.hex());
        if issues.is_empty() {
            out.push(Verdict {
                id: format!("checks:{label}"),
                kind: "visual".into(),
                passed: true,
                detail,
                finding: String::new(),
            });
        }
        for issue in &issues {
            let evidence = if issue.kind == Kind::Visual {
                format!("{shot_evidence}, {diff_evidence}")
            } else {
                shot_evidence.clone()
            };
            let finding = file(reviews, run, issue, &evidence);
            out.push(Verdict {
                id: format!("{}:{}:{}", issue.kind.as_str(), label, issue.rule),
                kind: issue.kind.as_str().into(),
                passed: false,
                detail: format!(
                    "{} {} on {}: {}",
                    issue.kind.as_str(),
                    issue.rule,
                    issue.element,
                    issue.message
                ),
                finding,
            });
        }
    }
    releases.update(&candidate.id, |r| {
        r.artifacts.retain(|a| !a.is_ux());
        r.artifacts.extend(rows);
    });
    Ok(out)
}

#[derive(Debug, Default, Deserialize)]
struct UxBody {
    #[serde(default)]
    base: String,
    #[serde(default)]
    ux: Option<String>,
}

/// The project's UI checks as its repository declares them (`.plaza/ux.toml` at main), or None when it declares none.
fn declared(app: &App, project: &str) -> Result<Option<String>, String> {
    let Some(repo) = app.projects.get(project).and_then(|p| p.repo) else {
        return Ok(None);
    };
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?;
    let client = app.weft.client("hub")?;
    // A repository answers a missing file with an error: the project declares no checks.
    match client.get_bytes(&format!("/revisions/{rev}/file?path={FILE}")) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| format!("{FILE} is not text: {e}")),
        Err(_) => Ok(None),
    }
}

fn ux_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: UxBody = if req.body.is_empty() {
        UxBody::default()
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(body) => body,
            Err(e) => return Response::error(400, &format!("the body is not a UI request: {e}")),
        }
    };
    let Some(run) = app
        .test_lab
        .runs()
        .into_iter()
        .find(|r| r.id == p.get("id"))
    else {
        return Response::error(404, "no such run");
    };
    if run.state != "done" {
        return Response::error(409, "that run is not done: its UI checks run when it ends");
    }
    if run.release.is_empty() {
        return Response::error(
            400,
            "the run qualifies no release, so it has no UI baseline to judge by",
        );
    }
    if body.base.is_empty() {
        return Response::error(400, "the UI checks need the lane's preview address (base)");
    }
    let Some(candidate) = app.releases.get(&run.release) else {
        return Response::error(400, "the run's release is not in the hub");
    };
    let text = match body.ux {
        Some(text) => text,
        None => match declared(app, &run.project) {
            Ok(Some(text)) => text,
            Ok(None) => {
                return Response::error(400, "the project declares no UI checks (.plaza/ux.toml)");
            }
            Err(why) => return Response::error(400, &why),
        },
    };
    let spec = match Spec::parse(&text) {
        Ok(spec) => spec,
        Err(why) => return Response::error(400, &why),
    };
    let checks = spec.checks(&body.base);
    let reads = match crate::user_test::ux_reads(app, &run.project, &candidate.from, &spec, &checks)
    {
        Ok(reads) => reads,
        Err(why) => return Response::error(400, &why),
    };
    let prior = app.releases.list(&run.project);
    match stage(
        app.blobs.as_ref(),
        &app.reviews,
        &app.releases,
        &prior,
        &run,
        &candidate,
        &spec,
        &reads,
    ) {
        Ok(verdicts) => Response::json(
            200,
            &json!({"run": run.id, "release": run.release, "verdicts": verdicts}),
        ),
        Err(why) => Response::error(400, &why),
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/test-lab/runs/{id}/ux", ux_route)
        .doc(RouteDoc::new("A done run's UI checks: its candidate's pages read in the lane's test fork at each declared size, theme and locale, judged against the previous qualified stable release's screenshots; a failing check is filed as a visual, accessibility or layout finding on the run's change, and the candidate's screenshots are kept as its release's artifacts").errors(&[400, 404, 409]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_observe::tests::{review_of, reviews_in};
    use crate::releases::entity::ReleaseState;
    use crate::test_lab::LabRun;
    use apz_ux::audit::audit;
    use apz_ux::tree::{AxNode, Snapshot};
    use apz_ux::visual::{Raster, Rect};

    const DECLARED: &str = "[visual]\nchannel_threshold = 8\nmax_changed_ratio = 0.001\n\n[[device]]\nname = \"desktop\"\nwidth = 64\nheight = 48\n\n[[page]]\nname = \"shop\"\npath = \"/\"\ndevices = [\"desktop\"]\n";

    fn release(id: &str, state: &str) -> Release {
        serde_json::from_value(json!({
            "id": id, "project": "p", "repo": "r", "version": "1.0.0", "channel": "stable",
            "lane": "main", "from": "abc", "state": state, "notes": "", "changes": [],
            "artifacts": [], "verdicts": [], "promotions": [], "sync": [],
            "created_ms": 0, "updated_ms": 0,
        }))
        .expect("a release")
    }

    fn lab_run(id: &str, release: &str, change: &str) -> LabRun {
        LabRun {
            id: id.into(),
            project: "p".into(),
            channel: "stable".into(),
            state: "done".into(),
            cpus: 1,
            memory_mib: 512,
            needs: Vec::new(),
            reservations: Vec::new(),
            enqueued_ms: 1_700_000_000_000,
            started_ms: Some(1_700_000_000_000),
            ended_ms: Some(1_700_000_000_100),
            outcome: None,
            change: change.into(),
            topology: "fake-run".into(),
            release: release.into(),
            token_sha: crate::lab_observe::token_hash("t"),
            observed: 0,
            dropped: 0,
        }
    }

    fn spec() -> Spec {
        Spec::parse(DECLARED).unwrap()
    }

    fn check() -> Check {
        spec().checks("http://preview.test").remove(0)
    }

    fn block() -> Rect {
        Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        }
    }

    /// A 64 by 48 page with a dark block at `at` (none for a blank page), as the browser's PNG.
    fn picture(at: Option<Rect>) -> Vec<u8> {
        let pixels = (0..64 * 48)
            .map(|i| {
                let (x, y) = (i % 64, i / 64);
                match at {
                    Some(r) if r.contains(x, y) => [20, 20, 20, 255],
                    _ => [255, 255, 255, 255],
                }
            })
            .collect();
        visual::encode(&Raster {
            width: 64,
            height: 48,
            pixels,
        })
        .unwrap()
    }

    /// The read of the check: its screenshot and the issues the rules found on it.
    fn read(screenshot: Vec<u8>, issues: Vec<Issue>) -> (Check, Report) {
        let check = check();
        let report = Report {
            label: check.label(),
            screenshot,
            visual: None,
            diff: None,
            issues,
        };
        (check, report)
    }

    /// A temporary object store for the candidate's pictures, and a review graph with the change `c1` in it.
    fn env() -> (tempfile::TempDir, Blobs, Reviews) {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::scratch(&dir.path().join("ux")).unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        (dir, blobs, reviews)
    }

    #[test]
    fn a_run_with_no_previous_release_records_the_baseline_and_passes() {
        let (_dir, blobs, reviews) = env();
        let store = Releases::default();
        store.insert(release("rel-1", "candidate"));
        let candidate = store.get("rel-1").unwrap();
        let reads = [read(picture(Some(block())), Vec::new())];
        let verdicts = stage(
            &blobs,
            &reviews,
            &store,
            &[],
            &lab_run("q1", "rel-1", "c1"),
            &candidate,
            &spec(),
            &reads,
        )
        .unwrap();
        assert_eq!(verdicts.len(), 1, "{verdicts:?}");
        assert!(verdicts[0].passed, "{verdicts:?}");
        assert!(
            verdicts[0].detail.contains("records the baseline"),
            "{}",
            verdicts[0].detail
        );
        assert!(verdicts[0].finding.is_empty());
        // The candidate's picture is kept as its release's artifact, keyed by page, size, theme and locale, and is not a build.
        let stored = store.get("rel-1").unwrap();
        assert_eq!(stored.artifacts.len(), 1);
        assert_eq!(stored.artifacts[0].target, "ux/shop/desktop/light/en");
        assert!(stored.artifacts[0].is_ux());
        assert_eq!(reviews.run("rv-c1").unwrap().findings().count(), 0);
    }

    #[test]
    fn a_seeded_shift_against_the_stored_baseline_files_a_visual_finding() {
        let (_dir, blobs, reviews) = env();
        let store = Releases::default();
        store.insert(release("rel-1", "candidate"));
        let first = store.get("rel-1").unwrap();
        stage(
            &blobs,
            &reviews,
            &store,
            &[],
            &lab_run("q1", "rel-1", ""),
            &first,
            &spec(),
            &[read(picture(Some(block())), Vec::new())],
        )
        .unwrap();
        // The previous release is qualified: its pictures are the baseline of the next.
        store.update("rel-1", |r| r.state = ReleaseState::Qualified);
        store.insert(release("rel-2", "candidate"));
        let candidate = store.get("rel-2").unwrap();
        let prior = store.list("p");
        // The candidate's block moved 3 pixels right: a layout regression.
        let shifted = Rect {
            x: block().x + 3,
            ..block()
        };
        let reads = [read(picture(Some(shifted)), Vec::new())];
        let verdicts = stage(
            &blobs,
            &reviews,
            &store,
            &prior,
            &lab_run("q2", "rel-2", "c1"),
            &candidate,
            &spec(),
            &reads,
        )
        .unwrap();
        let visual: Vec<&Verdict> = verdicts.iter().filter(|v| v.kind == "visual").collect();
        assert_eq!(visual.len(), 1, "{verdicts:?}");
        assert!(!visual[0].passed);
        assert!(
            visual[0].detail.contains("pixels changed"),
            "{}",
            visual[0].detail
        );
        assert!(visual[0].detail.contains("rel-1"), "{}", visual[0].detail);
        let (review, id) = visual[0]
            .finding
            .split_once('/')
            .expect("the finding is filed");
        let run = reviews.run(review).unwrap();
        let finding = run.findings().find(|f| f.id == id).expect("on the run");
        assert_eq!(finding.kind, "visual");
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert!(finding.proof.contains("diff picture"), "{}", finding.proof);
        assert!(finding.proof.contains("screenshot"), "{}", finding.proof);
    }

    #[test]
    fn a_missing_label_files_an_accessibility_finding() {
        let (_dir, blobs, reviews) = env();
        let store = Releases::default();
        store.insert(release("rel-1", "candidate"));
        let candidate = store.get("rel-1").unwrap();
        // The tree of a page whose icon-only button has no name.
        let tree = Snapshot {
            lang: "en".into(),
            nodes: vec![AxNode {
                key: "dom-2".into(),
                role: "button".into(),
                name: String::new(),
                ignored: false,
            }],
            elements: Vec::new(),
        };
        let reads = [read(picture(None), audit(&tree))];
        let verdicts = stage(
            &blobs,
            &reviews,
            &store,
            &[],
            &lab_run("q1", "rel-1", "c1"),
            &candidate,
            &spec(),
            &reads,
        )
        .unwrap();
        let access: Vec<&Verdict> = verdicts
            .iter()
            .filter(|v| v.kind == "accessibility")
            .collect();
        assert_eq!(access.len(), 1, "{verdicts:?}");
        assert!(!access[0].passed);
        assert!(
            access[0].detail.contains("control-name"),
            "{}",
            access[0].detail
        );
        let (review, id) = access[0]
            .finding
            .split_once('/')
            .expect("the finding is filed");
        let run = reviews.run(review).unwrap();
        let finding = run.findings().find(|f| f.id == id).expect("on the run");
        assert_eq!(finding.kind, "accessibility");
        assert!(finding.text.contains("control-name"), "{}", finding.text);
    }
}
