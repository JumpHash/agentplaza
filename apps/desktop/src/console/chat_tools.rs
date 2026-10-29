//! The chat's model chip, context bar and breakdown (the chats brief). The IDE agent chat, the Overview's orchestrator
//! composer and the agent page each hold one [`ChatTools`] for their target: [`load`]
//! reads the model list, the chat's model and the hub's context for its latest work; the `context` event and each
//! send refresh the context ([`ChatTools::on_event`], [`ChatTools::refresh`]); the picker and the effort row save
//! the chat's model through the hub ([`update`]).
//!
//! The hub is the source of every number here. The breakdown is a column above the bar (the composer grows while
//! it is open): iced's `Stack` sizes to its tallest child, so a floating popover would need a fixed-height anchor.

use iced::widget::{Space, button, column, container, row};
use iced::{Alignment, Background, Border, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::client::{self, Client};
use super::picker::{self, Choice, Picker, PickerMsg};
use super::ui::{self, Kind};
use crate::theme;

/// The six parts of a chat's context, in the order the hub lists them, with the names the breakdown shows.
const PARTS: [(&str, &str); 6] = [
    ("system", "System"),
    ("tools", "Tools"),
    ("history", "History"),
    ("tool_results", "Tool results"),
    ("attachments", "Attachments"),
    ("summary", "Summary"),
];
/// Every `@effort` suffix a model value may carry; "none" is the default, so it reads as no effort.
const SUFFIXES: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
/// The breakdown popover's width.
const POPOVER_W: f32 = 360.0;
/// The context track's height.
const TRACK_H: f32 = 4.0;
/// The track's resolution: a part's segment is its share of this many units.
const TRACK_UNITS: u128 = 1000;

/// Which chat's model and context a [`ChatTools`] shows: an agent's chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Agent(u64),
}

/// One tool's row in the breakdown: its calls, the failed ones, and the tokens its results took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub tool: String,
    pub calls: u64,
    pub errors: u64,
    pub result_tokens: u64,
}

/// The hub's context for a chat's latest work (`GET .../context`): the model, the window (None when unknown),
/// the tokens in use with their parts, and the tool calls, largest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub model: String,
    pub window: Option<u64>,
    pub used: u64,
    /// The parts the hub lists, in its order: (part, tokens).
    pub parts: Vec<(String, u64)>,
    pub tools: Vec<ToolRow>,
    pub calls: u64,
    pub compactions: u64,
}

impl Context {
    /// Reads the hub's answer; None when it carries no `used`.
    pub fn parse(v: &Value) -> Option<Context> {
        let used = v["used"].as_u64()?;
        let parts = v["parts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        Some((
                            p["part"].as_str()?.to_string(),
                            p["tokens"].as_u64().unwrap_or(0),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tools = v["tools"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(ToolRow {
                            tool: t["tool"].as_str()?.to_string(),
                            calls: t["calls"].as_u64().unwrap_or(0),
                            errors: t["errors"].as_u64().unwrap_or(0),
                            result_tokens: t["result_tokens"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Context {
            model: v["model"].as_str().unwrap_or_default().to_string(),
            window: v["window"].as_u64().filter(|w| *w > 0),
            used,
            parts,
            tools,
            calls: v["calls"].as_u64().unwrap_or(0),
            compactions: v["compactions"].as_u64().unwrap_or(0),
        })
    }

    /// The tokens one part holds (0 when the hub lists none).
    pub fn part(&self, name: &str) -> u64 {
        self.parts
            .iter()
            .find(|(p, _)| p == name)
            .map_or(0, |(_, n)| *n)
    }

    /// The share of the window in use (0..), None when the window is unknown.
    pub fn ratio(&self) -> Option<f64> {
        self.window.map(|w| self.used as f64 / w as f64)
    }
}

/// What [`load`] reads: the target, the model list, the chat's record and its context.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub target: Target,
    pub models: Value,
    pub record: Value,
    pub context: Value,
}

/// A chat's model chip, context bar and breakdown. The fields are the chat's state; the screens only call the
/// functions of this module.
#[derive(Debug, Clone, Default)]
pub struct ChatTools {
    target: Option<Target>,
    /// The agent behind the chat: its own cid.
    cid: Option<u64>,
    /// The agent's default model (its `model`), which the chat's own model overrides.
    default_model: String,
    /// The chat's own model, `provider/model[@effort]`, when one is set.
    override_model: Option<String>,
    models: Value,
    choices: Vec<Choice>,
    picker: Picker,
    context: Option<Context>,
    breakdown: bool,
    error: Option<String>,
    saving: bool,
    compressing: bool,
}

#[derive(Debug, Clone)]
pub enum ChatToolsMsg {
    /// The full load landed (see [`load`]).
    Loaded(Result<Fetched, String>),
    /// A context read landed (see [`ChatTools::refresh`]).
    Read(Result<Value, String>),
    /// A picker message: its choice saves the model, its star saves a favourite.
    Picker(PickerMsg),
    /// The effort row was chosen ("" is the default).
    Effort(String),
    /// The model save landed; a failure reloads what the hub holds.
    Saved(Result<(), String>),
    /// The breakdown opens (true) or closes (false).
    Breakdown(bool),
    Compress,
    Compressed(Result<Value, String>),
    Favourited(Result<Vec<String>, String>),
}

fn record_path(target: &Target) -> String {
    match target {
        Target::Agent(cid) => format!("/v1/agents/{cid}"),
    }
}

fn context_path(target: &Target) -> String {
    format!("{}/context", record_path(target))
}

/// Reads the chat's model list, record and context. The result lands as [`ChatToolsMsg::Loaded`].
pub fn load(client: &Client, target: Target) -> Task<ChatToolsMsg> {
    let c = client.clone();
    client::task(
        move || -> Result<Fetched, String> {
            let models = c.get::<Value>("/v1/models")?;
            let record = c.get::<Value>(&record_path(&target))?;
            let context = c.get::<Value>(&context_path(&target))?;
            Ok(Fetched {
                target,
                models,
                record,
                context,
            })
        },
        ChatToolsMsg::Loaded,
    )
}

fn read_context(client: &Client, target: Target) -> Task<ChatToolsMsg> {
    let c = client.clone();
    client::task(
        move || c.get::<Value>(&context_path(&target)),
        ChatToolsMsg::Read,
    )
}

/// Applies the messages of a chat's tools; returns the task the caller maps to its own message.
pub fn update(state: &mut ChatTools, msg: ChatToolsMsg, client: &Client) -> Task<ChatToolsMsg> {
    match msg {
        ChatToolsMsg::Loaded(Ok(fetched)) => {
            state.apply(&fetched);
            Task::none()
        }
        ChatToolsMsg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        ChatToolsMsg::Read(Ok(v)) => {
            if let Some(ctx) = Context::parse(&v) {
                state.context = Some(ctx);
            }
            Task::none()
        }
        ChatToolsMsg::Read(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        ChatToolsMsg::Picker(m) => {
            if let PickerMsg::Effort(effort) = &m {
                return update(state, ChatToolsMsg::Effort(effort.clone()), client);
            }
            let pick = match &m {
                PickerMsg::Pick(id) => Some(id.clone()),
                _ => None,
            };
            let star = match &m {
                PickerMsg::Favourite(id) => Some(id.clone()),
                _ => None,
            };
            picker::update(&mut state.picker, &m);
            if let Some(id) = pick {
                let effort = split_effort(state.value()).1.to_string();
                return state.save(client, with_effort(&id, &effort));
            }
            if let Some(id) = star {
                let c = client.clone();
                return client::task(
                    move || picker::save_favourite(&c, &id),
                    ChatToolsMsg::Favourited,
                );
            }
            Task::none()
        }
        ChatToolsMsg::Effort(effort) => {
            // An effort on the default model pins that model for the chat, as the agent page does.
            let base = split_effort(state.value()).0.to_string();
            if base.is_empty() {
                return Task::none();
            }
            state.save(client, with_effort(&base, &effort))
        }
        ChatToolsMsg::Saved(Ok(())) => {
            state.saving = false;
            Task::none()
        }
        ChatToolsMsg::Saved(Err(e)) => {
            // The save did not take: the chip goes back to what the hub holds.
            state.saving = false;
            state.error = Some(e);
            match state.target.clone() {
                Some(target) => load(client, target),
                None => Task::none(),
            }
        }
        ChatToolsMsg::Breakdown(open) => {
            state.breakdown = open;
            Task::none()
        }
        ChatToolsMsg::Compress => {
            let Some(cid) = state.cid else {
                return Task::none();
            };
            state.compressing = true;
            state.error = None;
            let c = client.clone();
            let path = format!("/v1/agents/{cid}/control");
            client::task(
                move || c.post::<Value>(&path, &json!({ "action": "compress" })),
                ChatToolsMsg::Compressed,
            )
        }
        ChatToolsMsg::Compressed(Ok(_)) => {
            state.compressing = false;
            state.refresh(client)
        }
        ChatToolsMsg::Compressed(Err(e)) => {
            state.compressing = false;
            state.error = Some(e);
            Task::none()
        }
        ChatToolsMsg::Favourited(Ok(favourites)) => {
            picker::mark_favourites(&mut state.models, &favourites);
            picker::sync(&mut state.picker, &state.models);
            Task::none()
        }
        ChatToolsMsg::Favourited(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
    }
}

impl ChatTools {
    /// Puts the loaded record, models and context into the state.
    fn apply(&mut self, f: &Fetched) {
        let record = &f.record;
        let (cid, override_model, default_model) = match &f.target {
            Target::Agent(cid) => (
                Some(*cid),
                record["model_override"].as_str(),
                record["model"].as_str().unwrap_or_default(),
            ),
        };
        self.target = Some(f.target.clone());
        self.cid = cid;
        self.override_model = override_model.filter(|s| !s.is_empty()).map(str::to_string);
        self.default_model = default_model.to_string();
        self.models = f.models.clone();
        self.context = Context::parse(&f.context).or(self.context.take());
        self.error = None;

        // The choices are the hub's models, and the chat's own model when the list does not offer it.
        let mut choices = picker::choices(&self.models);
        let current = split_effort(self.value()).0.to_string();
        if !current.is_empty() && !choices.iter().any(|c| c.id == current) {
            choices.push(Choice {
                provider: current.split('/').next().unwrap_or_default().to_string(),
                label: current.clone(),
                id: current,
            });
        }
        self.choices = choices;
        picker::sync(&mut self.picker, &self.models);
    }

    /// The model value the chat runs with: its own override, else the record's default, else the model its last
    /// call ran. Empty when none is known.
    pub fn value(&self) -> &str {
        self.override_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(Some(self.default_model.as_str()).filter(|s| !s.is_empty()))
            .or(self
                .context
                .as_ref()
                .map(|c| c.model.as_str())
                .filter(|s| !s.is_empty()))
            .unwrap_or("")
    }

    /// Whether [`load`] has landed for this chat (a screen loads once; the events and sends refresh after).
    pub fn is_loaded(&self) -> bool {
        self.target.is_some()
    }

    /// Reads the chat's context again (after a send, or on the `context` event).
    pub fn refresh(&self, client: &Client) -> Task<ChatToolsMsg> {
        match self.target.clone() {
            Some(target) => read_context(client, target),
            None => Task::none(),
        }
    }

    /// The task for a hub event that concerns this chat: a `context` event for its agent refreshes the context.
    /// None for any other event.
    pub fn on_event(
        &self,
        client: &Client,
        kind: &str,
        data: &Value,
    ) -> Option<Task<ChatToolsMsg>> {
        if kind != "context" || self.cid? != data["cid"].as_u64()? {
            return None;
        }
        Some(self.refresh(client))
    }

    fn save(&mut self, client: &Client, value: String) -> Task<ChatToolsMsg> {
        let Some(target) = self.target.clone() else {
            return Task::none();
        };
        self.error = None;
        self.saving = true;
        self.override_model = (!value.is_empty()).then(|| value.clone());
        let body = json!({ "model": self.override_model.clone() });
        let c = client.clone();
        let path = record_path(&target);
        client::task(
            move || c.patch::<Value>(&path, &body).map(|_| ()),
            ChatToolsMsg::Saved,
        )
    }
}

/// The model chip and compact effort picker share one line, as in the web composer.
pub fn chip(st: &ChatTools) -> Element<'_, ChatToolsMsg> {
    let (model, effort) = split_effort(st.value());
    picker::view(
        &st.picker,
        &st.choices,
        model,
        Some(effort),
        ChatToolsMsg::Picker,
    )
}

/// The context bar: a thin track in the parts' colours, the count and the share of the window at its right, and
/// Compress. A click opens the breakdown above the bar. Before the first read it says so.
pub fn bar(st: &ChatTools) -> Element<'_, ChatToolsMsg> {
    let p = theme::palette();
    let (ink, ink_3, rule, crit, warn) = (p.ink, p.ink_3, p.rule, p.crit, p.warn);
    let Some(ctx) = st.context.as_ref() else {
        return ui::faint("Loading the context").into();
    };
    let tone = match ctx.ratio() {
        Some(r) if r > 0.95 => crit,
        Some(r) if r > 0.8 => warn,
        _ => ink_3,
    };

    let total = ctx.window.unwrap_or(ctx.used).max(ctx.used).max(1);
    let mut track = row![].width(Length::Fill).height(Length::Fixed(TRACK_H));
    let mut filled: u128 = 0;
    for (name, _) in PARTS {
        let n = ctx.part(name);
        if n == 0 {
            continue;
        }
        let units = scale(n, total);
        filled += u128::from(units);
        track = track.push(segment(units, part_color(name)));
    }
    if ctx.window.is_some() {
        let rest = TRACK_UNITS.saturating_sub(filled);
        if rest > 0 {
            track = track.push(segment(rest as u16, rule));
        }
    }

    let text = match ctx.window {
        Some(w) => format!(
            "{} / {} · {}",
            tokens_text(ctx.used),
            tokens_text(w),
            share_text(ctx.used, w)
        ),
        None => format!("{} used", tokens_text(ctx.used)),
    };
    let compress = ui::btn_sm(
        "Compress",
        None,
        Kind::Ghost,
        (st.cid.is_some() && !st.compressing).then_some(ChatToolsMsg::Compress),
    );
    let line = row![track, ui::mono(text, 12.0, tone), compress]
        .spacing(10)
        .align_y(Alignment::Center);
    let clickable = button(line)
        .padding(0)
        .width(Length::Fill)
        .style(move |_: &Theme, _| button::Style {
            background: None,
            text_color: ink,
            ..Default::default()
        })
        .on_press(ChatToolsMsg::Breakdown(!st.breakdown));

    let mut col = column![].spacing(6).width(Length::Fill);
    if st.breakdown {
        col = col.push(row![Space::new().width(Length::Fill), breakdown(st, ctx)]);
    }
    col.push(clickable).into()
}

/// The error the last action left, under the composer.
pub fn notice(st: &ChatTools) -> Option<Element<'_, ChatToolsMsg>> {
    st.error.as_deref().map(ui::error_line)
}

/// The breakdown: the model and window, the context's parts, the tool calls and the model calls.
fn breakdown<'a>(st: &'a ChatTools, ctx: &'a Context) -> Element<'a, ChatToolsMsg> {
    let p = theme::palette();
    let (raised, rule_strong, ink, ink_2, ink_3, crit) =
        (p.raised, p.rule_strong, p.ink, p.ink_2, p.ink_3, p.crit);
    let model = split_effort(st.value()).0;
    let model = if model.is_empty() {
        ctx.model.as_str()
    } else {
        model
    };
    let head = match ctx.window {
        Some(w) => format!("{model} · {} window", tokens_text(w)),
        None => format!("{model} · window unknown"),
    };
    let denom = ctx.window.unwrap_or(ctx.used);

    let mut parts = column![ui::label("Context")].spacing(6);
    for (name, label) in PARTS {
        let n = ctx.part(name);
        parts = parts.push(
            row![
                container(Space::new())
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
                    .style(move |_: &Theme| container::Style {
                        background: Some(Background::Color(part_color(name))),
                        border: Border {
                            radius: 2.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                ui::sans(label, 12.5, ink).width(Length::Fill),
                ui::mono(tokens_text(n), 12.0, ink_2),
                ui::mono(share_text(n, denom), 12.0, ink_3),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let mut tools = column![ui::label("Tool calls")].spacing(6);
    if ctx.tools.is_empty() {
        tools = tools.push(ui::faint("No tool calls yet"));
    }
    for t in &ctx.tools {
        let failed = if t.errors > 0 { crit } else { ink_3 };
        tools = tools.push(
            row![
                ui::mono(t.tool.clone(), 12.0, ink).width(Length::Fill),
                ui::mono(t.calls.to_string(), 12.0, ink_2),
                ui::mono(format!("{} err", t.errors), 12.0, failed),
                ui::mono(tokens_text(t.result_tokens), 12.0, ink_2),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let body = column![
        ui::mono(head, 12.0, ink_2),
        parts,
        tools,
        ui::faint(format!(
            "{} model calls · {} compactions",
            ctx.calls, ctx.compactions
        )),
    ]
    .spacing(14);
    container(body)
        .padding(14)
        .width(Length::Fixed(POPOVER_W))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(raised)),
            border: Border {
                color: rule_strong,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn segment<'a>(units: u16, color: Color) -> Element<'a, ChatToolsMsg> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::FillPortion(units))
        .height(Length::Fixed(TRACK_H))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

fn part_color(name: &str) -> Color {
    let p = theme::palette();
    match name {
        "tools" => p.info,
        "history" => p.signal,
        "tool_results" => p.review,
        "attachments" => p.ok,
        "summary" => p.warn,
        _ => p.ink_3,
    }
}

/// A part's share of `total` in track units (at least 1 for a part that holds anything, at most the track).
fn scale(n: u64, total: u64) -> u16 {
    let units = (u128::from(n) * TRACK_UNITS / u128::from(total.max(1))).clamp(1, TRACK_UNITS);
    units as u16
}

/// A token count as the consoles show it: "812", "41.2k", "200k", "1.2M".
pub fn tokens_text(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 999_950 {
        scaled(n, 1_000, "k")
    } else {
        scaled(n, 1_000_000, "M")
    }
}

/// `n` in units of `unit`, rounded to one decimal, the decimal dropped when it is zero.
fn scaled(n: u64, unit: u64, suffix: &str) -> String {
    let tenths = (u128::from(n) * 10 + u128::from(unit) / 2) / u128::from(unit);
    if tenths % 10 == 0 {
        format!("{}{suffix}", tenths / 10)
    } else {
        format!("{}.{}{suffix}", tenths / 10, tenths % 10)
    }
}

/// `part` of `of` as a whole percentage ("21%"); "0%" when `of` is zero.
pub fn share_text(part: u64, of: u64) -> String {
    if of == 0 {
        return "0%".to_string();
    }
    let pct = (u128::from(part) * 100 + u128::from(of) / 2) / u128::from(of);
    format!("{pct}%")
}

/// A model value's model and effort: `provider/model@high` is ("provider/model", "high"). An unknown suffix stays
/// in the model; "none" reads as the default (no effort).
pub fn split_effort(value: &str) -> (&str, &str) {
    match value.rsplit_once('@') {
        Some((model, effort)) if SUFFIXES.contains(&effort) => {
            (model, if effort == "none" { "" } else { effort })
        }
        _ => (value, ""),
    }
}

/// The model value for `model` at `effort`: the suffix is dropped for the default effort, and a value with no
/// model stays empty.
pub fn with_effort(model: &str, effort: &str) -> String {
    let (base, _) = split_effort(model);
    if base.is_empty() || effort.is_empty() {
        base.to_string()
    } else {
        format!("{base}@{effort}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_read_as_the_web_does() {
        assert_eq!(tokens_text(0), "0");
        assert_eq!(tokens_text(812), "812");
        assert_eq!(tokens_text(1_000), "1k");
        assert_eq!(tokens_text(1_234), "1.2k");
        assert_eq!(tokens_text(41_200), "41.2k");
        assert_eq!(tokens_text(200_000), "200k");
        assert_eq!(tokens_text(999_949), "999.9k");
        assert_eq!(tokens_text(999_950), "1M");
        assert_eq!(tokens_text(1_200_000), "1.2M");
        assert_eq!(tokens_text(2_000_000), "2M");
    }

    #[test]
    fn shares_round_to_whole_percents() {
        assert_eq!(share_text(41_200, 200_000), "21%");
        assert_eq!(share_text(1, 3), "33%");
        assert_eq!(share_text(2, 3), "67%");
        assert_eq!(share_text(5, 0), "0%");
    }

    #[test]
    fn effort_suffix_splits_and_joins() {
        assert_eq!(split_effort("openai/gpt@high"), ("openai/gpt", "high"));
        assert_eq!(split_effort("openai/gpt@none"), ("openai/gpt", ""));
        assert_eq!(split_effort("openai/gpt"), ("openai/gpt", ""));
        assert_eq!(
            split_effort("openrouter/x@weird"),
            ("openrouter/x@weird", "")
        );
        assert_eq!(with_effort("openai/gpt@low", "max"), "openai/gpt@max");
        assert_eq!(with_effort("openai/gpt@low", ""), "openai/gpt");
        assert_eq!(with_effort("openai/gpt", "high"), "openai/gpt@high");
        assert_eq!(with_effort("", "high"), "");
    }

    #[test]
    fn context_reads_the_hub_answer() {
        let v = json!({
            "model": "anthropic/claude-x", "window": 200000, "used": 41200,
            "parts": [{"part": "system", "tokens": 1000}, {"part": "history", "tokens": 40200}],
            "tools": [{"tool": "read_file", "calls": 3, "errors": 1, "result_tokens": 900}],
            "calls": 4, "compactions": 1
        });
        let ctx = Context::parse(&v).expect("a context with used");
        assert_eq!(ctx.window, Some(200_000));
        assert_eq!(ctx.part("history"), 40_200);
        assert_eq!(ctx.part("summary"), 0);
        assert_eq!(ctx.tools[0].errors, 1);
        assert!((ctx.ratio().unwrap() - 0.206).abs() < 1e-9);
        assert!(Context::parse(&json!({"model": "x"})).is_none());
    }
}
