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

/// A value as text.
pub fn format(unit: Unit, v: f64) -> String {
    let _ = (unit, v);
    todo!()
}

/// The axis ceiling: the smallest of 1, 2, 2.5, 5, 10 × 10^k at or above `max` (1 for `max <= 0`).
pub fn nice(max: f64) -> f64 {
    let _ = max;
    todo!()
}

/// Five ticks from 0 to `nice(max · 1.05)`.
pub fn ticks(max: f64) -> [f64; 5] {
    let ym = nice(max * 1.05);
    [0.0, ym / 4.0, ym / 2.0, ym * 3.0 / 4.0, ym]
}

/// The label of a point at `t_ms` on an axis stepping `step_ms`, at `offset_min` minutes from UTC:
/// step ≥ 1 day → `Oct 16`; step ≥ 1 h → `Oct 16 14:00`; step < 1 min → `14:20:05`; else `14:20`.
pub fn time_label(t_ms: u64, step_ms: u64, offset_min: i32) -> String {
    let _ = (t_ms, step_ms, offset_min);
    todo!("civil-from-days; no chrono")
}
