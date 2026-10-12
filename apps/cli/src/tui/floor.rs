//! The plaza drawn in the terminal: the rooms of the floor seen from above, each agent a glyph in
//! its room (coloured by role, its state shown), and the plaza's traffic — messages, model calls,
//! tool runs — as packets travelling between them, live from the hub's events.

use std::collections::{BTreeSet, HashMap};
use std::f32::consts::{FRAC_PI_2, TAU};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

/// How long a packet takes to travel from its source to its target, in seconds.
const PACKET_SECS: f32 = 1.0;
/// The most packets in flight at once; the oldest is dropped past this.
const MAX_PACKETS: usize = 96;
/// Working agents blink at this rate, in cycles per second.
const BLINK_HZ: f32 = 1.5;
/// Width of one agent's slot in the gateway: a glyph, a space, a short name.
const SLOT_W: u16 = 8;
/// The widest a room gets, in cells; a longer role name is cut to fit.
const ROOM_MAX_W: usize = 22;
/// The gateway's hue, as on the web floor.
const HUB_HUE: f32 = 25.0;
/// Tools that hand a message or a task to another agent (`to_cid`, or `assignee`).
const MESSAGE_TOOLS: [&str; 3] = ["plaza_send_message", "plaza_assign", "plaza_post_task"];

static NULL: Value = Value::Null;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Working,
    Idle,
    Offline,
}

impl Status {
    fn parse(s: &str) -> Status {
        match s {
            "working" => Status::Working,
            "idle" => Status::Idle,
            _ => Status::Offline,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Agent {
    cid: i64,
    name: String,
    role: String,
    status: Status,
    orchestrator: bool,
}

/// What a packet carries; each kind has its own glyph and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Message,
    ModelCall,
    ToolRun,
}

impl Kind {
    fn glyph(self) -> &'static str {
        match self {
            Kind::Message => "●",
            Kind::ModelCall => "◆",
            Kind::ToolRun => "■",
        }
    }

    fn color(self) -> Color {
        match self {
            Kind::Message => Color::Rgb(90, 200, 255),
            Kind::ModelCall => Color::Rgb(220, 120, 255),
            Kind::ToolRun => Color::Rgb(255, 200, 80),
        }
    }
}

/// A packet's end: an agent by cid, or the gateway in the middle of the plaza.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Hub,
    Agent(i64),
}

#[derive(Clone, Debug, PartialEq)]
struct Packet {
    kind: Kind,
    from: End,
    to: End,
    age: f32,
}

/// The floor's state. `tui::app` owns one, feeds it the agents list and every live event, ticks it
/// at the frame rate and renders it into the Floor tab.
#[derive(Debug, Default)]
pub struct FloorView {
    agents: Vec<Agent>,
    packets: Vec<Packet>,
    /// Blink phase of the working agents, in radians.
    phase: f32,
}

impl FloorView {
    pub fn new() -> FloorView {
        FloorView::default()
    }

    /// The agents (the `GET /v1/agents` answer), on load and on every poll.
    pub fn on_agents(&mut self, agents: &Value) {
        let list = agents.as_array().or_else(|| agents.get("agents").and_then(Value::as_array));
        let Some(list) = list else { return };
        self.agents = list.iter().filter_map(parse_agent).collect();
    }

    /// One live event from `/v1/ws` (`kind`, `data`), as they arrive.
    pub fn on_event(&mut self, kind: &str, data: &Value) {
        match kind {
            "conversation" => {
                let from = data.get("from_cid").and_then(Value::as_i64);
                let to = data.get("to_cid").and_then(Value::as_i64);
                if let (Some(from), Some(to)) = (from, to) {
                    self.send(Kind::Message, End::Agent(from), End::Agent(to));
                }
            }
            "transcript" => self.on_transcript(data),
            _ => {}
        }
    }

    /// Advances the animation by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.phase = (self.phase + dt * BLINK_HZ * TAU) % TAU;
        for p in &mut self.packets {
            p.age += dt;
        }
        self.packets.retain(|p| p.age < PACKET_SECS);
    }

    /// Draws the floor into `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if area.width < 10 || area.height < 4 {
            return;
        }
        let buf = frame.buffer_mut();
        let legend = area.height >= 8;
        let floor = Rect { height: if legend { area.height - 1 } else { area.height }, ..area };

        let rooms = layout(&self.room_sizes(), floor);
        let mut at: HashMap<i64, (f32, f32)> = HashMap::new();
        for (role, rect) in &rooms {
            let color = role_color(role);
            draw_box(buf, *rect, role, color);
            let members: Vec<&Agent> =
                self.agents.iter().filter(|a| !a.orchestrator && a.role == *role).collect();
            self.draw_members(buf, inner(*rect), 1, &members, color, &mut at);
        }

        let hub = hub_rect(floor);
        let hub_color = hue_color(HUB_HUE);
        draw_box(buf, hub, "gateway", hub_color);
        let orchestrators: Vec<&Agent> = self.agents.iter().filter(|a| a.orchestrator).collect();
        let hub_inner = inner(hub);
        let hub_cols = usize::from((hub_inner.width / SLOT_W).max(1));
        self.draw_members(buf, hub_inner, hub_cols, &orchestrators, hub_color, &mut at);
        let hub_at = ((hub.x + hub.width / 2) as f32, (hub.y + hub.height / 2) as f32);

        for p in &self.packets {
            let from = match p.from {
                End::Hub => Some(hub_at),
                End::Agent(cid) => at.get(&cid).copied(),
            };
            let to = match p.to {
                End::Hub => Some(hub_at),
                End::Agent(cid) => at.get(&cid).copied(),
            };
            if let (Some(from), Some(to)) = (from, to) {
                draw_packet(buf, floor, p, from, to);
            }
        }

        if legend {
            draw_legend(buf, area);
        }
    }

    /// Adds a packet between two known ends; events naming an unknown agent are dropped.
    fn send(&mut self, kind: Kind, from: End, to: End) {
        if !self.knows(from) || !self.knows(to) {
            return;
        }
        if self.packets.len() >= MAX_PACKETS {
            self.packets.remove(0);
        }
        self.packets.push(Packet { kind, from, to, age: 0.0 });
    }

    fn knows(&self, end: End) -> bool {
        match end {
            End::Hub => true,
            End::Agent(cid) => self.agents.iter().any(|a| a.cid == cid),
        }
    }

    /// The live transcript: model calls and tool runs leave an agent for the gateway, messages
    /// and task hand-offs travel between agents.
    fn on_transcript(&mut self, entry: &Value) {
        let Some(cid) = entry.get("cid").and_then(Value::as_i64) else { return };
        let inner = entry.get("data").unwrap_or(&NULL);
        let me = End::Agent(cid);
        match entry.get("kind").and_then(Value::as_str).unwrap_or_default() {
            "model_request" => self.send(Kind::ModelCall, me, End::Hub),
            "tool_request" => {
                let mut tool = text_of(inner, "tool");
                if tool.is_empty() {
                    tool = text_of(inner, "name");
                }
                if MESSAGE_TOOLS.contains(&tool.as_str()) {
                    let input = inner.get("input").unwrap_or(&NULL);
                    let to = input.get("to_cid").or_else(|| input.get("assignee")).and_then(Value::as_i64);
                    if let Some(to) = to {
                        self.send(Kind::Message, me, End::Agent(to));
                    }
                } else {
                    self.send(Kind::ToolRun, me, End::Hub);
                }
            }
            "work_start" => {
                let from = inner.get("origin").and_then(|o| o.get("from_cid")).and_then(Value::as_i64);
                match from {
                    Some(from) => self.send(Kind::Message, End::Agent(from), me),
                    None if inner.get("kind").and_then(Value::as_str) == Some("task") => {
                        if let Some(boss) = self.agents.iter().find(|a| a.orchestrator).map(|a| a.cid) {
                            self.send(Kind::Message, End::Agent(boss), me);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// The non-gateway roles, sorted: one room each.
    fn room_roles(&self) -> Vec<String> {
        let set: BTreeSet<&str> =
            self.agents.iter().filter(|a| !a.orchestrator).map(|a| a.role.as_str()).collect();
        set.into_iter().map(str::to_string).collect()
    }

    /// Each non-gateway role with the size its room needs.
    fn room_sizes(&self) -> Vec<(String, u16, u16)> {
        self.room_roles()
            .into_iter()
            .map(|role| {
                let names: Vec<&str> = self
                    .agents
                    .iter()
                    .filter(|a| !a.orchestrator && a.role == role)
                    .map(|a| a.name.as_str())
                    .collect();
                let (w, h) = room_size(&role, &names);
                (role, w, h)
            })
            .collect()
    }

    /// Draws the agents of one room (or the gateway) into `inner`, `cols` to a row, recording where
    /// each glyph is.
    fn draw_members(
        &self,
        buf: &mut Buffer,
        inner: Rect,
        cols: usize,
        members: &[&Agent],
        color: Color,
        at: &mut HashMap<i64, (f32, f32)>,
    ) {
        if inner.width == 0 || inner.height == 0 || members.is_empty() {
            return;
        }
        let capacity = cols * inner.height as usize;
        let shown = if members.len() > capacity { capacity - 1 } else { members.len() };
        let blink_on = self.phase.sin() > 0.0;
        for (i, a) in members[..shown].iter().enumerate() {
            let (x, y) = slot(inner, cols, i);
            let (glyph, glyph_style, name_style) = agent_style(a, color, blink_on);
            put(buf, x, y, glyph, glyph_style);
            // In a multi-column row a name stops at its slot's edge; a room's name runs to its wall.
            let edge = if cols > 1 { (x + SLOT_W).min(inner.right()) } else { inner.right() };
            let room = edge.saturating_sub(x + 2) as usize;
            put(buf, x + 2, y, &trunc(&a.name, room), name_style);
            at.insert(a.cid, (x as f32, y as f32));
        }
        if members.len() > shown {
            let (x, y) = slot(inner, cols, capacity - 1);
            let more = format!("+{}", members.len() - shown);
            put(buf, x, y, &more, Style::default().fg(Color::DarkGray));
        }
    }
}

fn parse_agent(v: &Value) -> Option<Agent> {
    let cid = v.get("cid")?.as_i64()?;
    let mut name = text_of(v, "name");
    if name.is_empty() {
        name = format!("#{cid}");
    }
    Some(Agent {
        cid,
        name,
        role: text_of(v, "role"),
        status: Status::parse(&text_of(v, "status")),
        orchestrator: v.get("tier").and_then(Value::as_str) == Some("orchestrator"),
    })
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The size of one room: wide enough for the full role name and for its longest "● name" line,
/// one row per member and at least three rows in all.
fn room_size(role: &str, names: &[&str]) -> (u16, u16) {
    let longest = names.iter().map(|n| n.chars().count() + 2).max().unwrap_or(0);
    let w = (role.chars().count() + 6).max(longest + 4).min(ROOM_MAX_W);
    let h = (names.len() + 2).max(3);
    (w as u16, h as u16)
}

/// Places the rooms `(role, width, height)` in `area`: on a ring around its centre when they fit
/// there without touching, otherwise in a tidy grid that fills it. No two rooms overlap; a room
/// larger than the area is cut down to it.
pub fn layout(rooms: &[(String, u16, u16)], area: Rect) -> Vec<(String, Rect)> {
    let n = rooms.len();
    if n == 0 || area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let rooms: Vec<(String, u16, u16)> =
        rooms.iter().map(|(role, w, h)| (role.clone(), (*w).min(area.width), (*h).min(area.height))).collect();
    let (w, h) = (area.width as f32, area.height as f32);
    let max_w = rooms.iter().map(|r| r.1).max().unwrap_or(0) as f32;
    let max_h = rooms.iter().map(|r| r.2).max().unwrap_or(0) as f32;
    let (cx, cy) = (area.x as f32 + w / 2.0, area.y as f32 + h / 2.0);
    // The largest ring on which every room still lies inside the area.
    let (rx, ry) = ((w - max_w).max(0.0) / 2.0, (h - max_h).max(0.0) / 2.0);
    let ring: Vec<Rect> = rooms
        .iter()
        .enumerate()
        .map(|(i, &(_, rw, rh))| {
            let a = -FRAC_PI_2 + TAU * i as f32 / n as f32;
            let x = cx + rx * a.cos() - rw as f32 / 2.0;
            let y = cy + ry * a.sin() - rh as f32 / 2.0;
            Rect::new(snap(x, area.x, area.right() - rw), snap(y, area.y, area.bottom() - rh), rw, rh)
        })
        .collect();
    if crowded(&ring) {
        return grid(&rooms, area).unwrap_or_default();
    }
    rooms.into_iter().map(|r| r.0).zip(ring).collect()
}

/// The rooms in rows of equal cells, as many columns as the area takes, centred. `None` when not
/// even one room per cell fits.
fn grid(rooms: &[(String, u16, u16)], area: Rect) -> Option<Vec<(String, Rect)>> {
    let n = rooms.len();
    let cell_w = rooms.iter().map(|r| r.1).max()? + 1;
    let cell_h = rooms.iter().map(|r| r.2).max()? + 1;
    // A block of `c` columns is `c * cell` cells less the last gap; it must fit the area.
    let fits = |c: usize| {
        let rows = n.div_ceil(c);
        c * usize::from(cell_w) <= usize::from(area.width) + 1
            && rows * usize::from(cell_h) <= usize::from(area.height) + 1
    };
    let cols = (1..=n).rev().find(|&c| fits(c))?;
    let rows = n.div_ceil(cols);
    let block_w = cols as u16 * cell_w - 1;
    let block_h = rows as u16 * cell_h - 1;
    let x0 = area.x + area.width.saturating_sub(block_w) / 2;
    let y0 = area.y + area.height.saturating_sub(block_h) / 2;
    Some(
        rooms
            .iter()
            .enumerate()
            .map(|(i, (role, w, h))| {
                let (col, row) = ((i % cols) as u16, (i / cols) as u16);
                (role.clone(), Rect::new(x0 + col * cell_w, y0 + row * cell_h, *w, *h))
            })
            .collect(),
    )
}

fn snap(v: f32, lo: u16, hi: u16) -> u16 {
    v.round().clamp(lo as f32, hi as f32) as u16
}

/// True when two rooms are closer than one free cell.
fn crowded(rects: &[Rect]) -> bool {
    rects.iter().enumerate().any(|(i, a)| {
        rects[i + 1..]
            .iter()
            .any(|b| !(a.x > b.right() || b.x > a.right() || a.y > b.bottom() || b.y > a.bottom()))
    })
}

/// The gateway's box, in the middle of the floor.
fn hub_rect(area: Rect) -> Rect {
    let w = (area.width / 3).clamp(10, 26).min(area.width);
    let h = 5.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn inner(r: Rect) -> Rect {
    Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2))
}

/// Where the i-th agent of a room sits: left to right, then down.
fn slot(inner: Rect, cols: usize, i: usize) -> (u16, u16) {
    let col = (i % cols) as u16;
    let row = (i / cols) as u16;
    (inner.x + col * SLOT_W, inner.y + row)
}

/// The glyph, its style and the name's style for one agent.
fn agent_style(a: &Agent, color: Color, blink_on: bool) -> (&'static str, Style, Style) {
    match a.status {
        Status::Working => {
            let m = if blink_on { Modifier::BOLD } else { Modifier::DIM };
            ("●", Style::default().fg(color).add_modifier(m), Style::default().fg(color))
        }
        Status::Idle => ("●", Style::default().fg(color), Style::default().fg(Color::Gray)),
        Status::Offline => (
            "○",
            Style::default().fg(color).add_modifier(Modifier::DIM),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ),
    }
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Writes one cell or string, clipped to the buffer.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style) {
    if buf.area.contains(Position::new(x, y)) {
        buf.set_string(x, y, s, style);
    }
}

/// A room outline with its label on the top edge.
fn draw_box(buf: &mut Buffer, r: Rect, label: &str, color: Color) {
    if r.width < 2 || r.height < 2 {
        return;
    }
    let style = Style::default().fg(color).add_modifier(Modifier::DIM);
    let (x0, y0) = (r.x, r.y);
    let (x1, y1) = (r.x + r.width - 1, r.y + r.height - 1);
    for x in x0 + 1..x1 {
        put(buf, x, y0, "─", style);
        put(buf, x, y1, "─", style);
    }
    for y in y0 + 1..y1 {
        put(buf, x0, y, "│", style);
        put(buf, x1, y, "│", style);
    }
    put(buf, x0, y0, "┌", style);
    put(buf, x1, y0, "┐", style);
    put(buf, x0, y1, "└", style);
    put(buf, x1, y1, "┘", style);
    let max = (r.width as usize).saturating_sub(4);
    if max > 2 && !label.is_empty() {
        let text = format!(" {} ", trunc(label, max - 2));
        put(buf, x0 + 2, y0, &text, Style::default().fg(color).add_modifier(Modifier::BOLD));
    }
}

/// Moves a packet along a bowed arc from `from` to `to` over `PACKET_SECS`, with a short trail,
/// fading (dimmed) over its last stretch.
fn draw_packet(buf: &mut Buffer, floor: Rect, p: &Packet, from: (f32, f32), to: (f32, f32)) {
    let t = (p.age / PACKET_SECS).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    let ctrl = (mid.0 - dy * 0.2, mid.1 + dx * 0.2);
    let point = |u: f32| {
        let v = 1.0 - u;
        (
            v * v * from.0 + 2.0 * v * u * ctrl.0 + u * u * to.0,
            v * v * from.1 + 2.0 * v * u * ctrl.1 + u * u * to.1,
        )
    };
    let modifier = if t > 0.7 { Modifier::DIM } else { Modifier::BOLD };
    let style = Style::default().fg(p.kind.color()).add_modifier(modifier);
    for k in 1..=3 {
        let (x, y) = point((s - 0.05 * k as f32).max(0.0));
        put_cell(buf, floor, x, y, "·", style);
    }
    let (x, y) = point(s);
    put_cell(buf, floor, x, y, p.kind.glyph(), style);
}

fn put_cell(buf: &mut Buffer, floor: Rect, x: f32, y: f32, s: &str, style: Style) {
    let (x, y) = (x.round(), y.round());
    if x < floor.x as f32 || y < floor.y as f32 || x >= floor.right() as f32 || y >= floor.bottom() as f32 {
        return;
    }
    put(buf, x as u16, y as u16, s, style);
}

fn draw_legend(buf: &mut Buffer, area: Rect) {
    let line = Line::from(vec![
        Span::styled("● ", Style::default().fg(Kind::Message.color())),
        Span::raw("message  "),
        Span::styled("◆ ", Style::default().fg(Kind::ModelCall.color())),
        Span::raw("model call  "),
        Span::styled("■ ", Style::default().fg(Kind::ToolRun.color())),
        Span::raw("tool run  "),
        Span::styled("● ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw("working  "),
        Span::styled("● ", Style::default().fg(Color::Gray)),
        Span::raw("idle  "),
        Span::styled("○ ", Style::default().fg(Color::DarkGray)),
        Span::raw("offline"),
    ]);
    buf.set_line(area.x, area.bottom() - 1, &line, area.width);
}

/// A colour for a hue in degrees, at a fixed saturation and lightness.
fn hue_color(hue: f32) -> Color {
    let (s, l) = (0.6_f32, 0.62_f32);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = hue.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to8 = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(to8(r), to8(g), to8(b))
}

/// A role's colour: the same hash the web floor uses to pick a hue.
fn role_color(role: &str) -> Color {
    let mut h: u32 = 0;
    for b in role.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    hue_color((h % 360) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    #[test]
    fn rooms_fit_the_area_and_do_not_overlap() {
        let area = Rect::new(0, 0, 120, 40);
        let rooms: Vec<(String, u16, u16)> = (0..8).map(|i| (format!("role{i}"), 12, 3)).collect();
        let placed = layout(&rooms, area);
        assert_eq!(placed.len(), 8);
        for (role, r) in &placed {
            assert!(
                r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom(),
                "{role} outside the area: {r:?}"
            );
        }
        let rects: Vec<Rect> = placed.iter().map(|(_, r)| *r).collect();
        assert!(!crowded(&rects), "rooms touch: {rects:?}");
    }

    #[test]
    fn sixteen_roles_show_their_full_names_without_overlap() {
        let roles = [
            "architect", "edge-operator", "orchestrator", "designer", "developer", "reviewer", "tester",
            "security", "data-engineer", "devops", "product-owner", "writer", "analyst", "support",
            "researcher", "qa-lead",
        ];
        let agents: Vec<Value> = roles
            .iter()
            .enumerate()
            .map(|(i, role)| json!({"cid": i as i64 + 1, "name": format!("agent{i:02}"), "role": role, "status": "idle"}))
            .collect();
        let mut view = FloorView::new();
        view.on_agents(&Value::Array(agents));

        let placed = layout(&view.room_sizes(), Rect::new(0, 0, 160, 39));
        assert_eq!(placed.len(), 16);
        let rects: Vec<Rect> = placed.iter().map(|(_, r)| *r).collect();
        assert!(!crowded(&rects), "rooms overlap: {rects:?}");

        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        term.draw(|f| view.render(f, f.area())).unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<String> = (0..40).map(|y| (0..160).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
        for role in roles {
            let label = format!(" {role} ");
            assert!(rows.iter().any(|row| row.contains(&label)), "{role} is not shown in full");
        }
    }

    #[test]
    fn message_packet_reaches_its_target_then_is_removed() {
        let mut view = FloorView::new();
        view.on_agents(&json!([
            {"cid": 1, "name": "ada", "role": "dev", "status": "working"},
            {"cid": 2, "name": "bob", "role": "ops", "status": "idle"}
        ]));
        view.on_event("conversation", &json!({"from_cid": 1, "to_cid": 2}));
        view.on_event("conversation", &json!({"from_cid": 1, "to_cid": 9}));
        assert_eq!(view.packets.len(), 1);
        assert_eq!(view.packets[0].to, End::Agent(2));
        view.tick(0.5);
        assert_eq!(view.packets.len(), 1, "still in flight at half way");
        view.tick(0.25);
        view.tick(0.25);
        assert!(view.packets.is_empty(), "removed once it has arrived");
    }

    #[test]
    fn agents_update_in_place_and_the_floor_renders() {
        let mut view = FloorView::new();
        view.on_agents(&json!([
            {"cid": 1, "name": "ada", "role": "dev", "status": "idle"},
            {"cid": 2, "name": "boss", "role": "", "status": "working", "tier": "orchestrator"}
        ]));
        view.on_event("conversation", &json!({"from_cid": 1, "to_cid": 2}));
        view.on_event("transcript", &json!({"cid": 1, "kind": "tool_request", "data": {"tool": "bash", "input": {}}}));
        view.on_agents(&json!([
            {"cid": 1, "name": "ada2", "role": "dev", "status": "offline"},
            {"cid": 2, "name": "boss", "role": "", "status": "idle", "tier": "orchestrator"}
        ]));
        assert_eq!(view.packets.len(), 2, "on_agents keeps packets in flight");
        assert_eq!(view.agents[0].name, "ada2");
        assert_eq!(view.agents[0].status, Status::Offline);

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| view.render(f, f.area())).unwrap();
    }
}
