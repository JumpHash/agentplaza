//! The agent page's side panels (`agent-detail.ts` side column, `agent-harness.ts`): model and
//! reasoning effort (the model picker), harness limits and loops, permissions. Also the memory
//! library the page's Memory tab shows (`memory.ts`): the notes, a find box, and a reader.

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Border, Element, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::agent::ago;
use crate::console::picker::{self, Choice, Picker, PickerMsg};
use crate::console::{Ctx, client, rich, ui};
use crate::theme::{MONO_MEDIUM, SANS, SANS_MEDIUM, SANS_SEMIBOLD, TYPE_ITEM, TYPE_META};

/// The reasoning efforts a model name can pin (`provider/model@effort`).
const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The permission modes as the web offers them: (mode, label, hint).
const PERMISSIONS: [(&str, &str, &str); 3] = [
    ("autonomous", "Autonomous", "acts on its own"),
    (
        "ask",
        "Ask first",
        "write and irreversible actions wait for your approval in Approvals",
    ),
    ("read_only", "Read-only", "can look, not change"),
];

#[derive(Debug, Default)]
pub struct Side {
    pub cid: u64,
    pub error: Option<String>,
    pub notice: Option<String>,
    // The model: the agent's record, the catalogue and the picker.
    working: bool,
    tier: String,
    model: String,
    model_source: String,
    /// The override as the server last said it (`provider/model@effort`, '' = the default).
    applied: String,
    /// The picker's model (`provider/model`, '' = the default) and its reasoning effort.
    choice: String,
    effort: String,
    picker: Picker,
    /// The hub's `/v1/models` view, and the model policy's lists from `/v1/settings`.
    models: Value,
    policy_orchestrator: Vec<String>,
    policy_worker: Vec<String>,
    catalogue_loaded: bool,
    choices: Vec<Choice>,
    // Permissions.
    permission: Option<String>,
    permission_failed: bool,
    // Harness: the limit fields, the steer note and the loops.
    limits_loaded: bool,
    turns: String,
    compress: String,
    keep: String,
    steer: String,
    loops: Vec<Loop>,
    new_loop: bool,
    loop_prompt: String,
    loop_every: String,
    loop_max: String,
    // Memory: the notes, the find box, and the one being read.
    memory_loaded: bool,
    notes: Vec<Note>,
    search: String,
    reading: Option<Note>,
    // The rich content of the note being read: the hub's cards and images (`console::rich`).
    cards: rich::Cards,
    // The graph view of the same notes (a toggle beside the list).
    graph_on: bool,
    graph: super::memory_graph::Layout,
    /// The note the keyboard is on in the graph (ringed), walked with Tab.
    graph_kb: Option<String>,
    /// The debounce tick of the latest `memory` event: only the last one reloads the notes.
    memory_tick: u64,
}

/// An agent's harness limits; a missing one is the plaza default.
#[derive(Debug, Clone, Deserialize)]
pub struct Limits {
    pub max_turns: Option<u64>,
    pub compress_at: Option<u64>,
    pub keep_recent: Option<u64>,
}

/// One of this agent's loops.
#[derive(Debug, Clone, Deserialize)]
pub struct Loop {
    pub id: String,
    #[serde(default)]
    pub cid: u64,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub every_secs: u64,
    #[serde(default)]
    pub max_runs: u64,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub last_work: Option<u64>,
    #[serde(default)]
    pub last_summary: Option<String>,
}

/// One memory note; the list carries no body, a read does.
#[derive(Debug, Clone, Deserialize)]
pub struct Note {
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_ms: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub backlinks: Vec<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

/// Where a note came from: the task and the work item that wrote it.
#[derive(Debug, Clone, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub work: Option<u64>,
}

/// What a control call does to the panels once it succeeds.
#[derive(Debug, Clone, Copy)]
pub enum After {
    Keep,
    ClearSteer,
    ReloadLimits,
}

#[derive(Debug, Clone)]
pub enum SideMsg {
    /// A key the graph takes: Tab walks the notes, Enter or Space opens the one in focus.
    Key(super::memory_graph::Key),
    /// The 30-second fallback reload of the notes, while the Memory tab is shown.
    Refresh,
    Agent(Result<Value, String>),
    Catalogue(Result<(Value, Value), String>),
    /// The owner's favourites after a star in the picker.
    Favourited(Result<Vec<String>, String>),
    Picker(PickerMsg),
    ModelSaved(Result<Value, String>),
    PermissionLoaded(Result<String, String>),
    ChoosePermission(String),
    PermissionSaved(Result<String, String>),
    LimitsLoaded(Result<Limits, String>),
    TurnsIn(String),
    CompressIn(String),
    KeepIn(String),
    SaveLimits,
    SteerIn(String),
    Steer,
    CompressNow,
    ExtendTurns,
    Control {
        fallback: &'static str,
        after: After,
        result: Result<Value, String>,
    },
    Loops(Result<Vec<Loop>, String>),
    LoopPrompt(String),
    LoopEvery(String),
    LoopMax(String),
    ToggleNewLoop,
    StartLoop,
    LoopStarted(Result<Value, String>),
    StopLoop(String),
    LoopStopped(Result<Value, String>),
    Memory(Result<Vec<Note>, String>),
    Search(String),
    SearchSubmit,
    OpenNote(String),
    NoteRead(Result<Note, String>),
    /// A rich-content click or load in the note being read.
    Rich(rich::Ev),
    ToggleGraph,
    /// Shows the graph (true) or the list (false): the web's Graph and List buttons.
    SetGraph(bool),
    MemoryTick(Result<u64, String>),
    Graph(super::memory_graph::Msg),
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AgentRec {
    #[serde(default)]
    tier: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    model_source: String,
    #[serde(default)]
    model_override: Option<String>,
    #[serde(default)]
    status: String,
    /// The robot species the hub sent (read by the floor once it is wired).
    #[serde(default)]
    #[allow(dead_code)]
    species: Option<String>,
}

#[derive(Deserialize)]
struct NotesEnvelope {
    #[serde(default)]
    notes: Vec<Note>,
}

/// Loads the panels' data for agent `cid`. A new agent resets the panels; the inputs and the
/// open note survive the 4 s reloads.
pub fn load(side: &mut Side, cid: u64, ctx: &Ctx) -> Task<SideMsg> {
    if side.cid != cid {
        *side = Side {
            cid,
            ..Side::default()
        };
        side.loop_every = "30".into();
        side.loop_max = "0".into();
        side.graph_on = true;
    }
    side.error = None;
    let mut tasks = vec![
        fetch_agent(ctx, cid),
        fetch_loops(ctx, cid),
        fetch_memory(ctx, cid),
    ];
    if !side.catalogue_loaded {
        tasks.push(fetch_catalogue(ctx));
    }
    if side.permission.is_none() && !side.permission_failed {
        tasks.push(fetch_permission(ctx, cid));
    }
    if !side.limits_loaded {
        tasks.push(fetch_limits(ctx, cid));
    }
    Task::batch(tasks)
}

pub fn update(side: &mut Side, msg: SideMsg, ctx: &Ctx) -> Task<SideMsg> {
    let cid = side.cid;
    match msg {
        SideMsg::Agent(Ok(v)) | SideMsg::ModelSaved(Ok(v)) => {
            if let Ok(a) = serde_json::from_value::<AgentRec>(v) {
                apply_agent(side, a);
            }
            Task::none()
        }
        SideMsg::Agent(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::ModelSaved(Err(e)) => {
            // The save did not take: the picker goes back to what the server holds.
            side.error = Some(e);
            let applied = side.applied.clone();
            sync_choice(side, &applied);
            rebuild(side);
            Task::none()
        }
        SideMsg::Favourited(Ok(favourites)) => {
            picker::mark_favourites(&mut side.models, &favourites);
            picker::sync(&mut side.picker, &side.models);
            rebuild(side);
            Task::none()
        }
        SideMsg::Favourited(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::Catalogue(Ok((models, settings))) => {
            side.models = models;
            picker::sync(&mut side.picker, &side.models);
            side.policy_orchestrator = strings(&settings["model_policy"]["orchestrator"]);
            side.policy_worker = strings(&settings["model_policy"]["worker"]);
            side.catalogue_loaded = true;
            rebuild(side);
            Task::none()
        }
        SideMsg::Catalogue(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::Picker(m) => {
            picker::update(&mut side.picker, &m);
            match m {
                PickerMsg::Favourite(id) => {
                    let c = ctx.client.clone();
                    crate::console::client::task(
                        move || picker::save_favourite(&c, &id),
                        SideMsg::Favourited,
                    )
                }
                PickerMsg::Pick(id) => {
                    let value = with_effort(&id, &side.effort);
                    side.choice = id;
                    rebuild(side);
                    save_model(side, ctx, value)
                }
                PickerMsg::Effort(e) => {
                    // Effort on the default model pins that model for this agent, as the web does.
                    let base = if side.choice.is_empty() {
                        strip_effort(&side.model).to_string()
                    } else {
                        side.choice.clone()
                    };
                    side.effort = e.clone();
                    if base.is_empty() {
                        Task::none()
                    } else {
                        save_model(side, ctx, with_effort(&base, &e))
                    }
                }
                _ => Task::none(),
            }
        }
        SideMsg::PermissionLoaded(Ok(mode)) => {
            side.permission = Some(mode);
            Task::none()
        }
        SideMsg::PermissionLoaded(Err(_)) => {
            side.permission_failed = true;
            Task::none()
        }
        SideMsg::ChoosePermission(mode) => {
            if side.permission.is_none() || side.permission.as_deref() == Some(mode.as_str()) {
                return Task::none();
            }
            let c = ctx.client.clone();
            let path = format!("/v1/agents/{cid}/permissions");
            let m = mode.clone();
            client::task(
                move || {
                    c.call("PUT", &path, Some(&json!({ "mode": m.clone() })))
                        .map(|_| m)
                },
                SideMsg::PermissionSaved,
            )
        }
        SideMsg::PermissionSaved(Ok(mode)) => {
            side.permission = Some(mode);
            Task::none()
        }
        SideMsg::PermissionSaved(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::LimitsLoaded(Ok(l)) => {
            side.turns = l.max_turns.map(|n| n.to_string()).unwrap_or_default();
            side.compress = l.compress_at.map(kb_text).unwrap_or_default();
            side.keep = l.keep_recent.map(|n| n.to_string()).unwrap_or_default();
            side.limits_loaded = true;
            Task::none()
        }
        SideMsg::LimitsLoaded(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::TurnsIn(s) => {
            side.turns = s;
            Task::none()
        }
        SideMsg::CompressIn(s) => {
            side.compress = s;
            Task::none()
        }
        SideMsg::KeepIn(s) => {
            side.keep = s;
            Task::none()
        }
        SideMsg::SaveLimits => {
            let (turns, kb, keep) = match (
                parse_limit(&side.turns),
                parse_limit(&side.compress),
                parse_limit(&side.keep),
            ) {
                (Ok(t), Ok(k), Ok(p)) => (t, k, p),
                _ => {
                    side.error = Some("Limits must be numbers of 0 or more".into());
                    return Task::none();
                }
            };
            if turns.is_none() && kb.is_none() && keep.is_none() {
                side.error = Some("Fill in a limit to save it".into());
                return Task::none();
            }
            let mut body = json!({ "action": "set_limits" });
            if let Some(n) = turns {
                body["max_turns"] = num_json(n);
            }
            if let Some(n) = kb {
                body["compress_at"] = json!((n * 1024.0).round() as u64);
            }
            if let Some(n) = keep {
                body["keep_recent"] = num_json(n);
            }
            control(cid, ctx, body, "Limits saved", After::ReloadLimits)
        }
        SideMsg::SteerIn(s) => {
            side.steer = s;
            Task::none()
        }
        SideMsg::Steer => {
            let text = side.steer.trim().to_string();
            if text.is_empty() || !side.working {
                return Task::none();
            }
            control(
                cid,
                ctx,
                json!({ "action": "steer", "text": text }),
                "Steer sent",
                After::ClearSteer,
            )
        }
        SideMsg::CompressNow => {
            if !side.working {
                return Task::none();
            }
            control(
                cid,
                ctx,
                json!({ "action": "compress" }),
                "Compression requested",
                After::Keep,
            )
        }
        SideMsg::ExtendTurns => {
            if !side.working {
                return Task::none();
            }
            control(
                cid,
                ctx,
                json!({ "action": "extend_turns", "turns": 20 }),
                "Turns extended",
                After::Keep,
            )
        }
        SideMsg::Control {
            fallback,
            after,
            result,
        } => match result {
            Ok(v) => {
                side.notice = Some(result_text(&v, fallback));
                match after {
                    After::Keep => Task::none(),
                    After::ClearSteer => {
                        side.steer.clear();
                        Task::none()
                    }
                    After::ReloadLimits => {
                        side.limits_loaded = false;
                        fetch_limits(ctx, cid)
                    }
                }
            }
            Err(e) => {
                side.error = Some(e);
                Task::none()
            }
        },
        SideMsg::Loops(Ok(list)) => {
            let mut list = list;
            list.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
            side.loops = list;
            Task::none()
        }
        SideMsg::Loops(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::LoopPrompt(s) => {
            side.loop_prompt = s;
            Task::none()
        }
        SideMsg::LoopEvery(s) => {
            side.loop_every = s;
            Task::none()
        }
        SideMsg::LoopMax(s) => {
            side.loop_max = s;
            Task::none()
        }
        SideMsg::ToggleNewLoop => {
            side.new_loop = !side.new_loop;
            Task::none()
        }
        SideMsg::StartLoop => {
            let text = side.loop_prompt.trim().to_string();
            if text.is_empty() {
                side.error = Some("Write the loop prompt first".into());
                return Task::none();
            }
            let minutes = side
                .loop_every
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|m| *m >= 1);
            let runs = side.loop_max.trim().parse::<u64>().ok();
            let (Some(minutes), Some(runs)) = (minutes, runs) else {
                side.error = Some("Every must be at least 1 minute, and max runs 0 or more".into());
                return Task::none();
            };
            let c = ctx.client.clone();
            let body =
                json!({ "cid": cid, "prompt": text, "every_minutes": minutes, "max_runs": runs });
            client::task(
                move || c.post::<Value>("/v1/loops", &body),
                SideMsg::LoopStarted,
            )
        }
        SideMsg::LoopStarted(Ok(_)) => {
            side.loop_prompt.clear();
            side.new_loop = false;
            side.notice = Some("Loop started".into());
            fetch_loops(ctx, cid)
        }
        SideMsg::LoopStarted(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::StopLoop(id) => {
            let c = ctx.client.clone();
            let path = format!("/v1/loops/{id}");
            client::task(move || c.delete(&path), SideMsg::LoopStopped)
        }
        SideMsg::LoopStopped(Ok(_)) => {
            side.notice = Some("Loop stopped".into());
            fetch_loops(ctx, cid)
        }
        SideMsg::LoopStopped(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::Memory(Ok(list)) => {
            side.memory_loaded = true;
            // Show the newest note until one is open; refresh an open note that changed.
            let fetch = match side.reading.as_ref() {
                Some(open) => list
                    .iter()
                    .find(|n| n.slug == open.slug)
                    .filter(|n| n.updated_ms != open.updated_ms)
                    .map(|n| n.slug.clone()),
                None => list
                    .iter()
                    .max_by_key(|n| n.updated_ms)
                    .map(|n| n.slug.clone()),
            };
            if side
                .reading
                .as_ref()
                .is_some_and(|open| !list.iter().any(|n| n.slug == open.slug))
            {
                side.reading = None;
            }
            side.graph.refresh(&list);
            side.notes = list;
            match fetch {
                Some(slug) => fetch_note(ctx, cid, slug),
                None => Task::none(),
            }
        }
        SideMsg::Memory(Err(e)) => {
            side.memory_loaded = true;
            side.error = Some(e);
            Task::none()
        }
        SideMsg::Search(s) => {
            side.search = s;
            Task::none()
        }
        // Enter opens the first note whose title holds the find text, as the web does.
        SideMsg::SearchSubmit => {
            let q = side.search.trim().to_lowercase();
            match side
                .notes
                .iter()
                .find(|n| n.title.to_lowercase().contains(&q))
            {
                Some(n) => fetch_note(ctx, cid, n.slug.clone()),
                None => Task::none(),
            }
        }
        SideMsg::OpenNote(slug) => fetch_note(ctx, cid, slug),
        SideMsg::NoteRead(Ok(note)) => {
            side.reading = Some(note);
            prefetch_reading(side, ctx)
        }
        SideMsg::Rich(ev) => {
            rich::apply(&mut side.cards, ev);
            Task::none()
        }
        SideMsg::NoteRead(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
        SideMsg::ToggleGraph => {
            side.graph_on = !side.graph_on;
            Task::none()
        }
        SideMsg::SetGraph(on) => {
            side.graph_on = on;
            Task::none()
        }
        SideMsg::Graph(super::memory_graph::Msg::Open(slug)) => fetch_note(ctx, cid, slug),
        SideMsg::Graph(super::memory_graph::Msg::Drag { slug, x, y }) => {
            side.graph.hold(&slug, x, y);
            Task::none()
        }
        SideMsg::Graph(super::memory_graph::Msg::Drop) => {
            side.graph.release();
            Task::none()
        }
        SideMsg::Key(super::memory_graph::Key::Tab { back }) => {
            side.graph_kb = super::memory_graph::walk(&side.graph, side.graph_kb.as_deref(), back);
            Task::none()
        }
        SideMsg::Key(super::memory_graph::Key::Open) => match side.graph_kb.clone() {
            Some(slug) => fetch_note(ctx, cid, slug),
            None => Task::none(),
        },
        SideMsg::Refresh => fetch_memory(ctx, cid),
        SideMsg::MemoryTick(Ok(tick)) => {
            if tick == side.memory_tick {
                fetch_memory(ctx, cid)
            } else {
                Task::none()
            }
        }
        SideMsg::MemoryTick(Err(_)) => Task::none(),
    }
}

/// A rich-content click or load, as the side column's message.
fn on_rich(action: rich::RichAction) -> SideMsg {
    SideMsg::Rich(rich::Ev::Action(action))
}

/// Asks the hub for the cards of the note being read that it does not hold yet.
fn prefetch_reading(side: &mut Side, ctx: &Ctx) -> Task<SideMsg> {
    let Some(note) = side.reading.as_ref() else {
        return Task::none();
    };
    let body = note.body.clone();
    let rctx = rich::context(&side.cards, None, None);
    rich::prefetch(&mut side.cards, &ctx.client, &rctx, &[body.as_str()]).map(SideMsg::Rich)
}

/// The side column (the web's `aside.ag-side`, minus the conduct and checkpoint cards the page draws): the model, the
/// harness and the permissions, 12 apart.
pub fn view<'a>(side: &'a Side, _ctx: &'a Ctx) -> Element<'a, SideMsg> {
    let mut parts: Vec<Element<'a, SideMsg>> = Vec::new();
    if let Some(e) = &side.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &side.notice {
        parts.push(ui::faint(n.as_str()).into());
    }
    parts.push(model_card(side));
    parts.push(harness_card(side));
    parts.push(permission_card(side));
    column(parts).spacing(GAP).into()
}

// ---- Views ----

/// The gap between the blocks of a card and between the side's cards (the web's `--gap`).
const GAP: f32 = 12.0;

/// A block heading inside a card (the web's `h4`): sans semibold at the item size, in ink.
fn sub_head<'a>(title: &str) -> Element<'a, SideMsg> {
    text(title.to_string())
        .font(SANS_SEMIBOLD)
        .size(TYPE_ITEM)
        .color(crate::theme::palette().ink)
        .into()
}

/// A field's label (the web's `.field-l`): sans medium at 12.5 in ink-2.
fn field_label<'a>(label: &str) -> Element<'a, SideMsg> {
    text(label.to_string())
        .font(SANS_MEDIUM)
        .size(12.5)
        .color(crate::theme::palette().ink_2)
        .into()
}

/// A labelled control (the web's `.field`): the label, the control and an optional hint under it.
fn labelled<'a>(
    label: &str,
    control: Element<'a, SideMsg>,
    hint: Option<&str>,
) -> Element<'a, SideMsg> {
    let mut items = vec![field_label(label), control];
    if let Some(hint) = hint {
        items.push(ui::faint(hint.to_string()).into());
    }
    column(items).spacing(5).width(Length::Fill).into()
}

fn model_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let source = match side.model_source.as_str() {
        "agent" => "set for this agent",
        "project" => "from its project",
        "role" => "from role",
        _ => "default",
    };
    let current = row![
        text(side.model.clone())
            .font(MONO_MEDIUM)
            .size(TYPE_ITEM)
            .color(crate::theme::palette().ink),
        ui::faint(source)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let choose = picker::view(
        &side.picker,
        &side.choices,
        side.choice.as_str(),
        Some(side.effort.as_str()),
        SideMsg::Picker,
    );
    ui::card(
        "Model",
        column![current, labelled("Change model", choose, None)]
            .spacing(GAP)
            .into(),
    )
}

fn harness_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let working = side.working;
    let go = |m: SideMsg| working.then_some(m);

    // Limits: three fields across, then Save limits at the right (the web's `.ag-grid3` and `.cluster`).
    let limits = column![
        sub_head("Limits"),
        row![
            field(
                "Turns per work item",
                "default",
                &side.turns,
                SideMsg::TurnsIn
            ),
            field(
                "Compress at (KB)",
                "default",
                &side.compress,
                SideMsg::CompressIn
            ),
            field("Keep recent turns", "default", &side.keep, SideMsg::KeepIn),
        ]
        .spacing(GAP),
        row![
            Space::new().width(Length::Fill),
            ui::btn_sm(
                "Save limits",
                None,
                ui::Kind::Primary,
                Some(SideMsg::SaveLimits)
            )
        ],
    ]
    .spacing(GAP);

    // Running work: enabled only while the agent is working.
    let mut running: Vec<Element<'a, SideMsg>> = vec![sub_head("Running work")];
    if !working {
        running.push(ui::faint("Available while the agent is working.").into());
    }
    running.push(
        row![
            ui::btn(
                "Compress now",
                None,
                ui::Kind::Secondary,
                go(SideMsg::CompressNow)
            ),
            ui::btn(
                "+20 turns",
                None,
                ui::Kind::Secondary,
                go(SideMsg::ExtendTurns)
            ),
        ]
        .spacing(8)
        .into(),
    );
    running.push(
        row![
            text_input("Steer the current work…", &side.steer)
                .on_input(SideMsg::SteerIn)
                .on_submit(SideMsg::Steer)
                .style(crate::theme::input)
                .width(Length::Fill),
            ui::btn_sm("Steer", None, ui::Kind::Primary, go(SideMsg::Steer)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );

    // Loops: this agent's, newest first, each under a hairline; the new one opens in its dialog.
    let mut loops: Vec<Element<'a, SideMsg>> = vec![
        sub_head("Loops"),
        ui::btn(
            "New loop",
            None,
            ui::Kind::Secondary,
            Some(SideMsg::ToggleNewLoop),
        ),
    ];
    if side.loops.is_empty() {
        loops.push(ui::faint("No loops for this agent.").into());
    }
    for l in &side.loops {
        loops.push(ui::hairline());
        loops.push(loop_row(l));
    }

    ui::card(
        "Harness",
        column![
            limits,
            column(running).spacing(8),
            column(loops).spacing(GAP)
        ]
        .spacing(GAP)
        .into(),
    )
}

fn permission_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let body: Element<'a, SideMsg> = if side.permission_failed {
        ui::faint("Could not load this setting").into()
    } else if side.permission.is_none() {
        ui::faint("Loading…").into()
    } else {
        let items: Vec<Element<'a, SideMsg>> = PERMISSIONS
            .into_iter()
            .map(|(mode, label, hint)| {
                let active = side.permission.as_deref() == Some(mode);
                choice(
                    label,
                    hint,
                    active,
                    SideMsg::ChoosePermission(mode.to_string()),
                )
            })
            .collect();
        column(items).spacing(GAP).into()
    };
    ui::card("Permissions", body)
}

/// One permission mode as the web's `.ag-choice`: a full-width bordered button, its name over its hint, the chosen
/// one tinted in signal.
fn choice<'a>(name: &str, hint: &str, active: bool, msg: SideMsg) -> Element<'a, SideMsg> {
    let p = crate::theme::palette();
    let body = column![
        text(name.to_string())
            .font(SANS_SEMIBOLD)
            .size(TYPE_ITEM)
            .color(p.ink),
        text(hint.to_string())
            .font(SANS)
            .size(TYPE_ITEM)
            .color(p.ink_3),
    ]
    .spacing(4)
    .width(Length::Fill);
    button(body)
        .padding(12)
        .width(Length::Fill)
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: Some(if active { p.signal_soft } else { p.sheet }.into()),
            text_color: p.ink,
            border: Border {
                color: if active { p.signal } else { p.rule },
                width: 1.0,
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// A `memory` event from the hub: the notes reload once the burst has settled (250 ms, as the web), so a run of
/// writes costs one fetch.
pub fn memory_changed(side: &mut Side) -> Task<SideMsg> {
    side.memory_tick += 1;
    let tick = side.memory_tick;
    client::task(
        move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            Ok::<u64, String>(tick)
        },
        SideMsg::MemoryTick,
    )
}

/// The note a backlink or `[[link]]` names: its slug, or the slug of its title (the web's `resolveRef`).
fn resolve_ref<'a>(notes: &'a [Note], r: &str) -> Option<&'a Note> {
    let want = slugify(r);
    notes
        .iter()
        .find(|n| n.slug == r || n.slug == want || slugify(&n.title) == want)
}

/// The web's slug rule: lower case, runs of other characters become one dash, no edge dashes, at most 60.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(60).collect()
}

/// The Memory tab (`memory.ts`): a card with the note count and the find box, then Graph or List, and the chosen note
/// read beside the graph (or under the table).
pub fn memory_view<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let count = if side.memory_loaded {
        let n = side.notes.len();
        format!("{n} {}", if n == 1 { "note" } else { "notes" })
    } else {
        String::new()
    };
    let find = text_input("Find a note…", &side.search)
        .on_input(SideMsg::Search)
        .on_submit(SideMsg::SearchSubmit)
        .style(crate::theme::input)
        .width(Length::Fixed(208.0));
    let head = row![muted(count), find]
        .spacing(8)
        .align_y(Alignment::Center);
    let body: Element<'a, SideMsg> = if !side.memory_loaded {
        muted("Loading memory…")
    } else if side.notes.is_empty() {
        ui::empty(
            "brain",
            "No memories yet",
            "Notes the agent writes appear here, linked like a little brain.",
        )
    } else {
        memory_body(side)
    };
    container(column![head, body].spacing(GAP))
        .padding(16)
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// The Graph and List choices and what they show for the find text.
fn memory_body<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let q = side.search.trim().to_lowercase();
    let toolbar = row![
        ui::btn_sm(
            "Graph",
            None,
            ui::Kind::Ghost,
            Some(SideMsg::SetGraph(true))
        ),
        ui::btn_sm(
            "List",
            None,
            ui::Kind::Ghost,
            Some(SideMsg::SetGraph(false))
        ),
    ]
    .spacing(4);
    let mut matched: Vec<&Note> = side
        .notes
        .iter()
        .filter(|n| q.is_empty() || n.title.to_lowercase().contains(&q))
        .collect();
    matched.sort_by_key(|n| std::cmp::Reverse(n.updated_ms));
    if matched.is_empty() {
        return column![toolbar, muted("No notes match.")]
            .spacing(GAP)
            .into();
    }
    let open = side.reading.as_ref().map(|n| n.slug.as_str());
    if side.graph_on {
        let graph = super::memory_graph::view(&side.graph, q, side.graph_kb.as_deref(), open)
            .map(SideMsg::Graph);
        let left = column![graph, ui::faint(GRAPH_HINT)]
            .spacing(8)
            .width(Length::FillPortion(13));
        let right = container(reader(side)).width(Length::FillPortion(10));
        column![toolbar, row![left, right].spacing(GAP)]
            .spacing(GAP)
            .into()
    } else {
        column![toolbar, note_table(&matched, open), reader(side)]
            .spacing(GAP)
            .into()
    }
}

const GRAPH_HINT: &str =
    "Click a note to read it. Solid lines are links; dashed lines join notes that share a tag.";

/// A line of secondary text at the item size (the web's `.muted`).
fn muted<'a>(content: impl Into<String>) -> Element<'a, SideMsg> {
    text(content.into())
        .font(SANS)
        .size(TYPE_ITEM)
        .color(crate::theme::palette().ink_3)
        .into()
}

/// The open note, or the prompt to pick one.
fn reader<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    match &side.reading {
        Some(doc) => {
            let rctx = rich::context(&side.cards, None, None);
            reader_view(doc, &side.notes, &rctx, &side.cards)
        }
        None => muted("Pick a note to read it."),
    }
}

/// One cell of a table: padded 9 by 12, its first and last cells flush with the card's text.
fn cell<'a>(
    content: Element<'a, SideMsg>,
    portion: u16,
    first: bool,
    last: bool,
) -> Element<'a, SideMsg> {
    container(content)
        .padding(Padding {
            top: 9.0,
            right: if last { 0.0 } else { 12.0 },
            bottom: 9.0,
            left: if first { 0.0 } else { 12.0 },
        })
        .width(Length::FillPortion(portion))
        .into()
}

/// The List view as the web's `.tbl`: the labels over a strong rule, a ruled row per note, the open one tinted, a click
/// opening it.
fn note_table<'a>(notes: &[&'a Note], open: Option<&str>) -> Element<'a, SideMsg> {
    let p = crate::theme::palette();
    let label = |s: &str, portion: u16, first: bool, last: bool| {
        cell(
            text(s.to_string())
                .font(SANS_MEDIUM)
                .size(TYPE_META)
                .color(p.ink_3)
                .into(),
            portion,
            first,
            last,
        )
    };
    let head = row![
        label("Title", 4, true, false),
        label("Kind", 2, false, false),
        label("Tags", 3, false, false),
        label("Updated", 2, false, true),
    ];
    let mut lines: Vec<Element<'a, SideMsg>> = vec![
        head.into(),
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_strong.into()),
                ..Default::default()
            })
            .into(),
    ];
    for n in notes {
        let tags: Vec<Element<'a, SideMsg>> = n
            .tags
            .iter()
            .map(|t| ui::badge(&format!("#{t}"), ui::Tone::Muted))
            .collect();
        let content = row![
            cell(
                text(n.title.clone())
                    .font(SANS)
                    .size(TYPE_ITEM)
                    .color(p.ink)
                    .into(),
                4,
                true,
                false,
            ),
            cell(ui::badge(&n.kind, kind_tone(&n.kind)), 2, false, false),
            cell(ui::chip_row(tags), 3, false, false),
            cell(ui::faint(ago(n.updated_ms)).into(), 2, false, true),
        ]
        .align_y(Alignment::Center);
        let is_open = open == Some(n.slug.as_str());
        lines.push(
            button(content)
                .padding(0)
                .width(Length::Fill)
                .style(move |_: &Theme, status: button::Status| {
                    let hovered =
                        matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: (hovered || is_open)
                            .then(|| crate::theme::palette().hover.into()),
                        text_color: crate::theme::palette().ink,
                        border: Border::default(),
                        ..Default::default()
                    }
                })
                .on_press(SideMsg::OpenNote(n.slug.clone()))
                .into(),
        );
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// The tone a note's kind is drawn in (the web's `KIND_T`).
fn kind_tone(kind: &str) -> ui::Tone {
    match kind {
        "person" => ui::Tone::Signal,
        "journal" => ui::Tone::Warn,
        _ => ui::Tone::Info,
    }
}

/// A loop as the web's `.ag-row`: the prompt, its state and runs, the last summary, and Stop while it runs.
fn loop_row<'a>(l: &'a Loop) -> Element<'a, SideMsg> {
    let tone = match l.state.as_str() {
        "running" => ui::Tone::Info,
        "done" => ui::Tone::Ok,
        _ => ui::Tone::Muted,
    };
    let max = if l.max_runs > 0 {
        l.max_runs.to_string()
    } else {
        "∞".to_string()
    };
    let mut meta = format!("every {} min · runs {}/{}", l.every_secs / 60, l.runs, max);
    if let Some(at) = l.last_work {
        meta.push_str(&format!(" · last {}", ago(at)));
    }
    let mut lines: Vec<Element<'a, SideMsg>> = vec![
        text(clip(&l.prompt, 80))
            .font(SANS)
            .size(TYPE_ITEM)
            .color(crate::theme::palette().ink)
            .into(),
        row![ui::badge(&l.state, tone), ui::faint(meta)]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    ];
    if let Some(summary) = &l.last_summary {
        lines.push(ui::faint(clip(summary, 120)).into());
    }
    let mut line = row![column(lines).spacing(4).width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Start);
    if l.state == "running" {
        line = line.push(ui::btn_sm(
            "Stop",
            None,
            ui::Kind::Danger,
            Some(SideMsg::StopLoop(l.id.clone())),
        ));
    }
    container(line)
        .padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 8.0,
            left: 0.0,
        })
        .width(Length::Fill)
        .into()
}

/// One note open: its kind, title and stamp (when it was updated and by which work), its tags as chips, its body, and
/// the notes that link to it (the web's `.mem-read`).
fn reader_view<'a>(
    doc: &'a Note,
    notes: &'a [Note],
    rctx: &rich::RichCtx,
    cards: &'a rich::Cards,
) -> Element<'a, SideMsg> {
    let mut stamp = format!("Updated {}", ago(doc.updated_ms));
    if let Some(p) = &doc.provenance {
        if let Some(task) = &p.task {
            stamp.push_str(&format!(" · task {task}"));
        }
        if let Some(work) = p.work {
            stamp.push_str(&format!(" · work #{work}"));
        }
    }
    let mut items: Vec<Element<'a, SideMsg>> = vec![
        row![
            ui::badge(&doc.kind, kind_tone(&doc.kind)),
            text(doc.title.clone())
                .font(SANS_SEMIBOLD)
                .size(TYPE_ITEM)
                .color(crate::theme::palette().ink),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        ui::faint(stamp).into(),
    ];
    if !doc.tags.is_empty() {
        let tags: Vec<Element<'a, SideMsg>> = doc
            .tags
            .iter()
            .map(|t| ui::badge(&format!("#{t}"), ui::Tone::Muted))
            .collect();
        items.push(ui::chip_row(tags));
    }
    items.push(rich::view(&doc.body, rctx, cards, &on_rich));
    if !doc.backlinks.is_empty() {
        // Each backlink opens its note; one that names no note yet reads as a plain chip.
        let chips: Vec<Element<'a, SideMsg>> = doc
            .backlinks
            .iter()
            .map(|b| match resolve_ref(notes, b) {
                Some(n) => link_chip(&n.title, n.slug.clone()),
                None => ui::badge(b, ui::Tone::Muted),
            })
            .collect();
        items.push(
            column![sub_head("Linked from"), ui::chip_row(chips)]
                .spacing(8)
                .into(),
        );
    }
    column(items).spacing(8).into()
}

/// A note's title as a chip that opens the note (the web's `.chip.t-info` button).
fn link_chip<'a>(title: &str, slug: String) -> Element<'a, SideMsg> {
    let ink = crate::theme::palette().ink;
    button(ui::badge(title, ui::Tone::Info))
        .padding(0)
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: None,
            text_color: ink,
            border: Border::default(),
            ..Default::default()
        })
        .on_press(SideMsg::OpenNote(slug))
        .into()
}

/// A labelled limit field: a label over an input, the placeholder saying the plaza default applies when it is blank.
fn field<'a>(
    label: &str,
    placeholder: &'static str,
    value: &'a str,
    on_input: fn(String) -> SideMsg,
) -> Element<'a, SideMsg> {
    labelled(
        label,
        text_input(placeholder, value)
            .on_input(on_input)
            .style(crate::theme::input)
            .width(Length::Fill)
            .into(),
        None,
    )
}

/// The New loop dialog (the web's `modal('New loop')`): the prompt, the minutes between runs and the most runs (0 until
/// the agent says LOOP DONE), with Start loop at the foot of the body. `None` while it is closed; the page lays it over
/// its screen with `ui::with_modal`.
pub fn dialog<'a>(side: &'a Side) -> Option<Element<'a, SideMsg>> {
    if !side.new_loop {
        return None;
    }
    let prompt = labelled(
        "Prompt",
        text_input("What the agent should do on each run…", &side.loop_prompt)
            .on_input(SideMsg::LoopPrompt)
            .on_submit(SideMsg::StartLoop)
            .style(crate::theme::input)
            .width(Length::Fill)
            .into(),
        None,
    );
    let every = labelled(
        "Every (minutes)",
        text_input("30", &side.loop_every)
            .on_input(SideMsg::LoopEvery)
            .style(crate::theme::input)
            .width(Length::Fill)
            .into(),
        None,
    );
    let max = labelled(
        "Max runs",
        text_input("0", &side.loop_max)
            .on_input(SideMsg::LoopMax)
            .style(crate::theme::input)
            .width(Length::Fill)
            .into(),
        Some("0 = until the agent says LOOP DONE"),
    );
    let body = column![
        prompt,
        every,
        max,
        row![
            Space::new().width(Length::Fill),
            ui::btn_sm(
                "Start loop",
                None,
                ui::Kind::Primary,
                Some(SideMsg::StartLoop)
            ),
        ],
    ]
    .spacing(GAP);
    Some(ui::modal(
        "New loop",
        body.into(),
        Vec::new(),
        SideMsg::ToggleNewLoop,
        460.0,
    ))
}

// ---- Model and effort helpers (`agent-detail.ts`) ----

/// The effort a `provider/model@effort` value pins ('' when none).
fn effort_of(value: &str) -> &str {
    match value.rsplit_once('@') {
        Some((_, e)) if EFFORTS.contains(&e) => e,
        _ => "",
    }
}

fn strip_effort(value: &str) -> &str {
    match value.rsplit_once('@') {
        Some((base, e)) if EFFORTS.contains(&e) => base,
        _ => value,
    }
}

fn with_effort(value: &str, effort: &str) -> String {
    if value.is_empty() {
        String::new()
    } else if effort.is_empty() {
        value.to_string()
    } else {
        format!("{}@{effort}", strip_effort(value))
    }
}

/// Puts the server's override into the picker: its model and its effort.
fn sync_choice(side: &mut Side, value: &str) {
    side.choice = strip_effort(value).to_string();
    side.effort = effort_of(value).to_string();
}

/// The picker's choices, as the web builds them: a worker may run the policy's worker list
/// first, then every model the providers list; an orchestrator only its policy list. The
/// default comes first, and an override no longer offered is kept as a choice.
fn rebuild(side: &mut Side) {
    let all = picker::choices(&side.models);
    let orchestrator = side.tier == "orchestrator";
    let policy: &[String] = if orchestrator {
        &side.policy_orchestrator
    } else {
        &side.policy_worker
    };
    let label_of = |id: &str| {
        all.iter()
            .find(|c| c.id == id)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let provider_of = |id: &str| id.split('/').next().unwrap_or_default().to_string();

    let mut out = vec![Choice {
        id: String::new(),
        provider: String::new(),
        label: "Use the default".into(),
    }];
    for id in policy {
        if !out.iter().any(|c| c.id == *id) {
            out.push(Choice {
                id: id.clone(),
                provider: provider_of(id.as_str()),
                label: label_of(id.as_str()),
            });
        }
    }
    if !orchestrator {
        for c in &all {
            if !out.iter().any(|o| o.id == c.id) {
                out.push(c.clone());
            }
        }
    }
    if !side.choice.is_empty() && !out.iter().any(|o| o.id == side.choice) {
        out.push(Choice {
            id: side.choice.clone(),
            provider: provider_of(side.choice.as_str()),
            label: side.choice.clone(),
        });
    }
    side.choices = out;
}

fn apply_agent(side: &mut Side, a: AgentRec) {
    side.tier = a.tier;
    side.model = a.model;
    side.model_source = a.model_source;
    side.working = a.status == "working";
    let want = a.model_override.unwrap_or_default();
    if want != side.applied {
        sync_choice(side, &want);
        side.applied = want;
    }
    rebuild(side);
}

fn save_model(side: &Side, ctx: &Ctx, value: String) -> Task<SideMsg> {
    let c = ctx.client.clone();
    let path = format!("/v1/agents/{}", side.cid);
    let model = if value.is_empty() {
        Value::Null
    } else {
        Value::String(value)
    };
    let body = json!({ "model": model });
    client::task(move || c.patch::<Value>(&path, &body), SideMsg::ModelSaved)
}

// ---- Harness helpers (`agent-harness.ts`) ----

/// A limit field's number: `Ok(None)` when blank, `Err` when not a number of 0 or more.
fn parse_limit(raw: &str) -> Result<Option<f64>, ()> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<f64>() {
        Ok(n) if n.is_finite() && n >= 0.0 => Ok(Some(n)),
        _ => Err(()),
    }
}

fn num_json(n: f64) -> Value {
    if n.fract() == 0.0 {
        json!(n as u64)
    } else {
        json!(n)
    }
}

/// Bytes as the KB field shows them (one decimal at most).
fn kb_text(bytes: u64) -> String {
    let kb = (bytes as f64 / 1024.0 * 10.0).round() / 10.0;
    if kb.fract() == 0.0 {
        format!("{}", kb as u64)
    } else {
        format!("{kb}")
    }
}

/// The text a control call's result shows, or the fallback when the result says nothing.
fn result_text(v: &Value, fallback: &str) -> String {
    let text = v
        .as_str()
        .or_else(|| v.get("message").and_then(Value::as_str))
        .or_else(|| v.get("note").and_then(Value::as_str));
    text.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---- Requests ----

fn control(
    cid: u64,
    ctx: &Ctx,
    body: Value,
    fallback: &'static str,
    after: After,
) -> Task<SideMsg> {
    let c = ctx.client.clone();
    let path = format!("/v1/agents/{cid}/control");
    client::task(
        move || c.post::<Value>(&path, &body),
        move |result| SideMsg::Control {
            fallback,
            after,
            result,
        },
    )
}

fn fetch_agent(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Value>(&format!("/v1/agents/{cid}")),
        SideMsg::Agent,
    )
}

fn fetch_catalogue(ctx: &Ctx) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || -> Result<(Value, Value), String> {
            Ok((c.get("/v1/models")?, c.get("/v1/settings")?))
        },
        SideMsg::Catalogue,
    )
}

fn fetch_permission(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            c.get::<Value>(&format!("/v1/agents/{cid}/permissions"))
                .map(|v| v["mode"].as_str().unwrap_or_default().to_string())
        },
        SideMsg::PermissionLoaded,
    )
}

fn fetch_limits(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Limits>(&format!("/v1/agents/{cid}/limits")),
        SideMsg::LimitsLoaded,
    )
}

fn fetch_loops(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            c.get::<Vec<Loop>>("/v1/loops").map(|all| {
                all.into_iter()
                    .filter(|l| l.cid == cid)
                    .collect::<Vec<Loop>>()
            })
        },
        SideMsg::Loops,
    )
}

fn fetch_memory(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || {
            c.get::<NotesEnvelope>(&format!("/v1/agents/{cid}/memory"))
                .map(|e| e.notes)
        },
        SideMsg::Memory,
    )
}

fn fetch_note(ctx: &Ctx, cid: u64, slug: String) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Note>(&format!("/v1/agents/{cid}/memory/{slug}")),
        SideMsg::NoteRead,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_pins_ride_on_the_model_name() {
        assert_eq!(effort_of("openai/gpt@high"), "high");
        assert_eq!(effort_of("openai/gpt@turbo"), "");
        assert_eq!(strip_effort("openai/gpt@high"), "openai/gpt");
        assert_eq!(with_effort("openai/gpt", "max"), "openai/gpt@max");
        assert_eq!(with_effort("", "max"), "");
    }
}
