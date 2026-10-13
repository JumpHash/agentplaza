//! The memory graph (parity gap 12, the web's `memory-graph.ts`): an agent's notes as bubbles on a canvas, `[[links]]`
//! as ink curves and shared tags as faint straight lines. The layout is the web's force simulation (same constants,
//! same cooling: alpha from 1 down by 0.985 to 0.004, repulsion, name-tag collision, springs, a pull to the middle), run
//! once per note set with a seeded generator, so a given set of notes always lands in the same place. Hovering a note
//! lights it and its neighbours, a click selects it and opens it in the host's reader, dragging pans, dragging a note
//! moves it while it is held (the rest settle around it, and it floats back on release, as the web does), the wheel
//! zooms around the pointer, and a right click fits the view again. Tab and Shift+Tab walk the notes in a stable order
//! (by slug), the focused one is ringed, and Enter or Space opens it. The find text dims the notes whose titles miss it.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;

use iced::keyboard::{self, key::Named};
use iced::widget::canvas::{self, Canvas, Event, Frame, Path, Stroke, Text};
use iced::{mouse, Color, Element, Length, Pixels, Point, Rectangle, Renderer, Size, Theme};

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
    label: String,
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
    neighbours: Vec<Vec<usize>>,
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
        let key: Vec<(String, u64)> = notes.iter().map(|n| (n.slug.clone(), n.updated_ms)).collect();
        if key == self.key {
            return;
        }
        let prev: HashMap<String, (f32, f32)> = self.nodes.iter().map(|n| (n.slug.clone(), (n.x, n.y))).collect();
        *self = Layout::build(notes, &prev, key);
    }

    /// Holds a note at (x, y) while it is dragged; the rest of the graph settles around it.
    pub fn hold(&mut self, slug: &str, x: f32, y: f32) {
        let Some(i) = self.nodes.iter().position(|n| n.slug == slug) else { return };
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
                    label,
                    x,
                    y,
                    vx: 0.0,
                    vy: 0.0,
                }
            })
            .collect();

        // `[[links]]` (by slug, or by the web's normalised form), then a faint edge between neighbours sharing a tag.
        let index: HashMap<&str, usize> = notes.iter().enumerate().map(|(i, n)| (n.slug.as_str(), i)).collect();
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
        let mut neighbours = vec![Vec::new(); nodes.len()];
        for e in &edges {
            if !e.tag {
                neighbours[e.a].push(e.b);
                neighbours[e.b].push(e.a);
            }
        }

        // Cooling: a fresh layout starts hot, a refresh starts warm. Same decay and stop as the web.
        let mut alpha = if prev.is_empty() { 1.0 } else { 0.3 };
        nodes.shrink_to_fit();
        let mut lay = Layout { nodes, edges, neighbours, key, held: None };
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

fn push_edge(edges: &mut Vec<Edge>, seen: &mut HashSet<(usize, usize)>, a: usize, b: usize, tag: bool) {
    if a == b {
        return;
    }
    if seen.insert((a.min(b), a.max(b))) {
        edges.push(Edge { a, b, tag });
    }
}

fn resolve(index: &HashMap<&str, usize>, r: &str) -> Option<usize> {
    index.get(r).copied().or_else(|| index.get(norm(r).as_str()).copied())
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

fn kind_colour(kind: &str) -> Color {
    match kind {
        "person" => Color::from_rgb8(0xf5, 0xa3, 0xc7),
        "journal" => Color::from_rgb8(0xf7, 0xd1, 0x54),
        _ => Color::from_rgb8(0x8f, 0xb8, 0xff),
    }
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
    let k = ((w - 60.0) / (x1 - x0).max(1e-3)).min((h - 60.0) / (y1 - y0).max(1e-3)).clamp(0.25, 1.6);
    Cam { k, x: w / 2.0 - (x0 + x1) / 2.0 * k, y: h / 2.0 - (y0 + y1) / 2.0 * k }
}

/// The node under a point (canvas-relative), the nearest one when several overlap.
fn hit(lay: &Layout, cam: Cam, p: Point) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, n) in lay.nodes.iter().enumerate() {
        let d = (p.x - (cam.x + n.x * cam.k)).hypot(p.y - (cam.y + n.y * cam.k));
        let reach = (n.r * cam.k).max(6.0) + 3.0;
        if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// The view's own state: the camera (`None` until the user moves it, then the fit is kept), the pointer, the hover,
/// the selection, the note being dragged and the note the keyboard is on.
#[derive(Debug, Default)]
pub struct View {
    cam: Option<Cam>,
    press: Option<Point>,
    drag: Option<Point>,
    moved: bool,
    grab: Option<String>,
    hover: Option<usize>,
    selected: Option<usize>,
}

struct GraphView<'a> {
    layout: &'a Layout,
    /// The find text, trimmed and lower case: notes whose titles lack it are dimmed.
    query: String,
    /// The note the keyboard is on (ringed), from the host.
    kb: Option<&'a str>,
}

impl GraphView<'_> {
    fn cam(&self, st: &View, bounds: Rectangle) -> Cam {
        st.cam.unwrap_or_else(|| fit(self.layout, bounds))
    }
}

impl canvas::Program<Msg> for GraphView<'_> {
    type State = View;

    fn update(&self, st: &mut View, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Msg>> {
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
                    st.cam = Some(Cam { x: cam.x + dx, y: cam.y + dy, ..cam });
                    return Some(canvas::Action::request_redraw());
                }
                let hover = hit(self.layout, self.cam(st, bounds), p);
                if hover != st.hover {
                    st.hover = hover;
                    return Some(canvas::Action::request_redraw());
                }
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                st.press = Some(p);
                st.drag = Some(p);
                st.moved = false;
                let cam = self.cam(st, bounds);
                st.grab = hit(self.layout, cam, p).map(|i| self.layout.nodes[i].slug.clone());
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
                let i = hit(self.layout, self.cam(st, bounds), press)?;
                st.selected = Some(i);
                Some(canvas::Action::publish(Msg::Open(self.layout.nodes[i].slug.clone())))
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
                let cam = self.cam(st, bounds);
                // Zoom around the pointer, as the web does.
                let k = (cam.k * (-dy * 0.0015).exp()).clamp(0.2, 3.0);
                let s = k / cam.k;
                st.cam = Some(Cam { k, x: p.x - (p.x - cam.x) * s, y: p.y - (p.y - cam.y) * s });
                Some(canvas::Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(&self, st: &View, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry<Renderer>> {
        let pal = crate::theme::palette();
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), pal.paper);
        let lay = self.layout;
        if lay.nodes.is_empty() {
            return vec![frame.into_geometry()];
        }
        let cam = self.cam(st, bounds);
        let to_screen = |x: f32, y: f32| Point::new(cam.x + x * cam.k, cam.y + y * cam.k);
        // Hover wins over the selection; the focus and its neighbours stay bright, the rest fade.
        let focus = st.hover.or(st.selected);
        let lit = |i: usize| match focus {
            None => true,
            Some(f) => i == f || lay.neighbours[f].contains(&i),
        };
        // The find text dims the notes whose titles miss it (the web's `off` class, the strongest fade).
        let matches = |n: &Node| self.query.is_empty() || n.title.to_lowercase().contains(&self.query);
        let kb = self.kb.and_then(|s| lay.nodes.iter().position(|n| n.slug == s));

        for e in &lay.edges {
            let (na, nb) = (&lay.nodes[e.a], &lay.nodes[e.b]);
            let (a, b) = (to_screen(na.x, na.y), to_screen(nb.x, nb.y));
            let on = !e.tag && focus.is_some_and(|f| e.a == f || e.b == f);
            let fade_to = if focus.is_none() || on { 1.0 } else { 0.15 };
            let path = Path::new(|p| {
                p.move_to(a);
                if e.tag {
                    p.line_to(b);
                } else {
                    let (mx, my) = ((na.x + nb.x) / 2.0, (na.y + nb.y) / 2.0);
                    let (dx, dy) = (nb.x - na.x, nb.y - na.y);
                    p.quadratic_curve_to(to_screen(mx - dy * 0.12, my + dx * 0.12), b);
                }
            });
            let stroke = if e.tag {
                Stroke::default().with_color(fade(pal.ink_3, fade_to * 0.6)).with_width(1.0)
            } else {
                Stroke::default().with_color(fade(pal.ink, fade_to)).with_width(1.8)
            };
            frame.stroke(&path, stroke);
        }

        // Name tags counter-scale below zoom 1, as the web's do.
        let ls = (1.0 / cam.k).clamp(1.0, 1.8);
        for (i, n) in lay.nodes.iter().enumerate() {
            let c = to_screen(n.x, n.y);
            let r = (n.r * cam.k).max(4.0);
            let a = if !matches(n) {
                0.08
            } else if lit(i) {
                1.0
            } else {
                0.3
            };
            frame.fill(&Path::circle(c, r), fade(kind_colour(&n.kind), a));
            frame.fill(&Path::circle(Point::new(c.x - r * 0.35, c.y - r * 0.4), r * 0.28), fade(Color::WHITE, 0.55 * a));
            frame.stroke(&Path::circle(c, r), Stroke::default().with_color(fade(pal.ink, a)).with_width(1.6));
            if kb == Some(i) {
                frame.stroke(&Path::circle(c, r + 5.0), Stroke::default().with_color(pal.ink).with_width(2.5));
            }

            let tw = n.tag_w * ls * cam.k;
            let top = c.y + (n.r + 7.0 * ls) * cam.k;
            frame.fill(&Path::rectangle(Point::new(c.x - tw / 2.0, top), Size::new(tw, 20.0 * ls * cam.k)), fade(pal.sheet, 0.92 * a));
            frame.fill_text(Text {
                content: n.label.clone(),
                position: Point::new(c.x - tw / 2.0 + 9.0 * ls * cam.k, top + 3.0 * ls * cam.k),
                color: fade(pal.ink, a),
                size: Pixels(12.0 * ls * cam.k),
                ..Text::default()
            });
        }

        if let Some(s) = st.selected {
            frame.fill_text(Text {
                content: lay.nodes[s].title.clone(),
                position: Point::new(12.0, bounds.height - 24.0),
                color: pal.ink,
                size: Pixels(13.0),
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, st: &View, _bounds: Rectangle, _cursor: mouse::Cursor) -> mouse::Interaction {
        if st.moved {
            mouse::Interaction::Grabbing
        } else if st.hover.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

/// The graph for the Memory tab: fills the column, 480 px tall like the list it replaces. `query` is the find text:
/// notes whose titles miss it are dimmed.
pub fn view<'a>(layout: &'a Layout, query: String, kb: Option<&'a str>) -> Element<'a, Msg> {
    let query = query.trim().to_lowercase();
    Canvas::new(GraphView { layout, query, kb }).width(Length::Fill).height(Length::Fixed(480.0)).into()
}
