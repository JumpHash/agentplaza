//! The agent page's side panels (`agent-detail.ts` side column, `agent-harness.ts`,
//! `memory.ts`): model and reasoning effort (the model picker), harness limits and loops,
//! permissions, and memory.

use iced::widget::{column, row, text_input};
use iced::{Element, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::console::picker::{self, Choice, Picker, PickerMsg};
use crate::console::{client, ui, Ctx};

/// The reasoning efforts a model name can pin (`provider/model@effort`).
const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// The permission modes as the web offers them: (mode, label, hint).
const PERMISSIONS: [(&str, &str, &str); 3] = [
    ("autonomous", "Autonomous", "acts on its own"),
    ("ask", "Ask first", "write and irreversible actions wait for your approval in Approvals"),
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
    loop_prompt: String,
    loop_every: String,
    loop_max: String,
    // Memory: the notes and the one being read.
    memory_loaded: bool,
    notes: Vec<Note>,
    reading: Option<Note>,
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
    pub body: String,
    #[serde(default)]
    pub backlinks: Vec<String>,
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
    Agent(Result<Value, String>),
    Catalogue(Result<(Value, Value), String>),
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
    Control { fallback: &'static str, after: After, result: Result<Value, String> },
    Loops(Result<Vec<Loop>, String>),
    LoopPrompt(String),
    LoopEvery(String),
    LoopMax(String),
    StartLoop,
    LoopStarted(Result<Value, String>),
    StopLoop(String),
    LoopStopped(Result<Value, String>),
    Memory(Result<Vec<Note>, String>),
    OpenNote(String),
    NoteRead(Result<Note, String>),
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
        *side = Side { cid, ..Side::default() };
        side.loop_every = "30".into();
        side.loop_max = "0".into();
    }
    side.error = None;
    let mut tasks = vec![fetch_agent(ctx, cid), fetch_loops(ctx, cid), fetch_memory(ctx, cid)];
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
        SideMsg::Catalogue(Ok((models, settings))) => {
            side.models = models;
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
                PickerMsg::Pick(id) => {
                    let value = with_effort(&id, &side.effort);
                    side.choice = id;
                    rebuild(side);
                    save_model(side, ctx, value)
                }
                PickerMsg::Effort(e) => {
                    // Effort on the default model pins that model for this agent, as the web does.
                    let base = if side.choice.is_empty() { strip_effort(&side.model).to_string() } else { side.choice.clone() };
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
            client::task(move || c.call("PUT", &path, Some(&json!({ "mode": m.clone() }))).map(|_| m), SideMsg::PermissionSaved)
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
            let (turns, kb, keep) = match (parse_limit(&side.turns), parse_limit(&side.compress), parse_limit(&side.keep)) {
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
            control(cid, ctx, json!({ "action": "steer", "text": text }), "Steer sent", After::ClearSteer)
        }
        SideMsg::CompressNow => {
            if !side.working {
                return Task::none();
            }
            control(cid, ctx, json!({ "action": "compress" }), "Compression requested", After::Keep)
        }
        SideMsg::ExtendTurns => {
            if !side.working {
                return Task::none();
            }
            control(cid, ctx, json!({ "action": "extend_turns", "turns": 20 }), "Turns extended", After::Keep)
        }
        SideMsg::Control { fallback, after, result } => match result {
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
            list.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
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
        SideMsg::StartLoop => {
            let text = side.loop_prompt.trim().to_string();
            if text.is_empty() {
                side.error = Some("Write the loop prompt first".into());
                return Task::none();
            }
            let minutes = side.loop_every.trim().parse::<u64>().ok().filter(|m| *m >= 1);
            let runs = side.loop_max.trim().parse::<u64>().ok();
            let (Some(minutes), Some(runs)) = (minutes, runs) else {
                side.error = Some("Every must be at least 1 minute, and max runs 0 or more".into());
                return Task::none();
            };
            let c = ctx.client.clone();
            let body = json!({ "cid": cid, "prompt": text, "every_minutes": minutes, "max_runs": runs });
            client::task(move || c.post::<Value>("/v1/loops", &body), SideMsg::LoopStarted)
        }
        SideMsg::LoopStarted(Ok(_)) => {
            side.loop_prompt.clear();
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
                Some(open) => list.iter().find(|n| n.slug == open.slug).filter(|n| n.updated_ms != open.updated_ms).map(|n| n.slug.clone()),
                None => list.iter().max_by_key(|n| n.updated_ms).map(|n| n.slug.clone()),
            };
            if side.reading.as_ref().is_some_and(|open| !list.iter().any(|n| n.slug == open.slug)) {
                side.reading = None;
            }
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
        SideMsg::OpenNote(slug) => fetch_note(ctx, cid, slug),
        SideMsg::NoteRead(Ok(note)) => {
            side.reading = Some(note);
            Task::none()
        }
        SideMsg::NoteRead(Err(e)) => {
            side.error = Some(e);
            Task::none()
        }
    }
}

/// The side column.
pub fn view<'a>(side: &'a Side, ctx: &'a Ctx) -> Element<'a, SideMsg> {
    let _ = ctx;
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
    parts.push(memory_card(side));
    column(parts).spacing(16).into()
}

// ---- Views ----

fn model_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let source = match side.model_source.as_str() {
        "agent" => "set for this agent",
        "role" => "from role",
        _ => "default",
    };
    let current = row![ui::mono(side.model.clone(), 12.0, ui::INK), ui::faint(source)].spacing(8);
    let choose = picker::view(&side.picker, &side.choices, side.choice.as_str(), Some(side.effort.as_str()), SideMsg::Picker);
    ui::card("Model", column![current, ui::label("Change model"), choose].spacing(8).into())
}

fn harness_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let working = side.working;
    let go = |m: SideMsg| working.then_some(m);

    let limits = column![
        field("Turns per work item", "default", &side.turns, SideMsg::TurnsIn),
        field("Compress at (KB)", "default", &side.compress, SideMsg::CompressIn),
        field("Keep recent turns", "default", &side.keep, SideMsg::KeepIn),
        ui::btn("Save limits", None, ui::Kind::Primary, Some(SideMsg::SaveLimits)),
    ]
    .spacing(10);

    let mut running: Vec<Element<'a, SideMsg>> = Vec::new();
    if !working {
        running.push(ui::faint("Available while the agent is working.").into());
    }
    running.push(
        row![
            ui::btn("Compress now", None, ui::Kind::Secondary, go(SideMsg::CompressNow)),
            ui::btn("+20 turns", None, ui::Kind::Secondary, go(SideMsg::ExtendTurns)),
        ]
        .spacing(8)
        .into(),
    );
    running.push(
        row![
            text_input("Steer the current work…", &side.steer).on_input(SideMsg::SteerIn).on_submit(SideMsg::Steer).style(crate::theme::input),
            ui::btn("Steer", None, ui::Kind::Primary, go(SideMsg::Steer)),
        ]
        .spacing(8)
        .into(),
    );

    let loops: Vec<Element<'a, SideMsg>> = side.loops.iter().map(loop_row).collect();
    let loop_list: Element<'a, SideMsg> = if side.loops.is_empty() {
        ui::faint("No loops for this agent.").into()
    } else {
        column(loops).spacing(10).into()
    };

    let new_loop = column![
        text_input("What the agent should do on each run…", &side.loop_prompt)
            .on_input(SideMsg::LoopPrompt)
            .on_submit(SideMsg::StartLoop)
            .style(crate::theme::input),
        row![
            field("Every (minutes)", "30", &side.loop_every, SideMsg::LoopEvery),
            field("Max runs", "0 = until the agent says LOOP DONE", &side.loop_max, SideMsg::LoopMax),
        ]
        .spacing(8),
        ui::btn("Start loop", None, ui::Kind::Primary, Some(SideMsg::StartLoop)),
    ]
    .spacing(8);

    ui::card(
        "Harness",
        column![
            ui::label("Limits"),
            limits,
            ui::label("Running work"),
            column(running).spacing(8),
            ui::label("Loops"),
            loop_list,
            ui::label("New loop"),
            new_loop,
        ]
        .spacing(12)
        .into(),
    )
}

fn permission_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let body: Element<'a, SideMsg> = if side.permission_failed {
        ui::faint("Could not load this setting").into()
    } else if side.permission.is_none() {
        ui::faint("Loading…").into()
    } else {
        let mut items: Vec<Element<'a, SideMsg>> = Vec::new();
        for (mode, label, hint) in PERMISSIONS {
            let active = side.permission.as_deref() == Some(mode);
            let kind = if active { ui::Kind::Primary } else { ui::Kind::Secondary };
            items.push(column![ui::btn(label, None, kind, Some(SideMsg::ChoosePermission(mode.to_string()))), ui::faint(hint)].spacing(4).into());
        }
        column(items).spacing(10).into()
    };
    ui::card("Permissions", body)
}

fn memory_card<'a>(side: &'a Side) -> Element<'a, SideMsg> {
    let body: Element<'a, SideMsg> = if !side.memory_loaded {
        ui::loading()
    } else if side.notes.is_empty() {
        ui::empty("memory", "No memories yet", "Notes the agent writes appear here, linked like a little brain.")
    } else {
        let rows: Vec<Element<'a, SideMsg>> = side
            .notes
            .iter()
            .map(|n| {
                let open = side.reading.as_ref().is_some_and(|r| r.slug == n.slug);
                note_row(n, open)
            })
            .collect();
        let reader: Element<'a, SideMsg> = match &side.reading {
            Some(doc) => reader_view(doc),
            None => ui::faint("Choose a note to read it.").into(),
        };
        let count = format!("{} {}", side.notes.len(), if side.notes.len() == 1 { "note" } else { "notes" });
        column![ui::faint(count), column(rows).spacing(8), reader].spacing(12).into()
    };
    ui::card("Memory", body)
}

fn loop_row<'a>(l: &'a Loop) -> Element<'a, SideMsg> {
    let tone = match l.state.as_str() {
        "running" => ui::Tone::Info,
        "done" => ui::Tone::Ok,
        _ => ui::Tone::Muted,
    };
    let max = if l.max_runs > 0 { l.max_runs.to_string() } else { "∞".to_string() };
    let mut meta = row![
        ui::badge(&l.state, tone),
        ui::faint(format!("every {} min", l.every_secs / 60)),
        ui::faint(format!("runs {}/{}", l.runs, max)),
    ]
    .spacing(8);
    if l.state == "running" {
        meta = meta.push(ui::btn("Stop", None, ui::Kind::Danger, Some(SideMsg::StopLoop(l.id.clone()))));
    }
    let mut items: Vec<Element<'a, SideMsg>> = vec![ui::mono(clip(&l.prompt, 80), 12.0, ui::INK).into(), meta.into()];
    if let Some(summary) = &l.last_summary {
        items.push(ui::faint(clip(summary, 120)).into());
    }
    column(items).spacing(4).into()
}

fn note_row<'a>(n: &'a Note, open: bool) -> Element<'a, SideMsg> {
    let tone = match n.kind.as_str() {
        "person" => ui::Tone::Signal,
        "journal" => ui::Tone::Warn,
        _ => ui::Tone::Info,
    };
    let kind = if open { ui::Kind::Primary } else { ui::Kind::Ghost };
    let tags = n.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
    column![
        row![ui::badge(&n.kind, tone), ui::btn(&n.title, None, kind, Some(SideMsg::OpenNote(n.slug.clone())))].spacing(8),
        ui::faint(tags),
    ]
    .spacing(2)
    .into()
}

fn reader_view<'a>(doc: &'a Note) -> Element<'a, SideMsg> {
    let mut items: Vec<Element<'a, SideMsg>> = vec![ui::mono(doc.title.clone(), 14.0, ui::INK).into(), ui::markdown(&doc.body)];
    if !doc.backlinks.is_empty() {
        items.push(ui::faint(format!("Linked from: {}", doc.backlinks.join(", "))).into());
    }
    column(items).spacing(10).into()
}

/// A labelled limit field; the placeholder says the plaza default applies when it is blank.
fn field<'a>(label: &'static str, placeholder: &'static str, value: &'a str, on_input: fn(String) -> SideMsg) -> Element<'a, SideMsg> {
    column![ui::label(label), text_input(placeholder, value).on_input(on_input).style(crate::theme::input)].spacing(4).into()
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
    let policy: &[String] = if orchestrator { &side.policy_orchestrator } else { &side.policy_worker };
    let label_of = |id: &str| all.iter().find(|c| c.id == id).map(|c| c.label.clone()).unwrap_or_else(|| id.to_string());
    let provider_of = |id: &str| id.split('/').next().unwrap_or_default().to_string();

    let mut out = vec![Choice { id: String::new(), provider: String::new(), label: "Use the default".into() }];
    for id in policy {
        if !out.iter().any(|c| c.id == *id) {
            out.push(Choice { id: id.clone(), provider: provider_of(id.as_str()), label: label_of(id.as_str()) });
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
        out.push(Choice { id: side.choice.clone(), provider: provider_of(side.choice.as_str()), label: side.choice.clone() });
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
    let model = if value.is_empty() { Value::Null } else { Value::String(value) };
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
    text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| fallback.to_string())
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

// ---- Requests ----

fn control(cid: u64, ctx: &Ctx, body: Value, fallback: &'static str, after: After) -> Task<SideMsg> {
    let c = ctx.client.clone();
    let path = format!("/v1/agents/{cid}/control");
    client::task(move || c.post::<Value>(&path, &body), move |result| SideMsg::Control { fallback, after, result })
}

fn fetch_agent(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<Value>(&format!("/v1/agents/{cid}")), SideMsg::Agent)
}

fn fetch_catalogue(ctx: &Ctx) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || -> Result<(Value, Value), String> { Ok((c.get("/v1/models")?, c.get("/v1/settings")?)) },
        SideMsg::Catalogue,
    )
}

fn fetch_permission(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Value>(&format!("/v1/agents/{cid}/permissions")).map(|v| v["mode"].as_str().unwrap_or_default().to_string()),
        SideMsg::PermissionLoaded,
    )
}

fn fetch_limits(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<Limits>(&format!("/v1/agents/{cid}/limits")), SideMsg::LimitsLoaded)
}

fn fetch_loops(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Vec<Loop>>("/v1/loops").map(|all| all.into_iter().filter(|l| l.cid == cid).collect::<Vec<Loop>>()),
        SideMsg::Loops,
    )
}

fn fetch_memory(ctx: &Ctx, cid: u64) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<NotesEnvelope>(&format!("/v1/agents/{cid}/memory")).map(|e| e.notes), SideMsg::Memory)
}

fn fetch_note(ctx: &Ctx, cid: u64, slug: String) -> Task<SideMsg> {
    let c = ctx.client.clone();
    client::task(move || c.get::<Note>(&format!("/v1/agents/{cid}/memory/{slug}")), SideMsg::NoteRead)
}
