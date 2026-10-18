//! The bodies of Weft's statements and the things they build.
//!
//! Every change to history is one signed statement (see `record.rs`). Its `kind` names one of the bodies
//! below; `state.rs` replays them in order into the state the server answers from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::Hash;
use crate::keys::PublicKey;
use crate::record::Actor;

/// A file's change in a revision: its new content, or `None` when it is deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    pub path: String,
    pub after: Option<Hash>,
    #[serde(default = "regular")]
    pub mode: u32,
}

pub fn regular() -> u32 {
    0o100644
}

/// One file in a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub hash: Hash,
    pub mode: u32,
}

/// A full tree: path → entry.
pub type Tree = BTreeMap<String, Entry>;

/// The id of a revision: `w-1041.d3` (a draft), `w-1041.r1` (sealed), `w-1041.l` (landed), or `root` (the
/// empty tree every repo starts from).
pub type RevId = String;

pub fn change_name(id: u64) -> String {
    format!("w-{id}")
}

pub fn parse_change(s: &str) -> Result<u64, String> {
    s.strip_prefix("w-")
        .and_then(|n| n.split('.').next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("not a change: {s:?} (w-<n>)"))
}

/// Who holds a key, as its certificate says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub actor: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

// ── statement bodies ──────────────────────────────────────────────────────────────────────────────────────────

/// `genesis`: the first record; names the owner's root key. Signed by that key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub owner: PublicKey,
    pub plaza: String,
}

/// `cert.issue`: a key belongs to an actor, with these capabilities. Hub capabilities: `certify-agents`
/// (issue agent and node certificates), `rule-for-owner` (sign rulings, edits and releases the owner makes in
/// the console, marked as such).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CertIssue {
    pub key: PublicKey,
    pub holder: Holder,
    #[serde(default)]
    pub caps: Vec<String>,
}

/// `cert.revoke`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CertRevoke {
    pub key: String,
    pub reason: String,
}

/// `repo.create`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoCreate {
    pub repo: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub description: String,
}

/// `lane.open`: a task's lane, on `base` (a landed revision of `ref`) or stacked on another change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaneOpen {
    pub repo: String,
    pub change: String,
    #[serde(default = "main_ref", rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub title: String,
    pub author: Actor,
    #[serde(default)]
    pub stacked_on: Option<String>,
    pub base: RevId,
    /// `normal`, `revert`, `owner-edit`, `resolve`.
    #[serde(default = "normal_kind")]
    pub kind: String,
    #[serde(default)]
    pub reverts: Option<String>,
}

pub fn main_ref() -> String {
    "main".into()
}

fn normal_kind() -> String {
    "normal".into()
}

/// `revision.draft`: a turn's snapshot (deltas against the lane's base, cumulative).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draft {
    pub change: String,
    pub n: u32,
    pub deltas: Vec<Delta>,
    #[serde(default)]
    pub tool_calls: Vec<String>,
    #[serde(default)]
    pub turn: Option<u64>,
    /// The model the agent ran on this turn.
    #[serde(default)]
    pub model: Option<String>,
}

/// `revision.seal`: the worker's finished revision (round `n`), deltas against the lane's base.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Seal {
    pub change: String,
    pub n: u32,
    pub deltas: Vec<Delta>,
    #[serde(default)]
    pub tool_calls: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// One intent: a plain title and what it covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentDef {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    /// `feature`, `fix`, `improvement`, `refactor`, `docs`, `test`, `chore`, …
    #[serde(default)]
    pub kind: String,
    /// Paths (or globs) and `path::symbol` selectors.
    #[serde(default)]
    pub covers: Vec<String>,
}

/// `intent.set`: the intents of a change's round (replaces any before).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntentSet {
    pub change: String,
    pub n: u32,
    pub intents: Vec<IntentDef>,
}

/// `attest`: a signed record about a revision (review verdicts, checks, the gate, rulings, builds, …).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attest {
    /// The revision it is about.
    pub revision: RevId,
    pub digest: Hash,
    /// `written`, `checked`, `review`, `constraints`, `gate`, `ruling`, …
    pub predicate_type: String,
    pub predicate: Value,
}

/// `queue.add` / `queue.remove`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueMove {
    pub repo: String,
    #[serde(default = "main_ref", rename = "ref")]
    pub reference: String,
    pub change: String,
    #[serde(default)]
    pub n: u32,
    #[serde(default)]
    pub reason: String,
}

/// `conflict`: the head of a queue could not be merged onto its ref.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub change: String,
    pub n: u32,
    pub onto: RevId,
    pub files: Vec<String>,
    #[serde(default)]
    pub detail: Value,
}

/// `land`: the hub moves `ref` from `expected` to the change's landed revision, whose deltas are against
/// `expected`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Land {
    pub repo: String,
    #[serde(default = "main_ref", rename = "ref")]
    pub reference: String,
    pub change: String,
    pub n: u32,
    pub expected: RevId,
    pub deltas: Vec<Delta>,
    #[serde(default)]
    pub merged: Vec<String>,
}

/// `import`: one git commit, landed as an imported change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Import {
    pub repo: String,
    #[serde(default = "main_ref", rename = "ref")]
    pub reference: String,
    pub change: String,
    pub git: String,
    pub author_name: String,
    pub author_email: String,
    pub at_ms: u64,
    pub message: String,
    pub expected: RevId,
    pub deltas: Vec<Delta>,
    /// `Co-authored-by` trailers that name an AI assistant: the change's ranges are "mixed".
    #[serde(default)]
    pub ai_coauthors: Vec<String>,
}

/// `abandon`: a lane that will never land (the task was withdrawn or failed).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Abandon {
    pub change: String,
    pub reason: String,
}

/// `undo`: undoes an earlier op, where that makes sense.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Undo {
    pub seq: u64,
    pub reason: String,
}

/// `path.rule`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRule {
    pub repo: String,
    pub glob: String,
    /// Roles that may read these paths (empty: everyone in the project).
    #[serde(default)]
    pub read: Vec<String>,
    /// Extra reviewer roles a change here needs (`owner` for the owner).
    #[serde(default)]
    pub reviewers: Vec<String>,
    #[serde(default = "yes")]
    pub export: bool,
}

fn yes() -> bool {
    true
}

/// `connection.add`: a git remote this repo exports to (imported from once, when connected).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub repo: String,
    pub id: String,
    /// `https://github.com/owner/repo.git`, `file:///path/to/bare.git`, …
    pub remote: String,
    /// `github`, `git`.
    #[serde(default)]
    pub platform: String,
    #[serde(default = "main_ref")]
    pub branch: String,
    /// The git author of exported commits: the owner as the platform's authorization knows them.
    pub author_name: String,
    pub author_email: String,
    /// Where the credential is, in apid's vault (never the credential itself).
    #[serde(default)]
    pub credential: Option<String>,
}

/// `connection.remove`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionRemove {
    pub repo: String,
    pub id: String,
}

/// `export`: what a connection received for a landed change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    pub repo: String,
    pub connection: String,
    pub change: String,
    /// One per intent, in order.
    pub commits: Vec<ExportedCommit>,
    pub head: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedCommit {
    pub intent: String,
    pub git: String,
}

/// `connection.state`: the remote moved on its own, or an error, or back in step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionState {
    pub repo: String,
    pub id: String,
    /// `in-step`, `moved`, `error`.
    pub state: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub remote_head: Option<String>,
}

/// `artifact.put`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactPut {
    pub id: String,
    pub repo: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub target: String,
    pub digest: Hash,
    pub size: u64,
    pub built_from: RevId,
    #[serde(default)]
    pub builder: String,
    #[serde(default)]
    pub command: String,
    /// The SLSA provenance statement (in-toto).
    #[serde(default)]
    pub provenance: Value,
    /// `true` when a second build on another node gave the same bytes.
    #[serde(default)]
    pub reproduced: bool,
    /// The change it belongs to, for deliverables.
    #[serde(default)]
    pub change: Option<String>,
}

/// `release.propose`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleasePropose {
    pub repo: String,
    pub version: String,
    pub from: RevId,
    #[serde(default)]
    pub line: Option<String>,
    pub notes: String,
    #[serde(default)]
    pub targets: Vec<String>,
}

/// `release.sign`: the owner signs a release (its artifacts, by digest).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseSign {
    pub repo: String,
    pub version: String,
    pub artifacts: Vec<String>,
    pub digest: Hash,
}

/// `release.export`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseExport {
    pub repo: String,
    pub version: String,
    pub connection: String,
    pub tag: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// `pack.add`: a language or format pack (its files are blobs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackAdd {
    pub name: String,
    pub version: u32,
    /// The grammar's language function name (`rust` for `tree_sitter_rust`).
    pub language: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(default)]
    pub shebangs: Vec<String>,
    /// The WebAssembly grammar.
    pub wasm: Hash,
    /// Definitions and references (a tree-sitter tags query).
    pub tags: Hash,
}

/// `pack.remove`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackRemove {
    pub name: String,
}
