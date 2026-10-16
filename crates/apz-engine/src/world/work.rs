//! `floor3d/robot-work.ts`: the robots' work on the board. The behaviour engine (`world::behaviour`) decides each robot's
//! activity every decision tick (`scoring.decision_s` of simulation time). This module turns those decisions into what
//! the floor needs:
//!   - `goal_for(bot)`: where the robot should walk now, with a key that changes when the goal does;
//!   - `arrived(bot)`: once the robot has reached its goal, starts the activity's clip, tool and carried item (and shows
//!     the walk clip while it walks);
//!   - `facing(bot)`: the heading a station or a partner wants, or None (the floor keeps the nav pose's heading).
//!
//! The floor owns one `RobotWork`: `note_tool` reports tool calls, `bus_frame` reports hub events, and `frame(dt, bots,
//! held)` runs once per frame before the poses are read. Nothing here drives the nav world, reads wall-clock time or
//! draws random numbers: the engine's clock is the frame time.

#![allow(dead_code)] // the Floor integration consumes it next

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use serde_json::{Value, json};

use super::AgentInfo;
use super::behaviour::{
    Behaviour, BehaviourConfig, BotIn, BotState, Decision, EventIn, Personality as BehPersonality,
    Pos, QuirkIn, SpotIn, TickIn,
};
use super::board_layout::{BoardDistrict, BoardLayout, TraceGraph};
use super::spots::{
    RoomFrame, RoomItem, RoomSpot, board_spots, desk_spots, district_key, region_of, room_spots,
};
use crate::hardware::{Model, Params};
use crate::robot::Robot;
use crate::robots::types::Personality as SpeciesPersonality;
use crate::robots::types::Species;

static CONFIG: OnceLock<BehaviourConfig> = OnceLock::new();

/// The behaviour config (`assets/robots/behaviour.json`), parsed once.
fn config() -> &'static BehaviourConfig {
    CONFIG.get_or_init(|| {
        serde_json::from_str(include_str!("../../../../assets/robots/behaviour.json"))
            .expect("behaviour.json parses")
    })
}

/// A mover stands this far beside its partner (plaza units).
const BESIDE: f32 = 1.2;
/// A robot has arrived when it stands this close to its goal and is not moving (plaza units).
const ARRIVE: f32 = 0.6;
/// Seconds a chat message keeps its recipient as talkingTo (the engine's huddle partner), in simulation time.
const TALK_S: f64 = 8.0;
/// The board slots a failure can be responded to at (a failed work item names no component).
const FAILURE_SLOTS: [&str; 6] = ["cpu", "memory", "network", "io", "bmc", "storage"];
/// The straight-line walk speed (plaza units per second), the floor's fallback when it has no nav world.
const STRAIGHT_SPEED: f32 = 1.6;

/// The floor's robot as the work sees it: the agent's identity and status, its pose and its robot.
pub struct WorkBot {
    pub agent: AgentInfo,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub moving: bool,
    /// The robot's species (its gait, its own spots): the floor's robot drives the clips (`arrived`, `unreachable`).
    pub species: &'static Species,
}

/// Where a robot should walk now: the goal (x, z, and the yaw on arrival or None), and a key that changes exactly when
/// the goal does.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkGoal {
    pub key: String,
    pub goal: (f32, f32, Option<f32>),
    pub activity: String,
}

/// What the work keeps per robot. `seq` counts the decisions the engine changed; `started` is the seq whose activity is on.
struct Track {
    dec: Option<Decision>,
    seq: i64,
    started: i64,
    /// The goal of the current decision (set by `goal_for`), and whether there is one.
    target: (f32, f32),
    has_target: bool,
    /// A mover's side of its partner: the unit direction from the partner, fixed when the decision changes.
    side: (f32, f32),
    side_seq: i64,
    /// The clip the robot plays by hold (None: its state's clip), the tool in its grip and the item it carries.
    shown: Option<String>,
    hand: Option<String>,
    carried: Option<String>,
    last_tool: Option<String>,
    talking_to: Option<i64>,
    talk_until: f64,
    /// Spot keys the nav world could not reach for this robot: sent as its avoid list until its next changed decision.
    unreach: BTreeSet<String>,
}

impl Track {
    fn new() -> Track {
        Track {
            dec: None,
            seq: 0,
            started: -1,
            target: (0.0, 0.0),
            has_target: false,
            side: (0.0, 0.0),
            side_seq: -1,
            shown: None,
            hand: None,
            carried: None,
            last_tool: None,
            talking_to: None,
            talk_until: 0.0,
            unreach: BTreeSet::new(),
        }
    }
}

/// The last frame's view of a robot, for the partner and orchestrator lookups (the TS `byCid` map).
#[derive(Clone, Copy)]
struct Seen {
    x: f32,
    y: f32,
    orchestrator: bool,
}

fn cid_of(b: &WorkBot) -> i64 {
    b.agent.cid as i64
}

fn track_entry(tracks: &mut BTreeMap<i64, Track>, cid: i64) -> &mut Track {
    tracks.entry(cid).or_insert_with(Track::new)
}

/// The yaw from a robot to a point the TS way: `Math.atan2(dx, dz)`.
fn face_point(b: &WorkBot, p: Seen) -> f32 {
    (p.x - b.x).atan2(p.y - b.y)
}

/// A goal coordinate's key part: JS `Math.round(v * 2)`.
fn quant(v: f32) -> i64 {
    (v * 2.0 + 0.5).floor() as i64
}

/// The fallback walk: one frame of a straight line toward (gx, gz), through nothing (the nav world is what avoids
/// obstacles). Sets the robot's x, y and moving; returns the travel yaw, or None when it stands still.
pub fn straight_step(b: &mut WorkBot, gx: f32, gz: f32, dt: f32) -> Option<f32> {
    let dx = gx - b.x;
    let dz = gz - b.y;
    let dist = dx.hypot(dz);
    b.moving = dist > 0.02;
    if !b.moving {
        return None;
    }
    let step = (STRAIGHT_SPEED * dt).min(dist);
    b.x += dx / dist * step;
    b.y += dz / dist * step;
    Some(dx.atan2(dz))
}

/// A spot's nav component (-1 off the walk), cached until the next second's refresh.
fn spot_comp_of(
    cache: &mut BTreeMap<String, i64>,
    reach: Option<&dyn Fn(f32, f32) -> i64>,
    s: &SpotIn,
) -> i64 {
    *cache
        .entry(s.key.clone())
        .or_insert_with(|| reach.map_or(-1, |f| f(s.x as f32, s.z as f32)))
}

fn bot_state(status: &str) -> BotState {
    match status {
        "idle" => BotState::Idle,
        "working" => BotState::Working,
        _ => BotState::Offline,
    }
}

fn beh_personality(p: SpeciesPersonality) -> BehPersonality {
    BehPersonality {
        social: p.social as f64,
        energy: p.energy as f64,
        focus: p.focus as f64,
        curiosity: p.curiosity as f64,
        play: p.play as f64,
        tidy: p.tidy as f64,
    }
}

pub struct RobotWork {
    engine: Behaviour<'static>,
    tracks: BTreeMap<i64, Track>,
    rooms: BTreeMap<String, RoomFrame>,
    /// The board's own spots and the desks; `spot_list` is what the engine reads.
    board_list: Vec<SpotIn>,
    homes: Option<HashMap<u64, (usize, f32, f32)>>,
    districts: Vec<BoardDistrict>,
    desks: Vec<SpotIn>,
    spot_list: Vec<SpotIn>,
    desk_keys: BTreeSet<String>,
    by_cid: BTreeMap<i64, Seen>,
    prev_status: BTreeMap<i64, String>,
    pending: Vec<EventIn>,
    /// The last stage seen of each project (a stand-up is a stage change).
    stages: BTreeMap<String, String>,
    clock: f64,
    next_tick: f64,
    held: bool,
    /// The nav component of a point (-1 off the walk), from the floor; each spot's component is cached for a second.
    reach: Option<Box<dyn Fn(f32, f32) -> i64>>,
    spot_comp: BTreeMap<String, i64>,
    avoid_dbg: BTreeMap<i64, (i64, usize, usize)>,
    spot_comp_at: f64,
}

impl Default for RobotWork {
    fn default() -> Self {
        Self::new()
    }
}

impl RobotWork {
    pub fn new() -> RobotWork {
        RobotWork {
            engine: Behaviour::new(config()),
            tracks: BTreeMap::new(),
            rooms: BTreeMap::new(),
            board_list: Vec::new(),
            homes: None,
            districts: Vec::new(),
            desks: Vec::new(),
            spot_list: Vec::new(),
            desk_keys: BTreeSet::new(),
            by_cid: BTreeMap::new(),
            prev_status: BTreeMap::new(),
            pending: Vec::new(),
            stages: BTreeMap::new(),
            clock: 0.0,
            next_tick: 0.0,
            held: false,
            reach: None,
            spot_comp: BTreeMap::new(),
            avoid_dbg: BTreeMap::new(),
            spot_comp_at: f64::NEG_INFINITY,
        }
    }

    /// The rooms are about to rebuild: their regions start again.
    pub fn begin_rooms(&mut self) {
        self.rooms.clear();
    }

    /// A room was built on district `d` (its group turned by `turn`): its stations become the room's region.
    pub fn add_room(
        &mut self,
        d: &BoardDistrict,
        turn: f32,
        spots: &[RoomSpot],
        items: &[RoomItem],
    ) {
        let region = self.rooms.len() as i64 + 1;
        let frame = RoomFrame {
            region,
            d: d.clone(),
            spots: room_spots(region, d, turn, spots, items),
        };
        self.rooms.insert(district_key(&d.district), frame);
        self.recombine();
    }

    /// The board's spots (components, commons, docks, traces, anywhere) for this board and layout. Call after the rooms.
    pub fn set_board(
        &mut self,
        model: &Model,
        params: &Params,
        graph: &TraceGraph,
        layout: &BoardLayout,
    ) {
        self.board_list = board_spots(model, params, graph, layout);
        self.recombine();
    }

    /// The desks of the layout's agents. Call on every layout update (agents come and go without a rebuild).
    pub fn set_layout(&mut self, layout: &BoardLayout) {
        self.homes = Some(layout.homes.clone());
        self.districts = layout.districts.clone();
        self.recombine();
    }

    /// The nav component of a point (-1 off the walk), from the nav world: a robot's and a spot's reachability.
    pub fn set_reach(&mut self, component: Box<dyn Fn(f32, f32) -> i64>) {
        self.reach = Some(component);
        self.spot_comp.clear();
    }

    /// The floor reports a tool call an agent made (its transcript's tool_request).
    pub fn note_tool(&mut self, cid: i64, tool: &str, inp: &Value) {
        let clock = self.clock;
        let to_cid = if tool == "plaza_send_message" {
            inp.get("to_cid").and_then(Value::as_i64)
        } else {
            None
        };
        let tr = track_entry(&mut self.tracks, cid);
        tr.last_tool = Some(tool.to_string());
        match to_cid {
            Some(to) => {
                tr.talking_to = Some(to);
                tr.talk_until = clock + TALK_S;
            }
            None => tr.talking_to = None,
        }
        if tool == "plaza_pay"
            && let Some(to) = inp.get("to").and_then(Value::as_i64)
        {
            self.pending.push(EventIn::Payment { to });
        }
    }

    /// A bus frame from the hub (the floor subscribes): a project's stage or lead (a stand-up), a shared or delivered
    /// file (a delivery to the orchestrator), or a failed work item (a failure to respond to). Queued for the next tick.
    pub fn bus_frame(&mut self, kind: &str, data: &Value) {
        let Some(d) = data.as_object() else { return };
        match kind {
            "project" => {
                let id = d.get("id").and_then(Value::as_str);
                let lead = d.get("lead_cid").and_then(Value::as_i64);
                let stage = d.get("stage").and_then(Value::as_str).unwrap_or("");
                let (Some(id), Some(lead)) = (id, lead) else {
                    return;
                };
                if self.stages.get(id).map(String::as_str) == Some(stage) {
                    return;
                }
                self.stages.insert(id.to_string(), stage.to_string());
                // The members: the lowest-numbered robots that are on the board, other than the lead (the feed names none).
                let max = config().events.standup.max as usize;
                let members: Vec<i64> = self
                    .by_cid
                    .keys()
                    .copied()
                    .filter(|&c| c != lead)
                    .take(max)
                    .collect();
                self.pending.push(EventIn::Standup {
                    conductor: lead,
                    members,
                });
            }
            "file" => {
                if d.get("deleted").and_then(Value::as_bool) == Some(true) {
                    return;
                }
                let Some(author) = d.get("author_cid").and_then(Value::as_i64) else {
                    return;
                };
                if let Some(to) = self.orchestrator_cid()
                    && to != author
                {
                    self.pending.push(EventIn::Deliver {
                        from: author,
                        to,
                        item: Some("crate".into()),
                    });
                }
            }
            "work" => {
                if d.get("state").and_then(Value::as_str) != Some("ended")
                    || d.get("status").and_then(Value::as_str) != Some("failed")
                {
                    return;
                }
                let Some(work) = d.get("work").and_then(Value::as_i64) else {
                    return;
                };
                // The feed names no component: the failure's slot is picked by its work id (a stable spread over the slots).
                let slot = FAILURE_SLOTS[work.rem_euclid(FAILURE_SLOTS.len() as i64) as usize];
                self.pending.push(EventIn::Respond {
                    slot: slot.to_string(),
                });
            }
            _ => {}
        }
    }

    /// The orchestrator's cid (the lead of the plaza's work), or None when it is not on the board.
    fn orchestrator_cid(&self) -> Option<i64> {
        self.by_cid
            .iter()
            .find(|(_, s)| s.orchestrator)
            .map(|(c, _)| *c)
    }

    /// One frame, before the poses are read: the engine's clock, the status events, and a decision tick when due.
    /// `held` is the plaza's STOP ALL state.
    pub fn frame(&mut self, dt: f32, bots: &[WorkBot], held: bool) {
        self.clock += dt as f64;
        self.held = held;
        self.by_cid.clear();
        for b in bots {
            let seen = Seen {
                x: b.x,
                y: b.y,
                orchestrator: b.agent.tier == "orchestrator",
            };
            self.by_cid.insert(cid_of(b), seen);
        }
        let cids: BTreeSet<i64> = bots.iter().map(cid_of).collect();
        self.tracks.retain(|c, _| cids.contains(c));
        self.prev_status.retain(|c, _| cids.contains(c));
        for b in bots {
            let cid = cid_of(b);
            if self.prev_status.get(&cid).map(String::as_str) == Some("working")
                && b.agent.status == "idle"
            {
                self.pending.push(EventIn::Celebrate { cid });
            }
            self.prev_status.insert(cid, b.agent.status.clone());
        }
        if self.clock >= self.next_tick {
            self.next_tick = self.clock + config().scoring.decision_s;
            self.tick(bots, held);
        }
    }

    /// The goal the robot walks to now, or None when it has none yet. The key changes exactly when the goal does: a
    /// station's key changes with its decision; a mover's with its side of the partner (quantised, so a moving partner
    /// does not re-send the goal every frame).
    pub fn goal_for(&mut self, b: &WorkBot) -> Option<WorkGoal> {
        let cid = cid_of(b);
        let partner_seen = |d: &Decision, by_cid: &BTreeMap<i64, Seen>| {
            d.partner.and_then(|c| by_cid.get(&c).copied())
        };
        let by_cid = &self.by_cid;
        let tr = track_entry(&mut self.tracks, cid);
        let Some(d) = tr.dec.clone() else {
            tr.has_target = false;
            return None;
        };
        let p = partner_seen(&d, by_cid);
        if let Some(g) = d.goal {
            tr.target = (g.x as f32, g.z as f32);
            tr.has_target = true;
            let goal = (g.x as f32, g.z as f32, g.face.map(|f| f as f32));
            return Some(WorkGoal {
                key: tr.seq.to_string(),
                goal,
                activity: d.activity,
            });
        }
        let Some(p) = p else {
            tr.has_target = false;
            return None;
        };
        if tr.side_seq != tr.seq {
            tr.side_seq = tr.seq;
            let dx = b.x - p.x;
            let dz = b.y - p.y;
            let len = dx.hypot(dz);
            tr.side = if len > 1e-6 {
                (dx / len, dz / len)
            } else {
                (-1.0, 0.0)
            };
        }
        let x = p.x + tr.side.0 * BESIDE;
        let z = p.y + tr.side.1 * BESIDE;
        tr.target = (x, z);
        tr.has_target = true;
        let key = format!("{}:{},{}", tr.seq, quant(x), quant(z));
        Some(WorkGoal {
            key,
            goal: (x, z, None),
            activity: d.activity,
        })
    }

    /// Called every frame after the robot's pose is read. Walking shows the walk clip and puts down the last activity's
    /// tool and item; once the robot has reached its goal, the decision's activity starts (its clip, tool and carried item).
    pub fn arrived(&mut self, b: &WorkBot, robot: &mut Robot) {
        let cid = cid_of(b);
        let tr = track_entry(&mut self.tracks, cid);
        let Some(d) = tr.dec.clone() else { return };
        if tr.started == tr.seq {
            return;
        }
        let reached =
            tr.has_target && !b.moving && (b.x - tr.target.0).hypot(b.y - tr.target.1) < ARRIVE;
        if !reached {
            if tr.shown.take().is_some() {
                robot.play(None, None);
            }
            if tr.hand.take().is_some() {
                robot.hold(None);
            }
            if tr.carried.take().is_some() {
                robot.carry(None);
            }
            return;
        }
        tr.started = tr.seq;
        let clip = if d.clip == "@walk" {
            None
        } else {
            Some(d.clip.clone())
        };
        robot.play(clip.as_deref(), Some(true));
        tr.shown = clip;
        robot.hold(d.tool.as_deref());
        tr.hand = d.tool;
        robot.carry(d.carry.as_deref());
        tr.carried = d.carry;
    }

    /// The heading a station or a partner wants (the floor eases to it), or None (the floor keeps the nav pose's heading):
    /// a host faces its partner; a mover faces its partner once it stands beside it; a station faces its arrival yaw.
    pub fn facing(&mut self, b: &WorkBot) -> Option<f32> {
        let cid = cid_of(b);
        let by_cid = &self.by_cid;
        let tr = track_entry(&mut self.tracks, cid);
        let d = tr.dec.as_ref()?;
        if let Some(p) = d.partner.and_then(|c| by_cid.get(&c).copied()) {
            if d.goal.is_some() {
                return Some(face_point(b, p));
            }
            return if tr.started == tr.seq {
                Some(face_point(b, p))
            } else {
                None
            };
        }
        if tr.started == tr.seq
            && let Some(face) = d.goal.and_then(|g| g.face)
        {
            return Some(face as f32);
        }
        None
    }

    /// Whether a robot is in a chat or a huddle right now (its label shows the talk dots).
    pub fn talking(&self, cid: i64) -> bool {
        self.tracks
            .get(&cid)
            .and_then(|t| t.dec.as_ref())
            .is_some_and(|d| {
                d.partner.is_some() && (d.activity == "chat" || d.activity == "huddle")
            })
    }

    /// The nav world gave up a robot's walk (no progress for a while; its goal is cleared). The robot's spot goes on its
    /// avoid list until its next changed decision, so the engine picks another spot. The decision counts as arrived where
    /// the robot stands, so a new decision's clock starts when the robot reaches its new goal. Its tool and item are put
    /// down. `bot` is the robot, when the floor has it (the put-down needs it).
    pub fn unreachable(&mut self, cid: i64, robot: Option<&mut Robot>) {
        let Some(tr) = self.tracks.get_mut(&cid) else {
            return;
        };
        let Some(spot) = tr.dec.as_ref().map(|d| d.spot.clone()) else {
            return;
        };
        if let Some(s) = spot {
            tr.unreach.insert(s);
        }
        tr.has_target = false;
        tr.started = tr.seq;
        let mut robot = robot;
        if tr.shown.take().is_some()
            && let Some(r) = robot.as_mut()
        {
            r.play(None, None);
        }
        if tr.hand.take().is_some()
            && let Some(r) = robot.as_mut()
        {
            r.hold(None);
        }
        if tr.carried.take().is_some()
            && let Some(r) = robot.as_mut()
        {
            r.carry(None);
        }
    }

    /// Debug (window.__apzBots): what the work holds for a robot, and the engine clock and held flag. Read-only.
    pub fn debug_of(&self, cid: i64) -> Value {
        let tr = self.tracks.get(&cid);
        let d = tr.and_then(|t| t.dec.as_ref());
        json!({
            "clock": self.clock,
            "held": self.held,
            "seq": tr.map(|t| t.seq),
            "started": tr.map(|t| t.started),
            "hasTarget": tr.map(|t| t.has_target),
            "unreach": tr.map(|t| t.unreach.iter().cloned().collect::<Vec<String>>()),
            "avoidDbg": self.avoid_dbg.get(&cid).map(|&(comp, local, avoid)| json!({"comp": comp, "local": local, "avoid": avoid})),
            "target": tr.map(|t| json!({"x": t.target.0, "z": t.target.1})),
            "shown": tr.and_then(|t| t.shown.clone()),
            "hand": tr.and_then(|t| t.hand.clone()),
            "carried": tr.and_then(|t| t.carried.clone()),
            "dec": d.map(|d| json!({
                "activity": d.activity,
                "clip": d.clip,
                "tool": d.tool,
                "carry": d.carry,
                "spot": d.spot,
                "goal": d.goal.map(|g| json!({"x": g.x, "z": g.z, "region": g.region, "face": g.face})),
                "partner": d.partner,
                "until": d.until,
                "changed": d.changed,
            })),
        })
    }

    /// Rebuilds the spot list. Desks are recomputed here too: a room built after the layout gives its desks their region.
    fn recombine(&mut self) {
        self.desks = match &self.homes {
            Some(homes) => {
                let rooms = &self.rooms;
                desk_spots(homes, &self.districts, |d| {
                    rooms.get(&district_key(d)).map_or(0, |r| r.region)
                })
            }
            None => Vec::new(),
        };
        self.desk_keys = self.desks.iter().map(|s| s.key.clone()).collect();
        let mut all: Vec<SpotIn> = self.board_list.clone();
        all.extend(self.desks.iter().cloned());
        for r in self.rooms.values() {
            all.extend(r.spots.iter().cloned());
        }
        self.spot_list = all;
    }

    /// The engine's input from the floor's robots, then its decisions (a changed decision starts a new activity).
    fn tick(&mut self, bots: &[WorkBot], held: bool) {
        // A spot's nav component is read once a second (spots do not move); a robot's own is read every tick.
        if self.clock - self.spot_comp_at >= 1.0 {
            self.spot_comp_at = self.clock;
            self.spot_comp.clear();
        }
        let mut robots: Vec<BotIn> = Vec::with_capacity(bots.len());
        for b in bots {
            let cid = cid_of(b);
            let tr = track_entry(&mut self.tracks, cid);
            let last_tool = tr.last_tool.clone();
            let talking_to = tr.talking_to.filter(|_| self.clock < tr.talk_until);
            let arrived = tr.started == tr.seq || !tr.has_target;
            let unreach = tr.unreach.clone();
            let sp = b.species;
            let desk = format!("desk:{cid}");
            let avoid = self.avoid_for(b, &unreach);
            robots.push(BotIn {
                cid,
                role: if b.agent.tier == "orchestrator" {
                    "orchestrator".to_string()
                } else {
                    b.agent.role.clone()
                },
                personality: beh_personality(sp.personality),
                state: bot_state(&b.agent.status),
                last_tool,
                talking_to,
                at: Pos {
                    x: b.x as f64,
                    z: b.y as f64,
                    region: region_of(self.rooms.values(), b.x, b.y),
                },
                desk: if self.desk_keys.contains(&desk) {
                    Some(desk)
                } else {
                    None
                },
                quirks: sp
                    .quirks
                    .iter()
                    .map(|q| QuirkIn {
                        clip: q.clip.clone(),
                        weight: q.weight as f64,
                    })
                    .collect(),
                arrived: Some(arrived),
                avoid,
            });
        }
        let events = std::mem::take(&mut self.pending);
        let spots = self.spot_list.clone();
        let decisions = self.engine.step(TickIn {
            now: self.clock,
            robots,
            spots,
            held,
            events,
        });
        for d in decisions {
            let cid = d.cid;
            let changed = d.changed;
            let tr = track_entry(&mut self.tracks, cid);
            if changed {
                tr.seq += 1;
                tr.unreach.clear(); // the avoid list was used for this decision; the next one starts clean
            }
            tr.dec = Some(d);
        }
    }

    /// The spots a robot cannot be sent to: its unreachable spots, and, while it has a local spot free (one in its nav
    /// component, not unreachable), every spot outside that component. A robot with no local spot keeps only its
    /// unreachable ones (so it falls back to the rest, and never walks at a wall trying to reach an island).
    fn avoid_for(&mut self, b: &WorkBot, unreach: &BTreeSet<String>) -> Vec<String> {
        let cid = cid_of(b);
        let rc = self.reach.as_deref().map_or(-1, |f| f(b.x, b.y));
        if rc < 0 {
            let avoid: Vec<String> = unreach.iter().cloned().collect();
            return self.note_avoid(cid, rc, 0, avoid);
        }
        let mut avoid: BTreeSet<String> = unreach.clone();
        let mut local = 0usize;
        let mut free = false;
        let mut far: Vec<String> = Vec::new();
        for s in &self.spot_list {
            if spot_comp_of(&mut self.spot_comp, self.reach.as_deref(), s) == rc {
                local += 1;
                if !unreach.contains(&s.key) {
                    free = true;
                }
            } else {
                far.push(s.key.clone());
            }
        }
        if free {
            avoid.extend(far);
        }
        let out: Vec<String> = avoid.into_iter().collect();
        self.note_avoid(cid, rc, local, out)
    }

    /// Debug (window.__apzBots): a robot's component, its local spots and its avoid list length, as of the last tick.
    fn note_avoid(&mut self, cid: i64, comp: i64, local: usize, avoid: Vec<String>) -> Vec<String> {
        self.avoid_dbg.insert(cid, (comp, local, avoid.len()));
        avoid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::{Robot, robot_palette};
    use crate::world::District;

    fn room_district() -> BoardDistrict {
        BoardDistrict {
            plan: None,
            key: "dev".into(),
            district: District {
                role: "dev".into(),
                cx: 0.0,
                cy: 0.0,
                r: 3.0,
                hue: 0.0,
                round: false,
            },
            y: 0.0,
            slot: "s0".into(),
            w: 6.0,
            d: 6.0,
        }
    }

    fn bot(cid: u64, x: f32, y: f32) -> WorkBot {
        WorkBot {
            agent: AgentInfo {
                cid,
                name: format!("a{cid}"),
                role: "dev".into(),
                status: "idle".into(),
                tier: "worker".into(),
                model: "m".into(),
                species: None,
            },
            x,
            y,
            heading: 0.0,
            moving: false,
            species: Robot::new(0.0, false, &robot_palette(), 0.0).species(),
        }
    }

    #[test]
    fn robots_work_a_room_and_keep_their_goal() {
        let mut work = RobotWork::new();
        work.begin_rooms();
        let spots: Vec<RoomSpot> = vec![
            RoomSpot {
                item: 0,
                x: 1.0,
                z: 1.0,
                face: 0.0,
                clip: None,
            },
            RoomSpot {
                item: 0,
                x: -1.0,
                z: -1.0,
                face: 180.0,
                clip: None,
            },
        ];
        work.add_room(&room_district(), 0.0, &spots, &[]);
        let mut bots = vec![bot(1, 1.0, 1.0), bot(2, -1.0, -1.0)];
        for _ in 0..300 {
            work.frame(1.0 / 60.0, &bots, false);
        }
        let mut goals = Vec::new();
        for b in &bots {
            let g = work
                .goal_for(b)
                .expect("an idle robot has a goal after 5 s");
            assert!(!g.key.is_empty());
            let (gx, gz, _) = g.goal;
            let at_spot = work
                .spot_list
                .iter()
                .any(|s| (s.x as f32 - gx).abs() < 1e-3 && (s.z as f32 - gz).abs() < 1e-3);
            let beside_partner = bots
                .iter()
                .any(|o| o.agent.cid != b.agent.cid && (o.x - gx).hypot(o.y - gz) < BESIDE + 0.01);
            assert!(
                at_spot || beside_partner,
                "a goal at a board spot or beside a partner"
            );
            goals.push(g);
        }
        // A robot that stands at its goal arrives; its goal key does not change.
        let (gx, gz, _) = goals[0].goal;
        bots[0].x = gx;
        bots[0].y = gz;
        bots[0].moving = false;
        let mut robot = Robot::new(0.0, false, &robot_palette(), 0.0);
        work.arrived(&bots[0], &mut robot);
        let again = work.goal_for(&bots[0]).expect("the goal is kept");
        assert_eq!(again.key, goals[0].key);
    }
}
