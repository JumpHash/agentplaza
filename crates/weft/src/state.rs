//! The state Weft answers from, rebuilt by replaying the log.
//!
//! [`State::apply`] takes one record and its statement. With `dry` it only checks the statement (is the signer
//! allowed to say this, does it fit what is already there); without, it also changes the state. The store runs
//! it dry, writes the record, then runs it for real, so a refused statement never reaches the log, and a
//! record that reached the log always applies.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::Value;

use crate::hash::Hash;
use crate::keys::PublicKey;
use crate::model::*;
use crate::record::{Actor, LogRecord, Signed, Statement};

#[derive(Clone, Debug, Serialize)]
pub struct KeyInfo {
    pub id: String,
    pub key: PublicKey,
    pub holder: Holder,
    pub caps: Vec<String>,
    pub issued_seq: u64,
    pub issuer: String,
    pub revoked_seq: Option<u64>,
    pub revoked_reason: Option<String>,
}

impl KeyInfo {
    pub fn can(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnState {
    pub conn: Connection,
    pub added_seq: u64,
    /// Exported changes, in order: (change, git head after it).
    pub exported: Vec<(u64, String)>,
    pub head: Option<String>,
    pub state: String,
    pub detail: String,
    /// While the remote's own commits are being adopted: the remote head and the change that adopts it.
    pub adopting: Option<(String, String)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Release {
    pub propose: ReleasePropose,
    pub proposed_seq: u64,
    pub signed: Option<(ReleaseSign, u64, String)>,
    pub exports: Vec<ReleaseExport>,
    /// Its life as its statements made it (22.2).
    pub life: ReleaseLife,
    /// Whether the owner edited its notes (`release.notes`): the edited notes are the release's from then on.
    pub notes_edited: bool,
}

/// A verdict on a release (`release.verdict`).
#[derive(Clone, Debug, Serialize)]
pub struct Verdict {
    pub source: String,
    pub verdict: String,
    pub reason: String,
    pub seq: u64,
}

/// A release's life: its lifecycle state (22.2) and the statements that moved it there. The states are `draft`,
/// `candidate`, `qualifying`, `qualified`, `published`, `promoted`, `failed`, `abandoned`, `superseded` and `yanked`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReleaseLife {
    pub state: String,
    /// The candidate's statement and its sequence number.
    pub candidate: Option<(ReleaseCandidate, u64)>,
    pub verdicts: Vec<Verdict>,
    /// Each promotion: the environment or store track, and the statement's sequence number.
    pub promotions: Vec<(String, u64)>,
    pub published: Option<u64>,
    /// The yank's reason and sequence number.
    pub yanked: Option<(String, u64)>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Repo {
    pub name: String,
    pub project: Option<String>,
    pub description: String,
    pub created_seq: u64,
    /// Each ref's landed revisions, oldest first; the last is the head.
    pub refs: BTreeMap<String, Vec<RevId>>,
    /// Each ref's landing queue (change ids).
    pub queues: BTreeMap<String, Vec<u64>>,
    pub rules: Vec<PathRule>,
    pub connections: BTreeMap<String, ConnState>,
    pub releases: BTreeMap<String, Release>,
}

impl Repo {
    pub fn head(&self, reference: &str) -> RevId {
        self.refs
            .get(reference)
            .and_then(|v| v.last())
            .cloned()
            .unwrap_or_else(|| "root".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeState {
    Open,
    Sealed,
    Queued,
    Conflict,
    Landed,
    Abandoned,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportInfo {
    pub git: String,
    pub author_name: String,
    pub author_email: String,
    pub at_ms: u64,
    pub message: String,
    pub ai_coauthors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub id: u64,
    pub name: String,
    pub repo: String,
    pub reference: String,
    pub task: Option<String>,
    pub title: String,
    pub author: Actor,
    pub base: RevId,
    pub stacked_on: Option<u64>,
    pub kind: String,
    pub reverts: Option<u64>,
    pub drafts: Vec<RevId>,
    pub rounds: Vec<RevId>,
    pub intents: BTreeMap<u32, Vec<IntentDef>>,
    pub records: Vec<u64>,
    pub state: ChangeState,
    pub landed: Option<RevId>,
    pub conflict: Option<Conflict>,
    pub opened_seq: u64,
    pub opened_ms: u64,
    pub import: Option<ImportInfo>,
    pub exported: BTreeMap<String, Vec<ExportedCommit>>,
    /// The reviews written about this change, in order (`review.outcome`).
    pub reviews: Vec<ReviewRecord>,
}

impl Change {
    /// The latest sealed revision.
    pub fn latest(&self) -> Option<&RevId> {
        self.rounds.last()
    }

    pub fn round(&self) -> u32 {
        self.rounds.len() as u32
    }
}

/// A `review.outcome` at its place in the log.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewRecord {
    pub seq: u64,
    pub at_ms: u64,
    /// The key the hub signed it with.
    pub key: String,
    pub outcome: ReviewOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RevKind {
    Draft,
    Sealed,
    Landed,
    Imported,
}

#[derive(Clone, Debug, Serialize)]
pub struct Revision {
    pub id: RevId,
    pub change: u64,
    pub kind: RevKind,
    pub n: u32,
    pub base: RevId,
    pub deltas: Vec<Delta>,
    pub digest: Hash,
    pub tool_calls: Vec<String>,
    pub turn: Option<u64>,
    pub model: Option<String>,
    pub actor: Actor,
    pub seq: u64,
    pub at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordInfo {
    pub seq: u64,
    pub at_ms: u64,
    pub actor: Actor,
    pub key_id: String,
    /// Signed by the hub on the owner's behalf from the console (`rule-for-owner`).
    pub via_console: bool,
    pub attest: Attest,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpInfo {
    pub seq: u64,
    pub kind: String,
    pub actor: Actor,
    pub at_ms: u64,
    pub summary: String,
    /// What an undoable op touched: a change, `change#round`, `repo|glob`, `repo|connection` or a pack.
    pub subject: Option<String>,
    pub undone_by: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PackInfo {
    pub pack: PackAdd,
    pub added_seq: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactInfo {
    pub artifact: ArtifactPut,
    pub seq: u64,
    pub at_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    pub plaza: String,
    pub owner: Option<PublicKey>,
    pub keys: HashMap<String, KeyInfo>,
    pub repos: BTreeMap<String, Repo>,
    pub changes: BTreeMap<u64, Change>,
    pub revisions: HashMap<RevId, Revision>,
    pub records: BTreeMap<u64, RecordInfo>,
    pub ops: Vec<OpInfo>,
    pub packs: BTreeMap<String, PackInfo>,
    pub artifacts: BTreeMap<String, ArtifactInfo>,
    pub last_change: u64,
}

/// Kinds only the owner may sign (or the hub, marked, with `rule-for-owner`).
const OWNER_KINDS: &[&str] = &[
    "repo.create",
    "ref.create",
    "path.rule",
    "connection.add",
    "connection.remove",
    "pack.add",
    "pack.remove",
    "undo",
];
/// Kinds the hub signs (the owner may too).
const HUB_KINDS: &[&str] = &[
    "lane.open",
    "queue.add",
    "queue.remove",
    "conflict",
    "land",
    "import",
    "export",
    "connection.state",
    "artifact.put",
    "release.propose",
    "release.export",
    "abandon",
    "lane.rebase",
    "lane.author",
    // Owner-signed before W61: the hub signs a release's digest now, and the owner may too.
    "release.sign",
    "release.candidate",
    "release.qualify",
    "release.verdict",
    "release.publish",
    "release.promote",
    "release.yank",
    "release.lane",
    "release.notes",
    "review.outcome",
];

/// The digest of a revision: its base and its deltas.
pub fn digest(base: &str, deltas: &[Delta]) -> Hash {
    let mut sorted = deltas.to_vec();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let body = serde_json::to_vec(&serde_json::json!({ "base": base, "deltas": sorted })).unwrap();
    Hash::of(&body)
}

fn parse<T: serde::de::DeserializeOwned>(body: &Value, kind: &str) -> Result<T, String> {
    serde_json::from_value(body.clone()).map_err(|e| format!("{kind}: {e}"))
}

fn check_deltas(deltas: &[Delta]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for d in deltas {
        if d.path.is_empty()
            || d.path.starts_with('/')
            || d.path
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(format!("bad path {:?}", d.path));
        }
        if !seen.insert(&d.path) {
            return Err(format!("{} appears twice", d.path));
        }
        if ![0o100644, 0o100755, 0o120000].contains(&d.mode) {
            return Err(format!("{}: unsupported mode {:o}", d.path, d.mode));
        }
    }
    Ok(())
}

impl State {
    /// The key a record was signed with, if it is known and valid at `seq`.
    pub fn key_at(&self, key_id: &str, seq: u64) -> Result<&KeyInfo, String> {
        let info = self
            .keys
            .get(key_id)
            .ok_or_else(|| format!("unknown key {key_id}"))?;
        if let Some(r) = info.revoked_seq
            && seq > r
        {
            return Err(format!("key {key_id} was revoked at record {r}"));
        }
        Ok(info)
    }

    pub fn change(&self, name: &str) -> Result<&Change, String> {
        let id = parse_change(name)?;
        self.changes
            .get(&id)
            .ok_or_else(|| format!("no change {name}"))
    }

    fn change_mut(&mut self, name: &str) -> Result<&mut Change, String> {
        let id = parse_change(name)?;
        self.changes
            .get_mut(&id)
            .ok_or_else(|| format!("no change {name}"))
    }

    pub fn repo(&self, name: &str) -> Result<&Repo, String> {
        self.repos
            .get(name)
            .ok_or_else(|| format!("no repo {name}"))
    }

    /// A release of a repository, by version.
    pub fn release(&self, repo: &str, version: &str) -> Result<&Release, String> {
        self.repo(repo)?
            .releases
            .get(version)
            .ok_or_else(|| format!("no release {version}"))
    }

    fn release_mut(&mut self, repo: &str, version: &str) -> Result<&mut Release, String> {
        self.repo_mut(repo)?
            .releases
            .get_mut(version)
            .ok_or_else(|| format!("no release {version}"))
    }

    fn repo_mut(&mut self, name: &str) -> Result<&mut Repo, String> {
        self.repos
            .get_mut(name)
            .ok_or_else(|| format!("no repo {name}"))
    }

    /// A carried review (`origin`) is accepted only when the source hub signed it, under this repository's owner:
    /// the certificates lead from the owner's key to the key that signed the statement, and the statement is the
    /// same review as the body the importing hub signs (apart from the change and revision names it has here).
    fn check_origin(&self, r: &ReviewOutcome) -> Result<(), String> {
        let Some(origin) = &r.origin else {
            return Ok(());
        };
        let owner = self.owner.ok_or("the log has no genesis")?;
        if origin.owner != owner {
            return Err(format!(
                "{}: the carried review was signed under another owner",
                r.run
            ));
        }
        let signed = Signed::from_json(&origin.signed)?;
        let source_stmt = signed.statement()?;
        if source_stmt.kind != "review.outcome" || source_stmt.actor != Actor::Hub {
            return Err(format!(
                "{}: a carried review is a review the hub signed",
                r.run
            ));
        }
        let mut source: ReviewOutcome = parse(&source_stmt.body, "review.outcome")?;
        if source.origin.is_some() {
            return Err(format!("{}: a carried review is not carried again", r.run));
        }
        let mut mine = r.clone();
        for body in [&mut source, &mut mine] {
            body.origin = None;
            body.change.clear();
            body.revision.clear();
        }
        if serde_json::to_value(&source).ok() != serde_json::to_value(&mine).ok() {
            return Err(format!(
                "{}: the carried review is not the review it names",
                r.run
            ));
        }
        // Down the certificates from the owner's key.
        let mut key = owner;
        let mut hub_certified = false;
        for cert in &origin.certs {
            let c = Signed::from_json(cert)?;
            if c.key_id != key.id() {
                return Err(format!(
                    "{}: a certificate is not signed by the key above it",
                    r.run
                ));
            }
            key.verify(&c.stmt, &c.sig)?;
            let cs = c.statement()?;
            if cs.kind != "cert.issue" {
                return Err(format!("{}: a certificate is not a cert.issue", r.run));
            }
            let body: CertIssue = parse(&cs.body, "cert.issue")?;
            hub_certified = body.holder.actor == Actor::Hub;
            key = body.key;
        }
        if !hub_certified || key.id() != signed.key_id {
            return Err(format!(
                "{}: the review is not signed by a certified hub key",
                r.run
            ));
        }
        key.verify(&signed.stmt, &signed.sig)
            .map_err(|e| format!("{}: the carried review does not verify: {e}", r.run))
    }

    /// Checks (`dry`) or applies one record. Returns a one-line summary for the op log.
    pub fn apply(
        &mut self,
        rec: &LogRecord,
        stmt: &Statement,
        dry: bool,
    ) -> Result<String, String> {
        let seq = rec.seq;
        let kind = stmt.kind.as_str();
        // The first record names the owner's key and is signed by it.
        if kind == "genesis" {
            if seq != 1 || self.owner.is_some() {
                return Err("genesis must be the first record".into());
            }
            let g: Genesis = parse(&stmt.body, kind)?;
            if g.owner.id() != rec.signed.key_id || stmt.actor != Actor::Owner {
                return Err("genesis must be signed by the owner key it names".into());
            }
            if !dry {
                self.plaza = g.plaza.clone();
                self.owner = Some(g.owner);
                self.keys.insert(
                    g.owner.id(),
                    KeyInfo {
                        id: g.owner.id(),
                        key: g.owner,
                        holder: Holder {
                            actor: Actor::Owner,
                            name: Some("owner".into()),
                            role: None,
                            project: None,
                        },
                        caps: vec![],
                        issued_seq: seq,
                        issuer: g.owner.id(),
                        revoked_seq: None,
                        revoked_reason: None,
                    },
                );
                self.op(seq, stmt, format!("plaza {} begins", g.plaza));
            }
            return Ok(format!("genesis of {}", g.plaza));
        }
        if self.owner.is_none() {
            return Err("the log has no genesis".into());
        }
        let key = self.key_at(&rec.signed.key_id, seq)?.clone();
        // Who may sign as whom.
        let via_console = key.holder.actor == Actor::Hub
            && stmt.actor == Actor::Owner
            && key.can("rule-for-owner");
        if key.holder.actor != stmt.actor && !via_console {
            return Err(format!(
                "a {} key cannot sign as {}",
                key.holder.actor.label(),
                stmt.actor.label()
            ));
        }
        if OWNER_KINDS.contains(&kind) && stmt.actor != Actor::Owner {
            return Err(format!("only the owner signs {kind}"));
        }
        if HUB_KINDS.contains(&kind) && !matches!(stmt.actor, Actor::Hub | Actor::Owner) {
            return Err(format!("only the hub signs {kind}"));
        }
        // A review is the hub's own record: the owner's key (even through the console) does not sign one.
        if kind == "review.outcome" && (stmt.actor != Actor::Hub || key.holder.actor != Actor::Hub)
        {
            return Err("only the hub's key signs review.outcome".into());
        }
        let summary = match kind {
            "cert.issue" => {
                let c: CertIssue = parse(&stmt.body, kind)?;
                let allowed = stmt.actor == Actor::Owner
                    || (stmt.actor == Actor::Hub
                        && key.can("certify-agents")
                        && matches!(c.holder.actor, Actor::Agent { .. } | Actor::Node { .. })
                        && c.caps.is_empty());
                if !allowed {
                    return Err("only the owner certifies this key".into());
                }
                if self.keys.contains_key(&c.key.id()) {
                    return Err(format!("key {} is already certified", c.key.id()));
                }
                let s = format!(
                    "certified {} for {}",
                    &c.key.id()[..8],
                    c.holder.actor.label()
                );
                if !dry {
                    self.keys.insert(
                        c.key.id(),
                        KeyInfo {
                            id: c.key.id(),
                            key: c.key,
                            holder: c.holder,
                            caps: c.caps,
                            issued_seq: seq,
                            issuer: key.id.clone(),
                            revoked_seq: None,
                            revoked_reason: None,
                        },
                    );
                }
                s
            }
            "cert.revoke" => {
                let c: CertRevoke = parse(&stmt.body, kind)?;
                let target = self
                    .keys
                    .get(&c.key)
                    .ok_or_else(|| format!("unknown key {}", c.key))?;
                let allowed = stmt.actor == Actor::Owner
                    || (key.can("certify-agents")
                        && matches!(
                            target.holder.actor,
                            Actor::Agent { .. } | Actor::Node { .. }
                        ));
                if !allowed {
                    return Err("only the owner revokes this key".into());
                }
                if Some(target.key) == self.owner {
                    return Err("the owner's root key is rotated, not revoked".into());
                }
                let s = format!("revoked {} ({})", &c.key[..8.min(c.key.len())], c.reason);
                if !dry {
                    let k = self.keys.get_mut(&c.key).unwrap();
                    k.revoked_seq = Some(seq);
                    k.revoked_reason = Some(c.reason);
                }
                s
            }
            "repo.create" => {
                let r: RepoCreate = parse(&stmt.body, kind)?;
                if r.repo.is_empty()
                    || !r
                        .repo
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                {
                    return Err(format!("bad repo name {:?}", r.repo));
                }
                if self.repos.contains_key(&r.repo) {
                    return Err(format!("repo {} exists", r.repo));
                }
                if !dry {
                    self.repos.insert(
                        r.repo.clone(),
                        Repo {
                            name: r.repo.clone(),
                            project: r.project.clone(),
                            description: r.description.clone(),
                            created_seq: seq,
                            ..Default::default()
                        },
                    );
                }
                format!("repo {} created", r.repo)
            }
            "ref.create" => {
                let r: RefCreate = parse(&stmt.body, kind)?;
                if r.reference.is_empty()
                    || !r
                        .reference
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                {
                    return Err(format!("bad ref name {:?}", r.reference));
                }
                if r.at != "root" && !self.revisions.contains_key(&r.at) {
                    return Err(format!("no revision {}", r.at));
                }
                if self.repo(&r.repo)?.refs.contains_key(&r.reference) {
                    return Err(format!("ref {} exists in {}", r.reference, r.repo));
                }
                if !dry && let Some(repo) = self.repos.get_mut(&r.repo) {
                    let landed = if r.at == "root" {
                        Vec::new()
                    } else {
                        vec![r.at.clone()]
                    };
                    repo.refs.insert(r.reference.clone(), landed);
                }
                format!("ref {} created at {} in {}", r.reference, r.at, r.repo)
            }
            "lane.open" => {
                let l: LaneOpen = parse(&stmt.body, kind)?;
                let id = parse_change(&l.change)?;
                if id != self.last_change + 1 {
                    return Err(format!(
                        "the next change is w-{}, not {}",
                        self.last_change + 1,
                        l.change
                    ));
                }
                self.repo(&l.repo)?;
                if l.base != "root" && !self.revisions.contains_key(&l.base) {
                    return Err(format!("no revision {}", l.base));
                }
                let stacked = match &l.stacked_on {
                    Some(s) => Some(self.change(s)?.id),
                    None => None,
                };
                if l.kind == "owner-edit" && l.author != Actor::Owner {
                    return Err("an owner edit is authored by the owner".into());
                }
                let reverts = match &l.reverts {
                    Some(r) => Some(self.change(r)?.id),
                    None => None,
                };
                let s = format!(
                    "{} opened in {} on {} ({})",
                    l.change, l.repo, l.base, l.title
                );
                if !dry {
                    self.last_change = id;
                    self.changes.insert(
                        id,
                        Change {
                            id,
                            name: l.change.clone(),
                            repo: l.repo.clone(),
                            reference: l.reference.clone(),
                            task: l.task.clone(),
                            title: l.title.clone(),
                            author: l.author.clone(),
                            base: l.base.clone(),
                            stacked_on: stacked,
                            kind: l.kind.clone(),
                            reverts,
                            drafts: vec![],
                            rounds: vec![],
                            intents: BTreeMap::new(),
                            records: vec![],
                            state: ChangeState::Open,
                            landed: None,
                            conflict: None,
                            opened_seq: seq,
                            opened_ms: stmt.at_ms,
                            import: None,
                            exported: BTreeMap::new(),
                            reviews: vec![],
                        },
                    );
                }
                s
            }
            "revision.draft" | "revision.seal" => {
                let (change, n, deltas, tool_calls, turn, model, sealed) =
                    if kind == "revision.draft" {
                        let d: Draft = parse(&stmt.body, kind)?;
                        (
                            d.change,
                            d.n,
                            d.deltas,
                            d.tool_calls,
                            d.turn,
                            d.model,
                            false,
                        )
                    } else {
                        let s: Seal = parse(&stmt.body, kind)?;
                        (s.change, s.n, s.deltas, s.tool_calls, None, s.model, true)
                    };
                check_deltas(&deltas)?;
                let c = self.change(&change)?;
                if stmt.actor != c.author && stmt.actor != Actor::Hub {
                    return Err(format!("only {} writes {}", c.author.label(), change));
                }
                if !matches!(
                    c.state,
                    ChangeState::Open | ChangeState::Sealed | ChangeState::Conflict
                ) {
                    return Err(format!(
                        "{change} is {:?}; it takes no new revisions",
                        c.state
                    ));
                }
                let expected = if sealed {
                    c.rounds.len() as u32 + 1
                } else {
                    c.drafts.len() as u32 + 1
                };
                if n != expected {
                    return Err(format!(
                        "{change}: the next {} is {expected}, not {n}",
                        if sealed { "round" } else { "draft" }
                    ));
                }
                let base = c.base.clone();
                let id = if sealed {
                    format!("{change}.r{n}")
                } else {
                    format!("{change}.d{n}")
                };
                let dg = digest(&base, &deltas);
                let s = format!("{id}: {} files", deltas.len());
                if !dry {
                    let cid = c.id;
                    self.revisions.insert(
                        id.clone(),
                        Revision {
                            id: id.clone(),
                            change: cid,
                            kind: if sealed {
                                RevKind::Sealed
                            } else {
                                RevKind::Draft
                            },
                            n,
                            base,
                            deltas,
                            digest: dg,
                            tool_calls,
                            turn,
                            model,
                            actor: stmt.actor.clone(),
                            seq,
                            at_ms: stmt.at_ms,
                        },
                    );
                    let c = self.changes.get_mut(&cid).unwrap();
                    if sealed {
                        c.rounds.push(id);
                        c.state = ChangeState::Sealed;
                        c.conflict = None;
                    } else {
                        c.drafts.push(id);
                    }
                }
                s
            }
            "intent.set" => {
                let i: IntentSet = parse(&stmt.body, kind)?;
                let c = self.change(&i.change)?;
                if stmt.actor != c.author && !matches!(stmt.actor, Actor::Hub | Actor::Owner) {
                    return Err(format!(
                        "only {} or the hub names {}'s intents",
                        c.author.label(),
                        i.change
                    ));
                }
                if i.n == 0 || i.n > c.rounds.len() as u32 + 1 {
                    return Err(format!("{} has no round {}", i.change, i.n));
                }
                if i.intents.iter().any(|x| x.title.trim().is_empty()) {
                    return Err("an intent needs a title".into());
                }
                let s = format!("{} round {}: {} intents", i.change, i.n, i.intents.len());
                if !dry {
                    self.change_mut(&i.change)?.intents.insert(i.n, i.intents);
                }
                s
            }
            "attest" => {
                let a: Attest = parse(&stmt.body, kind)?;
                let rev = self
                    .revisions
                    .get(&a.revision)
                    .ok_or_else(|| format!("no revision {}", a.revision))?;
                if rev.digest != a.digest {
                    return Err(format!(
                        "{} does not have digest {}",
                        a.revision,
                        a.digest.short()
                    ));
                }
                let author = self.changes[&rev.change].author.clone();
                match a.predicate_type.as_str() {
                    "written" if stmt.actor != author => {
                        return Err("only the author signs written".into());
                    }
                    "review" if stmt.actor == author => {
                        return Err("an author never reviews their own change".into());
                    }
                    "review" if !matches!(stmt.actor, Actor::Agent { .. }) => {
                        return Err("a review is signed by a reviewer agent".into());
                    }
                    "ruling" if stmt.actor != Actor::Owner => {
                        return Err("only the owner rules".into());
                    }
                    "gate" | "checked" | "queued" | "landed" | "built"
                        if stmt.actor != Actor::Hub =>
                    {
                        return Err(format!("only the hub signs {}", a.predicate_type));
                    }
                    _ => {}
                }
                let s = format!(
                    "{} on {} by {}",
                    a.predicate_type,
                    a.revision,
                    stmt.actor.label()
                );
                if !dry {
                    let cid = rev.change;
                    self.records.insert(
                        seq,
                        RecordInfo {
                            seq,
                            at_ms: stmt.at_ms,
                            actor: stmt.actor.clone(),
                            key_id: key.id.clone(),
                            via_console,
                            attest: a,
                        },
                    );
                    self.changes.get_mut(&cid).unwrap().records.push(seq);
                }
                s
            }
            "queue.add" => {
                let q: QueueMove = parse(&stmt.body, kind)?;
                let c = self.change(&q.change)?;
                if c.state != ChangeState::Sealed {
                    return Err(format!(
                        "{} is {:?}; only a sealed change joins a queue",
                        q.change, c.state
                    ));
                }
                if q.n != c.round() {
                    return Err(format!(
                        "{} is at round {}, not {}",
                        q.change,
                        c.round(),
                        q.n
                    ));
                }
                let repo = self.repo(&q.repo)?;
                if c.repo != q.repo || c.reference != q.reference {
                    return Err(format!(
                        "{} belongs to {}:{}",
                        q.change, c.repo, c.reference
                    ));
                }
                if repo
                    .queues
                    .get(&q.reference)
                    .is_some_and(|v| v.contains(&c.id))
                {
                    return Err(format!("{} is already queued", q.change));
                }
                let s = format!("{} queued on {}:{}", q.change, q.repo, q.reference);
                if !dry {
                    let cid = c.id;
                    self.repo_mut(&q.repo)?
                        .queues
                        .entry(q.reference)
                        .or_default()
                        .push(cid);
                    self.changes.get_mut(&cid).unwrap().state = ChangeState::Queued;
                }
                s
            }
            "queue.remove" => {
                let q: QueueMove = parse(&stmt.body, kind)?;
                let c = self.change(&q.change)?;
                let cid = c.id;
                let repo = self.repo(&q.repo)?;
                if !repo
                    .queues
                    .get(&q.reference)
                    .is_some_and(|v| v.contains(&cid))
                {
                    return Err(format!(
                        "{} is not queued on {}:{}",
                        q.change, q.repo, q.reference
                    ));
                }
                let s = format!("{} left the queue ({})", q.change, q.reason);
                if !dry {
                    self.repo_mut(&q.repo)?
                        .queues
                        .get_mut(&q.reference)
                        .unwrap()
                        .retain(|x| *x != cid);
                    let c = self.changes.get_mut(&cid).unwrap();
                    if c.state == ChangeState::Queued {
                        c.state = ChangeState::Sealed;
                    }
                }
                s
            }
            "conflict" => {
                let x: Conflict = parse(&stmt.body, kind)?;
                let c = self.change(&x.change)?;
                let cid = c.id;
                let (repo, reference) = (c.repo.clone(), c.reference.clone());
                let s = format!("{} conflicts on {} files", x.change, x.files.len());
                if !dry {
                    if let Some(q) = self.repo_mut(&repo)?.queues.get_mut(&reference) {
                        q.retain(|v| *v != cid);
                    }
                    let c = self.changes.get_mut(&cid).unwrap();
                    c.state = ChangeState::Conflict;
                    c.conflict = Some(x);
                }
                s
            }
            "land" => {
                let l: Land = parse(&stmt.body, kind)?;
                check_deltas(&l.deltas)?;
                let c = self.change(&l.change)?;
                let cid = c.id;
                let repo = self.repo(&l.repo)?;
                if repo.head(&l.reference) != l.expected {
                    return Err(format!(
                        "{}:{} moved: its head is {}, not {}",
                        l.repo,
                        l.reference,
                        repo.head(&l.reference),
                        l.expected
                    ));
                }
                if !matches!(c.state, ChangeState::Queued | ChangeState::Sealed) {
                    return Err(format!("{} is {:?}; it cannot land", l.change, c.state));
                }
                if c.kind != "owner-edit"
                    && !repo
                        .queues
                        .get(&l.reference)
                        .is_some_and(|q| q.first() == Some(&cid) || q.contains(&cid))
                {
                    return Err(format!("{} is not in the queue", l.change));
                }
                let id = format!("{}.l", l.change);
                let dg = digest(&l.expected, &l.deltas);
                let s = format!("{} landed on {}:{}", l.change, l.repo, l.reference);
                if !dry {
                    self.revisions.insert(
                        id.clone(),
                        Revision {
                            id: id.clone(),
                            change: cid,
                            kind: RevKind::Landed,
                            n: l.n,
                            base: l.expected.clone(),
                            deltas: l.deltas,
                            digest: dg,
                            tool_calls: vec![],
                            turn: None,
                            model: None,
                            actor: Actor::Hub,
                            seq,
                            at_ms: stmt.at_ms,
                        },
                    );
                    let r = self.repo_mut(&l.repo)?;
                    r.refs
                        .entry(l.reference.clone())
                        .or_default()
                        .push(id.clone());
                    if let Some(q) = r.queues.get_mut(&l.reference) {
                        q.retain(|v| *v != cid);
                    }
                    let c = self.changes.get_mut(&cid).unwrap();
                    c.state = ChangeState::Landed;
                    c.landed = Some(id);
                }
                s
            }
            "import" => {
                let i: Import = parse(&stmt.body, kind)?;
                check_deltas(&i.deltas)?;
                let id = parse_change(&i.change)?;
                if id != self.last_change + 1 {
                    return Err(format!(
                        "the next change is w-{}, not {}",
                        self.last_change + 1,
                        i.change
                    ));
                }
                let repo = self.repo(&i.repo)?;
                if repo.head(&i.reference) != i.expected {
                    return Err(format!(
                        "{}:{} moved during the import",
                        i.repo, i.reference
                    ));
                }
                let rid = format!("{}.l", i.change);
                let dg = digest(&i.expected, &i.deltas);
                let title = i.message.lines().next().unwrap_or("").to_string();
                let s = format!(
                    "{} imported from git {}",
                    i.change,
                    &i.git[..12.min(i.git.len())]
                );
                if !dry {
                    self.last_change = id;
                    self.revisions.insert(
                        rid.clone(),
                        Revision {
                            id: rid.clone(),
                            change: id,
                            kind: RevKind::Imported,
                            n: 0,
                            base: i.expected.clone(),
                            deltas: i.deltas,
                            digest: dg,
                            tool_calls: vec![],
                            turn: None,
                            model: None,
                            actor: Actor::Hub,
                            seq,
                            at_ms: i.at_ms,
                        },
                    );
                    self.repo_mut(&i.repo)?
                        .refs
                        .entry(i.reference.clone())
                        .or_default()
                        .push(rid.clone());
                    self.changes.insert(
                        id,
                        Change {
                            id,
                            name: i.change.clone(),
                            repo: i.repo.clone(),
                            reference: i.reference.clone(),
                            task: None,
                            title,
                            author: Actor::Owner,
                            base: i.expected.clone(),
                            stacked_on: None,
                            kind: "imported".into(),
                            reverts: None,
                            drafts: vec![],
                            rounds: vec![],
                            intents: BTreeMap::new(),
                            records: vec![],
                            state: ChangeState::Landed,
                            landed: Some(rid),
                            conflict: None,
                            opened_seq: seq,
                            opened_ms: i.at_ms,
                            import: Some(ImportInfo {
                                git: i.git,
                                author_name: i.author_name,
                                author_email: i.author_email,
                                at_ms: i.at_ms,
                                message: i.message,
                                ai_coauthors: i.ai_coauthors,
                            }),
                            exported: BTreeMap::new(),
                            reviews: vec![],
                        },
                    );
                }
                s
            }
            "lane.rebase" => {
                let r: LaneRebase = parse(&stmt.body, kind)?;
                let c = self.change(&r.change)?;
                if matches!(c.state, ChangeState::Landed | ChangeState::Abandoned) {
                    return Err(format!("{} is {:?}; it cannot move", r.change, c.state));
                }
                if r.onto != "root" && !self.revisions.contains_key(&r.onto) {
                    return Err(format!("no revision {}", r.onto));
                }
                let s = format!("{} now starts from {} ({})", r.change, r.onto, r.reason);
                if !dry {
                    let c = self.change_mut(&r.change)?;
                    c.base = r.onto;
                    if c.state == ChangeState::Conflict {
                        c.state = ChangeState::Open;
                    }
                }
                s
            }
            "review.outcome" => {
                let r: ReviewOutcome = parse(&stmt.body, kind)?;
                r.check()?;
                let cid = self.change(&r.change)?.id;
                // A carried review names the revision of its source; this repository does not hold it.
                if r.origin.is_none() && !self.revisions.contains_key(&r.revision) {
                    return Err(format!("no revision {}", r.revision));
                }
                self.check_origin(&r)?;
                let confirmed = r
                    .findings
                    .iter()
                    .filter(|f| f.verdict == "confirmed")
                    .count();
                let s = format!(
                    "{} review {}: {} ({confirmed} confirmed, {} dropped)",
                    r.change,
                    r.run,
                    r.decision,
                    r.findings.len() - confirmed
                );
                if !dry {
                    self.changes
                        .get_mut(&cid)
                        .unwrap()
                        .reviews
                        .push(ReviewRecord {
                            seq,
                            at_ms: stmt.at_ms,
                            key: key.id.clone(),
                            outcome: r,
                        });
                }
                s
            }
            "lane.author" => {
                let a: LaneAuthor = parse(&stmt.body, kind)?;
                let c = self.change(&a.change)?;
                if matches!(c.state, ChangeState::Landed | ChangeState::Abandoned) {
                    return Err(format!(
                        "{} is {:?}; it takes no new author",
                        a.change, c.state
                    ));
                }
                if !matches!(a.author, Actor::Agent { .. }) {
                    return Err(format!(
                        "a lane is written by an agent, not {}",
                        a.author.label()
                    ));
                }
                let s = format!(
                    "{} is now written by {} ({})",
                    a.change,
                    a.author.label(),
                    a.reason
                );
                if !dry {
                    self.change_mut(&a.change)?.author = a.author;
                }
                s
            }
            "abandon" => {
                let a: Abandon = parse(&stmt.body, kind)?;
                let c = self.change(&a.change)?;
                if c.state == ChangeState::Landed {
                    return Err(format!("{} has landed; revert it instead", a.change));
                }
                let (cid, repo, reference) = (c.id, c.repo.clone(), c.reference.clone());
                let s = format!("{} abandoned ({})", a.change, a.reason);
                if !dry {
                    if let Some(q) = self.repo_mut(&repo)?.queues.get_mut(&reference) {
                        q.retain(|v| *v != cid);
                    }
                    self.changes.get_mut(&cid).unwrap().state = ChangeState::Abandoned;
                }
                s
            }
            "undo" => {
                let u: Undo = parse(&stmt.body, kind)?;
                let target = self
                    .ops
                    .iter()
                    .find(|o| o.seq == u.seq)
                    .ok_or_else(|| format!("no op {}", u.seq))?;
                if target.undone_by.is_some() {
                    return Err(format!("op {} is already undone", u.seq));
                }
                // Undo only what can be taken back without rewriting history: queue entries, intents,
                // path rules, connections and packs. A landed change is reverted by a new change.
                let undoable = [
                    "queue.add",
                    "intent.set",
                    "path.rule",
                    "connection.add",
                    "pack.add",
                    "abandon",
                ];
                if !undoable.contains(&target.kind.as_str()) {
                    return Err(format!(
                        "{} cannot be undone; {}",
                        target.kind,
                        if target.kind == "land" {
                            "revert the change instead"
                        } else {
                            "it is part of history"
                        }
                    ));
                }
                let s = format!("undid op {} ({})", u.seq, target.kind);
                if !dry {
                    self.undo(u.seq, seq)?;
                }
                s
            }
            "path.rule" => {
                let r: PathRule = parse(&stmt.body, kind)?;
                self.repo(&r.repo)?;
                let s = format!("rule on {}:{}", r.repo, r.glob);
                if !dry {
                    let repo = self.repo_mut(&r.repo)?;
                    repo.rules.retain(|x| x.glob != r.glob);
                    repo.rules.push(r);
                }
                s
            }
            "connection.add" => {
                let c: Connection = parse(&stmt.body, kind)?;
                let repo = self.repo(&c.repo)?;
                if repo.connections.contains_key(&c.id) {
                    return Err(format!("{} already has a connection {}", c.repo, c.id));
                }
                if c.author_email.is_empty() {
                    return Err("a connection needs the author's email".into());
                }
                let s = format!("{} connected to {}", c.repo, c.remote);
                if !dry {
                    let id = c.id.clone();
                    self.repo_mut(&c.repo)?.connections.insert(
                        id,
                        ConnState {
                            conn: c,
                            added_seq: seq,
                            exported: vec![],
                            head: None,
                            state: "in-step".into(),
                            detail: String::new(),
                            adopting: None,
                        },
                    );
                }
                s
            }
            "connection.remove" => {
                let c: ConnectionRemove = parse(&stmt.body, kind)?;
                if !self.repo(&c.repo)?.connections.contains_key(&c.id) {
                    return Err(format!("no connection {}", c.id));
                }
                if !dry {
                    self.repo_mut(&c.repo)?.connections.remove(&c.id);
                }
                format!("{} disconnected {}", c.repo, c.id)
            }
            "connection.state" => {
                let c: ConnectionState = parse(&stmt.body, kind)?;
                if !self.repo(&c.repo)?.connections.contains_key(&c.id) {
                    return Err(format!("no connection {}", c.id));
                }
                let s = format!("{} {}: {}", c.repo, c.id, c.state);
                if !dry {
                    let conn = self.repo_mut(&c.repo)?.connections.get_mut(&c.id).unwrap();
                    if c.state == "adopting" {
                        conn.adopting = c.remote_head.clone().zip(c.adopt_change.clone());
                    } else if c.state == "in-step" {
                        conn.adopting = None;
                        if c.remote_head.is_some() {
                            conn.head = c.remote_head;
                        }
                    }
                    conn.state = c.state;
                    conn.detail = c.detail;
                }
                s
            }
            "export" => {
                let e: Export = parse(&stmt.body, kind)?;
                let c = self.change(&e.change)?;
                if c.state != ChangeState::Landed {
                    return Err(format!("{} has not landed", e.change));
                }
                let cid = c.id;
                if !self.repo(&e.repo)?.connections.contains_key(&e.connection) {
                    return Err(format!("no connection {}", e.connection));
                }
                let s = format!(
                    "{} exported to {} as {} commits",
                    e.change,
                    e.connection,
                    e.commits.len()
                );
                if !dry {
                    let conn = self
                        .repo_mut(&e.repo)?
                        .connections
                        .get_mut(&e.connection)
                        .unwrap();
                    conn.exported.push((cid, e.head.clone()));
                    conn.head = Some(e.head.clone());
                    if conn
                        .adopting
                        .as_ref()
                        .is_some_and(|(_, adopt)| *adopt == e.change)
                    {
                        conn.adopting = None;
                    }
                    if conn.adopting.is_none() {
                        conn.state = "in-step".into();
                    }
                    self.changes
                        .get_mut(&cid)
                        .unwrap()
                        .exported
                        .insert(e.connection, e.commits);
                }
                s
            }
            "artifact.put" => {
                let a: ArtifactPut = parse(&stmt.body, kind)?;
                self.repo(&a.repo)?;
                if self.artifacts.contains_key(&a.id) {
                    return Err(format!("artifact {} exists", a.id));
                }
                if a.built_from != "root" && !self.revisions.contains_key(&a.built_from) {
                    return Err(format!("no revision {}", a.built_from));
                }
                let s = format!("artifact {} ({} {})", a.id, a.name, a.target);
                if !dry {
                    self.artifacts.insert(
                        a.id.clone(),
                        ArtifactInfo {
                            artifact: a,
                            seq,
                            at_ms: stmt.at_ms,
                        },
                    );
                }
                s
            }
            "release.propose" => {
                let r: ReleasePropose = parse(&stmt.body, kind)?;
                let repo = self.repo(&r.repo)?;
                if repo.releases.contains_key(&r.version) {
                    return Err(format!("{} {} exists", r.repo, r.version));
                }
                let rev = self
                    .revisions
                    .get(&r.from)
                    .ok_or_else(|| format!("no revision {}", r.from))?;
                if !matches!(rev.kind, RevKind::Landed | RevKind::Imported) {
                    return Err("a release is cut from landed work".into());
                }
                let s = format!("release {} {} proposed from {}", r.repo, r.version, r.from);
                if !dry {
                    let v = r.version.clone();
                    self.repo_mut(&r.repo)?.releases.insert(
                        v,
                        Release {
                            propose: r,
                            proposed_seq: seq,
                            signed: None,
                            exports: vec![],
                            notes_edited: false,
                            life: ReleaseLife {
                                state: "draft".into(),
                                ..ReleaseLife::default()
                            },
                        },
                    );
                }
                s
            }
            "release.sign" => {
                let r: ReleaseSign = parse(&stmt.body, kind)?;
                let rel = self
                    .repo(&r.repo)?
                    .releases
                    .get(&r.version)
                    .ok_or_else(|| format!("no release {}", r.version))?;
                if rel.signed.is_some() {
                    return Err(format!("{} is already signed", r.version));
                }
                for a in &r.artifacts {
                    if !self.artifacts.contains_key(a) {
                        return Err(format!("no artifact {a}"));
                    }
                }
                let s = format!(
                    "release {} {} signed{}",
                    r.repo,
                    r.version,
                    if via_console {
                        " (from the console)"
                    } else {
                        ""
                    }
                );
                if !dry {
                    let (repo, v) = (r.repo.clone(), r.version.clone());
                    let kid = key.id.clone();
                    self.repo_mut(&repo)?.releases.get_mut(&v).unwrap().signed =
                        Some((r, seq, kid));
                    // A signed release moves its release line.
                    let rel = &self.repos[&repo].releases[&v];
                    let line = rel
                        .propose
                        .line
                        .clone()
                        .unwrap_or_else(|| format!("release/{}", line_of(&v)));
                    let from = rel.propose.from.clone();
                    let refs = &mut self.repo_mut(&repo)?.refs;
                    let entry = refs.entry(line).or_default();
                    if entry.last() != Some(&from) {
                        entry.push(from);
                    }
                }
                s
            }
            "release.candidate" => {
                let c: ReleaseCandidate = parse(&stmt.body, kind)?;
                let rel = self.release(&c.repo, &c.version)?;
                if rel.life.state != "draft" {
                    return Err(format!(
                        "{} is {}; a candidate is cut from a draft",
                        c.version, rel.life.state
                    ));
                }
                let targets = rel.propose.targets.clone();
                for t in &targets {
                    let built = c.artifacts.iter().any(|a| {
                        self.artifacts
                            .get(a)
                            .is_some_and(|i| i.artifact.repo == c.repo && &i.artifact.target == t)
                    });
                    let skipped = c.not_run.iter().any(|n| &n.target == t);
                    if !built && !skipped {
                        return Err(format!("{} has no artifact for target {t}", c.version));
                    }
                }
                if let Some(n) = c.not_run.iter().find(|n| !targets.contains(&n.target)) {
                    return Err(format!(
                        "{} names no declared target {}",
                        c.version, n.target
                    ));
                }
                if c.artifacts.is_empty() {
                    return Err("a candidate holds at least one artifact".into());
                }
                if crate::release::release_digest(self, &c.repo, &c.version, &c.artifacts)?
                    != c.digest
                {
                    return Err(format!(
                        "{} candidate digest is not its artifacts'",
                        c.version
                    ));
                }
                let s = format!(
                    "{} {} candidate ({} artifacts){}",
                    c.repo,
                    c.version,
                    c.artifacts.len(),
                    c.number.map(|n| format!(" rc.{n}")).unwrap_or_default()
                );
                if !dry {
                    let life = &mut self.release_mut(&c.repo, &c.version)?.life;
                    life.state = "candidate".into();
                    life.candidate = Some((c, seq));
                }
                s
            }
            "release.qualify" => {
                let m: ReleaseMove = parse(&stmt.body, kind)?;
                let state = self.release(&m.repo, &m.version)?.life.state.clone();
                if state != "candidate" {
                    return Err(format!(
                        "{} is {state}; only a candidate is qualified",
                        m.version
                    ));
                }
                let s = format!("{} {} qualifying", m.repo, m.version);
                if !dry {
                    self.release_mut(&m.repo, &m.version)?.life.state = "qualifying".into();
                }
                s
            }
            "release.verdict" => {
                let v: ReleaseVerdict = parse(&stmt.body, kind)?;
                if !matches!(v.source.as_str(), "ci" | "qualification") {
                    return Err(format!(
                        "a verdict comes from ci or qualification, not {}",
                        v.source
                    ));
                }
                let state = self.release(&v.repo, &v.version)?.life.state.clone();
                let next = match (v.verdict.as_str(), v.source.as_str(), state.as_str()) {
                    ("failed", _, "candidate" | "qualifying") => "failed",
                    ("abandoned", _, "draft" | "candidate" | "qualifying") => "abandoned",
                    ("passed", "ci", "candidate") => "candidate",
                    ("passed", "ci", "qualifying") => "qualifying",
                    ("passed", "qualification", "qualifying") => "qualified",
                    _ => {
                        return Err(format!(
                            "a {} verdict from {} does not apply to {} ({state})",
                            v.verdict, v.source, v.version
                        ));
                    }
                };
                let s = format!(
                    "{} {} {} from {} ({next})",
                    v.repo, v.version, v.verdict, v.source
                );
                if !dry {
                    let life = &mut self.release_mut(&v.repo, &v.version)?.life;
                    life.verdicts.push(Verdict {
                        source: v.source,
                        verdict: v.verdict,
                        reason: v.reason,
                        seq,
                    });
                    life.state = next.into();
                }
                s
            }
            "release.publish" => {
                let m: ReleaseMove = parse(&stmt.body, kind)?;
                let rel = self.release(&m.repo, &m.version)?;
                let ci_passed = rel
                    .life
                    .verdicts
                    .iter()
                    .rev()
                    .find(|v| v.source == "ci")
                    .is_some_and(|v| v.verdict == "passed");
                let ready =
                    rel.life.state == "qualified" || (rel.life.state == "candidate" && ci_passed);
                if !ready {
                    return Err(format!(
                        "{} is {} with no passing checks or qualification; it cannot be published",
                        m.version, rel.life.state
                    ));
                }
                // The line's older published releases are superseded by this one.
                let line = rel
                    .propose
                    .line
                    .clone()
                    .unwrap_or_else(|| format!("release/{}", line_of(&m.version)));
                let proposed = rel.proposed_seq;
                let s = format!("{} {} published", m.repo, m.version);
                if !dry {
                    let repo = self.repo_mut(&m.repo)?;
                    for (v, other) in repo.releases.iter_mut() {
                        let same_line = other
                            .propose
                            .line
                            .clone()
                            .unwrap_or_else(|| format!("release/{}", line_of(v)))
                            == line;
                        if v != &m.version
                            && same_line
                            && other.proposed_seq < proposed
                            && matches!(other.life.state.as_str(), "published" | "promoted")
                        {
                            other.life.state = "superseded".into();
                        }
                    }
                    let life = &mut repo.releases.get_mut(&m.version).expect("checked").life;
                    life.state = "published".into();
                    life.published = Some(seq);
                }
                s
            }
            "release.promote" => {
                let m: ReleaseMove = parse(&stmt.body, kind)?;
                let environment = m
                    .environment
                    .clone()
                    .filter(|e| !e.trim().is_empty())
                    .ok_or("a promotion names its environment or store track")?;
                let state = self.release(&m.repo, &m.version)?.life.state.clone();
                if !matches!(state.as_str(), "published" | "promoted") {
                    return Err(format!(
                        "{} is {state}; only a published release is promoted",
                        m.version
                    ));
                }
                let s = format!("{} {} promoted to {environment}", m.repo, m.version);
                if !dry {
                    let life = &mut self.release_mut(&m.repo, &m.version)?.life;
                    life.state = "promoted".into();
                    life.promotions.push((environment, seq));
                }
                s
            }
            "release.yank" => {
                let m: ReleaseMove = parse(&stmt.body, kind)?;
                if m.reason.trim().is_empty() {
                    return Err("a yank says why".into());
                }
                let state = self.release(&m.repo, &m.version)?.life.state.clone();
                if !matches!(state.as_str(), "published" | "promoted" | "superseded") {
                    return Err(format!(
                        "{} is {state}; only a published release is yanked",
                        m.version
                    ));
                }
                let s = format!("{} {} yanked ({})", m.repo, m.version, m.reason);
                if !dry {
                    let life = &mut self.release_mut(&m.repo, &m.version)?.life;
                    life.state = "yanked".into();
                    life.yanked = Some((m.reason, seq));
                }
                s
            }
            "release.notes" => {
                let m: ReleaseNotes = parse(&stmt.body, kind)?;
                self.release(&m.repo, &m.version)?;
                let s = format!("{} {} notes edited", m.repo, m.version);
                if !dry {
                    let r = self.release_mut(&m.repo, &m.version)?;
                    r.propose.notes = m.notes;
                    r.notes_edited = true;
                }
                s
            }
            "release.lane" => {
                let c: ReleaseLaneCut = parse(&stmt.body, kind)?;
                let rel = self.release(&c.repo, &c.version)?;
                if !matches!(
                    rel.life.state.as_str(),
                    "published" | "promoted" | "superseded"
                ) {
                    return Err(format!(
                        "{} is {}; a lane is cut from a published release",
                        c.version, rel.life.state
                    ));
                }
                let line = rel
                    .propose
                    .line
                    .clone()
                    .unwrap_or_else(|| format!("release/{}", line_of(&c.version)));
                let from = rel.propose.from.clone();
                let chain = self
                    .repo(&c.repo)?
                    .refs
                    .get(&line)
                    .cloned()
                    .unwrap_or_default();
                // A lane is cut once: a cut again is a no-op while the line still starts at the release.
                if !chain.is_empty() && !chain.contains(&from) {
                    return Err(format!("{line} exists and does not start at {from}"));
                }
                let s = format!("{} {line} cut at {from} from {}", c.repo, c.version);
                if !dry && chain.is_empty() {
                    self.repo_mut(&c.repo)?.refs.insert(line, vec![from]);
                }
                s
            }
            "release.export" => {
                let r: ReleaseExport = parse(&stmt.body, kind)?;
                let rel = self
                    .repo(&r.repo)?
                    .releases
                    .get(&r.version)
                    .ok_or_else(|| format!("no release {}", r.version))?;
                if rel.signed.is_none() {
                    return Err("only a signed release is exported".into());
                }
                let s = format!(
                    "release {} {} exported to {}",
                    r.repo, r.version, r.connection
                );
                if !dry {
                    let (repo, v) = (r.repo.clone(), r.version.clone());
                    self.repo_mut(&repo)?
                        .releases
                        .get_mut(&v)
                        .unwrap()
                        .exports
                        .push(r);
                }
                s
            }
            "pack.add" => {
                let p: PackAdd = parse(&stmt.body, kind)?;
                if let Some(old) = self.packs.get(&p.name)
                    && old.pack.version >= p.version
                {
                    return Err(format!(
                        "pack {} is already at version {}",
                        p.name, old.pack.version
                    ));
                }
                let s = format!("pack {}@{}", p.name, p.version);
                if !dry {
                    self.packs.insert(
                        p.name.clone(),
                        PackInfo {
                            pack: p,
                            added_seq: seq,
                        },
                    );
                }
                s
            }
            "pack.remove" => {
                let p: PackRemove = parse(&stmt.body, kind)?;
                if !self.packs.contains_key(&p.name) {
                    return Err(format!("no pack {}", p.name));
                }
                if !dry {
                    self.packs.remove(&p.name);
                }
                format!("pack {} removed", p.name)
            }
            other => return Err(format!("unknown kind {other}")),
        };
        if !dry {
            self.op(seq, stmt, summary.clone());
        }
        Ok(summary)
    }

    fn op(&mut self, seq: u64, stmt: &Statement, summary: String) {
        let b = &stmt.body;
        let s = |k: &str| b[k].as_str().unwrap_or_default().to_string();
        let subject = match stmt.kind.as_str() {
            "queue.add" | "abandon" => Some(s("change")),
            "intent.set" => Some(format!("{}#{}", s("change"), b["n"].as_u64().unwrap_or(0))),
            "path.rule" => Some(format!("{}|{}", s("repo"), s("glob"))),
            "connection.add" => Some(format!("{}|{}", s("repo"), s("id"))),
            "pack.add" => Some(s("name")),
            _ => None,
        };
        self.ops.push(OpInfo {
            seq,
            kind: stmt.kind.clone(),
            actor: stmt.actor.clone(),
            at_ms: stmt.at_ms,
            summary,
            subject,
            undone_by: None,
        });
    }

    fn undo(&mut self, target: u64, by: u64) -> Result<(), String> {
        let op = self
            .ops
            .iter()
            .find(|o| o.seq == target)
            .cloned()
            .ok_or("no such op")?;
        let subject = op.subject.clone().unwrap_or_default();
        match op.kind.as_str() {
            "queue.add" => {
                if let Ok(c) = self.change(&subject) {
                    let (cid, repo, reference, queued) = (
                        c.id,
                        c.repo.clone(),
                        c.reference.clone(),
                        c.state == ChangeState::Queued,
                    );
                    if queued {
                        if let Some(q) = self
                            .repos
                            .get_mut(&repo)
                            .and_then(|r| r.queues.get_mut(&reference))
                        {
                            q.retain(|v| *v != cid);
                        }
                        self.changes.get_mut(&cid).unwrap().state = ChangeState::Sealed;
                    }
                }
            }
            "abandon" => {
                if let Ok(c) = self.change(&subject) {
                    let cid = c.id;
                    let c = self.changes.get_mut(&cid).unwrap();
                    if c.state == ChangeState::Abandoned {
                        c.state = if c.rounds.is_empty() {
                            ChangeState::Open
                        } else {
                            ChangeState::Sealed
                        };
                    }
                }
            }
            "intent.set" => {
                if let Some((change, n)) = subject.split_once('#')
                    && let (Ok(c), Ok(n)) = (self.change(change), n.parse::<u32>())
                {
                    let cid = c.id;
                    self.changes.get_mut(&cid).unwrap().intents.remove(&n);
                }
            }
            "path.rule" => {
                if let Some((repo, glob)) = subject.split_once('|')
                    && let Some(r) = self.repos.get_mut(repo)
                {
                    r.rules.retain(|x| x.glob != glob);
                }
            }
            "connection.add" => {
                if let Some((repo, id)) = subject.split_once('|')
                    && let Some(r) = self.repos.get_mut(repo)
                {
                    r.connections.remove(id);
                }
            }
            "pack.add" => {
                self.packs.remove(&subject);
            }
            _ => {}
        }
        if let Some(o) = self.ops.iter_mut().find(|o| o.seq == target) {
            o.undone_by = Some(by);
        }
        Ok(())
    }
}

/// `v0.9.0` → `0.9`.
pub fn line_of(version: &str) -> String {
    let v = version.trim_start_matches('v');
    let mut parts = v.split('.');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}.{b}"),
        (Some(a), None) => a.to_string(),
        _ => v.to_string(),
    }
}
