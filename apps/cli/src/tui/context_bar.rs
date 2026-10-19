//! The context of a chat, and the model and effort it runs at, as the TUI's chat views show them: the Workspace's agent
//! chat and the Chat tab's conversation. A bar above the input says how full the context is (`41.2k / 200k · 21%`);
//! `i` opens the breakdown (the six parts, the tool calls, the calls and compactions); `M` opens the model and effort
//! picker, which saves the choice at once. The hub answers `GET /v1/agents/{cid}/context` (or the conversation's), the
//! model list is `GET /v1/models`, and a choice is a `PATCH` of the chat's `model` (see `crates/apid/src/context_track.rs`).
//! This file holds the data, its formatting, the picker's keys and its drawing; the views call it, and the app runs
//! the requests it asks for (`Ask`).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use serde_json::{Value, json};

use super::app::Palette;
use super::screens::clip;
use super::workspace::{Got, Key};
use crate::ctx::{Client, seg};

/// How often the context of a chat on screen is asked for again (the events refresh it sooner).
pub const EVERY: f32 = 10.0;

/// The six parts of a context, in the hub's order, with the names the views write.
pub const PARTS: [(&str, &str); 6] = [
    ("system", "system"),
    ("tools", "tools"),
    ("history", "history"),
    ("tool_results", "tool results"),
    ("attachments", "attachments"),
    ("summary", "summary"),
];

/// The effort levels a model entry can carry (`provider/model@effort`), by the name the picker shows. Default carries
/// no suffix: the provider's own default.
pub const EFFORTS: [(&str, Option<&str>); 5] = [
    ("Default", None),
    ("Low", Some("low")),
    ("Medium", Some("medium")),
    ("High", Some("high")),
    ("Max", Some("max")),
];

/// What a context belongs to: an agent's chat (its cid), or an owner conversation (the orchestrator's work in it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Owner {
    Agent(u64),
    Conversation(String),
}

impl Owner {
    /// Where its context is read.
    pub fn context_path(&self) -> String {
        match self {
            Owner::Agent(cid) => format!("/v1/agents/{cid}/context"),
            Owner::Conversation(id) => format!("/v1/owner/conversations/{}/context", seg(id)),
        }
    }

    /// Where its model is set (`PATCH {model}`).
    pub fn model_path(&self) -> String {
        match self {
            Owner::Agent(cid) => format!("/v1/agents/{cid}"),
            Owner::Conversation(id) => format!("/v1/owner/conversations/{}", seg(id)),
        }
    }
}

/// A request the views ask the app to run: a context read, the model list, or a model choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Fetch(Owner),
    Models,
    /// A choice of model (`provider/model[@effort]`), or None for the default.
    Save {
        owner: Owner,
        model: Option<String>,
    },
}

/// How full a context is: calm, over 80 % (warn), or over 95 % (crit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Calm,
    Warn,
    Crit,
}

/// One tool's tally in the work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub calls: u64,
    pub errors: u64,
    pub result_tokens: u64,
}

/// A chat's context as the hub last answered it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Context {
    /// The orchestrator or agent whose calls these are.
    pub cid: u64,
    pub model: String,
    /// The model's window in tokens, when known.
    pub window: Option<u64>,
    /// The latest call's context in tokens.
    pub used: u64,
    /// Tokens by part, in `PARTS` order.
    pub parts: [u64; 6],
    /// The tool calls of the work item, largest result first (the hub's order).
    pub tools: Vec<Tool>,
    pub calls: u64,
    pub compactions: u64,
}

impl Context {
    /// Reads `GET .../context`. None when the answer has no `used`.
    pub fn parse(v: &Value) -> Option<Context> {
        let used = v.get("used")?.as_u64()?;
        let mut parts = [0u64; 6];
        for part in v
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = part.get("part").and_then(Value::as_str).unwrap_or("");
            if let Some(i) = PARTS.iter().position(|(key, _)| *key == name) {
                parts[i] = part.get("tokens").and_then(Value::as_u64).unwrap_or(0);
            }
        }
        let tools = v
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|t| Tool {
                name: t
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
                calls: t.get("calls").and_then(Value::as_u64).unwrap_or(0),
                errors: t.get("errors").and_then(Value::as_u64).unwrap_or(0),
                result_tokens: t.get("result_tokens").and_then(Value::as_u64).unwrap_or(0),
            })
            .collect();
        Some(Context {
            cid: v.get("cid").and_then(Value::as_u64).unwrap_or(0),
            model: v
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            window: v.get("window").and_then(Value::as_u64).filter(|w| *w > 0),
            used,
            parts,
            tools,
            calls: v.get("calls").and_then(Value::as_u64).unwrap_or(0),
            compactions: v.get("compactions").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// The fill: used over the window, when the window is known.
    pub fn tone(&self) -> Tone {
        match self.window {
            Some(window) if self.used * 100 > window * 95 => Tone::Crit,
            Some(window) if self.used * 100 > window * 80 => Tone::Warn,
            _ => Tone::Calm,
        }
    }
}

/// A token count as the consoles write it: 812, 1k, 41.2k, 200k, 1.2M.
pub fn compact(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 999_500 {
        return scaled(n, 1_000, "k");
    }
    scaled(n, 1_000_000, "M")
}

/// `n` in units of `unit`: one decimal under 100 units (dropped when it is zero), whole units above.
fn scaled(n: u64, unit: u64, suffix: &str) -> String {
    let tenths = (n * 10 + unit / 2) / unit;
    if tenths < 1_000 {
        return if tenths.is_multiple_of(10) {
            format!("{}{suffix}", tenths / 10)
        } else {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        };
    }
    format!("{}{suffix}", (n + unit / 2) / unit)
}

/// The share of `of` in `total`, in whole percent (0 when the total is 0).
pub fn percent(of: u64, total: u64) -> u64 {
    (of * 100 + total / 2).checked_div(total).unwrap_or(0)
}

/// The bar's text: `41.2k / 200k · 21%`, or `41.2k used` when the window is not known.
pub fn bar_text(ctx: &Context) -> String {
    match ctx.window {
        Some(window) => format!(
            "{} / {} · {}%",
            compact(ctx.used),
            compact(window),
            percent(ctx.used, window)
        ),
        None => format!("{} used", compact(ctx.used)),
    }
}

/// The colour of each part's segment of the bar, in `PARTS` order.
fn part_color(part: usize, palette: Palette) -> Color {
    match part {
        0 => Color::Gray,
        1 => Color::Blue,
        2 => palette.accent,
        3 => Color::Magenta,
        4 => palette.ok,
        _ => Color::Yellow,
    }
}

fn tone_style(tone: Tone, palette: Palette) -> Style {
    match tone {
        Tone::Calm => Style::new().fg(palette.muted),
        Tone::Warn => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        Tone::Crit => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

/// The track of the bar, `width` cells: each part's share as full blocks in its colour, then the empty rest as light
/// shade. Scaled to the window when it is known, else to what the parts add up to.
fn track(ctx: &Context, width: usize, palette: Palette) -> Vec<Span<'static>> {
    let total: u64 = ctx.parts.iter().sum();
    let scale = ctx.window.unwrap_or(total.max(ctx.used)).max(1);
    let width_u = width as u64;
    let mut cells: Vec<(char, Style)> = Vec::with_capacity(width);
    let mut so_far = 0u64;
    for (i, tokens) in ctx.parts.iter().enumerate() {
        so_far += tokens;
        let end = ((so_far.min(scale) * width_u + scale / 2) / scale) as usize;
        let style = Style::new().fg(part_color(i, palette));
        while cells.len() < end.min(width) {
            cells.push(('█', style));
        }
    }
    let empty = Style::new().fg(palette.muted);
    while cells.len() < width {
        cells.push(('░', empty));
    }
    group(cells)
}

/// Joins cells of one style into spans.
fn group(cells: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in cells {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(ch),
            _ => spans.push(Span::styled(ch.to_string(), style)),
        }
    }
    spans
}

/// The bar's line over the input: the track, then the text (its colour follows the tone). `reading…` until the
/// context is known.
pub fn bar_line(ctx: Option<&Context>, width: u16, palette: Palette) -> Line<'static> {
    let Some(ctx) = ctx else {
        return Line::from(Span::styled(
            "context · reading…",
            Style::new().fg(palette.muted),
        ));
    };
    let width = usize::from(width);
    let text = bar_text(ctx);
    let text_width = text.chars().count();
    let text_style = tone_style(ctx.tone(), palette);
    if width < text_width + 1 + 8 {
        return Line::from(Span::styled(clip(&text, width), text_style));
    }
    let mut spans = track(ctx, width - text_width - 1, palette);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(text, text_style));
    Line::from(spans)
}

/// The breakdown's rows: the model and window, the parts with their tokens and share, the tool calls, and the calls and
/// compactions. Plain text, one string per line, shared by the panel and `/context`.
pub fn plain(ctx: &Context) -> Vec<String> {
    let mut lines = vec![format!(
        "{} · window {}",
        if ctx.model.is_empty() {
            "-"
        } else {
            &ctx.model
        },
        ctx.window.map_or("unknown".to_string(), compact)
    )];
    lines.push(String::new());
    lines.push("Context".to_string());
    let total: u64 = ctx.parts.iter().sum();
    for ((_, label), tokens) in PARTS.iter().zip(ctx.parts) {
        lines.push(format!(
            "  {label:<14}{:>8}{:>5}%",
            compact(tokens),
            percent(tokens, total)
        ));
    }
    lines.push(String::new());
    lines.push("Tool calls".to_string());
    if ctx.tools.is_empty() {
        lines.push("  No tool calls yet".to_string());
    }
    for tool in &ctx.tools {
        lines.push(format!(
            "  {:<20}{:>6} calls{:>6} errors{:>9} tokens",
            clip(&tool.name, 20),
            tool.calls,
            tool.errors,
            compact(tool.result_tokens)
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "{} model calls · {} compactions",
        ctx.calls, ctx.compactions
    ));
    lines
}

/// The breakdown as styled lines for the panel: the same rows as `plain`, with the swatches, the tool errors in crit
/// when there are any, and the headings in bold.
pub fn breakdown_lines(ctx: Option<&Context>, palette: Palette) -> Vec<Line<'static>> {
    let muted = Style::new().fg(palette.muted);
    let Some(ctx) = ctx else {
        return vec![Line::from(Span::styled("reading the context…", muted))];
    };
    let heading = Style::new().add_modifier(Modifier::BOLD);
    let total: u64 = ctx.parts.iter().sum();
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            if ctx.model.is_empty() {
                "-".to_string()
            } else {
                ctx.model.clone()
            },
            heading,
        ),
        Span::styled(
            format!(
                " · window {}",
                ctx.window.map_or("unknown".to_string(), compact)
            ),
            muted,
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("Context", heading)));
    for (i, ((_, label), tokens)) in PARTS.iter().zip(ctx.parts).enumerate() {
        lines.push(Line::from(vec![
            Span::styled("█ ", Style::new().fg(part_color(i, palette))),
            Span::raw(format!(
                "{label:<14}{:>8}{:>5}%",
                compact(tokens),
                percent(tokens, total)
            )),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("Tool calls", heading)));
    if ctx.tools.is_empty() {
        lines.push(Line::from(Span::styled("  No tool calls yet", muted)));
    }
    for tool in &ctx.tools {
        let errors_style = if tool.errors > 0 {
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            muted
        };
        lines.push(Line::from(vec![
            Span::raw(format!(
                "  {:<20}{:>6} calls",
                clip(&tool.name, 20),
                tool.calls
            )),
            Span::styled(format!("{:>6} errors", tool.errors), errors_style),
            Span::raw(format!("{:>9} tokens", compact(tool.result_tokens))),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!(
            "{} model calls · {} compactions",
            ctx.calls, ctx.compactions
        ),
        muted,
    )));
    lines
}

/// The breakdown panel over the bottom right of `area` (the chat's transcript), `i` closes it.
pub fn panel(frame: &mut Frame, area: Rect, ctx: Option<&Context>, palette: Palette) {
    let width = area.width.min(60);
    let lines = breakdown_lines(ctx, palette);
    let height = (lines.len() as u16 + 2).min(area.height);
    if width < 20 || height < 3 {
        return;
    }
    let rect = Rect {
        x: area.x + area.width - width,
        y: area.y + area.height - height,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" context · i closes ")
        .border_style(Style::new().fg(palette.accent));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// One row of the picker: the default, or a `provider/model` the hub lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// `provider/model`; None for the default row.
    pub entry: Option<String>,
    pub favourite: bool,
    pub window: Option<u64>,
}

/// The model and effort picker of one chat. The rows come when the hub answers the model list (`load`); until then the
/// picker says it is reading them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub owner: Owner,
    /// The chat's model as it is set now (`provider/model`, without the effort).
    pub current: Option<String>,
    /// The rows: the default first, then the favourites, then the rest by name. None while they are read.
    pub choices: Option<Vec<Choice>>,
    pub cursor: usize,
    /// Index into `EFFORTS`.
    pub effort: usize,
    /// An error (or the save's state) to say under the rows.
    pub note: Option<String>,
}

/// What a key does to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stroke {
    Keep,
    Close,
    /// Save this choice: a model with its effort, or None for the default.
    Save {
        owner: Owner,
        model: Option<String>,
    },
}

impl Picker {
    /// A picker for a chat whose model is `current` (`provider/model@effort`, or None). The effort is read from it.
    pub fn open(owner: Owner, current: Option<&str>) -> Picker {
        let text = current.unwrap_or("");
        let (entry, level) = text.split_once('@').unwrap_or((text, ""));
        let effort = EFFORTS
            .iter()
            .position(|(_, suffix)| *suffix == Some(level))
            .unwrap_or(0);
        Picker {
            owner,
            current: (!entry.is_empty()).then(|| entry.to_string()),
            choices: None,
            cursor: 0,
            effort,
            note: None,
        }
    }

    /// Takes the hub's model list (`GET /v1/models`): favourites first, then by name, under the default row.
    pub fn load(&mut self, listing: &Value) {
        let mut models: Vec<Choice> = listing
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let provider = m.get("provider")?.as_str()?;
                let id = m.get("id")?.as_str()?;
                Some(Choice {
                    entry: Some(format!("{provider}/{id}")),
                    favourite: m.get("favourite").and_then(Value::as_bool) == Some(true),
                    window: m.get("context").and_then(Value::as_u64),
                })
            })
            .collect();
        models.sort_by(|a, b| {
            b.favourite
                .cmp(&a.favourite)
                .then_with(|| a.entry.cmp(&b.entry))
        });
        let mut choices = vec![Choice {
            entry: None,
            favourite: false,
            window: None,
        }];
        choices.extend(models);
        self.cursor = choices
            .iter()
            .position(|c| c.entry == self.current)
            .unwrap_or(0);
        self.choices = Some(choices);
    }

    /// The list could not be read: only the default row is offered, with the reason under it.
    pub fn fail(&mut self, error: &str) {
        self.choices = Some(vec![Choice {
            entry: None,
            favourite: false,
            window: None,
        }]);
        self.cursor = 0;
        self.note = Some(format!("models: {error}"));
    }

    /// The model the save sends: the row's entry with the effort's suffix, or None for the default row.
    pub fn chosen(&self) -> Option<Option<String>> {
        let choices = self.choices.as_ref()?;
        let entry = choices.get(self.cursor)?.entry.as_ref();
        Some(entry.map(|e| match EFFORTS[self.effort].1 {
            Some(level) => format!("{e}@{level}"),
            None => e.clone(),
        }))
    }

    /// One key: the rows move with the arrows (j, k), the effort with Tab, BackTab, h and l; Enter saves; Esc closes.
    pub fn key(&mut self, key: Key) -> Stroke {
        let rows = self.choices.as_ref().map_or(0, Vec::len);
        match key {
            Key::Esc | Key::CtrlC => Stroke::Close,
            Key::Up | Key::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                Stroke::Keep
            }
            Key::Down | Key::Char('j') => {
                if self.cursor + 1 < rows {
                    self.cursor += 1;
                }
                Stroke::Keep
            }
            Key::PageUp => {
                self.cursor = self.cursor.saturating_sub(10);
                Stroke::Keep
            }
            Key::PageDown => {
                self.cursor = (self.cursor + 10).min(rows.saturating_sub(1));
                Stroke::Keep
            }
            Key::Tab | Key::Char('l') => {
                self.effort = (self.effort + 1) % EFFORTS.len();
                Stroke::Keep
            }
            Key::BackTab | Key::Char('h') => {
                self.effort = (self.effort + EFFORTS.len() - 1) % EFFORTS.len();
                Stroke::Keep
            }
            Key::Enter => match self.chosen() {
                Some(model) => Stroke::Save {
                    owner: self.owner.clone(),
                    model,
                },
                None => {
                    self.note = Some("still reading the models".to_string());
                    Stroke::Keep
                }
            },
            _ => Stroke::Keep,
        }
    }
}

/// The picker over `area`, centred. Its rows scroll with the cursor.
pub fn overlay(frame: &mut Frame, area: Rect, picker: &Picker, palette: Palette) {
    let width = area.width.min(68);
    let list_rows = picker.choices.as_ref().map_or(1, |c| c.len().min(10)) as u16;
    // Borders, the rows, a gap, the effort, the note, the keys.
    let height = (list_rows + 6).min(area.height);
    if width < 30 || height < 5 {
        return;
    }
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let muted = Style::new().fg(palette.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();
    match &picker.choices {
        None => lines.push(Line::from(Span::styled("reading the models…", muted))),
        Some(choices) => {
            let shown = usize::from(list_rows).max(1);
            let top = if picker.cursor >= shown {
                picker.cursor + 1 - shown
            } else {
                0
            };
            for (i, choice) in choices.iter().enumerate().skip(top).take(shown) {
                let label = match &choice.entry {
                    Some(entry) => entry.clone(),
                    None => "Default · the tier policy's model".to_string(),
                };
                let window = choice
                    .window
                    .map_or(String::new(), |w| format!("  {}", compact(w)));
                let star = if choice.favourite { "★ " } else { "  " };
                let text = clip(
                    &format!("{star}{label}{window}"),
                    usize::from(width).saturating_sub(4),
                );
                let style = if i == picker.cursor {
                    Style::new().fg(palette.paper).bg(palette.accent)
                } else {
                    Style::new()
                };
                lines.push(Line::from(Span::styled(text, style)));
            }
        }
    }
    lines.push(Line::raw(""));
    let mut effort = vec![Span::styled("effort  ", muted)];
    for (i, (name, _)) in EFFORTS.iter().enumerate() {
        let style = if i == picker.effort {
            Style::new()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new()
        };
        effort.push(Span::styled(format!(" {name} "), style));
        effort.push(Span::raw(" "));
    }
    lines.push(Line::from(effort));
    if let Some(note) = &picker.note {
        lines.push(Line::from(Span::styled(
            clip(note, usize::from(width).saturating_sub(4)),
            Style::new().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "↑↓ model · Tab or h/l effort · Enter saves · Esc closes",
        muted,
    )));
    let block = Block::bordered()
        .title(" model · effort ")
        .border_style(Style::new().fg(palette.accent));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// Runs one request the views asked for, on the calling thread (the app calls it on its own thread). The answer is a
/// `Got` for the workspace to take.
pub fn answer(client: &Client, ask: Ask) -> Got {
    match ask {
        Ask::Fetch(owner) => {
            let result = client.get(&owner.context_path());
            Got::Context { owner, result }
        }
        Ask::Models => Got::Models(client.get("/v1/models")),
        Ask::Save { owner, model } => {
            let result = client.patch(&owner.model_path(), &json!({ "model": model }));
            Got::Saved { owner, result }
        }
    }
}

/// The model string a chat's `model` field holds, as the picker opens with it: `provider/model@effort`, or None.
pub fn model_of(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

/// The text of `/model <provider/model[@effort]|default>` as the entry the hub takes: None for the default. Only the
/// shape is checked here; the hub refuses a model outside the tier policy.
pub fn model_entry(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("usage: /model <provider/model[@effort]|default>".to_string());
    }
    if text == "default" {
        return Ok(None);
    }
    let (entry, level) = match text.split_once('@') {
        Some((entry, level)) => (entry, Some(level)),
        None => (text, None),
    };
    let well_formed = entry.split_once('/').is_some_and(|(provider, model)| {
        !provider.is_empty() && !model.is_empty() && !model.contains(char::is_whitespace)
    });
    if !well_formed {
        return Err(format!(
            "{text}: use provider/model, with an optional @effort"
        ));
    }
    match level {
        None => Ok(Some(entry.to_string())),
        Some(level) if EFFORTS.iter().any(|(_, s)| *s == Some(level)) => {
            Ok(Some(format!("{entry}@{level}")))
        }
        Some(level) => Err(format!(
            "{level}: effort is one of low, medium, high, max (or none for the default)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;
    use serde_json::json;

    use super::*;

    fn palette() -> Palette {
        Palette {
            accent: Color::Red,
            paper: Color::White,
            ok: Color::Green,
            muted: Color::DarkGray,
        }
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn sample() -> Context {
        Context {
            cid: 3,
            model: "anthropic/claude-sonnet-4-5@high".into(),
            window: Some(200_000),
            used: 41_200,
            parts: [2_000, 6_000, 25_000, 6_000, 0, 2_200],
            tools: vec![Tool {
                name: "bash".into(),
                calls: 12,
                errors: 2,
                result_tokens: 9_000,
            }],
            calls: 7,
            compactions: 1,
        }
    }

    #[test]
    fn counts_read_as_the_consoles_write_them() {
        assert_eq!(compact(812), "812");
        assert_eq!(compact(1_000), "1k");
        assert_eq!(compact(41_234), "41.2k");
        assert_eq!(compact(200_000), "200k");
        assert_eq!(compact(999_499), "999k");
        assert_eq!(compact(999_500), "1M");
        assert_eq!(compact(1_240_000), "1.2M");
        assert_eq!(compact(1_250_000), "1.3M", "halves round up");
        assert_eq!(compact(1_000_000), "1M");
        assert_eq!(percent(41_200, 200_000), 21);
        assert_eq!(percent(5, 0), 0);
    }

    #[test]
    fn the_bar_text_names_the_window_when_it_is_known() {
        assert_eq!(bar_text(&sample()), "41.2k / 200k · 21%");
        let unknown = Context {
            window: None,
            ..sample()
        };
        assert_eq!(bar_text(&unknown), "41.2k used");
    }

    #[test]
    fn the_tone_turns_warn_over_80_and_crit_over_95() {
        let at = |used| Context { used, ..sample() };
        assert_eq!(at(160_000).tone(), Tone::Calm, "80 % is not over 80");
        assert_eq!(at(170_000).tone(), Tone::Warn);
        assert_eq!(at(190_000).tone(), Tone::Warn, "95 % is not over 95");
        assert_eq!(at(191_000).tone(), Tone::Crit);
        let unknown = Context {
            window: None,
            used: 999_999,
            ..sample()
        };
        assert_eq!(unknown.tone(), Tone::Calm);
    }

    #[test]
    fn the_hub_answer_is_read_by_part_order_not_by_its_list_order() {
        let answer = json!({
            "cid": 3, "work": 9, "model": "anthropic/claude-sonnet-4-5", "window": 200000,
            "used": 41200, "output": 300,
            "parts": [{"part": "history", "tokens": 25000}, {"part": "system", "tokens": 2000}],
            "tools": [{"tool": "bash", "calls": 12, "errors": 2, "result_tokens": 9000}],
            "calls": 7, "compactions": 1, "updated_ms": 5
        });
        let ctx = Context::parse(&answer).expect("a context with used");
        assert_eq!(ctx.parts, [2_000, 0, 25_000, 0, 0, 0]);
        assert_eq!(ctx.window, Some(200_000));
        assert_eq!(ctx.tools[0].errors, 2);
        assert!(Context::parse(&json!({"cid": 3})).is_none());
        let no_window = Context::parse(&json!({"used": 5, "window": null})).unwrap();
        assert_eq!(no_window.window, None);
    }

    #[test]
    fn the_bar_fills_its_track_by_part_and_keeps_the_text_at_its_right() {
        let line = bar_line(Some(&sample()), 40, palette());
        let text = text_of(&line);
        assert_eq!(text.chars().count(), 40);
        assert!(text.ends_with("41.2k / 200k · 21%"), "{text}");
        let track: String = text.chars().take(40 - 19).collect();
        assert!(track.starts_with('█'), "the system part comes first");
        assert!(
            track.ends_with('░'),
            "the rest of the window is empty: {track}"
        );
        // The parts add up to the 41.2k used: 41.2k of 200k is 4 of the 21 track cells.
        let filled = track.chars().filter(|c| *c == '█').count();
        assert_eq!(filled, 4);
    }

    #[test]
    fn the_bar_says_reading_until_the_context_is_known_and_shortens_when_narrow() {
        assert_eq!(
            text_of(&bar_line(None, 40, palette())),
            "context · reading…"
        );
        assert_eq!(
            text_of(&bar_line(Some(&sample()), 12, palette())),
            "41.2k / 200…"
        );
    }

    #[test]
    fn the_breakdown_lists_each_part_its_tokens_and_share_and_the_tool_calls() {
        let lines: Vec<String> = plain(&sample());
        assert_eq!(lines[0], "anthropic/claude-sonnet-4-5@high · window 200k");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("history") && l.contains("25k") && l.contains("61%"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("tool results") && l.contains("6k"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("bash") && l.contains("12 calls") && l.contains("2 errors"))
        );
        assert_eq!(lines.last().unwrap(), "7 model calls · 1 compactions");
        let empty = Context {
            tools: Vec::new(),
            ..sample()
        };
        assert!(plain(&empty).iter().any(|l| l == "  No tool calls yet"));
        let styled: Vec<String> = breakdown_lines(Some(&sample()), palette())
            .iter()
            .map(text_of)
            .collect();
        assert!(
            styled.iter().any(|l| l.starts_with("█ history")),
            "{styled:?}"
        );
        assert_eq!(
            text_of(&breakdown_lines(None, palette())[0]),
            "reading the context…"
        );
    }

    #[test]
    fn a_model_argument_is_provider_model_with_an_optional_effort_or_default() {
        assert_eq!(model_entry("default"), Ok(None));
        assert_eq!(
            model_entry("anthropic/claude-sonnet-4-5"),
            Ok(Some("anthropic/claude-sonnet-4-5".into()))
        );
        assert_eq!(
            model_entry(" openai/gpt-x@max "),
            Ok(Some("openai/gpt-x@max".into()))
        );
        assert!(model_entry("").unwrap_err().starts_with("usage:"));
        assert!(model_entry("gpt-x").unwrap_err().contains("provider/model"));
        assert!(model_entry("a/b c").unwrap_err().contains("provider/model"));
        assert!(
            model_entry("a/b@turbo")
                .unwrap_err()
                .contains("effort is one of")
        );
    }

    #[test]
    fn the_picker_opens_on_the_chats_model_and_effort() {
        let picker = Picker::open(Owner::Agent(3), Some("anthropic/claude-sonnet-4-5@high"));
        assert_eq!(
            picker.current.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(EFFORTS[picker.effort].0, "High");
        let none = Picker::open(Owner::Agent(3), None);
        assert_eq!(none.current, None);
        assert_eq!(none.effort, 0);
    }

    #[test]
    fn the_picker_lists_favourites_first_under_the_default_and_saves_with_its_effort() {
        let listing = json!({"models": [
            {"provider": "openai", "id": "gpt-x", "favourite": false, "context": 128000},
            {"provider": "anthropic", "id": "claude-b", "favourite": true, "context": 200000},
            {"provider": "anthropic", "id": "claude-a", "favourite": false},
        ]});
        let mut picker = Picker::open(Owner::Conversation("conv-3".into()), Some("openai/gpt-x"));
        assert_eq!(picker.chosen(), None, "nothing is listed yet");
        assert_eq!(picker.key(Key::Enter), Stroke::Keep);
        assert!(picker.note.is_some());
        picker.load(&listing);
        let entries: Vec<Option<String>> = picker
            .choices
            .as_ref()
            .unwrap()
            .iter()
            .map(|c| c.entry.clone())
            .collect();
        assert_eq!(
            entries,
            vec![
                None,
                Some("anthropic/claude-b".into()),
                Some("anthropic/claude-a".into()),
                Some("openai/gpt-x".into()),
            ]
        );
        assert_eq!(picker.cursor, 3, "opens on the current model");
        assert_eq!(picker.key(Key::Char('k')), Stroke::Keep);
        assert_eq!(picker.key(Key::Tab), Stroke::Keep);
        assert_eq!(picker.key(Key::Tab), Stroke::Keep);
        assert_eq!(
            picker.key(Key::Enter),
            Stroke::Save {
                owner: Owner::Conversation("conv-3".into()),
                model: Some("anthropic/claude-a@medium".into()),
            }
        );
        picker.cursor = 0;
        picker.effort = 3;
        assert_eq!(
            picker.key(Key::Enter),
            Stroke::Save {
                owner: Owner::Conversation("conv-3".into()),
                model: None,
            },
            "the default row saves no model, whatever the effort"
        );
    }

    #[test]
    fn keys_move_the_picker_and_escape_closes_it_without_saving() {
        let mut picker = Picker::open(Owner::Agent(1), None);
        picker.load(&json!({"models": [{"provider": "a", "id": "b"}]}));
        assert_eq!(picker.key(Key::Up), Stroke::Keep);
        assert_eq!(picker.cursor, 0, "the cursor stops at the top");
        assert_eq!(picker.key(Key::Down), Stroke::Keep);
        assert_eq!(picker.key(Key::Down), Stroke::Keep);
        assert_eq!(picker.cursor, 1, "and at the last row");
        assert_eq!(picker.key(Key::BackTab), Stroke::Keep);
        assert_eq!(EFFORTS[picker.effort].0, "Max", "effort wraps back");
        assert_eq!(picker.key(Key::Esc), Stroke::Close);
        assert_eq!(picker.key(Key::CtrlC), Stroke::Close);
    }

    #[test]
    fn a_failed_listing_offers_the_default_and_says_why() {
        let mut picker = Picker::open(Owner::Agent(1), None);
        picker.fail("HTTP 503");
        assert_eq!(picker.note.as_deref(), Some("models: HTTP 503"));
        assert_eq!(picker.chosen(), Some(None));
    }

    #[test]
    fn owners_name_their_context_and_model_paths() {
        assert_eq!(Owner::Agent(7).context_path(), "/v1/agents/7/context");
        assert_eq!(Owner::Agent(7).model_path(), "/v1/agents/7");
        assert_eq!(
            Owner::Conversation("conv-3".into()).context_path(),
            "/v1/owner/conversations/conv-3/context"
        );
        assert_eq!(
            Owner::Conversation("conv-3".into()).model_path(),
            "/v1/owner/conversations/conv-3"
        );
    }
}
