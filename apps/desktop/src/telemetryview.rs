//! The session's figures as iced canvases: the fleet charts and the run pane's
//! sparkline.
//!
//! Every drawn thing comes from [`chart`](crate::chart) and
//! [`history`](crate::history) — the window, the ceiling, the stretch a line
//! is drawn in — so what a frame paints and what the unit tests assert are the
//! same arithmetic. The plaza's language carries over from the map: ruled
//! gridlines, mono labels, the ink pair, and the one hot signal for the run
//! the operator picked.

use std::ops::Range;

use iced::mouse;
use iced::widget::canvas::{self, Action, Canvas, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::chart::{self, Plate, Window};
use crate::history::{self, FleetSample, RunSlice, HISTORY_CAP};
use crate::map;
use crate::model::{Message, duration_label};
use crate::theme;

/// Which figure a fleet chart draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    /// The guest memory the live VMs have committed, in MiB.
    Ram,
    /// How many VMs are up.
    Vms,
    /// The fleet's serial throughput, in lines per second.
    Throughput,
}

impl ChartKind {
    /// The figure's name, as the chart's caption starts.
    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Ram => "committed guest RAM",
            ChartKind::Vms => "live vms",
            ChartKind::Throughput => "fleet serial throughput",
        }
    }

    /// The fleet's own figure at one observation.
    pub fn value(self, sample: &FleetSample) -> f32 {
        match self {
            ChartKind::Ram => sample.ram_mib as f32,
            ChartKind::Vms => sample.vms as f32,
            ChartKind::Throughput => sample.lps,
        }
    }

    /// One run's figure at one observation.
    pub fn slice_value(self, slice: &RunSlice) -> f32 {
        match self {
            ChartKind::Ram => slice.ram_mib as f32,
            ChartKind::Vms => slice.vms as f32,
            ChartKind::Throughput => slice.lps,
        }
    }

    /// A figure on the axis, as the register writes figures.
    pub fn tick(self, value: f32) -> String {
        match self {
            ChartKind::Ram => crate::model::memory_label(value.round() as u64),
            ChartKind::Vms => format!("{value:.0}"),
            ChartKind::Throughput => format!("{value:.0}"),
        }
    }
}

/// One fleet chart: the observations, the figure to draw, and the run whose
/// band carries the register's one hot signal.
#[derive(Debug)]
pub struct FleetChart<'a> {
    /// The observations in the window, oldest first.
    pub samples: &'a [FleetSample],
    /// Which figure to draw.
    pub kind: ChartKind,
    /// The picked run: with one picked, RAM and live VMs decompose into that
    /// run's bands and throughput lays the run's own line over the fleet's.
    pub picked: Option<&'a str>,
    /// A ceiling the chart cannot be drawn under whatever the figures do: the
    /// host's own memory, for the RAM chart. It never raises the scale — the
    /// chart is the fleet's own figures — it is ruled inside them when the
    /// fleet has been asked for more than the host has.
    pub cap: Option<f32>,
    /// The spacing past which two observations are unobserved time.
    pub gap_ms: u64,
}

/// One fleet chart, filling its slot.
pub fn chart(plate: FleetChart<'_>) -> Canvas<FleetChart<'_>, Message> {
    Canvas::new(plate)
        .width(Length::Fill)
        .height(Length::Fill)
}

impl FleetChart<'_> {
    /// Draws the gridlines, the time axis, and the figures themselves.
    fn paint(&self, frame: &mut Frame, size: Size) {
        let plate = Plate::inside(size.width, size.height);
        let window = Window::of(self.samples);
        let peak = self
            .samples
            .iter()
            .map(|sample| self.kind.value(sample))
            .fold(0.0f32, f32::max);
        let ceiling = chart::nice_ceil(peak);
        let spans = history::segments(self.samples, self.gap_ms);

        self.paint_axes(frame, &plate, &window, ceiling);

        let stacked = self.picked.is_some() && self.kind != ChartKind::Throughput;
        if stacked {
            self.paint_stack(frame, &plate, &window, ceiling, &spans);
        } else {
            self.paint_line(
                frame,
                &plate,
                &window,
                ceiling,
                &spans,
                theme::INK,
                1.5,
                |sample| self.kind.value(sample),
            );
        }
        if self.kind == ChartKind::Throughput {
            if let Some(run) = self.picked {
                self.paint_line(frame, &plate, &window, ceiling, &spans, theme::SIGNAL, 1.8, |sample| {
                    slice_value(sample, run, self.kind)
                });
            }
        }

        // Unobserved time is un-inked: a dotted rule in the margin's colour
        // marks each stretch the register did not see.
        self.paint_gaps(frame, &plate, &window, &spans);

        // The newest observation, blotted.
        if let Some(newest) = self.samples.last() {
            let (value, colour) = match (self.kind, self.picked) {
                (ChartKind::Throughput, Some(run)) => {
                    (slice_value(newest, run, self.kind), theme::SIGNAL)
                }
                (kind, Some(_)) => (kind.value(newest), theme::INK),
                (kind, None) => (kind.value(newest), theme::INK),
            };
            let point = Point::new(
                window.x(newest.at_ms, plate.x, plate.width),
                plate.y(value, ceiling),
            );
            frame.fill_rectangle(
                Point::new(point.x - 2.0, point.y - 2.0),
                Size::new(4.0, 4.0),
                colour,
            );
        }
    }

    /// The ruled gridlines, their figures, the time axis, and the host cap.
    fn paint_axes(&self, frame: &mut Frame, plate: &Plate, window: &Window, ceiling: f32) {
        for value in chart::ruled(ceiling) {
            let y = plate.y(value, ceiling);
            frame.stroke(
                &Path::line(Point::new(plate.x, y), Point::new(plate.right(), y)),
                Stroke::default()
                    // The baseline is ruled in ink: it is where zero is.
                    .with_color(if value == 0.0 { theme::INK_2 } else { theme::RULE })
                    .with_width(1.0),
            );
            frame.fill_text(canvas_text(
                self.kind.tick(value),
                Point::new(4.0, y - 13.0),
                9.5,
                theme::INK_3,
            ));
        }
        // The ceiling the machine's own memory puts on a RAM chart, when the
        // fleet has not reached it: the overcommit story's own line.
        if let Some(cap) = self.cap.filter(|cap| *cap > 0.0 && *cap < ceiling) {
            let y = plate.y(cap, ceiling);
            frame.stroke(
                &Path::line(Point::new(plate.x, y), Point::new(plate.right(), y)),
                Stroke::default().with_color(theme::WARN).with_width(1.0),
            );
            frame.fill_text(canvas_text(
                format!("host {}", self.kind.tick(cap)),
                Point::new(4.0, y - 13.0),
                9.0,
                theme::WARN,
            ));
        }
        frame.fill_text(canvas_text(
            format!("-{}", duration_label(window.span_ms())),
            Point::new(plate.x, plate.baseline() + 3.0),
            9.5,
            theme::INK_3,
        ));
        frame.fill_text(canvas_text(
            "now".to_string(),
            Point::new(plate.right() - 21.0, plate.baseline() + 3.0),
            9.5,
            theme::INK_3,
        ));
    }

    /// One series as a line per continuous stretch.
    #[allow(clippy::too_many_arguments)]
    fn paint_line(
        &self,
        frame: &mut Frame,
        plate: &Plate,
        window: &Window,
        ceiling: f32,
        spans: &[Range<usize>],
        color: Color,
        width: f32,
        value: impl Fn(&FleetSample) -> f32,
    ) {
        for span in spans {
            let points = self.stretch(span, plate, window, ceiling, &value);
            stroke_path(frame, &points, color, width, false);
        }
    }

    /// The runs a stacked chart lays bands for, in the sequence their ids name
    /// and then by id, so a band keeps its place from frame to frame.
    fn band_order(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self
            .samples
            .iter()
            .flat_map(|sample| sample.runs.iter().map(|slice| slice.run.as_str()))
            .collect();
        ids.sort_unstable_by_key(|id| (map::seq_of(id), *id));
        ids.dedup();
        ids
    }

    /// The decomposing view: one closed band per run, its floor the bands under
    /// it and its roof its own figure above them.
    fn paint_stack(
        &self,
        frame: &mut Frame,
        plate: &Plate,
        window: &Window,
        ceiling: f32,
        spans: &[Range<usize>],
    ) {
        let order = self.band_order();
        for (at, run) in order.iter().enumerate() {
            // A run with no figure anywhere in the window has no band: an
            // accent drawn on the floor would be a figure nobody observed.
            if self
                .samples
                .iter()
                .all(|sample| slice_value(sample, run, self.kind) <= 0.0)
            {
                continue;
            }
            let floor = |sample: &FleetSample| -> f32 {
                order[..at]
                    .iter()
                    .map(|id| slice_value(sample, id, self.kind))
                    .sum()
            };
            let signal = self.picked == Some(*run);
            let color = if signal { theme::SIGNAL } else { theme::INK_3 };
            let width = if signal { 1.8 } else { 1.0 };
            for span in spans {
                let mut points: Vec<Point> = Vec::with_capacity((span.end - span.start) * 2);
                for index in span.clone() {
                    let sample = &self.samples[index];
                    let roof = floor(sample) + slice_value(sample, run, self.kind);
                    points.push(point_at(window, plate, ceiling, sample, roof));
                }
                for index in (span.start..span.end).rev() {
                    let sample = &self.samples[index];
                    points.push(point_at(window, plate, ceiling, sample, floor(sample)));
                }
                stroke_path(frame, &points, color, width, true);
            }
        }
    }

    /// The dotted rules that mark the stretches nobody observed.
    fn paint_gaps(
        &self,
        frame: &mut Frame,
        plate: &Plate,
        window: &Window,
        spans: &[Range<usize>],
    ) {
        for span in spans.iter().skip(1) {
            let left = self.samples[span.start - 1].at_ms;
            let right = self.samples[span.start].at_ms;
            let x = window.x(left + (right - left) / 2, plate.x, plate.width);
            let mut y = plate.y;
            while y < plate.baseline() {
                let to = (y + 3.0).min(plate.baseline());
                frame.stroke(
                    &Path::line(Point::new(x, y), Point::new(x, to)),
                    Stroke::default().with_color(theme::MARGIN).with_width(1.0),
                );
                y += 6.0;
            }
        }
    }

    /// One stretch of one series, in the plate's own coordinates.
    fn stretch(
        &self,
        span: &Range<usize>,
        plate: &Plate,
        window: &Window,
        ceiling: f32,
        value: &impl Fn(&FleetSample) -> f32,
    ) -> Vec<Point> {
        span.clone()
            .map(|index| {
                let sample = &self.samples[index];
                point_at(window, plate, ceiling, sample, value(sample))
            })
            .collect()
    }
}

impl canvas::Program<Message> for FleetChart<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        _event: &Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        // The charts are read, not pressed: the picker above them is widgets.
        None
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        self.paint(&mut frame, bounds.size());
        vec![frame.into_geometry()]
    }
}

/// A VM's own serial rate, as the run pane's sparkline: the register's ink on
/// a ruled baseline, grown in from the right of the window the ring keeps.
#[derive(Debug)]
pub struct Sparkline<'a> {
    /// The VM's smoothed rates, oldest first.
    pub points: &'a [f32],
    /// Whether the VM is still up: a live series ends in the ok blot.
    pub live: bool,
}

/// One VM's sparkline, filling its slot.
pub fn sparkline(line: Sparkline<'_>) -> Canvas<Sparkline<'_>, Message> {
    Canvas::new(line)
        .width(Length::Fill)
        .height(Length::Fill)
}

impl Sparkline<'_> {
    /// Draws the baseline and the VM's own series.
    fn paint(&self, frame: &mut Frame, size: Size) {
        let left = 1.0;
        let width = (size.width - 2.0).max(1.0);
        let top = 3.0;
        let height = (size.height - 6.0).max(1.0);
        let baseline = top + height;

        frame.stroke(
            &Path::line(Point::new(left, baseline), Point::new(left + width, baseline)),
            Stroke::default().with_color(theme::RULE).with_width(1.0),
        );

        let ceiling = chart::nice_ceil(self.points.iter().copied().fold(0.0f32, f32::max));
        let slots = (HISTORY_CAP - 1) as f32;
        let offset = HISTORY_CAP.saturating_sub(self.points.len()) as f32;
        let at = |index: usize, value: f32| {
            Point::new(
                left + width * ((offset + index as f32) / slots),
                baseline - height * (value.max(0.0) / ceiling).min(1.0),
            )
        };

        let points: Vec<Point> = self
            .points
            .iter()
            .enumerate()
            .map(|(index, value)| at(index, *value))
            .collect();
        stroke_path(frame, &points, theme::INK, 1.5, false);

        if let Some(last) = self.points.last() {
            let point = at(self.points.len() - 1, *last);
            frame.fill_rectangle(
                Point::new(point.x - 2.0, point.y - 2.0),
                Size::new(4.0, 4.0),
                if self.live { theme::OK } else { theme::INK_3 },
            );
        }
    }
}

impl canvas::Program<Message> for Sparkline<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        _event: &Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        None
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        self.paint(&mut frame, bounds.size());
        vec![frame.into_geometry()]
    }
}

/// Where one observation's figure stands on a chart.
fn point_at(window: &Window, plate: &Plate, ceiling: f32, sample: &FleetSample, value: f32) -> Point {
    Point::new(
        window.x(sample.at_ms, plate.x, plate.width),
        plate.y(value, ceiling),
    )
}

/// One run's figure at one observation, and nothing when the run is not in it.
fn slice_value(sample: &FleetSample, run: &str, kind: ChartKind) -> f32 {
    sample
        .runs
        .iter()
        .find(|slice| slice.run == run)
        .map_or(0.0, |slice| kind.slice_value(slice))
}

/// Strokes a polyline. A single point is not a line, so a lone observation is
/// left to the blot rather than drawn as a zero-length stroke.
fn stroke_path(frame: &mut Frame, points: &[Point], color: Color, width: f32, close: bool) {
    if points.is_empty() {
        return;
    }
    frame.stroke(
        &Path::new(|builder| {
            builder.move_to(points[0]);
            for point in &points[1..] {
                builder.line_to(*point);
            }
            if close {
                builder.close();
            }
        }),
        Stroke::default().with_color(color).with_width(width),
    );
}

/// A canvas label in the register's mono voice, anchored at `position`.
fn canvas_text(content: String, position: Point, size: f32, color: Color) -> Text {
    Text {
        content,
        position,
        color,
        size: size.into(),
        font: Font::MONOSPACE,
        align_x: Alignment::Left,
        ..Text::default()
    }
}
