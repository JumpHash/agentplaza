//! `floor.ts` lines ~700–945: each frame — robots wander, visit, chat and stroll (with ground
//! height, slopes, falling, collisions), packets fly, dust drifts, props and rooms animate.

use std::cell::RefCell;
use std::collections::HashSet;

use glam::{Quat, Vec3};
use serde_json::Value;

use super::super::hardware;
use super::super::props::Box2;
use super::super::robot::RobotState;
use super::super::scene::Obj;
use super::super::textures::Rng;
use super::board_layout::{BoardLayout, board_height_at, path_on_traces};
use super::{Bot, District, Floor, Packet, Phase, Ride, Social, display_name};

/// A working slot's glow plate at full strength (floor.ts GLOW_OPACITY).
const GLOW_OPACITY: f32 = 0.4;

const PACKET_MS: f64 = 1500.0;
const SPEED: f32 = 1.6; // grid units per second; one grid cell is one world unit
const MAX_PACKETS: usize = 60;
const GRAVITY: f32 = 9.8; // world units per second squared
const SOCIAL_NEAR: f32 = 9.5; // rooms this close count as neighbours for a visit or a chat
const STROLL_R: f32 = 3.1; // the stroll ring just outside the centre
const ROBOT_R: f32 = 0.55; // robot footprint radius for collisions
const DEG: f32 = std::f32::consts::PI / 180.0;
const STEER: [f32; 5] = [0.0, 30.0 * DEG, -30.0 * DEG, 60.0 * DEG, -60.0 * DEG]; // headings tried when blocked
const DUST_N: usize = 90;
/// What a lift ride sent: the goal (x, z) on the other level and the cids of the robots sent.
#[derive(Debug, Clone, PartialEq)]
pub struct LiftRide {
    pub goal: (f32, f32),
    pub robots: Vec<u64>,
}

/// A lift ride's time limit, and how close a ride's goal must be (stopped) to count as arrived.
const RIDE_S: f32 = 120.0;
const RIDE_ARRIVE: f32 = 0.15;
const WAVE_SECS: f32 = 1.5; // how long a selected robot waves at the camera

thread_local! {
    /// `Math.random()` for the floor: one seeded stream per thread.
    static RNG: RefCell<Rng> = RefCell::new(Rng::new(0x5eed_f10a));
}

fn random() -> f32 {
    RNG.with(|r| r.borrow_mut().next())
}

/// `str(v)`: the string, or "" for anything else.
fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}

/// `obj[key] ?? undefined`: a present, non-null field.
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| !x.is_null())
}

/// `clip(text, max)` from the UI components: cut to `max` characters with an ellipsis.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

/// Whether (x, y) lies within a robot's width of a prop footprint.
fn near(p: &Box2, x: f32, y: f32) -> bool {
    x > p.x0 - ROBOT_R && x < p.x1 + ROBOT_R && y > p.z0 - ROBOT_R && y < p.z1 + ROBOT_R
}

/// Whether two rooms are close enough for a visit or a chat.
fn nearby(a: &District, c: &District) -> bool {
    a.role == c.role || (a.cx - c.cx).hypot(a.cy - c.cy) <= SOCIAL_NEAR
}

/// The room whose cell holds (x, y), if any.
fn room_at(layout: &BoardLayout, x: f32, y: f32) -> Option<usize> {
    layout.districts.iter().position(|bd| {
        (x - bd.district.cx).abs() <= bd.w / 2.0 && (y - bd.district.cy).abs() <= bd.d / 2.0
    })
}

/// Whether bot `i` centred at (x, y) would clash with a prop, or overlap another robot.
fn blocked(bots: &[Bot], i: usize, props: &[Box2], x: f32, y: f32, check_props: bool) -> bool {
    let b = &bots[i];
    if check_props {
        for p in props {
            if near(p, x, y) && !near(p, b.x, b.y) {
                return true;
            }
        }
    }
    let partner = b.social.as_ref().and_then(|s| s.partner);
    for (j, o) in bots.iter().enumerate() {
        if j == i || Some(o.agent.cid) == partner {
            continue;
        }
        let d = (o.x - x).hypot(o.y - y);
        if d < ROBOT_R * 2.0 && d < (o.x - b.x).hypot(o.y - b.y) {
            return true;
        }
    }
    false
}

/// `recipientOf`: the cid a tool call hands a message or a task to, if any.
fn recipient_of(tool: &str, inp: &Value) -> Option<u64> {
    let to = field(inp, "to_cid").or_else(|| field(inp, "assignee"))?;
    let sends = tool == "plaza_send_message";
    let assigns = tool == "plaza_assign" || tool == "plaza_post_task";
    if sends || assigns { to.as_u64() } else { None }
}

impl Floor {
    /// One frame: `dt` seconds since the last (capped; the Active motion), `now_ms` the wall clock, and `ambient` the
    /// ambient clock in seconds with `ambient_dt` its real step. The stars, planet, dust, props and board atom read
    /// `ambient`, so they look the same at any tick rate.
    pub fn step(&mut self, dt: f32, now_ms: f64, ambient: f32, ambient_dt: f32) {
        let dt = dt.min(0.05); // the web caps a frame at 50 ms
        self.time += dt;
        self.now_ms = now_ms;
        self.update_bots(dt, now_ms);
        self.update_packets();
        self.update_dust(ambient);
        self.stars.set_rotation(0.0, ambient * 0.004, 0.0);
        self.planet.get().rotation.y = ambient * 0.05;
        self.step_board(dt, ambient, ambient_dt);
        for anim in self.prop_anim.iter_mut() {
            anim(ambient);
        }
    }

    /// Sends the `n` robots nearest lift `index` (off its plate) on a real trip through it (`__apzLiftRide`): a goal on the
    /// other level just beyond the lift, walked in place of each robot's work goal until it arrives or 120 s pass. Robots on
    /// the plate ride down to a goal on the board beyond the lift's front; robots on the board ride up to a goal on the plate
    /// beyond its edge. The ride goal is the lift's own (its yaw), so the nav world boards the robots.
    pub fn lift_ride(&mut self, index: usize, n: usize) -> Result<LiftRide, String> {
        const NO_LIFT: &str = "no nav or no such lift";
        let nav = self.nav.as_ref().ok_or(NO_LIFT)?;
        let grid = nav.world.grid();
        let l = *grid.lifts.get(index).ok_or(NO_LIFT)?;
        let (ox, oz, yaw) = (l.x, l.z, l.face);
        // The robots' level decides the way.
        let on_plate = |b: &&Bot| {
            (f64::from(b.py) - l.high).abs() < 0.05 && !self.rides.contains_key(&b.agent.cid)
        };
        let from_high = self.bots.iter().filter(|b| on_plate(b)).count() >= n.max(1);
        let sg = if from_high { 1.0 } else { -1.0 }; // +1: out toward the walkway (the board), -1: back toward the plate
        let level = if from_high { l.low } else { l.high };
        let (s, c) = (yaw.sin(), yaw.cos());
        let mut goal = None;
        let mut d = 0.3;
        while d < 3.0 && goal.is_none() {
            for lat in [0.0, 0.1, -0.1, 0.2, -0.2] {
                let x = ox + sg * s * d + c * lat;
                let z = oz + sg * c * d - s * lat;
                if let Some(cell) = nav.world.debug_cell(x, z) {
                    let side = if from_high {
                        cell.region == 0
                    } else {
                        cell.region != 0
                    };
                    if cell.walk && side && (cell.y - level).abs() < 0.03 {
                        goal = Some((x, z));
                        break;
                    }
                }
            }
            d += 0.05;
        }
        let goal =
            goal.ok_or_else(|| "no walk cell beyond the lift on the other level".to_string())?;
        let plate = if from_high { l.high } else { l.low };
        let mut near: Vec<&Bot> = self
            .bots
            .iter()
            .filter(|b| {
                (f64::from(b.py) - plate).abs() < 0.05 && !self.rides.contains_key(&b.agent.cid)
            })
            .collect();
        near.sort_by(|a, b| {
            let da = (f64::from(a.x) - ox).hypot(f64::from(a.y) - oz);
            let db = (f64::from(b.x) - ox).hypot(f64::from(b.y) - oz);
            da.total_cmp(&db)
        });
        near.truncate(n);
        let robots: Vec<u64> = near.iter().map(|b| b.agent.cid).collect();
        let goal = (goal.0 as f32, goal.1 as f32);
        for &cid in &robots {
            self.rides.insert(
                cid,
                Ride {
                    goal: [goal.0, goal.1],
                    left: RIDE_S,
                },
            );
        }
        Ok(LiftRide { goal, robots })
    }

    /// The floor's motion: Active while robots stand on it (they walk and animate their clips) or packets fly;
    /// otherwise Ambient, since the stars and the planet turn on the clock with every step.
    pub fn motion(&self) -> crate::stage::Motion {
        if !self.bots.is_empty() || !self.packets.is_empty() {
            crate::stage::Motion::Active
        } else {
            crate::stage::Motion::Ambient
        }
    }

    /// The height of the surface under (x, y): the board's, or the bare floor without a board.
    fn ground_at(&self, x: f32, y: f32) -> f32 {
        self.board
            .as_ref()
            .map_or(0.0, |b| board_height_at(x, y, &b.layout))
    }

    /// The point robot `i` walks to on the way to `goal`. A goal in another room is reached along the
    /// copper: the route is set once per goal (from the robot's place then), and its waypoints are
    /// taken in turn. Otherwise, or without a board, the goal itself.
    fn waypoint(&mut self, i: usize, goal: (f32, f32)) -> (f32, f32) {
        let Some(board) = self.board.as_ref() else {
            return goal;
        };
        let (x, y) = (self.bots[i].x, self.bots[i].y);
        let key = [goal.0, goal.1];
        if self.bots[i].route_goal != Some(key) {
            self.bots[i].route_goal = Some(key);
            self.bots[i].route.clear();
            if room_at(&board.layout, goal.0, goal.1) != Some(self.bots[i].district) {
                // The path starts at the robot's own place: that point is where it already stands.
                let path = path_on_traces(&board.graph, [x, y], key);
                self.bots[i].route = path.into_iter().skip(1).collect();
            }
        }
        let bot = &mut self.bots[i];
        while let Some(p) = bot.route.first().copied() {
            if (p[0] - bot.x).hypot(p[1] - bot.y) < 0.05 {
                bot.route.remove(0);
            } else {
                break;
            }
        }
        match bot.route.first() {
            Some(p) => (p[0], p[1]),
            None => goal,
        }
    }

    /// The board's frame: a slot whose rooms have working agents glows brighter (its glow plate too), its traces carry
    /// more pulses, and the built parts and the atom animate. A machine without a board does nothing.
    fn step_board(&mut self, dt: f32, ambient: f32, ambient_dt: f32) {
        let time = ambient;
        let Some(board) = self.board.as_mut() else {
            return;
        };
        let mut busy: HashSet<String> = HashSet::new();
        let mut working = 0usize;
        for bot in &self.bots {
            if bot.agent.status == "working" {
                working += 1;
                if let Some(bd) = board.layout.districts.get(bot.district) {
                    busy.insert(bd.slot.clone());
                }
            }
        }
        // Each slot's level eases toward 1 while one of its rooms works, toward 0 when none does.
        let ease = (dt * 4.0).min(1.0);
        for bd in &board.layout.districts {
            let target = if busy.contains(&bd.slot) { 1.0 } else { 0.0 };
            let level = board.level.entry(bd.slot.clone()).or_insert(0.0);
            *level += (target - *level) * ease;
        }
        for (slot, (mesh, mat)) in &board.glow_plates {
            let level = board.level.get(slot).copied().unwrap_or(0.0);
            mat.get().opacity = GLOW_OPACITY * level;
            mesh.set_visible(level > 0.01);
        }
        for (slot, mats) in &board.glow {
            let level = board.level.get(slot).copied().unwrap_or(0.0);
            for (mat, base) in mats {
                mat.get().emissive_intensity = base * (1.0 + 2.0 * level);
            }
        }
        for tp in board.pulses.iter_mut() {
            let end = |s: &Option<String>| {
                s.as_ref()
                    .and_then(|s| board.level.get(s))
                    .copied()
                    .unwrap_or(0.0)
            };
            let busy_end = end(&tp.from).max(end(&tp.to));
            tp.pulses.update(ambient_dt, 0.4 + 0.6 * busy_end);
        }
        let share = working as f32 / self.bots.len().max(1) as f32;
        board.built.update(
            ambient_dt,
            ambient,
            &hardware::State {
                status: board.status,
                activity: share,
                open: 1.0,
            },
        );
        if let Some(anim) = board.atom.as_mut() {
            anim(time);
        }
    }

    /// `sendPacket(from, to, tone)`: ok = the ok tone, else the accent.
    pub fn send_packet(&mut self, from: u64, to: u64, ok: bool) {
        if from == to {
            return;
        }
        let core = if ok {
            Obj::mesh(&self.shared.packet_geo, &self.shared.ok)
        } else {
            Obj::mesh(&self.shared.packet_geo, &self.shared.accent)
        };
        let halo = if ok {
            Obj::mesh(&self.shared.halo_geo, &self.shared.ok_halo)
        } else {
            Obj::mesh(&self.shared.halo_geo, &self.shared.accent_halo)
        };
        core.set_dynamic(true); // moved along its arc every frame
        halo.set_dynamic(true);
        self.root.add(&core);
        self.root.add(&halo);
        self.packets.push(Packet {
            from,
            to,
            start: self.now_ms,
            core,
            halo,
        });
        while self.packets.len() > MAX_PACKETS {
            let p = self.packets.remove(0);
            self.drop_packet(&p);
        }
    }

    /// One event from the console's stream. `kind` is the event channel the web subscribes to:
    /// "transcript" (`data` is an entry `{cid, kind, data}`: tool calls set activities and send
    /// packets, work start/end too), "delta" (`{cid}`: thinking) or "conversation" (`{from_cid,
    /// to_cid}`: an ok packet), and "project", "file" and "work" (the work's bus frames). Other channels are ignored.
    pub fn on_event(&mut self, kind: &str, data: &Value) {
        match kind {
            "transcript" => self.on_transcript(data),
            "delta" => {
                if let Some(i) = data
                    .get("cid")
                    .and_then(Value::as_u64)
                    .and_then(|c| self.index_of(c))
                {
                    self.set_activity(i, "thinking…".to_string());
                }
            }
            "conversation" => {
                let from = data.get("from_cid").and_then(Value::as_u64);
                let to = data.get("to_cid").and_then(Value::as_u64);
                if let (Some(from), Some(to)) = (from, to) {
                    self.send_packet(from, to, true);
                }
            }
            // The bus frames that give robots their stand-ups, deliveries and responses (robot-work.ts busFrame).
            "project" | "file" | "work" => self.work.bus_frame(kind, data),
            _ => {}
        }
    }

    fn index_of(&self, cid: u64) -> Option<usize> {
        self.bots.iter().position(|b| b.agent.cid == cid)
    }

    /// `nameOf(cid)`: the agent's name, else the raw value ("?" when absent).
    fn name_of(&self, v: Option<&Value>) -> String {
        if let Some(bot) = v.and_then(Value::as_u64).and_then(|c| self.index_of(c)) {
            let agent = &self.bots[bot].agent;
            return display_name(agent.cid, &agent.name);
        }
        match v {
            None | Some(Value::Null) => "?".to_string(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    }

    /// `toolActivity`: the one line that says what a tool call is doing.
    fn tool_activity(&self, tool: &str, inp: &Value) -> String {
        if tool == "bash" {
            let first = text(inp.get("command"))
                .split('\n')
                .next()
                .unwrap_or("")
                .to_string();
            return format!("$ {}", clip(&first, 120));
        }
        if tool == "plaza_send_message" {
            let who = self.name_of(inp.get("to_cid"));
            return format!("✉ to {who}: {}", clip(&text(inp.get("text")), 100));
        }
        let assignee = field(inp, "assignee");
        if tool == "plaza_assign" || (tool == "plaza_post_task" && assignee.is_some()) {
            let who = self.name_of(assignee.or_else(|| field(inp, "to_cid")));
            return format!("→ task for {who}: {}", text(inp.get("title")));
        }
        if tool == "share_file" {
            return format!("⇪ shared {}", text(inp.get("path")));
        }
        let path = text(inp.get("path"));
        let target = if path.is_empty() {
            text(inp.get("url"))
        } else {
            path
        };
        if target.is_empty() {
            format!("→ {tool}")
        } else {
            format!("→ {tool} {}", clip(&target, 80))
        }
    }

    /// The `transcript` channel: one entry from a robot's transcript.
    fn on_transcript(&mut self, e: &Value) {
        let Some(cid) = e.get("cid").and_then(Value::as_u64) else {
            return;
        };
        let bot = self.index_of(cid);
        let data = e.get("data").cloned().unwrap_or(Value::Null);
        match text(e.get("kind")).as_str() {
            "tool_request" => {
                let mut tool = text(data.get("tool"));
                if tool.is_empty() {
                    tool = text(data.get("name"));
                }
                if tool.is_empty() {
                    tool = "tool".to_string();
                }
                let inp = field(&data, "input").cloned().unwrap_or(Value::Null);
                // The work hears every tool call (robot-work.ts noteTool): a delivery or a response may follow.
                self.work.note_tool(cid as i64, &tool, &inp);
                if let Some(i) = bot {
                    let line = self.tool_activity(&tool, &inp);
                    self.set_activity(i, line);
                }
                if let Some(to) = recipient_of(&tool, &inp) {
                    self.send_packet(cid, to, false);
                }
            }
            "model_request" => {
                if let Some(i) = bot {
                    self.set_activity(i, "thinking…".to_string());
                }
            }
            "work_end" => {
                if let Some(i) = bot {
                    self.set_activity(i, "idle".to_string());
                }
            }
            "work_start" => {
                let from = data
                    .get("origin")
                    .and_then(|o| o.get("from_cid"))
                    .and_then(Value::as_u64);
                if let Some(from) = from {
                    self.send_packet(from, cid, false);
                } else if text(data.get("kind")) == "task" {
                    let boss = self
                        .bots
                        .iter()
                        .find(|b| b.agent.tier == "orchestrator")
                        .map(|b| b.agent.cid);
                    if let Some(boss) = boss {
                        self.send_packet(boss, cid, false);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_activity(&mut self, i: usize, text: String) {
        self.bots[i].activity = text;
    }

    fn drop_packet(&self, p: &Packet) {
        self.root.remove(&p.core);
        self.root.remove(&p.halo);
    }

    /// Social behaviour: an idle robot may visit a neighbour, chat, or stroll to the ring.
    fn start_social(&mut self, i: usize, now: f64) {
        self.bots[i].next_social = now + 8000.0 + random() as f64 * 12000.0;
        if random() < 0.3 {
            let (x, y) = (self.bots[i].x, self.bots[i].y);
            let ang = y.atan2(x);
            self.bots[i].social = Some(Social {
                phase: Phase::Stroll,
                partner: None,
                gx: ang.cos() * STROLL_R,
                gy: ang.sin() * STROLL_R,
                until: f64::INFINITY,
            });
            return;
        }
        let mine = self.bots[i].district;
        let pool: Vec<usize> = (0..self.bots.len())
            .filter(|&j| {
                j != i
                    && self.bots[j].agent.status == "idle"
                    && self.bots[j].social.is_none()
                    && nearby(
                        &self.districts[self.bots[j].district],
                        &self.districts[mine],
                    )
            })
            .collect();
        if pool.is_empty() {
            return;
        }
        let pick = pool[((random() * pool.len() as f32) as usize).min(pool.len() - 1)];
        let (px, py, pcid) = (
            self.bots[pick].x,
            self.bots[pick].y,
            self.bots[pick].agent.cid,
        );
        let my_cid = self.bots[i].agent.cid;
        self.bots[i].social = Some(Social {
            phase: Phase::Go,
            partner: Some(pcid),
            gx: px - 1.0,
            gy: py,
            until: f64::INFINITY,
        });
        self.bots[pick].social = Some(Social {
            phase: Phase::Talk,
            partner: Some(my_cid),
            gx: px,
            gy: py,
            until: f64::INFINITY,
        });
    }

    fn end_social(&mut self, i: usize, now: f64) {
        let me = self.bots[i].agent.cid;
        let s = self.bots[i].social.take();
        self.bots[i].next_wander = now;
        if let Some(partner) = s.and_then(|s| s.partner).and_then(|pc| self.index_of(pc)) {
            if self.bots[partner].social.as_ref().and_then(|s| s.partner) == Some(me) {
                self.bots[partner].social = None;
                self.bots[partner].next_wander = now;
            }
        }
    }

    /// Robots: wander or socialise when idle, head home when working or offline, animate, tint pads.
    fn update_bots(&mut self, dt: f32, now: f64) {
        let t = self.time;
        // A ride whose robot is gone ends with it.
        let cids: Vec<u64> = self.bots.iter().map(|b| b.agent.cid).collect();
        self.rides.retain(|cid, _| cids.contains(cid));
        let colors = self.colors;
        for light in self.lit_by_role.values_mut() {
            light.lit = false;
        }
        // A selected robot reacts once per selection: it waves at the camera (turning to face it, see below) and
        // spawns hearts, then goes back to what it was doing. Only its heading and pose change; it is not moved.
        if self.selected != self.reacted {
            self.reacted = self.selected;
            if let Some(cid) = self.selected {
                if let Some(b) = self.bots.iter_mut().find(|b| b.agent.cid == cid) {
                    b.robot.wave(WAVE_SECS);
                    b.robot.fx("hearts");
                }
            }
        }
        let eye = self.orbit.camera(1.0).eye; // the eye's position does not depend on the aspect
        // The board's navigation walks the robots (nav.rs): one step with the goals set last frame, then each robot takes
        // its pose and sends its goal below. Without it, the robots walk straight on the engine's own layout.
        let walked = match self.nav.as_mut() {
            Some(nav) => {
                let list: Vec<(u64, f32, f32)> =
                    self.bots.iter().map(|b| (b.agent.cid, b.x, b.y)).collect();
                nav.sync(&list);
                // A walk the nav gave up: the work puts its spot on the robot's avoid list and picks another.
                for cid in nav.step(dt) {
                    if let Some(b) = self.bots.iter_mut().find(|b| b.agent.cid == cid) {
                        self.work.unreachable(cid as i64, Some(&mut b.robot));
                    }
                }
                true
            }
            None => false,
        };
        // With a nav, the robots' work decides where each robot goes and what it does there (robot-work.ts), as the
        // web's floor does; the engine's own layout keeps the wander and the visits below.
        let work_bot = |b: &Bot| super::work::WorkBot {
            agent: b.agent.clone(),
            x: b.x,
            y: b.y,
            heading: b.heading,
            moving: b.moving,
            species: b.robot.species(),
        };
        if walked {
            let list: Vec<super::work::WorkBot> = self.bots.iter().map(work_bot).collect();
            self.work.frame(dt, &list, false);
        }
        for i in 0..self.bots.len() {
            let status = self.bots[i].agent.status.clone();
            let cid = self.bots[i].agent.cid;
            let roaming = status == "idle";
            let partner = self.bots[i].social.as_ref().and_then(|s| s.partner);
            let partner_gone = partner.is_some_and(|pc| match self.index_of(pc) {
                Some(j) => self.bots[j].agent.status != "idle",
                None => true,
            });
            if self.bots[i].social.is_some() && (!roaming || partner_gone) {
                self.end_social(i, now);
            }
            if roaming && self.bots[i].social.is_none() {
                if now >= self.bots[i].next_wander {
                    let (cx, cy, r) = {
                        let d = &self.districts[self.bots[i].district];
                        (d.cx, d.cy, d.r)
                    };
                    let span = (r - 0.7).max(0.5);
                    let tx = cx + (random() * 2.0 - 1.0) * span;
                    let ty = cy + (random() * 2.0 - 1.0) * span;
                    let b = &mut self.bots[i];
                    b.tx = tx;
                    b.ty = ty;
                    b.next_wander = now + 3000.0 + random() as f64 * 4000.0;
                }
                if now >= self.bots[i].next_social {
                    self.start_social(i, now);
                }
            }
            // `s` is this frame's view of the social (the TS object); the arrival step updates it.
            let mut s = self.bots[i].social.clone();
            let home = self.bots[i].home;
            let (mut gx, mut gy) = home;
            if roaming {
                gx = s.as_ref().map_or(self.bots[i].tx, |sc| sc.gx);
                gy = s.as_ref().map_or(self.bots[i].ty, |sc| sc.gy);
            }
            if walked {
                if let Some(r) = self.rides.get(&cid) {
                    (gx, gy) = (r.goal[0], r.goal[1]);
                } else if let Some(g) = self.work.goal_for(&work_bot(&self.bots[i])) {
                    (gx, gy) = (g.goal.0, g.goal.1);
                }
            }
            let pose = match self.nav.as_mut() {
                Some(nav) if walked => {
                    nav.goal(cid, gx, gy);
                    nav.pose(cid)
                }
                _ => None,
            };
            if let Some(p) = pose {
                let b = &mut self.bots[i];
                b.x = p.x as f32;
                b.y = p.z as f32;
                b.py = p.y as f32;
                b.vy = 0.0;
            }
            let (x0, y0) = (self.bots[i].x, self.bots[i].y);
            let (dx, dy) = (gx - x0, gy - y0);
            let dist = dx.hypot(dy);
            self.bots[i].moving = pose.map_or(dist > 0.02, |p| p.moving);
            // A ride ends on arrival (stopped on its goal) or when its time is up.
            if let Some(r) = self.rides.get_mut(&cid) {
                r.left -= dt;
                let b = &self.bots[i];
                let arrived = !b.moving && (b.x - r.goal[0]).hypot(b.y - r.goal[1]) < RIDE_ARRIVE;
                if r.left <= 0.0 || arrived {
                    self.rides.remove(&cid);
                }
            }
            // Step toward the next waypoint (the goal itself when the walk is in one room). A blocked
            // step tries nearby headings, else the robot waits this frame.
            let (wx, wy) = self.waypoint(i, (gx, gy));
            let (ex, ey) = (wx - x0, wy - y0);
            let check_props = (home.0 - x0).hypot(home.1 - y0) > 0.9;
            let mut travel: Option<f32> = pose.map(|p| p.heading as f32);
            if self.bots[i].moving && pose.is_none() {
                let step = (SPEED * dt).min(ex.hypot(ey));
                let to_goal = ex.atan2(ey);
                for off in STEER {
                    let ang = to_goal + off;
                    let nx = x0 + ang.sin() * step;
                    let ny = y0 + ang.cos() * step;
                    if blocked(&self.bots, i, &self.prop_boxes, nx, ny, check_props) {
                        continue;
                    }
                    self.bots[i].x = nx;
                    self.bots[i].y = ny;
                    travel = Some(ang);
                    break;
                }
            }
            // Social arrivals: a visitor that has reached its partner starts the chat; a stroller waits briefly at the ring.
            if roaming {
                if let Some(sc) = s.as_mut() {
                    let moving = self.bots[i].moving;
                    if sc.phase == Phase::Go && !moving {
                        let until = now + 3000.0 + random() as f64 * 2000.0;
                        sc.phase = Phase::Talk;
                        sc.until = until;
                        if let Some(j) = sc.partner.and_then(|pc| self.index_of(pc)) {
                            if let Some(ps) = self.bots[j].social.as_mut() {
                                ps.until = until;
                            }
                        }
                    } else if sc.phase == Phase::Stroll && !moving && sc.until == f64::INFINITY {
                        sc.until = now + 2000.0;
                    }
                    self.bots[i].social = Some(sc.clone());
                    if now >= sc.until {
                        self.end_social(i, now);
                    }
                }
            }
            // Local +Z is forward. Talkers face each other; walkers face their direction of travel.
            let talker = s
                .as_ref()
                .filter(|sc| sc.phase == Phase::Talk)
                .and_then(|sc| sc.partner)
                .and_then(|pc| self.index_of(pc));
            let (x, y) = (self.bots[i].x, self.bots[i].y);
            let moving = self.bots[i].moving;
            let want = match talker {
                Some(j) => Some((self.bots[j].x - x).atan2(self.bots[j].y - y)),
                None if moving => Some(travel.unwrap_or_else(|| ex.atan2(ey))),
                None => None,
            };
            // The work's station or partner sets the heading where it has one (robot-work.ts facing).
            let want = if walked {
                self.work.facing(&work_bot(&self.bots[i])).or(want)
            } else {
                want
            };
            // A waving robot turns to face the camera (its heading: local +Z is forward, as the walk's).
            let want = if self.bots[i].robot.waving() {
                Some((eye.x - x).atan2(eye.z - y))
            } else {
                want
            };
            if let Some(want) = want {
                let d = want - self.bots[i].heading;
                let diff = d.sin().atan2(d.cos());
                self.bots[i].heading += diff * (dt * 8.0).min(1.0);
            }
            if walked {
                let wb = work_bot(&self.bots[i]);
                self.work.arrived(&wb, &mut self.bots[i].robot);
            }
            let state = match status.as_str() {
                "working" => RobotState::Working,
                "offline" => RobotState::Offline,
                _ if moving => RobotState::Walking,
                _ => RobotState::Idle,
            };
            if self.bots[i].state != Some(state) {
                self.bots[i].state = Some(state);
                self.bots[i].robot.set_state(state);
            }
            let highlight = self.selected == Some(cid) || self.hovered == Some(cid);
            if highlight != self.bots[i].highlighted {
                self.bots[i].highlighted = highlight;
                self.bots[i].robot.set_highlighted(highlight);
            }
            // Gravity: walk up slopes, fall where the ground drops away (the navigation's body does it when it walks).
            let ground = self.ground_at(x, y);
            if let Some(p) = pose {
                let b = &mut self.bots[i];
                b.robot.set_body(super::nav::body_of(p.body), t);
                b.robot.group.set_scalar(p.scale as f32);
            } else {
                let b = &mut self.bots[i];
                if b.py > ground + 0.001 {
                    b.vy -= GRAVITY * dt;
                    b.py = ground.max(b.py + b.vy * dt);
                    if b.py == ground {
                        b.vy = 0.0;
                    }
                } else {
                    b.py = ground;
                    b.vy = 0.0;
                }
            }
            // Lean with the ground: the slope's normal from the surface around the robot, then the heading.
            let e = 0.3;
            let sx = (self.ground_at(x + e, y) - self.ground_at(x - e, y)) / (2.0 * e);
            let sz = (self.ground_at(x, y + e) - self.ground_at(x, y - e)) / (2.0 * e);
            let slope = Vec3::new(-sx, 1.0, -sz).normalize();
            let working = status == "working";
            let role = if working {
                Some(self.districts[self.bots[i].district].role.clone())
            } else {
                None
            };
            {
                let b = &mut self.bots[i];
                b.robot.group.set_position(b.x, b.py, b.y);
                let target = Quat::from_rotation_arc(Vec3::Y, slope)
                    * Quat::from_axis_angle(Vec3::Y, b.heading);
                b.tilt = b.tilt.slerp(target, 1.0 - (-10.0 * dt).exp());
                b.robot.group.set_quaternion(b.tilt);
                b.robot.update(dt, t);
                b.pad_mat.get().color = if working {
                    colors.info
                } else if status == "idle" {
                    colors.ok
                } else {
                    colors.ink3
                };
                // Working robots stay at their desk, carrying a glowing task token; their district's monitors light up.
                b.token.set_visible(working);
            }
            if let Some(role) = role {
                if let Some(light) = self.lit_by_role.get_mut(&role) {
                    light.lit = true;
                }
                let b = &mut self.bots[i];
                let head = b.robot.label_anchor();
                b.token.set_position(
                    head.x,
                    head.y + 0.6 + (t * 2.5 + cid as f32).sin() * 0.08,
                    head.z,
                );
                b.token.get().rotation.y += dt * 1.8;
            }
        }
        for light in self.lit_by_role.values() {
            let want = if light.lit { 1.2 } else { 0.05 };
            for m in &light.screens {
                let mut mat = m.get();
                if mat.emissive_intensity != want {
                    mat.emissive_intensity = want;
                }
            }
        }
    }

    /// Dust motes drift up and sway; rewritten as the dust cloud's points.
    fn update_dust(&mut self, t: f32) {
        let points: Vec<[f32; 3]> = self
            .dust_seeds
            .iter()
            .take(DUST_N)
            .enumerate()
            .map(|(i, d)| {
                let fi = i as f32;
                [
                    d[0] + (t * 0.3 + fi).sin() * 0.25,
                    0.3 + ((d[1] + t * d[3]) % 4.2),
                    d[2] + (t * 0.25 + fi).cos() * 0.25,
                ]
            })
            .collect();
        self.dust.set_points(points);
    }

    /// Packets move along a curved arc between their robots and expire after `PACKET_MS`.
    fn update_packets(&mut self) {
        let now = self.now_ms;
        let mut i = self.packets.len();
        while i > 0 {
            i -= 1;
            let (from, to, start) = (
                self.packets[i].from,
                self.packets[i].to,
                self.packets[i].start,
            );
            let t = (now - start) / PACKET_MS;
            let ends = self.index_of(from).zip(self.index_of(to));
            let Some((a, b)) = ends.filter(|_| t < 1.0) else {
                let p = self.packets.remove(i);
                self.drop_packet(&p);
                continue;
            };
            let p0 = self.bots[a].robot.label_anchor();
            let p2 = self.bots[b].robot.label_anchor();
            let mut mid = p0.lerp(p2, 0.5);
            mid.y += 0.8 + p0.distance(p2) * 0.3;
            let t = t as f32;
            let u = 1.0 - t;
            let pos = p0 * (u * u) + mid * (2.0 * u * t) + p2 * (t * t);
            self.packets[i].core.set_position(pos.x, pos.y, pos.z);
            self.packets[i].halo.set_position(pos.x, pos.y, pos.z);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::robot::Robot;
    use super::super::super::scene::{Color, Mat, Obj};
    use super::super::AgentInfo;
    use super::*;

    fn district() -> District {
        District {
            role: "worker".to_string(),
            cx: 0.0,
            cy: 0.0,
            r: 4.0,
            hue: 0.0,
            round: false,
        }
    }

    fn bot_at(floor: &Floor, cid: u64, x: f32, y: f32, status: &str) -> Bot {
        Bot {
            agent: AgentInfo {
                cid,
                name: format!("bot{cid}"),
                role: "worker".to_string(),
                status: status.to_string(),
                tier: "worker".to_string(),
                model: "test".to_string(),
                species: None,
            },
            district: 0,
            home: (0.0, 0.0),
            x,
            y,
            tx: x,
            ty: y,
            heading: 0.0,
            moving: false,
            py: 0.0,
            vy: 0.0,
            state: None,
            next_wander: f64::INFINITY,
            next_social: f64::INFINITY,
            social: None,
            activity: String::new(),
            robot: Robot::new(0.0, false, &floor.robot_palette, 0.0),
            robot_key: format!("bot{cid}"),
            tilt: Quat::IDENTITY,
            pad: Obj::group(),
            pad_mat: Mat::toon(Color::hex(0)),
            token: Obj::group(),
            highlighted: false,
            route: Vec::new(),
            route_goal: None,
        }
    }

    #[test]
    fn packet_disappears_after_packet_ms() {
        let mut floor = Floor::new();
        floor.districts.push(district());
        let a = bot_at(&floor, 1, 0.0, 0.0, "idle");
        let b = bot_at(&floor, 2, 2.0, 0.0, "idle");
        floor.bots.push(a);
        floor.bots.push(b);
        floor.step(0.0, 0.0, 0.0, 0.0);
        floor.send_packet(1, 2, true);
        assert_eq!(floor.packets.len(), 1);
        floor.step(0.016, PACKET_MS - 1.0, 0.0, 0.0);
        assert_eq!(
            floor.packets.len(),
            1,
            "still in flight just before PACKET_MS"
        );
        floor.step(0.016, PACKET_MS + 1.0, 0.0, 0.0);
        assert!(floor.packets.is_empty(), "gone once PACKET_MS has passed");
    }

    fn board_floor(roles: &[&str], status: &str) -> Floor {
        let model = hardware::model("board/workstation").expect("the workstation board parses");
        let mut floor = Floor::new();
        floor.show_board(
            "test".into(),
            model,
            hardware::Params::default().with("gpus", 1.0),
            hardware::Status::Online,
        );
        let list: Vec<AgentInfo> = roles
            .iter()
            .enumerate()
            .map(|(i, role)| AgentInfo {
                cid: i as u64 + 1,
                name: format!("agent {i}"),
                role: role.to_string(),
                status: status.to_string(),
                tier: "worker".to_string(),
                model: "test".to_string(),
                species: None,
            })
            .collect();
        floor.sync(&list);
        floor
    }

    /// floor.ts's glow plates: a slot whose rooms have a working agent lights its plate (additive, GLOW_OPACITY at full
    /// level), and an idle board shows none.
    #[test]
    fn a_working_slot_lights_its_glow_plate() {
        let lit = |status: &str| {
            let mut floor = board_floor(&["builder", "librarian", "reviewer"], status);
            for k in 0..240 {
                floor.step(
                    1.0 / 60.0,
                    f64::from(k) * 1000.0 / 60.0 + 10_000.0,
                    0.0,
                    0.0,
                );
            }
            let board = floor.board.as_ref().expect("a board");
            assert!(!board.glow_plates.is_empty(), "the board has glow plates");
            board
                .glow_plates
                .values()
                .filter(|(mesh, mat)| mesh.get().visible && mat.get().opacity > 0.3)
                .count()
        };
        assert!(lit("working") > 0, "no glow plate lit over a working slot");
        assert_eq!(lit("idle"), 0, "a glow plate lit on an idle board");
    }

    #[test]
    fn robots_walk_the_shared_layouts_navigation() {
        // The floor owns its pending shared layout by its address, so it is built where it stays (board_floor moves it).
        let mut floor = board_floor(&["builder", "librarian", "reviewer"], "idle");
        let list: Vec<AgentInfo> = floor.bots.iter().map(|b| b.agent.clone()).collect();
        floor.layout_cache = None;
        floor.sync(&list);
        let t0 = std::time::Instant::now();
        while floor.nav.is_none() && t0.elapsed() < std::time::Duration::from_secs(30) {
            floor.step_rooms(std::time::Duration::from_millis(50));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            floor.nav.is_some(),
            "the shared layout's navigation is built"
        );
        let start: Vec<(f32, f32)> = floor.bots.iter().map(|b| (b.x, b.y)).collect();
        for k in 0..900 {
            floor.step(
                1.0 / 60.0,
                f64::from(k) * 1000.0 / 60.0 + 10_000.0,
                0.0,
                0.0,
            );
            let nav = floor.nav.as_ref().expect("still there");
            for b in &floor.bots {
                if let Some(c) = nav.world.grid().cell_at(f64::from(b.x), f64::from(b.y)) {
                    assert_eq!(
                        nav.world.grid().blocked[c],
                        0,
                        "a robot stands on a blocked cell"
                    );
                }
            }
        }
        let moved = floor
            .bots
            .iter()
            .zip(&start)
            .filter(|(b, s)| (b.x - s.0).hypot(b.y - s.1) > 0.5)
            .count();
        assert!(moved > 0, "the idle robots wander on the grid");
        // The robots act on the work's decisions (robot-work.ts): each has an activity.
        for b in &floor.bots {
            let d = floor.work.debug_of(b.agent.cid as i64);
            assert!(
                d.pointer("/dec/activity").is_some_and(|a| a.is_string()),
                "robot {} has no work decision: {d}",
                b.agent.cid
            );
        }
    }

    #[test]
    fn a_lift_ride_takes_the_robot_to_the_other_level() {
        let mut floor = board_floor(&["builder", "librarian", "reviewer"], "idle");
        let list: Vec<AgentInfo> = floor.bots.iter().map(|b| b.agent.clone()).collect();
        floor.layout_cache = None;
        floor.sync(&list);
        let t0 = std::time::Instant::now();
        while floor.nav.is_none() && t0.elapsed() < std::time::Duration::from_secs(30) {
            floor.step_rooms(std::time::Duration::from_millis(50));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut now = 10_000.0;
        let mut frames = |floor: &mut Floor, n: usize| {
            for _ in 0..n {
                floor.step(1.0 / 60.0, now, 0.0, 0.0);
                now += 1000.0 / 60.0;
            }
        };
        frames(&mut floor, 120);
        let (low, high, lx, lz) = {
            let grid = floor
                .nav
                .as_ref()
                .expect("the board has a nav")
                .world
                .grid();
            let l = *grid.lifts.first().expect("the board has a lift");
            (l.low, l.high, l.x, l.z)
        };
        // A robot stands on the plate, at the top of the lift.
        let cid = floor.bots[0].agent.cid;
        let nav = floor.nav.as_mut().expect("the board has a nav");
        let id = nav.robot_of(cid).expect("the robot is in the nav");
        nav.world.shove(id, lx, lz);
        frames(&mut floor, 2);
        let on_plate = floor.bot(cid).expect("the robot").py;
        assert!(
            (f64::from(on_plate) - high).abs() < 0.05,
            "the robot stands on the plate at {on_plate}, not {high}"
        );

        let ride = floor.lift_ride(0, 1).expect("the robot on the plate rides");
        assert_eq!(ride.robots, vec![cid], "the robot nearest the lift is sent");
        assert!(floor.rides.contains_key(&cid));
        let mut ended = false;
        for _ in 0..(120 * 60) {
            frames(&mut floor, 1);
            if !floor.rides.contains_key(&cid) {
                ended = true;
                break;
            }
        }
        assert!(ended, "the ride ends within 120 s");
        let end = floor.bot(cid).expect("the robot").py;
        assert!(
            (f64::from(end) - low).abs() < 0.05,
            "the robot ends on the other level at {end}, not {low}"
        );
        // The work's goal takes over again once the ride ends.
        assert!(floor.work.debug_of(cid as i64).is_object());
    }

    #[test]
    fn the_nav_overlay_is_built_and_follows_the_walk_when_asked() {
        crate::set_env_overrides(std::collections::BTreeMap::from([(
            "APZ_NAV_OVERLAY".to_string(),
            "1".to_string(),
        )]));
        let mut floor = board_floor(&["builder", "librarian", "reviewer"], "idle");
        let list: Vec<AgentInfo> = floor.bots.iter().map(|b| b.agent.clone()).collect();
        floor.layout_cache = None;
        floor.sync(&list);
        let t0 = std::time::Instant::now();
        while floor.nav.is_none() && t0.elapsed() < std::time::Duration::from_secs(30) {
            floor.step_rooms(std::time::Duration::from_millis(50));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::set_env_overrides(std::collections::BTreeMap::new());
        for k in 0..300 {
            floor.step(
                1.0 / 60.0,
                f64::from(k) * 1000.0 / 60.0 + 10_000.0,
                0.0,
                0.0,
            );
        }
        let nav = floor.nav.as_ref().expect("the board has a nav");
        let overlay = nav
            .overlay
            .as_ref()
            .expect("APZ_NAV_OVERLAY=1 builds the overlay with the nav");
        assert!(overlay.cells > 0, "the overlay draws the walkable cells");
    }

    #[test]
    fn a_busy_room_lifts_its_slot_level() {
        let mut floor = board_floor(&["builder", "librarian"], "working");
        let slot = floor.board.as_ref().expect("a board").layout.districts[0]
            .slot
            .clone();
        for _ in 0..80 {
            floor.step(0.05, 0.0, 0.0, 0.0);
        }
        let level = floor
            .board
            .as_ref()
            .expect("a board")
            .level
            .get(&slot)
            .copied()
            .unwrap_or(0.0);
        assert!(
            level > 0.95,
            "a slot with a working room settles at full level, got {level}"
        );
    }

    #[test]
    fn a_walk_across_rooms_takes_the_copper() {
        let mut floor = board_floor(&["builder", "librarian"], "idle");
        let goal = {
            let bd = &floor.board.as_ref().expect("a board").layout.districts[1];
            (bd.district.cx, bd.district.cy)
        };
        let wp = floor.waypoint(0, goal);
        assert!(
            floor.bots[0].route_goal == Some([goal.0, goal.1]),
            "the route is set for the goal"
        );
        assert!(
            !floor.bots[0].route.is_empty(),
            "the walk to another room has a route"
        );
        assert!(
            wp != goal || floor.bots[0].route.len() == 1,
            "the first step is a waypoint of the copper"
        );
    }

    #[test]
    fn walking_bot_moves_toward_its_target() {
        let mut floor = Floor::new();
        floor.districts.push(district());
        let mut bot = bot_at(&floor, 7, 0.0, 0.0, "idle");
        // Wandering and socialising are parked far in the future, so only the walk to (3, 0) runs.
        bot.tx = 3.0;
        bot.ty = 0.0;
        floor.bots.push(bot);
        floor.step(0.05, 500.0, 0.0, 0.0); // the frame cap: a 50 ms step
        let bot = &floor.bots[0];
        assert!(bot.moving, "a bot short of its target is moving");
        // One step of SPEED * dt = 0.08 straight toward +x (the target lies along grid x).
        assert!((bot.x - 0.08).abs() < 1e-4, "x = {}", bot.x);
        assert!(bot.y.abs() < 1e-4, "y = {}", bot.y);
    }

    #[test]
    fn a_selected_robot_waves_at_the_camera_then_stops() {
        let mut floor = Floor::new();
        floor.districts.push(district());
        // Wandering and socialising are parked, so the robot only stands where it is.
        floor.bots.push(bot_at(&floor, 5, 2.0, 0.0, "idle"));
        floor.step(0.05, 500.0, 0.0, 0.0);
        assert!(!floor.bots[0].robot.waving(), "nothing is selected yet");

        floor.selected = Some(5);
        floor.step(0.05, 500.0, 0.0, 0.0);
        let eye = floor.orbit.camera(1.0).eye;
        let want = (eye.x - 2.0).atan2(eye.z);
        for _ in 0..10 {
            floor.step(0.05, 500.0, 0.0, 0.0); // 0.5 s in: still waving, and turned to the camera
        }
        let bot = &floor.bots[0];
        assert!(bot.robot.waving(), "a selected robot waves");
        let turn = (bot.heading - want).sin().atan2((bot.heading - want).cos());
        assert!(turn.abs() < 0.05, "heading {} wants {want}", bot.heading);
        assert!(
            (bot.x - 2.0).abs() < 1e-6 && bot.y.abs() < 1e-6,
            "the robot is not moved"
        );

        for _ in 0..20 {
            floor.step(0.05, 500.0, 0.0, 0.0); // 1.5 s in: the wave is over
        }
        assert!(
            !floor.bots[0].robot.waving(),
            "the wave stops after WAVE_SECS"
        );

        // The same selection reacts once: it does not wave again until the selection changes.
        for _ in 0..5 {
            floor.step(0.05, 500.0, 0.0, 0.0);
        }
        assert!(!floor.bots[0].robot.waving(), "one reaction per selection");
    }
}
