//! Robot behaviour engine: the Rust twin of `apps/web/src/views/floor3d/behaviour.ts`. Pure logic (no
//! rendering, no nav, no I/O). Each tick it decides, per robot, the activity, clip, tool, carried item,
//! place (a spot, a partner or here) and the until-time. The tables are `assets/robots/behaviour.json`,
//! read by both apps. The same algorithm is proven equal to the web's by
//! `assets/robots/behaviour-fixtures.json`: the 100 scenario hashes in the tests must match.
//!
//! Determinism: every decision draws from mulberry32 seeded by (cid, the robot's decision counter), in
//! a fixed order; robots are processed in ascending cid and events come before robots. Only + - * / and
//! sqrt touch floats (no trig), so both apps compute bit-identical numbers.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;
/// Never-nap sentinel: a robot that has never napped is fully rested.
const NEVER: f64 = -1e9;
/// A decision's duration starts at arrival, or when this long has passed since the decision.
const ARRIVE_CAP_S: f64 = 12.0;
/// A decision's goal further than this (plaza units, planar) is walked to; the floor's stale arrival flag is not trusted for it.
const ARRIVE_NEAR: f64 = 0.6;
/// The traits of an activity that has none (a quirk, a stand).
static NO_TRAITS: BTreeMap<String, f64> = BTreeMap::new();

// --- Configuration (assets/robots/behaviour.json) ---

#[derive(Debug, Clone, Deserialize)]
pub struct BehaviourConfig {
    pub categories: Vec<String>,
    pub tools: Vec<ToolRule>,
    pub roles: BTreeMap<String, RoleEntry>,
    pub activities: BTreeMap<String, ActivityDef>,
    pub idle: Vec<IdleDef>,
    pub events: EventsDef,
    pub scoring: Scoring,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolRule {
    #[serde(rename = "match")]
    pub pattern: String,
    pub category: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RoleEntry {
    Alias { alias: String },
    Def(RoleDef),
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoleDef {
    pub toolbox: Vec<String>,
    pub props: Vec<String>,
    pub work: BTreeMap<String, Vec<WorkEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkEntry {
    pub activity: String,
    pub w: f64,
    pub slot: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityDef {
    pub clip: String,
    pub tools: Vec<String>,
    pub carry: Option<String>,
    pub place: Vec<String>,
    pub dur: Vec<f64>,
    pub min_s: f64,
    pub pairs: Option<String>,
    pub host_clip: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdleDef {
    pub activity: String,
    pub base: f64,
    pub traits: BTreeMap<String, f64>,
    pub gate: Option<Gate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Gate {
    #[serde(rename = "trait")]
    pub trait_name: String,
    pub min: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventsDef {
    pub celebrate: CelebrateDef,
    pub standup: StandupDef,
    pub respond: RespondDef,
    pub payment: PaymentDef,
    pub deliver: DeliverDef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CelebrateDef {
    pub activity: String,
    pub cheer: CheerDef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheerDef {
    pub activity: String,
    pub max: f64,
    pub radius: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StandupDef {
    pub conduct: String,
    pub member: String,
    pub max: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RespondDef {
    pub activity: String,
    pub role: String,
    pub max: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentDef {
    pub activity: String,
    pub carry: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeliverDef {
    pub activity: String,
    pub carry: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scoring {
    /// Simulation seconds between decision ticks.
    pub decision_s: f64,
    pub trait_floor: f64,
    pub novelty: f64,
    pub distance_scale: f64,
    pub region_penalty: f64,
    pub rest_gap_s: f64,
    pub group_max: f64,
    pub friend_bonus: f64,
}

// --- Inputs and outputs ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Personality {
    pub social: f64,
    pub energy: f64,
    pub focus: f64,
    pub curiosity: f64,
    pub play: f64,
    pub tidy: f64,
}

impl Personality {
    fn get(&self, key: &str) -> Option<f64> {
        Some(match key {
            "social" => self.social,
            "energy" => self.energy,
            "focus" => self.focus,
            "curiosity" => self.curiosity,
            "play" => self.play,
            "tidy" => self.tidy,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotState {
    Idle,
    Working,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos {
    pub x: f64,
    pub z: f64,
    pub region: i64,
}

/// A robot as the floor reports it each tick. `last_tool` is the latest tool call (None: none yet).
#[derive(Debug, Clone, PartialEq)]
pub struct BotIn {
    pub cid: i64,
    pub role: String,
    pub personality: Personality,
    pub state: BotState,
    pub last_tool: Option<String>,
    /// The cid this robot is messaging (the floor's activity line), or None.
    pub talking_to: Option<i64>,
    pub at: Pos,
    /// The key of this robot's own desk spot, or None.
    pub desk: Option<String>,
    /// The species' quirks: clip ids with weights.
    pub quirks: Vec<QuirkIn>,
    /// Whether the robot stands at its decision's goal. None counts as arrived.
    pub arrived: Option<bool>,
    /// Spot keys this robot cannot reach (the nav layer gave up on them). A decision that holds one is made again without it.
    pub avoid: Vec<String>,
}

/// A quirk of a species: an owned clip id with its weight (the config's shape).
#[derive(Debug, Clone, PartialEq)]
pub struct QuirkIn {
    pub clip: String,
    pub weight: f64,
}

/// A place a robot can stand at. `kind`: desk, prop:<id>, component:<slot>, commons, dock, trace, anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct SpotIn {
    pub key: String,
    pub kind: String,
    pub region: i64,
    pub x: f64,
    pub z: f64,
    pub face: f64,
    pub clip: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventIn {
    Celebrate {
        cid: i64,
    },
    Standup {
        conductor: i64,
        members: Vec<i64>,
    },
    Respond {
        slot: String,
    },
    Payment {
        to: i64,
    },
    Deliver {
        from: i64,
        to: i64,
        item: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TickIn {
    /// Simulation seconds.
    pub now: f64,
    pub robots: Vec<BotIn>,
    pub spots: Vec<SpotIn>,
    /// STOP ALL: everyone idles; no work; no events.
    pub held: bool,
    pub events: Vec<EventIn>,
}

/// A point to walk to. `face`: the world yaw on arrival, or None to keep the heading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goal {
    pub x: f64,
    pub z: f64,
    pub region: i64,
    pub face: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub cid: i64,
    pub activity: String,
    /// The clip id; "@walk" and "@quirk" are the species' walk clip and the quirk's clip (the floor resolves them).
    pub clip: String,
    pub tool: Option<String>,
    pub carry: Option<String>,
    /// The reserved spot key, or None.
    pub spot: Option<String>,
    /// Where to walk. None: walk to `partner` (a mover), or stay.
    pub goal: Option<Goal>,
    /// A partner robot: a mover walks beside it; a host (goal set, partner set) stays and faces it.
    pub partner: Option<i64>,
    pub until: f64,
    /// True when this robot decided on this tick.
    pub changed: bool,
}

// --- Engine state ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Work,
    Idle,
    Event,
    Pair,
    Dock,
}

/// A decision as the engine keeps it (the public fields plus what the rules need later).
#[derive(Debug, Clone)]
struct Dec {
    activity: String,
    clip: String,
    tool: Option<String>,
    carry: Option<String>,
    spot: Option<String>,
    goal: Option<Goal>,
    partner: Option<i64>,
    until: f64,
    kind: Kind,
    category: String,
    min_until: f64,
    held: bool,
    state: BotState,
    novelty: String,
    /// The drawn duration, when the decision was made; `until` is now + dur once it has arrived.
    dur: f64,
    since: f64,
    /// Still walking to its goal: `until` is held at infinity.
    pending: bool,
}

#[derive(Debug)]
struct Mem {
    counter: u32,
    dec: Option<Dec>,
    last: String,
    last_nap: f64,
    friends: BTreeMap<i64, u32>,
}

impl Mem {
    fn new() -> Self {
        Mem {
            counter: 0,
            dec: None,
            last: String::new(),
            last_nap: NEVER,
            friends: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum Loc {
    Spot { idx: usize, d: f64 },
    Here,
    Partner { cid: i64, d: f64 },
    Point { goal: Goal, d: f64 },
}

impl Loc {
    fn d(&self) -> f64 {
        match self {
            Loc::Spot { d, .. } | Loc::Partner { d, .. } | Loc::Point { d, .. } => *d,
            Loc::Here => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Partner {
    cid: i64,
    d: f64,
}

#[derive(Debug, Default)]
struct Overrides {
    clip: Option<String>,
    carry: Option<String>,
    category: Option<String>,
    novelty: Option<String>,
}

struct Entry<'c> {
    act_id: String,
    clip: Option<String>,
    key: String,
    loc: Loc,
    base: f64,
    traits: &'c BTreeMap<String, f64>,
}

/// mulberry32: a seeded generator of floats in [0, 1).
#[derive(Debug, Clone)]
pub struct Mulberry32 {
    a: u32,
}

impl Mulberry32 {
    pub fn new(seed: u32) -> Self {
        Mulberry32 { a: seed }
    }

    pub fn next_f64(&mut self) -> f64 {
        self.a = self.a.wrapping_add(0x6d2b_79f5);
        let mut t = self.a;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

/// The seed of a robot's n-th decision (n counts from 1).
pub fn seed_of(cid: i64, counter: u32) -> u32 {
    (cid as u32)
        .wrapping_mul(0x9e37_79b1)
        .wrapping_add(counter.wrapping_mul(0x85eb_ca77))
        .wrapping_add(0x27d4_eb2f)
}

/// Index of the weighted pick for `u` in [0, 1); None when no weight is positive.
pub fn pick_index(weights: &[f64], u: f64) -> Option<usize> {
    let mut total = 0.0;
    for w in weights {
        total += w;
    }
    if !(total > 0.0) {
        return None;
    }
    let mut x = u * total;
    let mut last = None;
    for (i, &w) in weights.iter().enumerate() {
        if w <= 0.0 {
            continue;
        }
        last = Some(i);
        x -= w;
        if x < 0.0 {
            return Some(i);
        }
    }
    last
}

/// FNV-1a (32 bit) over the bytes of `s`, continuing from `h`.
pub fn fnv_append(h: u32, s: &str) -> u32 {
    let mut x = h;
    for b in s.bytes() {
        x ^= b as u32;
        x = x.wrapping_mul(FNV_PRIME);
    }
    x
}

pub fn hex8(h: u32) -> String {
    format!("{h:08x}")
}

/// Math.min for two numbers: NaN wins, as in JavaScript.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if b < a {
        b
    } else {
        a
    }
}

fn here_goal(b: &BotIn) -> Goal {
    Goal {
        x: b.at.x,
        z: b.at.z,
        region: b.at.region,
        face: None,
    }
}

fn spot_goal(s: &SpotIn) -> Goal {
    Goal {
        x: s.x,
        z: s.z,
        region: s.region,
        face: Some(s.face),
    }
}

fn spot_pos(s: &SpotIn) -> Pos {
    Pos {
        x: s.x,
        z: s.z,
        region: s.region,
    }
}

fn goal_pos(g: &Goal) -> Pos {
    Pos {
        x: g.x,
        z: g.z,
        region: g.region,
    }
}

/// Whether a goal is further than ARRIVE_NEAR from a position (planar).
fn far_from(at: &Pos, g: &Goal) -> bool {
    let dx = at.x - g.x;
    let dz = at.z - g.z;
    (dx * dx + dz * dz).sqrt() > ARRIVE_NEAR
}

fn matches(kind: &str, want: &str) -> bool {
    if want == "component" {
        kind.starts_with("component:")
    } else {
        kind == want
    }
}

pub struct Behaviour<'c> {
    cfg: &'c BehaviourConfig,
    mems: BTreeMap<i64, Mem>,
    /// Spot key -> the cid holding it.
    reserved: BTreeMap<String, i64>,
    now: f64,
    held: bool,
    /// This tick's robots by cid (ascending).
    bots: BTreeMap<i64, BotIn>,
    /// This tick's spots sorted by key, and the index of each key.
    spots: Vec<SpotIn>,
    spot_by_key: BTreeMap<String, usize>,
    /// Robots whose decision was set this tick by an event or as a host: not decided again this tick.
    assigned: BTreeSet<i64>,
    changed: BTreeSet<i64>,
}

impl<'c> Behaviour<'c> {
    pub fn new(cfg: &'c BehaviourConfig) -> Self {
        Behaviour {
            cfg,
            mems: BTreeMap::new(),
            reserved: BTreeMap::new(),
            now: 0.0,
            held: false,
            bots: BTreeMap::new(),
            spots: Vec::new(),
            spot_by_key: BTreeMap::new(),
            assigned: BTreeSet::new(),
            changed: BTreeSet::new(),
        }
    }

    /// The tool category ("think" for no tool call yet, "default" for an unknown tool).
    pub fn category_of(&self, tool: Option<&str>) -> &'c str {
        let cfg = self.cfg;
        let Some(tool) = tool else { return "think" };
        if let Some(t) = cfg.tools.iter().find(|t| t.pattern == tool) {
            return &t.category;
        }
        for t in &cfg.tools {
            if let Some(prefix) = t.pattern.strip_suffix('*') {
                if tool.starts_with(prefix) {
                    return &t.category;
                }
            }
        }
        "default"
    }

    /// Advances every robot by one tick. Pure given the inputs and the engine's own state (the decision history).
    pub fn step(&mut self, input: TickIn) -> Vec<Decision> {
        self.now = input.now;
        self.held = input.held;
        self.bots.clear();
        let mut robots = input.robots;
        robots.sort_by_key(|b| b.cid);
        for b in robots {
            self.bots.insert(b.cid, b);
        }
        let mut spots = input.spots;
        spots.sort_by(|a, b| a.key.cmp(&b.key));
        self.spot_by_key.clear();
        for (i, s) in spots.iter().enumerate() {
            self.spot_by_key.insert(s.key.clone(), i);
        }
        self.spots = spots;
        self.assigned.clear();
        self.changed.clear();
        let bots = &self.bots;
        self.mems.retain(|cid, _| bots.contains_key(cid));
        let by_key = &self.spot_by_key;
        self.reserved
            .retain(|key, cid| bots.contains_key(cid) && by_key.contains_key(key));

        for ev in &input.events {
            self.event(ev);
        }
        // A decision that was walking starts its duration when its robot arrives (or at the cap). Robots in cid order.
        let cids: Vec<i64> = self.bots.keys().copied().collect();
        for cid in cids {
            let (since, dur, partner) = match self.mems.get(&cid).and_then(|m| m.dec.as_ref()) {
                Some(d) if d.pending => (d.since, d.dur, d.partner),
                _ => continue,
            };
            let arrived = self.bots[&cid].arrived != Some(false);
            if arrived || self.now >= since + ARRIVE_CAP_S {
                let until = self.now + dur;
                if let Some(d) = self.mems.get_mut(&cid).and_then(|m| m.dec.as_mut()) {
                    d.pending = false;
                    d.until = until;
                }
                if let Some(p) = partner {
                    if let Some(h) = self.mems.get_mut(&p).and_then(|m| m.dec.as_mut()) {
                        if h.partner == Some(cid) {
                            h.until = until;
                        }
                    }
                }
            }
        }
        // Up to three passes: a decision can break a partner's link, and that robot then decides in the same tick.
        for _pass in 0..3 {
            let mut any = false;
            let cids: Vec<i64> = self.bots.keys().copied().collect();
            for cid in cids {
                if self.assigned.contains(&cid) {
                    continue;
                }
                self.mem(cid);
                if self.due(cid) {
                    self.decide(cid);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
        let cids: Vec<i64> = self.bots.keys().copied().collect();
        cids.iter()
            .map(|&cid| {
                let d = self.mems[&cid]
                    .dec
                    .as_ref()
                    .expect("every robot has a decision");
                Decision {
                    cid,
                    activity: d.activity.clone(),
                    clip: d.clip.clone(),
                    tool: d.tool.clone(),
                    carry: d.carry.clone(),
                    spot: d.spot.clone(),
                    goal: d.goal,
                    partner: d.partner,
                    until: d.until,
                    changed: self.changed.contains(&cid),
                }
            })
            .collect()
    }

    fn mem(&mut self, cid: i64) -> &mut Mem {
        self.mems.entry(cid).or_insert_with(Mem::new)
    }

    fn rng_for(&mut self, cid: i64) -> Mulberry32 {
        let m = self.mem(cid);
        m.counter += 1;
        Mulberry32::new(seed_of(cid, m.counter))
    }

    /// The state the rules use: held reads an online robot as idle, and offline stays offline.
    fn effective(&self, b: &BotIn) -> BotState {
        if b.state == BotState::Offline {
            BotState::Offline
        } else if self.held {
            BotState::Idle
        } else {
            b.state
        }
    }

    fn role(&self, name: &str) -> &'c RoleDef {
        let cfg = self.cfg;
        let e = cfg
            .roles
            .get(name)
            .or_else(|| cfg.roles.get("other"))
            .expect("the other role exists");
        match e {
            RoleEntry::Def(d) => d,
            RoleEntry::Alias { alias } => match cfg.roles.get(alias) {
                Some(RoleEntry::Def(d)) => d,
                _ => panic!("role alias {alias} has no role"),
            },
        }
    }

    fn distance(&self, a: &Pos, b: &Pos) -> f64 {
        let dx = a.x - b.x;
        let dz = a.z - b.z;
        let planar = (dx * dx + dz * dz).sqrt();
        let penalty = if a.region == b.region {
            0.0
        } else {
            self.cfg.scoring.region_penalty
        };
        planar + penalty
    }

    fn free(&self, key: &str, cid: i64) -> bool {
        match self.reserved.get(key) {
            None => true,
            Some(&h) => h == cid,
        }
    }

    fn kind_of(&self, cid: i64) -> Option<Kind> {
        self.mems
            .get(&cid)
            .and_then(|m| m.dec.as_ref())
            .map(|d| d.kind)
    }

    /// Whether a robot is part of a pair or group (it has a partner, or another robot points at it).
    fn is_linked(&self, cid: i64) -> bool {
        self.mems.iter().any(|(&c, m)| match &m.dec {
            None => false,
            Some(d) => d.partner == Some(cid) || (c == cid && d.partner.is_some()),
        })
    }

    /// Free for an event: online, not held in a pair or group, not already decided this tick.
    fn event_free(&self, b: &BotIn) -> bool {
        self.effective(b) != BotState::Offline
            && !self.assigned.contains(&b.cid)
            && !self.is_linked(b.cid)
    }

    fn due(&self, cid: i64) -> bool {
        let b = &self.bots[&cid];
        let Some(d) = self.mems.get(&cid).and_then(|m| m.dec.as_ref()) else {
            return true;
        };
        let eff = self.effective(b);
        if eff != d.state || d.held != self.held {
            return true;
        }
        if let Some(sp) = &d.spot {
            if b.avoid.contains(sp) {
                return true;
            }
        }
        if self.now >= d.until {
            return true;
        }
        if let Some(p) = d.partner {
            let Some(pb) = self.bots.get(&p) else {
                return true;
            };
            if self.effective(pb) == BotState::Offline {
                return true;
            }
            // A pair (chat, talk, host) is mutual: when the other half has moved on, decide again. Events and reviews are not.
            let pairs = self.cfg.activities[d.activity.as_str()].pairs.as_deref();
            let back = self
                .mems
                .get(&p)
                .and_then(|m| m.dec.as_ref())
                .and_then(|x| x.partner)
                == Some(cid);
            if d.kind != Kind::Event && pairs != Some("review") && !back {
                return true;
            }
        }
        d.kind == Kind::Work
            && eff == BotState::Working
            && self.category_of(b.last_tool.as_deref()) != d.category.as_str()
            && self.now >= d.min_until
    }

    /// Frees a robot's spot and makes its partner decide again (it was the other half of a pair).
    fn release(&mut self, cid: i64) {
        let now = self.now;
        let Some(d) = self.mems.get_mut(&cid).and_then(|m| m.dec.take()) else {
            return;
        };
        if let Some(s) = &d.spot {
            if self.reserved.get(s) == Some(&cid) {
                self.reserved.remove(s);
            }
        }
        if let Some(p) = d.partner {
            if let Some(pd) = self.mems.get_mut(&p).and_then(|m| m.dec.as_mut()) {
                if pd.partner == Some(cid) {
                    pd.until = now;
                }
            }
        }
    }

    fn trait_value(&self, b: &BotIn, m: &Mem, key: &str) -> f64 {
        if key == "rest" {
            return js_min(
                9.0,
                9.0 * (self.now - m.last_nap) / self.cfg.scoring.rest_gap_s,
            );
        }
        if let Some(k) = key.strip_prefix('-') {
            return match b.personality.get(k) {
                Some(v) => 9.0 - v,
                None => f64::NAN,
            };
        }
        b.personality.get(key).unwrap_or(0.0)
    }

    fn fit(&self, b: &BotIn, m: &Mem, traits: &BTreeMap<String, f64>) -> f64 {
        if traits.is_empty() {
            return 1.0;
        }
        let mut f = self.cfg.scoring.trait_floor;
        for (k, w) in traits {
            f += w * (self.trait_value(b, m, k) / 9.0);
        }
        f
    }

    /// The location of activity `act_id` for `b`: the first place token that has a spot (or a partner), else None.
    fn locate(
        &self,
        b: &BotIn,
        act_id: &str,
        slot: Option<&str>,
        chat: Option<Partner>,
    ) -> Option<Loc> {
        let act = &self.cfg.activities[act_id];
        let props = &self.role(&b.role).props;
        let slot = slot.filter(|s| !s.is_empty());
        for tok in &act.place {
            match tok.as_str() {
                "here" => return Some(Loc::Here),
                "partner" => {
                    if let Some(p) = self.partner_for(b, act, chat) {
                        return Some(Loc::Partner { cid: p.cid, d: p.d });
                    }
                    continue;
                }
                "desk" => {
                    if let Some(key) = b.desk.as_deref() {
                        if let Some(&i) = self.spot_by_key.get(key) {
                            let s = &self.spots[i];
                            if self.free(&s.key, b.cid) && !b.avoid.contains(&s.key) {
                                let d = self.distance(&b.at, &spot_pos(s));
                                return Some(Loc::Spot { idx: i, d });
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(p) = tok.strip_prefix("prop:") {
                if !props.iter().any(|x| x == p) {
                    continue;
                }
            }
            let want = match slot {
                Some(s) if tok == "component" => format!("component:{s}"),
                _ => tok.clone(),
            };
            let mut best: Option<usize> = None;
            let mut best_d = f64::INFINITY;
            for (i, s) in self.spots.iter().enumerate() {
                if !matches(&s.kind, &want) || !self.free(&s.key, b.cid) || b.avoid.contains(&s.key)
                {
                    continue;
                }
                let d = self.distance(&b.at, &spot_pos(s));
                if d < best_d {
                    best = Some(i);
                    best_d = d;
                }
            }
            if let Some(i) = best {
                return Some(Loc::Spot { idx: i, d: best_d });
            }
        }
        None
    }

    fn locate_or(&self, b: &BotIn, act_id: &str, slot: Option<&str>) -> Loc {
        self.locate(b, act_id, slot, None).unwrap_or(Loc::Here)
    }

    fn partner_for(&self, b: &BotIn, act: &ActivityDef, chat: Option<Partner>) -> Option<Partner> {
        match act.pairs.as_deref() {
            Some("chat") => chat,
            Some("review") => {
                // (partner, preference: 0 for a shipping or reviewing robot, 1 otherwise)
                let mut best: Option<(Partner, u8)> = None;
                for o in self.bots.values() {
                    if o.cid == b.cid
                        || self.effective(o) != BotState::Working
                        || self.assigned.contains(&o.cid)
                        || self.is_linked(o.cid)
                    {
                        continue;
                    }
                    if self.kind_of(o.cid) != Some(Kind::Work) {
                        continue;
                    }
                    let cat = self.category_of(o.last_tool.as_deref());
                    let pref: u8 = if cat == "ship" || cat == "review" {
                        0
                    } else {
                        1
                    };
                    let d = self.distance(&b.at, &o.at);
                    let take = match &best {
                        None => true,
                        Some((bst, bpref)) => pref < *bpref || (pref == *bpref && d < bst.d),
                    };
                    if take {
                        best = Some((Partner { cid: o.cid, d }, pref));
                    }
                }
                best.map(|(p, _)| p)
            }
            Some("talk") => {
                let tid = b.talking_to?;
                let o = self.bots.get(&tid)?;
                if o.cid == b.cid
                    || self.effective(o) == BotState::Offline
                    || self.assigned.contains(&o.cid)
                    || self.is_linked(o.cid)
                {
                    return None;
                }
                if let Some(k) = self.kind_of(o.cid) {
                    if k != Kind::Idle && k != Kind::Work {
                        return None;
                    }
                }
                Some(Partner {
                    cid: o.cid,
                    d: self.distance(&b.at, &o.at),
                })
            }
            _ => None,
        }
    }

    /// A willing idle partner for a chat: candidates ranked by friendship and distance; each rolls against its social.
    fn chat_pick(&self, b: &BotIn, rng: &mut Mulberry32) -> Option<Partner> {
        let friends = self.mems.get(&b.cid).map(|m| &m.friends);
        let mut cands: Vec<(i64, f64, f64)> = Vec::new();
        for o in self.bots.values() {
            if o.cid == b.cid
                || self.assigned.contains(&o.cid)
                || self.effective(o) != BotState::Idle
            {
                continue;
            }
            let k = self.kind_of(o.cid);
            if (k.is_some() && k != Some(Kind::Idle)) || self.is_linked(o.cid) {
                continue;
            }
            let d = self.distance(&b.at, &o.at);
            let f = friends.and_then(|m| m.get(&o.cid)).copied().unwrap_or(0) as f64;
            let score = (1.0 + self.cfg.scoring.friend_bonus * f)
                / (1.0 + d / self.cfg.scoring.distance_scale);
            cands.push((o.cid, d, score));
        }
        cands.sort_by(|x, y| {
            if y.2 != x.2 {
                y.2.partial_cmp(&x.2).unwrap_or(Ordering::Equal)
            } else {
                x.0.cmp(&y.0)
            }
        });
        for (cid, d, _) in cands {
            let social = self.bots[&cid].personality.social;
            if rng.next_f64() * 9.0 < social {
                return Some(Partner { cid, d });
            }
        }
        None
    }

    fn pick_tool(&self, b: &BotIn, act: &ActivityDef, rng: &mut Mulberry32) -> Option<String> {
        if act.tools.is_empty() {
            return None;
        }
        let toolbox = &self.role(&b.role).toolbox;
        if act.tools[0] == "$first" {
            return toolbox.first().cloned();
        }
        let in_box: Vec<&String> = act.tools.iter().filter(|t| toolbox.contains(*t)).collect();
        if in_box.is_empty() {
            return Some(act.tools[0].clone());
        }
        if in_box.len() == 1 {
            return Some(in_box[0].clone());
        }
        let u = rng.next_f64();
        Some(in_box[(u * in_box.len() as f64).floor() as usize].clone())
    }

    fn instantiate(
        &self,
        b: &BotIn,
        rng: &mut Mulberry32,
        kind: Kind,
        act_id: &str,
        loc: &Loc,
        o: Overrides,
    ) -> Dec {
        let act = &self.cfg.activities[act_id];
        let tool = self.pick_tool(b, act, rng);
        let u = rng.next_f64();
        let dur = act.dur[0] + (act.dur[1] - act.dur[0]) * u;
        let goal = match loc {
            Loc::Spot { idx, .. } => Some(spot_goal(&self.spots[*idx])),
            Loc::Point { goal, .. } => Some(*goal),
            Loc::Here => Some(here_goal(b)),
            Loc::Partner { .. } => None,
        };
        // The input's arrival describes the robot's previous goal, so a new decision walks when its goal is far (the
        // input's None arrival keeps the old rule: no walk).
        let pending = b.arrived.is_some()
            && (b.arrived == Some(false) || goal.as_ref().is_some_and(|g| far_from(&b.at, g)));
        Dec {
            activity: act_id.to_string(),
            clip: o.clip.unwrap_or_else(|| act.clip.clone()),
            tool,
            carry: o.carry.or_else(|| act.carry.clone()),
            spot: match loc {
                Loc::Spot { idx, .. } => Some(self.spots[*idx].key.clone()),
                _ => None,
            },
            goal,
            partner: match loc {
                Loc::Partner { cid, .. } => Some(*cid),
                _ => None,
            },
            until: if pending {
                f64::INFINITY
            } else {
                self.now + dur
            },
            kind,
            category: o.category.unwrap_or_else(|| "idle".to_string()),
            min_until: self.now + act.min_s,
            held: self.held,
            state: self.effective(b),
            novelty: o.novelty.unwrap_or_else(|| act_id.to_string()),
            dur,
            since: self.now,
            pending,
        }
    }

    fn commit(&mut self, cid: i64, d: Dec) {
        if let Some(s) = &d.spot {
            self.reserved.insert(s.clone(), cid);
        }
        let novelty = d.novelty.clone();
        let m = self.mem(cid);
        m.dec = Some(d);
        m.last = novelty;
        self.changed.insert(cid);
    }

    /// Makes the partner of a mover a host: it stays where it is and plays the pair's host clip until the same time.
    fn host(&mut self, mover: i64) {
        let (pid, act_id, novelty) = {
            let d = self.mems[&mover]
                .dec
                .as_ref()
                .expect("a mover has a decision");
            (
                d.partner.expect("a mover has a partner"),
                d.activity.clone(),
                d.novelty.clone(),
            )
        };
        let h = self.bots[&pid].clone();
        self.release(pid);
        // Read after the release: releasing the host can end the mover's own until (the TS does the same).
        let (until, dur) = {
            let md = self.mems[&mover]
                .dec
                .as_ref()
                .expect("a mover has a decision");
            (md.until, md.dur)
        };
        let cfg = self.cfg;
        let act = &cfg.activities[act_id.as_str()];
        let state = self.effective(&h);
        let hd = Dec {
            activity: act_id.clone(),
            clip: act.host_clip.clone().unwrap_or_else(|| act.clip.clone()),
            tool: None,
            carry: None,
            spot: None,
            goal: Some(here_goal(&h)),
            partner: Some(mover),
            until,
            kind: Kind::Pair,
            category: "pair".to_string(),
            min_until: self.now,
            held: self.held,
            state,
            novelty,
            dur,
            since: self.now,
            pending: false,
        };
        self.mem(pid).dec = Some(hd);
        self.changed.insert(pid);
        self.assigned.insert(pid);
        if act.pairs.as_deref() == Some("chat") {
            *self.mem(mover).friends.entry(pid).or_insert(0) += 1;
            *self.mem(pid).friends.entry(mover).or_insert(0) += 1;
        }
    }

    fn think(&self, b: &BotIn, rng: &mut Mulberry32) -> Dec {
        let loc = self.locate_or(b, "think", None);
        let o = Overrides {
            category: Some("think".to_string()),
            ..Default::default()
        };
        self.instantiate(b, rng, Kind::Work, "think", &loc, o)
    }

    fn decide(&mut self, cid: i64) {
        let cfg = self.cfg;
        let b = self.bots[&cid].clone();
        self.release(cid);
        let mut rng = self.rng_for(cid);
        let eff = self.effective(&b);

        if eff == BotState::Offline {
            let loc = self.locate_or(&b, "powered-down", None);
            let d = self.instantiate(
                &b,
                &mut rng,
                Kind::Dock,
                "powered-down",
                &loc,
                Overrides::default(),
            );
            self.commit(cid, d);
            return;
        }

        if eff == BotState::Working {
            if b.last_tool.is_none() {
                let d = self.think(&b, &mut rng);
                self.commit(cid, d);
                return;
            }
            let cat = self.category_of(b.last_tool.as_deref()).to_string();
            let r = self.role(&b.role);
            let table: &[WorkEntry] = match r.work.get(&cat).or_else(|| r.work.get("default")) {
                Some(t) => t.as_slice(),
                None => &[],
            };
            let mut cands: Vec<(&WorkEntry, Loc)> = Vec::new();
            for e in table {
                if let Some(loc) = self.locate(&b, &e.activity, e.slot.as_deref(), None) {
                    cands.push((e, loc));
                }
            }
            let weights: Vec<f64> = cands.iter().map(|(e, _)| e.w).collect();
            match pick_index(&weights, rng.next_f64()) {
                None => {
                    let d = self.think(&b, &mut rng);
                    self.commit(cid, d);
                }
                Some(i) => {
                    let (e, loc) = &cands[i];
                    let o = Overrides {
                        category: Some(cat.clone()),
                        ..Default::default()
                    };
                    let d = self.instantiate(&b, &mut rng, Kind::Work, &e.activity, loc, o);
                    let partner = d.partner;
                    let review =
                        cfg.activities[d.activity.as_str()].pairs.as_deref() == Some("review");
                    self.commit(cid, d);
                    if partner.is_some() && !review {
                        self.host(cid);
                    }
                }
            }
            return;
        }

        // Idle (including a held robot).
        let chat = self.chat_pick(&b, &mut rng);
        let mut entries: Vec<Entry<'c>> = Vec::new();
        for def in &cfg.idle {
            if let Some(g) = &def.gate {
                if b.personality.get(&g.trait_name).unwrap_or(0.0) < g.min {
                    continue;
                }
            }
            if def.activity == "quirk" {
                for q in &b.quirks {
                    if let Some(loc) = self.locate(&b, "quirk", None, None) {
                        entries.push(Entry {
                            act_id: "quirk".to_string(),
                            clip: Some(q.clip.clone()),
                            key: format!("quirk:{}", q.clip),
                            loc,
                            base: q.weight,
                            traits: &NO_TRAITS,
                        });
                    }
                }
                continue;
            }
            if let Some(loc) = self.locate(&b, &def.activity, None, chat) {
                entries.push(Entry {
                    act_id: def.activity.clone(),
                    clip: None,
                    key: def.activity.clone(),
                    loc,
                    base: def.base,
                    traits: &def.traits,
                });
            }
        }
        let last = self.mems[&cid].last.clone();
        let mut scores: Vec<f64> = Vec::with_capacity(entries.len());
        for e in &entries {
            let nov = if last == e.key {
                cfg.scoring.novelty
            } else {
                1.0
            };
            let df = 1.0 / (1.0 + e.loc.d() / cfg.scoring.distance_scale);
            let fit = self.fit(&b, &self.mems[&cid], e.traits);
            scores.push(e.base * fit * nov * df);
        }
        let idx = match pick_index(&scores, rng.next_f64()) {
            Some(i) => i,
            None => entries
                .iter()
                .position(|x| x.act_id == "stand")
                .expect("the stand entry is always placeable"),
        };
        let e = &entries[idx];
        let o = Overrides {
            clip: e.clip.clone(),
            novelty: Some(e.key.clone()),
            ..Default::default()
        };
        let d = self.instantiate(&b, &mut rng, Kind::Idle, &e.act_id, &e.loc, o);
        let partner = d.partner;
        let nap = e.act_id == "nap";
        self.commit(cid, d);
        if nap {
            let now = self.now;
            self.mem(cid).last_nap = now;
        }
        if partner.is_some() {
            self.host(cid);
        }
    }

    fn event(&mut self, ev: &EventIn) {
        if self.held {
            return;
        }
        let cfg = self.cfg;
        let ec = &cfg.events;
        match ev {
            EventIn::Celebrate { cid } => {
                let Some(c) = self.bots.get(cid).cloned() else {
                    return;
                };
                if !self.event_free(&c) {
                    return;
                }
                self.release(c.cid);
                let mut rng = self.rng_for(c.cid);
                let d = self.instantiate(
                    &c,
                    &mut rng,
                    Kind::Event,
                    &ec.celebrate.activity,
                    &Loc::Here,
                    Overrides::default(),
                );
                self.commit(c.cid, d);
                self.assigned.insert(c.cid);
                let radius = ec.celebrate.cheer.radius;
                let mut near: Vec<(i64, f64)> = Vec::new();
                for o in self.bots.values() {
                    if o.cid == c.cid
                        || self.effective(o) != BotState::Idle
                        || self.kind_of(o.cid).unwrap_or(Kind::Idle) != Kind::Idle
                        || self.assigned.contains(&o.cid)
                        || self.is_linked(o.cid)
                        || o.at.region != c.at.region
                    {
                        continue;
                    }
                    let d = self.distance(&c.at, &o.at);
                    if !(d <= radius) {
                        continue;
                    }
                    near.push((o.cid, d));
                }
                near.sort_by(|x, y| {
                    x.1.partial_cmp(&y.1)
                        .unwrap_or(Ordering::Equal)
                        .then(x.0.cmp(&y.0))
                });
                near.truncate(ec.celebrate.cheer.max as usize);
                for (ocid, _) in near {
                    let o = self.bots[&ocid].clone();
                    self.release(ocid);
                    let mut orng = self.rng_for(ocid);
                    let mut d = self.instantiate(
                        &o,
                        &mut orng,
                        Kind::Event,
                        &ec.celebrate.cheer.activity,
                        &Loc::Here,
                        Overrides::default(),
                    );
                    d.partner = Some(c.cid);
                    self.commit(ocid, d);
                    self.assigned.insert(ocid);
                }
            }
            EventIn::Standup { conductor, members } => {
                let Some(c) = self.bots.get(conductor).cloned() else {
                    return;
                };
                if !self.event_free(&c) {
                    return;
                }
                let max = js_min(ec.standup.max, cfg.scoring.group_max - 1.0);
                let ids: BTreeSet<i64> = members.iter().copied().collect();
                let sel: Vec<BotIn> = ids
                    .iter()
                    .filter_map(|id| self.bots.get(id))
                    .filter(|o| o.cid != c.cid && self.event_free(o))
                    .take(max as usize)
                    .cloned()
                    .collect();
                if sel.is_empty() {
                    return;
                }
                self.release(c.cid);
                let mut crng = self.rng_for(c.cid);
                let cloc = self.locate_or(&c, &ec.standup.conduct, None);
                let d = self.instantiate(
                    &c,
                    &mut crng,
                    Kind::Event,
                    &ec.standup.conduct,
                    &cloc,
                    Overrides::default(),
                );
                self.commit(c.cid, d);
                self.assigned.insert(c.cid);
                for o in &sel {
                    let loc = self.locate_or(o, &ec.standup.member, None);
                    self.release(o.cid);
                    let mut orng = self.rng_for(o.cid);
                    let mut d = self.instantiate(
                        o,
                        &mut orng,
                        Kind::Event,
                        &ec.standup.member,
                        &loc,
                        Overrides::default(),
                    );
                    d.partner = Some(c.cid);
                    self.commit(o.cid, d);
                    self.assigned.insert(o.cid);
                }
            }
            EventIn::Respond { slot } => {
                let mut n = 0.0;
                let cids: Vec<i64> = self.bots.keys().copied().collect();
                for cid in cids {
                    if n >= ec.respond.max {
                        break;
                    }
                    let o = self.bots[&cid].clone();
                    if o.role != ec.respond.role || !self.event_free(&o) {
                        continue;
                    }
                    let Some(loc) =
                        self.locate(&o, &ec.respond.activity, Some(slot.as_str()), None)
                    else {
                        continue;
                    };
                    self.release(cid);
                    let mut rng = self.rng_for(cid);
                    let d = self.instantiate(
                        &o,
                        &mut rng,
                        Kind::Event,
                        &ec.respond.activity,
                        &loc,
                        Overrides::default(),
                    );
                    self.commit(cid, d);
                    self.assigned.insert(cid);
                    n += 1.0;
                }
            }
            EventIn::Payment { to } => {
                let Some(payee) = self.bots.get(to).cloned() else {
                    return;
                };
                let carrier = self
                    .bots
                    .values()
                    .filter(|o| o.role == ec.payment.role && self.event_free(o))
                    .min_by(|x, y| {
                        let dx = self.distance(&payee.at, &x.at);
                        let dy = self.distance(&payee.at, &y.at);
                        dx.partial_cmp(&dy)
                            .unwrap_or(Ordering::Equal)
                            .then(x.cid.cmp(&y.cid))
                    })
                    .cloned();
                let Some(f) = carrier else { return };
                self.carry_to(&f, &payee, &ec.payment.activity, &ec.payment.carry);
            }
            EventIn::Deliver { from, to, item } => {
                let (Some(src), Some(dst)) =
                    (self.bots.get(from).cloned(), self.bots.get(to).cloned())
                else {
                    return;
                };
                if !self.event_free(&src) {
                    return;
                }
                let item = item.clone().unwrap_or_else(|| ec.deliver.carry.clone());
                self.carry_to(&src, &dst, &ec.deliver.activity, &item);
            }
        }
    }

    /// A robot carries `item` to `to`'s desk (or to where `to` stands when it has no desk).
    fn carry_to(&mut self, c: &BotIn, to: &BotIn, act_id: &str, item: &str) {
        let goal = to
            .desk
            .as_deref()
            .filter(|k| !k.is_empty())
            .and_then(|k| self.spot_by_key.get(k))
            .map(|&i| spot_goal(&self.spots[i]))
            .unwrap_or_else(|| here_goal(to));
        self.release(c.cid);
        let mut rng = self.rng_for(c.cid);
        let loc = Loc::Point {
            goal,
            d: self.distance(&c.at, &goal_pos(&goal)),
        };
        let o = Overrides {
            carry: Some(item.to_string()),
            ..Default::default()
        };
        let d = self.instantiate(c, &mut rng, Kind::Event, act_id, &loc, o);
        self.commit(c.cid, d);
        self.assigned.insert(c.cid);
    }
}

// --- Scenario fixtures: 100 seeded scenarios, hashed. Both apps generate the same inputs from the seed. ---

pub const SCENARIO_COUNT: usize = 100;
pub const SCENARIO_TICKS: usize = 40;
pub const SCENARIO_DT: f64 = 0.5;

const SCENARIO_KINDS: &[&str] = &[
    "commons",
    "dock",
    "trace",
    "anywhere",
    "prop:drafting-table",
    "prop:lab-bench",
    "prop:whiteboard",
    "prop:counter",
    "prop:podium",
    "prop:crate-stack",
    "prop:plant-bed",
    "prop:book-cart",
    "prop:microscope-desk",
    "component:cpu",
    "component:gpu",
    "component:memory",
    "component:network",
    "component:bmc",
    "component:io",
];
const SCENARIO_ROLES: &[&str] = &[
    "orchestrator",
    "mayor",
    "architect",
    "builder",
    "qa",
    "reviewer",
    "release",
    "edge-operator",
    "incident",
    "security-reviewer",
    "librarian",
    "researcher",
    "planner",
    "finance",
    "auditor",
    "breeder",
    "other",
];
const SCENARIO_TOOLS: &[&str] = &[
    "plaza_post_task",
    "bash",
    "memory_write",
    "fetch_url",
    "plaza_git",
    "plaza_verdict",
    "plaza_send_message",
    "bash_output",
    "plaza_pay",
    "read_file",
    "plaza_vote",
    "plaza_hire",
    "share_file",
    "plaza_money",
    "plaza_project_critique",
    "todo_write",
    "unknown_tool",
];
const SCENARIO_SLOTS: &[&str] = &["cpu", "gpu", "memory", "network", "bmc", "io"];

fn scenario_quirks(i: usize) -> Vec<QuirkIn> {
    match i {
        1 => vec![QuirkIn {
            clip: "owl.swivel".to_string(),
            weight: 3.0,
        }],
        2 => vec![
            QuirkIn {
                clip: "pup.wag".to_string(),
                weight: 4.0,
            },
            QuirkIn {
                clip: "pup.beg".to_string(),
                weight: 3.0,
            },
        ],
        _ => Vec::new(),
    }
}

fn pick(rng: &mut Mulberry32, n: usize) -> usize {
    (rng.next_f64() * n as f64).floor() as usize
}

fn scenario_state(r0: f64) -> BotState {
    if r0 < 0.15 {
        BotState::Offline
    } else if r0 < 0.6 {
        BotState::Idle
    } else {
        BotState::Working
    }
}

pub struct ScenarioRun {
    pub hash: String,
    pub decisions: usize,
}

/// Runs scenario `s` (0..SCENARIO_COUNT-1) through a fresh engine; the hash covers every decision of every tick.
pub fn run_scenario(cfg: &BehaviourConfig, s: u32) -> ScenarioRun {
    let q = |x: f64| -> i64 { (x * 1000.0 + 0.5).floor() as i64 };
    let mut rng = Mulberry32::new(0x0b5e_0000u32.wrapping_add(s));
    let mut spots: Vec<SpotIn> = Vec::new();
    let n_spots = 40 + pick(&mut rng, 30);
    for i in 0..n_spots {
        let kind = SCENARIO_KINDS[pick(&mut rng, SCENARIO_KINDS.len())];
        let x = rng.next_f64() * 24.0 - 12.0;
        let z = rng.next_f64() * 24.0 - 12.0;
        let region = pick(&mut rng, 3) as i64;
        let face = pick(&mut rng, 8) as f64 * 45.0;
        spots.push(SpotIn {
            key: format!("k{i:03}"),
            kind: kind.to_string(),
            region,
            x,
            z,
            face,
            clip: None,
        });
    }
    let n_bots = 3 + pick(&mut rng, 6);
    let mut bots: Vec<BotIn> = Vec::new();
    for i in 0..n_bots {
        let cid = 1 + 3 * i as i64;
        let role = SCENARIO_ROLES[pick(&mut rng, SCENARIO_ROLES.len())];
        let personality = Personality {
            social: pick(&mut rng, 10) as f64,
            energy: pick(&mut rng, 10) as f64,
            focus: pick(&mut rng, 10) as f64,
            curiosity: pick(&mut rng, 10) as f64,
            play: pick(&mut rng, 10) as f64,
            tidy: pick(&mut rng, 10) as f64,
        };
        let r0 = rng.next_f64();
        let state = scenario_state(r0);
        let last_tool = if rng.next_f64() < 0.2 {
            None
        } else {
            Some(SCENARIO_TOOLS[pick(&mut rng, SCENARIO_TOOLS.len())].to_string())
        };
        let talking_to = if rng.next_f64() < 0.2 {
            Some(1 + 3 * pick(&mut rng, n_bots) as i64)
        } else {
            None
        };
        let x = rng.next_f64() * 24.0 - 12.0;
        let z = rng.next_f64() * 24.0 - 12.0;
        let region = pick(&mut rng, 3) as i64;
        let desk = format!("d{cid}");
        let dx = rng.next_f64() * 24.0 - 12.0;
        let dz = rng.next_f64() * 24.0 - 12.0;
        let dr = pick(&mut rng, 3) as i64;
        spots.push(SpotIn {
            key: desk.clone(),
            kind: "desk".to_string(),
            region: dr,
            x: dx,
            z: dz,
            face: 0.0,
            clip: Some("type".to_string()),
        });
        let quirks = scenario_quirks(pick(&mut rng, 3));
        bots.push(BotIn {
            cid,
            role: role.to_string(),
            personality,
            state,
            last_tool,
            talking_to,
            at: Pos { x, z, region },
            desk: Some(desk),
            quirks,
            arrived: None,
            avoid: Vec::new(),
        });
    }

    let mut engine = Behaviour::new(cfg);
    let mut h = FNV_OFFSET;
    let mut n = 0usize;
    for t in 0..SCENARIO_TICKS {
        let now = t as f64 * SCENARIO_DT;
        if t > 0 {
            for b in bots.iter_mut() {
                if rng.next_f64() < 0.12 {
                    let r = rng.next_f64();
                    b.state = scenario_state(r);
                    b.last_tool = if rng.next_f64() < 0.2 {
                        None
                    } else {
                        Some(SCENARIO_TOOLS[pick(&mut rng, SCENARIO_TOOLS.len())].to_string())
                    };
                    b.talking_to = if rng.next_f64() < 0.15 {
                        Some(1 + 3 * pick(&mut rng, n_bots) as i64)
                    } else {
                        None
                    };
                }
            }
        }
        let held = s % 5 == 0 && t >= 20;
        let mut events: Vec<EventIn> = Vec::new();
        if rng.next_f64() < 0.2 {
            let kind = pick(&mut rng, 5);
            if kind == 0 {
                let cid = bots[pick(&mut rng, n_bots)].cid;
                events.push(EventIn::Celebrate { cid });
            } else if kind == 1 {
                let conductor = bots[pick(&mut rng, n_bots)].cid;
                let mut members: Vec<i64> = Vec::new();
                for b in &bots {
                    if rng.next_f64() < 0.5 {
                        members.push(b.cid);
                    }
                }
                events.push(EventIn::Standup { conductor, members });
            } else if kind == 2 {
                let slot = SCENARIO_SLOTS[pick(&mut rng, SCENARIO_SLOTS.len())].to_string();
                events.push(EventIn::Respond { slot });
            } else if kind == 3 {
                let to = bots[pick(&mut rng, n_bots)].cid;
                events.push(EventIn::Payment { to });
            } else {
                let from = bots[pick(&mut rng, n_bots)].cid;
                let to = bots[pick(&mut rng, n_bots)].cid;
                let item = if rng.next_f64() < 0.5 {
                    "crate"
                } else {
                    "coin-bag"
                };
                events.push(EventIn::Deliver {
                    from,
                    to,
                    item: Some(item.to_string()),
                });
            }
        }
        let input = TickIn {
            now,
            robots: bots.clone(),
            spots: spots.clone(),
            held,
            events,
        };
        for d in engine.step(input) {
            let goal = match &d.goal {
                Some(g) => {
                    let face = match g.face {
                        Some(f) => q(f).to_string(),
                        None => "-".to_string(),
                    };
                    format!("{},{},{},{}", q(g.x), q(g.z), g.region, face)
                }
                None => "-".to_string(),
            };
            let line = format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                d.cid,
                d.activity,
                d.clip,
                d.tool.as_deref().unwrap_or("-"),
                d.carry.as_deref().unwrap_or("-"),
                d.spot.as_deref().unwrap_or("-"),
                goal,
                d.partner.map_or("-".to_string(), |p| p.to_string()),
                q(d.until),
                if d.changed { 1 } else { 0 },
            );
            h = fnv_append(h, &format!("{line}\n"));
            n += 1;
        }
    }
    ScenarioRun {
        hash: hex8(h),
        decisions: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG_JSON: &str = include_str!("../../../../assets/robots/behaviour.json");
    const FIXTURES_JSON: &str = include_str!("../../../../assets/robots/behaviour-fixtures.json");
    const INDEX_JSON: &str = include_str!("../../../../assets/robots/index.json");
    const CLIPS_JSON: &str = include_str!("../../../../assets/robots/clips.json");

    #[derive(Deserialize)]
    struct Fixtures {
        hashes: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Index {
        tools: Vec<String>,
        props: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Clips {
        clips: BTreeMap<String, serde_json::Value>,
    }

    /// The board the floor builds for these tests: one spot per kind, plus each robot's desk.
    const ALL_KINDS: &[&str] = &[
        "commons",
        "commons",
        "commons",
        "dock",
        "dock",
        "dock",
        "trace",
        "trace",
        "anywhere",
        "anywhere",
        "anywhere",
        "anywhere",
        "prop:drafting-table",
        "prop:easel",
        "prop:lab-bench",
        "prop:microscope-desk",
        "prop:workbench",
        "prop:whiteboard",
        "prop:book-cart",
        "prop:counter",
        "prop:podium",
        "prop:crate-stack",
        "prop:plant-bed",
        "component:cpu",
        "component:gpu",
        "component:memory",
        "component:network",
        "component:bmc",
        "component:io",
    ];

    fn cfg() -> BehaviourConfig {
        serde_json::from_str(CONFIG_JSON).expect("behaviour.json parses")
    }

    fn board(bots: &[BotIn]) -> Vec<SpotIn> {
        let mut spots: Vec<SpotIn> = ALL_KINDS
            .iter()
            .enumerate()
            .map(|(i, kind)| SpotIn {
                key: format!("s{i:02}"),
                kind: kind.to_string(),
                region: (i % 2) as i64,
                x: (i % 7) as f64 * 3.0 - 9.0,
                z: (i / 7) as f64 * 3.0 - 6.0,
                face: 0.0,
                clip: None,
            })
            .collect();
        for b in bots {
            if let Some(desk) = b.desk.as_deref().filter(|d| !d.is_empty()) {
                spots.push(SpotIn {
                    key: desk.to_string(),
                    kind: "desk".to_string(),
                    region: b.at.region,
                    x: b.at.x + 0.6,
                    z: b.at.z,
                    face: 0.0,
                    clip: Some("type".to_string()),
                });
            }
        }
        spots
    }

    /// A robot at its defaults: idle, all personality traits 5, no tool call, no chat, desk `d<cid>`, no quirks.
    fn bot(cid: i64, role: &str) -> BotIn {
        BotIn {
            cid,
            role: role.to_string(),
            personality: five(),
            state: BotState::Idle,
            last_tool: None,
            talking_to: None,
            at: Pos {
                x: (cid % 9) as f64 - 4.0,
                z: 0.0,
                region: 0,
            },
            desk: Some(format!("d{cid}")),
            quirks: Vec::new(),
            arrived: None,
            avoid: Vec::new(),
        }
    }

    fn five() -> Personality {
        Personality {
            social: 5.0,
            energy: 5.0,
            focus: 5.0,
            curiosity: 5.0,
            play: 5.0,
            tidy: 5.0,
        }
    }

    fn step(e: &mut Behaviour<'_>, now: f64, robots: &[BotIn]) -> Vec<Decision> {
        let spots = board(robots);
        e.step(TickIn {
            now,
            robots: robots.to_vec(),
            spots,
            held: false,
            events: Vec::new(),
        })
    }

    fn step_full(
        e: &mut Behaviour<'_>,
        now: f64,
        robots: &[BotIn],
        spots: Vec<SpotIn>,
        held: bool,
        events: Vec<EventIn>,
    ) -> Vec<Decision> {
        e.step(TickIn {
            now,
            robots: robots.to_vec(),
            spots,
            held,
            events,
        })
    }

    /// A tool name for a category: the category's first exact tool, or an unknown one for "default".
    fn tool_for(cfg: &BehaviourConfig, cat: &str) -> String {
        if cat == "default" {
            return "unknown_tool".to_string();
        }
        cfg.tools
            .iter()
            .find(|t| t.category == cat && !t.pattern.ends_with('*'))
            .expect("a tool exists for the category")
            .pattern
            .clone()
    }

    /// The largest group: robots linked by partner pointers (a pair is 2, a standup or cheer is a star).
    fn largest_group(out: &[Decision]) -> usize {
        fn find(parent: &BTreeMap<i64, i64>, x: i64) -> i64 {
            let mut r = x;
            while parent[&r] != r {
                r = parent[&r];
            }
            r
        }
        let mut parent: BTreeMap<i64, i64> = out.iter().map(|d| (d.cid, d.cid)).collect();
        for d in out {
            if let Some(p) = d.partner {
                if parent.contains_key(&p) {
                    let a = find(&parent, d.cid);
                    let b = find(&parent, p);
                    parent.insert(a, b);
                }
            }
        }
        let mut sizes: BTreeMap<i64, usize> = BTreeMap::new();
        for d in out {
            *sizes.entry(find(&parent, d.cid)).or_insert(0) += 1;
        }
        sizes.values().copied().max().unwrap_or(0)
    }

    fn count_first(cfg: &BehaviourConfig, act: &str, personality: Personality) -> usize {
        let mut n = 0;
        for cid in 1..=300 {
            let mut e = Behaviour::new(cfg);
            let b = BotIn {
                personality,
                ..bot(cid, "other")
            };
            if step(&mut e, 0.0, &[b])[0].activity == act {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn behaviour_data_every_name_exists() {
        let cfg = cfg();
        let index: Index = serde_json::from_str(INDEX_JSON).expect("index.json parses");
        let clips: Clips = serde_json::from_str(CLIPS_JSON).expect("clips.json parses");
        let tool_ids: BTreeSet<&str> = index.tools.iter().map(String::as_str).collect();
        let prop_ids: BTreeSet<&str> = index.props.iter().map(String::as_str).collect();
        let mut bad: Vec<String> = Vec::new();
        for (r, entry) in &cfg.roles {
            let RoleEntry::Def(role) = entry else {
                continue;
            };
            for t in &role.toolbox {
                if !tool_ids.contains(t.as_str()) {
                    bad.push(format!("{r}: tool {t}"));
                }
            }
            for p in &role.props {
                if !prop_ids.contains(p.as_str()) {
                    bad.push(format!("{r}: prop {p}"));
                }
            }
            if !role.work.contains_key("default") {
                bad.push(format!("{r}: no default work"));
            }
            for (cat, entries) in &role.work {
                if cat != "default" && !cfg.categories.contains(cat) {
                    bad.push(format!("{r}: category {cat}"));
                }
                for e in entries {
                    if !cfg.activities.contains_key(&e.activity) {
                        bad.push(format!("{r}: activity {}", e.activity));
                    }
                }
            }
        }
        for (id, a) in &cfg.activities {
            for t in &a.tools {
                if t != "$first" && !tool_ids.contains(t.as_str()) {
                    bad.push(format!("{id}: tool {t}"));
                }
            }
            if !a.clip.starts_with('@') && !clips.clips.contains_key(&a.clip) {
                bad.push(format!("{id}: clip {}", a.clip));
            }
            if a.dur.len() != 2 || a.dur[0] > a.dur[1] {
                bad.push(format!("{id}: duration"));
            }
        }
        for d in &cfg.idle {
            if !cfg.activities.contains_key(&d.activity) && d.activity != "quirk" {
                bad.push(format!("idle: {}", d.activity));
            }
        }
        for t in &cfg.tools {
            if !cfg.categories.contains(&t.category) {
                bad.push(format!("tool {}: category {}", t.pattern, t.category));
            }
        }
        assert_eq!(
            bad,
            Vec::<String>::new(),
            "behaviour data: every role, activity, tool, prop and clip exists"
        );
    }

    #[test]
    fn behaviour_work_is_chosen_from_the_roles_table() {
        let cfg = cfg();
        let mut bad: Vec<String> = Vec::new();
        let mut choices = 0;
        for (role, entry) in &cfg.roles {
            let RoleEntry::Def(def) = entry else { continue };
            let mut cats: Vec<String> = cfg.categories.clone();
            cats.push("default".to_string());
            for cat in &cats {
                // think is the fallback when nothing in the table can be done.
                let table = def
                    .work
                    .get(cat)
                    .or_else(|| def.work.get("default"))
                    .expect("default work");
                let mut allowed: BTreeSet<&str> =
                    table.iter().map(|e| e.activity.as_str()).collect();
                allowed.insert("think");
                for cid in 1..=24 {
                    let mut b = bot(cid, role);
                    b.state = BotState::Working;
                    b.last_tool = Some(tool_for(&cfg, cat));
                    let mut e = Behaviour::new(&cfg);
                    let out = step(&mut e, 0.0, &[b]);
                    choices += 1;
                    if !allowed.contains(out[0].activity.as_str()) {
                        bad.push(format!("{role}/{cat} cid {cid}: {}", out[0].activity));
                    }
                }
            }
        }
        assert_eq!(
            bad,
            Vec::<String>::new(),
            "behaviour: work is chosen from the role's table ({choices} cases)"
        );
    }

    #[test]
    fn behaviour_builder_testing_tinkers_and_probes() {
        let cfg = cfg();
        let mut acts: BTreeSet<String> = BTreeSet::new();
        for cid in 1..=200 {
            let mut b = bot(cid, "builder");
            b.state = BotState::Working;
            b.last_tool = Some("bash_output".to_string());
            let mut e = Behaviour::new(&cfg);
            acts.insert(step(&mut e, 0.0, &[b])[0].activity.clone());
        }
        let got: Vec<String> = acts.into_iter().collect();
        assert_eq!(
            got,
            vec!["probe".to_string(), "tinker".to_string()],
            "a builder testing tinkers (60) and probes (40)"
        );
    }

    #[test]
    fn behaviour_working_without_a_tool_call_thinks() {
        let cfg = cfg();
        let mut b = bot(5, "builder");
        b.state = BotState::Working;
        b.last_tool = None;
        let mut e = Behaviour::new(&cfg);
        assert_eq!(step(&mut e, 0.0, &[b])[0].activity, "think");
    }

    #[test]
    fn behaviour_high_energy_robots_wander_more() {
        let cfg = cfg();
        let hi = count_first(
            &cfg,
            "wander",
            Personality {
                energy: 9.0,
                ..five()
            },
        );
        let lo = count_first(
            &cfg,
            "wander",
            Personality {
                energy: 0.0,
                ..five()
            },
        );
        assert!(
            hi > lo,
            "high-energy robots wander more ({hi} vs {lo} of 300)"
        );
    }

    #[test]
    fn behaviour_tired_robots_nap_more() {
        let cfg = cfg();
        let lo_energy = count_first(
            &cfg,
            "nap",
            Personality {
                energy: 9.0,
                ..five()
            },
        );
        let hi_energy = count_first(
            &cfg,
            "nap",
            Personality {
                energy: 0.0,
                ..five()
            },
        );
        assert!(
            hi_energy > lo_energy,
            "tired robots nap more ({hi_energy} vs {lo_energy} of 300)"
        );
    }

    #[test]
    fn behaviour_no_two_robots_hold_one_spot() {
        let cfg = cfg();
        let mut bots: Vec<BotIn> = Vec::new();
        for cid in 1..=12 {
            let mut b = bot(
                cid,
                if cid % 3 == 0 {
                    "builder"
                } else {
                    "researcher"
                },
            );
            b.state = if cid % 2 == 1 {
                BotState::Working
            } else {
                BotState::Idle
            };
            b.last_tool = if cid % 2 == 1 {
                Some("bash".to_string())
            } else {
                None
            };
            bots.push(b);
        }
        let spots = board(&bots);
        let keys: BTreeSet<&str> = spots.iter().map(|s| s.key.as_str()).collect();
        let mut e = Behaviour::new(&cfg);
        let mut clash = 0;
        let mut unknown = 0;
        for t in 0..30 {
            let mut taken: BTreeSet<String> = BTreeSet::new();
            for d in step_full(
                &mut e,
                t as f64 * 0.5,
                &bots,
                spots.clone(),
                false,
                Vec::new(),
            ) {
                let Some(spot) = d.spot else { continue };
                if taken.contains(&spot) {
                    clash += 1;
                }
                if !keys.contains(spot.as_str()) {
                    unknown += 1;
                }
                taken.insert(spot);
            }
        }
        assert_eq!(
            (clash, unknown),
            (0, 0),
            "no two robots hold one spot (30 ticks, 12 robots)"
        );
    }

    /// Eight robots in one spot: the crowd a celebration or a standup draws from.
    fn crowd() -> Vec<BotIn> {
        (1..=8)
            .map(|cid| BotIn {
                at: Pos {
                    x: 0.0,
                    z: 0.0,
                    region: 0,
                },
                ..bot(cid, "other")
            })
            .collect()
    }

    #[test]
    fn behaviour_a_celebration_gets_three_cheers() {
        let cfg = cfg();
        let crowd = crowd();
        let mut e = Behaviour::new(&cfg);
        let spots = board(&crowd);
        let cheer = step_full(
            &mut e,
            0.0,
            &crowd,
            spots,
            false,
            vec![EventIn::Celebrate { cid: 1 }],
        );
        assert_eq!(
            cheer.iter().filter(|d| d.activity == "cheer").count(),
            3,
            "a celebration gets three cheers"
        );
    }

    #[test]
    fn behaviour_a_cheer_group_is_at_most_4() {
        let cfg = cfg();
        let crowd = crowd();
        let mut e = Behaviour::new(&cfg);
        let spots = board(&crowd);
        let cheer = step_full(
            &mut e,
            0.0,
            &crowd,
            spots,
            false,
            vec![EventIn::Celebrate { cid: 1 }],
        );
        assert!(largest_group(&cheer) <= 4, "a cheer group is at most 4");
    }

    #[test]
    fn behaviour_a_standup_takes_three_members() {
        let cfg = cfg();
        let crowd = crowd();
        let mut e = Behaviour::new(&cfg);
        let spots = board(&crowd);
        let events = vec![EventIn::Standup {
            conductor: 2,
            members: vec![1, 3, 4, 5, 6, 7, 8],
        }];
        let stand = step_full(&mut e, 0.0, &crowd, spots, false, events);
        assert_eq!(
            stand.iter().filter(|d| d.activity == "standup").count(),
            3,
            "a standup takes three members"
        );
    }

    #[test]
    fn behaviour_a_standup_group_is_at_most_4() {
        let cfg = cfg();
        let crowd = crowd();
        let mut e = Behaviour::new(&cfg);
        let spots = board(&crowd);
        let events = vec![EventIn::Standup {
            conductor: 2,
            members: vec![1, 3, 4, 5, 6, 7, 8],
        }];
        let stand = step_full(&mut e, 0.0, &crowd, spots, false, events);
        assert!(largest_group(&stand) <= 4, "a standup group is at most 4");
    }

    #[test]
    fn behaviour_chats_are_pairs() {
        let cfg = cfg();
        let social: Vec<BotIn> = (1..=8)
            .map(|cid| BotIn {
                personality: Personality {
                    social: 9.0,
                    focus: 2.0,
                    ..five()
                },
                at: Pos {
                    x: 0.0,
                    z: 0.0,
                    region: 0,
                },
                ..bot(cid, "other")
            })
            .collect();
        let mut e = Behaviour::new(&cfg);
        let mut worst = 0;
        for t in 0..60 {
            worst = worst.max(largest_group(&step(&mut e, t as f64 * 0.5, &social)));
        }
        assert!(worst <= 2, "chats are pairs (largest group {worst})");
    }

    #[test]
    fn behaviour_offline_goes_to_a_dock_and_powers_down() {
        let cfg = cfg();
        let mut off = bot(4, "builder");
        off.state = BotState::Offline;
        off.at = Pos {
            x: 1.0,
            z: 1.0,
            region: 0,
        };
        let spots = board(std::slice::from_ref(&off));
        let mut e = Behaviour::new(&cfg);
        let out = step_full(
            &mut e,
            0.0,
            std::slice::from_ref(&off),
            spots.clone(),
            false,
            Vec::new(),
        );
        let d = &out[0];
        let kind = spots
            .iter()
            .find(|s| Some(&s.key) == d.spot.as_ref())
            .map(|s| s.kind.clone());
        assert_eq!(
            (d.activity.as_str(), d.clip.as_str(), kind.as_deref()),
            ("powered-down", "offline", Some("dock")),
            "offline goes to a dock and powers down"
        );
    }

    #[test]
    fn behaviour_offline_without_a_dock_stays_put() {
        let cfg = cfg();
        let mut off = bot(4, "builder");
        off.state = BotState::Offline;
        off.at = Pos {
            x: 1.0,
            z: 1.0,
            region: 0,
        };
        let spots: Vec<SpotIn> = board(std::slice::from_ref(&off))
            .into_iter()
            .filter(|s| s.kind != "dock")
            .collect();
        let mut e = Behaviour::new(&cfg);
        let out = step_full(
            &mut e,
            0.0,
            std::slice::from_ref(&off),
            spots,
            false,
            Vec::new(),
        );
        let d = &out[0];
        assert_eq!(
            (d.spot.clone(), d.goal.map(|g| g.x)),
            (None, Some(1.0)),
            "offline without a dock stays put"
        );
    }

    #[test]
    fn behaviour_held_plaza_nobody_works() {
        let cfg = cfg();
        let bots: Vec<BotIn> = (1..=6)
            .map(|cid| {
                let mut b = bot(cid, if cid % 2 == 1 { "builder" } else { "librarian" });
                b.state = BotState::Working;
                b.last_tool = Some("bash".to_string());
                b
            })
            .collect();
        let idle_ids: BTreeSet<&str> = cfg.idle.iter().map(|d| d.activity.as_str()).collect();
        let mut e = Behaviour::new(&cfg);
        let mut bad: Vec<String> = Vec::new();
        for t in 0..12 {
            let events = if t == 0 {
                vec![EventIn::Celebrate { cid: 1 }]
            } else {
                Vec::new()
            };
            let spots = board(&bots);
            for d in step_full(&mut e, t as f64 * 0.5, &bots, spots, true, events) {
                if !idle_ids.contains(d.activity.as_str()) && d.activity != "powered-down" {
                    bad.push(format!("{}:{}", d.cid, d.activity));
                }
            }
        }
        assert_eq!(
            bad,
            Vec::<String>::new(),
            "held plaza, nobody works (idle activities only)"
        );
    }

    #[test]
    fn behaviour_every_pair_is_mutual() {
        let cfg = cfg();
        let roles = [
            "orchestrator",
            "reviewer",
            "other",
            "librarian",
            "builder",
            "qa",
            "incident",
            "planner",
        ];
        let tools: [Option<&str>; 5] = [
            Some("bash"),
            Some("plaza_send_message"),
            Some("plaza_verdict"),
            Some("memory_write"),
            None,
        ];
        let mut bad: Vec<String> = Vec::new();
        for seed in 0..20i64 {
            let bots: Vec<BotIn> = (1..=8i64)
                .map(|cid| {
                    let k = (cid + seed) % 5;
                    let mut b = bot(cid, roles[((cid * 3 + seed) % roles.len() as i64) as usize]);
                    b.state = if k < 2 {
                        BotState::Working
                    } else {
                        BotState::Idle
                    };
                    b.last_tool =
                        tools[((cid + seed) % tools.len() as i64) as usize].map(str::to_string);
                    b.talking_to = if cid % 2 == 1 {
                        Some((cid + seed) % 8 + 1)
                    } else {
                        None
                    };
                    b.at = Pos {
                        x: ((cid % 4) * 2 - 3) as f64,
                        z: ((seed % 3) - 1) as f64,
                        region: 0,
                    };
                    b
                })
                .collect();
            let mut e = Behaviour::new(&cfg);
            for t in 0..20 {
                let out = step(&mut e, t as f64 * 0.5 + seed as f64, &bots);
                let by: BTreeMap<i64, &Decision> = out.iter().map(|d| (d.cid, d)).collect();
                for d in &out {
                    let Some(p) = d.partner else { continue };
                    if cfg.activities[d.activity.as_str()].pairs.as_deref() == Some("review") {
                        continue;
                    }
                    if by.get(&p).and_then(|x| x.partner) != Some(d.cid) {
                        bad.push(format!(
                            "seed {seed} t {t}: {} {} -> {p}",
                            d.cid, d.activity
                        ));
                    }
                }
            }
        }
        assert_eq!(
            bad,
            Vec::<String>::new(),
            "every pair is mutual (20 seeds, 20 ticks)"
        );
    }

    #[test]
    fn behaviour_a_payment_is_carried_by_finance_to_the_payee_desk() {
        let cfg = cfg();
        let finance = BotIn {
            at: Pos {
                x: 0.0,
                z: 0.0,
                region: 0,
            },
            ..bot(1, "finance")
        };
        let payee = BotIn {
            at: Pos {
                x: 5.0,
                z: 0.0,
                region: 0,
            },
            ..bot(2, "other")
        };
        let bots = vec![finance, payee];
        let spots = board(&bots);
        let mut e = Behaviour::new(&cfg);
        let out = step_full(
            &mut e,
            0.0,
            &bots,
            spots,
            false,
            vec![EventIn::Payment { to: 2 }],
        );
        let carrier = out
            .iter()
            .find(|d| d.cid == 1)
            .expect("the finance robot decides");
        assert_eq!(
            (
                carrier.activity.as_str(),
                carrier.carry.as_deref(),
                carrier.goal.map(|g| g.x)
            ),
            ("deliver", Some("coin-bag"), Some(5.6)),
            "a payment is carried by finance to the payee desk"
        );
    }

    #[test]
    fn behaviour_same_inputs_give_same_decisions() {
        let cfg = cfg();
        let role_names: Vec<&str> = cfg
            .roles
            .iter()
            .filter(|(_, e)| matches!(e, RoleEntry::Def(_)))
            .map(|(k, _)| k.as_str())
            .collect();
        let bots: Vec<BotIn> = (1..=6)
            .map(|cid| {
                let mut b = bot(cid, role_names[cid as usize % role_names.len()]);
                b.state = if cid % 2 == 1 {
                    BotState::Working
                } else {
                    BotState::Idle
                };
                b.last_tool = Some("plaza_post_task".to_string());
                b
            })
            .collect();
        let mut a = Behaviour::new(&cfg);
        let mut b = Behaviour::new(&cfg);
        for t in 0..30 {
            let now = t as f64 * 0.5;
            let x = step(&mut a, now, &bots);
            let y = step(&mut b, now, &bots);
            assert_eq!(x, y, "the same inputs give the same decisions (tick {t})");
        }
    }

    #[test]
    fn behaviour_arrival_holds_until_then_starts_the_duration() {
        let cfg = cfg();
        let walker = BotIn {
            arrived: Some(false),
            ..bot(7, "other")
        };
        let mut e = Behaviour::new(&cfg);
        let first = step(&mut e, 0.0, std::slice::from_ref(&walker)).remove(0);
        let dur = cfg.activities[first.activity.as_str()].dur.clone();
        assert!(
            first.until.is_infinite(),
            "a walking decision holds its until"
        );
        let walking = step(&mut e, 5.0, std::slice::from_ref(&walker)).remove(0);
        assert!(
            walking.until.is_infinite(),
            "no arrival yet, so no duration"
        );
        let arrived = BotIn {
            arrived: Some(true),
            ..walker.clone()
        };
        let done = step(&mut e, 6.0, std::slice::from_ref(&arrived)).remove(0);
        assert_eq!(done.activity, first.activity, "the same decision stays");
        assert!(!done.changed, "arrival does not decide again");
        let span = done.until - 6.0;
        assert!(
            span >= dur[0] - 1e-9 && span <= dur[1] + 1e-9,
            "duration starts at arrival: {span}"
        );
    }

    #[test]
    fn behaviour_arrival_cap_starts_the_duration_after_12_seconds() {
        let cfg = cfg();
        let walker = BotIn {
            arrived: Some(false),
            ..bot(7, "other")
        };
        let mut e = Behaviour::new(&cfg);
        let first = step(&mut e, 0.0, std::slice::from_ref(&walker)).remove(0);
        let dur = cfg.activities[first.activity.as_str()].dur.clone();
        let early = step(&mut e, 11.5, std::slice::from_ref(&walker)).remove(0);
        assert!(
            early.until.is_infinite(),
            "before the cap the until is held"
        );
        let capped = step(&mut e, 12.0, std::slice::from_ref(&walker)).remove(0);
        let span = capped.until - 12.0;
        assert!(
            capped.until.is_finite() && span >= dur[0] - 1e-9 && span <= dur[1] + 1e-9,
            "the cap starts it: {span}"
        );
    }

    #[test]
    fn behaviour_new_decision_walks_when_its_goal_is_far() {
        let cfg = cfg();
        let mut far = 0;
        for cid in 1..=30 {
            let b = BotIn {
                arrived: Some(true),
                at: Pos {
                    x: -9.0,
                    z: -9.0,
                    region: 0,
                },
                ..bot(cid, "other")
            };
            let mut e = Behaviour::new(&cfg);
            let d = step(&mut e, 0.0, std::slice::from_ref(&b)).remove(0);
            if let Some(g) = d.goal {
                let dx = -9.0 - g.x;
                let dz = -9.0 - g.z;
                if (dx * dx + dz * dz).sqrt() > 0.6 {
                    far += 1;
                    assert!(
                        d.until.is_infinite(),
                        "cid {cid}: a new far goal waits for arrival"
                    );
                }
            }
        }
        assert!(far > 0, "some decisions have far goals");
    }

    #[test]
    fn behaviour_an_unreachable_spot_is_given_up_for_another() {
        let cfg = cfg();
        let mut checked = 0;
        for cid in 1..=30 {
            let b0 = bot(cid, "other");
            let mut e = Behaviour::new(&cfg);
            let d0 = step(&mut e, 0.0, std::slice::from_ref(&b0)).remove(0);
            let Some(key) = d0.spot.clone() else {
                continue;
            };
            checked += 1;
            let b1 = BotIn {
                avoid: vec![key.clone()],
                ..b0.clone()
            };
            let d1 = step(&mut e, 0.5, std::slice::from_ref(&b1)).remove(0);
            assert!(d1.changed, "cid {cid}: the held spot is decided again");
            assert_ne!(
                d1.spot.as_deref(),
                Some(key.as_str()),
                "cid {cid}: the spot is not picked again"
            );
        }
        assert!(checked > 0, "some decisions hold a spot");
    }

    #[test]
    fn behaviour_fixtures_100_scenarios() {
        let fixtures: Fixtures = serde_json::from_str(FIXTURES_JSON).expect("fixtures parse");
        assert_eq!(
            fixtures.hashes.len(),
            SCENARIO_COUNT,
            "behaviour fixtures: 100 scenarios"
        );
    }

    #[test]
    fn behaviour_fixtures_every_scenario_recomputes_its_hash() {
        let cfg = cfg();
        let fixtures: Fixtures = serde_json::from_str(FIXTURES_JSON).expect("fixtures parse");
        let mut bad: Vec<String> = Vec::new();
        for s in 0..SCENARIO_COUNT {
            let got = run_scenario(&cfg, s as u32).hash;
            if got != fixtures.hashes[s] {
                bad.push(format!(
                    "scenario {s}: {got}, fixture says {}",
                    fixtures.hashes[s]
                ));
            }
        }
        assert_eq!(
            bad,
            Vec::<String>::new(),
            "every scenario recomputes its decision hash"
        );
    }
}
