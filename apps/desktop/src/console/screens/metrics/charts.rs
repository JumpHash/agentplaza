//! The Metrics line chart, a canvas program drawing what the web's charts.ts draws: stacked areas, a percentile band,
//! dashed comparison overlays, markers, a limit, annotations, shaded gaps with their reason, the shared crosshair and
//! tooltip, drag-to-zoom and a click for what changed. Formatting is `apz_metrics_view::format`, the same as the
//! web's.

use apz_metrics_view::format::{format, ticks, time_label};
use apz_metrics_view::{Annotation, Chart, Series, Tone, XAxis};
use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, Path, Stroke};
use iced::widget::{Row, Space, button, column, container, text};
use iced::{Background, Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme};

use super::{Msg, Pointer, axis_color, grid_color, tone};
use crate::theme::{SANS, TYPE_META};

/// The space left of the plot for the value labels ("1.4 MiB/s" and "$1,264" fit at 11 px; the web uses the same
/// gutter), and right, above and below it.
const PAD_L: f32 = 64.0;
const PAD_R: f32 = 10.0;
const PAD_T: f32 = 8.0;
const PAD_B: f32 = 20.0;
/// A press that moves less than this many pixels is a click, not a drag.
const CLICK_PX: f32 = 6.0;
/// The tooltip's width and the height of each of its rows, in pixels.
const TIP_W: f32 = 200.0;
const TIP_ROW: f32 = 16.0;
/// The size of the axis, gap and label text, in pixels.
const LABEL_PX: f32 = 11.0;
/// The dash patterns of the lines, the comparison overlays, the markers, the limit and the annotations.
const DASH_SERIES: &[f32] = &[4.0, 3.0];
const DASH_GHOST: &[f32] = &[5.0, 4.0];
const DASH_MARKER: &[f32] = &[2.0, 3.0];
const DASH_LIMIT: &[f32] = &[5.0, 3.0];
const DASH_ANNOTATION: &[f32] = &[1.0, 3.0];

/// A chart panel's chart. `panel` names the panel in the messages it sends (Hover, DragStart/To/End, Point).
pub fn line<'a>(
    panel: &'a str,
    c: &'a Chart,
    pointer: &'a Pointer,
    annotations: &'a [Annotation],
    show_annotations: bool,
    compact: bool,
    tz: i32,
) -> Element<'a, Msg> {
    let scale = if compact { 0.8 } else { 1.0 };
    let height = f32::from(c.height) * scale;
    let plot: Element<'a, Msg> = iced::widget::canvas(LineChart {
        panel,
        c,
        pointer,
        annotations,
        show_annotations,
        tz,
    })
    .width(Length::Fill)
    .height(Length::Fixed(height))
    .into();
    column![legend(c), plot].spacing(6).into()
}

/// The canvas program. It holds what the chart shows; the press and drag it tracks live in [`Gesture`].
struct LineChart<'a> {
    panel: &'a str,
    c: &'a Chart,
    pointer: &'a Pointer,
    annotations: &'a [Annotation],
    show_annotations: bool,
    tz: i32,
}

/// The left button's press across a draw of the chart: where it went down, whether it has become a drag, and the
/// last fraction the pointer reached (where a drag ends when the pointer leaves the chart).
#[derive(Debug, Default)]
pub struct Gesture {
    press: Option<f32>,
    dragging: bool,
    last: f32,
}

/// The plot area inside a canvas of `size`: where the lines, areas and gridlines go.
#[derive(Clone, Copy)]
struct Plot {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

fn plot_area(size: Size) -> Plot {
    Plot {
        left: PAD_L,
        top: PAD_T,
        width: (size.width - PAD_L - PAD_R).max(1.0),
        height: (size.height - PAD_T - PAD_B).max(1.0),
    }
}

/// The scales of one draw: the plot, the number of points and the top of the value axis.
#[derive(Clone, Copy)]
struct Scale {
    plot: Plot,
    n: usize,
    ym: f64,
}

impl Scale {
    /// The x of point `i` (fractional when it is between two points), spread edge to edge across the plot.
    fn x(&self, i: f32) -> f32 {
        if self.n <= 1 {
            self.plot.left
        } else {
            self.plot.left + i / (self.n - 1) as f32 * self.plot.width
        }
    }

    /// The y of value `v` on the value axis, zero at the bottom of the plot.
    fn y(&self, v: f64) -> f32 {
        self.plot.top + self.plot.height - (v / self.ym) as f32 * self.plot.height
    }
}

impl LineChart<'_> {
    fn is_time(&self) -> bool {
        matches!(self.c.x, XAxis::Time { .. })
    }

    /// Whether a hover at `frac` differs from the one the pointer state already holds for this panel.
    fn hover_differs(&self, frac: f32) -> bool {
        self.pointer.hover != Some(frac) || self.pointer.hover_panel.as_deref() != Some(self.panel)
    }

    /// The rows of the tooltip at point `i`: the time, each series, each overlay and the band's range.
    fn tip_rows(&self, i: usize) -> Vec<TipRow> {
        let c = self.c;
        let val = |v: Option<f64>| v.map_or_else(|| "—".to_string(), |v| format(c.unit, v));
        let mut rows = vec![TipRow {
            swatch: None,
            left: x_text(&c.x, i, self.tz),
            right: String::new(),
            muted: false,
        }];
        for s in &c.series {
            rows.push(TipRow {
                swatch: Some(tone(s.tone)),
                left: s.name.clone(),
                right: val(s.values.get(i).copied().flatten()),
                muted: false,
            });
        }
        for g in &c.ghosts {
            rows.push(TipRow {
                swatch: None,
                left: g.name.clone(),
                right: val(g.values.get(i).copied().flatten()),
                muted: true,
            });
        }
        if let Some(b) = &c.band {
            rows.push(TipRow {
                swatch: None,
                left: b.name.clone(),
                right: format!(
                    "{} – {}",
                    val(b.lo.get(i).copied()),
                    val(b.hi.get(i).copied())
                ),
                muted: true,
            });
        }
        rows
    }

    /// Draws the whole chart: the axes, the gaps, the band, the series, the overlays, the markers, the limit, the
    /// annotations, the crosshair and the drag brush, then the tooltip where the pointer is over this panel.
    fn paint(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let c = self.c;
        let palette = crate::theme::palette();
        let plot = plot_area(bounds.size());
        let bottom = plot.top + plot.height;
        let n = point_count(c);

        // Long series are thinned to one point per pixel bucket; every drawn list shares these starts.
        let starts = bucket_starts(n, plot.width as usize);
        let series: Vec<(&Series, Vec<Option<f64>>)> = c
            .series
            .iter()
            .map(|s| (s, thin(&s.values, &starts, false)))
            .collect();
        let ghosts: Vec<(&Series, Vec<Option<f64>>)> = c
            .ghosts
            .iter()
            .map(|g| (g, thin(&g.values, &starts, false)))
            .collect();
        let band = c.band.as_ref().map(|b| {
            (
                b,
                thin(&some(&b.lo), &starts, true),
                thin(&some(&b.hi), &starts, false),
            )
        });

        // The value axis reaches the highest stacked total or series, the overlays, the band, the maximum and the limit.
        let mut max = if c.stacked {
            stacked_totals(&series, starts.len())
                .into_iter()
                .fold(0.0, f64::max)
        } else {
            series.iter().map(|(_, v)| peak(v)).fold(0.0, f64::max)
        };
        for (_, v) in &ghosts {
            max = max.max(peak(v));
        }
        if let Some((_, _, hi)) = &band {
            max = max.max(peak(hi));
        }
        if let Some(y) = c.y_max {
            max = max.max(y);
        }
        if let Some(limit) = &c.limit {
            max = max.max(limit.value);
        }
        let tk = ticks(max);
        let scale = Scale {
            plot,
            n,
            ym: if tk[4] > 0.0 { tk[4] } else { 1.0 },
        };
        let xs: Vec<f32> = starts.iter().map(|&i| scale.x(i as f32)).collect();

        // Gridlines and the axis labels.
        let (grid, axis) = (grid_color(), axis_color());
        for &v in &tk {
            let y = scale.y(v);
            frame.stroke(
                &Path::line(
                    Point::new(plot.left, y),
                    Point::new(plot.left + plot.width, y),
                ),
                stroke(grid, 1.0, None),
            );
            text_at(
                frame,
                format(c.unit, v),
                Point::new(PAD_L - 6.0, y),
                axis,
                alignment::Horizontal::Right,
                alignment::Vertical::Center,
            );
        }
        for i in x_label_indexes(n, bounds.width) {
            let align = match i {
                0 => alignment::Horizontal::Left,
                i if i + 1 == n => alignment::Horizontal::Right,
                _ => alignment::Horizontal::Center,
            };
            text_at(
                frame,
                x_text(&c.x, i, self.tz),
                Point::new(scale.x(i as f32), bounds.height - 4.0),
                axis,
                align,
                alignment::Vertical::Bottom,
            );
        }

        // Gaps: a run of nulls in a line is shaded and named with its reason.
        if !c.stacked {
            for (s, vals) in &series {
                for (from, to) in null_runs(vals) {
                    let x0 = xs[from.saturating_sub(1)];
                    let x1 = xs[to.min(xs.len() - 1)];
                    let width = (x1 - x0).max(2.0);
                    frame.fill_rectangle(
                        Point::new(x0, plot.top),
                        Size::new(width, plot.height),
                        palette.rule_faint.scale_alpha(0.7),
                    );
                    let reason = s.gap_reason.as_deref().unwrap_or("no data");
                    text_at(
                        frame,
                        reason,
                        Point::new(x0 + width / 2.0, plot.top + 12.0),
                        palette.ink_3,
                        alignment::Horizontal::Center,
                        alignment::Vertical::Bottom,
                    );
                }
            }
        }

        // The percentile band: the high edge forward, the low edge back.
        if let Some((b, lo, hi)) = &band {
            let mut edge: Vec<Point> = hi
                .iter()
                .zip(&xs)
                .map(|(v, x)| Point::new(*x, scale.y(v.unwrap_or(0.0))))
                .collect();
            edge.extend(
                lo.iter()
                    .zip(&xs)
                    .rev()
                    .map(|(v, x)| Point::new(*x, scale.y(v.unwrap_or(0.0)))),
            );
            if !edge.is_empty() {
                frame.fill(&polygon(&edge), tone(b.tone).scale_alpha(0.18));
            }
        }

        // The series: stacked areas, each filled down to the one below; otherwise lines, with an area under each when set.
        if c.stacked {
            let mut base = vec![0.0; starts.len()];
            for (s, vals) in &series {
                let top: Vec<f64> = vals
                    .iter()
                    .zip(&base)
                    .map(|(v, b)| b + v.unwrap_or(0.0))
                    .collect();
                let mut edge: Vec<Point> = top
                    .iter()
                    .zip(&xs)
                    .map(|(v, x)| Point::new(*x, scale.y(*v)))
                    .collect();
                edge.extend(
                    base.iter()
                        .zip(&xs)
                        .rev()
                        .map(|(v, x)| Point::new(*x, scale.y(*v))),
                );
                frame.fill(&polygon(&edge), tone(s.tone).scale_alpha(0.78));
                base = top;
            }
        } else {
            for (s, vals) in &series {
                let color = tone(s.tone);
                let points = trace(vals, &xs, &scale);
                if c.area && !vals.is_empty() && vals.iter().all(Option::is_some) {
                    let mut edge: Vec<Point> = points.iter().flatten().copied().collect();
                    let floor = scale.y(0.0);
                    edge.push(Point::new(xs[xs.len() - 1], floor));
                    edge.push(Point::new(xs[0], floor));
                    frame.fill(&polygon(&edge), color.scale_alpha(0.12));
                }
                let (width, dash) = if s.dash {
                    (1.2, Some(DASH_SERIES))
                } else {
                    (1.6, None)
                };
                frame.stroke(&polyline(&points), stroke(color, width, dash));
            }
        }

        // The comparison overlays, dashed at three quarters strength.
        for (g, vals) in &ghosts {
            frame.stroke(
                &polyline(&trace(vals, &xs, &scale)),
                stroke(tone(g.tone).scale_alpha(0.75), 1.2, Some(DASH_GHOST)),
            );
        }

        // Markers: a dashed signal line with its label at the top.
        for mk in &c.markers {
            if mk.index >= n {
                continue;
            }
            let x = scale.x(mk.index as f32);
            let signal = tone(Tone::Signal);
            frame.stroke(
                &Path::line(Point::new(x, plot.top), Point::new(x, bottom)),
                stroke(signal, 1.0, Some(DASH_MARKER)),
            );
            text_at(
                frame,
                mk.label.as_str(),
                Point::new(x + 3.0, plot.top + 9.0),
                signal,
                alignment::Horizontal::Left,
                alignment::Vertical::Bottom,
            );
        }

        // The limit: a dashed crit line across the plot, labelled at the right.
        if let Some(limit) = &c.limit {
            let y = scale.y(limit.value);
            let crit = tone(Tone::Crit);
            frame.stroke(
                &Path::line(
                    Point::new(plot.left, y),
                    Point::new(plot.left + plot.width, y),
                ),
                stroke(crit, 1.0, Some(DASH_LIMIT)),
            );
            text_at(
                frame,
                limit.label.as_str(),
                Point::new(bounds.width - PAD_R, y - 4.0),
                crit,
                alignment::Horizontal::Right,
                alignment::Vertical::Bottom,
            );
        }

        // Annotations: a dotted line and a small triangle on the axis, on a time axis only.
        if let XAxis::Time { start_ms, step_ms } = &c.x {
            if *step_ms > 0 && c.annotate && self.show_annotations {
                for a in self.annotations {
                    let i = (a.t_ms as f64 - *start_ms as f64) / *step_ms as f64;
                    if !(i >= 0.0 && i <= n as f64 - 1.0) {
                        continue;
                    }
                    let x = scale.x(i as f32);
                    frame.stroke(
                        &Path::line(Point::new(x, plot.top), Point::new(x, bottom)),
                        stroke(palette.ink_3, 1.0, Some(DASH_ANNOTATION)),
                    );
                    frame.fill(
                        &polygon(&[
                            Point::new(x - 5.0, bottom),
                            Point::new(x + 5.0, bottom),
                            Point::new(x, bottom - 7.0),
                        ]),
                        palette.signal,
                    );
                }
            }
        }

        // The crosshair at the hovered fraction (shared by every chart), with a dot on each series there.
        if let (Some(frac), true) = (self.pointer.hover, n > 0) {
            if let Some(i) = index_at(frac, n) {
                let x = scale.x(i as f32);
                frame.stroke(
                    &Path::line(Point::new(x, plot.top), Point::new(x, bottom)),
                    stroke(palette.ink_3, 1.0, None),
                );
                let mut acc = 0.0;
                for (s, _) in &series {
                    let v = s.values.get(i).copied().flatten();
                    acc += v.unwrap_or(0.0);
                    if let Some(v) = v {
                        let cy = scale.y(if c.stacked { acc } else { v });
                        frame.fill(&Path::circle(Point::new(x, cy), 2.8), tone(s.tone));
                    }
                }
            }
        }

        // The drag brush: the span between the start and the current fraction, while this panel is being dragged.
        if let Some((panel, from, to)) = &self.pointer.drag {
            if panel.as_str() == self.panel && self.is_time() {
                let (lo, hi) = (from.min(*to).clamp(0.0, 1.0), from.max(*to).clamp(0.0, 1.0));
                frame.fill_rectangle(
                    Point::new(plot.left + lo * plot.width, plot.top),
                    Size::new((hi - lo) * plot.width, plot.height),
                    palette.signal.scale_alpha(0.12),
                );
            }
        }

        // The tooltip, on the chart the pointer is over: a sheet box with an ink-3 border and a 2 px ink shadow.
        if self.pointer.hover_panel.as_deref() == Some(self.panel) {
            if let Some(i) = self.pointer.hover.and_then(|frac| index_at(frac, n)) {
                let rows = self.tip_rows(i);
                let x = scale.x(i as f32);
                let height = 8.0 + rows.len() as f32 * TIP_ROW + 8.0;
                let left = (if x + 14.0 + TIP_W > bounds.width {
                    x - 210.0
                } else {
                    x + 14.0
                })
                .max(0.0);
                let top = plot.top + 2.0;
                frame.fill_rectangle(
                    Point::new(left + 2.0, top + 2.0),
                    Size::new(TIP_W, height),
                    palette.ink,
                );
                frame.fill_rectangle(
                    Point::new(left, top),
                    Size::new(TIP_W, height),
                    palette.sheet,
                );
                frame.stroke(
                    &Path::rectangle(Point::new(left, top), Size::new(TIP_W, height)),
                    stroke(palette.ink_3, 1.0, None),
                );
                for (k, row) in rows.iter().enumerate() {
                    let y = top + 8.0 + TIP_ROW / 2.0 + k as f32 * TIP_ROW;
                    let mut x0 = left + 8.0;
                    if let Some(color) = row.swatch {
                        frame.fill_rectangle(Point::new(x0, y - 4.0), Size::new(8.0, 8.0), color);
                        x0 += 14.0;
                    }
                    let ink = if row.muted {
                        palette.ink_3
                    } else {
                        palette.ink
                    };
                    text_at(
                        frame,
                        row.left.as_str(),
                        Point::new(x0, y),
                        ink,
                        alignment::Horizontal::Left,
                        alignment::Vertical::Center,
                    );
                    text_at(
                        frame,
                        row.right.as_str(),
                        Point::new(left + TIP_W - 8.0, y),
                        ink,
                        alignment::Horizontal::Right,
                        alignment::Vertical::Center,
                    );
                }
            }
        }
    }
}

/// One row of the tooltip: an optional swatch, the left text (a name or the time), the right text (a value).
struct TipRow {
    swatch: Option<Color>,
    left: String,
    right: String,
    muted: bool,
}

impl<'a> canvas::Program<Msg> for LineChart<'a> {
    type State = Gesture;

    fn update(
        &self,
        gesture: &mut Gesture,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
        let Event::Mouse(event) = event else {
            return None;
        };
        let plot = plot_area(bounds.size());
        let frac_of = |x: f32| (x - plot.left) / plot.width;
        let over = cursor.position_in(bounds).is_some();
        // The pointer's x inside the canvas, also while it is outside it during a drag.
        let x_now = cursor.position().map(|p| p.x - bounds.x);
        let panel = self.panel;

        match event {
            mouse::Event::CursorMoved { .. } => {
                let x = x_now?;
                let frac = frac_of(x);
                if let Some(press) = gesture.press {
                    gesture.last = frac.clamp(0.0, 1.0);
                    if gesture.dragging {
                        return Some(canvas::Action::publish(Msg::DragTo(gesture.last)));
                    }
                    if self.is_time() && (x - press).abs() >= CLICK_PX {
                        gesture.dragging = true;
                        return Some(canvas::Action::publish(Msg::DragStart(
                            panel.to_string(),
                            frac_of(press).clamp(0.0, 1.0),
                        )));
                    }
                    return None;
                }
                if over && (0.0..=1.0).contains(&frac) && self.hover_differs(frac) {
                    return Some(canvas::Action::publish(Msg::Hover(Some((
                        panel.to_string(),
                        frac,
                    )))));
                }
                None
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                if !over {
                    return None;
                }
                let x = x_now.filter(|x| (plot.left..=plot.left + plot.width).contains(x))?;
                gesture.press = Some(x);
                gesture.dragging = false;
                gesture.last = frac_of(x).clamp(0.0, 1.0);
                None
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                let press = gesture.press.take()?;
                let dragging = std::mem::take(&mut gesture.dragging);
                let end = x_now.map_or(gesture.last, |x| frac_of(x).clamp(0.0, 1.0));
                if dragging {
                    return Some(canvas::Action::publish(Msg::DragEnd(end)));
                }
                let n = point_count(self.c);
                let moved = x_now.is_some_and(|x| (x - press).abs() >= CLICK_PX);
                if moved {
                    return None;
                }
                // A click: the point under the release, with its time (none on a labelled axis).
                let i = index_at(end, n)?;
                Some(canvas::Action::publish(Msg::Point(
                    panel.to_string(),
                    i,
                    time_of(&self.c.x, i),
                )))
            }
            mouse::Event::CursorLeft => {
                gesture.press = None;
                if std::mem::take(&mut gesture.dragging) {
                    // The pointer left mid-drag: the drag ends where it last was.
                    return Some(canvas::Action::publish(Msg::DragEnd(gesture.last)));
                }
                if self.pointer.hover_panel.as_deref() == Some(panel)
                    && self.pointer.hover.is_some()
                {
                    return Some(canvas::Action::publish(Msg::Hover(None)));
                }
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _gesture: &Gesture,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        self.paint(&mut frame, bounds);
        vec![frame.into_geometry()]
    }
}

/// The legend row above the chart: a swatch and a name per series (a scoped series is a button that narrows to its
/// scope), then the dashed comparison overlays and the band.
fn legend(c: &Chart) -> Element<'_, Msg> {
    let palette = crate::theme::palette();
    let mut items: Vec<Element<'_, Msg>> = Vec::new();
    for s in &c.series {
        let item = entry(swatch(tone(s.tone), s.dash), &s.name, palette.ink_2);
        items.push(match &s.scope {
            Some(scope) => button(item)
                .padding(0.0)
                .style(button::text)
                .on_press(Msg::Scope(scope.clone()))
                .into(),
            None => item,
        });
    }
    for g in &c.ghosts {
        items.push(entry(swatch(tone(g.tone), true), &g.name, palette.ink_3));
    }
    if let Some(b) = &c.band {
        items.push(entry(
            bar(14.0, 8.0, tone(b.tone).scale_alpha(0.3)),
            &b.name,
            palette.ink_2,
        ));
    }
    Row::with_children(items)
        .spacing(14.0)
        .align_y(alignment::Vertical::Center)
        .into()
}

/// A legend entry: its mark, then its name.
fn entry<'a>(mark: Element<'a, Msg>, name: &str, color: Color) -> Element<'a, Msg> {
    Row::with_children(vec![
        mark,
        text(name.to_string())
            .size(TYPE_META)
            .font(SANS)
            .color(color)
            .into(),
    ])
    .spacing(6.0)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// A legend mark for a series: a solid bar, or three short bars for a dashed one.
fn swatch<'a>(color: Color, dashed: bool) -> Element<'a, Msg> {
    if dashed {
        Row::with_children((0..3).map(|_| bar(3.0, 2.0, color)))
            .spacing(2.0)
            .into()
    } else {
        bar(14.0, 3.0, color)
    }
}

/// A plain coloured rectangle of the given size.
fn bar<'a>(width: f32, height: f32, color: Color) -> Element<'a, Msg> {
    container(Space::new().width(width).height(height))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..container::Style::default()
        })
        .into()
}

/// The number of points a chart draws: the longest series, or the label count of a labelled axis.
fn point_count(c: &Chart) -> usize {
    let mut n = match &c.x {
        XAxis::Labels { labels } => labels.len(),
        XAxis::Time { .. } => 0,
    };
    for s in c.series.iter().chain(&c.ghosts) {
        n = n.max(s.values.len());
    }
    if let Some(b) = &c.band {
        n = n.max(b.hi.len()).max(b.lo.len());
    }
    n
}

/// The first point of each pixel bucket when `n` points are drawn across `width` pixels (one bucket per point when
/// they fit).
fn bucket_starts(n: usize, width: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let m = width.clamp(1, n);
    (0..m).map(|j| j * n / m).collect()
}

/// One value per bucket: the largest of each bucket (the smallest when `smallest`), None when the bucket has no value.
fn thin(values: &[Option<f64>], starts: &[usize], smallest: bool) -> Vec<Option<f64>> {
    starts
        .iter()
        .enumerate()
        .map(|(j, &from)| {
            let end = starts
                .get(j + 1)
                .copied()
                .unwrap_or(values.len())
                .min(values.len());
            let bucket = values.get(from..end).unwrap_or(&[]);
            bucket
                .iter()
                .flatten()
                .copied()
                .reduce(|a, b| if smallest { a.min(b) } else { a.max(b) })
        })
        .collect()
}

/// The indexes of the x-axis labels: the first, the last and evenly spaced between them (three labels under 460 px
/// wide, five from there).
fn x_label_indexes(n: usize, width: f32) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let steps = if width < 460.0 { 2 } else { 4 };
    let mut out: Vec<usize> = Vec::new();
    for k in 0..=steps {
        let i = ((k as f64 / steps as f64) * (n - 1) as f64).round() as usize;
        if out.last() != Some(&i) {
            out.push(i);
        }
    }
    out
}

/// The index of the point at fraction `frac` (0 to 1) of the x range of `n` points: the nearest one, or None when
/// there are no points.
fn index_at(frac: f32, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(0);
    }
    let i = (frac.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize;
    Some(i.min(n - 1))
}

/// The time of point `i` on a time axis, or None on a labelled axis.
fn time_of(x: &XAxis, i: usize) -> Option<u64> {
    match x {
        XAxis::Time { start_ms, step_ms } => Some(*start_ms + i as u64 * *step_ms),
        XAxis::Labels { .. } => None,
    }
}

/// The label of point `i` on the x axis: its time, or its label.
fn x_text(x: &XAxis, i: usize, tz: i32) -> String {
    match x {
        XAxis::Time { start_ms, step_ms } => {
            time_label(*start_ms + i as u64 * *step_ms, *step_ms, tz)
        }
        XAxis::Labels { labels } => labels.get(i).cloned().unwrap_or_default(),
    }
}

/// The largest value in a list (0 when there is none).
fn peak(vals: &[Option<f64>]) -> f64 {
    vals.iter().flatten().fold(0.0, |m, &v| m.max(v))
}

/// The cumulative total of every series at each of the first `len` points (a missing value counts as zero).
fn stacked_totals(series: &[(&Series, Vec<Option<f64>>)], len: usize) -> Vec<f64> {
    (0..len)
        .map(|j| series.iter().map(|(_, v)| v[j].unwrap_or(0.0)).sum::<f64>())
        .collect()
}

/// The runs of nulls in a list, as [from, to) index pairs.
fn null_runs(vals: &[Option<f64>]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < vals.len() {
        if vals[i].is_some() {
            i += 1;
            continue;
        }
        let from = i;
        while i < vals.len() && vals[i].is_none() {
            i += 1;
        }
        runs.push((from, i));
    }
    runs
}

/// The values of a list as present values, for the band's edges.
fn some(values: &[f64]) -> Vec<Option<f64>> {
    values.iter().map(|v| Some(*v)).collect()
}

/// The points of `vals` at the x positions `xs` (None where there is no value).
fn trace(vals: &[Option<f64>], xs: &[f32], scale: &Scale) -> Vec<Option<Point>> {
    vals.iter()
        .zip(xs)
        .map(|(v, x)| v.map(|v| Point::new(*x, scale.y(v))))
        .collect()
}

/// A pen path through `points`: a None lifts the pen, so a gap is left without a line across it.
fn polyline(points: &[Option<Point>]) -> Path {
    Path::new(|builder| {
        let mut down = false;
        for point in points {
            match point {
                Some(p) => {
                    if down {
                        builder.line_to(*p);
                    } else {
                        builder.move_to(*p);
                    }
                    down = true;
                }
                None => down = false,
            }
        }
    })
}

/// A closed path through `points`, for an area or a band.
fn polygon(points: &[Point]) -> Path {
    Path::new(|builder| {
        for (k, p) in points.iter().enumerate() {
            if k == 0 {
                builder.move_to(*p);
            } else {
                builder.line_to(*p);
            }
        }
        builder.close();
    })
}

/// A stroke of `width` in `color`, dashed with `dash` when given.
fn stroke(color: Color, width: f32, dash: Option<&'static [f32]>) -> Stroke<'static> {
    let base = Stroke::default().with_color(color).with_width(width);
    match dash {
        Some(segments) => Stroke {
            line_dash: canvas::LineDash {
                segments,
                offset: 0,
            },
            ..base
        },
        None => base,
    }
}

/// Text on the chart at `at`, in the sans face at the label size.
fn text_at(
    frame: &mut canvas::Frame,
    content: impl Into<String>,
    at: Point,
    color: Color,
    align_x: alignment::Horizontal,
    align_y: alignment::Vertical,
) {
    frame.fill_text(canvas::Text {
        content: content.into(),
        position: at,
        color,
        size: iced::Pixels(LABEL_PX),
        font: SANS,
        align_x: align_x.into(),
        align_y,
        ..canvas::Text::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thinned_series_keeps_the_largest_value_of_each_pixel_bucket() {
        let values = [Some(1.0), Some(5.0), Some(2.0), None, Some(7.0), Some(3.0)];
        let starts = bucket_starts(values.len(), 3);
        assert_eq!(starts, vec![0, 2, 4]);
        assert_eq!(
            thin(&values, &starts, false),
            vec![Some(5.0), Some(2.0), Some(7.0)]
        );
        assert_eq!(
            thin(&values, &starts, true),
            vec![Some(1.0), Some(2.0), Some(3.0)]
        );
    }

    #[test]
    fn a_fraction_picks_the_nearest_point_and_its_time() {
        assert_eq!(index_at(0.5, 5), Some(2));
        assert_eq!(index_at(1.0, 10), Some(9));
        assert_eq!(index_at(0.0, 0), None);
        let axis = XAxis::Time {
            start_ms: 1_000,
            step_ms: 500,
        };
        assert_eq!(time_of(&axis, 3), Some(2_500));
        assert_eq!(time_of(&XAxis::Labels { labels: Vec::new() }, 0), None);
    }
}
