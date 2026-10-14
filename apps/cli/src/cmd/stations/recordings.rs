//! `agentplaza stations recordings`: the recorded leases on the hub (docs/design/workstations.md, section 7): the list,
//! one recording with its actions, its frames as numbered JPEGs, its deletion, and the defaults a station that joins
//! from now on takes (`/v1/stations/recording-defaults`).

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use apz_station::StationSettings;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{OnOff, dash, items, on_off, yes_no};
use crate::cmd::prompts::get_bytes;
use crate::ctx::{self, Ctx, confirm, seg};

/// The list's default and largest page (the hub clamps it to the same range).
const LIST_DEFAULT: u32 = 50;
const LIST_MAX: u32 = 200;
const LIST_HEAD: [&str; 9] = [
    "id", "station", "agent", "started", "ended", "frames", "actions", "size", "takeover",
];
const EVENT_HEAD: [&str; 4] = ["at", "kind", "ok", "summary"];
const FRAME_HEAD: [&str; 3] = ["seq", "at", "captured"];

#[derive(Subcommand, Debug)]
pub enum Action {
    /// One recording: its details and its newest actions. --frames lists each frame's time too.
    Show {
        /// The recording's id (from `agentplaza stations recordings`).
        id: String,
        /// List every frame with the time it was captured.
        #[arg(long)]
        frames: bool,
    },
    /// Download every frame as a numbered JPEG into a folder: frame-00001-<ms>.jpg, the number and the time the frame
    /// was captured (epoch ms), or frame-00001.jpg for a frame kept before frames had times.
    Frames {
        /// The recording's id.
        id: String,
        /// The folder to write the frames into (created when missing).
        #[arg(short, long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Delete a recording and its frames. Asks first on a terminal unless --yes.
    Delete {
        /// The recording's id.
        id: String,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
    /// The defaults for the stations that join from now on. `defaults set` changes the flags it names.
    Defaults {
        #[command(subcommand)]
        action: Option<DefaultsCmd>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DefaultsCmd {
    /// Change the defaults the flags name; the others keep their values.
    Set(DefaultsArgs),
}

/// The recording defaults' flags. Each one is optional: a flag not given keeps the saved value.
#[derive(clap::Args, Debug, Default, Clone, PartialEq, Eq)]
pub struct DefaultsArgs {
    /// Whether a lease on a station is recorded.
    #[arg(long, value_enum)]
    pub record: Option<OnOff>,
    /// How many days ended recordings are kept.
    #[arg(long, value_name = "DAYS")]
    pub retention_days: Option<u32>,
    /// Frames per second a pixel lease keeps, from 1 to 5.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=5))]
    pub fps: Option<u8>,
    /// Whether the owner's takeovers of a session are recorded too.
    #[arg(long, value_enum)]
    pub record_takeovers: Option<OnOff>,
}

/// The recording defaults as the hub holds them (the same fields as a station's settings). A missing field takes the
/// station default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub record: bool,
    pub retention_days: u32,
    pub record_fps: u8,
    pub record_takeovers: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        let station = StationSettings::default();
        Self {
            record: station.record,
            retention_days: station.retention_days,
            record_fps: station.record_fps,
            record_takeovers: station.record_takeovers,
        }
    }
}

/// `stations recordings` with the list's flags, or one of its actions.
pub fn run(
    ctx: &Ctx,
    station: Option<&str>,
    limit: Option<u32>,
    before: Option<u64>,
    action: Option<Action>,
) -> Result<(), String> {
    match action {
        None => list(ctx, station, limit.unwrap_or(LIST_DEFAULT), before),
        Some(action) => {
            if station.is_some() || limit.is_some() || before.is_some() {
                return Err("the list's flags (--station, --limit, --before) go with the list, not with an action".into());
            }
            act(ctx, action)
        }
    }
}

fn act(ctx: &Ctx, action: Action) -> Result<(), String> {
    match action {
        Action::Show { id, frames } => show(ctx, &id, frames),
        Action::Frames { id, out } => frames(ctx, &id, &out),
        Action::Delete { id, yes } => delete(ctx, &id, yes),
        Action::Defaults { action: None } => defaults_show(ctx),
        Action::Defaults {
            action: Some(DefaultsCmd::Set(set)),
        } => defaults_set(ctx, &set),
    }
}

fn list(ctx: &Ctx, station: Option<&str>, limit: u32, before: Option<u64>) -> Result<(), String> {
    check_limit(limit)?;
    let mut query = format!("limit={limit}");
    if let Some(station) = station.filter(|s| !s.is_empty()) {
        query.push_str(&format!("&station={}", seg(station)));
    }
    if let Some(before) = before {
        query.push_str(&format!("&before={before}"));
    }
    let rows = ctx
        .client
        .get(&format!("/v1/stations/recordings?{query}"))?;
    ctx.out(&rows, |v| {
        let rows: Vec<Vec<String>> = items(v).iter().map(recording_row).collect();
        ctx::table(&LIST_HEAD, &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str, frames: bool) -> Result<(), String> {
    let detail = ctx
        .client
        .get(&format!("/v1/stations/recordings/{}", seg(id)))?;
    ctx.out(&detail, |d| {
        ctx::fields(&recording_fields(d));
        println!("\nactions");
        let rows: Vec<Vec<String>> = items(&d["events"]).iter().map(event_row).collect();
        ctx::table(&EVENT_HEAD, &rows);
        if frames {
            println!("\nframes");
            let rows = frame_rows(d);
            if rows.is_empty() {
                println!("no frame times: none kept, or kept before frames had times");
            } else {
                ctx::table(&FRAME_HEAD, &rows);
            }
        }
    });
    Ok(())
}

fn frames(ctx: &Ctx, id: &str, out: &Path) -> Result<(), String> {
    let detail = ctx
        .client
        .get(&format!("/v1/stations/recordings/{}", seg(id)))?;
    let total = detail["frames"].as_u64().unwrap_or(0);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let width = frame_width(total);
    let stamps = frame_stamps(&detail);
    let tty = std::io::stderr().is_terminal();
    for seq in 1..=total {
        let bytes = get_bytes(
            ctx,
            &format!("/v1/stations/recordings/{}/frames/{seq}", seg(id)),
        )?;
        let path = out.join(frame_file_name(seq, width, stamps.get(&seq).copied()));
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if tty {
            eprint!("\r  frame {seq}/{total}");
        }
    }
    if tty && total > 0 {
        eprintln!();
    }
    let saved = json!({ "id": id, "frames": total, "dir": out.display().to_string() });
    ctx.out(&saved, |_| {
        println!("saved {total} frames of {id} to {}", out.display())
    });
    Ok(())
}

fn delete(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(yes, &format!("Delete recording {id} and its frames?"))? {
        println!("kept recording {id}");
        return Ok(());
    }
    let gone = ctx
        .client
        .delete(&format!("/v1/stations/recordings/{}", seg(id)))?;
    ctx.out(&gone, |_| println!("recording {id} deleted"));
    Ok(())
}

fn defaults_show(ctx: &Ctx) -> Result<(), String> {
    let current = ctx.client.get("/v1/stations/recording-defaults")?;
    ctx.out(&current, |v| ctx::fields(&defaults_fields(v)));
    Ok(())
}

fn defaults_set(ctx: &Ctx, set: &DefaultsArgs) -> Result<(), String> {
    check_defaults_set(set)?;
    let current: Defaults =
        serde_json::from_value(ctx.client.get("/v1/stations/recording-defaults")?)
            .map_err(|e| format!("the hub's recording defaults: {e}"))?;
    let body = serde_json::to_value(merged_defaults(current, set)).map_err(|e| e.to_string())?;
    let saved = ctx.client.put("/v1/stations/recording-defaults", &body)?;
    ctx.out(&saved, |v| ctx::fields(&defaults_fields(v)));
    Ok(())
}

// ---- Pure parts (tested) ----------------------------------------------------------------------

/// The defaults with the flags applied: a flag not given keeps the saved value.
pub fn merged_defaults(mut current: Defaults, set: &DefaultsArgs) -> Defaults {
    if let Some(on) = set.record {
        current.record = on == OnOff::On;
    }
    if let Some(days) = set.retention_days {
        current.retention_days = days;
    }
    if let Some(fps) = set.fps {
        current.record_fps = fps;
    }
    if let Some(on) = set.record_takeovers {
        current.record_takeovers = on == OnOff::On;
    }
    current
}

/// Refuses a `defaults set` that names nothing, before the hub is asked.
pub fn check_defaults_set(set: &DefaultsArgs) -> Result<(), String> {
    if *set == DefaultsArgs::default() {
        return Err("nothing to set: give at least one flag (see `agentplaza stations recordings defaults set --help`)".into());
    }
    Ok(())
}

/// The list's limit must be the hub's range; the hub would clamp anything else silently.
pub fn check_limit(limit: u32) -> Result<(), String> {
    if (1..=LIST_MAX).contains(&limit) {
        Ok(())
    } else {
        Err(format!("limit is a number from 1 to {LIST_MAX}"))
    }
}

/// The width of the frame numbers: at least five digits, and as many as the last frame needs.
pub fn frame_width(total: u64) -> usize {
    total.to_string().len().max(5)
}

/// The file name of frame `seq` (counted from 1), zero-padded to `width`.
pub fn frame_name(seq: u64, width: usize) -> String {
    format!("frame-{seq:0width$}.jpg")
}

/// The file name of a frame with its capture time (epoch ms) when the hub has it, so the names say when each frame was
/// kept; otherwise the plain numbered name.
pub fn frame_file_name(seq: u64, width: usize, captured_ms: Option<u64>) -> String {
    match captured_ms {
        Some(ms) => format!("frame-{seq:0width$}-{ms}.jpg"),
        None => frame_name(seq, width),
    }
}

/// The capture time of each frame the detail carries (`frame_times`), by seq.
pub fn frame_stamps(detail: &Value) -> BTreeMap<u64, u64> {
    items(&detail["frame_times"])
        .iter()
        .filter_map(|f| Some((f["seq"].as_u64()?, f["ts_ms"].as_u64()?)))
        .collect()
}

/// One frame's cells: its seq, its time into the recording, and when it was captured.
pub fn frame_rows(detail: &Value) -> Vec<Vec<String>> {
    let started = detail["started_ms"].as_u64().unwrap_or(0);
    items(&detail["frame_times"])
        .iter()
        .map(|f| {
            let ts = f["ts_ms"].as_u64().unwrap_or(0);
            vec![
                ctx::text(f, "seq"),
                offset_text(ts.saturating_sub(started)),
                ctx::when(&f["ts_ms"]),
            ]
        })
        .collect()
}

/// A time into a recording as `m:ss.mmm`, or `h:mm:ss.mmm` past an hour.
pub fn offset_text(ms: u64) -> String {
    let (h, m, s, milli) = (
        ms / 3_600_000,
        ms % 3_600_000 / 60_000,
        ms % 60_000 / 1000,
        ms % 1000,
    );
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{milli:03}")
    } else {
        format!("{m}:{s:02}.{milli:03}")
    }
}

/// One recording's cells: id, station, agent, start, end (or "open"), frames, actions, size and takeover.
pub fn recording_row(r: &Value) -> Vec<String> {
    vec![
        ctx::text(r, "id"),
        ctx::text(r, "station"),
        ctx::text(r, "agent"),
        ctx::when(&r["started_ms"]),
        ended(r),
        ctx::text(r, "frames"),
        ctx::text(r, "actions"),
        crate::cmd::lab::size_text(r["bytes"].as_u64().unwrap_or(0) as f64),
        yes_no(&r["takeover"]),
    ]
}

/// A recording's details as `recordings show` prints them above its actions.
pub fn recording_fields(r: &Value) -> Vec<(&'static str, String)> {
    let size = match (r["width"].as_u64(), r["height"].as_u64()) {
        (Some(w), Some(h)) => format!("{w}x{h}"),
        _ => "-".to_string(),
    };
    vec![
        ("id", ctx::text(r, "id")),
        ("station", ctx::text(r, "station")),
        ("lease", ctx::text(r, "lease")),
        ("agent", ctx::text(r, "agent")),
        ("task", dash(ctx::text(r, "task"))),
        ("started", ctx::when(&r["started_ms"])),
        ("ended", ended(r)),
        ("frames", ctx::text(r, "frames")),
        ("actions", ctx::text(r, "actions")),
        (
            "size",
            crate::cmd::lab::size_text(r["bytes"].as_u64().unwrap_or(0) as f64),
        ),
        ("frame size", size),
        ("takeover", yes_no(&r["takeover"])),
    ]
}

/// One action's cells: when, kind, whether it worked, and its summary as one line of JSON.
pub fn event_row(e: &Value) -> Vec<String> {
    vec![
        ctx::when(&e["at_ms"]),
        ctx::text(e, "kind"),
        yes_no(&e["ok"]),
        e.get("summary").map(Value::to_string).unwrap_or_default(),
    ]
}

/// A recording as one line for a short list: id, start, frames, size and whether the owner took over.
pub fn recording_brief(r: &Value) -> String {
    format!(
        "{}  {}  {} frames  {}  takeover {}",
        ctx::text(r, "id"),
        ctx::when(&r["started_ms"]),
        ctx::text(r, "frames"),
        crate::cmd::lab::size_text(r["bytes"].as_u64().unwrap_or(0) as f64),
        yes_no(&r["takeover"])
    )
}

/// The defaults as `defaults` prints them, in the flags' own words.
pub fn defaults_fields(d: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("record", on_off(&d["record"])),
        (
            "retention",
            format!("{} days", ctx::text(d, "retention_days")),
        ),
        ("record fps", ctx::text(d, "record_fps")),
        ("takeovers recorded", on_off(&d["record_takeovers"])),
    ]
}

fn ended(r: &Value) -> String {
    if r["ended_ms"].is_null() {
        "open".to_string()
    } else {
        ctx::when(&r["ended_ms"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_numbered_from_one_and_padded_to_the_last_one() {
        assert_eq!(frame_width(3), 5);
        assert_eq!(frame_width(123_456), 6);
        assert_eq!(frame_name(1, 5), "frame-00001.jpg");
        assert_eq!(frame_name(120_000, 6), "frame-120000.jpg");
        let names: Vec<String> = (1..=10)
            .map(|seq| frame_name(seq, frame_width(10)))
            .collect();
        assert_eq!(names.first().unwrap(), "frame-00001.jpg");
        assert_eq!(names.last().unwrap(), "frame-00010.jpg");
    }

    #[test]
    fn a_frame_file_names_its_capture_time_when_the_hub_has_it() {
        assert_eq!(
            frame_file_name(3, 5, Some(1_700_000_000_123)),
            "frame-00003-1700000000123.jpg"
        );
        assert_eq!(
            frame_file_name(3, 5, None),
            "frame-00003.jpg",
            "a frame without a time keeps the plain name"
        );
    }

    #[test]
    fn the_frame_times_read_as_offsets_into_the_recording() {
        let detail = json!({
            "started_ms": 1000,
            "frame_times": [{"seq": 1, "ts_ms": 1000}, {"seq": 2, "ts_ms": 1500}, {"seq": 3, "ts_ms": 66_250}]
        });
        let stamps = frame_stamps(&detail);
        assert_eq!(stamps.get(&2), Some(&1500));
        assert_eq!(stamps.len(), 3);
        let rows = frame_rows(&detail);
        assert_eq!(rows[0][0], "1");
        assert_eq!(rows[1][1], "0:00.500");
        assert_eq!(rows[2][1], "1:05.250");
        assert!(
            frame_stamps(&json!({"frame_times": []})).is_empty(),
            "an older recording has none"
        );
        assert!(frame_rows(&json!({})).is_empty());
    }

    #[test]
    fn an_offset_reads_in_minutes_then_hours() {
        assert_eq!(offset_text(0), "0:00.000");
        assert_eq!(offset_text(3_725_004), "1:02:05.004");
    }

    #[test]
    fn the_list_limit_is_the_hubs_range() {
        assert!(check_limit(1).is_ok());
        assert!(check_limit(200).is_ok());
        assert!(check_limit(0).is_err());
        assert!(check_limit(201).is_err());
    }

    #[test]
    fn defaults_merge_the_flags_and_keep_the_rest() {
        let current = Defaults {
            record: true,
            retention_days: 30,
            record_fps: 2,
            record_takeovers: true,
        };
        let set = DefaultsArgs {
            record: Some(OnOff::Off),
            fps: Some(4),
            ..DefaultsArgs::default()
        };
        let merged = merged_defaults(current, &set);
        assert!(!merged.record);
        assert_eq!(merged.record_fps, 4, "the frames a second the flag names");
        assert_eq!(merged.retention_days, 30);
        assert!(merged.record_takeovers);
        let takeovers_off = DefaultsArgs {
            record_takeovers: Some(OnOff::Off),
            retention_days: Some(7),
            ..DefaultsArgs::default()
        };
        let merged = merged_defaults(merged, &takeovers_off);
        assert!(!merged.record_takeovers);
        assert_eq!(merged.retention_days, 7);
        assert!(!merged.record, "a flag not given keeps the value");
    }

    #[test]
    fn a_defaults_set_with_no_flag_is_refused_before_the_hub() {
        assert!(check_defaults_set(&DefaultsArgs::default()).is_err());
        assert!(
            check_defaults_set(&DefaultsArgs {
                fps: Some(1),
                ..DefaultsArgs::default()
            })
            .is_ok()
        );
    }

    #[test]
    fn the_defaults_start_from_the_station_defaults() {
        let station = StationSettings::default();
        let defaults = Defaults::default();
        assert_eq!(defaults.record, station.record);
        assert_eq!(defaults.retention_days, station.retention_days);
        assert_eq!(defaults.record_fps, station.record_fps);
        assert_eq!(defaults.record_takeovers, station.record_takeovers);
        let body = serde_json::to_value(Defaults {
            record: false,
            retention_days: 7,
            record_fps: 1,
            record_takeovers: false,
        })
        .unwrap();
        assert_eq!(
            body,
            json!({"record": false, "retention_days": 7, "record_fps": 1, "record_takeovers": false})
        );
    }

    #[test]
    fn a_recording_row_shows_open_and_the_takeover() {
        let open = json!({
            "id": "r1", "station": "w1", "agent": 7, "started_ms": 0, "ended_ms": null,
            "frames": 12, "actions": 3, "bytes": 2048, "takeover": true
        });
        let cells = recording_row(&open);
        assert_eq!(cells[4], "open");
        assert_eq!(cells[5], "12");
        assert_eq!(cells[7], "2.0 KiB");
        assert_eq!(cells[8], "yes");
        assert_eq!(recording_fields(&open)[6], ("ended", "open".to_string()));
    }

    #[test]
    fn an_action_row_keeps_its_summary_as_json() {
        let event = json!({ "at_ms": 0, "kind": "click", "ok": false, "summary": {"x": 4} });
        let cells = event_row(&event);
        assert_eq!(cells[1], "click");
        assert_eq!(cells[2], "no");
        assert_eq!(cells[3], "{\"x\":4}");
    }

    #[test]
    fn a_brief_recording_line_names_the_frames_and_the_takeover() {
        let line = recording_brief(
            &json!({ "id": "r9", "started_ms": 0, "frames": 4, "bytes": 1024, "takeover": false }),
        );
        assert!(line.starts_with("r9  "), "{line}");
        assert!(line.ends_with("4 frames  1.0 KiB  takeover no"), "{line}");
    }

    #[test]
    fn the_defaults_read_in_the_flags_words() {
        let fields = defaults_fields(
            &json!({ "record": true, "retention_days": 30, "record_fps": 2, "record_takeovers": false }),
        );
        assert_eq!(fields[0], ("record", "on".to_string()));
        assert_eq!(fields[1], ("retention", "30 days".to_string()));
        assert_eq!(fields[3], ("takeovers recorded", "off".to_string()));
    }
}
