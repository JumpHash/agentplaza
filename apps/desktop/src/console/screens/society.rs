//! The Society screen (parity with the web console's `society` view): the agents as a graph of
//! who knows whom, drawn on a canvas, and the selected agent's opinions in both directions.

use std::collections::{HashMap, HashSet};

use iced::widget::{canvas, column, container, row, scrollable, Space};
use iced::{mouse, Background, Color, Element, Length, Point, Rectangle, Renderer, Size, Task, Theme};
use serde::Deserialize;
use serde_json::Value;

use super::Id;
use crate::console::floor3d::world::layout::agent_hue;
use crate::console::ui::{self, Kind, Tone};
use crate::console::{client, Ctx};

const W: f32 = 960.0;
const H: f32 = 600.0;
const PAD: f32 = 90.0;
const FIRST_STEPS: usize = 300;
const LATER_STEPS: usize = 150;
const REST: f32 = 150.0;
const DISAGREE: f32 = 0.25;
const NOTE_PREVIEW: usize = 220;
const RED: Color = Color::from_rgb8(0xe5, 0x53, 0x3d);
const AMBER: Color = Color::from_rgb8(0xf2, 0xb8, 0x4b);
const GREEN: Color = Color::from_rgb8(0x3f, 0xae, 0x6a);

/// The society as `GET /v1/society` returns it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Society {
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub bonds: Vec<Bond>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub cid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub reputation: f32,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub wallet: f64,
}

/// A directed contact edge: the holder `from` and its opinion of `to`.
#[derive(Debug, Clone, Deserialize)]
pub struct Bond {
    pub from: u64,
    pub to: u64,
    pub kind: String,
    #[serde(default)]
    pub strength: f32,
    #[serde(default)]
    pub note: Option<String>,
    pub opinion: Opinion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Opinion {
    /// Expected trust, in [0, 1].
    #[serde(default)]
    pub e: f32,
    /// Uncertainty, in [0, 1].
    #[serde(default)]
    pub u: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    data: Option<Society>,
    loaded: bool,
    /// Where each agent sits, by cid; kept across refreshes so the graph does not jump.
    pos: HashMap<u64, Pt>,
    selected: Option<u64>,
    /// The peers whose private notes are expanded in the panel.
    open_notes: HashSet<u64>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Society, String>),
    /// Select an agent, or clear the selection with `None`.
    Pick(Option<u64>),
    /// The owner rates an agent: +1 trust more, -1 trust less.
    Rate(u64, f32),
    Rated(Result<Value, String>),
    /// Expand or collapse a peer's private note.
    Note(u64),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the society (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || client.get::<Society>("/v1/society"), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(data)) => {
            state.error = None;
            state.loaded = true;
            layout(&data.agents, &data.bonds, &mut state.pos);
            if state.selected.is_some_and(|cid| !data.agents.iter().any(|a| a.cid == cid)) {
                state.selected = None;
            }
            state.data = Some(data);
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            // Keep the last good view; the next poll tries again.
            state.loaded = true;
            state.error = Some(e);
            Task::none()
        }
        Msg::Pick(cid) => {
            state.selected = cid;
            state.open_notes.clear();
            Task::none()
        }
        Msg::Note(peer) => {
            if !state.open_notes.remove(&peer) {
                state.open_notes.insert(peer);
            }
            Task::none()
        }
        Msg::Rate(cid, value) => {
            let client = ctx.client.clone();
            let body = serde_json::json!({ "value": value });
            client::task(move || client.post::<Value>(&format!("/v1/agents/{cid}/rating"), &body), Msg::Rated)
        }
        Msg::Rated(result) => {
            if let Err(e) = result {
                state.error = Some(e);
            }
            load(state, ctx)
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.data {
        _ if !state.loaded => ui::loading(),
        None => ui::error_line(state.error.as_deref().unwrap_or("Could not load the society.")),
        Some(d) if d.agents.is_empty() => {
            ui::empty("society", "No agents yet", "Spawn agents and they show up here, with what they think of one another.")
        }
        Some(d) => {
            let graph = Graph { data: d, pos: &state.pos, selected: state.selected };
            let panel = match state.selected.and_then(|cid| d.agents.iter().find(|a| a.cid == cid)) {
                Some(me) => detail(state, d, me),
                None => legend(),
            };
            row![
                container(canvas(graph).width(Length::Fill).height(Length::Fill)).width(Length::FillPortion(3)).height(Length::Fill),
                container(scrollable(column![panel]).width(Length::Fill)).width(Length::FillPortion(2)).height(Length::Fill),
            ]
            .spacing(16)
            .height(Length::Fill)
            .into()
        }
    };
    ui::sheet(ui::view_head("Society", vec![]), body)
}

// ---- Layout ----------------------------------------------------------------

struct Body {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    fixed: bool,
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

/// Places the agents. Agents already placed stay where they are; only new ones move, under the
/// springs and repulsion of the whole graph. Departed agents are dropped.
fn layout(agents: &[Agent], bonds: &[Bond], pos: &mut HashMap<u64, Pt>) {
    let live: HashSet<u64> = agents.iter().map(|a| a.cid).collect();
    pos.retain(|cid, _| live.contains(cid));
    let fresh: Vec<u64> = agents.iter().map(|a| a.cid).filter(|cid| !pos.contains_key(cid)).collect();
    if fresh.is_empty() {
        return;
    }
    let placed = !pos.is_empty();
    for (i, cid) in fresh.iter().enumerate() {
        let angle = i as f32 * 2.39996; // golden angle, so a crowd fans out evenly
        let r = if placed { 40.0 } else { 70.0 + 26.0 * (i as f32).sqrt() };
        pos.insert(*cid, Pt { x: W / 2.0 + angle.cos() * r, y: H / 2.0 + angle.sin() * r });
    }

    let is_fresh: HashSet<u64> = fresh.iter().copied().collect();
    let mut bodies: Vec<Body> = agents
        .iter()
        .map(|a| {
            let p = pos.get(&a.cid).copied().unwrap_or(Pt { x: W / 2.0, y: H / 2.0 });
            Body { x: p.x, y: p.y, vx: 0.0, vy: 0.0, fixed: !is_fresh.contains(&a.cid) }
        })
        .collect();
    let index: HashMap<u64, usize> = agents.iter().enumerate().map(|(i, a)| (a.cid, i)).collect();

    // One spring per unordered pair; stronger contacts pull harder.
    let mut springs: Vec<(usize, usize, f32)> = Vec::new();
    for bond in bonds {
        let (Some(&a), Some(&z)) = (index.get(&bond.from), index.get(&bond.to)) else { continue };
        if a == z {
            continue;
        }
        let (lo, hi) = (a.min(z), a.max(z));
        let k = 0.01 + 0.02 * clamp(bond.strength / 10.0, 0.0, 1.0);
        match springs.iter_mut().find(|s| s.0 == lo && s.1 == hi) {
            Some(s) => s.2 = k.max(s.2),
            None => springs.push((lo, hi, k)),
        }
    }

    let steps = if fresh.len() == agents.len() { FIRST_STEPS } else { LATER_STEPS };
    for _ in 0..steps {
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let (dx, dy) = (bodies[j].x - bodies[i].x, bodies[j].y - bodies[i].y);
                let d = (dx * dx + dy * dy).sqrt().max(1.0);
                let f = 16000.0 / (d * d);
                let (fx, fy) = (f * dx / d, f * dy / d);
                bodies[i].vx -= fx;
                bodies[i].vy -= fy;
                bodies[j].vx += fx;
                bodies[j].vy += fy;
            }
        }
        for &(a, z, k) in &springs {
            let (dx, dy) = (bodies[z].x - bodies[a].x, bodies[z].y - bodies[a].y);
            let d = (dx * dx + dy * dy).sqrt().max(1.0);
            let f = k * (d - REST);
            let (fx, fy) = (f * dx / d, f * dy / d);
            bodies[a].vx += fx;
            bodies[a].vy += fy;
            bodies[z].vx -= fx;
            bodies[z].vy -= fy;
        }
        for b in &mut bodies {
            // Centring, then damping; placed agents keep their coordinates.
            b.vx = clamp((b.vx + (W / 2.0 - b.x) * 0.004) * 0.8, -25.0, 25.0);
            b.vy = clamp((b.vy + (H / 2.0 - b.y) * 0.004) * 0.8, -25.0, 25.0);
            if b.fixed {
                continue;
            }
            b.x = clamp(b.x + b.vx, PAD, W - PAD);
            b.y = clamp(b.y + b.vy, PAD, H - PAD);
        }
    }
    for (a, b) in agents.iter().zip(&bodies) {
        pos.insert(a.cid, Pt { x: b.x, y: b.y });
    }
}

// ---- Graph -----------------------------------------------------------------

/// The node's side in world units: bigger with reputation.
fn side(reputation: f32) -> f32 {
    46.0 + 12.0 * clamp(reputation / 100.0, 0.0, 1.0)
}

/// The trust band's colour: red below 0.4, amber to 0.6, green above.
fn band(e: f32) -> Color {
    if e < 0.4 {
        RED
    } else if e <= 0.6 {
        AMBER
    } else {
        GREEN
    }
}

fn name_of(a: &Agent) -> String {
    if a.name.trim().is_empty() {
        "Unnamed".to_string()
    } else {
        a.name.clone()
    }
}

/// The agent's head and body colour, as the web's `hsl()`.
fn hsl(h: f32, s: f32, l: f32) -> Color {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |t: f32| {
        let t = if t < 0.0 { t + 1.0 } else if t > 1.0 { t - 1.0 } else { t };
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    Color::from_rgb(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// Maps world coordinates to the canvas: the agents' bounds plus room for heads and name tags,
/// fitted and centred (the web's `preserveAspectRatio` meet).
#[derive(Clone, Copy)]
struct Xf {
    x0: f32,
    y0: f32,
    s: f32,
    dx: f32,
    dy: f32,
}

impl Xf {
    fn fit(pos: &HashMap<u64, Pt>, size: Size) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pos.values() {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let (bx, by, bw, bh) = if pos.is_empty() { (0.0, 0.0, W, H) } else { (x0 - 90.0, y0 - 80.0, x1 - x0 + 180.0, y1 - y0 + 170.0) };
        let s = (size.width / bw).min(size.height / bh).max(0.01);
        Self { x0: bx, y0: by, s, dx: (size.width - bw * s) / 2.0, dy: (size.height - bh * s) / 2.0 }
    }

    fn map(&self, p: Pt) -> Point {
        Point::new((p.x - self.x0) * self.s + self.dx, (p.y - self.y0) * self.s + self.dy)
    }
}

/// A curve from a's edge to z's edge, bowed to one side. Reversed bonds bow apart.
fn edge_curve(a: Pt, z: Pt, ra: f32, rz: f32) -> (Pt, Pt, Pt) {
    let (dx, dy) = (z.x - a.x, z.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let p0 = Pt { x: a.x + ux * ra, y: a.y + uy * ra };
    let p1 = Pt { x: z.x - ux * rz, y: z.y - uy * rz };
    let c = Pt { x: (p0.x + p1.x) / 2.0 - uy * len * 0.16, y: (p0.y + p1.y) / 2.0 + ux * len * 0.16 };
    (p0, c, p1)
}

struct Graph<'a> {
    data: &'a Society,
    pos: &'a HashMap<u64, Pt>,
    selected: Option<u64>,
}

impl Graph<'_> {
    /// The agent under a point of the canvas, if any: its body or its name tag.
    fn hit(&self, at: Point, size: Size) -> Option<u64> {
        let xf = Xf::fit(self.pos, size);
        for a in self.data.agents.iter().rev() {
            let Some(p) = self.pos.get(&a.cid) else { continue };
            let c = xf.map(*p);
            let half = side(a.reputation) * xf.s / 2.0;
            if (at.x - c.x).abs() <= half && at.y >= c.y - half - 16.0 && at.y <= c.y + half + 30.0 {
                return Some(a.cid);
            }
        }
        None
    }
}

impl canvas::Program<Msg> for Graph<'_> {
    type State = ();

    fn update(&self, _state: &mut (), event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Msg>> {
        let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else { return None };
        let at = cursor.position_in(bounds)?;
        // A click on an agent selects it; a click on the background clears the selection.
        Some(canvas::Action::publish(Msg::Pick(self.hit(at, bounds.size()))).and_capture())
    }

    fn draw(&self, _state: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let xf = Xf::fit(self.pos, bounds.size());
        let k = xf.s;
        let agents: HashMap<u64, &Agent> = self.data.agents.iter().map(|a| (a.cid, a)).collect();
        let bonds: HashSet<(u64, u64)> = self.data.bonds.iter().map(|b| (b.from, b.to)).collect();

        // Edges first, so the nodes sit on top of them.
        for b in &self.data.bonds {
            let (Some(from), Some(to), Some(a), Some(z)) = (self.pos.get(&b.from), self.pos.get(&b.to), agents.get(&b.from), agents.get(&b.to)) else {
                continue;
            };
            if b.from == b.to {
                continue;
            }
            let (p0, c, p1) = edge_curve(*from, *to, side(a.reputation) / 2.0 + 8.0, side(z.reputation) / 2.0 + 8.0);
            let (s0, sc, s1) = (xf.map(p0), xf.map(c), xf.map(p1));
            let path = canvas::Path::new(|pb| {
                pb.move_to(s0);
                pb.quadratic_curve_to(sc, s1);
            });
            let mutual = bonds.contains(&(b.to, b.from));
            let mut dashed = false;
            let mut arrow = false;
            let (colour, width) = match self.selected {
                None => (if mutual { ui::INK_2 } else { ui::INK_3 }, if mutual { 1.5 } else { 1.2 }),
                Some(sel) if b.from == sel || b.to == sel => {
                    let out = b.from == sel;
                    dashed = !out;
                    arrow = out;
                    (band(b.opinion.e), 1.5 + 4.0 * clamp(b.strength / 10.0, 0.0, 1.0))
                }
                Some(_) => (ui::INK_3.scale_alpha(0.25), 1.0),
            };
            let mut stroke = canvas::Stroke::default().with_color(colour).with_width(width * k.max(0.5));
            if dashed {
                stroke = canvas::Stroke { line_dash: canvas::stroke::LineDash { segments: &[6.0, 4.0], offset: 0 }, ..stroke };
            }
            frame.stroke(&path, stroke);
            if arrow {
                // The arrowhead points into the holder's peer, at the node's edge.
                let (dx, dy) = (s1.x - sc.x, s1.y - sc.y);
                let len = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / len, dy / len);
                let base = Point::new(s1.x - ux * 12.0, s1.y - uy * 12.0);
                let (nx, ny) = (-uy * 5.0, ux * 5.0);
                let head = canvas::Path::new(|pb| {
                    pb.move_to(s1);
                    pb.line_to(Point::new(base.x + nx, base.y + ny));
                    pb.line_to(Point::new(base.x - nx, base.y - ny));
                    pb.close();
                });
                frame.fill(&head, colour);
            }
        }

        // Nodes: a square body with a head, two eyes, a mouth and a name tag.
        for a in &self.data.agents {
            let Some(p) = self.pos.get(&a.cid) else { continue };
            let c = xf.map(*p);
            let sz = side(a.reputation) * k;
            let half = sz / 2.0;
            let hue = agent_hue(a.cid);
            let linked = self.selected.is_some_and(|sel| bonds.contains(&(sel, a.cid)) || bonds.contains(&(a.cid, sel)));
            let alpha = if self.selected.is_some_and(|sel| sel != a.cid && !linked) { 0.35 } else { 1.0 };
            let ink = ui::INK.scale_alpha(alpha);

            // The print shadow, the antenna and its tip.
            frame.fill_rectangle(Point::new(c.x - half + 3.0, c.y - half + 3.0), Size::new(sz, sz), ui::INK.scale_alpha(0.85 * alpha));
            let antenna = canvas::Path::line(Point::new(c.x, c.y - half), Point::new(c.x, c.y - half - 12.0 * k));
            frame.stroke(&antenna, canvas::Stroke::default().with_color(ink).with_width(3.0 * k));
            let tip = canvas::Path::circle(Point::new(c.x, c.y - half - 13.0 * k), 4.5 * k);
            frame.fill(&tip, hsl(hue, 0.7, 0.55).scale_alpha(alpha));
            frame.stroke(&tip, canvas::Stroke::default().with_color(ink).with_width(2.5 * k));

            // The body, and the ring round the selected agent.
            let body = canvas::Path::rectangle(Point::new(c.x - half, c.y - half), Size::new(sz, sz));
            frame.fill(&body, hsl(hue, 0.6, 0.64).scale_alpha(alpha));
            frame.stroke(&body, canvas::Stroke::default().with_color(ink).with_width(3.0 * k));
            if self.selected == Some(a.cid) {
                let ring = canvas::Path::rectangle(Point::new(c.x - half - 5.0, c.y - half - 5.0), Size::new(sz + 10.0, sz + 10.0));
                frame.stroke(&ring, canvas::Stroke::default().with_color(ui::SIGNAL).with_width(2.0));
            }

            // Two eyes: a white sclera with an ink pupil each.
            for dir in [-1.0_f32, 1.0] {
                let ex = c.x + dir * sz * 0.2;
                let ey = c.y - sz * 0.08;
                let sclera = canvas::Path::circle(Point::new(ex, ey), sz * 0.14);
                frame.fill(&sclera, Color::WHITE.scale_alpha(alpha));
                frame.stroke(&sclera, canvas::Stroke::default().with_color(ink).with_width(2.0 * k));
                let pupil = canvas::Path::circle(Point::new(ex + 1.5 * k, ey + 1.5 * k), sz * 0.06);
                frame.fill(&pupil, ink);
            }

            // The mouth.
            let mouth = canvas::Path::new(|pb| {
                pb.move_to(Point::new(c.x - sz * 0.16, c.y + sz * 0.22));
                pb.quadratic_curve_to(Point::new(c.x, c.y + sz * 0.34), Point::new(c.x + sz * 0.16, c.y + sz * 0.22));
            });
            frame.stroke(&mouth, canvas::Stroke::default().with_color(ink).with_width(2.5 * k).with_line_cap(canvas::LineCap::Round));

            // The name tag, under the body.
            let label: String = {
                let name = name_of(a);
                if name.chars().count() > 16 {
                    name.chars().take(15).chain(std::iter::once('…')).collect()
                } else {
                    name
                }
            };
            let tag_w = (label.chars().count() as f32 * 7.4 + 16.0).max(56.0);
            let tag_top = c.y + half + 8.0 * k;
            let tag = canvas::Path::rectangle(Point::new(c.x - tag_w / 2.0, tag_top), Size::new(tag_w, 22.0));
            frame.fill(&tag, ui::PAPER.scale_alpha(alpha));
            frame.stroke(&tag, canvas::Stroke::default().with_color(ink).with_width(1.0));
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(c.x - tag_w / 2.0 + 8.0, tag_top + 4.0),
                color: ink,
                size: 13.0.into(),
                font: crate::theme::MONO,
                ..canvas::Text::default()
            });
        }

        vec![frame.into_geometry()]
    }
}

// ---- Panel -----------------------------------------------------------------

/// The bond from `from` to `to`, if there is one.
fn bond_between<'a>(d: &'a Society, from: u64, to: u64) -> Option<&'a Bond> {
    d.bonds.iter().find(|b| b.from == from && b.to == to)
}

/// The selected agent's panel: its own reputation and contacts, both ways round.
fn detail<'a>(state: &'a State, d: &'a Society, me: &'a Agent) -> Element<'a, Msg> {
    let names: HashMap<u64, &Agent> = d.agents.iter().map(|a| (a.cid, a)).collect();
    let name = name_of(me);
    let rep = clamp(me.reputation, 0.0, 100.0);
    let rated = me.confidence >= 0.05;

    // Two holders disagree when their opinions of each other differ by more than DISAGREE.
    let gap = |peer: u64| -> bool {
        match (bond_between(d, me.cid, peer), bond_between(d, peer, me.cid)) {
            (Some(ab), Some(ba)) => (ab.opinion.e - ba.opinion.e).abs() > DISAGREE,
            _ => false,
        }
    };
    let peer_row = |peer: u64, bond: &'a Bond| -> Element<'a, Msg> {
        let peer_name = names.get(&peer).map(|a| name_of(a)).unwrap_or_else(|| format!("agent {peer}"));
        let (kind_tone, kind) = match bond.kind.as_str() {
            "hired" => (Tone::Signal, "hired"),
            "was-hired" => (Tone::Info, "was-hired"),
            "added" => (Tone::Ok, "added"),
            _ => (Tone::Muted, "met"),
        };
        let mut top = row![ui::btn(&peer_name, None, Kind::Ghost, Some(Msg::Pick(Some(peer)))), ui::badge(kind, kind_tone)].spacing(8);
        if gap(peer) {
            top = top.push(ui::badge("they disagree", Tone::Warn));
        }
        let mut item = column![top, bar(&bond.opinion)].spacing(6);
        if let Some(note) = bond.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            let open = state.open_notes.contains(&peer);
            let long = note.chars().count() > NOTE_PREVIEW;
            let shown = if long && !open { format!("{}…", note.chars().take(NOTE_PREVIEW).collect::<String>()) } else { note.to_string() };
            item = item.push(ui::markdown(&shown));
            if long {
                item = item.push(ui::btn(if open { "Show less" } else { "Show all" }, None, Kind::Ghost, Some(Msg::Note(peer))));
            }
        }
        container(item).padding([10.0, 0.0]).width(Length::Fill).style(hairline_below).into()
    };

    let outgoing: Vec<Element<'a, Msg>> = d
        .bonds
        .iter()
        .filter(|b| b.from == me.cid && names.contains_key(&b.to))
        .map(|b| peer_row(b.to, b))
        .collect();
    let incoming: Vec<Element<'a, Msg>> = d
        .bonds
        .iter()
        .filter(|b| b.to == me.cid && names.contains_key(&b.from))
        .map(|b| peer_row(b.from, b))
        .collect();

    let mut panel = column![
        column![ui::mono(name.clone(), 18.0, ui::INK), ui::faint(me.role.clone())].spacing(4),
        stat("Reputation", if rated { format!("{} / 100", rep.round()) } else { "Unrated".to_string() }),
        meter(if rated { rep / 100.0 } else { 0.0 }),
        stat("Confidence", format!("{}%", (clamp(me.confidence, 0.0, 1.0) * 100.0).round())),
        stat("Wallet", compact(me.wallet)),
        row![
            ui::btn("Trust more", None, Kind::Primary, Some(Msg::Rate(me.cid, 1.0))),
            ui::btn("Trust less", None, Kind::Secondary, Some(Msg::Rate(me.cid, -1.0))),
        ]
        .spacing(8),
        ui::label(&format!("How {name} sees others")),
    ]
    .spacing(14);
    let outgoing_list: Element<'a, Msg> = if outgoing.is_empty() { ui::faint("No contacts yet.").into() } else { column(outgoing).spacing(4).into() };
    let incoming_list: Element<'a, Msg> = if incoming.is_empty() { ui::faint("Nobody has met them yet.").into() } else { column(incoming).spacing(4).into() };
    panel = panel.push(outgoing_list);
    panel = panel.push(ui::label(&format!("How others see {name}")));
    panel = panel.push(incoming_list);

    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        out.push(ui::error_line(e));
    }
    out.push(panel.into());
    column(out).spacing(14).padding(4.0).into()
}

fn stat<'a>(key: &'static str, value: String) -> Element<'a, Msg> {
    row![ui::faint(key), Space::new().width(Length::Fill), ui::mono(value, 13.0, ui::INK)].into()
}

/// Reputation as a bar, filled to `frac`.
fn meter<'a>(frac: f32) -> Element<'a, Msg> {
    let filled = permille(frac);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if filled > 0 {
        parts.push(tint(Length::FillPortion(filled), ui::INK));
    }
    if filled < 1000 {
        parts.push(Space::new().width(Length::FillPortion(1000 - filled)).height(Length::Fill).into());
    }
    container(row(parts).height(Length::Fill)).width(Length::Fill).height(6.0).style(boxed).into()
}

/// An opinion: the filled part is the expected trust e; the lighter tail runs on by the uncertainty u.
fn bar<'a>(o: &Opinion) -> Element<'a, Msg> {
    let e = clamp(o.e, 0.0, 1.0);
    let tail = clamp(o.u, 0.0, 1.0).min(1.0 - e);
    let fill = permille(e);
    let tail_w = permille(tail).min(1000 - fill);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if fill > 0 {
        parts.push(tint(Length::FillPortion(fill), band(e)));
    }
    if tail_w > 0 {
        parts.push(tint(Length::FillPortion(tail_w), ui::INK_3.scale_alpha(0.3)));
    }
    if fill + tail_w < 1000 {
        parts.push(Space::new().width(Length::FillPortion(1000 - fill - tail_w)).height(Length::Fill).into());
    }
    let track = container(row(parts).height(Length::Fill)).width(Length::Fill).height(8.0).style(boxed);
    row![track, ui::mono(format!("{e:.2}"), 12.0, ui::INK)].spacing(8).into()
}

/// The page with nothing selected: how to read the graph.
fn legend<'a>() -> Element<'a, Msg> {
    let line = |colour: Color, height: f32| -> Element<'a, Msg> { tint_sized(18.0, height, colour) };
    let dashed = || -> Element<'a, Msg> {
        row![tint_sized(5.0, 2.0, ui::INK_3), tint_sized(5.0, 2.0, ui::INK_3), tint_sized(5.0, 2.0, ui::INK_3)].spacing(2).into()
    };
    let entry = |swatch: Element<'a, Msg>, text: &'static str| -> Element<'a, Msg> {
        row![container(swatch).width(24.0).center_y(Length::Shrink), ui::prose(text)].spacing(10).into()
    };
    column![
        column![ui::mono("Nothing selected", 18.0, ui::INK), ui::faint("Pick an agent to see how it sees the others, and how they see it.")].spacing(4),
        ui::label("Reading the graph"),
        entry(tint_sized(18.0, 10.0, RED), "trust below 0.4"),
        entry(tint_sized(18.0, 10.0, AMBER), "trust from 0.4 to 0.6"),
        entry(tint_sized(18.0, 10.0, GREEN), "trust above 0.6"),
        entry(line(ui::INK_3, 2.0), "solid, with an arrowhead: its own opinions"),
        entry(dashed(), "dashed: the opinions held of it"),
        entry(line(ui::INK_3, 4.0), "thicker line: a stronger contact"),
        entry(line(ui::INK_2, 3.0), "darker grey: a mutual pair"),
        ui::faint("Trusting an agent more or less moves the opinions held of it."),
    ]
    .spacing(10)
    .into()
}

// ---- Small pieces ----------------------------------------------------------

/// A share of a bar in thousandths, so a fraction can be a FillPortion.
fn permille(frac: f32) -> u16 {
    (clamp(frac, 0.0, 1.0) * 1000.0).round() as u16
}

/// A solid strip filling its portion of a row.
fn tint<'a>(width: Length, colour: Color) -> Element<'a, Msg> {
    container(Space::new().width(width).height(Length::Fill)).style(move |_: &Theme| filled(colour)).into()
}

fn tint_sized<'a>(width: f32, height: f32, colour: Color) -> Element<'a, Msg> {
    container(Space::new().width(width).height(height)).style(move |_: &Theme| filled(colour)).into()
}

fn filled(colour: Color) -> container::Style {
    container::Style { background: Some(Background::Color(colour)), ..container::Style::default() }
}

/// A square-cornered box with a thin ink rule.
fn boxed(_: &Theme) -> container::Style {
    container::Style { border: iced::Border { color: ui::INK, width: 1.0, radius: 0.0.into() }, ..container::Style::default() }
}

/// A peer's row, with a rule under it.
fn hairline_below(_: &Theme) -> container::Style {
    container::Style { border: iced::Border { color: ui::RULE, width: 1.0, radius: 0.0.into() }, ..container::Style::default() }
}

/// A wallet figure, shortened: 1.2k, 3.4M.
fn compact(value: f64) -> String {
    let v = value.abs();
    let sign = if value < 0.0 { "-" } else { "" };
    if v >= 1_000_000.0 {
        format!("{sign}{:.1}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{sign}{:.1}k", v / 1_000.0)
    } else {
        format!("{sign}{:.0}", v)
    }
}
