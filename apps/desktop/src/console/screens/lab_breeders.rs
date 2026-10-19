//! The Lab's breeders (`LabBreeders`): the agents with the breeder role, each with the GPU-hours
//! its jobs have used and the per-job cap S-006c sets, its jobs (linked to their
//! detail screens) and its recent Foundry tool calls. The calls come from the agent's own
//! transcript, read as the agent's page reads it and kept to the foundry_* tools and invoke_model.
//! A job belongs to a breeder when its `agent` is the breeder's CID (the hub's job summary carries it). The web's `lab-breeders.ts` is the twin.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use iced::widget::{Space, column, container, row};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::Value;

use super::Id;
use super::lab::{self, Section};
use super::lab_common;

use crate::console::Ctx;
use crate::console::client::{self, Client};
use crate::console::ui::{self, Kind, Tone};

/// The per-job GPU-hour cap when the laws cannot be read: S-006c permits a breeder's job up to
/// 8 GPU-hours (S-006d refuses more).
const PER_JOB_CAP_FALLBACK: f64 = 8.0;
/// The transcript page size the agent page reads with, and the calls shown per breeder and kept.
const PAGE: usize = 1000;
const RECENT: usize = 12;
const KEPT: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct AgentRow {
    cid: u64,
    name: String,
    role: String,
    status: String,
}

/// A job as the list gives it, with the submitting CID when the hub says it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct JobRow {
    id: String,
    status: String,
    recipe: String,
    created_ms: u64,
    gpu_hours: f64,
    budget_gpu_hours: f64,
    agent: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Entry {
    seq: u64,
    at_ms: u64,
    kind: String,
    data: Value,
}

/// One Foundry tool call of a breeder: its request, what its decision or result said, and whether it was refused.
#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    call: String,
    at_ms: u64,
    name: String,
    detail: String,
    result: String,
    refused: bool,
}

/// A breeder's read position in its transcript, and what that read found.
#[derive(Debug, Clone, Default)]
pub struct Cursor {
    after: u64,
    names: BTreeMap<String, String>,
    calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct Breeder {
    agent: AgentRow,
    jobs: Vec<JobRow>,
    /// The newest calls first.
    calls: Vec<ToolCall>,
    used: f64,
    /// S-006c's per-job GPU-hour cap.
    cap: f64,
    note: String,
}

#[derive(Debug, Default)]
pub struct State {
    /// The breeders, None until the first load.
    pub breeders: Option<Vec<Breeder>>,
    pub error: Option<String>,
    /// Each breeder's transcript cursor, kept across loads so a load reads only what is new.
    cursors: BTreeMap<u64, Cursor>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<(Vec<Breeder>, BTreeMap<u64, Cursor>), String>),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Loaded(_) => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let cursors = state.cursors.clone();
    client::task(move || fetch(&c, cursors), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok((breeders, cursors))) => {
            state.breeders = Some(breeders);
            state.cursors = cursors;
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(err)) => {
            state.error = Some(err);
            Task::none()
        }
    }
}

fn fetch(
    c: &Client,
    mut cursors: BTreeMap<u64, Cursor>,
) -> Result<(Vec<Breeder>, BTreeMap<u64, Cursor>), String> {
    let agents: Vec<AgentRow> = c.get("/v1/agents")?;
    let jobs: Vec<JobRow> = c.get("/v1/foundry/jobs")?;
    let mut mine: Vec<AgentRow> = agents.into_iter().filter(|a| a.role == "breeder").collect();
    mine.sort_by_key(|a| a.cid);
    let mut out = Vec::new();
    for agent in mine {
        let mut cur = cursors.remove(&agent.cid).unwrap_or_default();
        let note = match read(c, agent.cid, &mut cur) {
            Ok(()) => String::new(),
            Err(err) => format!("Transcript unreachable: {err}"),
        };
        // A breeder's jobs are the jobs the hub says were submitted with its CID.
        let own: Vec<JobRow> = jobs
            .iter()
            .filter(|j| j.agent == Some(agent.cid))
            .cloned()
            .collect();
        let used = own.iter().map(|j| j.gpu_hours).sum();
        let cap = per_job_cap(c);
        let calls = cur.calls.iter().rev().take(RECENT).cloned().collect();
        cursors.insert(agent.cid, cur);
        out.push(Breeder {
            agent,
            jobs: own,
            calls,
            used,
            cap,
            note,
        });
    }
    Ok((out, cursors))
}

/// S-006c's per-job cap, read from the laws once and kept; the fallback while it cannot be read.
fn per_job_cap(c: &Client) -> f64 {
    static CAP: OnceLock<f64> = OnceLock::new();
    if let Some(cap) = CAP.get() {
        return *cap;
    }
    match read_cap(c) {
        Some(cap) => *CAP.get_or_init(|| cap),
        None => PER_JOB_CAP_FALLBACK,
    }
}

/// The number in S-006c's condition `context.gpu_hours <= N`.
fn read_cap(c: &Client) -> Option<f64> {
    let laws: Vec<Value> = c.get("/v1/laws").ok()?;
    let law = laws
        .iter()
        .find(|l| l.get("id").and_then(Value::as_str) == Some("S-006c"))?;
    let conditions = law
        .get("conditions")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let at = conditions.find("context.gpu_hours")?;
    let rest = conditions[at + "context.gpu_hours".len()..]
        .trim_start()
        .strip_prefix("<=")?
        .trim_start();
    let number: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    number.parse().ok()
}

/// Reads the transcript from the cursor on, a page at a time, until the newest entry.
fn read(c: &Client, cid: u64, cur: &mut Cursor) -> Result<(), String> {
    loop {
        let mut page: Vec<Entry> = c.get(&format!(
            "/v1/agents/{cid}/transcript?after={}&limit={PAGE}",
            cur.after
        ))?;
        page.sort_by_key(|e| e.seq);
        for e in &page {
            absorb(cur, e);
        }
        if page.len() < PAGE {
            break;
        }
    }
    if cur.calls.len() > KEPT {
        let excess = cur.calls.len() - KEPT;
        cur.calls.drain(..excess);
    }
    Ok(())
}

fn is_foundry(name: &str) -> bool {
    name.starts_with("foundry_") || name == "invoke_model"
}

/// Folds one transcript entry into a breeder's cursor: the Foundry requests, then their decisions and results.
fn absorb(cur: &mut Cursor, e: &Entry) {
    cur.after = cur.after.max(e.seq);
    let call = e
        .data
        .get("call")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if e.kind == "tool_request" {
        let name = e
            .data
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !is_foundry(&name) {
            return;
        }
        cur.names.insert(call.clone(), name.clone());
        cur.calls.push(ToolCall {
            call,
            at_ms: e.at_ms,
            name,
            detail: detail_of(e.data.get("input")),
            result: String::new(),
            refused: false,
        });
        return;
    }
    let Some(name) = cur.names.get(&call).cloned() else {
        return;
    };
    let Some(row) = cur.calls.iter_mut().rev().find(|c| c.call == call) else {
        return;
    };
    let output = e
        .data
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if e.kind == "tool_decision" {
        row.refused = e.data.get("decision").and_then(Value::as_str) == Some("deny");
        if !output.is_empty() {
            row.result = output.clone();
        }
        if name == "foundry_submit_job"
            && let Some((id, false)) = job_of(&output)
        {
            let status = status_of(&output).unwrap_or_else(|| "submitted".to_string());
            row.result = format!("job {id} {status}");
        }
    } else if e.kind == "tool_result" {
        if !output.is_empty() && row.result.is_empty() {
            row.result = output;
        }
        if e.data.get("ok").and_then(Value::as_bool) == Some(false) {
            row.refused = true;
        }
    }
}

/// The job a submit call's output names: the job view's `"id"` (a dispatched job), or the refusal's
/// `job <id> did not dispatch` (refused true). The output may be cut short; the id comes first.
fn job_of(output: &str) -> Option<(String, bool)> {
    if let Some(start) = output.find("\"id\":\"") {
        let rest = &output[start + 6..];
        return Some((rest[..rest.find('"')?].to_string(), false));
    }
    let rest = output.strip_prefix("job ")?;
    let (id, tail) = rest.split_once(' ')?;
    tail.starts_with("did not dispatch")
        .then(|| (id.to_string(), true))
}

fn status_of(output: &str) -> Option<String> {
    let start = output.find("\"status\":\"")?;
    let rest = &output[start + 10..];
    Some(rest[..rest.find('"')?].to_string())
}

/// The one input a call names: its model, its job's recipe, its job, or its name.
fn detail_of(input: Option<&Value>) -> String {
    let Some(i) = input else { return String::new() };
    let recipe = i
        .get("job")
        .and_then(|j| j.get("recipe"))
        .and_then(Value::as_str);
    let job = i.get("job").and_then(Value::as_str);
    let name = i.get("name").and_then(Value::as_str);
    let kind = i.get("kind").and_then(Value::as_str);
    i.get("model")
        .and_then(Value::as_str)
        .or(recipe)
        .or(job)
        .or(name)
        .or(kind)
        .unwrap_or("")
        .to_string()
}

fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        format!("{}…", one.chars().take(max).collect::<String>())
    } else {
        one
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.breeders {
        None => match state.error.as_deref() {
            Some(err) => lab_common::err(err),
            None => lab_common::note("Loading breeders"),
        },
        Some(list) if list.is_empty() => lab_common::empty(
            "bot",
            "No breeders yet.",
            Some("A breeder is an agent with the breeder role; create one from Agents."),
            None,
        ),
        Some(list) => {
            let jobs: usize = list.iter().map(|b| b.jobs.len()).sum();
            let used: f64 = list.iter().map(|b| b.used).sum();
            let summary = format!(
                "{} breeders · {} jobs · {} used",
                lab_common::count(list.len() as u64),
                lab_common::count(jobs as u64),
                lab_common::gpu_hours(used),
            );
            let mut parts: Vec<Element<'a, Msg>> = vec![lab_common::note(summary)];
            if let Some(err) = state.error.as_deref() {
                parts.push(lab_common::err(err));
            }
            parts.push(grid(list.iter().map(breeder_card).collect(), BREEDER_COLS));
            parts.push(jobs_card(list));
            parts.push(calls_card(list));
            column(parts).spacing(16).width(Length::Fill).into()
        }
    };
    lab::frame(
        Section::Breeders,
        ui::scroll(body).height(Length::Fill).into(),
        Msg::Go,
    )
}

/// One breeder's card, as the web's: its name and status, its role and CID, then the GPU-hours used,
/// the jobs and the per-job cap; a transcript that cannot be read is noted on it.
fn breeder_card<'a>(b: &'a Breeder) -> Element<'a, Msg> {
    let a = &b.agent;
    let name = if a.name.trim().is_empty() {
        format!("agent {}", a.cid)
    } else {
        a.name.clone()
    };
    let mut body: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge(&a.status, status_tone(&a.status)),
            lab_common::note(format!("breeder · CID {}", a.cid)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
        kv_line("GPU-h used", lab_common::gpu_hours(b.used)),
        kv_line("Jobs", lab_common::count(b.jobs.len() as u64)),
        kv_line(
            "Per-job cap",
            format!("{} GPU-h", lab_common::number(b.cap, 0)),
        ),
    ];
    if !b.note.is_empty() {
        body.push(lab_common::err(&b.note));
    }
    ui::card_with(
        &name,
        vec![ui::btn_sm(
            "Agent page",
            None,
            Kind::Ghost,
            Some(Msg::Go(Id::Agent(a.cid))),
        )],
        column(body).spacing(10).width(Length::Fill).into(),
    )
}

/// A label in a fixed column beside its value, as the web's `dl.kv`.
fn kv_line<'a>(label: &str, value: String) -> Element<'a, Msg> {
    row![
        container(lab_common::note(label.to_string())).width(Length::Fixed(120.0)),
        ui::sans(value, 13.0, crate::theme::palette().ink),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .into()
}

/// A grid of cards, `cols` to a row (the web's `.lab2-grid`).
fn grid<'a>(cards: Vec<Element<'a, Msg>>, cols: usize) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut cells: Vec<Element<'a, Msg>> = Vec::new();
        for _ in 0..cols {
            match cards.next() {
                Some(card) => cells.push(container(card).width(Length::FillPortion(1)).into()),
                None => cells.push(Space::new().width(Length::FillPortion(1)).into()),
            }
        }
        rows.push(row(cells).spacing(12).width(Length::Fill).into());
    }
    column(rows).spacing(12).width(Length::Fill).into()
}

/// Every breeder's jobs in one table: each row opens its job.
fn jobs_card<'a>(breeders: &'a [Breeder]) -> Element<'a, Msg> {
    let jobs: Vec<&JobRow> = breeders.iter().flat_map(|b| b.jobs.iter()).collect();
    let body: Element<'a, Msg> = if jobs.is_empty() {
        lab_common::note("No jobs yet.")
    } else {
        let cols = [
            lab_common::col("Job", 2, false),
            lab_common::col("Status", 2, false),
            lab_common::col("Recipe", 3, false),
            lab_common::col("GPU-h", 2, true),
            lab_common::col("Created", 3, false),
        ];
        let now = lab_common::now_ms();
        let rows: Vec<lab_common::Row<'a, Msg>> = jobs
            .iter()
            .map(|j| lab_common::Row {
                cells: vec![
                    lab_common::cell(j.id.clone(), true),
                    ui::badge(&j.status, job_tone(&j.status)),
                    lab_common::cell(j.recipe.clone(), true),
                    lab_common::cell(format!("{:.2}", j.gpu_hours), false),
                    lab_common::cell(lab_common::ago_text(j.created_ms, now), false),
                ],
                on_press: Some(Msg::Go(Id::LabJobDetail(j.id.clone()))),
            })
            .collect();
        lab_common::table(&cols, rows)
    };
    ui::card("Jobs", body)
}

/// Each breeder's recent Foundry calls, newest first, read from its transcript.
fn calls_card<'a>(breeders: &'a [Breeder]) -> Element<'a, Msg> {
    let parts: Vec<Element<'a, Msg>> = breeders
        .iter()
        .map(|b| {
            if !b.note.is_empty() && b.calls.is_empty() {
                return lab_common::note(b.note.clone());
            }
            if b.calls.is_empty() {
                return lab_common::note("No Foundry calls yet.");
            }
            let cols = [
                lab_common::col("Time", 2, false),
                lab_common::col("Tool", 2, false),
                lab_common::col("Input", 4, false),
                lab_common::col("Result", 5, false),
            ];
            let now = lab_common::now_ms();
            let rows: Vec<lab_common::Row<'a, Msg>> = b
                .calls
                .iter()
                .map(|c| {
                    let input = if c.detail.is_empty() {
                        "—".to_string()
                    } else {
                        clip(&c.detail, 80)
                    };
                    let result: Element<'a, Msg> = if c.refused {
                        let tail = if c.result.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ": {}",
                                clip(
                                    &c.result.split_whitespace().collect::<Vec<_>>().join(" "),
                                    140
                                )
                            )
                        };
                        ui::sans(format!("Refused{tail}"), 13.0, crate::theme::palette().crit)
                            .into()
                    } else if c.result.is_empty() {
                        lab_common::cell("—", false)
                    } else {
                        lab_common::cell(
                            clip(
                                &c.result.split_whitespace().collect::<Vec<_>>().join(" "),
                                140,
                            ),
                            false,
                        )
                    };
                    lab_common::Row {
                        cells: vec![
                            lab_common::cell(lab_common::ago_text(c.at_ms, now), false),
                            lab_common::cell(c.name.clone(), true),
                            lab_common::cell(input, true),
                            result,
                        ],
                        on_press: None,
                    }
                })
                .collect();
            lab_common::table(&cols, rows)
        })
        .collect();
    ui::card(
        "Recent Foundry calls",
        column(parts).spacing(12).width(Length::Fill).into(),
    )
}

fn job_tone(status: &str) -> Tone {
    match status {
        "running" => Tone::Info,
        "done" => Tone::Ok,
        "failed" => Tone::Danger,
        "cancelled" => Tone::Warn,
        _ => Tone::Muted,
    }
}

fn status_tone(status: &str) -> Tone {
    match status {
        "working" => Tone::Signal,
        "idle" => Tone::Ok,
        _ => Tone::Muted,
    }
}

/// Cards per row in the breeders grid, as the web's `lab2-grid` at the console's width.
const BREEDER_COLS: usize = 3;
