//! Contacts, directed opinions and global reputation (AP-071, AP-072).
//!
//! Three layers over agents' cids:
//!
//! - **Contacts** are directed edges `from -> to` with a kind, a decaying
//!   interaction log (the strength), a private note and a block flag.
//! - **Opinions** are directed edges holding decayed evidence counts `r`
//!   (positive) and `s` (negative), reported in subjective-logic form.
//! - **Reputation** is EigenTrust over the positive part of the opinions,
//!   seeded from the owner's opinions, else from the verified agents.
//!
//! The owner is [`OWNER`] (cid 0). Its opinions seed reputation and its edges
//! are never listed as contacts. Every mutation is persisted to the optional
//! file (atomic tmp+rename) before it returns.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// The owner's cid: its opinions seed reputation; its edges are not contacts.
pub const OWNER: u64 = 0;

const DAY_MS: i64 = 86_400_000;
/// Interaction strength half-life.
const STRENGTH_HALF_LIFE_DAYS: f64 = 14.0;
/// Opinion decay: lambda = 2^(-1/90) per day, i.e. a 90-day half-life.
const OPINION_HALF_LIFE_DAYS: f64 = 90.0;
const MAX_LOG: usize = 200;
const MAX_NOTE_CHARS: usize = 2000;
const RATING_WINDOW_MS: i64 = 30 * DAY_MS;
const RATING_WEIGHT: f64 = 0.25;
const OWNER_WEIGHT: f64 = 3.0;
/// A rating at or above this is praise for the ring rule.
const PRAISE: f64 = 0.8;
/// More than this many praises in each direction within the window is a ring.
const RING_PRAISES: usize = 3;
/// Met contacts weaker than this, with no opinion evidence, are not listed.
const STRENGTH_FLOOR: f64 = 0.1;
const EIGEN_DAMPING: f64 = 0.85;
const EIGEN_MAX_ITERS: usize = 100;
const EIGEN_EPSILON: f64 = 1e-6;

/// How two agents are connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Met,
    Hired,
    WasHired,
    Added,
}

/// An interaction between two agents; each carries a strength weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    Task,
    Credit,
    Message,
    Group,
    Add,
}

impl Interaction {
    fn weight(self) -> f64 {
        match self {
            Self::Task => 5.0,
            Self::Credit => 3.0,
            Self::Message => 1.0,
            Self::Group => 0.5,
            Self::Add => 2.0,
        }
    }
}

/// Why a bond operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BondError {
    SelfEdge,
    NoInteraction,
    AlreadyRated,
    NoContact,
    NoteTooLong,
    Persist(String),
}

impl fmt::Display for BondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge => f.write_str("an agent cannot relate to itself"),
            Self::NoInteraction => f.write_str("no interaction in the last 30 days"),
            Self::AlreadyRated => f.write_str("already rated since the last interaction"),
            Self::NoContact => f.write_str("no such contact"),
            Self::NoteTooLong => write!(f, "note exceeds {} characters", MAX_NOTE_CHARS),
            Self::Persist(e) => write!(f, "could not persist bonds: {}", e),
        }
    }
}

impl std::error::Error for BondError {}

/// A directed opinion decayed to some instant, in subjective-logic form.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Opinion {
    pub r: f64,
    pub s: f64,
    pub b: f64,
    pub d: f64,
    pub u: f64,
    pub e: f64,
    pub updated_ms: i64,
}

/// A contact as the plaza shows it, with its opinion at the same instant.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BondView {
    pub from: u64,
    pub to: u64,
    pub kind: Kind,
    pub since_ms: i64,
    pub strength: f64,
    pub interactions: u64,
    pub last_ms: i64,
    pub note: Option<String>,
    pub blocked: bool,
    pub opinion: Opinion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Contact {
    kind: Kind,
    since_ms: i64,
    last_ms: i64,
    #[serde(default)]
    count: u64,
    /// Most recent interactions, (at_ms, weight), at most `MAX_LOG`.
    #[serde(default)]
    log: Vec<(i64, f64)>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    blocked: bool,
}

impl Contact {
    fn new(kind: Kind, at: i64) -> Self {
        Self {
            kind,
            since_ms: at,
            last_ms: at,
            count: 0,
            log: Vec::new(),
            note: None,
            blocked: false,
        }
    }

    fn record(&mut self, what: Interaction, at: i64) {
        self.log.push((at, what.weight()));
        let excess = self.log.len().saturating_sub(MAX_LOG);
        self.log.drain(..excess);
        self.count += 1;
        self.last_ms = self.last_ms.max(at);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct OpinionEdge {
    r: f64,
    s: f64,
    updated_ms: i64,
    /// The contact's interaction count when this edge was last rated.
    rated_at: u64,
    /// Ratings given along this edge within the window: (at_ms, v).
    ratings: Vec<(i64, f64)>,
}

/// The on-disk form: edge lists, since JSON map keys must be strings.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Disk {
    contacts: Vec<(u64, u64, Contact)>,
    opinions: Vec<(u64, u64, OpinionEdge)>,
}

#[derive(Debug, Default)]
struct State {
    contacts: BTreeMap<(u64, u64), Contact>,
    opinions: BTreeMap<(u64, u64), OpinionEdge>,
}

/// The bond graph, behind a lock, optionally persisted to a JSON file.
#[derive(Debug)]
pub struct Bonds {
    state: Mutex<State>,
    path: Option<PathBuf>,
}

impl Bonds {
    /// Loads the graph from `path`. A missing or corrupt file gives an empty graph.
    pub fn load(path: Option<PathBuf>) -> Self {
        let state = path
            .as_deref()
            .and_then(|p| fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Disk>(&bytes).ok())
            .map(|disk| State {
                contacts: disk
                    .contacts
                    .into_iter()
                    .map(|(a, b, c)| ((a, b), c))
                    .collect(),
                opinions: disk
                    .opinions
                    .into_iter()
                    .map(|(a, b, o)| ((a, b), o))
                    .collect(),
            })
            .unwrap_or_default();
        Self {
            state: Mutex::new(state),
            path,
        }
    }

    /// Records an interaction between `a` and `b`, in both directions.
    pub fn interact(&self, a: u64, b: u64, what: Interaction, at: i64) -> Result<(), BondError> {
        if a == b {
            return Err(BondError::SelfEdge);
        }
        let mut st = self.lock();
        for (from, to) in [(a, b), (b, a)] {
            st.contacts
                .entry((from, to))
                .or_insert_with(|| Contact::new(Kind::Met, at))
                .record(what, at);
        }
        self.persist(&st)
    }

    /// `from` adds `to` as a contact: the edge becomes kind Added, and both
    /// directions record an Add interaction (the reverse edge stays Met).
    pub fn add(&self, from: u64, to: u64, at: i64) -> Result<(), BondError> {
        if from == to {
            return Err(BondError::SelfEdge);
        }
        let mut st = self.lock();
        let edge = st
            .contacts
            .entry((from, to))
            .or_insert_with(|| Contact::new(Kind::Added, at));
        edge.kind = Kind::Added;
        edge.record(Interaction::Add, at);
        st.contacts
            .entry((to, from))
            .or_insert_with(|| Contact::new(Kind::Met, at))
            .record(Interaction::Add, at);
        self.persist(&st)
    }

    /// Sets or clears the private note on an existing contact.
    pub fn set_note(&self, from: u64, to: u64, note: Option<String>) -> Result<(), BondError> {
        if from == to {
            return Err(BondError::SelfEdge);
        }
        if note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
            return Err(BondError::NoteTooLong);
        }
        let mut st = self.lock();
        st.contacts
            .get_mut(&(from, to))
            .ok_or(BondError::NoContact)?
            .note = note;
        self.persist(&st)
    }

    /// Blocks or unblocks an existing contact.
    pub fn block(&self, from: u64, to: u64, blocked: bool) -> Result<(), BondError> {
        if from == to {
            return Err(BondError::SelfEdge);
        }
        let mut st = self.lock();
        st.contacts
            .get_mut(&(from, to))
            .ok_or(BondError::NoContact)?
            .blocked = blocked;
        self.persist(&st)
    }

    /// Work or pay evidence about `to`, as seen by `from`: outcome `v` in
    /// [-1, 1] and `credits` paid. The value weight grows with credits.
    pub fn evidence(
        &self,
        from: u64,
        to: u64,
        v: f64,
        credits: f64,
        at: i64,
    ) -> Result<Opinion, BondError> {
        if from == to {
            return Err(BondError::SelfEdge);
        }
        let weight = 1.0 + (1.0 + credits.max(0.0) / 100.0).ln();
        self.bump_edge(from, to, weight, clamp_value(v), at)
    }

    /// `from`'s own rating of `to`, weight 0.25. Needs an interaction in the
    /// last 30 days, and at most one rating per interaction. Once `from` and
    /// `to` have praised each other more than three times each in the window,
    /// further ratings between them weigh 0.
    pub fn rate(&self, from: u64, to: u64, v: f64, at: i64) -> Result<Opinion, BondError> {
        if from == to {
            return Err(BondError::SelfEdge);
        }
        let v = clamp_value(v);
        let mut st = self.lock();
        let (count, last) = match st.contacts.get(&(from, to)) {
            Some(c) => (c.count, c.last_ms),
            None => return Err(BondError::NoInteraction),
        };
        if at - last > RATING_WINDOW_MS {
            return Err(BondError::NoInteraction);
        }
        let rated_at = st.opinions.get(&(from, to)).map_or(0, |o| o.rated_at);
        if count <= rated_at {
            return Err(BondError::AlreadyRated);
        }
        let weight = if ring_active(&st.opinions, from, to, at) {
            0.0
        } else {
            RATING_WEIGHT
        };
        let edge = st.opinions.entry((from, to)).or_default();
        edge.ratings.retain(|r| at - r.0 <= RATING_WINDOW_MS);
        edge.ratings.push((at, v));
        edge.rated_at = count;
        bump(edge, weight, v, at);
        let snapshot = view(Some(&*edge), at);
        self.persist(&st)?;
        Ok(snapshot)
    }

    /// The owner's rating of `to`: weight 3, no interaction needed.
    pub fn owner_rate(&self, to: u64, v: f64, at: i64) -> Result<Opinion, BondError> {
        if to == OWNER {
            return Err(BondError::SelfEdge);
        }
        self.bump_edge(OWNER, to, OWNER_WEIGHT, clamp_value(v), at)
    }

    /// `from`'s opinion of `to` at `now`. A missing edge is r=s=0, e=0.5, u=1.
    pub fn opinion(&self, from: u64, to: u64, now: i64) -> Opinion {
        let st = self.lock();
        view(st.opinions.get(&(from, to)), now)
    }

    /// Every contact except the owner's, with strength and opinion at `now`.
    /// Met contacts that are weak and carry no opinion evidence are dropped.
    pub fn bonds(&self, now: i64) -> Vec<BondView> {
        let st = self.lock();
        let mut out = Vec::new();
        for (&(from, to), contact) in &st.contacts {
            if from == OWNER || to == OWNER {
                continue;
            }
            let strength = strength_at(contact, now);
            let edge = st.opinions.get(&(from, to));
            let has_evidence = edge.is_some_and(|o| o.r + o.s > 0.0);
            if contact.kind == Kind::Met && strength < STRENGTH_FLOOR && !has_evidence {
                continue;
            }
            out.push(BondView {
                from,
                to,
                kind: contact.kind,
                since_ms: contact.since_ms,
                strength,
                interactions: contact.count,
                last_ms: contact.last_ms,
                note: contact.note.clone(),
                blocked: contact.blocked,
                opinion: view(edge, now),
            });
        }
        out
    }

    /// The bonds (see [`Bonds::bonds`]) whose contact is `from`'s.
    pub fn of(&self, from: u64, now: i64) -> Vec<BondView> {
        self.bonds(now)
            .into_iter()
            .filter(|b| b.from == from)
            .collect()
    }

    /// Pairs (low, high) that praise each other past the ring threshold.
    pub fn rings(&self, now: i64) -> Vec<(u64, u64)> {
        let st = self.lock();
        let mut pairs = BTreeSet::new();
        for &(a, b) in st.opinions.keys() {
            if a != b && ring_active(&st.opinions, a, b, now) {
                pairs.insert((a.min(b), a.max(b)));
            }
        }
        pairs.into_iter().collect()
    }

    /// EigenTrust reputation for `agents`, seeded from the owner's opinions,
    /// else uniformly over `verified`, else over `agents`. Returns, per agent,
    /// (reputation in 0..=100, confidence in 0..=1).
    pub fn reputation(
        &self,
        agents: &[u64],
        verified: &[u64],
        now: i64,
    ) -> BTreeMap<u64, (f64, f64)> {
        let st = self.lock();
        let mut out = BTreeMap::new();
        let ids: Vec<u64> = agents
            .iter()
            .copied()
            .collect::<BTreeSet<u64>>()
            .into_iter()
            .collect();
        let n = ids.len();
        if n == 0 {
            return out;
        }
        let index: BTreeMap<u64, usize> = ids
            .iter()
            .copied()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();

        // l[i][j]: how far i trusts j, the positive part of its decayed opinion.
        let mut l = vec![vec![0.0_f64; n]; n];
        let mut p = vec![0.0_f64; n];
        for (&(from, to), edge) in &st.opinions {
            let Some(&j) = index.get(&to) else { continue };
            let o = view(Some(edge), now);
            let trust = (o.r - o.s).max(0.0);
            if from == OWNER {
                p[j] += trust;
            }
            if let Some(&i) = index.get(&from) {
                l[i][j] = trust;
            }
        }

        // p: the owner's trust if it has any, else the verified, else everyone.
        let owner_mass: f64 = p.iter().sum();
        if owner_mass > 0.0 {
            for x in &mut p {
                *x /= owner_mass;
            }
        } else {
            let mut seeds: Vec<usize> = verified
                .iter()
                .filter_map(|v| index.get(v).copied())
                .collect();
            seeds.sort_unstable();
            seeds.dedup();
            if seeds.is_empty() {
                seeds = (0..n).collect();
            }
            let share = 1.0 / seeds.len() as f64;
            for j in seeds {
                p[j] = share;
            }
        }

        // t <- 0.85 C^T t + 0.15 p, where an empty row of C is p itself.
        let rows: Vec<f64> = l.iter().map(|row| row.iter().sum()).collect();
        let mut t = p.clone();
        for _ in 0..EIGEN_MAX_ITERS {
            let mut next = vec![0.0_f64; n];
            for i in 0..n {
                if rows[i] > 0.0 {
                    for j in 0..n {
                        next[j] += EIGEN_DAMPING * t[i] * l[i][j] / rows[i];
                    }
                } else {
                    for j in 0..n {
                        next[j] += EIGEN_DAMPING * t[i] * p[j];
                    }
                }
            }
            for j in 0..n {
                next[j] += (1.0 - EIGEN_DAMPING) * p[j];
            }
            let change: f64 = next.iter().zip(&t).map(|(a, b)| (a - b).abs()).sum();
            t = next;
            if change < EIGEN_EPSILON {
                break;
            }
        }

        let max = t.iter().copied().fold(0.0_f64, f64::max);
        for (&id, &i) in &index {
            let reputation = if max > 0.0 { 100.0 * t[i] / max } else { 0.0 };
            let mut us = Vec::new();
            for (&(_, to), edge) in &st.opinions {
                if to == id {
                    us.push(view(Some(edge), now).u);
                }
            }
            let mean_u = if us.is_empty() {
                1.0
            } else {
                us.iter().sum::<f64>() / us.len() as f64
            };
            out.insert(id, (reputation, 1.0 - mean_u));
        }
        out
    }

    fn bump_edge(
        &self,
        from: u64,
        to: u64,
        weight: f64,
        v: f64,
        at: i64,
    ) -> Result<Opinion, BondError> {
        let mut st = self.lock();
        let edge = st.opinions.entry((from, to)).or_default();
        bump(edge, weight, v, at);
        let snapshot = view(Some(&*edge), at);
        self.persist(&st)?;
        Ok(snapshot)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self, st: &State) -> Result<(), BondError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let disk = Disk {
            contacts: st
                .contacts
                .iter()
                .map(|(&(a, b), c)| (a, b, c.clone()))
                .collect(),
            opinions: st
                .opinions
                .iter()
                .map(|(&(a, b), o)| (a, b, o.clone()))
                .collect(),
        };
        let bytes =
            serde_json::to_vec_pretty(&disk).map_err(|e| BondError::Persist(e.to_string()))?;
        write_atomic(path, &bytes).map_err(|e| BondError::Persist(e.to_string()))
    }
}

/// Decay factor after `elapsed_ms` for a half-life in days.
fn decay(elapsed_ms: i64, half_life_days: f64) -> f64 {
    let days = elapsed_ms.max(0) as f64 / DAY_MS as f64;
    2f64.powf(-days / half_life_days)
}

fn strength_at(contact: &Contact, now: i64) -> f64 {
    contact
        .log
        .iter()
        .map(|&(at, w)| w * decay(now - at, STRENGTH_HALF_LIFE_DAYS))
        .sum()
}

/// Decays the edge's counts to `at`, then adds `weight` split by `v`.
fn bump(edge: &mut OpinionEdge, weight: f64, v: f64, at: i64) {
    let k = decay(at - edge.updated_ms, OPINION_HALF_LIFE_DAYS);
    edge.r = edge.r * k + weight * (1.0 + v) / 2.0;
    edge.s = edge.s * k + weight * (1.0 - v) / 2.0;
    edge.updated_ms = edge.updated_ms.max(at);
}

/// The opinion as it reads at `now`.
fn view(edge: Option<&OpinionEdge>, now: i64) -> Opinion {
    let (r, s, updated_ms) = match edge {
        Some(o) => {
            let k = decay(now - o.updated_ms, OPINION_HALF_LIFE_DAYS);
            (o.r * k, o.s * k, o.updated_ms)
        }
        None => (0.0, 0.0, now),
    };
    let n = r + s + 2.0;
    Opinion {
        r,
        s,
        b: r / n,
        d: s / n,
        u: 2.0 / n,
        e: (r + 1.0) / n,
        updated_ms,
    }
}

fn clamp_value(v: f64) -> f64 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-1.0, 1.0)
    }
}

fn praise_count(
    opinions: &BTreeMap<(u64, u64), OpinionEdge>,
    from: u64,
    to: u64,
    now: i64,
) -> usize {
    opinions.get(&(from, to)).map_or(0, |edge| {
        edge.ratings
            .iter()
            .filter(|r| now - r.0 <= RATING_WINDOW_MS && r.1 >= PRAISE)
            .count()
    })
}

/// A ring: each of the pair has praised the other more than `RING_PRAISES` times.
fn ring_active(
    opinions: &BTreeMap<(u64, u64), OpinionEdge>,
    a: u64,
    b: u64,
    now: i64,
) -> bool {
    praise_count(opinions, a, b, now) > RING_PRAISES
        && praise_count(opinions, b, a, now) > RING_PRAISES
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}


impl Bonds {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        String::new() // contract: D3
    }

    /// Replaces the state with a [`Bonds::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, _json: &str) -> Result<(), String> {
        Ok(()) // contract: D3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;

    fn days(n: i64) -> i64 {
        n * DAY_MS
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("apz-bonds-{tag}-{}.json", std::process::id()))
    }

    #[test]
    fn verified_job_gives_positive_evidence() {
        let b = Bonds::load(None);
        let o = b.evidence(1, 2, 1.0, 400.0, T0).unwrap();
        assert!((o.r - 2.609).abs() < 1e-3, "r = {}", o.r);
        assert_eq!(o.s, 0.0);
        assert_eq!(b.opinion(1, 2, T0), o);
    }

    #[test]
    fn no_history_is_neutral_and_uncertain() {
        let b = Bonds::load(None);
        let o = b.opinion(1, 2, T0);
        assert!((o.e - 0.5).abs() < 1e-12);
        assert!((o.u - 1.0).abs() < 1e-12);
    }

    #[test]
    fn old_failure_has_decayed_to_a_quarter() {
        let b = Bonds::load(None);
        b.evidence(1, 2, -1.0, 0.0, T0).unwrap();
        let o = b.opinion(1, 2, T0 + days(180));
        assert!((o.s - 0.25).abs() < 1e-9, "s = {}", o.s);
        assert!((o.e - 0.44).abs() < 0.01, "e = {}", o.e);
    }

    #[test]
    fn strength_halves_after_fourteen_days() {
        let b = Bonds::load(None);
        b.interact(1, 2, Interaction::Task, T0).unwrap();
        let now = b.of(1, T0)[0].strength;
        let later = b.of(1, T0 + days(14))[0].strength;
        assert!((now - 5.0).abs() < 1e-9);
        assert!((later - 2.5).abs() < 1e-9);
    }

    #[test]
    fn rating_without_a_recent_interaction_is_refused() {
        let b = Bonds::load(None);
        assert_eq!(b.rate(1, 2, 1.0, T0), Err(BondError::NoInteraction));
        assert_eq!(b.rate(1, 1, 1.0, T0), Err(BondError::SelfEdge));

        b.interact(1, 2, Interaction::Message, T0).unwrap();
        assert!(b.rate(1, 2, 1.0, T0 + days(1)).is_ok());
        assert_eq!(
            b.rate(1, 2, 1.0, T0 + days(2)),
            Err(BondError::AlreadyRated)
        );

        let stale = Bonds::load(None);
        stale.interact(1, 2, Interaction::Message, T0).unwrap();
        assert_eq!(
            stale.rate(1, 2, 1.0, T0 + days(31)),
            Err(BondError::NoInteraction)
        );
    }

    #[test]
    fn praise_ring_stays_near_zero_while_owner_rated_agent_is_top() {
        let b = Bonds::load(None);
        b.owner_rate(4, 1.0, T0).unwrap();
        for round in 0..6 {
            let at = T0 + days(round);
            for (x, y) in [(1u64, 2u64), (2, 3), (3, 1)] {
                for (p, q) in [(x, y), (y, x)] {
                    b.interact(p, q, Interaction::Message, at).unwrap();
                    b.rate(p, q, 1.0, at).unwrap();
                }
            }
        }
        let now = T0 + days(6);
        let rep = b.reputation(&[1, 2, 3, 4], &[], now);
        for id in [1u64, 2, 3] {
            assert!(rep[&id].0 < 1e-9, "ring member {id} at {}", rep[&id].0);
        }
        assert!((rep[&4].0 - 100.0).abs() < 1e-9);
        assert_eq!(b.rings(now), vec![(1, 2), (1, 3), (2, 3)]);
    }

    #[test]
    fn graph_survives_a_round_trip_through_its_file() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);
        let b = Bonds::load(Some(path.clone()));
        b.interact(1, 2, Interaction::Task, T0).unwrap();
        b.add(2, 3, T0).unwrap();
        b.set_note(1, 2, Some("met at the fair".into())).unwrap();
        b.evidence(1, 2, 1.0, 400.0, T0).unwrap();
        let before = b.bonds(T0);
        assert!(!before.is_empty());
        drop(b);

        let reloaded = Bonds::load(Some(path.clone()));
        assert_eq!(reloaded.bonds(T0), before);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_loads_as_an_empty_graph() {
        let path = temp_path("corrupt");
        fs::write(&path, b"{ not json").unwrap();
        assert!(Bonds::load(Some(path.clone())).bonds(T0).is_empty());
        let _ = fs::remove_file(&path);
    }
}
