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
    fn the_screen_edges_map_to_the_range_ends() {
        assert_eq!(absolute_axis(0, 0, 1920), 0);
        assert_eq!(absolute_axis(1919, 0, 1920), 65_535);
        assert_eq!(absolute_axis(960, 0, 1920), 32_784);
    }

    #[test]
    fn a_virtual_screen_left_of_the_primary_starts_at_its_own_origin() {
        assert_eq!(absolute_axis(-1920, -1920, 3840), 0);
        assert_eq!(absolute_axis(1919, -1920, 3840), 65_535);
        assert_eq!(absolute_axis(0, -1920, 3840), 32_776);
    }

    #[test]
    fn points_off_the_screen_clamp_to_its_edges() {
        assert_eq!(absolute_axis(-5000, 0, 1920), 0);
        assert_eq!(absolute_axis(5000, 0, 1920), 65_535);
    }

    #[test]
    fn a_one_point_screen_has_no_range() {
        assert_eq!(absolute_axis(0, 0, 1), 0);
        assert_eq!(absolute_axis(0, 0, 0), 0);
    }

    #[test]
    fn rectangles_become_bounds_and_inverted_ones_have_no_size() {
        assert_eq!(rect_bounds(10, 20, 110, 70), (10, 20, 100, 50));
        assert_eq!(rect_bounds(10, 20, 5, 5), (10, 20, 0, 0));
        assert_eq!(rect_bounds(-30, -40, 30, 40), (-30, -40, 60, 80));
    }

    #[test]
    fn positive_vertical_notches_scroll_down() {
        assert_eq!(vertical_wheel(1), (-120i32) as u32);
        assert_eq!(vertical_wheel(-3), 360);
        assert_eq!(vertical_wheel(0), 0);
    }

    #[test]
    fn positive_horizontal_notches_scroll_right() {
        assert_eq!(horizontal_wheel(2), 240);
        assert_eq!(horizontal_wheel(-1), (-120i32) as u32);
    }

    #[test]
    fn between_walks_a_straight_line() {
        assert_eq!(between((0, 0), (80, 40), 4, 8), (40, 20));
        assert_eq!(between((0, 0), (10, 10), 8, 8), (10, 10));
        assert_eq!(between((5, 5), (5, 5), 3, 8), (5, 5));
    }
}
