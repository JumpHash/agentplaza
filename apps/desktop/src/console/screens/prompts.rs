//! The Prompts list (docs/design/prompt-library.md): every prompt in the library with its kind, binding, live and best
//! versions and score, filtered by kind, archived state and a search. A new prompt is made in an inline form (kind,
//! title, optional id, binding, description, tags, and a body prefilled from the kind's template); creating one opens
//! it in the editor. The editor takes over the screen until it returns with `Back`, and the list reloads.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{
    Space, button, checkbox, column, container, row, text, text_editor, text_input,
};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Id, prompt_editor};
use crate::console::{Ctx, client, events, ui};

#[derive(Debug, Clone, Deserialize)]
pub struct Summary {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub binding: Option<String>,
    pub active_version: Option<i64>,
    pub latest_version: Option<i64>,
    pub best_version: Option<i64>,
    pub score: Option<f64>,
    pub updated_ms: i64,
    pub archived_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListResp {
    pub prompts: Vec<Summary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OneResp {
    pub prompt: Summary,
}

/// The kind filter above the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    System,
    AgentsMd,
    Fragment,
}

impl Filter {
    fn matches(self, kind: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::System => kind == "system",
            Filter::AgentsMd => kind == "agents_md",
            Filter::Fragment => kind == "fragment",
        }
    }
}

/// The kind of a new prompt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PKind {
    #[default]
    System,
    AgentsMd,
    Fragment,
}

fn wire(k: PKind) -> &'static str {
    match k {
        PKind::System => "system",
        PKind::AgentsMd => "agents_md",
        PKind::Fragment => "fragment",
    }
}

fn api_kind(k: PKind) -> apz_prompt::Kind {
    match k {
        PKind::System => apz_prompt::Kind::System,
        PKind::AgentsMd => apz_prompt::Kind::AgentsMd,
        PKind::Fragment => apz_prompt::Kind::Fragment,
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Loaded(Result<ListResp, String>),
    /// Open a prompt in the editor.
    Open(String),
    Go(Id),
    Filter(Filter),
    Archived(bool),
    Query(String),
    FormToggle,
    NewKind(PKind),
    NewTitle(String),
    NewId(String),
    NewRole(String),
    NewAll(bool),
    NewDescription(String),
    NewTags(String),
    NewBody(text_editor::Action),
    Create,
    Created(Result<OneResp, String>),
    /// Load the list again after a failure.
    Retry,
    /// Clear the kind filter and the search.
    ClearFilters,
    Editor(prompt_editor::Msg),
    /// A hub event: a prompt that changed reloads the list, or the editor when it is the one open.
    Event(events::Event),
}

#[derive(Debug, Default)]
pub struct State {
    loading: bool,
    error: Option<String>,
    prompts: Vec<Summary>,
    filter: Filter,
    show_archived: bool,
    query: String,
    form_open: bool,
    new_kind: PKind,
    new_title: String,
    new_id: String,
    new_role: String,
    new_all: bool,
    new_description: String,
    new_tags: String,
    new_body: text_editor::Content,
    body_touched: bool,
    creating: bool,
    form_error: Option<String>,
    editor: Option<prompt_editor::State>,
}

/// The screen navigation a message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Fetch the list (archived included when the toggle is on).
pub fn load(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.loading = true;
    st.error = None;
    let path = (if st.show_archived {
        "/v1/prompts?archived=true"
    } else {
        "/v1/prompts"
    })
    .to_string();
    let c = ctx.client.clone();
    client::task(move || c.get::<ListResp>(&path), Msg::Loaded)
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Loaded(res) => {
            st.loading = false;
            match res {
                Ok(r) => st.prompts = r.prompts,
                Err(e) => st.error = Some(e),
            }
            Task::none()
        }
        Msg::Open(id) => open_editor(st, id, ctx),
        Msg::Go(_) => Task::none(),
        Msg::Filter(f) => {
            st.filter = f;
            Task::none()
        }
        Msg::Archived(on) => {
            st.show_archived = on;
            load(st, ctx)
        }
        Msg::Query(q) => {
            st.query = q;
            Task::none()
        }
        Msg::FormToggle => {
            st.form_open = !st.form_open;
            st.form_error = None;
            refill(st);
            Task::none()
        }
        Msg::NewKind(k) => {
            st.new_kind = k;
            refill(st);
            Task::none()
        }
        Msg::NewTitle(v) => {
            st.new_title = v;
            Task::none()
        }
        Msg::NewId(v) => {
            st.new_id = v;
            Task::none()
        }
        Msg::NewRole(v) => {
            st.new_role = v;
            Task::none()
        }
        Msg::NewAll(v) => {
            st.new_all = v;
            Task::none()
        }
        Msg::NewDescription(v) => {
            st.new_description = v;
            Task::none()
        }
        Msg::NewTags(v) => {
            st.new_tags = v;
            Task::none()
        }
        Msg::NewBody(action) => {
            if action.is_edit() {
                st.body_touched = true;
            }
            st.new_body.perform(action);
            Task::none()
        }
        Msg::Create => create(st, ctx),
        Msg::Created(res) => {
            st.creating = false;
            match res {
                Ok(r) => {
                    st.form_open = false;
                    reset_form(st);
                    open_editor(st, r.prompt.id, ctx)
                }
                Err(e) => {
                    st.form_error = Some(if e.contains("409") {
                        "That id or binding is already taken.".to_string()
                    } else {
                        e
                    });
                    Task::none()
                }
            }
        }
        Msg::Retry => load(st, ctx),
        Msg::ClearFilters => {
            st.filter = Filter::All;
            st.query.clear();
            Task::none()
        }
        Msg::Editor(prompt_editor::Msg::Back) => {
            // With unsaved edits, the editor asks first (Discard leaves; Keep editing stays).
            let dirty = st.editor.as_ref().is_some_and(prompt_editor::is_dirty);
            if dirty {
                if let Some(ed) = st.editor.as_mut() {
                    prompt_editor::ask_leave(ed);
                }
                Task::none()
            } else {
                leave_editor(st, ctx)
            }
        }
        Msg::Editor(prompt_editor::Msg::Discard) => leave_editor(st, ctx),
        Msg::Editor(m) => match st.editor.as_mut() {
            Some(ed) => prompt_editor::update(ed, m, ctx).map(Msg::Editor),
            None => Task::none(),
        },
        Msg::Event(event) => {
            if event.kind != "prompt" {
                return Task::none();
            }
            let id = event
                .data
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match st.editor.as_mut() {
                Some(ed) if ed.id == id => prompt_editor::hub_changed(ed, ctx).map(Msg::Editor),
                Some(_) => Task::none(),
                None if st.loading => Task::none(),
                None => load(st, ctx),
            }
        }
    }
}

/// The hub's events while the Prompts screen is shown.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Close the editor and reload the list.
fn leave_editor(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.editor = None;
    load(st, ctx)
}

fn open_editor(st: &mut State, id: String, ctx: &Ctx) -> Task<Msg> {
    let (ed, task) = prompt_editor::open(id, ctx);
    st.editor = Some(ed);
    task.map(Msg::Editor)
}

/// Prefill the body from the kind's template while the author has not typed in it.
fn refill(st: &mut State) {
    if !st.body_touched {
        st.new_body = text_editor::Content::with_text(apz_prompt::template(api_kind(st.new_kind)));
    }
}

fn reset_form(st: &mut State) {
    st.new_title.clear();
    st.new_id.clear();
    st.new_role.clear();
    st.new_all = false;
    st.new_description.clear();
    st.new_tags.clear();
    st.new_body = text_editor::Content::default();
    st.body_touched = false;
}

fn create(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let title = st.new_title.trim().to_string();
    if title.is_empty() {
        st.form_error = Some("Give the prompt a title.".to_string());
        return Task::none();
    }
    let binding = prompt_editor::binding_for(api_kind(st.new_kind), &st.new_role, st.new_all);
    let mut payload = json!({
        "kind": wire(st.new_kind),
        "title": title,
        "description": st.new_description.trim(),
        "tags": prompt_editor::parse_tags(&st.new_tags),
        "body": st.new_body.text(),
    });
    let id = st.new_id.trim().to_string();
    if let Some(obj) = payload.as_object_mut() {
        if !id.is_empty() {
            obj.insert("id".to_string(), json!(id));
        }
        if let Some(b) = binding {
            obj.insert("binding".to_string(), json!(b));
        }
    }
    st.creating = true;
    st.form_error = None;
    let c = ctx.client.clone();
    let path = "/v1/prompts".to_string();
    client::task(move || c.post::<OneResp>(&path, &payload), Msg::Created)
}

fn ago(ms: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(ms);
    let secs = ((now - ms) / 1000).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

fn kind_label(k: &str) -> &'static str {
    match k {
        "system" => "System prompt",
        "agents_md" => "AGENTS.md",
        "fragment" => "Fragment",
        _ => "Prompt",
    }
}

fn binding_label(b: &str) -> String {
    if let Some(role) = b.strip_prefix("role:") {
        format!("Role: {role}")
    } else if b == "plaza" {
        "Whole plaza".to_string()
    } else {
        b.to_string()
    }
}

fn score_tone(s: f64) -> ui::Tone {
    if s >= 90.0 {
        ui::Tone::Ok
    } else if s >= 70.0 {
        ui::Tone::Warn
    } else {
        ui::Tone::Danger
    }
}

fn matches_query(p: &Summary, needle: &str) -> bool {
    p.id.to_lowercase().contains(needle)
        || p.title.to_lowercase().contains(needle)
        || p.description.to_lowercase().contains(needle)
        || p.tags.iter().any(|t| t.to_lowercase().contains(needle))
}

/// The Prompts' intro under the head (the web's wording).
const INTRO: &str = "The prompt library: system prompts, AGENTS.md files and fragments. Every version is checked and scored.";

/// The table's columns, by their share of the width: title, kind, binding, live, score, tags, updated.
const COLUMNS: [u16; 7] = [4, 2, 2, 1, 2, 3, 2];

pub fn view<'a>(st: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    if let Some(ed) = st.editor.as_ref() {
        return prompt_editor::view(ed, ctx).map(Msg::Editor);
    }
    let head = ui::view_head_with_intro(
        "Prompts",
        INTRO,
        vec![ui::btn(
            if st.form_open { "Close" } else { "New prompt" },
            Some("plus"),
            ui::Kind::Primary,
            Some(Msg::FormToggle),
        )],
    );
    let kinds = [
        (Filter::All, "All"),
        (Filter::System, "System prompts"),
        (Filter::AgentsMd, "AGENTS.md"),
        (Filter::Fragment, "Fragments"),
    ];
    // The tabs count the rows the archived toggle leaves, per kind.
    let live: Vec<&Summary> = st
        .prompts
        .iter()
        .filter(|p| st.show_archived || p.archived_ms.is_none())
        .collect();
    let labels: Vec<String> = kinds
        .iter()
        .map(|(f, name)| {
            format!(
                "{name} {}",
                live.iter().filter(|p| f.matches(&p.kind)).count()
            )
        })
        .collect();
    let options: Vec<(Filter, &str)> = kinds
        .iter()
        .zip(labels.iter())
        .map(|((f, _), label)| (*f, label.as_str()))
        .collect();
    let tabs = ui::tabs(&options, st.filter, Msg::Filter);
    let search = text_input("Search id, title, description or tags", &st.query)
        .on_input(Msg::Query)
        .padding(8)
        .style(crate::theme::input)
        .width(Length::Fill);
    let bar = row![
        search,
        checkbox(st.show_archived)
            .label("Show archived")
            .on_toggle(Msg::Archived),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut page = column![tabs, bar].spacing(12);
    if st.form_open {
        page = page.push(form_view(st));
    }
    if st.loading {
        page = page.push(ui::loading());
    }
    if let Some(e) = st.error.as_ref() {
        page = page.push(
            column![
                ui::error_line(e),
                ui::btn_sm("Try again", None, ui::Kind::Secondary, Some(Msg::Retry)),
            ]
            .spacing(8),
        );
    }

    let needle = st.query.trim().to_lowercase();
    let visible: Vec<&Summary> = live
        .iter()
        .copied()
        .filter(|p| st.filter.matches(&p.kind))
        .filter(|p| needle.is_empty() || matches_query(p, &needle))
        .collect();

    if !st.loading && st.error.is_none() {
        if st.prompts.is_empty() {
            page = page.push(ui::empty(
                "file",
                "No prompts yet",
                "The library holds the system prompts, AGENTS.md files and fragments that the plaza's agents read. Create one to start.",
            ));
        } else if visible.is_empty() {
            page = page.push(
                column![
                    ui::empty("search", "No prompts match these filters.", ""),
                    ui::btn_sm(
                        "Clear filters",
                        None,
                        ui::Kind::Ghost,
                        Some(Msg::ClearFilters)
                    ),
                ]
                .spacing(12),
            );
        } else {
            page = page.push(table(&visible));
        }
    }

    ui::sheet(head, ui::scroll(page).width(Length::Fill).into())
}

/// The prompts as the web's table: a head row in the label voice, one row a prompt (its title and id, kind, binding,
/// live version, score, tags and age); a press opens the prompt in the editor.
fn table<'a>(rows: &[&'a Summary]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let heads = [
        "Title", "Kind", "Binding", "Live", "Score", "Tags", "Updated",
    ];
    let head = row(heads
        .iter()
        .zip(COLUMNS)
        .map(|(name, portion)| {
            container(ui::label(name))
                .width(Length::FillPortion(portion))
                .into()
        })
        .collect::<Vec<Element<'a, Msg>>>())
    .spacing(0);
    let mut body = column![
        container(head).padding([9, 12]).width(Length::Fill),
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_strong.into()),
                ..Default::default()
            }),
    ];
    for &prompt in rows {
        let muted = p.ink_3;
        let mut title = column![
            text(prompt.title.as_str())
                .font(crate::theme::SANS_SEMIBOLD)
                .size(14.0)
                .color(p.ink)
        ];
        title = title.push(ui::mono(prompt.id.as_str(), 11.0, muted));
        let binding: Element<'_, Msg> = match prompt.binding.as_deref() {
            Some(b) => ui::badge(&binding_label(b), ui::Tone::Info),
            None => ui::faint("—").into(),
        };
        let live: Element<'_, Msg> = match prompt.active_version {
            Some(v) => text(format!("v{v}")).size(13.0).color(p.ink).into(),
            None => ui::faint("Unpublished").into(),
        };
        let mut score = column![].spacing(4);
        match prompt.score {
            Some(s) => score = score.push(ui::badge(&format!("{s:.0}"), score_tone(s))),
            None => score = score.push(ui::faint("—")),
        }
        if let Some(b) = prompt.best_version
            && Some(b) != prompt.active_version
        {
            score = score.push(ui::faint(format!("best: v{b}")));
        }
        let tags: Vec<Element<'_, Msg>> = prompt
            .tags
            .iter()
            .map(|t| ui::badge(t, ui::Tone::Muted))
            .collect();
        let cells: Vec<Element<'_, Msg>> = vec![
            container(title)
                .width(Length::FillPortion(COLUMNS[0]))
                .into(),
            container(text(kind_label(&prompt.kind)).size(13.0).color(p.ink_2))
                .width(Length::FillPortion(COLUMNS[1]))
                .into(),
            container(binding)
                .width(Length::FillPortion(COLUMNS[2]))
                .into(),
            container(live)
                .width(Length::FillPortion(COLUMNS[3]))
                .into(),
            container(score)
                .width(Length::FillPortion(COLUMNS[4]))
                .into(),
            container(ui::chip_row(tags))
                .width(Length::FillPortion(COLUMNS[5]))
                .into(),
            container(ui::faint(ago(prompt.updated_ms)))
                .width(Length::FillPortion(COLUMNS[6]))
                .into(),
        ];
        let line = button(row(cells).spacing(0).align_y(Alignment::Center))
            .on_press(Msg::Open(prompt.id.clone()))
            .style(button::text)
            .padding([9, 12])
            .width(Length::Fill);
        body = body.push(line);
        body = body.push(
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule.into()),
                    ..Default::default()
                }),
        );
    }
    container(body)
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A form field: its label above its input.
pub(super) fn field<'a, M: 'a>(label: &str, input: Element<'a, M>) -> Element<'a, M> {
    column![
        text(label.to_string())
            .size(12.0)
            .color(crate::theme::palette().ink_3),
        input
    ]
    .spacing(4)
    .into()
}

fn form_view(st: &State) -> Element<'_, Msg> {
    let kinds = [
        (PKind::System, "System prompt"),
        (PKind::AgentsMd, "AGENTS.md"),
        (PKind::Fragment, "Fragment"),
    ];
    let binding: Element<'_, Msg> = match st.new_kind {
        PKind::System => text_input("Role, for example builder", &st.new_role)
            .on_input(Msg::NewRole)
            .padding(8)
            .style(crate::theme::input)
            .into(),
        PKind::AgentsMd => checkbox(st.new_all)
            .label("Apply to every agent")
            .on_toggle(Msg::NewAll)
            .into(),
        PKind::Fragment => text("Fragments are not bound to an agent.")
            .size(13.0)
            .into(),
    };
    let mut col = column![
        field("Kind", ui::seg(&kinds, st.new_kind, Msg::NewKind)),
        field(
            "Title",
            text_input("Title", &st.new_title)
                .on_input(Msg::NewTitle)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        field(
            "Id (optional)",
            text_input("Left empty, the id is made from the title", &st.new_id)
                .on_input(Msg::NewId)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        field("Binding", binding),
        field(
            "Description",
            text_input("What this prompt is for", &st.new_description)
                .on_input(Msg::NewDescription)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        field(
            "Tags (comma separated)",
            text_input("review, safety", &st.new_tags)
                .on_input(Msg::NewTags)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        field(
            "Body",
            text_editor(&st.new_body)
                .style(crate::theme::editor)
                .on_action(Msg::NewBody)
                .padding(8)
                .height(Length::Fixed(240.0))
                .into(),
        ),
    ]
    .spacing(12);
    if let Some(e) = st.form_error.as_ref() {
        col = col.push(ui::error_line(e));
    }
    col = col.push(
        row![
            ui::btn(
                if st.creating { "Creating..." } else { "Create" },
                None,
                ui::Kind::Primary,
                (!st.creating).then_some(Msg::Create),
            ),
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::FormToggle)),
        ]
        .spacing(8),
    );
    ui::card("New prompt", col.into())
}
