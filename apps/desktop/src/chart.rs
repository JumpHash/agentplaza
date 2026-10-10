//! The charts' arithmetic — pure, iced-free.
//!
//! A chart is a plate with a window across it and a ceiling above it: the
//! window is the observations' own span, the ceiling is a figure a person
//! would write at or above the peak, and every point is placed by one mapping
//! from milliseconds and values into the canvas. The iced canvas in
//! [`telemetryview`](crate::telemetryview) only paints what this computes, so
//! the scale, the window and the placement are unit-tested without a display.

use crate::history::FleetSample;

/// The room the value labels get on the left of a chart's plate.
pub const VALUE_GUTTER: f32 = 62.0;
/// The room above the plate's ceiling: it holds the top gridline's own figure,
/// which is drawn a line-height above the line rather than clipped by the
/// canvas's edge.
pub const TOP_PAD: f32 = 16.0;
/// The room to the right of the newest observation.
pub const RIGHT_PAD: f32 = 10.0;
/// The room under the baseline for the time labels.
pub const TIME_GUTTER: f32 = 16.0;

/// The value a chart's ceiling rounds up to: 1, 2, 2.5 or 5 times a power of
/// ten, so the gridlines land on figures a person would write. Never below 1,
/// so an empty fleet still has an axis to stand on.
pub fn nice_ceil(peak: f32) -> f32 {
    // Also catches a NaN or a negative peak: a chart has no ceiling below 1.
    if !(peak > 1.0) {
        return 1.0;
    }
    let magnitude = 10f32.powf(peak.log10().floor());
    for step in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if peak <= step * magnitude {
            return step * magnitude;
        }
    }
    10.0 * magnitude
}

/// The time window a chart draws: the observations' own span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The oldest observation's wall clock.
    pub t0: u64,
    /// The newest observation's wall clock.
    pub t1: u64,
}

impl Window {
    /// The span of `samples`. One observation still gets a second of axis, so
    /// a series of one is a point rather than a division by zero.
    pub fn of(samples: &[FleetSample]) -> Self {
        match (samples.first(), samples.last()) {
            (Some(first), Some(last)) if last.at_ms > first.at_ms => Self {
                t0: first.at_ms,
                t1: last.at_ms,
            },
            (Some(first), _) => Self {
                t0: first.at_ms,
                t1: first.at_ms.saturating_add(1_000),
            },
            _ => Self { t0: 0, t1: 1_000 },
        }
    }

    /// How long the window is, never zero: it divides.
    pub fn span_ms(&self) -> u64 {
        self.t1.saturating_sub(self.t0).max(1)
    }

    /// Where `at_ms` falls across a ruled width: `left` at the start of the
    /// window, `left + width` at its end.
    pub fn x(&self, at_ms: u64, left: f32, width: f32) -> f32 {
        let into = at_ms.clamp(self.t0, self.t1).saturating_sub(self.t0) as f32;
        left + width * (into / self.span_ms() as f32)
    }
}

/// The rectangle a chart's series is drawn in, inside its canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    /// The plate's left edge, past the value gutter.
    pub x: f32,
    /// The plate's top edge, under the ceiling's own room.
    pub y: f32,
    /// The plate's ruled width.
    pub width: f32,
    /// The plate's ruled height.
    pub height: f32,
}

impl Plate {
    /// The plate inside a canvas of `width` by `height`, leaving the value
    /// gutter on the left and the time labels under the baseline.
    pub fn inside(width: f32, height: f32) -> Self {
        Self {
            x: VALUE_GUTTER,
            y: TOP_PAD,
            width: (width - VALUE_GUTTER - RIGHT_PAD).max(1.0),
            height: (height - TOP_PAD - TIME_GUTTER).max(1.0),
        }
    }

    /// Where `value` falls on a scale from 0 at the baseline to `ceiling` at
    /// the plate's top.
    pub fn y(&self, value: f32, ceiling: f32) -> f32 {
        let ceiling = if ceiling > 0.0 { ceiling } else { 1.0 };
        self.y + self.height * (1.0 - (value.max(0.0) / ceiling).min(1.0))
    }

    /// The baseline's own y: where a value of zero is drawn.
    pub fn baseline(&self) -> f32 {
        self.y + self.height
    }

    /// The plate's right edge.
    pub fn right(&self) -> f32 {
        self.x + self.width
    }
}

/// The gridlines a chart rules across its plate, from the baseline up.
pub fn ruled(ceiling: f32) -> [f32; 3] {
    [0.0, ceiling / 2.0, ceiling]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FleetSample;

    fn sample(at_ms: u64) -> FleetSample {
        FleetSample {
            no: 1,
            at_ms,
            vms: 0,
            ram_mib: 0,
            lps: 0.0,
            runs: Vec::new(),
        }
    }

    #[test]
    fn the_ceiling_rounds_up_to_a_figure_a_person_would_write() {
        assert_eq!(nice_ceil(0.0), 1.0, "an empty fleet still has an axis");
        assert_eq!(nice_ceil(-4.0), 1.0, "and a nonsense peak no lower");
        assert_eq!(nice_ceil(1.0), 1.0);
        assert_eq!(nice_ceil(1.5), 2.0);
        assert_eq!(nice_ceil(2.0), 2.0);
        assert_eq!(nice_ceil(3.0), 5.0);
        assert_eq!(nice_ceil(12.0), 20.0);
        assert_eq!(nice_ceil(126.0), 200.0);
        assert_eq!(nice_ceil(6_400.0), 10_000.0);
        assert_eq!(nice_ceil(128_320.0), 200_000.0);
        // The ceiling is never below its own peak, whatever the magnitude.
        for peak in [0.4f32, 7.0, 99.0, 640.0, 4_096.0, 125_312.0] {
            assert!(nice_ceil(peak) >= peak, "peak {peak}");
        }
    }

    #[test]
    fn the_window_spans_the_first_and_last_observation() {
        let samples = [sample(1_760_000_000_000), sample(1_760_000_002_000)];
        let window = Window::of(&samples);
        assert_eq!(window.t0, 1_760_000_000_000);
        assert_eq!(window.t1, 1_760_000_002_000);
        assert_eq!(window.span_ms(), 2_000);
        assert_eq!(window.x(1_760_000_000_000, 60.0, 400.0), 60.0);
        assert_eq!(window.x(1_760_000_002_000, 60.0, 400.0), 460.0);
        assert_eq!(window.x(1_760_000_001_000, 60.0, 400.0), 260.0, "midway");
        // A time outside the window clamps to its edge rather than leaving the
        // plate.
        assert_eq!(window.x(1_759_999_999_000, 60.0, 400.0), 60.0);
    }

    #[test]
    fn a_single_observation_gets_a_window_of_one_second() {
        let one = [sample(1_760_000_000_000)];
        let window = Window::of(&one);
        assert_eq!(window.span_ms(), 1_000);
        assert_eq!(window.x(1_760_000_000_000, 60.0, 400.0), 60.0);
        assert_eq!(window.span_ms(), Window::of(&[]).span_ms());
    }

    #[test]
    fn the_plate_puts_the_ceiling_at_the_top_and_zero_at_the_baseline() {
        let plate = Plate::inside(640.0, 150.0);
        assert_eq!(plate.x, VALUE_GUTTER);
        assert_eq!(plate.y, TOP_PAD);
        assert_eq!(plate.right(), 640.0 - RIGHT_PAD);
        assert_eq!(plate.baseline(), 150.0 - TIME_GUTTER);
        assert_eq!(plate.y(0.0, 100.0), plate.baseline());
        assert_eq!(plate.y(100.0, 100.0), plate.y);
        assert_eq!(plate.y(50.0, 100.0), (plate.y + plate.baseline()) / 2.0);
        // A figure past the ceiling, or a degenerate one, still lands inside.
        assert_eq!(plate.y(400.0, 100.0), plate.y);
        assert_eq!(plate.y(10.0, 0.0), plate.y, "a zero ceiling is not a divide");
    }

    #[test]
    fn a_plate_too_small_to_hold_its_gutters_clamps_instead_of_inverting() {
        let tight = Plate::inside(40.0, 12.0);
        assert_eq!(tight.width, 1.0);
        assert_eq!(tight.height, 1.0);
    }

    #[test]
    fn the_gridlines_are_the_baseline_and_the_top_with_a_rule_between() {
        assert_eq!(ruled(200.0), [0.0, 100.0, 200.0]);
    }
}
