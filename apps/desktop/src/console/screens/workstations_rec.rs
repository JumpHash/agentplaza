//! Workstation recordings (docs/design/workstations.md, sections 7 and 9): each station's recorded leases, with their
//! frames and actions, played back on the station's page; and the hub's recording defaults, for the stations that join
//! from now on.
//!
//!   GET    /v1/stations/recordings?station=&limit=     → [Recording], newest first
//!   GET    /v1/stations/recordings/{id}                → Recording + `events` (oldest first)
//!   GET    /v1/stations/recordings/{id}/frames/{seq}   → the frame's JPEG (seq counts from 1)
//!   DELETE /v1/stations/recordings/{id}                → {id, deleted} (not while its lease is open)
//!   GET    /v1/stations/recording-defaults             → RecordingDefaults
//!   PUT    /v1/stations/recording-defaults             RecordingDefaults → the saved defaults

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use apz_http::client::request;
use iced::widget::{checkbox, column, container, image, pick_list, row, slider, text_input};
use iced::{Alignment, ContentFit, Element, Length, Subscription, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::lab_common::now_ms;
use super::workstations::{Load, cells, field, mono_text, settle_into, table};
use crate::console::{Ctx, client, ui};
use crate::theme;

/// The frames a recording keeps a second at most (the hub's cap for its defaults).
const FPS_CAP: u8 = 5;
/// The most days a recording is kept: the hub's bounds.
const RETENTION_DAYS: (u32, u32) = (1, 3650);
/// How often the playhead moves while a recording plays, in ms.
const TICK_MS: u64 = 100;
/// How many frames ahead of the playhead are fetched, and how many are kept behind and ahead of it.
const AHEAD: u64 = 3;
const KEEP: u64 = 40;
/// The rows of recordings and of events a page shows.
const LIST_ROWS: u64 = 50;
const EVENT_ROWS: usize = 200;

/// A recording (`GET /v1/stations/recordings`): one lease's frames and actions.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Recording {
    pub id: String,
    pub station: String,
    pub lease: String,
    pub agent: u64,
    pub task: Option<String>,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    pub frames: u64,
    pub actions: u64,
    pub bytes: u64,
    pub takeover: bool,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

/// One action of a recording: when, its kind, whether it worked, and what it was.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Event {
    pub at_ms: u64,
    pub kind: String,
    pub ok: bool,
    pub summary: Value,
}

/// A recording with its actions (`GET /v1/stations/recordings/{id}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Detail {
    #[serde(flatten)]
    pub recording: Recording,
    pub events: Vec<Event>,
}

/// The recording settings the hub gives the stations that join from now on.
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
        Defaults {
            record: true,
            retention_days: 30,
            record_fps: 2,
            record_takeovers: true,
        }
    }
}

/// The defaults form as the owner edits it; numbers as text, so a half-typed value is still a value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DefaultsForm {
    record: bool,
    retention: String,
    fps: String,
    takeovers: bool,
}

impl From<&Defaults> for DefaultsForm {
    fn from(d: &Defaults) -> DefaultsForm {
        DefaultsForm {
            record: d.record,
            retention: d.retention_days.to_string(),
            fps: d.record_fps.to_string(),
            takeovers: d.record_takeovers,
        }
    }
}

/// One edit of the defaults form.
#[derive(Debug, Clone)]
pub enum DefaultsEdit {
    Record(bool),
    Retention(String),
    Fps(String),
    Takeovers(bool),
}

/// The playback speeds the picker offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedPick(pub f32);

impl std::fmt::Display for SpeedPick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}×", self.0)
    }
}

const SPEEDS: [SpeedPick; 4] = [
    SpeedPick(0.5),
    SpeedPick(1.0),
    SpeedPick(2.0),
    SpeedPick(4.0),
];

/// A recording open on the page: its detail, the playhead, and the frames fetched so far.
#[derive(Debug)]
struct Open {
    id: String,
    detail: Load<Detail>,
    /// The playhead, in ms from the recording's start.
    pos: u64,
    playing: bool,
    speed: f32,
    frames: HashMap<u64, image::Handle>,
    pending: BTreeSet<u64>,
}

#[derive(Debug, Default)]
pub struct State {
    /// The station the list is for, and its recordings.
    station: String,
    pub list: Load<Vec<Recording>>,
    open: Option<Open>,
    /// The recording whose deletion is asked for.
    confirm: Option<String>,
    pub defaults: Load<Defaults>,
    form: DefaultsForm,
    baseline: Option<DefaultsForm>,
    form_error: Option<String>,
    saving: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    ListLoaded(String, Result<Vec<Recording>, String>),
    Open(String),
    Opened(String, Result<Detail, String>),
    Close,
    TogglePlay,
    Speed(f32),
    Scrub(f64),
    Tick,
    Frame(String, u64, Result<Vec<u8>, String>),
    AskDelete(String),
    CancelDelete,
    Delete,
    Deleted(Result<Value, String>),
    DefaultsLoaded(Result<Defaults, String>),
    Edit(DefaultsEdit),
    SaveDefaults,
    DefaultsSaved(Result<Defaults, String>),
}

/// Loads a station's recordings. A change of station starts its list over.
pub fn load(state: &mut State, ctx: &Ctx, station: &str) -> Task<Msg> {
    if state.station != station {
        *state = State {
            station: station.to_string(),
            ..Default::default()
        };
    }
    let client = ctx.client.clone();
    let path = format!("/v1/stations/recordings?station={station}&limit={LIST_ROWS}");
    let id = station.to_string();
    client::task(
        move || client.get::<Vec<Recording>>(&path),
        move |r| Msg::ListLoaded(id.clone(), r),
    )
}

/// Loads the hub's recording defaults.
pub fn load_defaults(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Defaults>("/v1/stations/recording-defaults"),
        Msg::DefaultsLoaded,
    )
}

/// Reads one path's bytes from the hub, or the status it answered with.
fn get_bytes(client: &client::Client, path: &str) -> Result<Vec<u8>, String> {
    let reply =
        request(&client.target, "GET", path, None, client.timeout).map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// The frames the playhead needs and does not have: the frame it is on and the few after it.
fn fetch_task(ctx: &Ctx, open: &mut Open, frames: u64, duration: u64) -> Task<Msg> {
    let seq = frame_at(open.pos, duration, frames);
    let mut tasks = Vec::new();
    let last = seq.saturating_add(AHEAD).min(frames);
    for want in seq.max(1)..=last {
        if open.frames.contains_key(&want) || open.pending.contains(&want) {
            continue;
        }
        open.pending.insert(want);
        let client = ctx.client.clone();
        let path = format!("/v1/stations/recordings/{}/frames/{want}", open.id);
        let id = open.id.clone();
        tasks.push(client::task(
            move || get_bytes(&client, &path),
            move |r| Msg::Frame(id.clone(), want, r),
        ));
    }
    let keep_from = seq.saturating_sub(KEEP);
    let keep_to = seq.saturating_add(KEEP);
    open.frames.retain(|k, _| (keep_from..=keep_to).contains(k));
    Task::batch(tasks)
}

/// The recording's length as the page knows it: its frames' span when it is closed, or to now while it is open.
fn length_of(detail: &Detail) -> u64 {
    duration_ms(&detail.recording, now_ms())
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::ListLoaded(station, result) => {
            if station == state.station {
                settle_into(&mut state.list, result);
            }
            Task::none()
        }
        Msg::Open(id) => {
            state.open = Some(Open {
                id: id.clone(),
                detail: Load::Pending,
                pos: 0,
                playing: false,
                speed: 1.0,
                frames: HashMap::new(),
                pending: BTreeSet::new(),
            });
            state.confirm = None;
            let client = ctx.client.clone();
            let path = format!("/v1/stations/recordings/{id}");
            client::task(
                move || client.get::<Detail>(&path),
                move |r| Msg::Opened(id.clone(), r),
            )
        }
        Msg::Opened(id, result) => {
            let Some(open) = state.open.as_mut().filter(|o| o.id == id) else {
                return Task::none();
            };
            settle_into(&mut open.detail, result);
            match &open.detail {
                Load::Ready(detail) => {
                    let (frames, duration) = (detail.recording.frames, length_of(detail));
                    fetch_task(ctx, open, frames, duration)
                }
                _ => Task::none(),
            }
        }
        Msg::Close => {
            state.open = None;
            state.confirm = None;
            Task::none()
        }
        Msg::TogglePlay => {
            let Some(open) = state.open.as_mut() else {
                return Task::none();
            };
            let Load::Ready(detail) = &open.detail else {
                return Task::none();
            };
            let duration = length_of(detail);
            if open.pos >= duration {
                open.pos = 0;
            }
            open.playing = !open.playing;
            Task::none()
        }
        Msg::Speed(speed) => {
            if let Some(open) = state.open.as_mut() {
                open.speed = speed;
            }
            Task::none()
        }
        Msg::Scrub(fraction) => {
            let Some(open) = state.open.as_mut() else {
                return Task::none();
            };
            let Load::Ready(detail) = &open.detail else {
                return Task::none();
            };
            open.pos = at_fraction(fraction, length_of(detail));
            fetch_task_for(ctx, state)
        }
        Msg::Tick => {
            let Some(open) = state.open.as_mut() else {
                return Task::none();
            };
            let Load::Ready(detail) = &open.detail else {
                return Task::none();
            };
            let duration = length_of(detail);
            open.pos = advance(open.pos, TICK_MS, open.speed, duration);
            if open.pos >= duration {
                open.playing = false;
            }
            fetch_task_for(ctx, state)
        }
        Msg::Frame(id, seq, result) => {
            let Some(open) = state.open.as_mut().filter(|o| o.id == id) else {
                return Task::none();
            };
            open.pending.remove(&seq);
            match result {
                Ok(bytes) => {
                    open.frames.insert(seq, image::Handle::from_bytes(bytes));
                }
                Err(error) => state.error = Some(format!("A frame did not load: {error}")),
            }
            Task::none()
        }
        Msg::AskDelete(id) => {
            state.confirm = Some(id);
            Task::none()
        }
        Msg::CancelDelete => {
            state.confirm = None;
            Task::none()
        }
        Msg::Delete => {
            let Some(id) = state.confirm.take() else {
                return Task::none();
            };
            let client = ctx.client.clone();
            let path = format!("/v1/stations/recordings/{id}");
            client::task(move || client.delete(&path), Msg::Deleted)
        }
        Msg::Deleted(Ok(_)) => {
            state.error = None;
            state.notice = Some("Recording deleted".into());
            state.open = None;
            let station = state.station.clone();
            load(state, ctx, &station)
        }
        Msg::Deleted(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::DefaultsLoaded(result) => {
            settle_into(&mut state.defaults, result);
            if let Load::Ready(saved) = &state.defaults {
                let form = DefaultsForm::from(saved);
                if state.baseline.as_ref().is_none_or(|b| *b == state.form) {
                    state.baseline = Some(form.clone());
                    state.form = form;
                }
            }
            Task::none()
        }
        Msg::Edit(edit) => {
            state.form_error = None;
            match edit {
                DefaultsEdit::Record(on) => state.form.record = on,
                DefaultsEdit::Retention(text) => state.form.retention = text,
                DefaultsEdit::Fps(text) => state.form.fps = text,
                DefaultsEdit::Takeovers(on) => state.form.takeovers = on,
            }
            Task::none()
        }
        Msg::SaveDefaults => {
            if state.saving {
                return Task::none();
            }
            let defaults = match defaults_of(&state.form) {
                Ok(defaults) => defaults,
                Err(error) => {
                    state.form_error = Some(error);
                    return Task::none();
                }
            };
            state.saving = true;
            state.form_error = None;
            let body = serde_json::to_value(defaults).unwrap_or_default();
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("PUT", "/v1/stations/recording-defaults", Some(&body))
                        .and_then(|v| {
                            serde_json::from_value::<Defaults>(v).map_err(|e| e.to_string())
                        })
                },
                Msg::DefaultsSaved,
            )
        }
        Msg::DefaultsSaved(result) => {
            state.saving = false;
            match result {
                Ok(saved) => {
                    let form = DefaultsForm::from(&saved);
                    state.baseline = Some(form.clone());
                    state.form = form;
                    state.defaults = Load::Ready(saved);
                    state.notice = Some("Recording defaults saved".into());
                    state.error = None;
                }
                Err(error) => state.form_error = Some(error),
            }
            Task::none()
        }
    }
}

/// The open recording's frames, for the playhead as it stands now.
fn fetch_task_for(ctx: &Ctx, state: &mut State) -> Task<Msg> {
    let Some(open) = state.open.as_mut() else {
        return Task::none();
    };
    let Load::Ready(detail) = &open.detail else {
        return Task::none();
    };
    let (frames, duration) = (detail.recording.frames, length_of(detail));
    fetch_task(ctx, open, frames, duration)
}

/// The playback's clock: only while a recording plays.
pub fn subscription(state: &State) -> Subscription<Msg> {
    match &state.open {
        Some(open) if open.playing => {
            iced::time::every(Duration::from_millis(TICK_MS)).map(|_| Msg::Tick)
        }
        _ => Subscription::none(),
    }
}

/// Validates the defaults form into the settings to send, or the first problem to show.
pub fn defaults_of(form: &DefaultsForm) -> Result<Defaults, String> {
    let retention = whole_in(
        &form.retention,
        (RETENTION_DAYS.0 as u64, RETENTION_DAYS.1 as u64),
    )
    .ok_or_else(|| {
        format!(
            "Keep recordings for whole days, from {} to {}.",
            RETENTION_DAYS.0, RETENTION_DAYS.1
        )
    })?;
    let fps = whole_in(&form.fps, (0, FPS_CAP as u64)).ok_or_else(|| {
        format!(
            "Frames per second is a whole number from 0 to {FPS_CAP}. 0 keeps the actions only."
        )
    })?;
    Ok(Defaults {
        record: form.record,
        retention_days: retention as u32,
        record_fps: fps as u8,
        record_takeovers: form.takeovers,
    })
}

fn whole_in(text: &str, (lo, hi): (u64, u64)) -> Option<u64> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u64 = t.parse().ok()?;
    (lo..=hi).contains(&n).then_some(n)
}

// ---- Timeline math (pure, tested below) ----

/// A recording's length in ms: from its start to its end, or to `now` while it is open.
pub fn duration_ms(rec: &Recording, now: u64) -> u64 {
    rec.ended_ms.unwrap_or(now).saturating_sub(rec.started_ms)
}

/// The frame (counting from 1) on screen at `pos` ms. Frame k is taken at `(k - 1) * duration / frames`, so the
/// frames are spread evenly over the recording; the last frame holds to the end.
pub fn frame_at(pos: u64, duration: u64, frames: u64) -> u64 {
    if frames == 0 {
        return 0;
    }
    let k = (u128::from(pos) * u128::from(frames) / u128::from(duration.max(1))) as u64 + 1;
    k.min(frames)
}

/// When frame `seq` (counting from 1) was taken, in ms from the start.
pub fn frame_time(seq: u64, duration: u64, frames: u64) -> u64 {
    if frames == 0 {
        return 0;
    }
    (u128::from(seq.saturating_sub(1)) * u128::from(duration) / u128::from(frames)) as u64
}

/// The playhead after `elapsed_ms` of playing at `speed`, stopped at the end.
pub fn advance(pos: u64, elapsed_ms: u64, speed: f32, duration: u64) -> u64 {
    let step = (elapsed_ms as f64 * f64::from(speed)).round() as u64;
    pos.saturating_add(step).min(duration)
}

/// The playhead a slider fraction (0 to 1) names.
pub fn at_fraction(fraction: f64, duration: u64) -> u64 {
    (fraction.clamp(0.0, 1.0) * duration as f64).round() as u64
}

/// A time as m:ss, or h:mm:ss past an hour.
pub fn clock(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// An action's kind in words: its first letter capitalised, its underscores spaced.
pub fn event_label(kind: &str) -> String {
    let words = kind.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Action".to_string(),
    }
}

// ---- Views ----

/// The station's recordings: its list, or the recording open on the page. A deletion is asked for in place.
pub fn station_view<'a>(state: &'a State, names: &'a HashMap<u64, String>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error.as_str()));
    }
    if let Some(open) = &state.open {
        parts.push(detail_view(state, open, names));
    } else {
        parts.push(list_view(state, names));
    }
    column(parts).spacing(12).width(Length::Fill).into()
}

fn list_view<'a>(state: &'a State, names: &'a HashMap<u64, String>) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.list {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the recordings.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "clock",
            "No recordings yet",
            "A lease on a station that records keeps its recording here.",
        ),
        Load::Ready(list) => table(
            &[
                (2, "Started"),
                (3, "Agent"),
                (2, "Length"),
                (2, "Frames"),
                (2, "Actions"),
                (3, ""),
            ],
            list.iter()
                .map(|rec| {
                    let agent = names
                        .get(&rec.agent)
                        .cloned()
                        .unwrap_or_else(|| format!("agent {}", rec.agent));
                    let mut tags: Vec<Element<'a, Msg>> =
                        vec![ui::link("Open", Msg::Open(rec.id.clone()), false)];
                    if rec.takeover {
                        tags.push(ui::badge("Takeover", ui::Tone::Warn));
                    }
                    if rec.ended_ms.is_none() {
                        tags.push(ui::badge("Open", ui::Tone::Info));
                    }
                    cells(vec![
                        (2, ui::faint(super::lab_common::ago(rec.started_ms)).into()),
                        (3, mono_text(agent)),
                        (2, mono_text(clock(duration_ms(rec, now_ms())))),
                        (2, mono_text(rec.frames.to_string())),
                        (2, mono_text(rec.actions.to_string())),
                        (3, row(tags).spacing(10).align_y(Alignment::Center).into()),
                    ])
                })
                .collect(),
        ),
    };
    column![body].into()
}

fn detail_view<'a>(
    state: &'a State,
    open: &'a Open,
    names: &'a HashMap<u64, String>,
) -> Element<'a, Msg> {
    let detail = match &open.detail {
        Load::Pending => return ui::loading(),
        Load::Failed => return ui::empty("alert", "Could not load this recording.", ""),
        Load::Ready(detail) => detail,
    };
    let rec = &detail.recording;
    let duration = length_of(detail);
    let fraction = if duration == 0 {
        0.0
    } else {
        open.pos as f64 / duration as f64
    };
    let seq = frame_at(open.pos, duration, rec.frames);
    let agent = names
        .get(&rec.agent)
        .cloned()
        .unwrap_or_else(|| format!("agent {}", rec.agent));

    let picture: Element<'a, Msg> = match open.frames.get(&seq) {
        Some(handle) => image(handle.clone())
            .content_fit(ContentFit::Contain)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        None => ui::empty(
            "clock",
            if rec.frames == 0 {
                "No frames"
            } else {
                "Loading the frame"
            },
            if rec.frames == 0 {
                "This recording kept its actions only."
            } else {
                ""
            },
        ),
    };
    let picture = container(picture)
        .width(Length::Fill)
        .height(Length::Fixed(360.0))
        .style(ui::frame(Some(theme::SUBSTRATE), theme::RULE));

    let play = ui::btn(
        if open.playing { "Pause" } else { "Play" },
        None,
        ui::Kind::Primary,
        (rec.frames > 0).then_some(Msg::TogglePlay),
    );
    let speed = pick_list(SPEEDS.to_vec(), Some(SpeedPick(open.speed)), |s| {
        Msg::Speed(s.0)
    })
    .style(theme::pick_list)
    .text_size(13.0)
    .width(Length::Fixed(96.0));
    let scrub = slider(0.0..=1.0, fraction, Msg::Scrub).step(0.001);
    let clock_line = format!(
        "{} / {} · frame {} of {}",
        clock(open.pos),
        clock(duration),
        seq,
        rec.frames
    );

    let mut events: Vec<Element<'a, Msg>> = Vec::new();
    for event in detail.events.iter().take(EVENT_ROWS) {
        let offset = event.at_ms.saturating_sub(rec.started_ms);
        events.push(cells(vec![
            (2, mono_text(clock(offset))),
            (4, mono_text(event_label(&event.kind))),
            (
                2,
                ui::faint(if event.ok { "done" } else { "failed" }).into(),
            ),
        ]));
        events.push(ui::hairline());
    }
    let events: Element<'a, Msg> = if events.is_empty() {
        ui::empty("clock", "No actions", "")
    } else {
        column(events).width(Length::Fill).into()
    };

    let mut chips: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!(
            "{} · {} actions · {}",
            agent,
            rec.actions,
            super::lab_common::ago(rec.started_ms)
        ))
        .into(),
    ];
    if rec.takeover {
        chips.push(ui::badge("Takeover", ui::Tone::Warn));
    }

    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::link("All recordings", Msg::Close, false),
        ui::chip_row(chips),
        picture.into(),
        row![play, speed, Element::from(scrub.width(Length::Fill))]
            .spacing(10)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .into(),
        ui::faint(clock_line).into(),
        ui::label("Actions").into(),
        events,
    ];
    if let Some(confirm) = &state.confirm {
        if *confirm == rec.id {
            parts.push(
                column![
                    ui::faint("Delete this recording? Its frames and actions go for good."),
                    row![
                        ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CancelDelete)),
                        ui::btn("Delete", None, ui::Kind::Danger, Some(Msg::Delete)),
                    ]
                    .spacing(8),
                ]
                .spacing(10)
                .into(),
            );
        }
    } else {
        parts.push(
            row![ui::btn(
                "Delete recording",
                None,
                ui::Kind::Danger,
                Some(Msg::AskDelete(rec.id.clone()))
            )]
            .into(),
        );
    }
    column(parts).spacing(12).width(Length::Fill).into()
}

/// The hub's recording defaults: the card on the Workstations page.
pub fn defaults_view(state: &State) -> Element<'_, Msg> {
    match &state.defaults {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the recording defaults.", ""),
        Load::Ready(_) => form_view(state),
    }
}

fn form_view(state: &State) -> Element<'_, Msg> {
    let f = &state.form;
    let record = checkbox(f.record)
        .label("Record lease sessions on stations that join from now on")
        .text_size(13.0)
        .on_toggle(|on| Msg::Edit(DefaultsEdit::Record(on)));
    let takeovers = checkbox(f.takeovers)
        .label("Record the owner's takeovers too")
        .text_size(13.0)
        .on_toggle(|on| Msg::Edit(DefaultsEdit::Takeovers(on)));
    let retention = text_input("", &f.retention)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .width(Length::Fixed(120.0))
        .on_input(|t| Msg::Edit(DefaultsEdit::Retention(t)));
    let fps = text_input("", &f.fps)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .width(Length::Fixed(120.0))
        .on_input(|t| Msg::Edit(DefaultsEdit::Fps(t)));
    let mut body: Vec<Element<'_, Msg>> = vec![
        row![record, takeovers]
            .spacing(24)
            .align_y(Alignment::Center)
            .into(),
        row![
            field(
                "Keep recordings for (days)",
                retention.into(),
                &format!(
                    "{} to {}. Older recordings are deleted.",
                    RETENTION_DAYS.0, RETENTION_DAYS.1
                ),
            ),
            field(
                "Frames per second",
                fps.into(),
                &format!("0 to {FPS_CAP}. 0 keeps the actions only."),
            ),
        ]
        .spacing(16)
        .into(),
        row![ui::btn(
            if state.saving {
                "Saving…"
            } else {
                "Save defaults"
            },
            None,
            ui::Kind::Primary,
            (!state.saving).then_some(Msg::SaveDefaults),
        )]
        .into(),
    ];
    if let Some(error) = &state.form_error {
        body.push(ui::error_line(error.as_str()));
    }
    if let Some(notice) = &state.notice {
        body.push(ui::faint(notice.as_str()).into());
    }
    column(body).spacing(14).width(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(started: u64, ended: Option<u64>, frames: u64) -> Recording {
        Recording {
            started_ms: started,
            ended_ms: ended,
            frames,
            ..Default::default()
        }
    }

    #[test]
    fn a_recordings_length_runs_to_its_end_or_to_now_while_open() {
        assert_eq!(duration_ms(&rec(1_000, Some(6_000), 5), 99_000), 5_000);
        assert_eq!(
            duration_ms(&rec(1_000, None, 5), 4_000),
            3_000,
            "an open one runs to now"
        );
        assert_eq!(
            duration_ms(&rec(9_000, Some(1_000), 5), 0),
            0,
            "a bad clock never goes negative"
        );
    }

    #[test]
    fn frames_are_spread_evenly_and_the_playhead_finds_its_frame() {
        // Ten frames over ten seconds: frame k is at (k - 1) seconds.
        assert_eq!(frame_at(0, 10_000, 10), 1);
        assert_eq!(frame_at(999, 10_000, 10), 1);
        assert_eq!(frame_at(1_000, 10_000, 10), 2);
        assert_eq!(frame_at(9_999, 10_000, 10), 10);
        assert_eq!(
            frame_at(10_000, 10_000, 10),
            10,
            "the end holds the last frame"
        );
        assert_eq!(frame_time(1, 10_000, 10), 0);
        assert_eq!(frame_time(4, 10_000, 10), 3_000);
        assert_eq!(
            frame_at(frame_time(7, 10_000, 10), 10_000, 10),
            7,
            "a frame's own time is that frame"
        );
        assert_eq!(frame_at(500, 10_000, 0), 0, "no frames, no frame");
    }

    #[test]
    fn playback_advances_by_the_speed_and_stops_at_the_end() {
        assert_eq!(advance(0, 100, 1.0, 10_000), 100);
        assert_eq!(advance(0, 100, 4.0, 10_000), 400);
        assert_eq!(advance(0, 100, 0.5, 10_000), 50);
        assert_eq!(advance(9_950, 100, 1.0, 10_000), 10_000, "stops at the end");
    }

    #[test]
    fn a_slider_fraction_names_a_playhead_and_is_clamped() {
        assert_eq!(at_fraction(0.25, 10_000), 2_500);
        assert_eq!(at_fraction(1.7, 10_000), 10_000);
        assert_eq!(at_fraction(-0.5, 10_000), 0);
    }

    #[test]
    fn clocks_and_action_labels_read_as_people_write_them() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(75_900), "1:15");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(event_label("lease_ended"), "Lease ended");
        assert_eq!(event_label(""), "Action");
    }

    #[test]
    fn the_defaults_form_checks_each_bound_and_keeps_the_switches() {
        let form = DefaultsForm {
            record: false,
            retention: "14".into(),
            fps: "5".into(),
            takeovers: true,
        };
        assert_eq!(
            defaults_of(&form),
            Ok(Defaults {
                record: false,
                retention_days: 14,
                record_fps: 5,
                record_takeovers: true
            })
        );
        let too_fast = DefaultsForm {
            fps: "6".into(),
            ..form.clone()
        };
        assert!(
            defaults_of(&too_fast).is_err(),
            "the hub caps frames at five a second"
        );
        let no_days = DefaultsForm {
            retention: "".into(),
            ..form.clone()
        };
        assert!(defaults_of(&no_days).is_err());
        assert_eq!(DefaultsForm::from(&Defaults::default()).fps, "2");
    }

    #[test]
    fn a_recording_parses_with_its_hub_keys_and_tolerates_a_partial_one() {
        let full: Recording = serde_json::from_value(serde_json::json!({
            "id": "r1-1", "station": "w1", "lease": "l2", "agent": 7, "task": "t9",
            "started_ms": 10, "ended_ms": 20, "frames": 3, "actions": 4, "bytes": 99,
            "takeover": true, "width": 1920, "height": 1080
        }))
        .expect("a full recording parses");
        assert!(full.takeover);
        assert_eq!(full.width, Some(1920));
        let partial: Recording =
            serde_json::from_value(serde_json::json!({"id": "r2"})).expect("a partial one too");
        assert_eq!(partial.ended_ms, None);
        assert_eq!(partial.frames, 0);
    }
}
