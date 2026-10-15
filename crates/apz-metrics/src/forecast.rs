//! Projections, labelled as estimates wherever they are shown.

use crate::Point;
use serde::{Deserialize, Serialize};

const DAY_MS: f64 = 86_400_000.0;

/// Crossings further away than this are not reported.
const TEN_YEARS_DAYS: f64 = 10.0 * 365.25;

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
    let Some(fit) = Fit::of(history) else {
        return Projection::default();
    };
    if step_ms == 0 {
        return Projection::default();
    }
    let p10 = nearest_rank(&fit.residuals, 0.1);
    let p90 = nearest_rank(&fit.residuals, 0.9);
    let mut out = Projection {
        per_day: fit.slope,
        ..Projection::default()
    };
    let mut t = fit.last_ms;
    while let Some(next) = t.checked_add(step_ms) {
        if next > until_ms {
            break;
        }
        t = next;
        let value = fit.at((t - fit.last_ms) as f64 / DAY_MS);
        let steps = (t - fit.last_ms) as f64 / step_ms as f64;
        let widen = steps.sqrt().max(1.0);
        out.points.push((t, value));
        out.lo.push(value + p10 * widen);
        out.hi.push(value + p90 * widen);
    }
    out
}

/// When the fitted trend of `history` reaches `limit` (rising or falling towards it), if it does within ten
/// years. `None` when flat, moving away, or with fewer than two points.
pub fn crossing(history: &[Point], limit: f64) -> Option<u64> {
    let fit = Fit::of(history)?;
    if fit.slope == 0.0 {
        return None;
    }
    // Days from the last point at which the fitted line equals the limit. Zero or fewer means the line has
    // already reached the limit in its direction of travel, so the last point is the answer.
    let days = (limit - fit.intercept) / fit.slope;
    if !days.is_finite() {
        return None;
    }
    if days <= 0.0 {
        return Some(fit.last_ms);
    }
    if days > TEN_YEARS_DAYS {
        return None;
    }
    Some(fit.last_ms + (days * DAY_MS).round() as u64)
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
    let mut values: Vec<f64> = daily.iter().map(|&(_, spend)| spend).collect();
    let burn = if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };
    values.sort_by(|a, b| a.total_cmp(b));
    MonthSpend {
        month_to_date,
        projected: month_to_date + burn * days_left,
        lo: month_to_date + nearest_rank(&values, 0.1) * days_left,
        hi: month_to_date + nearest_rank(&values, 0.9) * days_left,
        burn_per_day: burn,
    }
}

/// The basis the owner chose (Settings → Metrics): 14 days by default, or 30.
pub const DEFAULT_BASIS_DAYS: u32 = 14;

/// A least-squares line over a history. Time is measured in days from the last point, so the last point sits at
/// zero and earlier points are negative.
struct Fit {
    /// The time of the last point, in ms.
    last_ms: u64,
    /// Change of the fitted value per day.
    slope: f64,
    /// The fitted value at the last point.
    intercept: f64,
    /// Observed minus fitted value for each point, ascending.
    residuals: Vec<f64>,
}

impl Fit {
    /// Fits `history`, or `None` when it has fewer than two points.
    fn of(history: &[Point]) -> Option<Fit> {
        if history.len() < 2 {
            return None;
        }
        let last_ms = history[history.len() - 1].0;
        let days: Vec<f64> = history
            .iter()
            .map(|&(t, _)| (t as f64 - last_ms as f64) / DAY_MS)
            .collect();
        let n = history.len() as f64;
        let mean_days = days.iter().sum::<f64>() / n;
        let mean_value = history.iter().map(|&(_, v)| v).sum::<f64>() / n;
        let spread: f64 = days.iter().map(|d| (d - mean_days).powi(2)).sum();
        let covariance: f64 = days
            .iter()
            .zip(history)
            .map(|(d, &(_, v))| (d - mean_days) * (v - mean_value))
            .sum();
        // Every point at one instant gives no trend, so the slope is zero rather than a division by zero.
        let slope = if spread > 0.0 {
            covariance / spread
        } else {
            0.0
        };
        let intercept = mean_value - slope * mean_days;
        let mut residuals: Vec<f64> = days
            .iter()
            .zip(history)
            .map(|(d, &(_, v))| v - (intercept + slope * d))
            .collect();
        residuals.sort_by(|a, b| a.total_cmp(b));
        Some(Fit {
            last_ms,
            slope,
            intercept,
            residuals,
        })
    }

    /// The fitted value `days` from the last point.
    fn at(&self, days: f64) -> f64 {
        self.intercept + self.slope * days
    }
}

/// The value at nearest rank `p` (from 0 to 1) of an ascending list, or 0 when the list is empty.
fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn a_rising_line_projects_upward_inside_its_band() {
        let history: Vec<Point> = [
            (0, 10.0),
            (1, 12.0),
            (2, 11.5),
            (3, 14.0),
            (4, 15.5),
            (5, 15.0),
            (6, 17.0),
        ]
        .iter()
        .map(|&(day, value)| (day * DAY, value))
        .collect();
        let projection = linear(&history, 10 * DAY, DAY);
        assert_eq!(projection.points.len(), 4);
        assert_eq!(projection.points[0].0, 7 * DAY);
        assert!(projection.per_day > 0.0);
        assert!(projection.points.last().unwrap().1 > 17.0);
        for (i, &(_, value)) in projection.points.iter().enumerate() {
            assert!(projection.lo[i] <= value && value <= projection.hi[i]);
        }
        assert!(projection.hi[3] > projection.lo[3]);
    }

    #[test]
    fn a_disk_at_ninety_percent_rising_a_point_a_day_is_seen_reaching_full() {
        let history: Vec<Point> = (0..=10).map(|day| (day * DAY, 80.0 + day as f64)).collect();
        assert_eq!(crossing(&history, 100.0), Some(20 * DAY));
    }

    #[test]
    fn month_spend_projects_month_to_date_plus_the_mean_for_the_days_left() {
        let daily = [(0, 10.0), (DAY, 20.0), (2 * DAY, 30.0)];
        let spend = month_spend(&daily, 100.0, 10.0);
        assert_eq!(spend.burn_per_day, 20.0);
        assert_eq!(spend.projected, 300.0);
        assert_eq!(spend.lo, 200.0);
        assert_eq!(spend.hi, 400.0);
    }
}
