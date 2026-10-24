//! The release entity (PLAN 22.2): a release with its own life, kept typed in `plaza.db`. The table `release` holds
//! one row per release; its changes, artifacts, verdicts, promotions and sync records are child tables. Every state the
//! entity has is what Weft's `release.*` statements made it (`flow.rs` reads it back after each one), so the entity is
//! a working copy: the statements are the truth and travel with the repository.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

/// Where a release is in its life (22.2). A failed candidate is never patched: a new one is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    Draft,
    Candidate,
    Qualifying,
    Qualified,
    Published,
    Promoted,
    Failed,
    Abandoned,
    Superseded,
    Yanked,
}

impl ReleaseState {
    /// The state a Weft statement names (`release.candidate` and the rest), as Weft's life spells it.
    pub fn of_weft(name: &str) -> Option<ReleaseState> {
        serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
    }
}

/// One release of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "release")]
pub struct Release {
    /// `rel-<n>`, unique in the hub.
    #[table(key)]
    pub id: String,
    #[table(index)]
    pub project: String,
    /// The Weft repository the release lives in, and the release's version there.
    pub repo: String,
    pub version: String,
    /// `stable`, `beta`, `nightly`, or one the project defines: how deep qualification goes (22.3).
    pub channel: String,
    /// The release it follows: the notes and every comparison start there.
    #[serde(default)]
    pub base: Option<String>,
    /// `main`, or a release lane for maintenance releases (22.3).
    pub lane: String,
    /// The revision the release is cut from.
    pub from: String,
    pub state: ReleaseState,
    /// The candidate's number, `rc.N`, when the project numbers its candidates.
    #[serde(default)]
    pub number: Option<u32>,
    /// The notes, written from the changes' review graphs at the draft.
    #[table(content)]
    pub notes: String,
    /// The release recipe's run (recipes/runs.rs): the latest attempt at the candidate.
    #[serde(default)]
    pub run: Option<String>,
    #[table(child = "release_change")]
    pub changes: Vec<ReleaseChange>,
    #[table(child = "release_artifact")]
    pub artifacts: Vec<ReleaseArtifact>,
    /// The targets no pool could build, each with the reason (20.5): the release is cut without them.
    #[serde(default)]
    #[table(child = "release_not_run")]
    pub not_run: Vec<ReleaseNotRun>,
    /// The verdicts on the release (its checks and its qualification), as Weft recorded them.
    #[table(child = "release_verdict")]
    pub verdicts: Vec<ReleaseVerdict>,
    #[table(child = "release_promotion")]
    pub promotions: Vec<ReleasePromotion>,
    /// Its tag and release on each git platform and registry (22.4; empty until 9.4).
    #[table(child = "release_sync")]
    pub sync: Vec<ReleaseSync>,
    /// The steps of its store submissions, per store track, as each finished (9.6; `stores`).
    #[serde(default)]
    #[table(child = "release_store_step")]
    pub store_steps: Vec<ReleaseStoreStep>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

/// A landing the release holds, with the review run that checked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseChange {
    #[table(key)]
    pub change: String,
    pub title: String,
    #[serde(default)]
    pub review_run: Option<String>,
}

/// One artifact per target: the file Weft holds, its digest and size, its signatures, its SBOM and provenance, and the
/// pool, base and run that built it (the pool is 7.5's placement; empty until a pool builds it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseArtifact {
    #[table(key)]
    pub target: String,
    /// The artifact's id in Weft.
    pub artifact: String,
    pub digest: String,
    pub size: u64,
    /// The signatures on the artifact, one line each (`signing.rs`: the hub's, and the platform's).
    #[serde(default)]
    pub signatures: String,
    /// The artifact's software bill of materials, a CycloneDX 1.5 document (`sbom.rs`).
    #[serde(default)]
    pub sbom: String,
    /// The artifact's provenance: an in-toto statement in a DSSE envelope the hub signed (`attest.rs`).
    #[serde(default)]
    pub provenance: String,
    #[serde(default)]
    pub pool: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub run: String,
}

impl ReleaseArtifact {
    /// A UI check's screenshot kept as the release's artifact (`ux/<page>/<device>/<theme>/<locale>`, lab_ux.rs): not a
    /// build, so verify, sync and the registries leave it out.
    pub fn is_ux(&self) -> bool {
        self.target.starts_with("ux/")
    }
}

/// A target a candidate did not build: no pool in service can build it, and the reason says which pool it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseNotRun {
    #[table(key)]
    pub target: String,
    pub reason: String,
}

/// A verdict Weft recorded on the release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseVerdict {
    /// `ci` or `qualification`.
    pub source: String,
    /// `passed`, `failed` or `abandoned`.
    pub verdict: String,
    #[serde(default)]
    pub reason: String,
    /// The statement's sequence number in Weft's log.
    pub seq: u64,
}

/// A promotion to a deploy environment or a store track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleasePromotion {
    pub environment: String,
    pub seq: u64,
    /// The health result the promotion was judged by (23; empty until then).
    #[serde(default)]
    pub health: String,
}

/// A release on a platform or registry: its state there and where (22.4; none until 9.4 syncs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseSync {
    pub platform: String,
    pub state: String,
    #[serde(default)]
    pub url: Option<String>,
    pub at_ms: u64,
}

/// One step of a store submission (9.6): `done` (with the reference the step produced, such as the build's or the edit's
/// id) or `failed` (with the reason). A done step is not sent again; a retry resumes from the failed one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct ReleaseStoreStep {
    /// The track's name (`testflight/internal`, `play/production`, …).
    pub track: String,
    pub step: String,
    pub state: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub reason: String,
    pub at_ms: u64,
}

#[derive(Default)]
struct Inner {
    releases: BTreeMap<String, Release>,
    next: u64,
}

/// Every release of every project.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Inner>,
}

fn numbered(id: &str) -> u64 {
    id.strip_prefix("rel-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

impl Store {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new release id.
    pub fn next_id(&self) -> String {
        let mut inner = self.lock();
        inner.next += 1;
        format!("rel-{}", inner.next)
    }

    pub fn insert(&self, release: Release) {
        self.lock().releases.insert(release.id.clone(), release);
    }

    pub fn get(&self, id: &str) -> Option<Release> {
        self.lock().releases.get(id).cloned()
    }

    /// The releases of a project, newest first.
    pub fn list(&self, project: &str) -> Vec<Release> {
        let mut out: Vec<Release> = self
            .lock()
            .releases
            .values()
            .filter(|r| r.project == project)
            .cloned()
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(numbered(&r.id)));
        out
    }

    /// Changes the release `id` in place and returns it as it is now (`None` when there is no such release).
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Release)) -> Option<Release> {
        let mut inner = self.lock();
        let release = inner.releases.get_mut(id)?;
        change(release);
        Some(release.clone())
    }

    /// The rows of every release, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        RowSet::list(def(), inner.releases.values())
    }

    /// Replaces the releases with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<Release> = apz_store::table::load_list(conn, def())?;
        let mut inner = self.lock();
        inner.next = stored.iter().map(|r| numbered(&r.id)).max().unwrap_or(0);
        inner.releases = stored.into_iter().map(|r| (r.id.clone(), r)).collect();
        Ok(())
    }
}

/// The table of the releases: `release`, with its child tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Release)
}
