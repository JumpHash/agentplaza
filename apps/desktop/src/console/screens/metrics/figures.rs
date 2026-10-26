//! The other Metrics figures, as the web's figures.ts: the latency heatmap, treemaps, gauges, lifecycle swimlanes and
//! sparklines (canvas programs).

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::f32::consts::PI;
use std::hash::{Hash, Hasher};

use apz_metrics_view::{
    GaugeItem, Heatmap, Lane, LegendItem, Swimlanes, Tone, TreeItem, Unit, format,
};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::canvas::{self, Canvas, Path, Stroke};
use iced::widget::{Space, column, container, responsive, row, text};
use iced::{
    Alignment, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Renderer, Size,
    Theme, mouse,
};

use super::{Msg, axis_color, grid_color, status, tone};
use crate::console::ui;
use crate::theme::{self, SANS, SANS_SEMIBOLD, TYPE_META};

/// The heatmap's margin on the left (for the duration labels) and at the bottom (for the time labels), in px.
const HEAT_PAD_LEFT: f32 = 50.0;
const HEAT_PAD_BOTTOM: f32 = 18.0;
/// The narrowest a heatmap is drawn at, in px.
const HEAT_MIN_W: f32 = 240.0;
/// Axis label size, in px.
const AXIS_SIZE: f32 = 10.0;
/// Tooltip text size, in px.
const TIP_SIZE: f32 = 11.0;

/// A treemap is drawn this tall, in px.
const TREE_H: f32 = 210.0;
/// Cells smaller than this (in px²) keep no text.
const TREE_TEXT_MIN_AREA: f32 = 2000.0;

/// A gauge's box, arc centre, arc radius, arc stroke and the gap between gauges, in px.
const GAUGE_W: f32 = 128.0;
const GAUGE_H: f32 = 78.0;
const GAUGE_CX: f32 = 64.0;
const GAUGE_CY: f32 = 62.0;
const GAUGE_R: f32 = 52.0;
const GAUGE_STROKE: f32 = 10.0;
const GAUGE_GAP: f32 = 8.0;

/// A swimlane's name column, row height, axis height and the narrowest the lanes are drawn at, in px.
const LANE_W: f32 = 70.0;
const LANE_H: f32 = 22.0;
const AXIS_H: f32 = 18.0;
const SWIM_MIN_W: f32 = 560.0;

/// Canvas text: the content, where it sits on its anchor point, and how it is set.
struct Label {
    content: String,
    at: Point,
    size: f32,
    color: Color,
    font: Font,
    x: Horizontal,
    y: Vertical,
}

impl Label {
    /// Text in the console's sans, starting at the top left of `at`.
    fn new(content: impl Into<String>, at: Point, size: f32, color: Color) -> Self {
        Label {
            content: content.into(),
            at,
            size,
            color,
            font: SANS,
            x: Horizontal::Left,
            y: Vertical::Top,
        }
    }

    /// Where the text sits on its anchor point: its start, middle or end, and its top, middle or bottom.
    fn align(mut self, x: Horizontal, y: Vertical) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    fn semibold(mut self) -> Self {
        self.font = SANS_SEMIBOLD;
        self
    }

    fn draw(self, frame: &mut canvas::Frame) {
        frame.fill_text(canvas::Text {
            content: self.content,
            position: self.at,
            color: self.color,
            size: Pixels(self.size),
            font: self.font,
            align_x: self.x.into(),
            align_y: self.y,
            ..canvas::Text::default()
        });
    }
}

/// A tooltip box at `at`, moved left when it would run past `limit` (the canvas's width).
fn tip(frame: &mut canvas::Frame, at: Point, text: &str, limit: f32) {
    let p = theme::palette();
    let size = Size::new(text.chars().count() as f32 * 6.4 + 12.0, 20.0);
    let origin = Point::new(at.x.min(limit - size.width).max(0.0), at.y);
    frame.fill_rectangle(origin, size, p.sheet);
    frame.stroke(
        &Path::rectangle(origin, size),
        Stroke::default().with_color(p.rule).with_width(1.0),
    );
    Label::new(
        text,
        Point::new(origin.x + 6.0, origin.y + 10.0),
        TIP_SIZE,
        p.ink,
    )
    .align(Horizontal::Left, Vertical::Center)
    .draw(frame);
}

/// A canvas's drawing that does not depend on the pointer or the time: the canvas keeps its geometry until the hash
/// of the inputs it was drawn from (`key`) or its size changes, so a redraw for the pointer alone reuses it.
#[derive(Default)]
struct Cached {
    cache: canvas::Cache,
    /// The hash of the inputs the cached geometry was drawn from.
    key: Cell<u64>,
}

impl Cached {
    /// The geometry of `paint`, drawn again only when `key` differs from the last draw's or the size changed.
    fn draw(
        &self,
        renderer: &Renderer,
        size: Size,
        key: u64,
        paint: impl Fn(&mut canvas::Frame),
    ) -> canvas::Geometry {
        if self.key.get() != key {
            self.cache.clear();
            self.key.set(key);
        }
        self.cache.draw(renderer, size, paint)
    }
}

/// Hashes a colour's channels into a canvas key.
fn hash_color(hasher: &mut DefaultHasher, c: Color) {
    for channel in [c.r, c.g, c.b, c.a] {
        channel.to_bits().hash(hasher);
    }
}

/// The heatmap's canvas state: the hovered cell (column, row) and the pointer's place on the canvas, and the cached
/// grid and labels.
#[derive(Default)]
struct HeatState {
    hover: Option<(usize, usize, Point)>,
    cached: Cached,
}

/// The latency heatmap: one canvas of `h.counts[col][row]` cells, row 0 at the bottom, with the duration labels at the
/// lowest, middle and highest rows, the time labels under, and a tooltip under the pointer. A click on a cell
/// sends its column's time as a point.
pub fn heatmap<'a>(panel: &'a str, h: &'a Heatmap, compact: bool, tz: i32) -> Element<'a, Msg> {
    let scale = if compact { 0.8 } else { 1.0 };
    let height = (f32::from(h.height) * scale).round();
    Canvas::new(Heat { panel, h, tz })
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

struct Heat<'a> {
    panel: &'a str,
    h: &'a Heatmap,
    tz: i32,
}

impl Heat<'_> {
    fn cols(&self) -> usize {
        self.h.counts.len()
    }

    fn rows(&self) -> usize {
        self.h.counts.first().map_or(0, Vec::len)
    }

    fn count(&self, c: usize, r: usize) -> f64 {
        self.h
            .counts
            .get(c)
            .and_then(|column| column.get(r))
            .copied()
            .unwrap_or(0.0)
    }

    fn time_at(&self, c: usize) -> u64 {
        self.h.start_ms + c as u64 * self.h.step_ms
    }

    /// The duration at the edge of row `r` (fractional rows allowed): log-spaced from `lo` to `hi`, linear when that
    /// is not possible.
    fn edge(&self, r: f64) -> f64 {
        let rows = self.rows() as f64;
        let (lo, hi) = (self.h.lo, self.h.hi);
        if lo > 0.0 && hi > lo {
            lo * (hi / lo).powf(r / rows)
        } else {
            lo + (hi - lo) * r / rows
        }
    }

    fn bucket_range(&self, r: usize) -> String {
        let unit = self.h.unit;
        format!(
            "{}–{}",
            format::format(unit, self.edge(r as f64)),
            format::format(unit, self.edge(r as f64 + 1.0))
        )
    }

    /// The width and height of one cell, for a canvas of `bounds`.
    fn cell_size(&self, bounds: Rectangle) -> (f32, f32) {
        let width = bounds.width.max(HEAT_MIN_W);
        let cw = (width - HEAT_PAD_LEFT) / self.cols().max(1) as f32;
        let rh = (bounds.height - HEAT_PAD_BOTTOM) / self.rows().max(1) as f32;
        (cw, rh)
    }

    /// The cell under a point of the canvas, as (column, row); none outside the grid.
    fn cell_at(&self, bounds: Rectangle, p: Point) -> Option<(usize, usize)> {
        let (cw, rh) = self.cell_size(bounds);
        let x = p.x - HEAT_PAD_LEFT;
        if x < 0.0 || p.y < 0.0 || cw <= 0.0 || rh <= 0.0 {
            return None;
        }
        let c = (x / cw) as usize;
        let from_top = (p.y / rh) as usize;
        if c >= self.cols() || from_top >= self.rows() {
            return None;
        }
        Some((c, self.rows() - 1 - from_top))
    }
}

impl canvas::Program<Msg> for Heat<'_> {
    type State = HeatState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
        let at = cursor.position_in(bounds);
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            let (c, _) = self.cell_at(bounds, at?)?;
            return Some(canvas::Action::publish(Msg::Point(
                self.panel.to_owned(),
                c,
                Some(self.time_at(c)),
            )));
        }
        if !matches!(event, Event::Mouse(_)) {
            return None;
        }
        let next = at.and_then(|p| self.cell_at(bounds, p).map(|(c, r)| (c, r, p)));
        if next != state.hover {
            state.hover = next;
            return Some(canvas::Action::request_redraw());
        }
        None
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let (cols, rows) = (self.cols(), self.rows());
        let (cw, rh) = self.cell_size(bounds);
        let width = bounds.width.max(HEAT_MIN_W);
        let axis = axis_color();
        let cold = tone(Tone::C1);
        let hot = tone(Tone::C3);

        // The grid and the labels depend on the counts, the scales, the zone and the colours, not on the pointer.
        let mut hasher = DefaultHasher::new();
        self.h.counts.len().hash(&mut hasher);
        for col in &self.h.counts {
            col.len().hash(&mut hasher);
            for v in col {
                v.to_bits().hash(&mut hasher);
            }
        }
        self.h.start_ms.hash(&mut hasher);
        self.h.step_ms.hash(&mut hasher);
        self.h.lo.to_bits().hash(&mut hasher);
        self.h.hi.to_bits().hash(&mut hasher);
        std::mem::discriminant(&self.h.unit).hash(&mut hasher);
        self.tz.hash(&mut hasher);
        hash_color(&mut hasher, axis);
        hash_color(&mut hasher, cold);
        hash_color(&mut hasher, hot);
        let cached = state
            .cached
            .draw(renderer, bounds.size(), hasher.finish(), |frame| {
                if cols == 0 || rows == 0 {
                    return;
                }
                let max = self
                    .h
                    .counts
                    .iter()
                    .flatten()
                    .fold(0.0_f64, |m, v| m.max(*v));
                let top = if max > 0.0 { max } else { 1.0 };
                for c in 0..cols {
                    for r in 0..rows {
                        let v = self.count(c, r) / top;
                        if v < 0.02 {
                            continue;
                        }
                        let base = if v > 0.6 { hot } else { cold };
                        let fill = Color {
                            a: (0.12 + v * 0.95).min(1.0) as f32,
                            ..base
                        };
                        frame.fill_rectangle(
                            Point::new(HEAT_PAD_LEFT + c as f32 * cw, (rows - 1 - r) as f32 * rh),
                            Size::new(cw.ceil(), rh.ceil()),
                            fill,
                        );
                    }
                }

                for (k, v) in [self.h.lo, self.edge(rows as f64 / 2.0), self.h.hi]
                    .into_iter()
                    .enumerate()
                {
                    let r = (k as f64 / 2.0 * (rows as f64 - 1.0)).round() as usize;
                    let y = (rows - 1 - r) as f32 * rh + rh / 2.0;
                    Label::new(
                        format::format(self.h.unit, v),
                        Point::new(HEAT_PAD_LEFT - 6.0, y),
                        AXIS_SIZE,
                        axis,
                    )
                    .align(Horizontal::Right, Vertical::Center)
                    .draw(frame);
                }

                let nt = if width < 460.0 { 2 } else { 4 };
                for k in 0..=nt {
                    let c = (k as f64 / nt as f64 * (cols as f64 - 1.0)).round() as usize;
                    let (x, align) = match k {
                        0 => (HEAT_PAD_LEFT, Horizontal::Left),
                        k if k == nt => (HEAT_PAD_LEFT + c as f32 * cw + cw, Horizontal::Right),
                        _ => (HEAT_PAD_LEFT + c as f32 * cw, Horizontal::Center),
                    };
                    Label::new(
                        format::time_label(self.time_at(c), self.h.step_ms, self.tz),
                        Point::new(x, bounds.height - 4.0),
                        AXIS_SIZE,
                        axis,
                    )
                    .align(align, Vertical::Bottom)
                    .draw(frame);
                }
            });

        // Only the tooltip moves with the pointer, so it goes on a layer of its own over the cached geometry.
        let Some((c, r, at)) = state.hover.filter(|_| cols > 0 && rows > 0) else {
            return vec![cached];
        };
        let mut overlay = canvas::Frame::new(renderer, bounds.size());
        let text = format!(
            "{} · {}: {:.0} calls",
            format::time_label(self.time_at(c), self.h.step_ms, self.tz),
            self.bucket_range(r),
            self.count(c, r)
        );
        tip(
            &mut overlay,
            Point::new(at.x + 12.0, at.y + 10.0),
            &text,
            width,
        );
        vec![cached, overlay.into_geometry()]
    }
}

/// A treemap cell in percent of the box: its origin and size, 0 to 100 each.
struct TreeCell<'a> {
    item: &'a TreeItem,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// An item's weight: its value when it is a positive number, else none.
fn weight(it: &TreeItem) -> f64 {
    if it.value.is_finite() && it.value > 0.0 {
        it.value
    } else {
        0.0
    }
}

/// Splits the items into cells that tile a 100 by 100 box (percent). The items are sorted by value, largest first;
/// each step cuts the box in two halves by value, along its longer side. Every item keeps a cell, and a cell's area
/// is its share of the total.
fn split_treemap(items: &[TreeItem]) -> Vec<TreeCell<'_>> {
    let mut sorted: Vec<&TreeItem> = items.iter().collect();
    sorted.sort_by(|a, b| weight(b).total_cmp(&weight(a)));
    let mut cells = Vec::with_capacity(items.len());
    place(&sorted, 0.0, 0.0, 100.0, 100.0, &mut cells);
    cells
}

/// Places the sorted `list` in the box at (`x`, `y`) of size `w` by `h`, cutting it in two halves by weight.
fn place<'a>(list: &[&'a TreeItem], x: f64, y: f64, w: f64, h: f64, cells: &mut Vec<TreeCell<'a>>) {
    match list {
        [] => {}
        [only] => cells.push(TreeCell {
            item: only,
            x,
            y,
            w,
            h,
        }),
        _ => {
            let sum: f64 = list.iter().map(|it| weight(it)).sum();
            let (k, f) = if sum > 0.0 {
                let mut k = 0;
                let mut acc = 0.0;
                while k < list.len() - 1 && acc + weight(list[k]) <= sum / 2.0 {
                    acc += weight(list[k]);
                    k += 1;
                }
                if k == 0 {
                    acc = weight(list[0]);
                    k = 1;
                }
                (k, acc / sum)
            } else {
                // Nothing has weight: halve the cells by count so they still tile the box.
                let k = list.len().div_ceil(2);
                (k, k as f64 / list.len() as f64)
            };
            let (head, tail) = list.split_at(k);
            if w >= h {
                place(head, x, y, w * f, h, cells);
                place(tail, x + w * f, y, w * (1.0 - f), h, cells);
            } else {
                place(head, x, y, w, h * f, cells);
                place(tail, x, y + h * f, w, h * (1.0 - f), cells);
            }
        }
    }
}

/// The cells of a treemap, coloured by tone; the cells big enough for it carry their name and "value · share".
struct Tiles<'a> {
    unit: Unit,
    cells: Vec<TreeCell<'a>>,
    total: f64,
}

impl<M> canvas::Program<M> for Tiles<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut hasher = DefaultHasher::new();
        std::mem::discriminant(&self.unit).hash(&mut hasher);
        self.total.to_bits().hash(&mut hasher);
        hash_color(&mut hasher, theme::palette().on_hue);
        for cell in &self.cells {
            cell.x.to_bits().hash(&mut hasher);
            cell.y.to_bits().hash(&mut hasher);
            cell.w.to_bits().hash(&mut hasher);
            cell.h.to_bits().hash(&mut hasher);
            cell.item.name.as_str().hash(&mut hasher);
            cell.item.value.to_bits().hash(&mut hasher);
            hash_color(&mut hasher, tone(cell.item.tone));
        }
        let geometry = state.draw(renderer, bounds.size(), hasher.finish(), |frame| {
            for cell in &self.cells {
                let x = (cell.x / 100.0) as f32 * bounds.width;
                let y = (cell.y / 100.0) as f32 * bounds.height;
                let w = (cell.w / 100.0) as f32 * bounds.width;
                let h = (cell.h / 100.0) as f32 * bounds.height;
                frame.fill_rectangle(Point::new(x, y), Size::new(w, h), tone(cell.item.tone));
                if w * h <= TREE_TEXT_MIN_AREA {
                    continue;
                }
                let share = if self.total > 0.0 {
                    cell.item.value.max(0.0) / self.total * 100.0
                } else {
                    0.0
                };
                let name = ui::ellipsize(&cell.item.name, ((w - 12.0) / 6.5).max(0.0) as usize);
                Label::new(
                    name,
                    Point::new(x + 6.0, y + 6.0),
                    12.0,
                    crate::theme::palette().on_hue,
                )
                .semibold()
                .draw(frame);
                Label::new(
                    format!(
                        "{} · {:.0}%",
                        format::format(self.unit, cell.item.value),
                        share
                    ),
                    Point::new(x + 6.0, y + 22.0),
                    11.0,
                    crate::theme::palette().on_hue,
                )
                .draw(frame);
            }
        });
        vec![geometry]
    }
}

/// A squarified-by-halves treemap of the items, `TREE_H` tall, coloured by tone; a cell too small for text keeps none.
pub fn treemap<'a>(unit: Unit, items: &'a [TreeItem]) -> Element<'a, Msg> {
    let total = items.iter().map(|it| it.value.max(0.0)).sum();
    Canvas::new(Tiles {
        unit,
        cells: split_treemap(items),
        total,
    })
    .width(Length::Fill)
    .height(Length::Fixed(TREE_H))
    .into()
}

/// One half-circle gauge: the arc fills to `frac`, coloured by its status, with the label in the arc.
struct Gauge<'a> {
    g: &'a GaugeItem,
}

impl<M> canvas::Program<M> for Gauge<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &Renderer,
        _theme: &Theme,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut hasher = DefaultHasher::new();
        self.g.frac.to_bits().hash(&mut hasher);
        self.g.label.as_str().hash(&mut hasher);
        hash_color(&mut hasher, status(self.g.status));
        hash_color(&mut hasher, grid_color());
        hash_color(&mut hasher, theme::palette().ink);
        let size = Size::new(GAUGE_W, GAUGE_H);
        let geometry = state.draw(renderer, size, hasher.finish(), |frame| {
            let center = Point::new(GAUGE_CX, GAUGE_CY);
            let frac = if self.g.frac.is_finite() {
                self.g.frac.clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            // The arc runs clockwise from the left end (angle PI) over the top, to `end`.
            let arc = |end: f32| {
                Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center,
                        radius: GAUGE_R,
                        start_angle: PI.into(),
                        end_angle: end.into(),
                    });
                })
            };
            let stroke =
                |color: Color| Stroke::default().with_color(color).with_width(GAUGE_STROKE);
            frame.stroke(&arc(2.0 * PI), stroke(grid_color()));
            frame.stroke(&arc(PI + PI * frac), stroke(status(self.g.status)));
            Label::new(
                self.g.label.as_str(),
                Point::new(GAUGE_CX, GAUGE_CY - 6.0),
                17.0,
                theme::palette().ink,
            )
            .semibold()
            .align(Horizontal::Center, Vertical::Bottom)
            .draw(frame);
        });
        vec![geometry]
    }
}

/// One gauge with its sub text under it.
fn gauge_cell<'a>(g: &'a GaugeItem) -> Element<'a, Msg> {
    let arc: Element<'a, Msg> = Canvas::new(Gauge { g })
        .width(Length::Fixed(GAUGE_W))
        .height(Length::Fixed(GAUGE_H))
        .into();
    column![
        arc,
        text(g.sub.as_str())
            .size(11.5)
            .color(theme::palette().ink_3)
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// Half-circle gauges side by side, as many to a row as the width holds (the last row keeps the same column widths).
pub fn gauges<'a>(items: &'a [GaugeItem]) -> Element<'a, Msg> {
    responsive(move |size| {
        let per_row = ((size.width + GAUGE_GAP) / (GAUGE_W + GAUGE_GAP))
            .floor()
            .max(1.0) as usize;
        let rows: Vec<Element<'a, Msg>> = items
            .chunks(per_row)
            .map(|chunk| {
                let mut cells: Vec<Element<'a, Msg>> = chunk.iter().map(gauge_cell).collect();
                cells.resize_with(per_row, || Space::new().width(Length::Fill).into());
                row(cells).spacing(GAUGE_GAP).into()
            })
            .collect();
        column(rows).spacing(GAUGE_GAP).into()
    })
    // Shrink: a responsive fills its parent's height by default, which collapses the card inside the scroll.
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

/// One lane's segments across the span: a canvas as wide as its row's plot, four pixels short of the right edge.
struct Segments<'a> {
    lane: &'a Lane,
    start: u64,
    span: u64,
}

impl<M> canvas::Program<M> for Segments<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut hasher = DefaultHasher::new();
        self.start.hash(&mut hasher);
        self.span.hash(&mut hasher);
        for seg in &self.lane.segs {
            seg.start_ms.hash(&mut hasher);
            seg.end_ms.hash(&mut hasher);
            hash_color(&mut hasher, tone(seg.tone));
        }
        let geometry = state.draw(renderer, bounds.size(), hasher.finish(), |frame| {
            let plot = bounds.width - 4.0;
            for seg in &self.lane.segs {
                let x0 = fraction(seg.start_ms, self.start, self.span) * plot;
                let x1 = fraction(seg.end_ms, self.start, self.span) * plot;
                if x1 <= x0 {
                    continue;
                }
                frame.fill_rectangle(
                    Point::new(x0, 3.0),
                    Size::new(x1 - x0, 15.0),
                    tone(seg.tone),
                );
            }
        });
        vec![geometry]
    }
}

/// Where a time falls across a span from `start`, as a fraction clamped to 0..=1.
fn fraction(t: u64, start: u64, span: u64) -> f32 {
    ((t as f64 - start as f64) / span as f64).clamp(0.0, 1.0) as f32
}

/// The time axis under the lanes: five labels from the span's start to its end.
struct Axis {
    start: u64,
    span: u64,
    tz: i32,
}

impl<M> canvas::Program<M> for Axis {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let color = axis_color();
        let mut hasher = DefaultHasher::new();
        self.start.hash(&mut hasher);
        self.span.hash(&mut hasher);
        self.tz.hash(&mut hasher);
        hash_color(&mut hasher, color);
        let geometry = state.draw(renderer, bounds.size(), hasher.finish(), |frame| {
            let plot = bounds.width - 4.0;
            for k in 0..=4u64 {
                let x = plot * k as f32 / 4.0;
                let align = match k {
                    0 => Horizontal::Left,
                    4 => Horizontal::Right,
                    _ => Horizontal::Center,
                };
                let t = self.start + self.span * k / 4;
                Label::new(
                    format::time_label(t, self.span / 4, self.tz),
                    Point::new(x, 3.0),
                    AXIS_SIZE,
                    color,
                )
                .align(align, Vertical::Top)
                .draw(frame);
            }
        });
        vec![geometry]
    }
}

/// A lane's row: its name at left (a button that narrows to the lane's scope when it has one), its segments after.
fn lane_row<'a>(lane: &'a Lane, start: u64, span: u64) -> Element<'a, Msg> {
    let name: Element<'a, Msg> = match &lane.scope {
        Some(scope) => ui::link(
            &ui::ellipsize(&lane.name, 10),
            Msg::Scope(scope.clone()),
            true,
        ),
        None => ui::mono(
            ui::ellipsize(&lane.name, 10),
            TYPE_META,
            theme::palette().ink_2,
        )
        .into(),
    };
    let segments = Canvas::new(Segments { lane, start, span })
        .width(Length::Fill)
        .height(Length::Fixed(LANE_H));
    row![
        container(name)
            .width(Length::Fixed(LANE_W))
            .height(Length::Fixed(LANE_H))
            .align_y(Vertical::Center),
        segments
    ]
    .into()
}

/// The time axis's row: the name column's space, then the axis.
fn axis_row<'a>(start: u64, span: u64, tz: i32) -> Element<'a, Msg> {
    row![
        Space::new().width(Length::Fixed(LANE_W)),
        Canvas::new(Axis { start, span, tz })
            .width(Length::Fill)
            .height(Length::Fixed(AXIS_H))
    ]
    .into()
}

/// The legend: a tone swatch and a label per item.
fn legend<'a>(items: &'a [LegendItem]) -> Element<'a, Msg> {
    let entries: Vec<Element<'a, Msg>> = items
        .iter()
        .map(|item| {
            let color = tone(item.tone);
            row![
                container(Space::new())
                    .width(Length::Fixed(10.0))
                    .height(Length::Fixed(8.0))
                    .style(move |_: &Theme| container::Style {
                        background: Some(color.into()),
                        ..Default::default()
                    }),
                ui::sans(item.label.as_str(), TYPE_META, theme::palette().ink_2),
            ]
            .spacing(6.0)
            .align_y(Vertical::Center)
            .into()
        })
        .collect();
    row(entries).spacing(14.0).into()
}

/// One lane per VM, coloured segments by state across the span, the time axis under and a legend; a lane with a
/// scope narrows the view to it when its name is pressed.
pub fn swimlanes<'a>(s: &'a Swimlanes, tz: i32) -> Element<'a, Msg> {
    responsive(move |size| {
        let width = size.width.max(SWIM_MIN_W);
        let span = s.end_ms.saturating_sub(s.start_ms).max(1);
        let mut rows: Vec<Element<'a, Msg>> = s
            .lanes
            .iter()
            .map(|lane| lane_row(lane, s.start_ms, span))
            .collect();
        rows.push(axis_row(s.start_ms, span, tz));
        column![
            ui::scroll_x(column(rows).width(Length::Fixed(width))),
            legend(&s.legend)
        ]
        .spacing(8.0)
        .into()
    })
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

/// The sparkline's canvas: the line through the values, with the area under it at a fifth of the tone's strength.
struct Spark<'a> {
    values: &'a [f64],
    tone: Tone,
    width: f32,
    height: f32,
}

impl<M> canvas::Program<M> for Spark<'_> {
    type State = Cached;

    fn draw(
        &self,
        state: &Cached,
        renderer: &Renderer,
        _theme: &Theme,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let color = tone(self.tone);
        let mut hasher = DefaultHasher::new();
        self.values.len().hash(&mut hasher);
        for v in self.values {
            v.to_bits().hash(&mut hasher);
        }
        hash_color(&mut hasher, color);
        let size = Size::new(self.width, self.height);
        let geometry = state.draw(renderer, size, hasher.finish(), |frame| {
            let n = self.values.len();
            if n == 0 {
                return;
            }
            let lo = self.values.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = self
                .values
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let range = hi - lo;
            let pts: Vec<Point> = self
                .values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let x = if n > 1 {
                        i as f32 / (n - 1) as f32 * self.width
                    } else {
                        self.width / 2.0
                    };
                    // A flat series sits in the middle of the box.
                    let y = if range > 0.0 {
                        self.height - 2.0 - ((v - lo) / range) as f32 * (self.height - 4.0)
                    } else {
                        self.height / 2.0
                    };
                    Point::new(x, y)
                })
                .collect();
            let line = Path::new(|b| {
                for (i, p) in pts.iter().enumerate() {
                    if i == 0 {
                        b.move_to(*p);
                    } else {
                        b.line_to(*p);
                    }
                }
            });
            frame.stroke(&line, Stroke::default().with_color(color).with_width(1.3));
        });
        vec![geometry]
    }
}

/// A small inline sparkline, `width` by `height` in px, filled under the line.
pub fn spark<'a>(values: &'a [f64], tone: Tone, width: f32, height: f32) -> Element<'a, Msg> {
    Canvas::new(Spark {
        values,
        tone,
        width,
        height,
    })
    .width(Length::Fixed(width))
    .height(Length::Fixed(height))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_treemap_split_keeps_every_item_and_its_areas_sum_to_the_whole() {
        let item = |name: &str, value: f64| TreeItem {
            name: name.into(),
            value,
            tone: Tone::C1,
        };
        let items = vec![
            item("a", 50.0),
            item("b", 20.0),
            item("c", 0.0),
            item("d", 15.0),
            item("e", 9.0),
            item("f", 3.0),
            item("g", 3.0),
        ];
        let cells = split_treemap(&items);

        let mut names: Vec<&str> = cells.iter().map(|c| c.item.name.as_str()).collect();
        names.sort();
        let mut expected: Vec<&str> = items.iter().map(|it| it.name.as_str()).collect();
        expected.sort();
        assert_eq!(names, expected);

        let area: f64 = cells.iter().map(|c| c.w * c.h).sum();
        assert!(
            (area - 10_000.0).abs() < 1e-6,
            "the areas sum to {area}, not 10000"
        );
    }
}
