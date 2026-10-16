//! One agent's page (parity with the web console's agent detail): the back link and actions, the
//! profile card, the transcript and chat (or the memory library), and the side column: model,
//! harness, permissions (`agent_side`), then details and queue. Restart, wipe and retire ask first;
//! cancel and stop act at once.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::keyboard;
use iced::widget::operation::snap_to;
use iced::widget::{
    Space, button, checkbox, column, container, row, scrollable, text, text_editor, text_input,
};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Id, agent_side, ide, lab_common, nodes, sanctions};
use crate::console::engine::world::layout::{agent_hue, role_hue};
use crate::console::rich;
use crate::console::{Ctx, client, events, ui};
use crate::theme::SANS;

/// The profile and infractions refresh at most this often; the transcript on every poll.
const EXTRAS_EVERY: Duration = Duration::from_secs(15);
const SIDE_WIDTH: f32 = 320.0;
const TRANSCRIPT_HEIGHT: f32 = 480.0;
/// The portrait's square, and the infractions shown before "Show all".
const PORTRAIT: f32 = 168.0;
const INFRACTIONS_SHOWN: usize = 10;
/// The transcript follows new text while the reader is within this many pixels of its bottom (the web's NEAR_BOTTOM_PX).
const NEAR_BOTTOM_PX: f32 = 80.0;
const TRANSCRIPT_ID: &str = "agent-transcript";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Transcript,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Restart,
    Wipe {
        memory: bool,
        transcript: bool,
        identity: bool,
    },
    Retire,
    /// Lift the suspension with this sanction id; the reason is typed in the card.
    Lift(u64),
    /// Roll the VM back to this checkpoint (by name). A safety checkpoint is taken first.
    Restore(String),
    /// Delete this checkpoint (by name).
    Delete(String),
    /// Fork the agent into the count typed in the card (`State::fork_n`).
    Fork,
    /// Move the agent to the node picked in the card (`State::move_to`).
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wipe {
    Memory,
    Transcript,
    Identity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Cancel,
    Stop,
    Sleep,
    Wake,
}

/// What a move did (`POST /v1/agents/{cid}/move`): the node it moved the agent from and to, the bytes it sent, and how
/// long it took.
#[derive(Debug, Clone, Deserialize)]
pub struct Moved {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub ms: u64,
}

/// One VM checkpoint, as `GET /v1/agents/{cid}/vm-checkpoints` lists it (apid's `SnapshotView`).
/// The `sleep` snapshot is listed too, with an empty label.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Checkpoint {
    pub name: String,
    #[serde(default)]
    pub label: String,
    /// `full` or `diff`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub taken_at_ms: u64,
    #[serde(default)]
    pub memory_written: u64,
    #[serde(default)]
    pub disk_bytes: u64,
    /// How long the VM was paused for the snapshot, in milliseconds.
    #[serde(default)]
    pub paused_ms: u64,
}

#[derive(Deserialize)]
struct CheckpointList {
    checkpoints: Vec<Checkpoint>,
}

#[derive(Debug, Default)]
pub struct State {
    pub cid: u64,
    /// The rich content of the transcript: the hub's cards and images for its references (`console::rich`).
    pub cards: rich::Cards,
    pub error: Option<String>,
    /// The line a finished action leaves (the web's toast): "Restarting", "Stopped", …
    pub notice: Option<String>,
    pub agent: Option<Value>,
    pub profile: Option<Value>,
    pub infractions: Vec<Value>,
    /// The last infractions request failed: the headline says so instead of a count.
    pub infractions_failed: bool,
    pub infractions_all: bool,
    pub entries: Vec<Value>,
    pub last_seq: u64,
    pub extras_at: Option<Instant>,
    pub fetching: bool,
    pub busy: bool,
    pub sending: bool,
    pub tab: Tab,
    pub composer: text_editor::Content,
    pub dialog: Option<Dialog>,
    /// The tool blocks and results the reader has opened, by (entry seq, block index).
    pub open: HashSet<(u64, usize)>,
    pub side: agent_side::Side,
    /// The agent's sanctions and suspensions, refreshed with the profile.
    pub conduct: Option<sanctions::Register>,
    /// The reason typed for a lift, and the lift's error (shown in its card).
    pub lift_reason: String,
    pub lift_error: Option<String>,
    /// The VM checkpoints, newest first, refreshed with the profile (`checkpoints_error` when the read failed).
    pub checkpoints: Vec<Checkpoint>,
    pub checkpoints_error: Option<String>,
    /// The label typed for the next checkpoint, and the count typed for a fork (its error in the card).
    pub checkpoint_label: String,
    pub fork_n: String,
    pub fork_error: Option<String>,
    /// The names of the agents the last fork made, listed under the checkpoints.
    pub forked: Vec<String>,
    /// The nodes the Move card lists (loaded when it opens), the one picked, and the move's error.
    pub move_nodes: Option<Vec<nodes::NodeView>>,
    pub move_to: Option<String>,
    pub move_error: Option<String>,
    /// The reply text the hub streams (`delta` events) before the transcript holds it.
    pub live: Vec<LiveTurn>,
    /// Set once the reader scrolls away from the bottom; until then the transcript follows new text.
    pub scrolled_up: bool,
}

/// One poll: the agent, the transcript after the last seq, and (when due) the profile and infractions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub cid: u64,
    pub agent: Value,
    pub entries: Vec<Value>,
    pub extras: Option<Extras>,
}

/// The slower reads of a poll: the profile, and the infractions (or why they did not load).
#[derive(Debug, Clone)]
pub struct Extras {
    pub profile: Option<Value>,
    pub infractions: Result<Vec<Value>, String>,
    pub checkpoints: Result<Vec<Checkpoint>, String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    /// Opens the IDE on a target (the agent's workspace, see `ide::target`).
    OpenIde(String),
    Tab(Tab),
    Polled(Result<Snapshot, String>),
    Edit(text_editor::Action),
    Send,
    Sent(Result<Value, String>),
    Run(Act),
    Ask(Dialog),
    Toggle(Wipe),
    Dismiss,
    Proceed,
    Done {
        retired: bool,
        note: &'static str,
        result: Result<(), String>,
    },
    Expand((u64, usize)),
    ShowAll,
    Side(agent_side::SideMsg),
    Conduct(Result<sanctions::Register, String>),
    LiftReason(String),
    Lifted(Result<sanctions::Sanction, String>),
    /// The checkpoint label typed for the next one.
    CheckpointLabel(String),
    /// Takes a checkpoint now, with the label typed (if any).
    Take,
    /// The fork count typed in the card.
    ForkCount(String),
    /// A fork answered: the names of the agents it made.
    Forked(Result<Vec<String>, String>),
    /// The nodes the Move card lists, loaded when it opens.
    MoveNodes(Result<Vec<nodes::NodeView>, String>),
    /// A node picked in the Move card.
    PickNode(String),
    /// Moves the agent to the node picked.
    Move,
    /// The move answered: where the agent went, or the hub's error.
    Moved(Result<Moved, String>),
    /// One event from the hub's stream (only `delta` matters here).
    Event(events::Event),
    /// The transcript was scrolled.
    Scrolled(scrollable::Viewport),
    /// A rich-content click (a chip, a link, an Open button) or a load of its cards and images.
    Rich(rich::Ev),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::OpenIde(target) => Some(Id::Ide(Some(target.clone()))),
        Msg::Rich(rich::Ev::Action(action)) => action.route(),
        Msg::Side(agent_side::SideMsg::Rich(rich::Ev::Action(action))) => action.route(),
        _ => None,
    }
}

/// A rich-content click or load, as this page's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The texts of the transcript that rich content reads: the prompts and the text blocks of the replies.
fn transcript_texts(entries: &[Value]) -> Vec<&str> {
    let mut texts = Vec::new();
    for e in entries {
        let d = e.get("data").unwrap_or(&Value::Null);
        match s(e, "kind") {
            "work_start" => texts.push(s(d, "prompt")),
            "model_response" => {
                for b in d
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if s(b, "type") == "text" {
                        texts.push(s(b, "text"));
                    }
                }
            }
            _ => {}
        }
    }
    texts.retain(|t| !t.is_empty());
    texts
}

/// Asks the hub for the cards of the transcript's references that it does not hold yet.
fn prefetch(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let texts = transcript_texts(&state.entries);
    let rctx = rich::context(&state.cards, None, None, None);
    rich::prefetch(&mut state.cards, &ctx.client, &rctx, &texts).map(Msg::Rich)
}

/// One piece of a streamed reply, in the order it arrived.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Tool(String),
}

/// A reply still streaming: the `delta` events of one request within one work item.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveTurn {
    pub work: u64,
    pub req: u64,
    pub pieces: Vec<Piece>,
}

/// Applies one `delta` event (`{cid, work, req, delta: {type, text | name}}`) to the live replies,
/// as the web does. False when it is not this agent's text or tool delta; thinking is not shown.
pub fn apply_delta(live: &mut Vec<LiveTurn>, cid: u64, data: &Value) -> bool {
    if u(data, "cid") != cid {
        return false;
    }
    let Some(part) = data.get("delta") else {
        return false;
    };
    let piece = match s(part, "type") {
        "text" => Piece::Text(s(part, "text").to_string()),
        "tool_use" => Piece::Tool(s(part, "name").to_string()),
        _ => return false,
    };
    let (work, req) = (u(data, "work"), u(data, "req"));
    let at = match live.iter().position(|t| t.work == work && t.req == req) {
        Some(at) => at,
        None => {
            live.push(LiveTurn {
                work,
                req,
                pieces: Vec::new(),
            });
            live.len() - 1
        }
    };
    let pieces = &mut live[at].pieces;
    match piece {
        Piece::Text(text) => match pieces.last_mut() {
            Some(Piece::Text(last)) => last.push_str(&text),
            _ => pieces.push(Piece::Text(text)),
        },
        tool => pieces.push(tool),
    }
    true
}

/// A model response has landed in the transcript: its live text for that work is no longer shown.
pub fn settle(live: &mut Vec<LiveTurn>, work: u64) {
    live.retain(|t| t.work != work);
}

/// The streamed pieces of one reply: its text, and a chip for each tool it is calling.
fn live_views<'a>(
    turn: &'a LiveTurn,
    rctx: &rich::RichCtx,
    cards: &rich::Cards,
) -> Vec<Element<'a, Msg>> {
    turn.pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(body) => rich::view(body.as_str(), rctx, cards, &on_rich),
            Piece::Tool(name) => text(format!("→ {name}"))
                .font(ui::SANS_SEMIBOLD)
                .size(12.0)
                .color(crate::theme::palette().ink)
                .into(),
        })
        .collect()
}

fn transcript_id() -> iced::widget::Id {
    iced::widget::Id::new(TRANSCRIPT_ID)
}

/// The hub's events, while this page is open: the live `delta` text.
pub fn stream(ctx: &Ctx) -> iced::Subscription<Msg> {
    // The graph's keys. A key a focused text field captures (the find box, the composer) has status Captured and never
    // reaches here, as the web ignores keys while typing.
    let keys = iced::event::listen_with(|event, status, _| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. })
            if status == iced::event::Status::Ignored =>
        {
            super::memory_graph::key(&key, modifiers.shift())
                .map(|k| Msg::Side(agent_side::SideMsg::Key(k)))
        }
        _ => None,
    });
    // The web re-reads the notes every 30 seconds as a fallback to the live events; the Memory tab ignores it when hidden.
    let refresh =
        iced::time::every(Duration::from_secs(30)).map(|_| Msg::Side(agent_side::SideMsg::Refresh));
    iced::Subscription::batch([
        events::subscription(&ctx.client).map(Msg::Event),
        keys,
        refresh,
    ])
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let side = agent_side::load(&mut state.side, state.cid, ctx).map(Msg::Side);
    if state.fetching {
        return side;
    }
    state.fetching = true;
    let extras = match state.extras_at {
        None => true,
        Some(at) => at.elapsed() >= EXTRAS_EVERY,
    };
    if extras {
        state.extras_at = Some(Instant::now());
    }
    let cid = state.cid;
    let after = state.last_seq;
    let client = ctx.client.clone();
    let poll = client::task(move || fetch(&client, cid, after, extras), Msg::Polled);
    let conduct = if extras {
        let client = ctx.client.clone();
        client::task(
            move || sanctions::list(&client, Some(cid), None, None),
            Msg::Conduct,
        )
    } else {
        Task::none()
    };
    Task::batch([poll, side, conduct])
}

fn fetch(client: &client::Client, cid: u64, after: u64, extras: bool) -> Result<Snapshot, String> {
    let agent: Value = client.get(&format!("/v1/agents/{cid}"))?;
    let mut entries: Vec<Value> = client.get(&format!(
        "/v1/agents/{cid}/transcript?after={after}&limit=1000"
    ))?;
    entries.sort_by_key(|e| u(e, "seq"));
    let extras = extras.then(|| {
        let profile = client
            .get::<Value>(&format!("/v1/agents/{cid}/profile"))
            .ok();
        let infractions = client.get::<Vec<Value>>(&format!("/v1/agents/{cid}/infractions"));
        let checkpoints = client
            .get::<Value>(&format!("/v1/agents/{cid}/vm-checkpoints"))
            .and_then(parse_checkpoints);
        Extras {
            profile,
            infractions,
            checkpoints,
        }
    });
    Ok(Snapshot {
        cid,
        agent,
        entries,
        extras,
    })
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::OpenIde(_) => Task::none(),
        Msg::Rich(ev) => {
            rich::apply(&mut state.cards, ev);
            Task::none()
        }
        Msg::Tab(tab) => {
            state.tab = tab;
            Task::none()
        }
        Msg::Polled(result) => {
            state.fetching = false;
            let mut grew = false;
            match result {
                Ok(snap) if snap.cid == state.cid => {
                    state.error = None;
                    state.agent = Some(snap.agent);
                    for e in snap.entries {
                        let seq = u(&e, "seq");
                        if seq > state.last_seq {
                            state.last_seq = seq;
                            if s(&e, "kind") == "model_response" {
                                settle(&mut state.live, u(&e, "work"));
                            }
                            state.entries.push(e);
                            grew = true;
                        }
                    }
                    if let Some(Extras {
                        profile,
                        infractions,
                        checkpoints,
                    }) = snap.extras
                    {
                        state.profile = profile;
                        match checkpoints {
                            Ok(list) => {
                                state.checkpoints = list;
                                state.checkpoints_error = None;
                            }
                            Err(e) => {
                                // Nothing stale under the error line, as the infractions do.
                                state.checkpoints.clear();
                                state.checkpoints_error = Some(e);
                            }
                        }
                        match infractions {
                            Ok(list) => {
                                state.infractions = list;
                                state.infractions_failed = false;
                            }
                            Err(_) => {
                                // Nothing stale under a headline that says the list is unavailable.
                                state.infractions.clear();
                                state.infractions_failed = true;
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => state.error = Some(e),
            }
            let scroll = if grew && !state.scrolled_up {
                snap_to(transcript_id(), scrollable::RelativeOffset::END)
            } else {
                Task::none()
            };
            Task::batch([scroll, prefetch(state, ctx)])
        }
        Msg::Event(e) if e.kind == "memory" => {
            // A note of this agent changed: the Memory tab reloads (debounced in the side panel).
            let mine = e.data.get("cid").and_then(Value::as_u64) == Some(state.cid);
            if mine {
                agent_side::memory_changed(&mut state.side).map(Msg::Side)
            } else {
                Task::none()
            }
        }
        Msg::Event(e) => {
            if e.kind != "delta"
                || !apply_delta(&mut state.live, state.cid, &e.data)
                || state.scrolled_up
            {
                return Task::none();
            }
            snap_to(transcript_id(), scrollable::RelativeOffset::END)
        }
        Msg::Scrolled(view) => {
            let gap =
                view.content_bounds().height - view.bounds().height - view.absolute_offset().y;
            state.scrolled_up = gap >= NEAR_BOTTOM_PX;
            Task::none()
        }
        Msg::Edit(action) => {
            state.composer.perform(action);
            Task::none()
        }
        Msg::Send => {
            let text = state.composer.text().trim().to_string();
            if text.is_empty() || state.sending || offline(state) {
                return Task::none();
            }
            state.sending = true;
            let client = ctx.client.clone();
            let path = format!("/v1/agents/{}/chat", state.cid);
            client::task(
                move || client.post::<Value>(&path, &json!({ "text": text })),
                Msg::Sent,
            )
        }
        Msg::Sent(result) => {
            state.sending = false;
            match result {
                Ok(_) => state.composer = text_editor::Content::new(),
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Run(act) => {
            let (verb, note) = match act {
                Act::Cancel => ("cancel", "Cancelled"),
                Act::Stop => ("stop", "Stopped"),
                Act::Sleep => ("sleep", "Sleeping"),
                Act::Wake => ("wake", "Waking"),
            };
            let path = format!("/v1/agents/{}/{verb}", state.cid);
            dispatch(state, ctx, path, json!({}), false, note)
        }
        Msg::Ask(dialog) => {
            if matches!(dialog, Dialog::Lift(_)) {
                state.lift_reason.clear();
                state.lift_error = None;
            }
            if matches!(dialog, Dialog::Move) {
                state.move_nodes = None;
                state.move_to = None;
                state.move_error = None;
                state.dialog = Some(dialog);
                return move_nodes_task(ctx);
            }
            if matches!(dialog, Dialog::Fork) {
                state.fork_n = "1".to_string();
                state.fork_error = None;
            }
            state.dialog = Some(dialog);
            Task::none()
        }
        Msg::Toggle(which) => {
            if let Some(Dialog::Wipe {
                memory,
                transcript,
                identity,
            }) = state.dialog.as_mut()
            {
                let flag = match which {
                    Wipe::Memory => memory,
                    Wipe::Transcript => transcript,
                    Wipe::Identity => identity,
                };
                *flag = !*flag;
            }
            Task::none()
        }
        Msg::Dismiss => {
            state.dialog = None;
            Task::none()
        }
        Msg::Proceed => {
            let cid = state.cid;
            // A lift keeps its card open until the hub answers, so a refused lift can be read and retried.
            if let Some(Dialog::Lift(id)) = state.dialog.clone() {
                let reason = state.lift_reason.trim().to_string();
                if reason.is_empty() || state.busy {
                    return Task::none();
                }
                state.busy = true;
                state.lift_error = None;
                let client = ctx.client.clone();
                return client::task(move || sanctions::lift(&client, id, &reason), Msg::Lifted);
            }
            // The fork card stays open until the hub answers, so a refused count can be read and corrected.
            if let Some(Dialog::Fork) = state.dialog.clone() {
                let Some(n) = fork_count(&state.fork_n) else {
                    return Task::none();
                };
                if state.busy {
                    return Task::none();
                }
                state.busy = true;
                state.fork_error = None;
                let client = ctx.client.clone();
                let path = format!("/v1/agents/{cid}/fork");
                return client::task(
                    move || {
                        client
                            .post::<Value>(&path, &json!({ "n": n }))
                            .and_then(|v| fork_names(&v))
                    },
                    Msg::Forked,
                );
            }
            match state.dialog.take() {
                Some(Dialog::Restart) => dispatch(
                    state,
                    ctx,
                    format!("/v1/agents/{cid}/restart"),
                    json!({}),
                    false,
                    "Restarting",
                ),
                Some(Dialog::Wipe {
                    memory,
                    transcript,
                    identity,
                }) => {
                    let body =
                        json!({ "memory": memory, "transcript": transcript, "identity": identity });
                    dispatch(
                        state,
                        ctx,
                        format!("/v1/agents/{cid}/wipe"),
                        body,
                        false,
                        "Wiped",
                    )
                }
                Some(Dialog::Retire) => dispatch(
                    state,
                    ctx,
                    format!("/v1/agents/{cid}/retire"),
                    json!({}),
                    true,
                    "Retired",
                ),
                Some(Dialog::Restore(name)) => dispatch(
                    state,
                    ctx,
                    format!("/v1/agents/{cid}/vm-checkpoints/{name}/restore"),
                    json!({}),
                    false,
                    "Rolled back",
                ),
                Some(Dialog::Delete(name)) => {
                    let path = format!("/v1/agents/{cid}/vm-checkpoints/{name}");
                    dispatch_with(state, ctx, false, "Checkpoint deleted", move |c| {
                        c.delete(&path).map(|_| ())
                    })
                }
                None | Some(Dialog::Lift(_)) | Some(Dialog::Fork) | Some(Dialog::Move) => {
                    Task::none()
                }
            }
        }
        Msg::MoveNodes(Ok(list)) => {
            state.move_nodes = Some(list);
            Task::none()
        }
        Msg::MoveNodes(Err(e)) => {
            state.move_nodes = Some(Vec::new());
            state.move_error = Some(e);
            Task::none()
        }
        Msg::PickNode(id) => {
            state.move_to = Some(id);
            Task::none()
        }
        Msg::Move => {
            let Some(to) = state.move_to.clone() else {
                return Task::none();
            };
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.move_error = None;
            let client = ctx.client.clone();
            let path = format!("/v1/agents/{}/move", state.cid);
            let body = json!({ "node": to });
            client::task(move || client.post::<Moved>(&path, &body), Msg::Moved)
        }
        Msg::Moved(Ok(done)) => {
            state.busy = false;
            state.dialog = None;
            state.move_to = None;
            state.move_nodes = None;
            state.error = None;
            state.notice = Some(format!(
                "Moved from {} to {} ({} in {} ms)",
                done.from,
                done.to,
                size_text(done.bytes),
                done.ms
            ));
            // The agent's node changed: read the page again now, not on the next tick.
            state.extras_at = None;
            load(state, ctx)
        }
        Msg::Moved(Err(e)) => {
            state.busy = false;
            state.move_error = Some(e);
            Task::none()
        }
        Msg::CheckpointLabel(label) => {
            state.checkpoint_label = label;
            Task::none()
        }
        Msg::Take => {
            let cid = state.cid;
            let label = match checkpoint_label(&state.checkpoint_label) {
                Ok(label) => label,
                Err(e) => {
                    state.error = Some(e);
                    return Task::none();
                }
            };
            state.checkpoint_label.clear();
            let body = match label {
                Some(label) => json!({ "label": label }),
                None => json!({}),
            };
            dispatch(
                state,
                ctx,
                format!("/v1/agents/{cid}/vm-checkpoints"),
                body,
                false,
                "Checkpoint taken",
            )
        }
        Msg::ForkCount(text) => {
            state.fork_n = text;
            Task::none()
        }
        Msg::Forked(result) => {
            state.busy = false;
            match result {
                Ok(names) => {
                    state.dialog = None;
                    state.fork_n.clear();
                    state.fork_error = None;
                    state.forked = names;
                    state.notice = Some("Forked".to_string());
                    // The fork took a checkpoint of its own, and the new agents join the roster.
                    state.extras_at = None;
                    return load(state, ctx);
                }
                Err(e) => state.fork_error = Some(e),
            }
            Task::none()
        }
        Msg::LiftReason(reason) => {
            state.lift_reason = reason;
            Task::none()
        }
        Msg::Lifted(result) => {
            state.busy = false;
            match result {
                Ok(_) => {
                    state.dialog = None;
                    state.lift_reason.clear();
                    state.lift_error = None;
                    state.notice = Some("Lifted".to_string());
                    // Refresh the register now, not on the next 15-second tick.
                    state.extras_at = None;
                    return load(state, ctx);
                }
                Err(e) => state.lift_error = Some(e),
            }
            Task::none()
        }
        Msg::Conduct(result) => {
            match result {
                Ok(register) => state.conduct = Some(register),
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Done {
            retired,
            note,
            result,
        } => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    if retired {
                        return Task::done(Msg::Go(Id::Agents));
                    }
                    state.notice = Some(note.to_string());
                    // Sleep, wake and the checkpoints change what the page shows: read them now, not on the next tick.
                    state.extras_at = None;
                    return load(state, ctx);
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Expand(key) => {
            if !state.open.remove(&key) {
                state.open.insert(key);
            }
            Task::none()
        }
        Msg::ShowAll => {
            state.infractions_all = !state.infractions_all;
            Task::none()
        }
        Msg::Side(agent_side::SideMsg::Key(_) | agent_side::SideMsg::Refresh)
            if !matches!(state.tab, Tab::Memory) =>
        {
            Task::none()
        }
        Msg::Side(m) => agent_side::update(&mut state.side, m, ctx).map(Msg::Side),
    }
}

/// Posts one head action; the head buttons stay disabled until it answers.
fn dispatch(
    state: &mut State,
    ctx: &Ctx,
    path: String,
    body: Value,
    retired: bool,
    note: &'static str,
) -> Task<Msg> {
    dispatch_with(state, ctx, retired, note, move |client| {
        client.post::<Value>(&path, &body).map(|_| ())
    })
}

/// Runs one head action on the client and reports it as `Msg::Done`; the head buttons stay disabled until it answers.
fn dispatch_with(
    state: &mut State,
    ctx: &Ctx,
    retired: bool,
    note: &'static str,
    call: impl FnOnce(&client::Client) -> Result<(), String> + Send + 'static,
) -> Task<Msg> {
    state.busy = true;
    state.notice = None;
    let client = ctx.client.clone();
    client::task(
        move || call(&client),
        move |result| Msg::Done {
            retired,
            note,
            result,
        },
    )
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let name = state
        .agent
        .as_ref()
        .map(agent_name)
        .unwrap_or_else(|| format!("Agent {}", state.cid));
    let busy = state.busy;
    let pick = |m: Msg| if busy { None } else { Some(m) };
    let working = state
        .agent
        .as_ref()
        .is_some_and(|a| s(a, "status") == "working");
    let asleep = state
        .agent
        .as_ref()
        .is_some_and(|a| s(a, "status") == "asleep");
    // The hub refuses a sleep while the agent works or has queued work (409), so the button is off then.
    let sleep_blocked = state
        .agent
        .as_ref()
        .is_some_and(|a| s(a, "status") == "working" || u(a, "queue") > 0);
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if state.agent.is_some() {
        let workspace = ide::target(&format!("agent:{}", state.cid), false, None);
        actions.push(ui::btn(
            "Open workspace",
            Some("square-code"),
            ui::Kind::Ghost,
            pick(Msg::OpenIde(workspace)),
        ));
        if working {
            actions.push(ui::btn(
                "Cancel work",
                Some("ban"),
                ui::Kind::Secondary,
                pick(Msg::Run(Act::Cancel)),
            ));
        }
        if asleep {
            actions.push(ui::btn(
                "Wake",
                None,
                ui::Kind::Secondary,
                pick(Msg::Run(Act::Wake)),
            ));
        } else {
            let sleep = if sleep_blocked {
                None
            } else {
                pick(Msg::Run(Act::Sleep))
            };
            actions.push(ui::btn("Sleep", None, ui::Kind::Secondary, sleep));
        }
        actions.push(ui::btn(
            "Restart",
            None,
            ui::Kind::Secondary,
            pick(Msg::Ask(Dialog::Restart)),
        ));
        actions.push(ui::btn(
            "Move…",
            None,
            ui::Kind::Secondary,
            pick(Msg::Ask(Dialog::Move)),
        ));
        actions.push(ui::btn(
            "Stop",
            Some("stop"),
            ui::Kind::Danger,
            pick(Msg::Run(Act::Stop)),
        ));
        actions.push(ui::btn(
            "Wipe…",
            None,
            ui::Kind::Secondary,
            pick(Msg::Ask(Dialog::Wipe {
                memory: false,
                transcript: false,
                identity: false,
            })),
        ));
        actions.push(ui::btn(
            "Retire",
            None,
            ui::Kind::Danger,
            pick(Msg::Ask(Dialog::Retire)),
        ));
    }

    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(n) = &state.notice {
        body.push(ui::faint(n.as_str()).into());
    }
    if let Some(d) = &state.dialog {
        body.push(dialog_card(state, d, &name));
    }
    match state.agent.as_ref() {
        Some(a) => {
            if let Some(e) = &state.error {
                body.push(ui::error_line(e));
            }
            body.push(profile_card(state, a));
            body.push(conduct_card(state));
        }
        None => match &state.error {
            Some(e) => body.push(ui::empty("alert", "Could not load this agent", e)),
            None => body.push(ui::loading()),
        },
    }

    let tabs = ui::tabs(
        &[(Tab::Transcript, "Transcript"), (Tab::Memory, "Memory")],
        state.tab,
        Msg::Tab,
    );
    let content: Element<'a, Msg> = match state.tab {
        Tab::Transcript => column![transcript_card(state), composer_view(state)]
            .spacing(16)
            .width(Length::Fill)
            .into(),
        Tab::Memory => agent_side::memory_view(&state.side).map(Msg::Side),
    };
    let main = column![tabs, content].spacing(16).width(Length::Fill);

    let mut side: Vec<Element<'a, Msg>> = vec![agent_side::view(&state.side, ctx).map(Msg::Side)];
    if let Some(a) = state.agent.as_ref() {
        side.push(details_card(a));
        side.push(queue_card(a));
        side.push(checkpoints_card(state));
    }
    let side = container(column(side).spacing(16)).width(Length::Fixed(SIDE_WIDTH));
    body.push(row![main, side].spacing(24).into());

    ui::sheet(
        head(actions),
        ui::scroll(column(body).spacing(16).width(Length::Fill)).into(),
    )
}

/// The web's head: the back link on its own line, the actions right-aligned under it.
fn head<'a>(actions: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let back = button(
        text("Agents")
            .font(SANS)
            .size(13.0)
            .color(crate::theme::palette().ink_3),
    )
    .padding(0)
    .style(|_: &Theme, _: button::Status| button::Style {
        background: None,
        text_color: crate::theme::palette().ink_3,
        border: Border::default(),
        ..Default::default()
    })
    .on_press(Msg::Go(Id::Agents));
    let mut line = row![Space::new().width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Center);
    for action in actions {
        line = line.push(action);
    }
    container(column![back, line].spacing(8))
        .padding([24, 0])
        .width(Length::Fill)
        .into()
}

fn dialog_card<'a>(state: &'a State, d: &Dialog, name: &str) -> Element<'a, Msg> {
    let busy = state.busy;
    let proceed = if busy { None } else { Some(Msg::Proceed) };
    match d {
        Dialog::Lift(_) => {
            let reason = text_input("Why lift this suspension?", &state.lift_reason)
                .on_input(Msg::LiftReason)
                .size(13.0)
                .style(crate::theme::input);
            let lift = if busy || state.lift_reason.trim().is_empty() {
                None
            } else {
                Some(Msg::Proceed)
            };
            let mut body = column![
                ui::faint("The suspension ends now. The reason goes on the record."),
                reason
            ]
            .spacing(10);
            if let Some(e) = &state.lift_error {
                body = body.push(ui::error_line(e));
            }
            body = body.push(
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Lift", None, ui::Kind::Primary, lift)
                ]
                .spacing(8),
            );
            ui::card(&format!("Lift {name}'s suspension?"), body.into())
        }
        Dialog::Move => move_card(state, name),
        Dialog::Restart => ui::card(
            &format!("Restart {name}?"),
            column![
                ui::faint("Its VM restarts."),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Restart", None, ui::Kind::Primary, proceed)
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Wipe {
            memory,
            transcript,
            identity,
        } => ui::card(
            &format!("Wipe {name}"),
            column![
                ui::faint("The agent's VM restarts with a fresh workspace. This cannot be undone."),
                checkbox(true).label("Workspace (always)"),
                checkbox(*memory)
                    .label("Memory notes")
                    .on_toggle(|_| Msg::Toggle(Wipe::Memory)),
                checkbox(*transcript)
                    .label("Conversation transcript")
                    .on_toggle(|_| Msg::Toggle(Wipe::Transcript)),
                checkbox(*identity)
                    .label("Name and reputation")
                    .on_toggle(|_| Msg::Toggle(Wipe::Identity)),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Wipe", None, ui::Kind::Danger, proceed)
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Retire => ui::card(
            &format!("Retire {name}?"),
            column![
                ui::faint("Its VM stops for good and it leaves the roster."),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Retire", None, ui::Kind::Danger, proceed)
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Restore(checkpoint) => ui::card(
            &format!("Roll back {checkpoint} to this checkpoint?"),
            column![
                ui::faint(
                    "Its VM — memory, disk and running processes — returns to that moment. A safety checkpoint is taken first, so you can undo this."
                ),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Roll back", None, ui::Kind::Primary, proceed)
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Delete(checkpoint) => ui::card(
            &format!("Delete checkpoint {checkpoint}?"),
            column![
                ui::faint("The checkpoint is removed for good. The VM is not changed."),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Delete", None, ui::Kind::Danger, proceed)
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Dialog::Fork => {
            let count = fork_count(&state.fork_n);
            let input = text_input("1–8", &state.fork_n)
                .on_input(Msg::ForkCount)
                .size(13.0)
                .style(crate::theme::input);
            let fork = if busy || count.is_none() {
                None
            } else {
                Some(Msg::Proceed)
            };
            let mut body = column![
                ui::faint(
                    "Makes new agents from a checkpoint taken now, each a copy of this one. How many (1 to 8)?"
                ),
                input
            ]
            .spacing(10);
            if let Some(e) = &state.fork_error {
                body = body.push(ui::error_line(e));
            }
            body = body.push(
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
                    ui::btn("Fork", None, ui::Kind::Primary, fork)
                ]
                .spacing(8),
            );
            ui::card(&format!("Fork {name}"), body.into())
        }
    }
}

/// The profile: the portrait with its stamp, the name and role, the money, standing, conduct and
/// record tiles, then the infractions (`agent-profile.ts`). The portrait is the agent's hue and
/// initial until the desktop draws portraits.
fn profile_card<'a>(state: &'a State, a: &'a Value) -> Element<'a, Msg> {
    let p = state.profile.as_ref();
    let pv = |k: &str| f(p, k);
    let orchestrator = s(a, "tier") == "orchestrator";
    let hue = if orchestrator {
        25.0
    } else {
        role_hue(s(a, "role"))
    };
    let (status_text, status_tone) = status_label(s(a, "status"));

    let name = agent_name(a);
    let initial = name
        .chars()
        .next()
        .map(String::from)
        .unwrap_or_else(|| "A".to_string());
    let stamp = container(
        text(if orchestrator {
            "ORCHESTRATOR"
        } else {
            "CITIZEN"
        })
        .font(ui::SANS_BOLD)
        .size(11.0)
        .color(crate::theme::palette().signal),
    )
    .padding([4, 8])
    .style(ui::frame(
        Some(crate::theme::palette().sheet),
        crate::theme::palette().signal,
    ));
    // The web's portrait: the agent's own robot (the shared offscreen render), with the letter tile under it as the fallback.
    let cid = u(a, "cid");
    let body_hue = agent_hue(cid);
    let letter = container(ui::sans(initial, 64.0, crate::theme::palette().ink))
        .width(Length::Fixed(PORTRAIT))
        .height(Length::Fixed(PORTRAIT))
        .center_x(Length::Fixed(PORTRAIT))
        .center_y(Length::Fixed(PORTRAIT))
        .style(ui::frame(
            Some(mix(
                ui::hsl(body_hue, 0.6, 0.75),
                crate::theme::palette().sheet,
                0.45,
            )),
            crate::theme::palette().ink,
        ));
    let spec = crate::console::render_card::PortraitSpec {
        cid,
        orchestrator,
        role_hue: hue,
        body_hue,
        offline: !matches!(s(a, "status"), "working" | "idle"),
        species: crate::console::engine::robot::species_of(Some(s(a, "species")), cid),
    };
    let layers: Vec<Element<'a, Msg>> =
        match crate::console::render_card::robot_portrait(7, spec, PORTRAIT) {
            Some(robot) => vec![letter.into(), robot],
            None => vec![letter.into()],
        };
    let picture = iced::widget::stack(layers)
        .width(Length::Fixed(PORTRAIT))
        .height(Length::Fixed(PORTRAIT));
    let portrait = column![picture, stamp]
        .spacing(10)
        .align_x(Alignment::Center);

    let stripe = ui::hsl(hue, 0.55, 0.4);
    let role_chip = row![
        container(Space::new().width(Length::Fixed(4.0)).height(Length::Fill)).style(
            move |_: &Theme| container::Style {
                background: Some(stripe.into()),
                ..Default::default()
            }
        ),
        container(
            text(s(a, "role_label").to_string())
                .font(ui::SANS_SEMIBOLD)
                .size(11.5)
                .color(crate::theme::palette().ink)
        )
        .padding([6, 10])
        .style(ui::frame(
            Some(crate::theme::palette().sheet),
            crate::theme::palette().ink
        )),
    ]
    .height(Length::Shrink);
    let tags = row![
        role_chip,
        ui::sans(s(a, "model"), 12.0, crate::theme::palette().ink_2),
        ui::faint(format!("agent #{}", u(a, "cid")))
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let ident = column![
        row![
            text(name)
                .font(ui::SANS_BOLD)
                .size(34.0)
                .color(crate::theme::palette().ink),
            ui::badge(status_text, status_tone)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        tags
    ]
    .spacing(12)
    .width(Length::Fill);

    let usage = a.get("usage");
    let tokens = f(usage, "input_tokens") + f(usage, "output_tokens");
    let conf = pv("confidence");
    let rep: Element<'a, Msg> = match p {
        Some(_) if conf >= 0.05 => meter(
            pv("reputation"),
            100.0,
            crate::theme::palette().ok,
            format!("{} / 100", pv("reputation").round()),
        ),
        Some(_) => figure_faint("Unrated"),
        None => figure("—".into()),
    };
    let rep_sub = match p {
        Some(_) if conf >= 0.05 => format!("{}% confident", (conf * 100.0).round()),
        Some(_) => "no rated work yet".to_string(),
        None => String::new(),
    };
    let conduct: Element<'a, Msg> = match p {
        Some(_) => meter(
            pv("conduct"),
            100.0,
            conduct_tone(pv("conduct")),
            conduct_text(pv("conduct")).to_string(),
        ),
        None => figure("—".into()),
    };
    let tk = |k: &str| f(p.and_then(|p| p.get("tasks")), k);
    let fines = p.and_then(|p| p.get("fines"));
    let tiles_a = row![
        tile(
            "Balance",
            p.map(|_| figure(format!("{} cr", compact(pv("wallet")))))
                .unwrap_or_else(|| figure("—".into())),
            match p {
                Some(_) => format!(
                    "earned {} · spent {}",
                    compact(pv("earned")),
                    compact(pv("spent"))
                ),
                None => String::new(),
            }
        ),
        tile("Reputation", rep, rep_sub),
        tile(
            "Conduct",
            conduct,
            match p {
                Some(_) => format!(
                    "{} violations · {} refusals",
                    violations(p).len(),
                    pv("denials").round()
                ),
                None => String::new(),
            }
        ),
    ]
    .spacing(12);
    let tiles_b = row![
        tile(
            "Tasks",
            p.map(|_| figure(format!("{} verified", tk("verified"))))
                .unwrap_or_else(|| figure("—".into())),
            match p {
                Some(_) => format!(
                    "{} done · {} failed · {} posted",
                    tk("done"),
                    tk("failed"),
                    tk("posted")
                ),
                None => String::new(),
            }
        ),
        tile(
            "Network",
            p.map(|_| figure(format!("{} contacts", pv("contacts"))))
                .unwrap_or_else(|| figure("—".into())),
            match p {
                Some(_) => format!("{} memories", pv("memory_notes")),
                None => String::new(),
            }
        ),
        tile(
            "Tokens",
            figure(compact(tokens)),
            match p {
                Some(p) => format!("joined {}", ago(u(p, "joined_ms"))),
                None => String::new(),
            }
        ),
    ]
    .spacing(12);

    let mut record: Vec<Element<'a, Msg>> = Vec::new();
    let viol = violations(p);
    if p.is_some() && (!viol.is_empty() || f(fines, "count") > 0.0) {
        record.push(ui::label("Record").into());
        for v in viol.iter().take(5) {
            record.push(
                row![
                    ui::badge(s(v, "law"), ui::Tone::Danger),
                    ui::prose(s(v, "detail")),
                    ui::faint(ago(u(v, "at_ms")))
                ]
                .spacing(8)
                .into(),
            );
        }
        if f(fines, "count") > 0.0 {
            record.push(
                ui::faint(format!(
                    "{} fines, {} cr in total",
                    f(fines, "count"),
                    compact(f(fines, "total"))
                ))
                .into(),
            );
        }
    } else if p.is_some() {
        record.push(ui::faint("A clean record: no violations, no fines.").into());
    }

    let n = state.infractions.len();
    let headline: Element<'a, Msg> = if state.extras_at.is_none() {
        ui::faint("Loading…").into()
    } else if state.infractions_failed {
        ui::faint("Infractions unavailable").into()
    } else if n == 0 {
        ui::badge("No infractions", ui::Tone::Ok)
    } else {
        ui::badge(
            &format!("{} infraction{}", n, if n == 1 { "" } else { "s" }),
            ui::Tone::Danger,
        )
    };
    let mut inf: Vec<Element<'a, Msg>> = vec![ui::label("Infractions").into(), headline];
    let shown = if state.infractions_all {
        n
    } else {
        INFRACTIONS_SHOWN
    };
    for r in state.infractions.iter().take(shown) {
        inf.push(
            row![
                ui::link(
                    pick_str(s(r, "law_title"), s(r, "law")),
                    Msg::Go(Id::Rules),
                    false
                ),
                ui::mono(s(r, "tool"), 12.0, crate::theme::palette().ink_2),
                ui::faint(s(r, "detail")),
                ui::faint(ago(u(r, "at_ms"))),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if n > INFRACTIONS_SHOWN {
        inf.push(ui::link(
            if state.infractions_all {
                "Show fewer"
            } else {
                "Show all"
            },
            Msg::ShowAll,
            false,
        ));
    }

    let info = column![
        ident,
        tiles_a,
        tiles_b,
        column(record).spacing(6),
        column(inf).spacing(6)
    ]
    .spacing(14)
    .width(Length::Fill);
    container(row![portrait, info].spacing(24))
        .padding(20)
        .width(Length::Fill)
        .style(ui::frame(
            Some(crate::theme::palette().sheet),
            crate::theme::palette().ink,
        ))
        .into()
}

/// The Conduct section: the active suspensions, each with a Lift, then the sanctions, newest first.
fn conduct_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(register) = state.conduct.as_ref() else {
        return ui::card("Conduct", ui::faint("Loading…").into());
    };
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let suspended: Vec<&sanctions::Suspension> =
        register.suspensions.iter().filter(|s| s.active).collect();
    if suspended.is_empty() {
        rows.push(ui::faint("No active suspensions.").into());
    } else {
        rows.push(ui::label("Suspended now").into());
        for s in suspended {
            rows.push(
                row![
                    ui::badge(&s.law, ui::Tone::Danger),
                    ui::mono(s.tool.clone(), 12.0, crate::theme::palette().ink_2),
                    ui::faint(left(s.until_ms)),
                    Space::new().width(Length::Fill),
                    ui::btn(
                        "Lift",
                        None,
                        ui::Kind::Primary,
                        (!state.busy).then(|| Msg::Ask(Dialog::Lift(s.id)))
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .into(),
            );
        }
    }
    rows.push(ui::label("Sanctions").into());
    if register.sanctions.is_empty() {
        rows.push(ui::faint("No sanctions on record.").into());
    } else {
        let mut newest: Vec<&sanctions::Sanction> = register.sanctions.iter().collect();
        newest.sort_by_key(|s| std::cmp::Reverse(s.at_ms));
        rows.extend(newest.into_iter().map(sanction_row));
    }
    ui::card("Conduct", column(rows).spacing(10).into())
}

/// One sanction: what, under which law, how severe, how many offences, the outcome, the fine, when,
/// and the owner's lift with its reason.
fn sanction_row<'a>(s: &'a sanctions::Sanction) -> Element<'a, Msg> {
    let tone = if s.lifted.is_some() {
        ui::Tone::Muted
    } else {
        ui::Tone::Danger
    };
    let offences = format!("{} offence{}", s.count, if s.count == 1 { "" } else { "s" });
    let mut line = column![
        row![
            ui::badge(&s.sanction, tone),
            ui::sans(s.law.clone(), 12.0, crate::theme::palette().ink),
            ui::sans(s.severity.clone(), 12.0, crate::theme::palette().ink_2),
            ui::faint(offences),
            Space::new().width(Length::Fill),
            ui::faint(ago(s.at_ms)),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    ]
    .spacing(4);
    if !s.outcome.is_empty() {
        line = line.push(ui::faint(s.outcome.clone()));
    }
    if s.fine > 0 {
        line = line.push(ui::faint(format!("{} cr fine", compact(s.fine as f64))));
    }
    if let Some(lifted) = &s.lifted {
        line = line.push(ui::faint(format!("lifted by owner: {}", lifted.why)));
    }
    if let Some(action) = &s.owner_action {
        let word = if action.action == "retire" {
            "Retired"
        } else {
            "Acknowledged"
        };
        line = line.push(ui::faint(format!(
            "{word} by the owner, {}",
            ago(action.at_ms)
        )));
    }
    line.into()
}

/// The time left on a suspension, in the web's short words.
fn left(until_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = until_ms.saturating_sub(now) / 1000;
    match secs {
        0..=59 => format!("{secs}s left"),
        60..=3599 => format!("{}m left", secs / 60),
        3600..=86_399 => format!("{}h left", secs / 3600),
        _ => format!("{}d left", secs / 86_400),
    }
}

/// A profile tile: its label, its figure (text or a meter) and its caption, on paper.
fn tile<'a>(label: &'static str, value: Element<'a, Msg>, sub: String) -> Element<'a, Msg> {
    container(
        column![
            text(label.to_string())
                .font(ui::SANS_SEMIBOLD)
                .size(11.0)
                .color(crate::theme::palette().ink_2),
            value,
            ui::sans(sub, 11.0, crate::theme::palette().ink_3)
        ]
        .spacing(6),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(ui::frame(
        Some(crate::theme::palette().paper),
        crate::theme::palette().rule,
    ))
    .into()
}

fn figure<'a>(value: String) -> Element<'a, Msg> {
    text(value)
        .font(ui::SANS_BOLD)
        .size(22.0)
        .color(crate::theme::palette().ink)
        .into()
}

fn figure_faint<'a>(value: &'static str) -> Element<'a, Msg> {
    ui::faint(value).into()
}

/// A meter: a 2px-ruled track filled to `value` of `max`, its caption under it.
fn meter<'a>(value: f64, max: f64, fill: Color, caption: String) -> Element<'a, Msg> {
    let pct = (value / max * 100.0).clamp(0.0, 100.0).round() as u16;
    let mut bar = row![].height(Length::Fill);
    if pct > 0 {
        bar = bar.push(
            container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::FillPortion(pct))
                .height(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(fill.into()),
                    ..Default::default()
                }),
        );
    }
    if pct < 100 {
        bar = bar.push(
            Space::new()
                .width(Length::FillPortion(100 - pct))
                .height(Length::Fill),
        );
    }
    let track = container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(14.0))
        .style(ui::frame(
            Some(crate::theme::palette().sheet),
            crate::theme::palette().ink,
        ));
    column![
        track,
        text(caption)
            .font(ui::SANS_SEMIBOLD)
            .size(12.0)
            .color(crate::theme::palette().ink)
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
}

fn transcript_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let rctx = rich::context(&state.cards, None, None, None);
    let (rctx, cards) = (&rctx, &state.cards);
    // Entries group into their work item's section; a rule divides one section from the next.
    let mut groups: Vec<Vec<&'a Value>> = Vec::new();
    for e in &state.entries {
        match groups.last_mut() {
            Some(group)
                if group
                    .first()
                    .is_some_and(|first| u(first, "work") == u(e, "work")) =>
            {
                group.push(e)
            }
            _ => groups.push(vec![e]),
        }
    }
    let count = groups.len();
    let mut sections: Vec<Element<'a, Msg>> = Vec::new();
    let mut shown: Vec<u64> = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        let work = u(group[0], "work");
        shown.push(work);
        let mut parts: Vec<Element<'a, Msg>> = Vec::new();
        for e in group {
            parts.extend(entry_view(&state.open, e, rctx, cards));
        }
        for turn in state.live.iter().filter(|t| t.work == work) {
            parts.extend(live_views(turn, rctx, cards));
        }
        sections.push(column(parts).spacing(12).width(Length::Fill).into());
        if i + 1 < count {
            sections.push(ui::hairline());
        }
    }
    // A reply whose work has no entry yet (its first deltas came before the next poll) goes last.
    let rest: Vec<Element<'a, Msg>> = state
        .live
        .iter()
        .filter(|t| !shown.contains(&t.work))
        .flat_map(|t| live_views(t, rctx, cards))
        .collect();
    if !rest.is_empty() {
        if count > 0 {
            sections.push(ui::hairline());
        }
        sections.push(column(rest).spacing(12).width(Length::Fill).into());
    }
    let lines: Element<'a, Msg> = column(sections).spacing(16).width(Length::Fill).into();
    container(
        ui::scroll(lines)
            .id(transcript_id())
            .on_scroll(Msg::Scrolled)
            .height(Length::Fixed(TRANSCRIPT_HEIGHT)),
    )
    .padding(16)
    .width(Length::Fill)
    .style(ui::frame(None, crate::theme::palette().rule))
    .into()
}

fn composer_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let offline = offline(state);
    let mut editor = text_editor(&state.composer)
        .placeholder("Message this agent…")
        .height(Length::Fixed(96.0))
        .key_binding(|kp| {
            if kp.key == keyboard::Key::Named(keyboard::key::Named::Enter) && kp.modifiers.control()
            {
                Some(text_editor::Binding::Custom(Msg::Send))
            } else {
                text_editor::Binding::from_key_press(kp)
            }
        });
    if !offline {
        editor = editor.on_action(Msg::Edit);
    }
    let can_send = !offline && !state.sending && !state.composer.text().trim().is_empty();
    let send = ui::btn(
        "Send",
        Some("send"),
        ui::Kind::Primary,
        if can_send { Some(Msg::Send) } else { None },
    );
    column![editor, row![Space::new().width(Length::Fill), send]]
        .spacing(8)
        .into()
}

/// One transcript entry as the web renders it: turns, tool calls and results (collapsed until
/// opened), decisions, errors. Blocks key their open state by the entry's seq and block index.
fn entry_view<'a>(
    open: &HashSet<(u64, usize)>,
    e: &'a Value,
    rctx: &rich::RichCtx,
    cards: &rich::Cards,
) -> Vec<Element<'a, Msg>> {
    let d = e.get("data").unwrap_or(&Value::Null);
    let seq = u(e, "seq");
    match s(e, "kind") {
        "work_start" => vec![
            ui::badge(s(d, "kind"), ui::Tone::Info),
            rich::view(s(d, "prompt"), rctx, cards, &on_rich),
        ],
        "work_end" => {
            let st = s(d, "status");
            let tone = if st.contains("fail") || st.contains("error") {
                ui::Tone::Danger
            } else if st.contains("cancel") || st.contains("stop") {
                ui::Tone::Warn
            } else {
                ui::Tone::Ok
            };
            vec![ui::badge(if st.is_empty() { "done" } else { st }, tone)]
        }
        "model_response" => {
            let mut out: Vec<Element<'a, Msg>> = Vec::new();
            if let Some(blocks) = d.get("content").and_then(Value::as_array) {
                for (idx, b) in blocks.iter().enumerate() {
                    out.extend(block_view(open, seq, idx, b, rctx, cards));
                }
            }
            out
        }
        "tool_decision" => match s(d, "decision") {
            "allow" => vec![ui::badge("ran in VM", ui::Tone::Info)],
            "done" => vec![ui::badge("ran on host", ui::Tone::Muted)],
            "deny" => vec![ui::badge(
                &format!("denied: {}", s(d, "reason")),
                ui::Tone::Danger,
            )],
            _ => Vec::new(),
        },
        "tool_result" => {
            let failed = d.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let body = pick_str(s(d, "content"), s(d, "output"));
            let key = (seq, 0);
            let opened = open.contains(&key);
            let (label, tone) = if failed {
                ("Error", crate::theme::palette().crit)
            } else {
                ("Output", crate::theme::palette().ok)
            };
            let mut out = vec![ui::disclosure(
                label.to_string(),
                tone,
                opened,
                Msg::Expand(key),
            )];
            if opened {
                out.push(pre(body.to_string()));
            }
            out
        }
        "error" => vec![ui::error_line(pick_str(s(d, "message"), "Error"))],
        _ => Vec::new(),
    }
}

fn block_view<'a>(
    open: &HashSet<(u64, usize)>,
    seq: u64,
    idx: usize,
    b: &'a Value,
    rctx: &rich::RichCtx,
    cards: &rich::Cards,
) -> Vec<Element<'a, Msg>> {
    match s(b, "type") {
        "text" => vec![rich::view(s(b, "text"), rctx, cards, &on_rich)],
        "tool_use" => {
            let key = (seq, idx);
            let opened = open.contains(&key);
            let mut out = vec![ui::chip(
                format!("→ {}", s(b, "name")),
                opened,
                Msg::Expand(key),
            )];
            if opened {
                let input = serde_json::to_string_pretty(b.get("input").unwrap_or(&Value::Null))
                    .unwrap_or_default();
                out.push(pre(input));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// An opened body in mono, on a ruled panel.
fn pre<'a>(body: String) -> Element<'a, Msg> {
    container(ui::mono(body, 12.0, crate::theme::palette().ink_2))
        .padding(8)
        .width(Length::Fill)
        .style(ui::frame(None, crate::theme::palette().rule))
        .into()
}

/// The details card: the agent's ids, its VM, when it connected and was seen, its tokens.
fn details_card<'a>(a: &'a Value) -> Element<'a, Msg> {
    let usage = a.get("usage");
    let run = match s(a, "run") {
        "" => "—",
        run => run,
    };
    let vm = a
        .get("vm")
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_string());
    let rows: Vec<Element<'a, Msg>> = vec![
        kv(
            "CID",
            ui::mono(u(a, "cid").to_string(), 13.0, crate::theme::palette().ink).into(),
        ),
        kv(
            "Run",
            ui::sans(run, 13.0, crate::theme::palette().ink).into(),
        ),
        kv("VM", ui::sans(vm, 13.0, crate::theme::palette().ink).into()),
        kv("Connected", ui::prose(when(a, "connected_at_ms")).into()),
        kv("Last seen", ui::prose(when(a, "last_seen_ms")).into()),
        kv(
            "Tokens in",
            ui::sans(
                compact(f(usage, "input_tokens")),
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        kv(
            "Tokens out",
            ui::sans(
                compact(f(usage, "output_tokens")),
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
    ];
    ui::card("Details", column(rows).spacing(8).into())
}

fn kv<'a>(key: &'static str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(key)).width(Length::Fixed(96.0)), value]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
}

/// The queue card: the work running now, and how many items wait behind it.
fn queue_card<'a>(a: &'a Value) -> Element<'a, Msg> {
    let work: Element<'a, Msg> = match a.get("work").filter(|w| !w.is_null()) {
        Some(w) => column![
            ui::badge(s(w, "kind"), ui::Tone::Info),
            ui::prose(preview(s(w, "prompt"), 160))
        ]
        .spacing(6)
        .into(),
        None => ui::faint("Nothing running").into(),
    };
    ui::card(
        "Queue",
        column![work, ui::faint(format!("{} queued", u(a, "queue")))]
            .spacing(8)
            .into(),
    )
}

/// Mixes `over` into `base` by `amount` (0 keeps `base`, 1 is `over`).
fn mix(base: Color, over: Color, amount: f32) -> Color {
    Color {
        r: base.r * amount + over.r * (1.0 - amount),
        g: base.g * amount + over.g * (1.0 - amount),
        b: base.b * amount + over.b * (1.0 - amount),
        a: 1.0,
    }
}

/// The profile's word and tone for an agent status. Asleep is its own state: its VM is snapshotted and stopped.
fn status_label(status: &str) -> (&'static str, ui::Tone) {
    match status {
        "working" => ("Working", ui::Tone::Info),
        "idle" => ("Idle", ui::Tone::Ok),
        "asleep" => ("Asleep", ui::Tone::Muted),
        _ => ("Offline", ui::Tone::Muted),
    }
}

/// The checkpoint list's body, read into the checkpoints (`{checkpoints: [...]}`, newest first).
fn parse_checkpoints(body: Value) -> Result<Vec<Checkpoint>, String> {
    serde_json::from_value::<CheckpointList>(body)
        .map(|list| list.checkpoints)
        .map_err(|e| e.to_string())
}

/// The names of the agents a fork made (`{forks: [{cid, run, name}], checkpoint}`).
fn fork_names(reply: &Value) -> Result<Vec<String>, String> {
    let forks = reply
        .get("forks")
        .and_then(Value::as_array)
        .ok_or_else(|| "the fork reply has no forks".to_string())?;
    Ok(forks.iter().map(|f| s(f, "name").to_string()).collect())
}

/// A fork's count: 1 to 8 copies, as the hub takes them.
fn fork_count(text: &str) -> Option<u64> {
    text.trim()
        .parse::<u64>()
        .ok()
        .filter(|n| (1..=8).contains(n))
}

/// A checkpoint's label, as the hub takes it: 1 to 40 characters with no control characters. Blank means none given.
fn checkpoint_label(text: &str) -> Result<Option<String>, String> {
    let label = text.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.chars().count() > 40 {
        return Err("A checkpoint label is 1 to 40 characters.".to_string());
    }
    if label.chars().any(char::is_control) {
        return Err("A checkpoint label has no control characters.".to_string());
    }
    Ok(Some(label.to_string()))
}

/// One checkpoint's row in the side column: its label (its name when it has none), its age and kind, what it
/// wrote and what it paused the VM for, then Restore and Delete, which ask first.
fn checkpoint_row<'a>(cp: &'a Checkpoint, busy: bool) -> Element<'a, Msg> {
    let pick = |m: Msg| if busy { None } else { Some(m) };
    let title = if cp.label.is_empty() {
        cp.name.clone()
    } else {
        cp.label.clone()
    };
    let facts = format!(
        "{} · memory {} · disk {} · paused {} ms",
        ago(cp.taken_at_ms),
        lab_common::size(cp.memory_written),
        lab_common::size(cp.disk_bytes),
        cp.paused_ms
    );
    column![
        row![
            ui::sans(title, 13.0, crate::theme::palette().ink).width(Length::Fill),
            ui::badge(&cp.kind, ui::Tone::Muted)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::faint(facts),
        row![
            ui::btn(
                "Restore…",
                None,
                ui::Kind::Secondary,
                pick(Msg::Ask(Dialog::Restore(cp.name.clone())))
            ),
            ui::btn(
                "Delete…",
                None,
                ui::Kind::Danger,
                pick(Msg::Ask(Dialog::Delete(cp.name.clone())))
            ),
        ]
        .spacing(8),
    ]
    .spacing(4)
    .into()
}

/// The machine checkpoints (`GET /v1/agents/{cid}/vm-checkpoints`): take one with an optional label, roll back to
/// one, delete one, or fork the agent. The fork's new agents are named under the card.
fn checkpoints_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let busy = state.busy;
    let pick = |m: Msg| if busy { None } else { Some(m) };
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.checkpoints_error {
        items.push(ui::error_line(e));
    }
    if state.checkpoints.is_empty() && state.checkpoints_error.is_none() {
        items.push(ui::faint("No checkpoints yet").into());
    }
    for cp in &state.checkpoints {
        items.push(checkpoint_row(cp, busy));
    }
    let label = text_input("Label (optional)", &state.checkpoint_label)
        .on_input(Msg::CheckpointLabel)
        .size(13.0)
        .style(crate::theme::input);
    items.push(label.into());
    items.push(
        row![
            ui::btn(
                "Take checkpoint",
                None,
                ui::Kind::Secondary,
                pick(Msg::Take)
            ),
            ui::btn(
                "Fork…",
                None,
                ui::Kind::Secondary,
                pick(Msg::Ask(Dialog::Fork))
            ),
        ]
        .spacing(8)
        .into(),
    );
    if !state.forked.is_empty() {
        items.push(ui::faint(format!("Forked: {}", state.forked.join(", "))).into());
    }
    ui::card("Machine checkpoints", column(items).spacing(12).into())
}

fn offline(state: &State) -> bool {
    state
        .agent
        .as_ref()
        .is_some_and(|a| s(a, "status") == "offline")
}

fn agent_name(a: &Value) -> String {
    let name = s(a, "name");
    if name.is_empty() {
        format!("Agent {}", u(a, "cid"))
    } else {
        name.to_string()
    }
}

fn conduct_text(score: f64) -> &'static str {
    if score >= 90.0 {
        "Exemplary"
    } else if score >= 70.0 {
        "Good standing"
    } else if score >= 40.0 {
        "Under watch"
    } else {
        "In trouble"
    }
}

fn conduct_tone(score: f64) -> Color {
    if score >= 90.0 {
        crate::theme::palette().ok
    } else if score >= 70.0 {
        crate::theme::palette().info
    } else if score >= 40.0 {
        crate::theme::palette().warn
    } else {
        crate::theme::palette().crit
    }
}

fn violations(p: Option<&Value>) -> &[Value] {
    p.and_then(|p| p.get("violations"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn f(v: Option<&Value>, key: &str) -> f64 {
    v.and_then(|v| v.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn pick_str<'a>(first: &'a str, fallback: &'a str) -> &'a str {
    if first.is_empty() { fallback } else { first }
}

fn preview(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn compact(n: f64) -> String {
    let a = n.abs();
    if a >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if a >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{}", n.round())
    }
}

/// How long ago an epoch-millisecond instant was, in the web's short words.
pub(super) fn ago(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// The `ago` of a record's field, or a dash when the record does not carry it.
fn when(a: &Value, key: &str) -> String {
    a.get(key)
        .and_then(Value::as_u64)
        .map(ago)
        .unwrap_or_else(|| "—".to_string())
}

/// The Move card: the nodes the agent may move to, one picked, then Move. The list is read when the card opens.
fn move_card<'a>(state: &'a State, name: &str) -> Element<'a, Msg> {
    let current = state
        .agent
        .as_ref()
        .map(|a| s(a, "node").to_string())
        .unwrap_or_default();
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    match &state.move_nodes {
        None => body.push(ui::loading()),
        Some(list) => {
            let targets = move_targets(list, &current);
            if targets.is_empty() {
                body.push(
                    ui::faint(
                        "No other node can take it: each one is draining, revoked, or the one it is on now.",
                    )
                    .into(),
                );
            }
            for node in targets {
                let kind = if state.move_to.as_deref() == Some(node.id.as_str()) {
                    ui::Kind::Primary
                } else {
                    ui::Kind::Secondary
                };
                let pick = (!state.busy).then(|| Msg::PickNode(node.id.clone()));
                body.push(ui::btn(&node_label(node), None, kind, pick));
            }
        }
    }
    if let Some(e) = &state.move_error {
        body.push(ui::error_line(e));
    }
    let can_move = !state.busy && state.move_to.is_some();
    body.push(
        row![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Dismiss)),
            ui::btn(
                "Move",
                None,
                ui::Kind::Primary,
                can_move.then_some(Msg::Move)
            ),
        ]
        .spacing(8)
        .into(),
    );
    ui::card(
        &format!("Move {name}"),
        column(body).spacing(10).width(Length::Fill).into(),
    )
}

/// The nodes an agent on `current` may move to: not draining, not revoked, and not the node it is on.
pub fn move_targets<'a>(all: &'a [nodes::NodeView], current: &str) -> Vec<&'a nodes::NodeView> {
    all.iter()
        .filter(|node| !node.draining && !node.revoked && node.id != current)
        .collect()
}

/// A node as the Move card names it: its name and id, or the id alone when it has no name.
fn node_label(node: &nodes::NodeView) -> String {
    if node.name.is_empty() {
        node.id.clone()
    } else {
        format!("{} ({})", node.name, node.id)
    }
}

/// Reads the plaza's nodes for the Move card.
fn move_nodes_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Vec<nodes::NodeView>>("/v1/nodes"),
        Msg::MoveNodes,
    )
}

/// A byte count in the words the console uses: megabytes with one decimal, or kilobytes.
fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_move_result_parses_and_reads_in_words() {
        let done: Moved =
            serde_json::from_str(r#"{"ok":true,"from":"n0","to":"n3","bytes":2097152,"ms":412}"#)
                .expect("the move answer parses");
        assert_eq!((done.from.as_str(), done.to.as_str()), ("n0", "n3"));
        assert_eq!(size_text(done.bytes), "2.0 MB");
        assert_eq!(size_text(5 * 1024), "5 KB");
        assert_eq!(done.ms, 412);
    }

    #[test]
    fn live_deltas_join_the_reply_and_skip_other_agents_and_thinking() {
        {
            // delta_text_joins_into_the_live_reply: text deltas join into one live reply
            let mut live = Vec::new();
            let text = |t: &str| json!({"cid": 7, "work": 3, "req": 1, "delta": {"type": "text", "text": t}});
            assert!(apply_delta(&mut live, 7, &text("Hel")));
            assert!(apply_delta(&mut live, 7, &text("lo")));
            assert_eq!(live.len(), 1);
            assert_eq!(live[0].pieces, vec![Piece::Text("Hello".into())]);
        }
        {
            // deltas_for_other_agents_and_thinking_are_ignored: deltas of another agent or
            // thinking deltas leave the live reply untouched
            let mut live = Vec::new();
            let other =
                json!({"cid": 8, "work": 3, "req": 1, "delta": {"type": "text", "text": "x"}});
            let thinking =
                json!({"cid": 7, "work": 3, "req": 1, "delta": {"type": "thinking", "text": "x"}});
            assert!(!apply_delta(&mut live, 7, &other));
            assert!(!apply_delta(&mut live, 7, &thinking));
            assert!(live.is_empty());
        }
    }

    #[test]
    fn tool_calls_keep_their_place_and_a_response_settles_the_turn() {
        let mut live = Vec::new();
        apply_delta(
            &mut live,
            7,
            &json!({"cid": 7, "work": 3, "req": 1, "delta": {"type": "text", "text": "Let me look. "}}),
        );
        apply_delta(
            &mut live,
            7,
            &json!({"cid": 7, "work": 3, "req": 1, "delta": {"type": "tool_use", "name": "read_file"}}),
        );
        apply_delta(
            &mut live,
            7,
            &json!({"cid": 7, "work": 3, "req": 1, "delta": {"type": "text", "text": "Done."}}),
        );
        assert_eq!(
            live[0].pieces,
            vec![
                Piece::Text("Let me look. ".into()),
                Piece::Tool("read_file".into()),
                Piece::Text("Done.".into())
            ]
        );
        settle(&mut live, 3);
        assert!(live.is_empty());
    }

    #[test]
    fn checkpoint_list_parses_in_the_hubs_order_and_rejects_a_missing_array() {
        {
            // checkpoints_read_in_the_hubs_order_with_the_sleep_snapshot
            let list = json!({"checkpoints": [
                {"name": "cp-2000-before-lunch", "cid": 7, "label": "before lunch", "kind": "diff",
                 "taken_at_ms": 2000, "memory_written": 1536, "disk_bytes": 8192, "paused_ms": 41},
                {"name": "sleep", "cid": 7, "label": "", "kind": "full",
                 "taken_at_ms": 1000, "memory_written": 0, "disk_bytes": 0, "paused_ms": 0}
            ]});
            let cps = parse_checkpoints(list).unwrap();
            assert_eq!(cps.len(), 2);
            assert_eq!(cps[0].name, "cp-2000-before-lunch");
            assert_eq!(cps[0].label, "before lunch");
            assert_eq!(cps[0].kind, "diff");
            assert_eq!(cps[0].memory_written, 1536);
            assert_eq!(cps[0].paused_ms, 41);
            assert_eq!(cps[1].name, "sleep");
            assert!(cps[1].label.is_empty());
        }
        {
            // a_checkpoint_list_without_the_array_is_an_error
            assert!(parse_checkpoints(json!({"error": "no agent"})).is_err());
        }
    }

    #[test]
    fn a_fork_reply_names_its_new_agents() {
        let reply = json!({
            "forks": [{"cid": 8, "run": "r1", "name": "Ada 2"}, {"cid": 9, "run": "r1", "name": "Ada 3"}],
            "checkpoint": {"name": "cp-1-fork"}
        });
        assert_eq!(fork_names(&reply).unwrap(), vec!["Ada 2", "Ada 3"]);
        assert!(fork_names(&json!({"ok": true})).is_err());
    }
}
