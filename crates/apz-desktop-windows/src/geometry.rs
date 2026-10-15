//! Screen geometry: points onto the absolute range SendInput takes, UI Automation rectangles onto bounds, and
//! wheel notches onto wheel data.

/// The top of the absolute range SendInput maps a virtual screen onto.
pub const ABSOLUTE_MAX: i32 = 65_535;

/// The data of one wheel notch (`WHEEL_DELTA`).
pub const WHEEL_DELTA: i32 = 120;

/// Maps one axis of a screen point onto the 0..=65535 range that `MOUSEEVENTF_ABSOLUTE` takes, over a virtual
/// screen that starts at `origin` and is `extent` points wide. The first point maps to 0 and the last to 65535;
/// points off the screen clamp to the ends.
pub fn absolute_axis(point: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 1 {
        return 0;
    }
    let offset = i64::from(point) - i64::from(origin);
    let scaled = offset * i64::from(ABSOLUTE_MAX) / i64::from(extent - 1);
    scaled.clamp(0, i64::from(ABSOLUTE_MAX)) as i32
}

/// A UI Automation rectangle (left, top, right, bottom) as `(x, y, w, h)`. An inverted rectangle has no size.
pub fn rect_bounds(left: i32, top: i32, right: i32, bottom: i32) -> (i32, i32, u32, u32) {
    (left, top, span(left, right), span(top, bottom))
}

fn span(from: i32, to: i32) -> u32 {
    u32::try_from(i64::from(to) - i64::from(from)).unwrap_or(0)
}

/// The `mouseData` of a vertical wheel event. Positive `notches` scroll the content down, as a DOM wheel event
/// does, which is the opposite sign to Windows' own wheel (positive is away from the user).
pub fn vertical_wheel(notches: i32) -> u32 {
    notches.saturating_mul(-WHEEL_DELTA) as u32
}

/// The `mouseData` of a horizontal wheel event. Positive `notches` scroll the content right.
pub fn horizontal_wheel(notches: i32) -> u32 {
    notches.saturating_mul(WHEEL_DELTA) as u32
}

/// The point `step` of `steps` of the way from `from` to `to`, on a straight line.
pub fn between(from: (i32, i32), to: (i32, i32), step: i32, steps: i32) -> (i32, i32) {
    let along = |a: i32, b: i32| {
        let delta = i64::from(b) - i64::from(a);
        (i64::from(a) + delta * i64::from(step) / i64::from(steps.max(1))) as i32
    };
    (along(from.0, to.0), along(from.1, to.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_axis_maps_the_screen_onto_the_full_range() {
        // The screen edges map to the range ends; a virtual screen left of the
        // primary starts at its own origin; points off the screen clamp to its
        // edges; a one-point screen has no range.
        for (case, value, origin, size, want) in [
            ("the left edge maps to 0", 0, 0, 1920, 0),
            ("the right edge maps to 65535", 1919, 0, 1920, 65_535),
            ("the middle maps to the middle", 960, 0, 1920, 32_784),
            (
                "a virtual screen's left edge maps to 0",
                -1920,
                -1920,
                3840,
                0,
            ),
            (
                "a virtual screen's right edge maps to 65535",
                1919,
                -1920,
                3840,
                65_535,
            ),
            (
                "a virtual screen's middle maps to its middle",
                0,
                -1920,
                3840,
                32_776,
            ),
            ("a point left of the screen clamps to 0", -5000, 0, 1920, 0),
            (
                "a point right of the screen clamps to 65535",
                5000,
                0,
                1920,
                65_535,
            ),
            ("a one-point screen at 0 maps to 0", 0, 0, 1, 0),
            ("a zero-size screen maps to 0", 0, 0, 0, 0),
        ] {
            assert_eq!(absolute_axis(value, origin, size), want, "{case}");
        }
    }

    #[test]
    fn wheel_deltas_follow_the_notch_direction() {
        // Positive vertical notches scroll down.
        for (case, notches, want) in [
            ("a positive vertical notch", 1, (-120i32) as u32),
            ("negative vertical notches", -3, 360),
            ("no vertical notches", 0, 0),
        ] {
            assert_eq!(vertical_wheel(notches), want, "{case}");
        }
        // Positive horizontal notches scroll right.
        for (case, notches, want) in [
            ("positive horizontal notches", 2, 240),
            ("a negative horizontal notch", -1, (-120i32) as u32),
        ] {
            assert_eq!(horizontal_wheel(notches), want, "{case}");
        }
    }
}
