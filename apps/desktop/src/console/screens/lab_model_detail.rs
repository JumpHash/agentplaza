//! One model (`LabModelDetail(id)`): its card (identity, source job, state, size, parameters), the
//! metrics per split, the calibration, the shadow totals and agreement, the input and output
//! schemas, the features, and the data provenance per split. Its actions are the registry's
//! (shadow, promote, retire), shared with `LabModels`, and an imported model's rederive. A promoted
//! model links to its model agent.
//! The web's `lab-model-detail.ts` is the twin.

use iced::widget::{column, container, row, text, text_input, Space};
use iced::{Border, Element, Length, Task, Theme};
use serde_json::{json, Value};

use super::lab::{self, Section};
use super::lab_common;
use super::lab_models::{self, Act, Controls};
use super::Id;
use crate::console::client;
use crate::console::lab_api::{self, ModelView};
use crate::console::ui::{self, Kind};
use crate::console::Ctx;
use crate::theme::{SANS};

/// The error the apid answers for an unknown model id.
const NO_SUCH_MODEL: &str = "no such model";

#[derive(Debug, Default)]
pub struct State {
    /// The model this screen shows.
    pub model: String,
    /// The model as apid last answered, None until the first answer.
    pub data: Option<ModelView>,
    pub error: Option<String>,
    /// True once apid says the id names no model.
    pub missing: bool,
    pub controls: Controls,
    /// The rederive confirm (an imported model only): open, its last refusal, and whether a call is in flight.
    pub rederive_open: bool,
    pub rederive_problem: Option<String>,
    pub rederiving: bool,
    /// The Try it box: its text, the last answer (or refusal), and whether a run is in flight.
    pub try_text: String,
    pub try_answer: Option<Result<Value, String>>,
    pub try_busy: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// A control's act (shadow, the dialogs).
    Act(Act),
    Loaded(Result<ModelView, String>),
    /// A started request's answer.
    Done(Result<ModelView, String>),
    /// The rederive confirm: open, dismissed, confirmed, and the answer.
    AskRederive,
    DismissRederive,
    ConfirmRederive,
    Rederived(Result<ModelView, String>),
    TryText(String),
    TryRun,
    Tried(Result<Value, String>),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let id = state.model.clone();
    client::task(move || lab_api::get_model(&c, &id), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(model)) => {
            state.data = Some(model);
            state.error = None;
            state.missing = false;
            Task::none()
        }
        Msg::Loaded(Err(err)) => {
            if err == NO_SUCH_MODEL {
                state.missing = true;
                state.data = None;
            } else {
                state.error = Some(err);
            }
            Task::none()
        }
        Msg::Act(act) => match state.controls.apply(act) {
            Some(request) => lab_models::run(&ctx.client, request, Msg::Done),
            None => Task::none(),
        },
        Msg::AskRederive => {
            state.rederive_open = true;
            state.rederive_problem = None;
            Task::none()
        }
        Msg::DismissRederive => {
            state.rederive_open = false;
            state.rederive_problem = None;
            Task::none()
        }
        Msg::ConfirmRederive => {
            if state.rederiving {
                return Task::none();
            }
            state.rederiving = true;
            state.rederive_problem = None;
            let c = ctx.client.clone();
            let id = state.model.clone();
            client::task(move || lab_api::rederive_model(&c, &id), Msg::Rederived)
        }
        Msg::Rederived(Ok(model)) => {
            state.rederiving = false;
            state.rederive_open = false;
            state.data = Some(model);
            state.error = None;
            load(state, ctx)
        }
        Msg::Rederived(Err(err)) => {
            state.rederiving = false;
            state.rederive_problem = Some(err);
            Task::none()
        }
        Msg::TryText(text) => {
            state.try_text = text;
            Task::none()
        }
        Msg::TryRun => {
            let text = state.try_text.trim().to_string();
            if text.is_empty() {
                state.try_answer = Some(Err("Type some text first.".to_string()));
                return Task::none();
            }
            if state.try_busy {
                return Task::none();
            }
            state.try_busy = true;
            state.try_answer = None;
            let c = ctx.client.clone();
            let path = format!("/v1/foundry/models/{}/invoke", enc(&state.model));
            client::task(move || c.post::<Value>(&path, &json!({ "text": text })), Msg::Tried)
        }
        Msg::Tried(result) => {
            state.try_busy = false;
            state.try_answer = Some(result);
            Task::none()
        }
        Msg::Done(result) => {
            let shown = result.clone();
            if state.controls.finish(result) {
                state.data = shown.ok();
                load(state, ctx)
            } else {
                Task::none()
            }
        }
    }
}


pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if state.missing {
        column![
            lab_common::empty("No such model.", "No model with this id."),
            ui::link("Go back to Models.", Msg::Go(Id::LabModels), false),
        ]
        .spacing(12)
        .align_x(iced::Alignment::Center)
        .into()
    } else {
        match &state.data {
            None => match state.error.as_deref() {
                Some(err) => ui::empty("alert", "Model unreachable", err),
                None => ui::loading(),
            },
            Some(model) => detail(model, state),
        }
    };
    lab::frame(Section::Models, body, Msg::Go)
}

fn detail<'a>(m: &'a ModelView, state: &'a State) -> Element<'a, Msg> {
    let back = ui::link("Back to Models", Msg::Go(Id::LabModels), true);
    // The state and the actions sit on their own line: a long model name would push them off the head.
    let head = ui::view_head(&m.card.name, vec![]);
    // An imported model can be rederived (apid reads its download again); no other state has that action.
    let rederive = (m.state == "imported")
        .then(|| ui::btn("Rederive", Some("refresh"), Kind::Secondary, (!state.rederiving).then_some(Msg::AskRederive)));
    let mut items: Vec<Element<'a, Msg>> = vec![ui::badge(&m.state, lab_models::tone(&m.state)), lab_models::actions(m, state.controls.busy, Msg::Act)];
    items.extend(rederive);
    let controls = row(items).spacing(14).align_y(iced::Alignment::Center);
    let mut parts: Vec<Element<'a, Msg>> = vec![back, head, lab_models::value(m.id.clone()), controls.into()];
    if let Some(dialog) = lab_models::dialog(&state.controls, Msg::Act) {
        parts.push(dialog);
    } else if state.rederive_open {
        parts.push(rederive_dialog(state));
    } else if let Some(problem) = state.controls.problem.as_deref() {
        parts.push(ui::error_line(problem));
    }
    if let Some(err) = state.error.as_deref() {
        parts.push(ui::error_line(err));
    }
    if let Some(card) = try_card(m, state) {
        parts.push(card);
    }
    // Two cards to a row, in the web's order: identity and shadow, metrics and calibration, the
    // input and output schemas, the features and the provenance.
    let cards = vec![
        identity_card(m),
        shadow_card(m),
        card_of("Metrics", grid(&["Split", "Metric", "Value"], rows_of(&m.card.metrics, true), "No metrics recorded.")),
        card_of("Calibration", grid(&["Measure", "Value"], rows_of(&m.calibration, false), "No calibration recorded.")),
        card_of("Input schema", grid(&["Field", "Type", "Required"], schema_rows(&m.card.input), "No input schema.")),
        card_of("Output schema", grid(&["Field", "Type", "Required"], schema_rows(&m.card.output), "No output schema.")),
        card_of("Features", grid(&["Name", "Kind", "Details"], feature_rows(&m.card.features), "No features.")),
        card_of("Data provenance", grid(&["Split", "Field", "Value"], rows_of(&m.provenance, true), "No provenance recorded.")),
    ];
    let mut cards = cards.into_iter();
    while let (Some(a), b) = (cards.next(), cards.next()) {
        parts.push(match b {
            Some(b) => row![container(a).width(Length::FillPortion(1)), container(b).width(Length::FillPortion(1))].spacing(22).into(),
            None => row![container(a).width(Length::FillPortion(1)), Space::new().width(Length::FillPortion(1))].spacing(22).into(),
        });
    }
    column(parts).spacing(22).into()
}

/// The rederive confirm: what it does, the last refusal, then Cancel and the confirm.
fn rederive_dialog<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!("Rederive {}?", state.model)).into(),
        ui::faint("The download is registered again: a model that now checks as servable is registered as one, otherwise it stays imported. If that fails, the record stays as it was.").into(),
    ];
    if let Some(problem) = state.rederive_problem.as_deref() {
        lines.push(ui::error_line(problem));
    }
    lines.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::DismissRederive)),
            ui::btn("Rederive", Some("refresh"), Kind::Primary, (!state.rederiving).then_some(Msg::ConfirmRederive)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
    );
    ui::card("Rederive model", column(lines).spacing(12).into())
}

/// A "Try it" card for a text model: in shadow or promoted, its text goes to invoke and the answer
/// shows below; in any other state, a note on how to make it runnable.
fn try_card<'a>(m: &'a ModelView, state: &'a State) -> Option<Element<'a, Msg>> {
    let serving = &m.card.serving;
    if serving.get("kind").and_then(Value::as_str) != Some("text") {
        return None;
    }
    let task = serving.get("task").and_then(Value::as_str).unwrap_or("text");
    if m.state != "shadow" && m.state != "promoted" {
        return Some(card_of("Try it", ui::faint("Shadow or promote this model to try it here.").into()));
    }
    let max_len = serving.get("max_len").and_then(Value::as_u64).map_or("—".to_string(), |n| n.to_string());
    let field = text_input("Text to run through the model", &state.try_text).on_input(Msg::TryText).on_submit(Msg::TryRun).padding(8);
    let run = ui::btn("Run", Some("play"), Kind::Primary, (!state.try_busy).then_some(Msg::TryRun));
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!("The text is cut to {max_len} tokens. Each run is a real call to the model.")).into(),
        row![field, run].spacing(8).align_y(iced::Alignment::Center).into(),
    ];
    match &state.try_answer {
        Some(Ok(answer)) => body.push(answer_view(answer)),
        Some(Err(err)) => body.push(ui::error_line(err)),
        None if state.try_busy => body.push(ui::faint("Running").into()),
        None => {}
    }
    Some(card_of(&format!("Try it · {task}"), column(body).spacing(12).into()))
}

/// A text model's answer: labels with scores as bars, or an embedding's size and first values.
fn answer_view<'a>(answer: &Value) -> Element<'a, Msg> {
    let ms = answer.get("ms").and_then(Value::as_u64).unwrap_or(0);
    let output = answer.get("output").cloned().unwrap_or(Value::Null);
    if let Some(embedding) = output.get("embedding").and_then(Value::as_array) {
        let values: Vec<f64> = embedding.iter().filter_map(Value::as_f64).collect();
        let first = values.iter().take(8).map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(", ");
        return column![
            lab_models::value(format!("Embedding · {} dimensions · {} ms", lab_common::count(values.len() as u64), lab_common::count(ms))),
            ui::faint(if values.len() > 8 { format!("{first}, …") } else { first }),
        ]
        .spacing(6)
        .into();
    }
    if let Some(scores) = output.get("scores").and_then(Value::as_array) {
        let top = output.get("label").and_then(Value::as_str).unwrap_or("—");
        let mut lines = column![lab_models::value(format!("{top} · {} ms", lab_common::count(ms)))].spacing(8);
        for s in scores {
            let label = s.get("label").and_then(Value::as_str).unwrap_or("—").to_string();
            let score = s.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            lines = lines.push(
                row![
                    container(lab_models::value(label)).width(Length::Fixed(140.0)),
                    container(meter(score)).width(Length::Fill),
                    container(lab_models::value(format!("{:.2}%", score * 100.0))).width(Length::Fixed(90.0)),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );
        }
        return lines.into();
    }
    lab_models::value(output.to_string())
}

/// Percent-encodes a path segment.
fn enc(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A card of the page: its title and body.
fn card_of<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    ui::card(title, body)
}

/// The identity card: the model's id, version, state, dates, source job, agent, roles, labels, size.
fn identity_card<'a>(m: &'a ModelView) -> Element<'a, Msg> {
    let agent: Element<'a, Msg> = match m.agent_cid {
        Some(cid) => ui::link(&format!("Open the model agent (CID {cid})"), Msg::Go(Id::Agent(cid)), true),
        None => lab_models::value("—".to_string()),
    };
    let job: Element<'a, Msg> = match &m.source_job {
        Some(job) => ui::link(job, Msg::Go(Id::LabJobDetail(job.clone())), true),
        None => lab_models::value("—".to_string()),
    };
    let labels = m.card.labels.clone().unwrap_or_default();
    let body = column![
        pair("ID", lab_models::value(m.id.clone())),
        pair("Version", lab_models::value(m.card.version.clone())),
        pair("State", ui::badge(&m.state, lab_models::tone(&m.state))),
        pair("State changed", lab_models::value(lab_common::time(m.state_changed_ms))),
        pair("Created", lab_models::value(lab_common::time(m.created_ms))),
        pair("Source job", job),
        pair("Model agent", agent),
        pair("Servable", lab_models::value(lab_models::servable_text(m))),
        pair("Allowed roles", lab_models::value(list_or_dash(&m.card.allowed_roles))),
        pair("Labels", lab_models::value(list_or_dash(&labels))),
        pair("Size", lab_models::value(lab_common::size(m.size_bytes))),
        pair("Parameters", lab_models::value(lab_common::count(m.parameters))),
        pair("ONNX sha256", lab_models::value(if m.card.onnx_sha256.is_empty() { "—".to_string() } else { in_lines(&m.card.onnx_sha256, 32) })),
    ]
    .spacing(10);
    card_of("Model", body.into())
}

/// The shadow card: the totals as tiles, the agreement as a meter, and a note on what is kept.
fn shadow_card<'a>(m: &'a ModelView) -> Element<'a, Msg> {
    let s = &m.shadow;
    let tiles = row![
        tile("Runs", lab_common::count(s.runs)),
        tile("Agreed", lab_common::count(s.agreed)),
        tile("Agreement", m.agreement_rate.map_or("—".to_string(), lab_common::percent)),
        tile("Cost saved", usd(s.cost_saved_usd)),
        tile("Updated", if s.updated_ms == 0 { "never".to_string() } else { lab_common::ago(s.updated_ms) }),
    ]
    .spacing(8);
    let bar: Element<'a, Msg> = match m.agreement_rate {
        Some(rate) => meter(rate),
        None => ui::faint("No shadow runs yet.").into(),
    };
    let body = column![
        tiles,
        bar,
        ui::faint("Totals since the model entered shadow. The registry keeps no per-run history, so there is no chart over time."),
    ]
    .spacing(14);
    card_of("Shadow", body.into())
}

/// A tally tile: its label over the figure, ruled.
fn tile<'a>(label: &str, figure: String) -> Element<'a, Msg> {
    container(column![ui::label(label), text(figure).font(SANS).size(16.0).color(crate::theme::palette().ink)].spacing(4))
        .padding(10)
        .width(Length::FillPortion(1))
        .style(|_: &Theme| container::Style { border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() }, ..Default::default() })
        .into()
}

/// The agreement as a bar: the signal fill over the faint rule, as the web's meter.
fn meter<'a>(rate: f64) -> Element<'a, Msg> {
    let pct = (rate.clamp(0.0, 1.0) * 100.0).round() as u16;
    row![
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion(pct.max(1)))
            .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().signal.into()), ..Default::default() }),
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion((100 - pct).max(1)))
            .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().rule_faint.into()), ..Default::default() }),
    ]
    .into()
}

/// A labelled figure: the label in caps beside its value.
fn pair<'a>(label: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(label)).width(Length::Fixed(130.0)), value].spacing(12).align_y(iced::Alignment::Center).into()
}

/// A table of text cells with an equal share per column; `none` stands in for no rows.
fn grid<'a>(heads: &[&str], rows: Vec<Vec<String>>, none: &str) -> Element<'a, Msg> {
    if rows.is_empty() {
        return ui::faint(none.to_string()).into();
    }
    let head = row(heads.iter().map(|h| cell(text(h.to_string()).font(SANS).size(11.0).color(crate::theme::palette().ink_3))).collect::<Vec<_>>()).spacing(10);
    let mut lines = column![head].spacing(8);
    for cells in rows {
        lines = lines.push(row(cells.into_iter().map(|c| cell(lab_models::value(c))).collect::<Vec<_>>()).spacing(10));
    }
    lines.into()
}

fn cell<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).width(Length::FillPortion(1)).into()
}

/// The leaves of a nested record as rows. With `split`, the first key of a deeper path names its
/// split (`val` in `val.accuracy`); the rest of the path is the name.
fn rows_of(value: &Value, split: bool) -> Vec<Vec<String>> {
    leaves(value, Vec::new())
        .into_iter()
        .map(|(path, v)| {
            if split {
                let (head, rest) = if path.len() > 1 { (path[0].clone(), path[1..].join(" ")) } else { ("—".to_string(), path.join(" ")) };
                vec![head, dash_if_empty(rest), value_text(&v)]
            } else {
                vec![dash_if_empty(path.join(" ")), value_text(&v)]
            }
        })
        .collect()
}

fn leaves(value: &Value, path: Vec<String>) -> Vec<(Vec<String>, Value)> {
    match value {
        Value::Object(map) => map
            .iter()
            .flat_map(|(key, v)| {
                let mut next = path.clone();
                next.push(key.clone());
                leaves(v, next)
            })
            .collect(),
        other => vec![(path, other.clone())],
    }
}

/// A JSON schema's properties as rows: name, type and required. A schema without properties shows
/// its leaves.
fn schema_rows(schema: &Value) -> Vec<Vec<String>> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return leaves(schema, Vec::new()).into_iter().map(|(path, v)| vec![dash_if_empty(path.join(" ")), value_text(&v), "—".to_string()]).collect();
    };
    let required: Vec<&str> = schema.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    properties
        .iter()
        .map(|(name, prop)| vec![name.clone(), type_of(prop), if required.contains(&name.as_str()) { "yes" } else { "no" }.to_string()])
        .collect()
}

/// A feature as a row: its name, its kind, and its other fields.
fn feature_rows(features: &[Value]) -> Vec<Vec<String>> {
    features
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let name = f.get("name").and_then(Value::as_str).map_or_else(|| i.to_string(), str::to_string);
            let kind = f.get("kind").and_then(Value::as_str).unwrap_or("—").to_string();
            let rest: Vec<String> = f
                .as_object()
                .map(|map| map.iter().filter(|(k, _)| k.as_str() != "name" && k.as_str() != "kind").map(|(k, v)| format!("{k}: {}", value_text(v))).collect())
                .unwrap_or_default();
            vec![name, kind, dash_if_empty(rest.join(", "))]
        })
        .collect()
}

/// A property's type: its enum when it has one, else its `type`.
fn type_of(prop: &Value) -> String {
    if let Some(options) = prop.get("enum").and_then(Value::as_array) {
        return format!("one of {}", options.iter().map(value_text).collect::<Vec<_>>().join(", "));
    }
    match prop.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(types)) => types.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | "),
        _ => "any".to_string(),
    }
}

/// A value as text: whole numbers grouped, fractions to four significant digits, lists joined,
/// objects as JSON.
fn value_text(v: &Value) -> String {
    match v {
        Value::Null => "—".to_string(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => lab_common::number(f, 0),
            Some(f) => trim_fraction(&format!("{f:.4}")),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(value_text).collect::<Vec<_>>().join(", "),
        Value::Object(_) => v.to_string(),
    }
}

/// `0.9100` to `0.91`.
fn trim_fraction(text: &str) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text.to_string()
    }
}

/// A long unbroken token (a hash) in lines of `width` characters, since text wraps only at spaces.
fn in_lines(token: &str, width: usize) -> String {
    let chars: Vec<char> = token.chars().collect();
    chars.chunks(width).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn list_or_dash(names: &[String]) -> String {
    if names.is_empty() {
        "—".to_string()
    } else {
        names.join(", ")
    }
}

fn dash_if_empty(text: String) -> String {
    if text.is_empty() {
        "—".to_string()
    } else {
        text
    }
}

/// A dollar figure: cents, or four decimals under a cent.
fn usd(v: f64) -> String {
    if v == 0.0 {
        "$0.00".to_string()
    } else if v.abs() < 0.01 {
        format!("${v:.4}")
    } else {
        format!("${}", lab_common::number(v, 2))
    }
}
