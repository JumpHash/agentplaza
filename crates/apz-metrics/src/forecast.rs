//! Projections, labelled as estimates wherever they are shown.

use crate::Point;
use serde::{Deserialize, Serialize};

/// A projected line with a p10–p90 band.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub points: Vec<Point>,
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    /// Slope per day of the fitted line.
    pub per_day: f64,
}

/// Fits a least-squares line to `history` and extends it from the last point to `until_ms` by `step_ms`. The
/// band is the fitted value plus the 10th and 90th percentile of the residuals, widened with the square root of
/// the distance from the last point (in steps). Fewer than two points give an empty projection.
pub fn linear(history: &[Point], until_ms: u64, step_ms: u64) -> Projection {
    let _ = (history, until_ms, step_ms);
    todo!()
}

/// When the fitted trend of `history` reaches `limit` (rising or falling towards it), if it does within ten
/// years. `None` when flat, moving away, or with fewer than two points.
pub fn crossing(history: &[Point], limit: f64) -> Option<u64> {
    let _ = (history, limit);
    todo!()
}

/// Month-end spend from daily totals of the month so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MonthSpend {
    pub month_to_date: f64,
    pub projected: f64,
    pub lo: f64,
    pub hi: f64,
    /// Mean spend per day over the basis window.
    pub burn_per_day: f64,
}

/// `daily` holds (day start ms, spend that day) for up to the last `basis_days` days (14 by default), which may
/// reach into the previous month; `month_to_date` is the month's spend so far; `days_left` the whole days left in
/// the month after today. The projection is month-to-date plus the basis mean for the days left; the band uses
/// the 10th and 90th percentile of the daily values.
pub fn month_spend(daily: &[(u64, f64)], month_to_date: f64, days_left: f64) -> MonthSpend {
    let _ = (daily, month_to_date, days_left);
    todo!()
}

/// The basis the owner chose (Settings → Metrics): 14 days by default, or 30.
pub const DEFAULT_BASIS_DAYS: u32 = 14;
