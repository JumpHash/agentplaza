//! `floor.ts` lines ~700–945: each frame — robots wander, visit, chat and stroll (with ground
//! height, slopes, falling, collisions), packets fly, dust drifts, props and rooms animate.

use std::cell::RefCell;

use glam::{Quat, Vec3};
use serde_json::Value;

use super::super::props::Box2;
use super::super::robot::RobotState;
use super::super::scene::Obj;
use super::super::textures::Rng;
use super::{Bot, District, Floor, Packet, Phase, Social};

const PACKET_MS: f64 = 1500.0;
const SPEED: f32 = 1.6; // grid units per second; one grid cell is one world unit
const MAX_PACKETS: usize = 60;
const SLAB_H: f32 = 0.5; // district slab height
const DAIS_H: f32 = 0.8; // orchestrator dais height
const DAIS_R: f32 = 2.3;
const RAMP: f32 = 1.5; // width of the sloped band around a district slab
const DAIS_RAMP: f32 = 2.2; // width of the dais's sloped skirt
const GRAVITY: f32 = 9.8; // world units per second squared
const SOCIAL_NEAR: f32 = 9.5; // districts this close count as neighbours for a visit or a chat
const STROLL_R: f32 = 3.1; // the stroll ring just outside the dais
const ROBOT_R: f32 = 0.55; // robot footprint radius for collisions
const DEG: f32 = std::f32::consts::PI / 180.0;
const STEER: [f32; 5] = [0.0, 30.0 * DEG, -30.0 * DEG, 60.0 * DEG, -60.0 * DEG]; // headings tried when blocked
const DUST_N: usize = 90;

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

fn top_of(d: &District) -> f32 {
    if d.role == "orchestrator" {
        DAIS_H
    } else {
        SLAB_H
    }
}

/// Whether two districts are close enough for a visit or a chat.
fn nearby(a: &District, c: &District) -> bool {
    a.role == c.role || (a.cx - c.cx).hypot(a.cy - c.cy) <= SOCIAL_NEAR
}

/// The top of whatever lies under (x, y): a dais, a district slab, or the bare floor.
fn surface_at(districts: &[District], x: f32, y: f32) -> f32 {
    let mut top = 0.0f32;
    for d in districts {
        let orch = d.role == "orchestrator";
        // Distance out from the walkable top: round for the dais and round rooms, square for the rest.
        let out = if orch {
            (x - d.cx).hypot(y - d.cy) - DAIS_R
        } else if d.round {
            (x - d.cx).hypot(y - d.cy) - d.r
        } else {
            (x - d.cx).abs().max((y - d.cy).abs()) - d.r
        };
        let band = if orch { DAIS_RAMP } else { RAMP };
        let h = if out <= 0.0 {
            top_of(d)
        } else if out < band {
            top_of(d) * (1.0 - out / band)
        } else {
            0.0
        };
        top = top.max(h);
    }
    top
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
    if sends || assigns {
        to.as_u64()
    } else {
        None
    }
}

impl Floor {
    /// One frame: `dt` seconds since the last, `now_ms` the wall clock.
    pub fn step(&mut self, dt: f32, now_ms: f64) {
        let dt = dt.min(0.05); // the web caps a frame at 50 ms
        self.time += dt;
        self.now_ms = now_ms;
        self.update_bots(dt, now_ms);
        self.update_packets();
        self.update_dust();
        self.clouds.set_rotation(0.0, self.time * 0.006, 0.0);
        (self.plaza_anim)(self.time);
        for anim in self.prop_anim.iter_mut() {
            anim(self.time);
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
        self.packets.push(Packet { from, to, start: self.now_ms, core, halo });
        while self.packets.len() > MAX_PACKETS {
            let p = self.packets.remove(0);
            self.drop_packet(&p);
        }
    }

    /// One event from the console's stream. `kind` is the event channel the web subscribes to:
    /// "transcript" (`data` is an entry `{cid, kind, data}`: tool calls set activities and send
    /// packets, work start/end too), "delta" (`{cid}`: thinking) or "conversation" (`{from_cid,
    /// to_cid}`: an ok packet). Other channels are ignored.
    pub fn on_event(&mut self, kind: &str, data: &Value) {
        match kind {
            "transcript" => self.on_transcript(data),
            "delta" => {
                if let Some(i) = data.get("cid").and_then(Value::as_u64).and_then(|c| self.index_of(c)) {
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
            _ => {}
        }
    }

    fn index_of(&self, cid: u64) -> Option<usize> {
        self.bots.iter().position(|b| b.agent.cid == cid)
    }

    /// `nameOf(cid)`: the agent's name, else the raw value ("?" when absent).
    fn name_of(&self, v: Option<&Value>) -> String {
        if let Some(bot) = v.and_then(Value::as_u64).and_then(|c| self.index_of(c)) {
            return self.bots[bot].agent.name.clone();
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
            let first = text(inp.get("command")).split('\n').next().unwrap_or("").to_string();
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
        let target = if path.is_empty() { text(inp.get("url")) } else { path };
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
                let from = data.get("origin").and_then(|o| o.get("from_cid")).and_then(Value::as_u64);
                if let Some(from) = from {
                    self.send_packet(from, cid, false);
                } else if text(data.get("kind")) == "task" {
                    let boss = self.bots.iter().find(|b| b.agent.tier == "orchestrator").map(|b| b.agent.cid);
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
                    && nearby(&self.districts[self.bots[j].district], &self.districts[mine])
            })
            .collect();
        if pool.is_empty() {
            return;
        }
        let pick = pool[((random() * pool.len() as f32) as usize).min(pool.len() - 1)];
        let (px, py, pcid) = (self.bots[pick].x, self.bots[pick].y, self.bots[pick].agent.cid);
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
        let colors = self.colors;
        for light in self.lit_by_role.values_mut() {
            light.lit = false;
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
            let (x0, y0) = (self.bots[i].x, self.bots[i].y);
            let (dx, dy) = (gx - x0, gy - y0);
            let dist = dx.hypot(dy);
            self.bots[i].moving = dist > 0.02;
            // Step toward the goal. A blocked step tries nearby headings, else the robot waits this frame.
            let check_props = (home.0 - x0).hypot(home.1 - y0) > 0.9;
            let mut travel: Option<f32> = None;
            if self.bots[i].moving {
                let step = (SPEED * dt).min(dist);
                let to_goal = dx.atan2(dy);
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
                None if moving => Some(travel.unwrap_or_else(|| dx.atan2(dy))),
                None => None,
            };
            if let Some(want) = want {
                let d = want - self.bots[i].heading;
                let diff = d.sin().atan2(d.cos());
                self.bots[i].heading += diff * (dt * 8.0).min(1.0);
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
            // Gravity: walk up slopes, fall where the ground drops away.
            let ground = surface_at(&self.districts, x, y);
            {
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
            let sx = (surface_at(&self.districts, x + e, y) - surface_at(&self.districts, x - e, y)) / (2.0 * e);
            let sz = (surface_at(&self.districts, x, y + e) - surface_at(&self.districts, x, y - e)) / (2.0 * e);
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
                let target = Quat::from_rotation_arc(Vec3::Y, slope) * Quat::from_axis_angle(Vec3::Y, b.heading);
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
                b.token.set_position(head.x, head.y + 0.6 + (t * 2.5 + cid as f32).sin() * 0.08, head.z);
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
    fn update_dust(&mut self) {
        let t = self.time;
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
            let (from, to, start) = (self.packets[i].from, self.packets[i].to, self.packets[i].start);
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
        District { role: "worker".to_string(), cx: 0.0, cy: 0.0, r: 4.0, hue: 0.0, round: false }
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
        floor.step(0.0, 0.0);
        floor.send_packet(1, 2, true);
        assert_eq!(floor.packets.len(), 1);
        floor.step(0.016, PACKET_MS - 1.0);
        assert_eq!(floor.packets.len(), 1, "still in flight just before PACKET_MS");
        floor.step(0.016, PACKET_MS + 1.0);
        assert!(floor.packets.is_empty(), "gone once PACKET_MS has passed");
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
        floor.step(0.05, 500.0); // the frame cap: a 50 ms step
        let bot = &floor.bots[0];
        assert!(bot.moving, "a bot short of its target is moving");
        // One step of SPEED * dt = 0.08 straight toward +x (the target lies along grid x).
        assert!((bot.x - 0.08).abs() < 1e-4, "x = {}", bot.x);
        assert!(bot.y.abs() < 1e-4, "y = {}", bot.y);
    }
}
