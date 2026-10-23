//! The release flow (PLAN 22.2): a draft is proposed in Weft and its release run starts; a candidate builds every
//! declared target (the build step's fan-out, one item per target), uploads each artifact to Weft and has the hub sign
//! the candidate; checks, qualification, publication, promotion and yanks are statements too. After each statement the
//! entity is read back from Weft's life of the release, so the entity never runs ahead of the log.
//!
//! The flow takes its three dependencies as [`Deps`]: Weft (a [`Ledger`]), the recipe runs and the entity store. The
//! hub's own Weft client is the production ledger; the tests give the flow a Weft of their own. A build is a [`Builder`]:
//! production has no build pool to place a target on until 7.5, so [`NoPools`] refuses; the tests build with a fake.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};
use weft::hash::Hash;

use super::channels;
use super::entity::{
    Release, ReleaseArtifact, ReleaseChange, ReleasePromotion, ReleaseState, ReleaseVerdict, Store,
};
use super::lanes;
use super::versions::{self, Bump, Note};
use crate::recipes::graph::Report;
use crate::recipes::runs::{RunState, Runs};
use crate::review_graph::{Reviews, Run};

/// What the flow needs from Weft: reads and hub writes through its routes, and the statements the hub signs.
pub trait Ledger {
    /// `GET /v1/<path>`.
    fn read(&self, path: &str) -> Result<Value, String>;
    /// `POST /v1/<path>` as the hub.
    fn post(&self, path: &str, body: &Value) -> Result<Value, String>;
    /// A statement of `kind` the hub signs.
    fn statement(&self, kind: &str, body: Value) -> Result<Value, String>;
    /// Uploads bytes to Weft's blob store and returns their hash.
    fn upload(&self, bytes: &[u8]) -> Result<Hash, String>;
}

impl Ledger for crate::weft_hub::Weft {
    fn read(&self, path: &str) -> Result<Value, String> {
        self.get(path)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.post(path, body)
    }

    fn statement(&self, kind: &str, body: Value) -> Result<Value, String> {
        self.hub(kind, body)
    }

    fn upload(&self, bytes: &[u8]) -> Result<Hash, String> {
        let hash = Hash::of(bytes);
        self.client("hub")?
            .upload(&[hash], &|_: &Hash| Ok(bytes.to_vec()))?;
        Ok(hash)
    }
}

/// One target's build: its bytes and what built them.
pub struct Built {
    pub bytes: Vec<u8>,
    /// The pool that built it (7.5), and the base image or machine.
    pub pool: String,
    pub base: String,
    pub builder: String,
    pub command: String,
}

/// Builds one target of a release. The production builder is [`NoPools`].
pub trait Builder {
    fn build(&self, release: &Release, target: &str) -> Result<Built, String>;
}

/// The builder of a deployment whose placement (7.5) has no pool for a target yet: every build is refused.
pub struct NoPools;

impl Builder for NoPools {
    fn build(&self, _release: &Release, target: &str) -> Result<Built, String> {
        Err(format!(
            "no build pool can build {target} yet (placing targets on pools is 7.5)"
        ))
    }
}

/// The dependencies of the flow.
pub struct Deps<'a> {
    pub weft: &'a dyn Ledger,
    pub runs: &'a Runs,
    pub reviews: &'a Reviews,
    pub releases: &'a Store,
}

/// A request to cut a draft (and optionally a candidate).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Cut {
    /// The version. When absent it is numbered from the changes since `base` (22.3).
    #[serde(default)]
    pub version: Option<String>,
    /// How the version is numbered when it is not given: `semver` (the default), `calendar` or `build`.
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    /// The release it follows, by id or version: the version is bumped from its version, and the notes start there.
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub lane: Option<String>,
    /// The revision to cut from (default: the head of main).
    #[serde(default)]
    pub from: Option<String>,
    /// The landed changes the release holds.
    #[serde(default)]
    pub changes: Vec<String>,
    #[serde(default)]
    pub number: Option<u32>,
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn text(v: &Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

/// The entity `id`, or why there is none.
pub fn get(d: &Deps, id: &str) -> Result<Release, String> {
    d.releases
        .get(id)
        .ok_or_else(|| "no such release".to_string())
}

/// The notes a change contributes to a release (22.2): its review graph's account when it has one (the run's summary,
/// the reasons of its outcome, and the findings its nodes raised), otherwise its intents (the latest round's, Weft's).
pub fn change_notes(title: &str, intents: &Value, review: Option<&Run>) -> String {
    let mut lines = vec![format!("## {title}")];
    if let Some(run) = review {
        let before = lines.len();
        if !run.summary.trim().is_empty() {
            lines.push(run.summary.trim().to_string());
        }
        if let Some(outcome) = &run.outcome {
            lines.extend(outcome.reasons.iter().map(|r| format!("- {r}")));
        }
        for node in &run.nodes {
            for f in &node.findings {
                lines.push(format!("- [{}] {}: {}", f.severity, f.symbol, f.text));
            }
        }
        if lines.len() > before {
            return lines.join("\n");
        }
    }
    // The fallback: the latest round's intents, each with its summary.
    for intent in latest_intents(intents) {
        lines.push(format!("- {}", intent["title"].as_str().unwrap_or("")));
        if let Some(summary) = intent["summary"].as_str().filter(|s| !s.trim().is_empty()) {
            lines.push(format!("  {}", summary.trim()));
        }
    }
    lines.join("\n")
}

/// The intents of a change's latest round (Weft keeps a change's intents per round: round `n` → its list).
fn latest_intents(intents: &Value) -> &[Value] {
    intents
        .as_object()
        .and_then(|rounds| {
            rounds
                .iter()
                .filter_map(|(n, list)| n.parse::<u32>().ok().map(|n| (n, list)))
                .max_by_key(|(n, _)| *n)
        })
        .and_then(|(_, list)| list.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The version of a draft that names none: the next one after its base, bumped by its changes, in the project's scheme.
/// The base is the one the cut names, or the newest release of the line (of main, when the cut is for main).
fn number(
    d: &Deps,
    project: &str,
    cut: &Cut,
    lane: &str,
    bumps: &[Bump],
) -> Result<String, String> {
    if bumps.is_empty() {
        return Err("a release needs a version, or changes to number it from".into());
    }
    let scheme = versions::Scheme::parse(cut.scheme.as_deref())?;
    let base = match cut.base.as_deref() {
        Some(base) => Some(
            d.releases
                .list(project)
                .into_iter()
                .find(|r| r.id == base || r.version == base)
                .ok_or_else(|| format!("{base} is not a release of {project}"))?
                .version,
        ),
        None => d
            .releases
            .list(project)
            .into_iter()
            .filter(|r| {
                !matches!(
                    r.state,
                    ReleaseState::Draft | ReleaseState::Failed | ReleaseState::Abandoned
                )
            })
            .find(|r| {
                if lane == "main" {
                    r.lane == "main"
                } else {
                    lanes::line_of(r) == lane
                }
            })
            .map(|r| r.version),
    };
    let (year, month) = versions::year_month(apz_plaza::now_ms());
    versions::next(
        scheme,
        base.as_deref(),
        bumps,
        lane != "main",
        (year, month),
    )
}

/// Starts a release run for `subject` with `targets` build items, and settles its draft (the draft is a stand-in
/// engine's: the proposal in Weft is its work).
fn start_run(d: &Deps, project: &str, subject: &str, targets: usize) -> Result<String, String> {
    let items = BTreeMap::from([("build".to_string(), targets.max(1) as u32)]);
    let run = d
        .runs
        .start_engine_items(
            "release",
            "release",
            None,
            Some(project),
            Some(subject),
            &items,
            now(),
        )
        .ok_or("no release recipe is on the hub")?;
    d.runs.settle(&run, "draft", &Report::verdict(true), now());
    Ok(run)
}

/// Cuts a draft: checks the changes are landed, proposes the release in Weft (the notes and the targets come from its
/// statement), and starts its run. `repo` is the project's Weft repository.
pub fn draft(d: &Deps, project: &str, repo: &str, cut: &Cut) -> Result<Release, String> {
    let channel = channels::of(cut.channel.as_deref().unwrap_or("stable"))?;
    let lane = cut.lane.clone().unwrap_or_else(|| "main".into());
    // A release of a line names the line to Weft (`release/1.4`); a release of main names none.
    let line = Some(lane.as_str()).filter(|l| *l != "main");
    let mut changes = Vec::new();
    let mut sections = Vec::new();
    let mut bumps = Vec::new();
    for change in &cut.changes {
        let detail = d.weft.read(&format!("/changes/{change}"))?;
        if detail["state"].as_str() != Some("landed") {
            return Err(format!(
                "{change} has not landed; a release holds landed changes"
            ));
        }
        let title = text(&detail["title"]);
        // A change with no intents is read by its title, as a patch unless its title says otherwise.
        let intents = latest_intents(&detail["intents"]);
        let notes: Vec<Note> = if intents.is_empty() {
            vec![Note {
                title: title.clone(),
                ..Note::default()
            }]
        } else {
            intents
                .iter()
                .map(|i| Note {
                    kind: text(&i["kind"]),
                    title: text(&i["title"]),
                    summary: text(&i["summary"]),
                })
                .collect()
        };
        bumps.push(versions::bump_of(&notes));
        // The change's review run: the newest review of its task (sealed rounds, not re-checks).
        let review = detail["task"]
            .as_str()
            .filter(|task| !task.is_empty())
            .and_then(|task| {
                d.reviews
                    .runs_of(task)
                    .into_iter()
                    .find(|run| run.purpose == "review")
            });
        sections.push(change_notes(&title, &detail["intents"], review.as_ref()));
        changes.push(ReleaseChange {
            change: change.clone(),
            title,
            review_run: review.map(|run| run.id),
        });
    }
    let version = match cut
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(version) => version.to_string(),
        None => number(d, project, cut, &lane, &bumps)?,
    };
    let proposal = d.weft.post(
        &format!("/repos/{repo}/releases"),
        &json!({ "version": version, "from": cut.from, "line": line }),
    )?;
    let targets: Vec<String> = proposal["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    let id = d.releases.next_id();
    let stamp = now();
    let release = Release {
        id: id.clone(),
        project: project.to_string(),
        repo: repo.to_string(),
        version: version.clone(),
        channel: channel.name,
        base: cut.base.clone(),
        lane,
        from: text(&proposal["from"]),
        state: ReleaseState::Draft,
        number: None,
        notes: if sections.is_empty() {
            text(&proposal["notes"])
        } else {
            // Each change's notes come from its review graph, or from its intents; Weft's proposal notes only when
            // no change is named.
            sections.join("\n\n")
        },
        run: None,
        sbom: String::new(),
        provenance: String::new(),
        changes,
        artifacts: Vec::new(),
        verdicts: Vec::new(),
        promotions: Vec::new(),
        sync: Vec::new(),
        created_ms: stamp,
        updated_ms: stamp,
    };
    d.releases.insert(release);
    let run = start_run(d, project, &id, targets.len())?;
    d.releases.update(&id, |r| r.run = Some(run.clone()));
    refresh(d, &id)
}

/// Cuts the candidate of a draft: builds every target, uploads each artifact, has the hub sign the candidate, and
/// completes the candidate step of its run. A refused build leaves the release a draft; the run that tried is failed.
pub fn candidate(
    d: &Deps,
    id: &str,
    builder: &dyn Builder,
    number: Option<u32>,
) -> Result<Release, String> {
    let rel = get(d, id)?;
    if rel.state != ReleaseState::Draft {
        return Err(format!(
            "{} is {:?}; a candidate is cut from a draft",
            rel.version, rel.state
        ));
    }
    let view = d
        .weft
        .read(&format!("/repos/{}/releases/{}", rel.repo, rel.version))?;
    let targets: Vec<String> = view["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    let targets = if targets.is_empty() {
        vec![String::new()]
    } else {
        targets
    };
    // The run the draft started, while it is still open; otherwise a new run whose draft is settled again.
    let run = match rel.run.clone().filter(|run| {
        d.runs
            .view(run)
            .is_some_and(|v| v.run.state == RunState::Running)
    }) {
        Some(run) => run,
        None => {
            let run = start_run(d, &rel.project, id, targets.len())?;
            d.releases.update(id, |r| r.run = Some(run.clone()));
            run
        }
    };
    d.runs.started(&run, "build", now());
    let mut artifacts = Vec::new();
    for target in &targets {
        match build_one(d, &rel, builder, target, &run) {
            Ok(row) => {
                artifacts.push(row);
                d.runs.settle(&run, "build", &Report::verdict(true), now());
            }
            Err(e) => {
                d.runs.settle(&run, "build", &Report::verdict(false), now());
                return Err(e);
            }
        }
    }
    let ids: Vec<String> = artifacts.iter().map(|a| a.artifact.clone()).collect();
    let digest = d.weft.post(
        &format!("/repos/{}/releases/{}/digest", rel.repo, rel.version),
        &json!({ "artifacts": ids }),
    )?["digest"]
        .clone();
    d.weft.statement(
        "release.candidate",
        json!({ "repo": rel.repo, "version": rel.version, "artifacts": ids, "digest": digest, "number": number }),
    )?;
    d.releases.update(id, |r| r.artifacts = artifacts.clone());
    d.runs.started(&run, "candidate", now());
    d.runs
        .settle(&run, "candidate", &Report::verdict(true), now());
    refresh(d, id)
}

/// One target: built, uploaded, and recorded in Weft as an artifact of the release's revision.
fn build_one(
    d: &Deps,
    rel: &Release,
    builder: &dyn Builder,
    target: &str,
    run: &str,
) -> Result<ReleaseArtifact, String> {
    let built = builder
        .build(rel, target)
        .map_err(|e| format!("{}: {e}", label(target)))?;
    let blob = d.weft.upload(&built.bytes)?;
    let art = d.weft.post(
        "/artifacts",
        &json!({
            "blob": blob, "repo": rel.repo, "name": rel.project, "version": rel.version, "target": target,
            "built_from": rel.from, "builder": built.builder, "command": built.command,
        }),
    )?;
    Ok(ReleaseArtifact {
        target: target.to_string(),
        artifact: text(&art["id"]),
        digest: text(&art["digest"]),
        size: art["size"].as_u64().unwrap_or(0),
        signatures: String::new(),
        pool: built.pool,
        base: built.base,
        run: run.to_string(),
    })
}

fn label(target: &str) -> &str {
    if target.is_empty() {
        "the release"
    } else {
        target
    }
}

/// Records the hub's check result for the release's commit (`ci`): passed or failed.
pub fn checks(d: &Deps, id: &str, passed: bool, reason: &str) -> Result<Release, String> {
    let verdict = if passed { "passed" } else { "failed" };
    move_with(
        d,
        id,
        "release.verdict",
        json!({ "source": "ci", "verdict": verdict, "reason": reason }),
    )
}

/// Sends a candidate to qualification (22.10).
pub fn qualify(d: &Deps, id: &str) -> Result<Release, String> {
    move_with(d, id, "release.qualify", json!({}))
}

/// Publishes a release whose checks passed (or that was qualified).
pub fn publish(d: &Deps, id: &str) -> Result<Release, String> {
    move_with(d, id, "release.publish", json!({}))
}

/// Promotes a published release to an environment or a store track.
pub fn promote(d: &Deps, id: &str, environment: &str) -> Result<Release, String> {
    move_with(
        d,
        id,
        "release.promote",
        json!({ "environment": environment }),
    )
}

/// Withdraws a published release, with the reason.
pub fn yank(d: &Deps, id: &str, reason: &str) -> Result<Release, String> {
    move_with(d, id, "release.yank", json!({ "reason": reason }))
}

/// Writes one statement about the release and reads its life back.
fn move_with(d: &Deps, id: &str, kind: &str, extra: Value) -> Result<Release, String> {
    let rel = get(d, id)?;
    let mut body = json!({ "repo": rel.repo, "version": rel.version });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            body.insert(k.clone(), v.clone());
        }
    }
    d.weft.statement(kind, body)?;
    refresh(d, id)
}

/// Reads the release's life from Weft into the entity: its state, verdicts, promotions and candidate number.
pub fn refresh(d: &Deps, id: &str) -> Result<Release, String> {
    let rel = get(d, id)?;
    let view = d
        .weft
        .read(&format!("/repos/{}/releases/{}", rel.repo, rel.version))?;
    let life = &view["life"];
    let state = life["state"]
        .as_str()
        .and_then(ReleaseState::of_weft)
        .ok_or("Weft's life of the release names no known state")?;
    let verdicts: Vec<ReleaseVerdict> = life["verdicts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| ReleaseVerdict {
            source: text(&v["source"]),
            verdict: text(&v["verdict"]),
            reason: text(&v["reason"]),
            seq: v["seq"].as_u64().unwrap_or(0),
        })
        .collect();
    let promotions: Vec<ReleasePromotion> = life["promotions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| ReleasePromotion {
            environment: text(&p[0]),
            seq: p[1].as_u64().unwrap_or(0),
            health: String::new(),
        })
        .collect();
    let number = life["candidate"][0]["number"].as_u64().map(|n| n as u32);
    let stamp = now();
    d.releases
        .update(id, |r| {
            r.state = state;
            r.verdicts = verdicts;
            r.promotions = promotions;
            if number.is_some() {
                r.number = number;
            }
            r.updated_ms = stamp;
        })
        .ok_or_else(|| "no such release".to_string())
}
