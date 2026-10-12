//! The Rules screen (parity with the web console's `rules` view): the plaza's laws (JHRL),
//! explained and grouped by tier, filterable, with the latest infractions beside them.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text_input};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::Value;

use super::Id;
use crate::console::ui::{self, Kind, Tone, INK, INK_2, INK_3, RULE, SHEET};
use crate::console::{client, Ctx};

const DAY_MS: i64 = 86_400_000;
const INFRACTION_FETCH: usize = 200;
const LATEST_SHOWN: usize = 20;
const INTRO: &str = "These are the plaza's laws. Every action an agent takes is checked against them before it happens; a law that forbids an action stops it, and the attempt is recorded as an infraction.";

/// The tiers in the order the web groups them: key, label, description.
const TIERS: [(&str, &str, &str); 5] = [
    ("constitution", "Constitution", "The plaza's fundamental rules; they override everything else."),
    ("statutes", "Statutes", "Standing rules for roles and everyday work."),
    ("tools", "Tools", "Who may use which tool."),
    ("operations", "Operations", "The kinds of actions the laws talk about."),
    ("other", "Other", ""),
];
const SEVERITIES: [&str; 4] = ["info", "minor", "major", "critical"];

/// The effect filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    #[default]
    All,
    Permit,
    Forbid,
}

/// An agent named by a law's usage.
#[derive(Debug, Clone)]
pub struct LawAgent {
    cid: u64,
    name: String,
    count: i64,
}

/// One law, as the hub describes it (`GET /v1/laws`).
#[derive(Debug, Clone)]
pub struct Law {
    id: String,
    title: String,
    tier: String,
    enforcement: String,
    severity: String,
    statement: String,
    rationale: String,
    effect: String,
    ops: Vec<String>,
    principal: String,
    resource: String,
    conditions: Vec<String>,
    explanation: String,
    file: String,
    source: String,
    refusals: i64,
    agents: Vec<LawAgent>,
    last_at_ms: Option<i64>,
}

/// One infraction (`GET /v1/infractions`).
#[derive(Debug, Clone)]
pub struct Infraction {
    cid: u64,
    law: String,
    at_ms: i64,
    tool: String,
    detail: String,
    law_title: String,
    agent_name: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    loaded: bool,
    laws: Vec<Law>,
    recent: Vec<Infraction>,
    query: String,
    effect: Effect,
    severity: Option<&'static str>,
    tier: Option<&'static str>,
    open_source: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<(Vec<Law>, Vec<Infraction>), String>),
    Query(String),
    Effect(Effect),
    Severity(Option<&'static str>),
    Tier(Option<&'static str>),
    /// Clears every filter.
    Reset,
    /// Opens or closes a law's JHRL source.
    ToggleSource(String),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the laws and the latest infractions (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let laws = client.get::<Value>("/v1/laws")?;
            let infractions = client.get::<Value>(&format!("/v1/infractions?limit={INFRACTION_FETCH}"))?;
            Ok((parse_laws(&laws), parse_infractions(&infractions)))
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => {}
        Msg::Loaded(Ok((laws, recent))) => {
            state.laws = laws;
            state.recent = recent;
            state.loaded = true;
            state.error = None;
        }
        Msg::Loaded(Err(e)) => state.error = Some(e),
        Msg::Query(q) => state.query = q,
        Msg::Effect(e) => state.effect = e,
        Msg::Severity(s) => state.severity = s,
        Msg::Tier(t) => state.tier = t,
        Msg::Reset => {
            state.query.clear();
            state.effect = Effect::All;
            state.severity = None;
            state.tier = None;
        }
        Msg::ToggleSource(id) => {
            if !state.open_source.remove(&id) {
                state.open_source.insert(id);
            }
        }
    }
    Task::none()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = now_ms();
    let body = column![
        container(ui::prose(INTRO)).max_width(620),
        stats(state, now),
        toolbar(state),
        row![
            container(results(state, now)).width(Length::FillPortion(3)),
            container(infractions(state, now)).width(Length::FillPortion(1)),
        ]
        .spacing(28),
    ]
    .spacing(18);
    ui::sheet(ui::view_head("Rules", vec![]), scrollable(body).height(Length::Fill).into())
}

// ---- data ----

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn int_of(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn parse_laws(v: &Value) -> Vec<Law> {
    v.as_array().map(|a| a.iter().map(parse_law).collect()).unwrap_or_default()
}

fn parse_law(v: &Value) -> Law {
    let agents = v
        .get("agents")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| LawAgent {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    name: text_of(x, "name"),
                    count: int_of(x, "count"),
                })
                .collect()
        })
        .unwrap_or_default();
    Law {
        id: text_of(v, "id"),
        title: text_of(v, "title"),
        tier: text_of(v, "tier"),
        enforcement: text_of(v, "enforcement"),
        severity: text_of(v, "severity"),
        statement: text_of(v, "statement"),
        rationale: text_of(v, "rationale"),
        effect: text_of(v, "effect"),
        ops: strings_of(v, "ops"),
        principal: text_of(v, "principal"),
        resource: text_of(v, "resource"),
        conditions: strings_of(v, "conditions"),
        explanation: text_of(v, "explanation"),
        file: text_of(v, "file"),
        source: text_of(v, "source"),
        refusals: int_of(v, "refusals"),
        agents,
        last_at_ms: v.get("last_at_ms").and_then(Value::as_i64),
    }
}

fn parse_infractions(v: &Value) -> Vec<Infraction> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| Infraction {
                    cid: x.get("cid").and_then(Value::as_u64).unwrap_or(0),
                    law: text_of(x, "law"),
                    at_ms: int_of(x, "at_ms"),
                    tool: text_of(x, "tool"),
                    detail: text_of(x, "detail"),
                    law_title: text_of(x, "law_title"),
                    agent_name: text_of(x, "agent_name"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tier a law belongs to, read from its tier field, then from the file it lives in.
fn tier_of(l: &Law) -> &'static str {
    for probe in [l.tier.to_lowercase(), l.file.to_lowercase()] {
        if probe.contains("const") {
            return "constitution";
        }
        if probe.contains("statut") {
            return "statutes";
        }
        if probe.contains("tool") {
            return "tools";
        }
        if probe.contains("operat") {
            return "operations";
        }
    }
    "other"
}

fn visible(state: &State, l: &Law) -> bool {
    let forbid = l.effect == "forbid";
    match state.effect {
        Effect::Permit if forbid => return false,
        Effect::Forbid if !forbid => return false,
        _ => {}
    }
    if let Some(s) = state.severity {
        if l.severity != s {
            return false;
        }
    }
    if let Some(t) = state.tier {
        if tier_of(l) != t {
            return false;
        }
    }
    let q = state.query.trim().to_lowercase();
    if !q.is_empty() {
        let hay = format!("{} {} {} {}", l.title, l.statement, l.id, l.ops.join(" ")).to_lowercase();
        if !hay.contains(&q) {
            return false;
        }
    }
    true
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// "Just now", "12m ago", "3h ago"; an em dash for no time.
fn ago(ms: i64, now: i64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let s = ((now - ms) / 1000).max(0);
    if s < 5 {
        "just now".into()
    } else if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn or(s: &str, fallback: &str) -> String {
    if s.is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

fn enforcement_label(e: &str) -> String {
    match e {
        "preventive" => "Stops it before it happens".into(),
        "detective" => "Recorded after the fact".into(),
        "" => "Enforcement not stated".into(),
        other => cap(other),
    }
}

fn severity_tone(s: &str) -> Tone {
    match s {
        "critical" => Tone::Danger,
        "major" => Tone::Warn,
        "minor" => Tone::Info,
        _ => Tone::Muted,
    }
}

// ---- view ----

fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn sheet_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SHEET.into()),
        border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn link_style(_: &Theme, _: button::Status) -> button::Style {
    button::Style { text_color: INK, ..Default::default() }
}

/// A mono text button that opens an agent.
fn link<'a>(label: &str, msg: Msg) -> Element<'a, Msg> {
    button(ui::mono(label.to_string(), 12.0, INK))
        .padding([0, 0])
        .style(link_style)
        .on_press(msg)
        .into()
}

/// A filter chip: the active one is solid, the rest are text-only.
fn chip<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    ui::btn(label, None, if active { Kind::Primary } else { Kind::Ghost }, Some(msg))
}

fn cell<'a>(label: &'static str, value: String) -> Element<'a, Msg> {
    container(column![ui::label(label), ui::mono(value, 18.0, INK)].spacing(4))
        .padding([10, 14])
        .width(Length::Fill)
        .style(boxed)
        .into()
}

fn stats<'a>(state: &State, now: i64) -> Element<'a, Msg> {
    let ready = state.loaded;
    let figure = |v: String| if ready { v } else { "—".to_string() };
    let forbids = state.laws.iter().filter(|l| l.effect == "forbid").count();
    let cut = now - 30 * DAY_MS;
    let n30 = state.recent.iter().filter(|i| i.at_ms >= cut).count();
    let n30_text = if state.recent.len() >= INFRACTION_FETCH { format!("{n30}+") } else { n30.to_string() };
    row![
        cell("Laws", figure(state.laws.len().to_string())),
        cell("Allows", figure((state.laws.len() - forbids).to_string())),
        cell("Forbids", figure(forbids.to_string())),
        cell("Infractions, 30 days", figure(n30_text)),
    ]
    .spacing(12)
    .into()
}

fn toolbar<'a>(state: &'a State) -> Element<'a, Msg> {
    let search = text_input("Search by title, statement, id or operation", &state.query)
        .on_input(Msg::Query)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(crate::theme::input);

    let mut sev: Vec<Element<'a, Msg>> = vec![chip("Any", state.severity.is_none(), Msg::Severity(None))];
    for s in SEVERITIES {
        sev.push(chip(&cap(s), state.severity == Some(s), Msg::Severity(Some(s))));
    }

    let mut effects: Vec<Element<'a, Msg>> = Vec::new();
    for (e, label) in [(Effect::All, "All"), (Effect::Permit, "Allows"), (Effect::Forbid, "Forbids")] {
        effects.push(chip(label, state.effect == e, Msg::Effect(e)));
    }

    let mut tabs: Vec<Element<'a, Msg>> = vec![chip("All", state.tier.is_none(), Msg::Tier(None))];
    for &(key, label, _) in TIERS.iter().take(4) {
        tabs.push(chip(label, state.tier == Some(key), Msg::Tier(Some(key))));
    }

    column![
        row![search, row(sev).spacing(6).align_y(Alignment::Center)].spacing(16).align_y(Alignment::Center),
        row![row(effects).spacing(6), row(tabs).spacing(6)].spacing(18).align_y(Alignment::Center),
    ]
    .spacing(12)
    .into()
}

fn results<'a>(state: &'a State, now: i64) -> Element<'a, Msg> {
    if !state.loaded {
        return if state.error.is_some() {
            ui::error_line("The rules could not be loaded just now.")
        } else {
            ui::loading()
        };
    }
    let shown: Vec<&Law> = state.laws.iter().filter(|l| visible(state, l)).collect();
    let noun = if state.laws.len() == 1 { "rule" } else { "rules" };
    let count = ui::mono(format!("Showing {} of {} {}", shown.len(), state.laws.len(), noun), 12.0, INK_3);

    let list: Element<'a, Msg> = if shown.is_empty() {
        container(
            column![
                ui::faint("No rules match these filters."),
                ui::btn("Clear filters", None, Kind::Secondary, Some(Msg::Reset)),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .padding([40, 16])
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
    } else {
        let mut groups: Vec<Element<'a, Msg>> = Vec::new();
        for &(key, label, desc) in TIERS.iter() {
            let items: Vec<&Law> = shown.iter().copied().filter(|l| tier_of(l) == key).collect();
            if items.is_empty() {
                continue;
            }
            groups.push(group_head(label, desc, items.len()));
            for l in items {
                groups.push(law_card(l, state.open_source.contains(&l.id), now));
            }
        }
        column(groups).spacing(14).into()
    };
    column![count, list].spacing(12).into()
}

fn group_head<'a>(label: &'static str, desc: &'static str, n: usize) -> Element<'a, Msg> {
    let head = row![ui::mono(label, 16.0, INK), ui::mono(n.to_string(), 12.0, INK_3)]
        .spacing(10)
        .align_y(Alignment::Center);
    let mut col = column![head].spacing(4);
    if !desc.is_empty() {
        col = col.push(ui::faint(desc));
    }
    col.into()
}

fn labelled<'a>(name: &str, value: &str) -> Element<'a, Msg> {
    row![
        ui::label(name).width(Length::Fixed(96.0)),
        ui::prose(value.to_string()).width(Length::Fill),
    ]
    .spacing(12)
    .into()
}

fn op_tag<'a>(op: &str) -> Element<'a, Msg> {
    container(ui::mono(op.to_string(), 11.5, INK_2))
        .padding([2, 6])
        .style(boxed)
        .into()
}

fn applies<'a>(l: &Law) -> Element<'a, Msg> {
    let scope = format!("{} → {}", or(&l.principal, "any principal"), or(&l.resource, "any resource"));
    let mut col: Vec<Element<'a, Msg>> = vec![ui::mono(scope, 12.0, INK).into()];
    if !l.ops.is_empty() {
        let tags: Vec<Element<'a, Msg>> = l.ops.iter().map(|o| op_tag(o)).collect();
        col.push(row(tags).spacing(6).into());
    }
    row![
        ui::label("Applies to").width(Length::Fixed(96.0)),
        column(col).spacing(6).width(Length::Fill),
    ]
    .spacing(12)
    .into()
}

fn usage<'a>(l: &Law, now: i64) -> Element<'a, Msg> {
    let n = l.refusals;
    let mut line = format!("Enforced {} {}", n, if n == 1 { "time" } else { "times" });
    if let Some(at) = l.last_at_ms {
        line.push_str(&format!(" · last {}", ago(at, now)));
    }
    if !l.agents.is_empty() {
        line.push_str(" · by");
    }
    let mut items: Vec<Element<'a, Msg>> = vec![ui::faint(line).into()];
    for a in &l.agents {
        items.push(link(&format!("{} ({})", a.name, a.count), Msg::Go(Id::Agent(a.cid))));
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

fn source_block<'a>(l: &Law, open: bool) -> Element<'a, Msg> {
    let toggle = ui::btn("How it is written (JHRL)", None, Kind::Ghost, Some(Msg::ToggleSource(l.id.clone())));
    if !open {
        return toggle;
    }
    let conds: Element<'a, Msg> = if l.conditions.is_empty() {
        ui::faint("No extra conditions.").into()
    } else {
        let lines: Vec<Element<'a, Msg>> = l.conditions.iter().map(|c| ui::faint(format!("· {c}")).into()).collect();
        column(lines).spacing(4).into()
    };
    column![
        toggle,
        ui::faint(format!("File: {}", or(&l.file, "—"))),
        conds,
        container(ui::mono(or(&l.source, "—"), 12.0, INK)).padding(10).width(Length::Fill).style(sheet_box),
    ]
    .spacing(10)
    .into()
}

fn law_card<'a>(l: &Law, open: bool, now: i64) -> Element<'a, Msg> {
    let forbid = l.effect == "forbid";
    let tags: Vec<Element<'a, Msg>> = vec![
        ui::badge(if forbid { "Forbids" } else { "Allows" }, if forbid { Tone::Danger } else { Tone::Ok }),
        ui::badge(&format!("Severity: {}", cap(&l.severity)), severity_tone(&l.severity)),
        ui::badge(
            &enforcement_label(&l.enforcement),
            if l.enforcement == "preventive" { Tone::Info } else { Tone::Muted },
        ),
    ];
    let head = row![
        column![ui::mono(l.title.clone(), 15.0, INK), ui::mono(l.id.clone(), 11.0, INK_3)]
            .spacing(2)
            .width(Length::Fill),
        row(tags).spacing(6).align_y(Alignment::Center),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut body: Vec<Element<'a, Msg>> = vec![
        head.into(),
        ui::prose(l.statement.clone()).into(),
        labelled("In short", &l.explanation),
    ];
    if !l.rationale.is_empty() {
        body.push(labelled("Why", &l.rationale));
    }
    body.push(applies(l));
    if l.refusals > 0 {
        body.push(usage(l, now));
    }
    body.push(source_block(l, open));

    container(column(body).spacing(10))
        .padding(16)
        .width(Length::Fill)
        .style(boxed)
        .into()
}

fn infractions<'a>(state: &State, now: i64) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if !state.loaded {
        if state.error.is_some() {
            ui::error_line("Infractions could not be loaded.")
        } else {
            ui::loading()
        }
    } else {
        let mut latest: Vec<&Infraction> = state.recent.iter().collect();
        latest.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
        latest.truncate(LATEST_SHOWN);
        if latest.is_empty() {
            ui::faint("No infractions yet.").into()
        } else {
            let rows: Vec<Element<'a, Msg>> = latest.into_iter().map(|i| infraction_row(i, now)).collect();
            column(rows).spacing(12).into()
        }
    };
    ui::card("Latest infractions", body)
}

fn infraction_row<'a>(i: &Infraction, now: i64) -> Element<'a, Msg> {
    let agent = if i.agent_name.is_empty() { format!("agent {}", i.cid) } else { i.agent_name.clone() };
    let law = if i.law_title.is_empty() { i.law.clone() } else { i.law_title.clone() };
    let mut meta: Vec<Element<'a, Msg>> = Vec::new();
    if !i.tool.is_empty() {
        meta.push(ui::mono(i.tool.clone(), 11.0, INK_2).into());
    }
    if !i.detail.is_empty() {
        meta.push(ui::faint(i.detail.clone()).into());
    }
    meta.push(ui::faint(ago(i.at_ms, now)).into());
    column![
        row![
            link(&agent, Msg::Go(Id::Agent(i.cid))),
            ui::faint("violated"),
            ui::mono(law, 12.0, INK),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        row(meta).spacing(8).align_y(Alignment::Center),
    ]
    .spacing(4)
    .into()
}
