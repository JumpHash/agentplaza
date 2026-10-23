//! Cron expressions for schedule triggers (PLAN 14.6): the five fields of a crontab (minute, hour, day of month, month,
//! day of week), in UTC. Each field takes `*`, a number, a range `a-b`, a step `*/n` or `a-b/n`, and a list of those
//! separated by commas. Day of week is 0-7 (0 and 7 are Sunday). When both day fields are restricted, a day matches if
//! either does, as cron does.

use std::collections::BTreeSet;

const MINUTE_MS: u64 = 60_000;
const DAY_MINUTES: u64 = 1440;
/// How many days a search looks ahead: five years covers every valid day of month and weekday pair (Feb 29 included).
const HORIZON_DAYS: u64 = 366 * 5;

/// A parsed expression: the values each field allows, and whether the day fields were unrestricted (`*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: BTreeSet<u64>,
    hours: BTreeSet<u64>,
    days: BTreeSet<u64>,
    months: BTreeSet<u64>,
    weekdays: BTreeSet<u64>,
    days_star: bool,
    weekdays_star: bool,
}

/// Parses one field's values between `min` and `max` (inclusive).
fn field(text: &str, min: u64, max: u64, name: &str) -> Result<BTreeSet<u64>, String> {
    let mut out = BTreeSet::new();
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u64 = step
                    .parse()
                    .map_err(|_| format!("{name}: the step {step} is not a number"))?;
                if step == 0 {
                    return Err(format!("{name}: a step of 0 never fires"));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (number(a, name)?, number(b, name)?)
        } else if part.contains('/') {
            (number(range, name)?, max)
        } else {
            let v = number(range, name)?;
            (v, v)
        };
        if lo < min || hi > max || lo > hi {
            return Err(format!("{name}: {range} is outside {min}-{max}"));
        }
        let mut v = lo;
        while v <= hi {
            out.insert(v);
            v += step;
        }
    }
    Ok(out)
}

fn number(text: &str, name: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{name}: {text} is not a number"))
}

/// Parses a five-field expression.
pub fn parse(expr: &str) -> Result<Cron, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return Err(format!(
            "a cron expression has five fields (minute hour day month weekday), not {}",
            fields.len()
        ));
    };
    let mut weekdays = field(dow, 0, 7, "weekday")?;
    if weekdays.remove(&7) {
        weekdays.insert(0);
    }
    Ok(Cron {
        minutes: field(minute, 0, 59, "minute")?,
        hours: field(hour, 0, 23, "hour")?,
        days: field(dom, 1, 31, "day")?,
        months: field(month, 1, 12, "month")?,
        weekdays,
        days_star: dom.starts_with('*'),
        weekdays_star: dow.starts_with('*'),
    })
}

/// The civil date of a day count since 1970-01-01 (proleptic Gregorian): (year, month, day).
fn civil(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u64;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u64;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

impl Cron {
    /// Whether day `day` (days since 1970-01-01, UTC) is one the expression fires on.
    fn day_matches(&self, day: u64) -> bool {
        let (_, month, dom) = civil(day as i64);
        let weekday = (day + 4) % 7;
        if !self.months.contains(&month) {
            return false;
        }
        let dom_ok = self.days.contains(&dom);
        let dow_ok = self.weekdays.contains(&weekday);
        match (self.days_star, self.weekdays_star) {
            (true, true) => true,
            (true, false) => dow_ok,
            (false, true) => dom_ok,
            (false, false) => dom_ok || dow_ok,
        }
    }

    /// The first minute strictly after `ms` (Unix milliseconds) the expression fires at, in milliseconds. None when it
    /// never fires within the search horizon.
    pub fn next_after(&self, ms: u64) -> Option<u64> {
        let first = ms / MINUTE_MS + 1;
        let first_day = first / DAY_MINUTES;
        for day in first_day..first_day + HORIZON_DAYS {
            if !self.day_matches(day) {
                continue;
            }
            for hour in &self.hours {
                for minute in &self.minutes {
                    let at = day * DAY_MINUTES + hour * 60 + minute;
                    if at >= first {
                        return Some(at * MINUTE_MS);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix milliseconds of a UTC date and time (the test's fake clock).
    fn at(y: i64, mo: u64, d: u64, h: u64, mi: u64) -> u64 {
        // days from civil (Hinnant), the inverse of `civil`
        let y = if mo <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = (if mo > 2 { mo - 3 } else { mo + 9 }) as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        (days as u64 * DAY_MINUTES + h * 60 + mi) * MINUTE_MS
    }

    #[test]
    fn a_daily_time_fires_at_that_minute_and_then_the_next_day() {
        let cron = parse("30 9 * * *").unwrap();
        let now = at(2026, 10, 23, 9, 29);
        assert_eq!(cron.next_after(now), Some(at(2026, 10, 23, 9, 30)));
        assert_eq!(
            cron.next_after(at(2026, 10, 23, 9, 30)),
            Some(at(2026, 10, 24, 9, 30))
        );
    }

    #[test]
    fn steps_lists_ranges_and_weekdays_are_read() {
        let every_quarter = parse("*/15 * * * *").unwrap();
        assert_eq!(
            every_quarter.next_after(at(2026, 10, 23, 9, 1)),
            Some(at(2026, 10, 23, 9, 15))
        );
        let weekdays = parse("0 8 * * 1-5").unwrap();
        // 2026-10-23 is a Friday, 2026-10-24 a Saturday, 2026-10-26 a Monday.
        assert_eq!(
            weekdays.next_after(at(2026, 10, 23, 9, 0)),
            Some(at(2026, 10, 26, 8, 0))
        );
        let sunday = parse("0 0 * * 7").unwrap();
        assert_eq!(
            sunday.next_after(at(2026, 10, 23, 0, 0)),
            Some(at(2026, 10, 25, 0, 0))
        );
        let list = parse("5,35 */12 * * *").unwrap();
        assert_eq!(
            list.next_after(at(2026, 10, 23, 0, 6)),
            Some(at(2026, 10, 23, 0, 35))
        );
    }

    #[test]
    fn a_restricted_day_of_month_and_weekday_match_either_as_cron_does() {
        // the 13th, or any Friday: 2026-10-23 is a Friday
        let either = parse("0 0 13 * 5").unwrap();
        assert_eq!(
            either.next_after(at(2026, 10, 14, 0, 0)),
            Some(at(2026, 10, 16, 0, 0))
        );
        let leap = parse("0 0 29 2 *").unwrap();
        assert_eq!(
            leap.next_after(at(2026, 10, 23, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn a_bad_expression_says_which_field_is_wrong() {
        assert!(parse("* * * *").unwrap_err().contains("five fields"));
        assert!(parse("60 * * * *").unwrap_err().contains("minute"));
        assert!(parse("*/0 * * * *").unwrap_err().contains("step"));
        assert!(parse("* 25 * * *").unwrap_err().contains("hour"));
        assert!(parse("* * * * mon").unwrap_err().contains("weekday"));
    }

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(
            civil(at(2026, 10, 23, 0, 0) as i64 / 86_400_000),
            (2026, 10, 23)
        );
    }
}
