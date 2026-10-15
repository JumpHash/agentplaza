//! Formatting shared by both consoles. The rules (pinned by `tests/format_golden.json`, which the web test suite
//! also reads, so `apps/web/src/metrics/format.ts` cannot drift):
//!
//! * not finite → `—`; negative → `-` then the absolute value formatted.
//! * usd: `$` then ≥ 1000 → whole with `,` thousands (`$1,264`); ≥ 100 → 0 decimals; ≥ 10 → 1; else 2.
//! * tokens: ≥ 1e9 → `1.23B`; ≥ 1e6 → `41.7M`; ≥ 1e3 → `9.8k`; else whole.
//! * count: ≥ 1000 → as tokens; else fractional below 10 → 1 decimal, otherwise whole.
//! * ms: ≥ 1000 → seconds (` s`), 0 decimals ≥ 10 s else 1 (`2.4 s`, `14 s`); else whole ` ms`.
//! * percent: < 10 → 1 decimal, else whole, then `%`.
//! * mib: ≥ 1024 → GiB with 1 decimal; else whole ` MiB`.
//! * bytes: < 1024 → whole ` B`; else KiB/MiB/GiB/TiB with 1 decimal below 100, whole above.
//! * bytes_per_sec: ≥ 1 GiB → `GiB/s` 1 decimal; ≥ 1 MiB → `MiB/s` 1 decimal; ≥ 1 KiB → whole `KiB/s`; else
//!   whole `B/s`.
//! * tokens_per_sec: whole ` tok/s`. ratio: 2 decimals.
//!
//! Rounding is half away from zero on the printed digits. Time labels use English month abbreviations and
//! 24-hour times at a given UTC offset.

use apz_metrics::catalog::Unit;

const KIB: f64 = 1024.0;
const MIB: f64 = KIB * 1024.0;
const GIB: f64 = MIB * 1024.0;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A value as text.
pub fn format(unit: Unit, v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    if v < 0.0 {
        return format!("-{}", format(unit, -v));
    }
    match unit {
        Unit::Usd => {
            if v >= 1000.0 {
                format!("${}", group_thousands(&fixed(v, 0)))
            } else {
                let decimals = if v >= 100.0 {
                    0
                } else if v >= 10.0 {
                    1
                } else {
                    2
                };
                format!("${}", fixed(v, decimals))
            }
        }
        Unit::Tokens => short_number(v),
        Unit::Count => {
            if v >= 1000.0 {
                short_number(v)
            } else if v < 10.0 {
                fixed(v, 1)
            } else {
                fixed(v, 0)
            }
        }
        Unit::Ms => {
            if v >= 1000.0 {
                let seconds = v / 1000.0;
                let decimals = if seconds >= 10.0 { 0 } else { 1 };
                format!("{} s", fixed(seconds, decimals))
            } else {
                format!("{} ms", fixed(v, 0))
            }
        }
        Unit::Percent => {
            let decimals = if v < 10.0 { 1 } else { 0 };
            format!("{}%", fixed(v, decimals))
        }
        Unit::Mib => {
            if v >= 1024.0 {
                format!("{} GiB", fixed(v / 1024.0, 1))
            } else {
                format!("{} MiB", fixed(v, 0))
            }
        }
        Unit::Bytes => bytes(v),
        Unit::BytesPerSec => {
            if v >= GIB {
                format!("{} GiB/s", fixed(v / GIB, 1))
            } else if v >= MIB {
                format!("{} MiB/s", fixed(v / MIB, 1))
            } else if v >= KIB {
                format!("{} KiB/s", fixed(v / KIB, 0))
            } else {
                format!("{} B/s", fixed(v, 0))
            }
        }
        Unit::TokensPerSec => format!("{} tok/s", fixed(v, 0)),
        Unit::Ratio => fixed(v, 2),
    }
}

/// The axis ceiling: the smallest of 1, 2, 2.5, 5, 10 × 10^k at or above `max` (1 for `max <= 0`).
pub fn nice(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let scale = 10f64.powi(max.log10().floor() as i32);
    let ratio = max / scale;
    [1.0, 2.0, 2.5, 5.0]
        .into_iter()
        .find(|step| *step >= ratio)
        .unwrap_or(10.0)
        * scale
}

/// Five ticks from 0 to `nice(max · 1.05)`.
pub fn ticks(max: f64) -> [f64; 5] {
    let ym = nice(max * 1.05);
    [0.0, ym / 4.0, ym / 2.0, ym * 3.0 / 4.0, ym]
}

/// The label of a point at `t_ms` on an axis stepping `step_ms`, at `offset_min` minutes from UTC:
/// step ≥ 1 day → `Oct 16`; step ≥ 1 h → `Oct 16 14:00`; step < 1 min → `14:20:05`; else `14:20`.
pub fn time_label(t_ms: u64, step_ms: u64, offset_min: i32) -> String {
    const MINUTE_MS: i64 = 60_000;
    const HOUR_MS: i64 = 60 * MINUTE_MS;
    const DAY_MS: i64 = 24 * HOUR_MS;

    let local = t_ms as i64 + i64::from(offset_min) * MINUTE_MS;
    let (month, day) = civil_from_days(local.div_euclid(DAY_MS));
    let in_day = local.rem_euclid(DAY_MS);
    let hour = in_day / HOUR_MS;
    let minute = in_day % HOUR_MS / MINUTE_MS;
    let second = in_day % MINUTE_MS / 1000;

    let clock = format!("{hour:02}:{minute:02}");
    let date = format!("{} {day}", MONTHS[(month - 1) as usize]);
    if step_ms >= DAY_MS as u64 {
        date
    } else if step_ms >= HOUR_MS as u64 {
        format!("{date} {clock}")
    } else if step_ms < MINUTE_MS as u64 {
        format!("{clock}:{second:02}")
    } else {
        clock
    }
}

/// The absolute value of `v` rounded half away from zero to `decimals` places. The caller prints the sign.
fn fixed(v: f64, decimals: u32) -> String {
    let scaled = (v.abs() * 10f64.powi(decimals as i32)).round();
    let digits = format!("{scaled:.0}");
    if decimals == 0 {
        return digits;
    }
    let places = decimals as usize;
    let padded = format!("{digits:0>width$}", width = places + 1);
    let (whole, fraction) = padded.split_at(padded.len() - places);
    format!("{whole}.{fraction}")
}

/// Thousands and above as `9.8k`, `41.7M` or `1.23B`; below a thousand, whole.
fn short_number(v: f64) -> String {
    if v >= 1e9 {
        format!("{}B", fixed(v / 1e9, 2))
    } else if v >= 1e6 {
        format!("{}M", fixed(v / 1e6, 1))
    } else if v >= 1e3 {
        format!("{}k", fixed(v / 1e3, 1))
    } else {
        fixed(v, 0)
    }
}

/// A byte count in the largest binary unit that keeps it at or above 1, with 1 decimal below 100.
fn bytes(v: f64) -> String {
    if v < KIB {
        return format!("{} B", fixed(v, 0));
    }
    let names = ["KiB", "MiB", "GiB", "TiB"];
    let mut value = v / KIB;
    let mut index = 0;
    while value >= KIB && index + 1 < names.len() {
        value /= KIB;
        index += 1;
    }
    let decimals = if value < 100.0 { 1 } else { 0 };
    format!("{} {}", fixed(value, decimals), names[index])
}

/// Digits between commas from the right, for a string of whole digits.
fn group_thousands(digits: &str) -> String {
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The month (1 to 12) and day of the month for a count of days since 1970-01-01 (Howard Hinnant's
/// days-to-civil algorithm).
fn civil_from_days(days: i64) -> (i64, i64) {
    let z = days + 719_468;
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (month, day)
}
