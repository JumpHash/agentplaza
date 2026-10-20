//! The memory graph (the web's `memory-graph.ts` and the `.mg` SVG): an agent's notes as rings on a sheet-2 panel
//! with the kind's glyph inside and the title under each, `[[links]]` as solid ink-3 lines and shared tags as dashed
//! ones. The layout is the desktop's force simulation (seeded, so a given set of notes always lands in the same
//! place). A click opens a note in the host's reader and the open one is ringed in signal; dragging pans, dragging a
//! note moves it while it is held, the wheel zooms around the pointer, a right click fits the view again, and the
//! buttons in the top-right corner zoom in, zoom out and fit. Tab and Shift+Tab walk the notes in a stable order (by
//! slug), the focused one is ringed, and Enter or Space opens it. The find text leaves out the notes whose titles miss it.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;

use iced::keyboard::{self, key::Named};
use iced::widget::canvas::{self, Canvas, Event, Frame, LineDash, Path, Stroke, Text};
use iced::widget::{container, responsive};
use iced::{
    Border, Color, Element, Length, Pixels, Point, Rectangle, Renderer, Size, Theme, alignment,
    mouse,
};

use super::agent_side::Note;

const LINK_LEN: f32 = 150.0;
const TAG_LEN: f32 = 230.0;
const SEED: u64 = 0x2545_F491_4F6C_DD1D;

/// The keys the graph takes (the host's subscription passes only the ones no focused text field captured).
#[derive(Debug, Clone, Copy)]
pub enum Key {
    /// Tab walks the notes forwards, Shift+Tab backwards.
    Tab { back: bool },
    /// Enter or Space opens the note in focus.
    Open,
}

/// The graph's key for a key press, if it takes it.
pub fn key(key: &keyboard::Key, shift: bool) -> Option<Key> {
    match key {
        keyboard::Key::Named(Named::Tab) => Some(Key::Tab { back: shift }),
        keyboard::Key::Named(Named::Enter | Named::Space) => Some(Key::Open),
        _ => None,
    }
}

/// The note the keyboard moves to: Tab walks the notes in a stable order (by slug), from the first (or, backwards, the
/// last) when none is in focus.
pub fn walk(layout: &Layout, kb: Option<&str>, back: bool) -> Option<String> {
    let mut order: Vec<usize> = (0..layout.nodes.len()).collect();
    order.sort_by(|&a, &b| layout.nodes[a].slug.cmp(&layout.nodes[b].slug));
    let n = order.len();
    if n == 0 {
        return None;
    }
    let cur = kb.and_then(|s| order.iter().position(|&i| layout.nodes[i].slug == s));
    let next = match (cur, back) {
        (None, false) => 0,
        (None, true) => n - 1,
        (Some(c), false) => (c + 1) % n,
        (Some(c), true) => (c + n - 1) % n,
    };
    Some(layout.nodes[order[next]].slug.clone())
}

/// What the graph tells the host.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A note was clicked (or Enter or Space was pressed on it): the host reads it into its reader.
    Open(String),
    /// A note is dragged to (x, y), in graph coordinates: the host holds it there.
    Drag { slug: String, x: f32, y: f32 },
    /// The drag ends: the host lets the note go.
    Drop,
}

#[derive(Debug, Clone, Copy)]
struct Cam {
    k: f32,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone)]
struct Node {
    slug: String,
    title: String,
    kind: String,
    r: f32,
    tag_w: f32,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    a: usize,
    b: usize,
    tag: bool,
}

/// The settled layout of one note set. Rebuilt only when the notes change; the host keeps it beside the notes.
#[derive(Debug, Default, Clone)]
pub struct Layout {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    key: Vec<(String, u64)>,
    /// The note held under the pointer while it is dragged: its index and where it sits.
    held: Option<(usize, f32, f32)>,
}

/// A small xorshift generator: the seed makes the layout repeatable.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }
}

impl Layout {
    /// Recomputes the layout when the set of notes (or a note's update stamp) changed. Notes that were already placed
    /// start where they were, so a refresh only nudges the graph.
    pub fn refresh(&mut self, notes: &[Note]) {
        let key: Vec<(String, u64)> = notes
            .iter()
            .map(|n| (n.slug.clone(), n.updated_ms))
            .collect();
        if key == self.key {
            return;
        }
        let prev: HashMap<String, (f32, f32)> = self
            .nodes
            .iter()
            .map(|n| (n.slug.clone(), (n.x, n.y)))
            .collect();
        *self = Layout::build(notes, &prev, key);
    }

    /// Holds a note at (x, y) while it is dragged; the rest of the graph settles around it.
    pub fn hold(&mut self, slug: &str, x: f32, y: f32) {
        let Some(i) = self.nodes.iter().position(|n| n.slug == slug) else {
            return;
        };
        self.held = Some((i, x, y));
        let mut rng = Rng(SEED);
        for _ in 0..8 {
            self.tick(0.3, &mut rng);
        }
    }

    /// Lets the held note go: it floats back into place around the others.
    pub fn release(&mut self) {
        if self.held.take().is_some() {
            let mut rng = Rng(SEED);
            for _ in 0..8 {
                self.tick(0.3, &mut rng);
            }
        }
    }

    fn build(notes: &[Note], prev: &HashMap<String, (f32, f32)>, key: Vec<(String, u64)>) -> Self {
        let mut rng = Rng(SEED);
        let mut nodes: Vec<Node> = notes
            .iter()
            .map(|n| {
                let label = clip(&n.title, 26);
                let (x, y) = prev.get(&n.slug).copied().unwrap_or_else(|| {
                    let a = rng.unit() * TAU;
                    (a.cos() * 60.0, a.sin() * 60.0)
                });
                Node {
                    slug: n.slug.clone(),
                    title: n.title.clone(),
                    kind: n.kind.clone(),
                    r: 17.0 + 4.0 * ((n.size as f32) / 200.0 + 1.0).log10(),
                    tag_w: label.chars().count() as f32 * 6.7 + 18.0,
                    x,
                    y,
                    vx: 0.0,
                    vy: 0.0,
                }
            })
            .collect();

        // `[[links]]` (by slug, or by the web's normalised form), then a faint edge between neighbours sharing a tag.
        let index: HashMap<&str, usize> = notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.slug.as_str(), i))
            .collect();
        let mut edges = Vec::new();
        let mut seen = HashSet::new();
        for (i, n) in notes.iter().enumerate() {
            for r in &n.links {
                if let Some(j) = resolve(&index, r) {
                    push_edge(&mut edges, &mut seen, i, j, false);
                }
            }
        }
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, n) in notes.iter().enumerate() {
            for t in &n.tags {
                match groups.iter().position(|(g, _)| g == t) {
                    Some(p) => groups[p].1.push(i),
                    None => groups.push((t.clone(), vec![i])),
                }
            }
        }
        for (_, members) in &groups {
            for w in members.windows(2) {
                push_edge(&mut edges, &mut seen, w[0], w[1], true);
            }
        }
        // Cooling: a fresh layout starts hot, a refresh starts warm. Same decay and stop as the web.
        let mut alpha = if prev.is_empty() { 1.0 } else { 0.3 };
        nodes.shrink_to_fit();
        let mut lay = Layout {
            nodes,
            edges,
            key,
            held: None,
        };
        loop {
            lay.tick(alpha, &mut rng);
            alpha *= 0.985;
            if alpha <= 0.004 {
                break;
            }
        }
        lay
    }

    fn tick(&mut self, alpha: f32, rng: &mut Rng) {
        let n = self.nodes.len();
        for i in 0..n {
            for j in (i + 1)..n {
                let (left, right) = self.nodes.split_at_mut(j);
                let a = &mut left[i];
                let b = &mut right[0];
                let mut dx = b.x - a.x;
                let mut dy = b.y - a.y;
                let mut d2 = dx * dx + dy * dy;
                if d2 < 1.0 {
                    dx = rng.unit() - 0.5;
                    dy = rng.unit() - 0.5;
                    d2 = 1.0;
                }
                let d = d2.sqrt();
                // Repulsion: everything keeps its distance.
                let f = 14000.0 / d2 * alpha;
                a.vx -= dx / d * f;
                a.vy -= dy / d * f;
                b.vx += dx / d * f;
                b.vy += dy / d * f;
                // Collision on the bubble plus its name tag, so labels never pile up.
                let ra = a.r.max(a.tag_w / 2.0) + 14.0;
                let rb = b.r.max(b.tag_w / 2.0) + 14.0;
                let overlap = ra + rb - d;
                if overlap > 0.0 {
                    let push = overlap / d * 0.5;
                    a.x -= dx * push;
                    a.y -= dy * push * 0.6;
                    b.x += dx * push;
                    b.y += dy * push * 0.6;
                }
            }
        }
        for e in 0..self.edges.len() {
            let Edge { a, b, tag } = self.edges[e];
            let dx = self.nodes[b].x - self.nodes[a].x;
            let dy = self.nodes[b].y - self.nodes[a].y;
            let h = dx.hypot(dy);
            let d = if h == 0.0 { 1.0 } else { h };
            let k = (if tag { 0.012 } else { 0.05 }) * alpha;
            let f = ((d - if tag { TAG_LEN } else { LINK_LEN }) / d) * k;
            self.nodes[a].vx += dx * f;
            self.nodes[a].vy += dy * f;
            self.nodes[b].vx -= dx * f;
            self.nodes[b].vy -= dy * f;
        }
        for p in &mut self.nodes {
            p.vx -= p.x * 0.008 * alpha;
            p.vy -= p.y * 0.008 * alpha;
            p.vx *= 0.6;
            p.vy *= 0.6;
            p.x += p.vx;
            p.y += p.vy;
        }
        // The held note stays where the pointer holds it, whatever pushed it.
        if let Some((i, x, y)) = self.held {
            let p = &mut self.nodes[i];
            p.x = x;
            p.y = y;
            p.vx = 0.0;
            p.vy = 0.0;
        }
    }
}

fn push_edge(
    edges: &mut Vec<Edge>,
    seen: &mut HashSet<(usize, usize)>,
    a: usize,
    b: usize,
    tag: bool,
) {
    if a == b {
        return;
    }
    if seen.insert((a.min(b), a.max(b))) {
        edges.push(Edge { a, b, tag });
    }
}

fn resolve(index: &HashMap<&str, usize>, r: &str) -> Option<usize> {
    index
        .get(r)
        .copied()
        .or_else(|| index.get(norm(r).as_str()).copied())
}

/// The web's slug form: lower case, runs of other characters become one dash, no edge dashes, at most 60 characters.
fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').chars().take(60).collect()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let head: String = s.chars().take(n - 1).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}

/// The glyph inside a note's ring, by kind (the web's `KG`).
fn glyph(kind: &str) -> &'static str {
    match kind {
        "person" => "@",
        "journal" => "#",
        _ => "¶",
    }
}

/// Whether a note takes part in the view: its title holds the find text (the web draws only the notes it is given).
fn shown(n: &Node, query: &str) -> bool {
    query.is_empty() || n.title.to_lowercase().contains(query)
}

fn fade(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// The camera that frames every note, as the web's `fit`.
fn fit(lay: &Layout, bounds: Rectangle) -> Cam {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for n in &lay.nodes {
        let half = n.r.max(n.tag_w / 2.0);
        x0 = x0.min(n.x - half);
        x1 = x1.max(n.x + half);
        y0 = y0.min(n.y - n.r);
        y1 = y1.max(n.y + n.r + 30.0);
    }
    let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));
    let k = ((w - 60.0) / (x1 - x0).max(1e-3))
        .min((h - 60.0) / (y1 - y0).max(1e-3))
        .clamp(0.25, 1.6);
    Cam {
        k,
        x: w / 2.0 - (x0 + x1) / 2.0 * k,
        y: h / 2.0 - (y0 + y1) / 2.0 * k,
    }
}

/// The zoom limits and the step one click of + or − takes.
const ZOOM_MIN: f32 = 0.2;
const ZOOM_MAX: f32 = 3.0;
const ZOOM_STEP: f32 = 1.25;

/// The camera zoomed by `factor` around `p` (canvas-relative): the point under `p` stays under it.
fn zoom_about(cam: Cam, p: Point, factor: f32) -> Cam {
    let k = (cam.k * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    let s = k / cam.k;
    Cam {
        k,
        x: p.x - (p.x - cam.x) * s,
        y: p.y - (p.y - cam.y) * s,
    }
}

/// The buttons in the canvas's top-right corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    ZoomIn,
    ZoomOut,
    Fit,
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Button::ZoomIn => "+",
            Button::ZoomOut => "−",
            Button::Fit => "Fit",
        }
    }
}

const BUTTON_H: f32 = 26.0;
const BUTTON_W: f32 = 30.0;
const FIT_W: f32 = 44.0;
const BUTTON_GAP: f32 = 6.0;
const BUTTON_MARGIN: f32 = 10.0;

/// Where each button sits: a row along the top-right corner, zoom in leftmost and fit rightmost.
fn buttons(bounds: Rectangle) -> [(Button, Rectangle); 3] {
    let mut right = bounds.width - BUTTON_MARGIN;
    [Button::Fit, Button::ZoomOut, Button::ZoomIn].map(|b| {
        let w = if b == Button::Fit { FIT_W } else { BUTTON_W };
        let rect = Rectangle {
            x: right - w,
            y: BUTTON_MARGIN,
            width: w,
            height: BUTTON_H,
        };
        right = rect.x - BUTTON_GAP;
        (b, rect)
    })
}

/// The button under a point (canvas-relative), if one is.
fn button_at(bounds: Rectangle, p: Point) -> Option<Button> {
    buttons(bounds)
        .into_iter()
        .find(|(_, r)| r.contains(p))
        .map(|(b, _)| b)
}

/// The node under a point (canvas-relative), the nearest one when several overlap.
fn hit(lay: &Layout, cam: Cam, p: Point, query: &str) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, n) in lay.nodes.iter().enumerate() {
        if !shown(n, query) {
            continue;
        }
        let d = (p.x - (cam.x + n.x * cam.k)).hypot(p.y - (cam.y + n.y * cam.k));
        let reach = (RING_R * cam.k).max(6.0) + 3.0;
        if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// The view's own state: the camera (`None` until the user moves it, then the fit is kept), the pointer, the hover,
/// the note being dragged and the press that may open a note.
#[derive(Debug, Default)]
pub struct View {
    cam: Option<Cam>,
    press: Option<Point>,
    drag: Option<Point>,
    moved: bool,
    grab: Option<String>,
    hover: Option<usize>,
}

struct GraphView<'a> {
    layout: &'a Layout,
    /// The find text, trimmed and lower case: notes whose titles lack it are left out.
    query: String,
    /// The note the keyboard is on (ringed), from the host.
    kb: Option<&'a str>,
    /// The note being read (filled in signal), from the host.
    reading: Option<&'a str>,
}

/// The ring's radius in graph units (the web's `r="18"`).
const RING_R: f32 = 18.0;

impl GraphView<'_> {
    fn cam(&self, st: &View, bounds: Rectangle) -> Cam {
        st.cam.unwrap_or_else(|| fit(self.layout, bounds))
    }
}

impl canvas::Program<Msg> for GraphView<'_> {
    type State = View;

    fn update(
        &self,
        st: &mut View,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = cursor.position_in(bounds)?;
                if let Some(last) = st.drag {
                    let (dx, dy) = (p.x - last.x, p.y - last.y);
                    if !st.moved && dx.hypot(dy) < 4.0 {
                        return None;
                    }
                    st.moved = true;
                    st.drag = Some(p);
                    let cam = self.cam(st, bounds);
                    if let Some(slug) = st.grab.clone() {
                        // A note under the press follows the pointer; the camera holds still meanwhile.
                        st.cam = Some(cam);
                        let (x, y) = ((p.x - cam.x) / cam.k, (p.y - cam.y) / cam.k);
                        return Some(canvas::Action::publish(Msg::Drag { slug, x, y }));
                    }
                    st.cam = Some(Cam {
                        x: cam.x + dx,
                        y: cam.y + dy,
                        ..cam
                    });
                    return Some(canvas::Action::request_redraw());
                }
                let hover = hit(self.layout, self.cam(st, bounds), p, &self.query);
                if hover != st.hover {
                    st.hover = hover;
                    return Some(canvas::Action::request_redraw());
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                if let Some(b) = button_at(bounds, p).filter(|_| !self.layout.nodes.is_empty()) {
                    let cam = self.cam(st, bounds);
                    let centre = Point::new(bounds.width / 2.0, bounds.height / 2.0);
                    match b {
                        Button::ZoomIn => st.cam = Some(zoom_about(cam, centre, ZOOM_STEP)),
                        Button::ZoomOut => st.cam = Some(zoom_about(cam, centre, 1.0 / ZOOM_STEP)),
                        Button::Fit => st.cam = None,
                    }
                    return Some(canvas::Action::request_redraw());
                }
                st.press = Some(p);
                st.drag = Some(p);
                st.moved = false;
                let cam = self.cam(st, bounds);
                st.grab = hit(self.layout, cam, p, &self.query)
                    .map(|i| self.layout.nodes[i].slug.clone());
                None
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was_moved = std::mem::take(&mut st.moved);
                st.drag = None;
                let grabbed = st.grab.take();
                let press = st.press.take()?;
                if was_moved {
                    return grabbed.map(|_| canvas::Action::publish(Msg::Drop));
                }
                let i = hit(self.layout, self.cam(st, bounds), press, &self.query)?;
                Some(canvas::Action::publish(Msg::Open(
                    self.layout.nodes[i].slug.clone(),
                )))
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                st.cam = None;
                Some(canvas::Action::request_redraw())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * 60.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                // Zoom around the pointer, as the web does.
                let cam = self.cam(st, bounds);
                st.cam = Some(zoom_about(cam, p, (-dy * 0.0015).exp()));
                Some(canvas::Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        st: &View,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let pal = crate::theme::palette();
        let mut frame = Frame::new(renderer, bounds.size());
        let lay = self.layout;
        if lay.nodes.is_empty() {
            return vec![frame.into_geometry()];
        }
        let cam = self.cam(st, bounds);
        let to_screen = |x: f32, y: f32| Point::new(cam.x + x * cam.k, cam.y + y * cam.k);
        let kb = self
            .kb
            .and_then(|s| lay.nodes.iter().position(|n| n.slug == s));
        let open = self
            .reading
            .and_then(|s| lay.nodes.iter().position(|n| n.slug == s));
        let shows = |i: usize| shown(&lay.nodes[i], &self.query);
        // Links are solid lines, shared tags dashed (the web's `.mg-link` and `.mg-tag`).
        for e in &lay.edges {
            if !shows(e.a) || !shows(e.b) {
                continue;
            }
            let (na, nb) = (&lay.nodes[e.a], &lay.nodes[e.b]);
            let path = Path::line(to_screen(na.x, na.y), to_screen(nb.x, nb.y));
            let stroke = if e.tag {
                Stroke {
                    line_dash: LineDash {
                        segments: &[3.0, 4.0],
                        offset: 0,
                    },
                    ..Stroke::default()
                }
                .with_color(fade(pal.ink_3, 0.45))
                .with_width(1.0)
            } else {
                Stroke::default()
                    .with_color(fade(pal.ink_3, 0.8))
                    .with_width(1.6)
            };
            frame.stroke(&path, stroke);
        }
        // A note: its ring (signal, filled soft when it is the one read), the kind's glyph inside, its title under.
        for (i, n) in lay.nodes.iter().enumerate() {
            if !shows(i) {
                continue;
            }
            let c = to_screen(n.x, n.y);
            let r = RING_R * cam.k;
            let is_open = open == Some(i);
            let thick = is_open || st.hover == Some(i);
            frame.fill(
                &Path::circle(c, r),
                if is_open { pal.signal_soft } else { pal.panel },
            );
            frame.stroke(
                &Path::circle(c, r),
                Stroke::default()
                    .with_color(pal.signal)
                    .with_width(if thick { 2.5 } else { 1.5 }),
            );
            if kb == Some(i) {
                frame.stroke(
                    &Path::circle(c, r + 5.0),
                    Stroke::default().with_color(pal.ink).with_width(2.5),
                );
            }
            frame.fill_text(Text {
                content: glyph(&n.kind).to_string(),
                position: c,
                color: pal.ink_2,
                size: Pixels(13.0 * cam.k),
                align_x: iced::advanced::text::Alignment::Center,
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
            frame.fill_text(Text {
                content: n.title.clone(),
                position: Point::new(c.x, c.y + (RING_R + 12.0) * cam.k),
                color: pal.ink,
                size: Pixels(11.0 * cam.k),
                align_x: iced::advanced::text::Alignment::Center,
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
        }
        // The zoom buttons sit in the top-right corner, as rounded controls on the panel.
        for (b, r) in buttons(bounds) {
            let shape = Path::rounded_rectangle(r.position(), r.size(), 7.0.into());
            frame.fill(&shape, pal.raised);
            frame.stroke(
                &shape,
                Stroke::default()
                    .with_color(pal.rule_strong)
                    .with_width(1.0),
            );
            frame.fill_text(Text {
                content: b.label().to_string(),
                position: Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0),
                color: pal.ink_2,
                size: Pixels(13.0),
                align_x: iced::advanced::text::Alignment::Center,
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
    fn mouse_interaction(
        &self,
        st: &View,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let on_button = cursor
            .position_in(bounds)
            .is_some_and(|p| button_at(bounds, p).is_some());
        if st.moved {
            mouse::Interaction::Grabbing
        } else if on_button || st.hover.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

/// The graph for the Memory tab, as the web's `.mg`: the column's full width at the web's 640 by 360 proportion, on a
/// sheet-2 panel. `query` is the find text (notes whose titles miss it are left out), `kb` the note the keyboard is on
/// and `reading` the note being read.
pub fn view<'a>(
    layout: &'a Layout,
    query: String,
    kb: Option<&'a str>,
    reading: Option<&'a str>,
) -> Element<'a, Msg> {
    let query = query.trim().to_lowercase();
    responsive(move |size: Size| {
        let graph = Canvas::new(GraphView {
            layout,
            query: query.clone(),
            kb,
            reading,
        })
        .width(Length::Fill)
        .height(Length::Fixed(size.width * 360.0 / 640.0));
        container(graph)
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().sheet_2.into()),
                border: Border::default().rounded(crate::theme::skin().radius),
                ..Default::default()
            })
            .into()
    })
    .into()
}
