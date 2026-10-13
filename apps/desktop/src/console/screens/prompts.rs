//! The Prompts list (docs/design/prompt-library.md): every prompt in the library with its kind, binding, live and best
//! versions and score, filtered by kind, archived state and a search. A new prompt is made in an inline form (kind,
//! title, optional id, binding, description, tags, and a body prefilled from the kind's template); creating one opens
//! it in the editor. The editor takes over the screen until it returns with `Back`, and the list reloads.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, checkbox, column, row, scrollable, text, text_editor, text_input};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::json;

use super::{prompt_editor, Id};
use crate::console::{client, ui, Ctx};


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
    let path = (if st.show_archived { "/v1/prompts?archived=true" } else { "/v1/prompts" }).to_string();
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
    }
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
        st.new_body = text_editor::Content::with_text(&apz_prompt::template(api_kind(st.new_kind)).to_string());
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
    let binding: Option<String> = match st.new_kind {
        PKind::System => {
            let role = st.new_role.trim();
            (!role.is_empty()).then(|| format!("role:{role}"))
        }
        PKind::AgentsMd => st.new_all.then(|| "plaza".to_string()),
        PKind::Fragment => None,
    };
    let tags: Vec<String> = st
        .new_tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect();
    let mut payload = json!({
        "kind": wire(st.new_kind),
        "title": title,
        "description": st.new_description.trim(),
        "tags": tags,
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

pub fn view<'a>(st: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    if let Some(ed) = st.editor.as_ref() {
        return prompt_editor::view(ed, ctx).map(Msg::Editor);
    }
    let head = ui::view_head(
        "Prompts",
        vec![ui::btn(
            if st.form_open { "Close" } else { "New prompt" },
            None,
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
    let toolbar = row![
        ui::seg(&kinds, st.filter, Msg::Filter),
        ui::seg(&[(false, "Hide archived"), (true, "Show archived")], st.show_archived, Msg::Archived),
    ]
    .spacing(12);
    let search = text_input("Search id, title, description or tags", &st.query)
        .on_input(Msg::Query)
        .padding(8)
        .style(crate::theme::input);

    let mut page = column![head, toolbar, search].spacing(14);
    if st.form_open {
        page = page.push(form_view(st));
    }
    if st.loading {
        page = page.push(ui::loading());
    }
    if let Some(e) = st.error.as_ref() {
        page = page.push(column![
            ui::error_line(e),
            ui::btn_sm("Try again", None, ui::Kind::Secondary, Some(Msg::Retry)),
        ]
        .spacing(8));
    }

    let needle = st.query.trim().to_lowercase();
    let visible: Vec<&Summary> = st
        .prompts
        .iter()
        .filter(|p| st.show_archived || p.archived_ms.is_none())
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
                    ui::empty("file", "Nothing matches", "Try another kind, or clear the search."),
                    ui::btn_sm("Clear filters", None, ui::Kind::Secondary, Some(Msg::ClearFilters)),
                ]
                .spacing(12),
            );
        }
    }

    let mut list = column![].spacing(10);
    for p in visible {
        list = list.push(row_card(p));
    }
    page = page.push(list);

    scrollable(page.padding(16)).width(Length::Fill).into()
}

fn row_card(p: &Summary) -> Element<'_, Msg> {
    let muted = crate::theme::palette().ink_3;
    let mut badges: Vec<Element<'_, Msg>> = vec![ui::badge(kind_label(&p.kind), ui::Tone::Muted)];
    if let Some(b) = p.binding.as_deref() {
        badges.push(ui::badge(&binding_label(b), ui::Tone::Info));
    }
    match p.active_version {
        Some(v) => badges.push(ui::badge(&format!("v{v} live"), ui::Tone::Ok)),
        None => {
            let label = match p.latest_version {
                Some(v) => format!("Unpublished, latest v{v}"),
                None => "Unpublished".to_string(),
            };
            badges.push(ui::badge(&label, ui::Tone::Warn));
        }
    }
    if let Some(s) = p.score {
        badges.push(ui::badge(&format!("score {s:.0}"), score_tone(s)));
    }
    if let Some(b) = p.best_version {
        if Some(b) != p.active_version {
            badges.push(ui::badge(&format!("best: v{b}"), ui::Tone::Muted));
        }
    }
    if p.archived_ms.is_some() {
        badges.push(ui::badge("Archived", ui::Tone::Muted));
    }

    let mut body = column![
        button(text(p.title.as_str()).size(16.0))
            .on_press(Msg::Open(p.id.clone()))
            .style(button::text),
        row(badges).spacing(6),
    ]
    .spacing(8);
    if !p.description.is_empty() {
        body = body.push(text(ui::ellipsize(&p.description, 200)).size(14.0));
    }
    if !p.tags.is_empty() {
        body = body.push(text(p.tags.join(" · ")).size(12.0).color(muted));
    }
    body = body.push(text(format!("updated {}", ago(p.updated_ms))).size(12.0).color(muted));
    ui::card(&p.id, body.into())
}

fn field<'a>(label: &str, input: Element<'a, Msg>) -> Element<'a, Msg> {
    column![text(label.to_string()).size(12.0).color(crate::theme::palette().ink_3), input]
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
        PKind::AgentsMd => checkbox(st.new_all).label("Apply to every agent").on_toggle(Msg::NewAll).into(),
        PKind::Fragment => text("Fragments are not bound to an agent.").size(13.0).into(),
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
