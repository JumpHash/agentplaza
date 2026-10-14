//! Shared helpers for the Lab screens, as the web's `lab-common.ts`: the formatters every section
//! uses (counts, sizes, durations, percents, GPU hours, times), the hub's live `training` frames on
//! the event stream, and the empty state. The shown screen reloads on every poll tick (the console's
//! 4 s `Tick`), so a Lab screen polls by its `load`; one that wants the live lines subscribes with
//! [`live`] and reads them with [`training_frame`].

use std::time::{SystemTime, UNIX_EPOCH};

use iced::{Element, Subscription};
use serde_json::Value;

use crate::console::{client::Client, events, ui};

/// A trainer line from the hub's `training` frame: the job's id and its stored event line
/// (`{seq, island, node, event}`, where `event.ev` names the trainer's event).
#[derive(Debug, Clone)]
pub struct TrainingFrame {
    pub job: String,
    pub line: Value,
}

/// The hub's event stream, for a screen that wants live `training` frames. Map its events through
/// [`training_frame`]; the stream is shared with every other subscriber of the same hub.
pub fn live(client: &Client) -> Subscription<events::Event> {
    events::subscription(client)
}

/// The `training` frame an event carries, or None for any other event.
pub fn training_frame(event: &events::Event) -> Option<TrainingFrame> {
    if event.kind != "training" {
        return None;
    }
    let job = event.data.get("job")?.as_str()?.to_string();
    let line = event.data.get("line")?.clone();
    Some(TrainingFrame { job, line })
}

/// The empty (or not-yet-built) state: the brain mark, a mono title and a serif sentence.
pub fn empty<'a, M: 'a>(title: &str, sentence: &str) -> Element<'a, M> {
    ui::empty("brain", title, sentence)
}

/// A count or quantity with grouping and `digits` decimals; a dash for a value that is not finite.
pub fn number(n: f64, digits: usize) -> String {
    if !n.is_finite() {
        return "—".to_string();
    }
    let text = format!("{:.*}", digits, n.abs());
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text.as_str(), None),
    };
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    let len = whole.len();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

/// A whole count with grouping.
pub fn count(n: u64) -> String {
    number(n as f64, 0)
}

/// A byte count in binary units (B, KB, MB, GB, TB).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    format!("{:.*} {}", digits, value, UNITS[unit])
}

/// A duration from milliseconds: `840 ms`, `12.5 s`, `4m 07s`, `2h 13m`.
pub fn duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let seconds = ms as f64 / 1000.0;
    if seconds < 60.0 {
        return format!("{seconds:.1} s");
    }
    let total = seconds.round() as u64;
    if total < 3600 {
        format!("{}m {:02}s", total / 60, total % 60)
    } else {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    }
}

/// A ratio (0 to 1) as a percent: `0.42` is `42%`. Pass `util_pct / 100` for the device figures.
pub fn percent(ratio: f64) -> String {
    if !ratio.is_finite() {
        return "—".to_string();
    }
    format!("{:.0}%", ratio * 100.0)
}

/// The GPU time a job or run has used.
pub fn gpu_hours(hours: f64) -> String {
    if !hours.is_finite() {
        return "—".to_string();
    }
    format!("{hours:.2} GPU-h")
}

/// An absolute local time from unix milliseconds, `2026-10-13 14:05` (the web's `fmtTime` prints the
/// same); a dash for 0.
pub fn time(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    match chrono::DateTime::from_timestamp_millis(ms as i64) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => "—".to_string(),
    }
}

/// A relative time, as the web's `ago`; `ms` is unix milliseconds.
pub fn ago(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let seconds = now_ms().saturating_sub(ms) / 1000;
    match seconds {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// The current time, unix milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
