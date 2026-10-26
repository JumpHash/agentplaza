//! Reusable canvas charts for the console: a stacked area over time with a hover crosshair and
//! tooltip, bars, and a sparkline. They take plain series and labels, so any screen can use them
//! (the Lab's job detail does). The math that places them (stacking, round axis
//! ceilings, blending between two frames, the point under a cursor) is pure and tested alone.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, Path, Stroke};
use iced::{Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::theme::SANS;

/// One named series: `values[i]` is the value at bucket (or point) `i`.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub key: String,
    pub color: Color,
    pub values: Vec<f64>,
}

/// The cumulative tops of stacked series, bottom first: `tops[s][i]` is the sum of series `0..=s`
/// at point `i`. Missing values count as zero.
pub fn stacked_tops(series: &[Series]) -> Vec<Vec<f64>> {
    let n = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let mut acc = vec![0.0; n];
    series
        .iter()
        .map(|s| {
            for (i, a) in acc.iter_mut().enumerate() {
                *a += s.values.get(i).copied().unwrap_or(0.0);
            }
            acc.clone()
        })
        .collect()
}

/// The smallest round axis ceiling (1, 2 or 5 times a power of ten) at or above `max`; 1 when
/// there is no positive value to show.
pub fn nice_ceiling(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let exp = 10f64.powf(max.log10().floor());
    let f = max / exp;
    let step = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * exp
}

/// Smoothstep easing over `[0, 1]`.
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The series at progress `t` (0 is `from`, 1 is `to`), matched by key. A key only in `from`
/// fades to zero; a key only in `to` grows from zero; missing values count as zero.
pub fn blend(from: &[Series], to: &[Series], t: f64) -> Vec<Series> {
    let mut out: Vec<Series> = to
        .iter()
        .map(|s| {
            let before = from.iter().find(|f| f.key == s.key);
            let values = (0..s.values.len())
                .map(|i| {
                    let a = before.and_then(|f| f.values.get(i).copied()).unwrap_or(0.0);
                    let b = s.values.get(i).copied().unwrap_or(0.0);
                    a + (b - a) * t
                })
                .collect();
            Series {
                key: s.key.clone(),
                color: s.color,
                values,
            }
        })
        .collect();
    for f in from.iter().filter(|f| !to.iter().any(|s| s.key == f.key)) {
        let values = f.values.iter().map(|a| a * (1.0 - t)).collect();
        out.push(Series {
            key: f.key.clone(),
            color: f.color,
            values,
        });
    }
    out
}

/// The x of point `i` of `n`, spread edge to edge over a plot that starts at `left` and is `width` wide.
pub fn point_x(i: usize, n: usize, left: f32, width: f32) -> f32 {
    if n <= 1 {
        left + width / 2.0
    } else {
        left + width * i as f32 / (n - 1) as f32
    }
}

/// The point nearest to `x` (in the same frame as `left`), or None when `x` is off the plot or there are no points.
/// The window point a snapshot hovers the charts at (`APZ_HOVER`). Set once, before the first draw,
/// by the snapshot path only.
static SNAPSHOT_HOVER: std::sync::OnceLock<iced::Point> = std::sync::OnceLock::new();

/// Sets the window point the charts draw a hover at (the snapshot path's `APZ_HOVER`).
pub fn set_snapshot_hover(point: iced::Point) {
    let _ = SNAPSHOT_HOVER.set(point);
}

/// The hovered point of a chart for the snapshot's window point, when that point is over its plot.
fn snapshot_hover_index(bounds: Rectangle, n: usize, left: f32, width: f32) -> Option<usize> {
    let p = *SNAPSHOT_HOVER.get()?;
    if !bounds.contains(p) {
        return None;
    }
    point_at(p.x - bounds.x, n, left, width)
}

pub fn point_at(x: f32, n: usize, left: f32, width: f32) -> Option<usize> {
    if n == 0 || width <= 0.0 || x < left || x > left + width {
        return None;
    }
    if n == 1 {
        return Some(0);
    }
    let i = ((x - left) / width * (n - 1) as f32).round() as usize;
    Some(i.min(n - 1))
}

/// The plot area inside a chart's bounds: (left, top, width, height), leaving room for the axes.
fn plot(bounds: Rectangle, pad_left: f32, pad_bottom: f32) -> (f32, f32, f32, f32) {
    let (top, right) = (10.0, 10.0);
    let width = (bounds.width - pad_left - right).max(1.0);
    let height = (bounds.height - top - pad_bottom).max(1.0);
    (pad_left, top, width, height)
}

fn label(
    frame: &mut canvas::Frame,
    content: String,
    at: Point,
    color: Color,
    size: f32,
    right: bool,
) {
    frame.fill_text(canvas::Text {
        content,
        position: at,
        color,
        size: iced::Pixels(size),
        font: SANS,
        align_x: if right {
            alignment::Horizontal::Right.into()
        } else {
            alignment::Horizontal::Left.into()
        },
        ..canvas::Text::default()
    });
}

/// Stacked areas over points, each filled from the one below it, with four gridlines, a
/// crosshair at the hovered point and a tooltip listing every series there.
pub struct StackedArea {
    pub series: Vec<Series>,
    /// One label per point: the x-axis ticks and the tooltip's heading.
    pub labels: Vec<String>,
    /// How values read on the axis and in the tooltip.
    pub format: fn(f64) -> String,
    /// What the chart says when every value is zero.
    pub empty: &'static str,
    /// The tooltip's heading per point when it differs from the x label (the Lab's "step N · time"); None uses the label.
    pub heads: Option<Vec<String>>,
}

/// Hashes a colour bit for bit, so a change to any channel changes a chart's key.
fn hash_color(h: &mut DefaultHasher, c: Color) {
    [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(h);
}

/// Hashes a series of values bit for bit, after their count.
fn hash_values(h: &mut DefaultHasher, values: &[f64]) {
    values.len().hash(h);
    for v in values {
        v.to_bits().hash(h);
    }
}

/// What a stacked area keeps between frames: its static drawing, the key that drawing was made
/// for, and the hovered point.
#[derive(Default)]
pub struct StackedState {
    cache: canvas::Cache,
    key: Cell<u64>,
    hover: Option<usize>,
}

impl StackedArea {
    /// A hash of every input the static drawing reads: the values, colours, labels, axis format,
    /// empty message and the palette colours it uses. The keys, heads and hover are left out, as
    /// the tooltip is drawn fresh each time.
    fn static_key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.series.len().hash(&mut h);
        for s in &self.series {
            hash_color(&mut h, s.color);
            hash_values(&mut h, &s.values);
        }
        self.labels.hash(&mut h);
        (self.format as usize).hash(&mut h);
        self.empty.hash(&mut h);
        hash_color(&mut h, crate::theme::palette().rule);
        hash_color(&mut h, crate::theme::palette().rule_faint);
        hash_color(&mut h, crate::theme::palette().ink_3);
        h.finish()
    }
}

impl<M> canvas::Program<M> for StackedArea {
    type State = StackedState;

    fn update(
        &self,
        state: &mut StackedState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<M>> {
        if !matches!(event, Event::Mouse(_)) {
            return None;
        }
        let (left, _, width, _) = plot(bounds, PAD_LEFT, PAD_BOTTOM);
        let next = cursor
            .position_in(bounds)
            .and_then(|p| point_at(p.x, self.labels.len(), left, width));
        if next != state.hover {
            state.hover = next;
            return Some(canvas::Action::request_redraw());
        }
        None
    }

    fn draw(
        &self,
        state: &StackedState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let (left, top, width, height) = plot(bounds, PAD_LEFT, PAD_BOTTOM);
        // The point count and the peak total come straight from the series, so a redraw for the
        // cursor builds no stacked sums.
        let n = self
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0);
        let peak = (0..n).fold(0.0, |best, i| {
            let total = self
                .series
                .iter()
                .fold(0.0, |acc, s| acc + s.values.get(i).copied().unwrap_or(0.0));
            f64::max(best, total)
        });
        let empty = n == 0 || peak <= 0.0;
        let ceiling = nice_ceiling(peak);
        let y = |v: f64| top + height - (v / ceiling) as f32 * height;
        let x = |i: usize| point_x(i, n, left, width);
        let at = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);

        // The static part (gridlines, areas, x-axis) is drawn again only when its inputs change,
        // so a cursor move does not tessellate it.
        let key = self.static_key();
        if state.key.get() != key {
            state.cache.clear();
            state.key.set(key);
        }
        let cached = state.cache.draw(renderer, bounds.size(), |frame| {
            for k in 0..=4 {
                let v = ceiling * k as f64 / 4.0;
                let yy = y(v);
                let rule = if k == 0 {
                    crate::theme::palette().rule
                } else {
                    crate::theme::palette().rule_faint
                };
                frame.stroke(
                    &Path::line(Point::new(left, yy), Point::new(left + width, yy)),
                    Stroke::default().with_color(rule).with_width(1.0),
                );
                label(
                    frame,
                    (self.format)(v),
                    Point::new(left - 8.0, yy - 6.0),
                    crate::theme::palette().ink_3,
                    10.5,
                    true,
                );
            }
            if empty {
                label(
                    frame,
                    self.empty.to_string(),
                    Point::new(left + width / 2.0, top + height / 2.0 - 7.0),
                    crate::theme::palette().ink_3,
                    13.0,
                    false,
                );
                return;
            }

            let tops = stacked_tops(&self.series);
            for (s, series) in self.series.iter().enumerate() {
                let upper = &tops[s];
                let lower: Vec<f64> = if s == 0 {
                    vec![0.0; n]
                } else {
                    tops[s - 1].clone()
                };
                let area = Path::new(|b| {
                    for i in 0..n {
                        let p = Point::new(x(i), y(at(upper, i)));
                        if i == 0 {
                            b.move_to(p);
                        } else {
                            b.line_to(p);
                        }
                    }
                    for i in (0..n).rev() {
                        b.line_to(Point::new(x(i), y(at(&lower, i))));
                    }
                    b.close();
                });
                frame.fill(
                    &area,
                    Color {
                        a: 0.55,
                        ..series.color
                    },
                );
                let edge = Path::new(|b| {
                    for i in 0..n {
                        let p = Point::new(x(i), y(at(upper, i)));
                        if i == 0 {
                            b.move_to(p);
                        } else {
                            b.line_to(p);
                        }
                    }
                });
                frame.stroke(
                    &edge,
                    Stroke::default().with_color(series.color).with_width(1.5),
                );
            }

            // The x-axis: up to six labels, evenly spaced over the points.
            let ticks = n.min(6);
            for k in 0..ticks {
                let i = if ticks <= 1 {
                    0
                } else {
                    k * (n - 1) / (ticks - 1)
                };
                let text = self.labels.get(i).cloned().unwrap_or_default();
                let anchor = if k == 0 {
                    Point::new(x(i) + 2.0, top + height + 6.0)
                } else if k + 1 == ticks {
                    Point::new(x(i) - 2.0, top + height + 6.0)
                } else {
                    Point::new(x(i), top + height + 6.0)
                };
                label(
                    frame,
                    text,
                    anchor,
                    crate::theme::palette().ink_3,
                    10.5,
                    k + 1 == ticks && ticks > 1,
                );
            }
        });

        // The crosshair and tooltip follow the cursor, so they sit on their own layer over the
        // cached part. A snapshot hovers the chart at its window point (`APZ_HOVER`), as a real
        // cursor would.
        let hover = state
            .hover
            .or_else(|| snapshot_hover_index(bounds, n, left, width));
        let i = match hover {
            Some(i) if i < n && !empty => i,
            _ => return vec![cached],
        };
        let mut overlay = canvas::Frame::new(renderer, bounds.size());
        let hx = x(i);
        overlay.stroke(
            &Path::line(Point::new(hx, top), Point::new(hx, top + height)),
            Stroke::default()
                .with_color(crate::theme::palette().ink_3)
                .with_width(1.0),
        );
        // Each series' own value at `i` is its stacked top less the top below it, summed in the
        // same order as the stacked tops.
        let mut below = 0.0;
        let rows: Vec<(Color, String)> = self
            .series
            .iter()
            .filter_map(|series| {
                let stacked = below + at(&series.values, i);
                let v = stacked - below;
                below = stacked;
                (v > 0.0).then(|| {
                    (
                        series.color,
                        format!("{}  {}", series.key, (self.format)(v)),
                    )
                })
            })
            .collect();
        let heading = self
            .heads
            .as_ref()
            .and_then(|h| h.get(i))
            .or_else(|| self.labels.get(i))
            .cloned()
            .unwrap_or_default();
        let box_w = 200.0;
        let box_h = 14.0 + 16.0 * (rows.len() as f32 + 1.0) + 6.0;
        let bx = if hx > left + width / 2.0 {
            hx - 12.0 - box_w
        } else {
            hx + 12.0
        };
        let by = top + 4.0;
        overlay.fill_rectangle(
            Point::new(bx, by),
            Size::new(box_w, box_h),
            crate::theme::palette().sheet,
        );
        overlay.stroke(
            &Path::rectangle(Point::new(bx, by), Size::new(box_w, box_h)),
            Stroke::default()
                .with_color(crate::theme::palette().rule)
                .with_width(1.0),
        );
        label(
            &mut overlay,
            heading,
            Point::new(bx + 10.0, by + 8.0),
            crate::theme::palette().ink,
            11.0,
            false,
        );
        for (k, (color, text)) in rows.into_iter().enumerate() {
            let row_y = by + 8.0 + 16.0 * (k as f32 + 1.0);
            overlay.fill(
                &Path::circle(Point::new(bx + 14.0, row_y + 6.0), 3.5),
                color,
            );
            label(
                &mut overlay,
                text,
                Point::new(bx + 24.0, row_y),
                crate::theme::palette().ink_2,
                11.0,
                false,
            );
        }
        vec![cached, overlay.into_geometry()]
    }
}

const PAD_LEFT: f32 = 58.0;
const PAD_BOTTOM: f32 = 22.0;

/// Vertical bars, one per point, scaled to the tallest.
pub struct Bars {
    pub values: Vec<f64>,
    pub color: Color,
}

/// What the bars keep between frames: their drawing and the key that drawing was made for.
#[derive(Default)]
pub struct BarsState {
    cache: canvas::Cache,
    key: Cell<u64>,
}

impl Bars {
    /// A hash of the inputs the bars are drawn from: values, bar colour and palette rule.
    fn static_key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        hash_color(&mut h, self.color);
        hash_values(&mut h, &self.values);
        hash_color(&mut h, crate::theme::palette().rule);
        h.finish()
    }
}

impl<M> canvas::Program<M> for Bars {
    type State = BarsState;

    fn draw(
        &self,
        state: &BarsState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // Nothing here depends on the cursor, so the whole chart sits in the cache.
        let key = self.static_key();
        if state.key.get() != key {
            state.cache.clear();
            state.key.set(key);
        }
        let bars = state.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (bounds.width, bounds.height);
            frame.stroke(
                &Path::line(Point::new(0.0, h - 0.5), Point::new(w, h - 0.5)),
                Stroke::default()
                    .with_color(crate::theme::palette().rule)
                    .with_width(1.0),
            );
            let n = self.values.len();
            let peak = self.values.iter().copied().fold(0.0, f64::max);
            if n > 0 && peak > 0.0 {
                let slot = w / n as f32;
                let bar = (slot * 0.7).max(1.0);
                for (i, v) in self.values.iter().enumerate() {
                    let bh = (*v / peak) as f32 * (h - 2.0);
                    if bh > 0.0 {
                        let bx = slot * i as f32 + (slot - bar) / 2.0;
                        frame.fill_rectangle(
                            Point::new(bx, h - bh - 1.0),
                            Size::new(bar, bh),
                            self.color,
                        );
                    }
                }
            }
        });
        vec![bars]
    }
}

/// What a sparkline keeps between frames: its drawing and the key that drawing was made for.
#[derive(Default)]
pub struct SparklineState {
    cache: canvas::Cache,
    key: Cell<u64>,
}

/// A line of values with a faint fill beneath it, scaled to its own peak.
pub struct Sparkline {
    pub values: Vec<f64>,
    pub color: Color,
}

impl Sparkline {
    /// A hash of every input the line is drawn from: the values, the colour, the size and the
    /// theme generation, which changes with every palette change.
    fn static_key(&self, size: Size) -> u64 {
        let mut h = DefaultHasher::new();
        hash_values(&mut h, &self.values);
        hash_color(&mut h, self.color);
        size.width.to_bits().hash(&mut h);
        size.height.to_bits().hash(&mut h);
        crate::theme::generation().hash(&mut h);
        h.finish()
    }
}

impl<M> canvas::Program<M> for Sparkline {
    type State = SparklineState;

    fn draw(
        &self,
        state: &SparklineState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // Nothing here depends on the cursor, so the whole line sits in the cache.
        let key = self.static_key(bounds.size());
        if state.key.get() != key {
            state.cache.clear();
            state.key.set(key);
        }
        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            let (w, h) = (bounds.width, bounds.height);
            let n = self.values.len();
            let peak = self.values.iter().copied().fold(0.0, f64::max);
            if n == 0 {
                return;
            }
            let y = |v: f64| {
                if peak > 0.0 {
                    h - 2.0 - (v / peak) as f32 * (h - 4.0)
                } else {
                    h - 2.0
                }
            };
            let line = Path::new(|b| {
                for (i, v) in self.values.iter().enumerate() {
                    let p = Point::new(point_x(i, n, 0.0, w), y(*v));
                    if i == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
            });
            let area = Path::new(|b| {
                b.move_to(Point::new(point_x(0, n, 0.0, w), h));
                for (i, v) in self.values.iter().enumerate() {
                    b.line_to(Point::new(point_x(i, n, 0.0, w), y(*v)));
                }
                b.line_to(Point::new(point_x(n - 1, n, 0.0, w), h));
                b.close();
            });
            frame.fill(
                &area,
                Color {
                    a: 0.18,
                    ..self.color
                },
            );
            frame.stroke(
                &line,
                Stroke::default().with_color(self.color).with_width(1.5),
            );
        });
        vec![geometry]
    }
}

/// A stacked area of `series` over `labels`, `height` pixels tall, filling the width.
pub fn stacked_area<'a, M: 'a>(
    series: Vec<Series>,
    labels: Vec<String>,
    format: fn(f64) -> String,
    empty: &'static str,
) -> Element<'a, M> {
    iced::widget::canvas(StackedArea {
        series,
        labels,
        format,
        empty,
        heads: None,
    })
    .width(Length::Fill)
    .height(Length::Fixed(260.0))
    .into()
}

/// A stacked area whose tooltip heads read `heads[i]` (the x-axis keeps `labels`).
pub fn stacked_area_with_heads<'a, M: 'a>(
    series: Vec<Series>,
    labels: Vec<String>,
    heads: Vec<String>,
    format: fn(f64) -> String,
    empty: &'static str,
) -> Element<'a, M> {
    iced::widget::canvas(StackedArea {
        series,
        labels,
        format,
        empty,
        heads: Some(heads),
    })
    .width(Length::Fill)
    .height(Length::Fixed(260.0))
    .into()
}

/// Bars of `values` in `color`, 56 pixels tall, filling the width.
pub fn bars<'a, M: 'a>(values: Vec<f64>, color: Color) -> Element<'a, M> {
    iced::widget::canvas(Bars { values, color })
        .width(Length::Fill)
        .height(Length::Fixed(56.0))
        .into()
}

/// A sparkline of `values` in `color`, 36 pixels tall, filling the width.
pub fn sparkline<'a, M: 'a>(values: Vec<f64>, color: Color) -> Element<'a, M> {
    iced::widget::canvas(Sparkline { values, color })
        .width(Length::Fill)
        .height(Length::Fixed(36.0))
        .into()
}
