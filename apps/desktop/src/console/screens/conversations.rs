//! The Conversations screen (parity with the web console's `conversations` view).
//!
//! Agent-to-agent threads grouped by the pair of agents: a list of pairs (newest first) beside the
//! selected thread, one readable column (oldest message at the top, the newest at the bottom). The
//! list and the thread page back through `next_before`; the search `q` narrows both. The header
//! carries the retention setting (how many exchanges to keep, and after how many days to delete).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iced::widget::operation::snap_to;
use iced::widget::{button, column, container, row, scrollable, text_input, Id as WidgetId, Space};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::{MONO};

/// The thread's scrollable, so a newly opened thread can start at its newest message.
const THREAD: &str = "conversations-thread";
/// Messages per page, for the list and for a thread.
const PAGE: u64 = 50;
/// How long the search waits for typing to stop.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// The retention bounds the hub accepts.
const KEEP_MIN: u64 = 100;
const KEEP_MAX: u64 = 100_000;
const DAYS_MAX: u64 = 3650;

#[derive(Debug, Default)]
pub struct State {
    /// The last load's failure, if it failed.
    pub error: Option<String>,
    loaded: bool,
    /// The messages loaded so far, newest first.
    conversations: Vec<Conversation>,
    /// The cursor for the next older page of the list; `None` when the list reaches the start.
    older: Option<u64>,
    busy_older: bool,
    list_error: Option<String>,
    /// `(cid, display name)` from `GET /v1/agents`.
    agents: Vec<(u64, String)>,
    /// The key (`a-b`, ordered) of the pair the reader picked; the newest pair when unset.
    selected: Option<String>,
    /// The key of the thread that is shown, to tell a newly opened thread from a redraw.
    shown: Option<String>,
    thread: Option<Thread>,
    thread_error: Option<String>,
    /// The search as typed, the query the list shows, and the keystroke a debounce belongs to.
    draft: String,
    query: String,
    seq: u64,
    retention: Retention,
    /// A live refresh is in flight; an event that came meanwhile runs once more when it lands.
    refreshing: bool,
    again: bool,
}

/// The selected pair's messages, newest first, with the cursor for the next older page.
#[derive(Debug, Clone)]
struct Thread {
    key: String,
    a: u64,
    b: u64,
    messages: Vec<Conversation>,
    older: Option<u64>,
    busy: bool,
}

/// The retention row: the values as typed, whether the reader has typed (a poll keeps the draft),
/// and the last save's outcome.
#[derive(Debug, Default)]
struct Retention {
    keep: String,
    days: String,
    dirty: bool,
    busy: bool,
    note: Option<Result<String, String>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// Open the thread with this pair key.
    Select(String),
    /// The search text changed.
    Search(String),
    /// The debounce for this keystroke ran out.
    Debounced(u64),
    /// The first page of the list for `q`, the agents, and the settings (`Null` when not fetched),
    /// or the failure.
    Loaded { q: String, result: Result<(Value, Value, Value), String> },
    /// The selected thread's first page for `q` (or, with `earlier`, the page before it).
    ThreadLoaded { key: String, q: String, earlier: bool, result: Result<Value, String> },
    /// Load the next older page of the list.
    LoadOlder,
    /// The next older page of the list, for `q`, or the failure.
    OlderLoaded { q: String, result: Result<Value, String> },
    /// Load the selected thread's earlier messages.
    LoadEarlier,
    /// The retention fields were edited.
    KeepIn(String),
    DaysIn(String),
    /// Save the retention; the result is the saved `(keep, days)`, or the failure.
    SaveRetention,
    RetentionSaved(Result<(u64, u64), String>),
    /// A hub event: a conversation or a settings change reloads the screen.
    Event(crate::console::events::Event),
}

/// One message from `GET /v1/conversations`.
#[derive(Debug, Clone)]
struct Conversation {
    from_cid: u64,
    to_cid: u64,
    text: String,
    created_at: i64,
    delivered: bool,
}

/// The messages between two agents, newest first.
#[derive(Debug, Clone)]
struct Pair {
    key: String,
    a: u64,
    b: u64,
    messages: Vec<Conversation>,
    last: Conversation,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the first page of the list, the agents and the settings, and the selected thread (called
/// when the screen opens and on every poll tick). The settings are not re-read while the reader has
/// typed a retention value, so the draft is kept.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let q = state.query.clone();
    let thread = current_key(state).map(|key| thread_page(ctx, key, q.clone(), None, false));
    let want_settings = !state.retention.dirty;
    let path = list_path(None, &q);
    let client = ctx.client.clone();
    let head = client::task(
        move || {
            let conversations = client.get::<Value>(&path)?;
            // The agents only name the threads: without them the list still shows, with ids.
            let agents = client.get::<Value>("/v1/agents").unwrap_or(Value::Null);
            let settings = if want_settings { client.get::<Value>("/v1/settings").unwrap_or(Value::Null) } else { Value::Null };
            Ok((conversations, agents, settings))
        },
        move |result| Msg::Loaded { q: q.clone(), result },
    );
    Task::batch(thread.into_iter().chain(std::iter::once(head)))
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let landed = matches!(msg, Msg::Loaded { .. });
    let task = apply(state, msg, ctx);
    if !landed {
        return task;
    }
    state.refreshing = false;
    if std::mem::take(&mut state.again) {
        state.refreshing = true;
        return Task::batch([task, load(state, ctx)]);
    }
    task
}

/// The screen's messages. `update` wraps it to keep the live refresh's in-flight flag.
fn apply(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "conversation" | "settings") {
                return Task::none();
            }
            if state.refreshing {
                state.again = true;
                Task::none()
            } else {
                state.refreshing = true;
                load(state, ctx)
            }
        }
        Msg::Go(_) => Task::none(),
        Msg::Select(key) => {
            state.selected = Some(key);
            match missing_thread(state) {
                Some(key) => {
                    state.thread = None;
                    state.thread_error = None;
                    thread_page(ctx, key, state.query.clone(), None, false)
                }
                None => Task::none(),
            }
        }
        Msg::Search(text) => {
            state.draft = text;
            state.seq += 1;
            let seq = state.seq;
            client::task(
                move || {
                    std::thread::sleep(DEBOUNCE);
                    Ok::<(), String>(())
                },
                move |_| Msg::Debounced(seq),
            )
        }
        Msg::Debounced(seq) => {
            let q = state.draft.trim().to_string();
            if seq != state.seq || q == state.query {
                return Task::none();
            }
            // A new query starts the list and the thread again from their first pages.
            state.query = q;
            state.conversations.clear();
            state.older = None;
            state.busy_older = false;
            state.list_error = None;
            state.thread = None;
            state.thread_error = None;
            state.shown = None;
            load(state, ctx)
        }
        Msg::Loaded { q, result } => {
            if q != state.query {
                return Task::none();
            }
            state.loaded = true;
            match result {
                Ok((conversations, agents, settings)) => {
                    state.error = None;
                    let (list, older) = merge_head(
                        parse_conversations(&conversations),
                        next_of(&conversations),
                        std::mem::take(&mut state.conversations),
                        state.older,
                    );
                    state.conversations = list;
                    state.older = older;
                    if !agents.is_null() {
                        state.agents = parse_agents(&agents);
                    }
                    if !state.retention.dirty {
                        if let Some((keep, days)) = retention_fields(&settings) {
                            state.retention.keep = keep;
                            state.retention.days = days;
                        }
                    }
                }
                Err(err) => state.error = Some(err),
            }
            match missing_thread(state) {
                Some(key) => thread_page(ctx, key, state.query.clone(), None, false),
                None => Task::none(),
            }
        }
        Msg::ThreadLoaded { key, q, earlier, result } => {
            if q != state.query || current_key(state).as_deref() != Some(key.as_str()) {
                return Task::none();
            }
            let Some((a, b)) = parse_key(&key) else { return Task::none() };
            match result {
                Ok(v) => {
                    state.thread_error = None;
                    let page = parse_thread(&v);
                    let next = next_of(&v);
                    if earlier {
                        if let Some(t) = state.thread.as_mut().filter(|t| t.key == key) {
                            t.messages.extend(page);
                            t.older = next;
                            t.busy = false;
                        }
                        Task::none()
                    } else {
                        let (old, cursor) = match state.thread.take() {
                            Some(t) if t.key == key => (t.messages, t.older),
                            _ => (Vec::new(), None),
                        };
                        let (messages, older) = merge_head(page, next, old, cursor);
                        state.thread = Some(Thread { key: key.clone(), a, b, messages, older, busy: false });
                        // A newly opened thread starts at its newest message; a redraw keeps the reader's place.
                        if state.shown.as_deref() != Some(key.as_str()) {
                            state.shown = Some(key);
                            snap_to(WidgetId::new(THREAD), scrollable::RelativeOffset::END)
                        } else {
                            Task::none()
                        }
                    }
                }
                Err(err) => {
                    if let Some(t) = state.thread.as_mut().filter(|t| t.key == key) {
                        t.busy = false;
                    }
                    state.thread_error = Some(err);
                    Task::none()
                }
            }
        }
        Msg::LoadOlder => match state.older {
            Some(before) if !state.busy_older => {
                state.busy_older = true;
                list_page(ctx, before, state.query.clone())
            }
            _ => Task::none(),
        },
        Msg::OlderLoaded { q, result } => {
            if q != state.query {
                return Task::none();
            }
            state.busy_older = false;
            match result {
                Ok(v) => {
                    state.list_error = None;
                    state.conversations.extend(parse_conversations(&v));
                    state.older = next_of(&v);
                }
                Err(err) => state.list_error = Some(err),
            }
            Task::none()
        }
        Msg::LoadEarlier => {
            let query = state.query.clone();
            match state.thread.as_mut() {
                Some(t) if !t.busy => match t.older {
                    Some(before) => {
                        t.busy = true;
                        thread_page(ctx, t.key.clone(), query, Some(before), true)
                    }
                    None => Task::none(),
                },
                _ => Task::none(),
            }
        }
        Msg::KeepIn(text) => {
            state.retention.keep = text;
            state.retention.dirty = true;
            state.retention.note = None;
            Task::none()
        }
        Msg::DaysIn(text) => {
            state.retention.days = text;
            state.retention.dirty = true;
            state.retention.note = None;
            Task::none()
        }
        Msg::SaveRetention => {
            let (keep, days) = match validate_retention(&state.retention.keep, &state.retention.days) {
                Ok(v) => v,
                Err(err) => {
                    state.retention.note = Some(Err(err));
                    return Task::none();
                }
            };
            state.retention.busy = true;
            state.retention.note = None;
            // Read the settings fresh, change only the two conversation fields, and write them back.
            let client = ctx.client.clone();
            client::task(
                move || {
                    let settings = client.get::<Value>("/v1/settings")?;
                    let body = with_retention(settings, keep, days)?;
                    client.call("PUT", "/v1/settings", Some(&body)).map(|_| (keep, days))
                },
                Msg::RetentionSaved,
            )
        }
        Msg::RetentionSaved(result) => {
            state.retention.busy = false;
            match result {
                Ok((keep, days)) => {
                    state.retention.keep = keep.to_string();
                    state.retention.days = days.to_string();
                    state.retention.dirty = false;
                    state.retention.note = Some(Ok("Saved.".to_string()));
                }
                Err(err) => state.retention.note = Some(Err(err)),
            }
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let pairs = group_pairs(&state.conversations);
    let current = current_index(&pairs, state.selected.as_deref());

    let search = text_input("Search messages", &state.draft)
        .on_input(Msg::Search)
        .font(MONO)
        .size(12.0)
        .padding(8)
        .style(crate::theme::input);
    let clear = ui::btn("Clear", None, ui::Kind::Ghost, (!state.draft.is_empty()).then(|| Msg::Search(String::new())));
    let search: Element<'a, Msg> = row![search, clear].spacing(6).align_y(Alignment::Center).into();

    let list: Element<'a, Msg> = if !state.loaded {
        ui::loading()
    } else if let Some(err) = &state.error {
        column![ui::empty("alert", "Could not load conversations.", ""), ui::error_line(err)].into()
    } else if pairs.is_empty() {
        let title = if state.query.is_empty() { "No conversations yet." } else { "No conversations match." };
        ui::empty("conversations", title, "")
    } else {
        column(pairs.iter().enumerate().map(|(i, p)| pair_item(p, i == current, &state.agents)))
            .width(Length::Fill)
            .into()
    };
    let mut rows: Vec<Element<'a, Msg>> = vec![list];
    if let Some(err) = &state.list_error {
        rows.push(ui::error_line(err));
    }
    if state.older.is_some() {
        rows.push(page_button("Load older", state.busy_older, Msg::LoadOlder));
    }
    let list_col = container(
        column![search, scrollable(column(rows).width(Length::Fill)).height(Length::Fill)]
            .spacing(10)
            .height(Length::Fill),
    )
    .width(Length::Fixed(340.0))
    .height(Length::Fill);

    let shown = pairs.get(current).and_then(|p| state.thread.as_ref().filter(|t| t.key == p.key));
    let thread: Element<'a, Msg> = match (pairs.get(current), shown) {
        (None, _) => Space::new().width(Length::Fill).into(),
        (Some(_), Some(t)) => thread_view(t, &state.agents, state.thread_error.as_deref()),
        (Some(_), None) => match &state.thread_error {
            Some(err) => ui::error_line(err),
            None => ui::loading(),
        },
    };

    let divider = container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill).style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().rule.into()),
        ..Default::default()
    });

    let body: Element<'a, Msg> = row![list_col, divider, thread].height(Length::Fixed(640.0)).into();
    let head: Element<'a, Msg> = column![ui::view_head("Conversations", vec![]), retention_row(&state.retention)].spacing(12).into();
    ui::sheet(head, body)
}

/// The retention row under the title: "Keep the newest [n] exchanges · delete after [d] days (0 = never)".
fn retention_row<'a>(r: &'a Retention) -> Element<'a, Msg> {
    let note: Element<'a, Msg> = match &r.note {
        Some(Ok(text)) => ui::faint(text.as_str()).into(),
        Some(Err(text)) => ui::error_line(text),
        None => Space::new().into(),
    };
    row![
        ui::faint("Keep the newest"),
        retention_field(&r.keep, "100000", 90.0, Msg::KeepIn),
        ui::faint("exchanges · delete after"),
        retention_field(&r.days, "0", 64.0, Msg::DaysIn),
        ui::faint("days (0 = never)"),
        ui::btn(if r.busy { "Saving…" } else { "Save" }, None, ui::Kind::Secondary, (!r.busy).then_some(Msg::SaveRetention)),
        note,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn retention_field<'a>(value: &'a str, placeholder: &'a str, width: f32, on_input: fn(String) -> Msg) -> Element<'a, Msg> {
    text_input(placeholder, value)
        .on_input(on_input)
        .font(MONO)
        .size(12.0)
        .padding(6)
        .width(Length::Fixed(width))
        .style(crate::theme::input)
        .into()
}

/// The index of the selected pair, or the newest (0) when none is selected or it is gone.
fn current_index(pairs: &[Pair], selected: Option<&str>) -> usize {
    pairs.iter().position(|p| Some(p.key.as_str()) == selected).unwrap_or(0)
}

/// The key of the pair on screen: the selected one, else the newest.
fn current_key(state: &State) -> Option<String> {
    let pairs = group_pairs(&state.conversations);
    pairs.get(current_index(&pairs, state.selected.as_deref())).map(|p| p.key.clone())
}

/// The key of the pair on screen when its thread is not loaded yet.
fn missing_thread(state: &State) -> Option<String> {
    current_key(state).filter(|key| state.thread.as_ref().map(|t| t.key.as_str()) != Some(key.as_str()))
}

/// Groups the messages by the unordered pair of agents; pairs newest first.
fn group_pairs(list: &[Conversation]) -> Vec<Pair> {
    let mut oldest_first: Vec<&Conversation> = list.iter().collect();
    oldest_first.sort_by_key(|c| c.created_at);
    let mut pairs: Vec<Pair> = Vec::new();
    let mut index: HashMap<(u64, u64), usize> = HashMap::new();
    for c in oldest_first {
        let (a, b) = (c.from_cid.min(c.to_cid), c.from_cid.max(c.to_cid));
        let i = *index.entry((a, b)).or_insert_with(|| {
            pairs.push(Pair { key: format!("{a}-{b}"), a, b, messages: Vec::new(), last: c.clone() });
            pairs.len() - 1
        });
        pairs[i].messages.push(c.clone());
        pairs[i].last = c.clone();
    }
    pairs.sort_by(|x, y| y.last.created_at.cmp(&x.last.created_at));
    pairs
}

fn label(agents: &[(u64, String)], cid: u64) -> String {
    agents.iter().find(|(id, _)| *id == cid).map(|(_, name)| name.clone()).unwrap_or_else(|| format!("agent {cid}"))
}

fn name_link<'a>(cid: u64, agents: &[(u64, String)]) -> Element<'a, Msg> {
    ui::btn(&label(agents, cid), None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(cid))))
}

fn pair_item<'a>(p: &Pair, active: bool, agents: &[(u64, String)]) -> Element<'a, Msg> {
    let names = format!("{} · {}", label(agents, p.a), label(agents, p.b));
    let meta = format!("{} · {} messages", ago(p.last.created_at, now_ms()), p.messages.len());
    let (background, edge) = if active { (crate::theme::palette().paper, crate::theme::palette().signal) } else { (crate::theme::palette().sheet, crate::theme::palette().rule) };
    button(column![ui::mono(names, 12.0, crate::theme::palette().ink), ui::prose(clip(&p.last.text, 90)), ui::faint(meta)].spacing(4))
        .width(Length::Fill)
        .padding(12)
        .on_press(Msg::Select(p.key.clone()))
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: Some(background.into()),
            text_color: crate::theme::palette().ink,
            border: Border { color: edge, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The open thread: its pair as the head, the earlier-messages button, then every message as one
/// readable column (speaker and time in mono, the text in serif).
fn thread_view<'a>(t: &Thread, agents: &[(u64, String)], error: Option<&str>) -> Element<'a, Msg> {
    let head = row![name_link(t.a, agents), ui::faint(" · "), name_link(t.b, agents)].spacing(6).align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![head.into(), ui::hairline()];
    if let Some(err) = error {
        items.push(ui::error_line(err));
    }
    if t.older.is_some() {
        items.push(page_button("Load earlier messages", t.busy, Msg::LoadEarlier));
    }
    // The messages are held newest first; the thread reads oldest first.
    items.extend(t.messages.iter().rev().map(|m| message(m, agents)));
    scrollable(column(items).spacing(18).padding(16).max_width(720.0).width(Length::Fill))
        .id(WidgetId::new(THREAD))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn message<'a>(m: &Conversation, agents: &[(u64, String)]) -> Element<'a, Msg> {
    let mut meta = row![name_link(m.from_cid, agents), ui::mono(ago(m.created_at, now_ms()), 11.0, crate::theme::palette().ink_3)]
        .spacing(10)
        .align_y(Alignment::Center);
    if !m.delivered {
        meta = meta.push(ui::badge("undelivered", ui::Tone::Warn));
    }
    column![meta, ui::markdown(&m.text)].spacing(6).width(Length::Fill).into()
}

/// A mono button for paging; it reads "Loading…" and is inert while its page is in flight.
fn page_button<'a>(label: &str, busy: bool, msg: Msg) -> Element<'a, Msg> {
    let text = if busy { "Loading…" } else { label };
    ui::btn(text, None, ui::Kind::Secondary, (!busy).then_some(msg))
}

fn thread_page(ctx: &Ctx, key: String, q: String, before: Option<u64>, earlier: bool) -> Task<Msg> {
    let Some((a, b)) = parse_key(&key) else { return Task::none() };
    let client = ctx.client.clone();
    let path = thread_path(a, b, before, &q);
    client::task(
        move || client.get::<Value>(&path),
        move |result| Msg::ThreadLoaded { key: key.clone(), q: q.clone(), earlier, result },
    )
}

fn list_page(ctx: &Ctx, before: u64, q: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = list_path(Some(before), &q);
    client::task(move || client.get::<Value>(&path), move |result| Msg::OlderLoaded { q: q.clone(), result })
}

/// The query string for one page: the page size, the `before` cursor when paging back, and the search.
fn page_query(before: Option<u64>, q: &str) -> String {
    let mut parts = vec![format!("limit={PAGE}")];
    if let Some(before) = before {
        parts.push(format!("before={before}"));
    }
    if !q.is_empty() {
        parts.push(format!("q={}", encode(q)));
    }
    parts.join("&")
}

fn list_path(before: Option<u64>, q: &str) -> String {
    format!("/v1/conversations?{}", page_query(before, q))
}

fn thread_path(a: u64, b: u64, before: Option<u64>, q: &str) -> String {
    format!("/v1/conversations/{a}/{b}?{}", page_query(before, q))
}

/// Percent-encodes everything but the unreserved characters, so a search reaches the hub intact.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A pair key (`a-b`) as its two agent ids.
fn parse_key(key: &str) -> Option<(u64, u64)> {
    let (a, b) = key.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Checks the retention fields: keep 100..=100000 exchanges, delete after 0..=3650 days (0 = never).
fn validate_retention(keep: &str, days: &str) -> Result<(u64, u64), String> {
    let keep: u64 = keep.trim().parse().map_err(|_| "Keep a whole number of exchanges.".to_string())?;
    if !(KEEP_MIN..=KEEP_MAX).contains(&keep) {
        return Err(format!("Keep between {KEEP_MIN} and {KEEP_MAX} exchanges."));
    }
    let days: u64 = days.trim().parse().map_err(|_| "Delete after a whole number of days (0 = never).".to_string())?;
    if days > DAYS_MAX {
        return Err(format!("Delete after at most {DAYS_MAX} days (0 = never)."));
    }
    Ok((keep, days))
}

/// The settings with the two conversation fields of `retention` set; the other fields stay as read.
fn with_retention(mut settings: Value, keep: u64, days: u64) -> Result<Value, String> {
    let root = settings.as_object_mut().ok_or("Settings did not load.")?;
    let retention = root.entry("retention").or_insert_with(|| json!({}));
    let fields = retention.as_object_mut().ok_or("Retention is not a settings object.")?;
    fields.insert("conversations_max".to_string(), json!(keep));
    fields.insert("conversations_days".to_string(), json!(days));
    Ok(settings)
}

/// The conversation retention in the settings, as the form's text; `None` when the hub has none.
fn retention_fields(settings: &Value) -> Option<(String, String)> {
    let r = settings.get("retention")?;
    let keep = r.get("conversations_max")?.as_u64()?;
    let days = r.get("conversations_days")?.as_u64()?;
    Some((keep.to_string(), days.to_string()))
}

/// The cursor for the next older page, if the hub gave one.
fn next_of(v: &Value) -> Option<u64> {
    v.get("next_before").and_then(Value::as_u64)
}

/// Merges a freshly loaded newest page into what is already loaded (both newest first). Loaded
/// messages older than the page are kept, so paging back survives a poll; the cursor is the one
/// for the oldest loaded message when older pages were kept, else the page's own.
fn merge_head(page: Vec<Conversation>, next: Option<u64>, loaded: Vec<Conversation>, loaded_cursor: Option<u64>) -> (Vec<Conversation>, Option<u64>) {
    if next.is_none() {
        return (page, None);
    }
    let floor = page.iter().map(|c| c.created_at).min().unwrap_or(i64::MAX);
    let kept: Vec<Conversation> = loaded.into_iter().filter(|c| c.created_at < floor).collect();
    let cursor = if kept.is_empty() { next } else { loaded_cursor };
    let mut all = page;
    all.extend(kept);
    (all, cursor)
}

/// Parses `GET /v1/conversations` (a bare array, or `{ "conversations": [...] }`), newest first.
fn parse_conversations(v: &Value) -> Vec<Conversation> {
    parse_messages(items(v, "conversations"))
}

/// Parses `GET /v1/conversations/{a}/{b}`: the page is oldest first, so it is reversed to newest first.
fn parse_thread(v: &Value) -> Vec<Conversation> {
    let mut messages = parse_messages(items(v, "messages"));
    messages.reverse();
    messages
}

fn parse_messages(list: &[Value]) -> Vec<Conversation> {
    list.iter()
        .filter_map(|c| {
            let from_cid = c.get("from_cid").and_then(Value::as_u64)?;
            let to_cid = c.get("to_cid").and_then(Value::as_u64)?;
            Some(Conversation {
                from_cid,
                to_cid,
                text: c.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                created_at: c.get("created_at").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                delivered: c.get("delivered").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// Parses `GET /v1/agents` into `(cid, name)`; an agent without a name is "Unnamed", as on the web.
fn parse_agents(v: &Value) -> Vec<(u64, String)> {
    items(v, "agents")
        .iter()
        .filter_map(|a| {
            let cid = a.get("cid").and_then(Value::as_u64)?;
            let raw = a.get("name").and_then(Value::as_str).unwrap_or("");
            let name = if raw.trim().is_empty() { "Unnamed".to_string() } else { raw.to_string() };
            Some((cid, name))
        })
        .collect()
}

/// The array under `key`, or the value itself when it is a bare array.
fn items<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.as_array().or_else(|| v.get(key).and_then(Value::as_array)).map(Vec::as_slice).unwrap_or(&[])
}

/// The web's `ago`: the age of a millisecond timestamp, as "just now", "5m ago", "3d ago".
fn ago(ms: i64, now: i64) -> String {
    if ms == 0 {
        return "\u{2014}".to_string();
    }
    let s = ((now - ms) as f64 / 1000.0).round().max(0.0) as i64;
    match s {
        s if s < 5 => "just now".to_string(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// Clips to `max` characters, ending with an ellipsis, as the web's `clip`.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}\u{2026}", text.chars().take(max.saturating_sub(1)).collect::<String>())
    } else {
        text.to_string()
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(created_at: i64) -> Conversation {
        Conversation { from_cid: 1, to_cid: 2, text: String::new(), created_at, delivered: true }
    }

    fn times(list: &[Conversation]) -> Vec<i64> {
        list.iter().map(|c| c.created_at).collect()
    }

    #[test]
    fn conversations_query_strings_page_and_search() {
        assert_eq!(list_path(None, ""), "/v1/conversations?limit=50");
        assert_eq!(list_path(Some(42), "a b&c"), "/v1/conversations?limit=50&before=42&q=a%20b%26c");
        assert_eq!(thread_path(207, 208, None, ""), "/v1/conversations/207/208?limit=50");
        assert_eq!(thread_path(207, 208, Some(7), "é"), "/v1/conversations/207/208?limit=50&before=7&q=%C3%A9");
    }

    #[test]
    fn conversations_pair_keys_parse_as_ids() {
        assert_eq!(parse_key("207-208"), Some((207, 208)));
        assert_eq!(parse_key("207"), None);
        assert_eq!(parse_key("x-1"), None);
    }

    #[test]
    fn conversations_retention_is_validated_in_range() {
        assert_eq!(validate_retention(" 100 ", "0"), Ok((100, 0)));
        assert_eq!(validate_retention("100000", "3650"), Ok((100_000, 3650)));
        assert!(validate_retention("99", "0").is_err());
        assert!(validate_retention("100001", "0").is_err());
        assert!(validate_retention("abc", "0").is_err());
        assert!(validate_retention("100", "-1").is_err());
        assert!(validate_retention("100", "3651").is_err());
    }

    #[test]
    fn conversations_retention_write_keeps_the_other_fields() {
        let settings = json!({
            "default_model": "x",
            "retention": {"posts_max": 5, "posts_days": 9, "conversations_max": 1000, "conversations_days": 0}
        });
        let out = with_retention(settings, 200, 30).unwrap();
        assert_eq!(out["default_model"], "x");
        assert_eq!(out["retention"], json!({"posts_max": 5, "posts_days": 9, "conversations_max": 200, "conversations_days": 30}));
        assert_eq!(with_retention(json!({}), 100, 0).unwrap()["retention"]["conversations_max"], 100);
        assert!(with_retention(json!([]), 100, 0).is_err());
    }

    #[test]
    fn conversations_retention_reads_back_as_text() {
        let settings = json!({"retention": {"conversations_max": 1000, "conversations_days": 0}});
        assert_eq!(retention_fields(&settings), Some(("1000".to_string(), "0".to_string())));
        assert_eq!(retention_fields(&json!({})), None);
    }

    #[test]
    fn conversations_head_merge_keeps_paged_back_messages() {
        // The page is newer than what is loaded: the loaded messages below it stay, with their cursor.
        let (all, cursor) = merge_head(vec![msg(12), msg(11)], Some(7), vec![msg(10), msg(9), msg(8)], Some(3));
        assert_eq!(times(&all), vec![12, 11, 10, 9, 8]);
        assert_eq!(cursor, Some(3));

        // Nothing below the page was loaded: the page's own cursor is the one to page from.
        let (all, cursor) = merge_head(vec![msg(12), msg(11)], Some(7), vec![msg(12)], Some(3));
        assert_eq!(times(&all), vec![12, 11]);
        assert_eq!(cursor, Some(7));

        // The hub has no older messages: the page is the whole list.
        let (all, cursor) = merge_head(vec![msg(12)], None, vec![msg(10), msg(9)], Some(3));
        assert_eq!(times(&all), vec![12]);
        assert_eq!(cursor, None);
    }

    #[test]
    fn conversations_thread_page_reads_newest_first() {
        let page = json!({"messages": [
            {"from_cid": 1, "to_cid": 2, "text": "a", "created_at": 1, "delivered": true},
            {"from_cid": 2, "to_cid": 1, "text": "b", "created_at": 2, "delivered": false}
        ], "next_before": 1});
        assert_eq!(times(&parse_thread(&page)), vec![2, 1]);
        assert_eq!(next_of(&page), Some(1));
        assert_eq!(next_of(&json!({"messages": []})), None);
    }
}

pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    iced::Subscription::batch([
        crate::console::events::subscription(&ctx.client).map(Msg::Event),
        iced::event::listen_with(escape),
    ])
}

/// Escape clears the search, as the web's search box does.
fn escape(event: iced::Event, _: iced::event::Status, _: iced::window::Id) -> Option<Msg> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        }) => Some(Msg::Search(String::new())),
        _ => None,
    }
}
