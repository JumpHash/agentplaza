//! The graph artifact body: nodes and edges drawn on a canvas. A layered graph takes the task graph's layout
//! (`apz_graph`), so its cards and routed links read like the Work screen's; a force graph takes its positions from
//! `apz_graph::force` at the width the canvas is given. Past 1,000 nodes the first 1,000 are drawn and a note says so.

use crate::theme::{Palette, SANS, SANS_MEDIUM, TYPE_META, TYPE_UI, palette, skin};
use apz_graph::force::{DEFAULT_ITERATIONS, DEFAULT_NODE_SIZE, ForceOptions};
use apz_view::{GraphBody, GraphEdge, GraphLayout, GraphNode, Tone};
use iced::widget::canvas::{self, Canvas, Frame, Path, Stroke};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::Alignment;
use iced::widget::{column, scrollable, text};
use iced::{Color, Element, Font, Length, Pixels, Point, Rectangle, Renderer, Size, Theme, mouse};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// The most nodes a graph draws; the rest are cut and a note says how many.
const MAX_NODES: usize = 1_000;
/// The height of a force graph's canvas.
const FORCE_HEIGHT: f32 = 420.0;
/// The tallest a layered graph shows before it scrolls.
const LAYERED_MAX_HEIGHT: f32 = 640.0;
/// A force node's radius.
const NODE_R: f32 = 7.0;
/// A layered card's size and the gaps, as the Work screen's card but shorter (a title and a line).
const CARD: apz_graph::Options = apz_graph::Options {
    card_width: 220.0,
    card_height: 56.0,
    gap_x: 48.0,
    gap_y: 14.0,
    pad: 12.0,
};

/// One graph panel: a layered graph (cards and routed links) or a force graph (circles and straight links), drawn at
/// the width the panel gives. Nodes past the first 1,000 are not drawn.
pub fn graph<'a, M: 'a>(body: &'a GraphBody) -> Element<'a, M> {
    let p = *palette();
    let shown: Vec<&'a GraphNode> = body.nodes.iter().take(MAX_NODES).collect();
    if shown.is_empty() {
        return text("An empty graph").size(TYPE_META).color(p.ink_3).into();
    }
    let ids: HashSet<&str> = shown.iter().map(|&n| n.id.as_str()).collect();
    let edges: Vec<&'a GraphEdge> = body
        .edges
        .iter()
        .filter(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()))
        .collect();
    let mut groups: HashMap<&'a str, usize> = HashMap::new();
    for &n in &shown {
        if !n.group.is_empty() {
            let next = groups.len();
            groups.entry(n.group.as_str()).or_insert(next);
        }
    }
    let note = (body.nodes.len() > shown.len()).then(|| {
        format!(
            "Showing the first {} of {} nodes.",
            shown.len(),
            body.nodes.len()
        )
    });
    let view: Element<'a, M> = match body.layout {
        GraphLayout::Layered => layered(shown, edges, body.directed, groups),
        GraphLayout::Force => forced(shown, edges, body.directed, groups),
    };
    match note {
        Some(note) => column(vec![view, text(note).size(TYPE_META).color(p.ink_3).into()])
            .spacing(6)
            .into(),
        None => view,
    }
}

/// The layered picture: the task graph's layout, drawn in a scrollable that is at most [`LAYERED_MAX_HEIGHT`] tall.
fn layered<'a, M: 'a>(
    nodes: Vec<&'a GraphNode>,
    edges: Vec<&'a GraphEdge>,
    directed: bool,
    groups: HashMap<&'a str, usize>,
) -> Element<'a, M> {
    let input = apz_graph::Graph {
        tasks: nodes
            .iter()
            .map(|n| apz_graph::Task { id: n.id.clone() })
            .collect(),
        edges: edges
            .iter()
            .map(|e| [e.from.clone(), e.to.clone()])
            .collect(),
    };
    let laid = apz_graph::layout(&input, &CARD);
    let (w, h) = (laid.width as f32, laid.height as f32);
    let paint = Layered {
        nodes,
        edges,
        groups,
        directed,
        laid,
    };
    let surface = Canvas::new(paint)
        .width(Length::Fixed(w))
        .height(Length::Fixed(h));
    scrollable(surface)
        .direction(Direction::Both {
            vertical: Scrollbar::new(),
            horizontal: Scrollbar::new(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(h.min(LAYERED_MAX_HEIGHT)))
        .into()
}

/// The force picture: circles at the positions `apz_graph::force` gives for the canvas's width.
fn forced<'a, M: 'a>(
    nodes: Vec<&'a GraphNode>,
    edges: Vec<&'a GraphEdge>,
    directed: bool,
    groups: HashMap<&'a str, usize>,
) -> Element<'a, M> {
    let index: HashMap<&'a str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, &n)| (n.id.as_str(), i))
        .collect();
    let pairs: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|e| Some((*index.get(e.from.as_str())?, *index.get(e.to.as_str())?)))
        .collect();
    let paint = Forced {
        nodes,
        pairs,
        groups,
        directed,
    };
    Canvas::new(paint)
        .width(Length::Fill)
        .height(Length::Fixed(FORCE_HEIGHT))
        .into()
}

/// A layered graph's drawing: the routes, then the cards over their ends.
struct Layered<'a> {
    nodes: Vec<&'a GraphNode>,
    edges: Vec<&'a GraphEdge>,
    groups: HashMap<&'a str, usize>,
    directed: bool,
    laid: apz_graph::Layout,
}

impl<M> canvas::Program<M> for Layered<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let p = *palette();
        let mut frame = Frame::new(renderer, bounds.size());
        let labels: HashMap<(&str, &str), &str> = self
            .edges
            .iter()
            .map(|e| ((e.from.as_str(), e.to.as_str()), e.label.as_str()))
            .collect();
        for route in &self.laid.routes {
            let points: Vec<Point> = route
                .points
                .iter()
                .map(|&[x, y]| Point::new(x as f32, y as f32))
                .collect();
            polyline(&mut frame, &points, p.rule_strong, self.directed);
            if let Some(label) = labels
                .get(&(route.from.as_str(), route.to.as_str()))
                .filter(|l| !l.is_empty())
            {
                let at = midpoint(&points);
                write(
                    &mut frame,
                    *label,
                    Point::new(at.x, at.y - 16.0),
                    TYPE_META,
                    p.ink_3,
                    SANS,
                    Alignment::Center,
                );
            }
        }
        let by_id: HashMap<&str, &GraphNode> =
            self.nodes.iter().map(|&n| (n.id.as_str(), n)).collect();
        for card in &self.laid.cards {
            let Some(&node) = by_id.get(card.id.as_str()) else {
                continue;
            };
            let origin = Point::new(card.x as f32, card.y as f32);
            let size = Size::new(card.width as f32, card.height as f32);
            let shape = Path::rounded_rectangle(origin, size, skin().radius.into());
            frame.fill(&shape, p.sheet);
            frame.stroke(&shape, Stroke::default().with_color(p.rule).with_width(1.0));
            frame.fill_rectangle(
                origin,
                Size::new(3.0, size.height),
                fill_of(node, &self.groups, &p),
            );
            write(
                &mut frame,
                clip(&node.label, 26),
                Point::new(origin.x + 14.0, origin.y + 12.0),
                TYPE_UI,
                p.ink,
                SANS_MEDIUM,
                Alignment::Left,
            );
            if !node.sub.is_empty() {
                write(
                    &mut frame,
                    clip(&node.sub, 32),
                    Point::new(origin.x + 14.0, origin.y + 32.0),
                    TYPE_META,
                    p.ink_3,
                    SANS,
                    Alignment::Left,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

/// A force graph's state: the positions it settled at, and the canvas width they were settled for.
type Settled = RefCell<Option<(f32, Vec<Point>)>>;

/// A force graph's drawing: straight links, then a circle per node with its label beside it.
struct Forced<'a> {
    nodes: Vec<&'a GraphNode>,
    pairs: Vec<(usize, usize)>,
    groups: HashMap<&'a str, usize>,
    directed: bool,
}

impl Forced<'_> {
    /// The node centres the force layout gives in a box of `width` by the force height.
    fn settle(&self, width: f32) -> Vec<Point> {
        let opts = ForceOptions {
            width,
            height: FORCE_HEIGHT,
            iterations: DEFAULT_ITERATIONS,
            node_size: DEFAULT_NODE_SIZE,
        };
        apz_graph::force::layout(self.nodes.len(), &self.pairs, &opts)
            .into_iter()
            .map(|(x, y)| Point::new(x, y))
            .collect()
    }
}

impl<M> canvas::Program<M> for Forced<'_> {
    type State = Settled;

    fn draw(
        &self,
        state: &Settled,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let p = *palette();
        let mut frame = Frame::new(renderer, bounds.size());
        // The layout runs again only when the width changes; a cache-free draw would settle it on every frame.
        let mut cache = state.borrow_mut();
        if cache.as_ref().is_none_or(|(w, _)| *w != bounds.width) {
            *cache = Some((bounds.width, self.settle(bounds.width)));
        }
        let at: &[Point] = cache.as_ref().map_or(&[], |(_, at)| at.as_slice());
        for &(a, b) in &self.pairs {
            let (from, to) = (at[a], at[b]);
            frame.stroke(
                &Path::line(from, to),
                Stroke::default().with_color(p.rule_strong).with_width(1.0),
            );
            if self.directed {
                arrow(
                    &mut frame,
                    from,
                    stop_short(from, to, NODE_R + 1.0),
                    p.rule_strong,
                );
            }
        }
        for (node, &pos) in self.nodes.iter().zip(at) {
            let disc = Path::circle(pos, NODE_R);
            frame.fill(&disc, fill_of(node, &self.groups, &p));
            frame.stroke(&disc, Stroke::default().with_color(p.sheet).with_width(1.5));
            // A label goes left of a node on the right side of the panel, so it stays inside the panel.
            let right = pos.x <= bounds.width * 0.6;
            let x = if right {
                pos.x + NODE_R + 5.0
            } else {
                pos.x - NODE_R - 5.0
            };
            write(
                &mut frame,
                clip(&node.label, 28),
                Point::new(x, pos.y - 7.0),
                TYPE_META,
                p.ink,
                SANS,
                if right {
                    Alignment::Left
                } else {
                    Alignment::Right
                },
            );
        }
        vec![frame.into_geometry()]
    }
}

/// The colour of a node: its tone when it has one, else its group's hue, else the ink of a note.
fn fill_of(node: &GraphNode, groups: &HashMap<&str, usize>, p: &Palette) -> Color {
    match node.tone {
        Some(tone) => tone_color(tone, p),
        None => groups
            .get(node.group.as_str())
            .map_or(p.ink_3, |&i| hue(p, i)),
    }
}

/// The colour of a group by its order of first appearance, wrapping after six hues.
fn hue(p: &Palette, i: usize) -> Color {
    [p.hue_0, p.hue_1, p.hue_2, p.hue_3, p.hue_4, p.hue_5][i % 6]
}

/// The palette colour of a graph tone.
fn tone_color(tone: Tone, p: &Palette) -> Color {
    match tone {
        Tone::C1 => p.chart_1,
        Tone::C2 => p.chart_2,
        Tone::C3 => p.chart_3,
        Tone::C4 => p.chart_4,
        Tone::C5 => p.chart_5,
        Tone::C6 => p.chart_6,
        Tone::C7 => p.chart_7,
        Tone::C8 => p.chart_8,
        Tone::Other => p.chart_other,
        Tone::Ink3 => p.ink_3,
        Tone::Rule => p.rule,
        Tone::Ok => p.ok,
        Tone::Warn => p.warn,
        Tone::Crit => p.crit,
        Tone::Info => p.info,
        Tone::Signal => p.signal,
    }
}

/// A polyline through `points`, with an arrowhead at its end when `arrow_head` is set.
fn polyline(frame: &mut Frame, points: &[Point], color: Color, arrow_head: bool) {
    let Some((first, rest)) = points.split_first() else {
        return;
    };
    if rest.is_empty() {
        return;
    }
    let path = Path::new(|b| {
        b.move_to(*first);
        for point in rest {
            b.line_to(*point);
        }
    });
    frame.stroke(&path, Stroke::default().with_color(color).with_width(1.5));
    if arrow_head {
        let from = points[points.len() - 2];
        arrow(frame, from, points[points.len() - 1], color);
    }
}

/// An arrowhead at `tip`, pointing away from `from`.
fn arrow(frame: &mut Frame, from: Point, tip: Point, color: Color) {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let base = Point::new(tip.x - ux * 7.0, tip.y - uy * 7.0);
    let (px, py) = (-uy * 4.0, ux * 4.0);
    let head = Path::new(|b| {
        b.move_to(Point::new(base.x + px, base.y + py));
        b.line_to(tip);
        b.line_to(Point::new(base.x - px, base.y - py));
    });
    frame.stroke(&head, Stroke::default().with_color(color).with_width(1.5));
}

/// The point `by` short of `to` on the way from `from`, so a line stops at a node's edge.
fn stop_short(from: Point, to: Point, by: f32) -> Point {
    let d = from.distance(to);
    if d <= by {
        return to;
    }
    Point::new(
        to.x - (to.x - from.x) * by / d,
        to.y - (to.y - from.y) * by / d,
    )
}

/// The point halfway along a polyline's length.
fn midpoint(points: &[Point]) -> Point {
    let total: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut left = total / 2.0;
    for w in points.windows(2) {
        let d = w[0].distance(w[1]);
        if d > 0.0 && left <= d {
            let t = left / d;
            return Point::new(
                w[0].x + (w[1].x - w[0].x) * t,
                w[0].y + (w[1].y - w[0].y) * t,
            );
        }
        left -= d;
    }
    points.first().copied().unwrap_or(Point::ORIGIN)
}

/// Text on the canvas, at `at`, in `size` points.
fn write(
    frame: &mut Frame,
    content: impl Into<String>,
    at: Point,
    size: f32,
    color: Color,
    font: Font,
    align: Alignment,
) {
    frame.fill_text(canvas::Text {
        content: content.into(),
        position: at,
        color,
        size: Pixels(size),
        font,
        align_x: align,
        ..canvas::Text::default()
    });
}

/// A label cut to `max` characters, with an ellipsis when it was longer.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
