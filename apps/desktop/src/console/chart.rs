//! Reusable canvas charts for the console: a stacked area over time with a hover crosshair and
//! tooltip, bars, and a sparkline. They take plain series and labels, so any screen can use them
//! (the Usage screen does; the Treasury can). The math that places them (stacking, round axis
//! ceilings, blending between two frames, the point under a cursor) is pure and tested alone.

use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, Path, Stroke};
use iced::{Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::theme::{MONO};

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
            Series { key: s.key.clone(), color: s.color, values }
        })
        .collect();
    for f in from.iter().filter(|f| !to.iter().any(|s| s.key == f.key)) {
        let values = f.values.iter().map(|a| a * (1.0 - t)).collect();
        out.push(Series { key: f.key.clone(), color: f.color, values });
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

fn label(frame: &mut canvas::Frame, content: String, at: Point, color: Color, size: f32, right: bool) {
    frame.fill_text(canvas::Text {
        content,
        position: at,
        color,
        size: iced::Pixels(size),
        font: MONO,
        align_x: if right { alignment::Horizontal::Right.into() } else { alignment::Horizontal::Left.into() },
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

impl<M> canvas::Program<M> for StackedArea {
    /// The hovered point.
    type State = Option<usize>;

    fn update(&self, hover: &mut Option<usize>, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<M>> {
        if !matches!(event, Event::Mouse(_)) {
            return None;
        }
        let (left, _, width, _) = plot(bounds, PAD_LEFT, PAD_BOTTOM);
        let next = cursor.position_in(bounds).and_then(|p| point_at(p.x, self.labels.len(), left, width));
        if next != *hover {
            *hover = next;
            return Some(canvas::Action::request_redraw());
        }
        None
    }

    fn draw(&self, hover: &Option<usize>, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (left, top, width, height) = plot(bounds, PAD_LEFT, PAD_BOTTOM);
        let tops = stacked_tops(&self.series);
        let n = tops.first().map_or(0, Vec::len);
        // A snapshot hovers the chart at its window point (`APZ_HOVER`), as a real cursor would.
        let hover = (*hover).or_else(|| snapshot_hover_index(bounds, n, left, width));
        let hover = &hover;
        let peak = tops.last().map_or(0.0, |t| t.iter().copied().fold(0.0, f64::max));
        let ceiling = nice_ceiling(peak);
        let y = |v: f64| top + height - (v / ceiling) as f32 * height;
        let x = |i: usize| point_x(i, n, left, width);
        let at = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);

        for k in 0..=4 {
            let v = ceiling * k as f64 / 4.0;
            let yy = y(v);
            let rule = if k == 0 { crate::theme::palette().rule } else { crate::theme::palette().rule_faint };
            frame.stroke(&Path::line(Point::new(left, yy), Point::new(left + width, yy)), Stroke::default().with_color(rule).with_width(1.0));
            label(&mut frame, (self.format)(v), Point::new(left - 8.0, yy - 6.0), crate::theme::palette().ink_3, 10.5, true);
        }
        if n == 0 || peak <= 0.0 {
            label(&mut frame, self.empty.to_string(), Point::new(left + width / 2.0, top + height / 2.0 - 7.0), crate::theme::palette().ink_3, 13.0, false);
            return vec![frame.into_geometry()];
        }

        for (s, series) in self.series.iter().enumerate() {
            let upper = &tops[s];
            let lower: Vec<f64> = if s == 0 { vec![0.0; n] } else { tops[s - 1].clone() };
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
            frame.fill(&area, Color { a: 0.55, ..series.color });
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
            frame.stroke(&edge, Stroke::default().with_color(series.color).with_width(1.5));
        }

        // The x-axis: up to six labels, evenly spaced over the points.
        let ticks = n.min(6);
        for k in 0..ticks {
            let i = if ticks <= 1 { 0 } else { k * (n - 1) / (ticks - 1) };
            let text = self.labels.get(i).cloned().unwrap_or_default();
            let anchor = if k == 0 {
                Point::new(x(i) + 2.0, top + height + 6.0)
            } else if k + 1 == ticks {
                Point::new(x(i) - 2.0, top + height + 6.0)
            } else {
                Point::new(x(i), top + height + 6.0)
            };
            label(&mut frame, text, anchor, crate::theme::palette().ink_3, 10.5, k + 1 == ticks && ticks > 1);
        }

        if let Some(i) = hover.filter(|i| *i < n) {
            let hx = x(i);
            frame.stroke(&Path::line(Point::new(hx, top), Point::new(hx, top + height)), Stroke::default().with_color(crate::theme::palette().ink_3).with_width(1.0));
            let rows: Vec<(Color, String)> = self
                .series
                .iter()
                .enumerate()
                .filter_map(|(s, series)| {
                    let v = at(&tops[s], i) - if s == 0 { 0.0 } else { at(&tops[s - 1], i) };
                    (v > 0.0).then(|| (series.color, format!("{}  {}", series.key, (self.format)(v))))
                })
                .collect();
            let heading = self.heads.as_ref().and_then(|h| h.get(i)).or_else(|| self.labels.get(i)).cloned().unwrap_or_default();
            let box_w = 200.0;
            let box_h = 14.0 + 16.0 * (rows.len() as f32 + 1.0) + 6.0;
            let bx = if hx > left + width / 2.0 { hx - 12.0 - box_w } else { hx + 12.0 };
            let by = top + 4.0;
            frame.fill_rectangle(Point::new(bx, by), Size::new(box_w, box_h), crate::theme::palette().sheet);
            frame.stroke(&Path::rectangle(Point::new(bx, by), Size::new(box_w, box_h)), Stroke::default().with_color(crate::theme::palette().rule).with_width(1.0));
            label(&mut frame, heading, Point::new(bx + 10.0, by + 8.0), crate::theme::palette().ink, 11.0, false);
            for (k, (color, text)) in rows.into_iter().enumerate() {
                let row_y = by + 8.0 + 16.0 * (k as f32 + 1.0);
                frame.fill(&Path::circle(Point::new(bx + 14.0, row_y + 6.0), 3.5), color);
                label(&mut frame, text, Point::new(bx + 24.0, row_y), crate::theme::palette().ink_2, 11.0, false);
            }
        }
        vec![frame.into_geometry()]
    }
}

const PAD_LEFT: f32 = 58.0;
const PAD_BOTTOM: f32 = 22.0;

/// Vertical bars, one per point, scaled to the tallest.
pub struct Bars {
    pub values: Vec<f64>,
    pub color: Color,
}

impl<M> canvas::Program<M> for Bars {
    type State = ();

    fn draw(&self, _state: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        frame.stroke(&Path::line(Point::new(0.0, h - 0.5), Point::new(w, h - 0.5)), Stroke::default().with_color(crate::theme::palette().rule).with_width(1.0));
        let n = self.values.len();
        let peak = self.values.iter().copied().fold(0.0, f64::max);
        if n > 0 && peak > 0.0 {
            let slot = w / n as f32;
            let bar = (slot * 0.7).max(1.0);
            for (i, v) in self.values.iter().enumerate() {
                let bh = (*v / peak) as f32 * (h - 2.0);
                if bh > 0.0 {
                    let bx = slot * i as f32 + (slot - bar) / 2.0;
                    frame.fill_rectangle(Point::new(bx, h - bh - 1.0), Size::new(bar, bh), self.color);
                }
            }
        }
        vec![frame.into_geometry()]
    }
}

/// A line of values with a faint fill beneath it, scaled to its own peak.
pub struct Sparkline {
    pub values: Vec<f64>,
    pub color: Color,
}

impl<M> canvas::Program<M> for Sparkline {
    type State = ();

    fn draw(&self, _state: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let n = self.values.len();
        let peak = self.values.iter().copied().fold(0.0, f64::max);
        if n == 0 {
            return vec![frame.into_geometry()];
        }
        let y = |v: f64| if peak > 0.0 { h - 2.0 - (v / peak) as f32 * (h - 4.0) } else { h - 2.0 };
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
        frame.fill(&area, Color { a: 0.18, ..self.color });
        frame.stroke(&line, Stroke::default().with_color(self.color).with_width(1.5));
        vec![frame.into_geometry()]
    }
}

/// A stacked area of `series` over `labels`, `height` pixels tall, filling the width.
pub fn stacked_area<'a, M: 'a>(series: Vec<Series>, labels: Vec<String>, format: fn(f64) -> String, empty: &'static str) -> Element<'a, M> {
    iced::widget::canvas(StackedArea { series, labels, format, empty, heads: None }).width(Length::Fill).height(Length::Fixed(260.0)).into()
}

/// A stacked area whose tooltip heads read `heads[i]` (the x-axis keeps `labels`).
pub fn stacked_area_with_heads<'a, M: 'a>(series: Vec<Series>, labels: Vec<String>, heads: Vec<String>, format: fn(f64) -> String, empty: &'static str) -> Element<'a, M> {
    iced::widget::canvas(StackedArea { series, labels, format, empty, heads: Some(heads) }).width(Length::Fill).height(Length::Fixed(260.0)).into()
}

/// Bars of `values` in `color`, 56 pixels tall, filling the width.
pub fn bars<'a, M: 'a>(values: Vec<f64>, color: Color) -> Element<'a, M> {
    iced::widget::canvas(Bars { values, color }).width(Length::Fill).height(Length::Fixed(56.0)).into()
}

/// A sparkline of `values` in `color`, 36 pixels tall, filling the width.
pub fn sparkline<'a, M: 'a>(values: Vec<f64>, color: Color) -> Element<'a, M> {
    iced::widget::canvas(Sparkline { values, color }).width(Length::Fill).height(Length::Fixed(36.0)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(key: &str, values: &[f64]) -> Series {
        Series { key: key.to_string(), color: crate::theme::palette().ink, values: values.to_vec() }
    }

    #[test]
    fn nice_ceiling_rounds_up_to_one_two_or_five() {
        assert_eq!(nice_ceiling(0.0), 1.0);
        assert_eq!(nice_ceiling(0.7), 1.0);
        assert_eq!(nice_ceiling(1.5), 2.0);
        assert_eq!(nice_ceiling(3.2), 5.0);
        assert_eq!(nice_ceiling(7.0), 10.0);
        assert_eq!(nice_ceiling(1200.0), 2000.0);
        assert_eq!(nice_ceiling(1000.0), 1000.0);
    }

    #[test]
    fn stacked_tops_accumulate_bottom_first_and_pad_short_series() {
        let s = [series("a", &[1.0, 2.0, 3.0]), series("b", &[10.0, 0.0])];
        let tops = stacked_tops(&s);
        assert_eq!(tops[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(tops[1], vec![11.0, 2.0, 3.0]);
    }

    #[test]
    fn blend_matches_keys_and_fades_the_missing_ones() {
        let from = [series("a", &[0.0, 4.0]), series("gone", &[8.0, 8.0])];
        let to = [series("a", &[10.0, 0.0]), series("new", &[2.0, 2.0])];
        let start = blend(&from, &to, 0.0);
        assert_eq!(start.iter().find(|s| s.key == "a").unwrap().values, vec![0.0, 4.0]);
        assert_eq!(start.iter().find(|s| s.key == "new").unwrap().values, vec![0.0, 0.0]);
        let end = blend(&from, &to, 1.0);
        assert_eq!(end.iter().find(|s| s.key == "a").unwrap().values, vec![10.0, 0.0]);
        assert_eq!(end.iter().find(|s| s.key == "new").unwrap().values, vec![2.0, 2.0]);
        assert_eq!(end.iter().find(|s| s.key == "gone").unwrap().values, vec![0.0, 0.0]);
        let mid = blend(&from, &to, 0.5);
        assert_eq!(mid.iter().find(|s| s.key == "a").unwrap().values, vec![5.0, 2.0]);
    }

    #[test]
    fn point_at_maps_the_plot_edges_and_rejects_outside() {
        assert_eq!(point_at(10.0, 5, 10.0, 100.0), Some(0));
        assert_eq!(point_at(110.0, 5, 10.0, 100.0), Some(4));
        assert_eq!(point_at(60.0, 5, 10.0, 100.0), Some(2));
        assert_eq!(point_at(9.0, 5, 10.0, 100.0), None);
        assert_eq!(point_at(111.0, 5, 10.0, 100.0), None);
        assert_eq!(point_at(50.0, 0, 10.0, 100.0), None);
        assert_eq!(point_at(50.0, 1, 10.0, 100.0), Some(0));
        assert_eq!(point_x(0, 5, 10.0, 100.0), 10.0);
        assert_eq!(point_x(4, 5, 10.0, 100.0), 110.0);
    }

    #[test]
    fn ease_is_clamped_monotone_and_hits_the_ends() {
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
        assert_eq!(ease(0.5), 0.5);
        let mut last = 0.0;
        for k in 0..=20 {
            let v = ease(k as f64 / 20.0);
            assert!(v >= last);
            last = v;
        }
    }
}
