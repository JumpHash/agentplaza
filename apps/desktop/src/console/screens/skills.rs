//! Skills: the skill library (docs/design/skill-library.md). The list with filters (source, binding, archived) and a
//! search, and a selection to export; New skill (from the template); Import (a zip, a .skill file or a folder on this
//! computer) and From GitHub, each shown as a preview with checks, then picked and committed. Opening one shows the
//! editor (`skill_editor`). Imported content is data: nothing here runs it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_skill::{Entry, find_skills, write_zip};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, text, text_editor, text_input,
};
use iced::{Element, Length, Subscription, Task};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Id, skill_editor};
use crate::console::{Ctx, client, events, ui};

/// The binding filter's first two choices.
const ANY: &str = "Any binding";
const NOT_BOUND: &str = "Not bound";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Built,
    Upload,
    Github { repo: String, commit: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<String>,
    pub active_version: Option<i64>,
    pub latest_version: i64,
    pub best_version: Option<i64>,
    pub score: Option<f64>,
    pub source: Source,
    pub updated_ms: i64,
    pub archived_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListResp {
    pub skills: Vec<Summary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OneResp {
    pub skill: Summary,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitResp {
    pub skills: Vec<Summary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RulesResp {
    pub template: String,
}

/// A role as GET /v1/roles lists it: its binding is `role:<id>`.
#[derive(Debug, Clone, Deserialize)]
pub struct Role {
    pub id: String,
    pub title: String,
}

/// The bindings a create or import form picks: every agent, and the roles ticked from the roles list.
#[derive(Debug, Clone, Default)]
pub struct Picks {
    every: bool,
    roles: BTreeSet<String>,
}

impl Picks {
    /// The binding strings: `plaza` first when every agent is ticked, then `role:<id>` for each role.
    fn bindings(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if self.every {
            out.push("plaza".to_string());
        }
        out.extend(self.roles.iter().map(|r| format!("role:{r}")));
        out
    }
}

/// Which form a bindings pick belongs to: New skill, or the import preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    New,
    Preview,
}

/// An import or GitHub preview: the skills found, each checked.
#[derive(Debug, Clone, Deserialize)]
pub struct Preview {
    pub preview_id: String,
    pub source: Source,
    pub skills: Vec<PreviewSkill>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewSkill {
    pub path: String,
    pub name: Option<String>,
    pub title: String,
    pub report: Report,
    #[serde(default)]
    pub files: u64,
    pub exists: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub warnings: u64,
}

/// The source filter above the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SourceFilter {
    #[default]
    All,
    Built,
    Upload,
    Github,
}

/// The panel open under the toolbar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Form {
    #[default]
    Closed,
    New,
    Import,
    Github,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<ListResp, String>),
    Rules(Result<RulesResp, String>),
    Roles(Result<Vec<Role>, String>),
    /// Load the list again after a failure.
    Retry,
    /// Clear the source and binding filters and the search.
    ClearFilters,
    Filter(SourceFilter),
    Binding(String),
    Archived(bool),
    Query(String),
    /// Tick or untick a row for export.
    Select(String, bool),
    Open(String),
    Editor(skill_editor::Msg),
    Form(Form),
    NewTitle(String),
    NewId(String),
    /// Tick every agent, or a role, for a form's bindings.
    Every(Target, bool),
    Role(Target, String, bool),
    NewBody(text_editor::Action),
    Create,
    Created(Result<OneResp, String>),
    ChooseFile,
    ChooseFolder,
    /// A file or a folder on disk; a folder is zipped first.
    Picked(Option<PathBuf>),
    Previewed(Result<Preview, String>),
    GithubUrl(String),
    GithubPreview,
    /// Tick or untick a skill of the preview.
    PreviewPick(String, bool),
    ClosePreview,
    Commit,
    Committed(Result<CommitResp, String>),
    ExportSelected,
    ExportPicked(Option<(PathBuf, Vec<String>)>),
    Exported(Result<String, String>),
    /// A hub event: a skill that changed reloads the list, or the editor when it is the one open.
    Event(events::Event),
}

#[derive(Debug, Default)]
pub struct State {
    loading: bool,
    error: Option<String>,
    /// A line under the toolbar: (text, is an error).
    notice: Option<(String, bool)>,
    skills: Vec<Summary>,
    template: String,
    /// The roles list, for the bindings pickers.
    roles: Vec<Role>,
    filter_source: SourceFilter,
    /// Empty for any binding, else a binding or NOT_BOUND.
    binding: String,
    show_archived: bool,
    query: String,
    selected: BTreeSet<String>,
    form: Form,
    new_title: String,
    new_id: String,
    new_picks: Picks,
    new_body: text_editor::Content,
    body_touched: bool,
    creating: bool,
    form_error: Option<String>,
    github_url: String,
    busy: bool,
    preview: Option<Preview>,
    picks: BTreeSet<String>,
    preview_picks: Picks,
    committing: bool,
    editor: Option<skill_editor::State>,
}

/// The screen navigation a message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Fetch the list (archived included when the toggle is on) and the roles, and the template once.
pub fn load(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.loading = true;
    st.error = None;
    let path: String = (if st.show_archived {
        "/v1/skills?archived=true"
    } else {
        "/v1/skills"
    })
    .to_string();
    let c = ctx.client.clone();
    let list = client::task(move || c.get::<ListResp>(&path), Msg::Loaded);
    let c = ctx.client.clone();
    let roles = client::task(move || c.get::<Vec<Role>>("/v1/roles"), Msg::Roles);
    if !st.template.is_empty() {
        return Task::batch([list, roles]);
    }
    let c = ctx.client.clone();
    let rules = client::task(move || c.get::<RulesResp>("/v1/skills/rules"), Msg::Rules);
    Task::batch([list, roles, rules])
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(res) => {
            st.loading = false;
            match res {
                Ok(r) => {
                    st.skills = r.skills;
                    let skills = &st.skills;
                    st.selected.retain(|id| skills.iter().any(|s| &s.id == id));
                }
                Err(e) => st.error = Some(e),
            }
            Task::none()
        }
        Msg::Rules(Ok(r)) => {
            st.template = r.template;
            Task::none()
        }
        Msg::Rules(Err(e)) => {
            st.notice = Some((e, true));
            Task::none()
        }
        Msg::Roles(Ok(mut roles)) => {
            // Every bindings picker lists roles by title (the web's and the editor's).
            roles.sort_by(|a, b| a.title.cmp(&b.title));
            st.roles = roles;
            Task::none()
        }
        Msg::Roles(Err(e)) => {
            st.notice = Some((e, true));
            Task::none()
        }
        Msg::Retry => load(st, ctx),
        Msg::ClearFilters => {
            st.filter_source = SourceFilter::All;
            st.binding.clear();
            st.query.clear();
            Task::none()
        }
        Msg::Filter(f) => {
            st.filter_source = f;
            Task::none()
        }
        Msg::Binding(choice) => {
            st.binding = if choice == ANY { String::new() } else { choice };
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
        Msg::Select(id, on) => {
            if on {
                st.selected.insert(id);
            } else {
                st.selected.remove(&id);
            }
            Task::none()
        }
        Msg::Open(id) => open_editor(st, id, ctx),
        Msg::Editor(skill_editor::Msg::Back) => {
            // With unsaved edits, the editor asks first (Discard leaves; Keep editing stays).
            let dirty = st.editor.as_ref().is_some_and(skill_editor::is_dirty);
            if dirty {
                if let Some(ed) = st.editor.as_mut() {
                    skill_editor::ask_leave(ed);
                }
                Task::none()
            } else {
                leave_editor(st, ctx)
            }
        }
        Msg::Editor(skill_editor::Msg::Discard) => leave_editor(st, ctx),
        Msg::Editor(m) => match st.editor.as_mut() {
            Some(ed) => skill_editor::update(ed, m, ctx).map(Msg::Editor),
            None => Task::none(),
        },
        Msg::Form(f) => {
            st.form = if st.form == f { Form::Closed } else { f };
            st.form_error = None;
            if st.form == Form::New {
                refill(st);
            }
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
        Msg::Every(target, on) => {
            picks_mut(st, target).every = on;
            Task::none()
        }
        Msg::Role(target, role, on) => {
            let picks = picks_mut(st, target);
            if on {
                picks.roles.insert(role);
            } else {
                picks.roles.remove(&role);
            }
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
        Msg::Created(Ok(r)) => {
            st.creating = false;
            st.form = Form::Closed;
            reset_form(st);
            open_editor(st, r.skill.id, ctx)
        }
        Msg::Created(Err(e)) => {
            st.creating = false;
            st.form_error = Some(e);
            Task::none()
        }
        Msg::ChooseFile => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a skill archive")
                    .add_filter("Skill archives", &["zip", "skill"])
                    .pick_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            Msg::Picked,
        ),
        Msg::ChooseFolder => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a folder of skills")
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().to_path_buf())
            },
            Msg::Picked,
        ),
        Msg::Picked(None) => Task::none(),
        Msg::Picked(Some(path)) => {
            st.busy = true;
            st.notice = None;
            let c = ctx.client.clone();
            client::task(
                move || {
                    let bytes = if path.is_dir() {
                        folder_zip(&path)?
                    } else {
                        std::fs::read(&path)
                            .map_err(|e| format!("The file could not be read: {e}"))?
                    };
                    zip_preview(&c, &bytes)
                },
                Msg::Previewed,
            )
        }
        Msg::Previewed(Ok(p)) => {
            st.busy = false;
            st.picks = p
                .skills
                .iter()
                .filter(|s| s.report.errors == 0 && s.exists.is_none())
                .map(|s| s.path.clone())
                .collect();
            st.preview_picks = Picks::default();
            st.preview = Some(p);
            Task::none()
        }
        Msg::Previewed(Err(e)) => {
            st.busy = false;
            st.notice = Some((e, true));
            Task::none()
        }
        Msg::GithubUrl(v) => {
            st.github_url = v;
            Task::none()
        }
        Msg::GithubPreview => {
            let url = st.github_url.trim().to_string();
            if url.is_empty() {
                return Task::none();
            }
            st.busy = true;
            st.notice = None;
            let c = ctx.client.clone();
            client::task(
                move || c.post::<Preview>("/v1/skills/github/preview", &json!({ "url": url })),
                Msg::Previewed,
            )
        }
        Msg::PreviewPick(path, on) => {
            if on {
                st.picks.insert(path);
            } else {
                st.picks.remove(&path);
            }
            Task::none()
        }
        Msg::ClosePreview => {
            st.preview = None;
            st.picks.clear();
            Task::none()
        }
        Msg::Commit => {
            let Some(p) = st.preview.as_ref() else {
                return Task::none();
            };
            let body = json!({
                "preview_id": p.preview_id,
                "picks": st.picks.iter().collect::<Vec<_>>(),
                "bindings": st.preview_picks.bindings(),
            });
            st.committing = true;
            let c = ctx.client.clone();
            client::task(
                move || c.post::<CommitResp>("/v1/skills/import/commit", &body),
                Msg::Committed,
            )
        }
        Msg::Committed(Ok(r)) => {
            st.committing = false;
            st.preview = None;
            st.picks.clear();
            st.form = Form::Closed;
            st.notice = Some((format!("Imported {} skill(s).", r.skills.len()), false));
            load(st, ctx)
        }
        Msg::Committed(Err(e)) => {
            st.committing = false;
            st.notice = Some((e, true));
            Task::none()
        }
        Msg::ExportSelected => {
            let ids: Vec<String> = st.selected.iter().cloned().collect();
            let name = if ids.len() == 1 {
                "skill.zip"
            } else {
                "skills.zip"
            };
            Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_title("Save the skills as a zip")
                        .set_file_name(name)
                        .add_filter("Zip", &["zip"])
                        .save_file()
                        .await
                        .map(|file| (file.path().to_path_buf(), ids))
                },
                Msg::ExportPicked,
            )
        }
        Msg::ExportPicked(None) => Task::none(),
        Msg::ExportPicked(Some((path, ids))) => {
            let c = ctx.client.clone();
            let p = format!("/v1/skills/export?ids={}", ids.join(","));
            client::task(
                move || {
                    let bytes = raw(&c, "GET", &p, None)?;
                    std::fs::write(&path, &bytes)
                        .map_err(|e| format!("The file could not be written: {e}"))?;
                    Ok(path.display().to_string())
                },
                Msg::Exported,
            )
        }
        Msg::Exported(Ok(path)) => {
            st.notice = Some((format!("Saved to {path}"), false));
            Task::none()
        }
        Msg::Exported(Err(e)) => {
            st.notice = Some((e, true));
            Task::none()
        }
        Msg::Event(event) => {
            if event.kind != "skill" {
                return Task::none();
            }
            let id = event
                .data
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match st.editor.as_mut() {
                Some(ed) if ed.id == id => skill_editor::hub_changed(ed, ctx).map(Msg::Editor),
                Some(_) => Task::none(),
                None if st.loading => Task::none(),
                None => load(st, ctx),
            }
        }
    }
}

/// The hub's events while the Skills screen is shown.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Close the editor and reload the list.
fn leave_editor(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.editor = None;
    load(st, ctx)
}

fn picks_mut(st: &mut State, target: Target) -> &mut Picks {
    match target {
        Target::New => &mut st.new_picks,
        Target::Preview => &mut st.preview_picks,
    }
}

fn open_editor(st: &mut State, id: String, ctx: &Ctx) -> Task<Msg> {
    let (ed, task) = skill_editor::open(id, ctx);
    st.editor = Some(ed);
    task.map(Msg::Editor)
}

/// Prefill the body from the template while the author has not typed in it.
fn refill(st: &mut State) {
    if !st.body_touched {
        st.new_body = text_editor::Content::with_text(&st.template);
    }
}

fn reset_form(st: &mut State) {
    st.new_title.clear();
    st.new_id.clear();
    st.new_picks = Picks::default();
    st.new_body = text_editor::Content::default();
    st.body_touched = false;
    st.form_error = None;
}

fn create(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let title = st.new_title.trim().to_string();
    if title.is_empty() {
        st.form_error = Some("Give the skill a title.".to_string());
        return Task::none();
    }
    let mut payload = json!({
        "title": title,
        "skill_md": st.new_body.text(),
        "bindings": st.new_picks.bindings(),
    });
    let id = st.new_id.trim().to_string();
    if !id.is_empty() {
        payload["id"] = json!(id);
    }
    st.creating = true;
    st.form_error = None;
    let c = ctx.client.clone();
    client::task(
        move || c.post::<OneResp>("/v1/skills", &payload),
        Msg::Created,
    )
}

/// One hub call with a raw body (a zip) or none; a non-2xx answer becomes `Err` with the hub's `error` text.
fn raw(
    c: &client::Client,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&c.target, method, path, body, c.timeout)
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    let error = serde_json::from_slice::<Value>(&reply.body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
    Err(error.unwrap_or_else(|| format!("HTTP {}", reply.status)))
}

fn parse<T: DeserializeOwned>(bytes: Vec<u8>) -> Result<T, String> {
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn zip_preview(c: &client::Client, bytes: &[u8]) -> Result<Preview, String> {
    parse(raw(c, "POST", "/v1/skills/import/preview", Some(bytes))?)
}

/// A folder on disk as a zip: every file under it (hidden folders skipped), under the folder's own name, then the
/// skills found in it (a folder with a SKILL.md) written as the archive the hub reads.
fn folder_zip(root: &Path) -> Result<Vec<u8>, String> {
    let base = root.parent().unwrap_or(root);
    let mut entries: Vec<Entry> = Vec::new();
    collect(base, root, &mut entries)?;
    let skills = find_skills(&entries, "");
    if skills.is_empty() {
        return Err("No SKILL.md was found in that folder.".to_string());
    }
    write_zip(&skills)
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<Entry>) -> Result<(), String> {
    let items = std::fs::read_dir(dir).map_err(|e| format!("The folder could not be read: {e}"))?;
    for item in items {
        let item = item.map_err(|e| e.to_string())?;
        let file_type = item.file_type().map_err(|e| e.to_string())?;
        let path = item.path();
        if file_type.is_dir() {
            if !item.file_name().to_string_lossy().starts_with('.') {
                collect(base, &path, out)?;
            }
        } else if file_type.is_file() {
            let rel = path.strip_prefix(base).map_err(|e| e.to_string())?;
            let parts: Vec<String> = rel
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            out.push(Entry {
                path: parts.join("/"),
                bytes: std::fs::read(&path)
                    .map_err(|e| format!("A file could not be read: {e}"))?,
                executable: is_executable(&path),
            });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    false
}

fn source_label(source: &Source) -> String {
    match source {
        Source::Built => "Built in".to_string(),
        Source::Upload => "Uploaded".to_string(),
        Source::Github { repo, commit } => format!(
            "GitHub {repo}@{}",
            commit.chars().take(7).collect::<String>()
        ),
    }
}

fn source_matches(filter: SourceFilter, source: &Source) -> bool {
    match filter {
        SourceFilter::All => true,
        SourceFilter::Built => matches!(source, Source::Built),
        SourceFilter::Upload => matches!(source, Source::Upload),
        SourceFilter::Github => matches!(source, Source::Github { .. }),
    }
}

fn binding_matches(bindings: &[String], choice: &str) -> bool {
    match choice {
        "" => true,
        NOT_BOUND => bindings.is_empty(),
        b => bindings.iter().any(|x| x == b),
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

fn binding_options(st: &State) -> Vec<String> {
    let bound: BTreeSet<&str> = st
        .skills
        .iter()
        .flat_map(|s| s.bindings.iter().map(String::as_str))
        .collect();
    let mut out = vec![ANY.to_string(), NOT_BOUND.to_string()];
    out.extend(bound.into_iter().map(str::to_string));
    out
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

fn matches_query(s: &Summary, needle: &str) -> bool {
    s.id.to_lowercase().contains(needle)
        || s.name.to_lowercase().contains(needle)
        || s.title.to_lowercase().contains(needle)
        || s.description.to_lowercase().contains(needle)
        || s.tags.iter().any(|t| t.to_lowercase().contains(needle))
}

/// The rows the filters and the search leave.
fn visible(st: &State) -> Vec<&Summary> {
    let needle = st.query.trim().to_lowercase();
    st.skills
        .iter()
        .filter(|s| st.show_archived || s.archived_ms.is_none())
        .filter(|s| source_matches(st.filter_source, &s.source))
        .filter(|s| binding_matches(&s.bindings, &st.binding))
        .filter(|s| needle.is_empty() || matches_query(s, &needle))
        .collect()
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

pub fn view<'a>(st: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    if let Some(ed) = st.editor.as_ref() {
        return skill_editor::view(ed, ctx).map(Msg::Editor);
    }
    let head = ui::view_head(
        "Skills",
        vec![
            ui::btn(
                if st.form == Form::New {
                    "Close"
                } else {
                    "New skill"
                },
                None,
                ui::Kind::Primary,
                Some(Msg::Form(Form::New)),
            ),
            ui::btn(
                "Import",
                None,
                ui::Kind::Secondary,
                Some(Msg::Form(Form::Import)),
            ),
            ui::btn(
                "From GitHub",
                None,
                ui::Kind::Secondary,
                Some(Msg::Form(Form::Github)),
            ),
            ui::btn(
                "Export selected",
                None,
                ui::Kind::Secondary,
                (!st.selected.is_empty()).then_some(Msg::ExportSelected),
            ),
        ],
    );
    let sources = [
        (SourceFilter::All, "All"),
        (SourceFilter::Built, "Built in"),
        (SourceFilter::Upload, "Uploaded"),
        (SourceFilter::Github, "GitHub"),
    ];
    let choice = if st.binding.is_empty() {
        ANY.to_string()
    } else {
        st.binding.clone()
    };
    let toolbar = row![
        ui::seg(&sources, st.filter_source, Msg::Filter),
        pick_list(binding_options(st), Some(choice), Msg::Binding),
        ui::seg(
            &[(false, "Hide archived"), (true, "Show archived")],
            st.show_archived,
            Msg::Archived
        ),
    ]
    .spacing(12);
    let search = text_input("Search id, name, title, description or tags", &st.query)
        .on_input(Msg::Query)
        .padding(8)
        .style(crate::theme::input);

    let mut page = column![head, toolbar, search].spacing(14);
    match st.form {
        Form::Closed => {}
        Form::New => page = page.push(new_form(st)),
        Form::Import => page = page.push(import_panel(st)),
        Form::Github => page = page.push(github_panel(st)),
    }
    if let Some((note, is_error)) = st.notice.as_ref() {
        let line: Element<'a, Msg> = if *is_error {
            ui::error_line(note)
        } else {
            text(note.as_str()).size(13.0).into()
        };
        page = page.push(line);
    }
    if let Some(p) = st.preview.as_ref() {
        page = page.push(preview_view(st, p));
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

    if !st.loading && st.error.is_none() {
        if st.skills.is_empty() {
            page = page.push(ui::empty(
                "wrench",
                "No skills yet",
                "The skill library holds the SKILL.md folders that the plaza's agents read. Create one, import a zip or a folder, or take one from GitHub.",
            ));
        } else if visible(st).is_empty() {
            page = page.push(
                column![
                    ui::empty(
                        "wrench",
                        "Nothing matches",
                        "Try another source or binding, or clear the search."
                    ),
                    ui::btn_sm(
                        "Clear filters",
                        None,
                        ui::Kind::Secondary,
                        Some(Msg::ClearFilters)
                    ),
                ]
                .spacing(12),
            );
        }
    }

    let mut list = column![].spacing(10);
    for s in visible(st) {
        list = list.push(row_card(s, st.selected.contains(&s.id)));
    }
    page = page.push(list);

    ui::scroll(page.padding(16)).width(Length::Fill).into()
}

fn row_card(s: &Summary, selected: bool) -> Element<'_, Msg> {
    let muted = crate::theme::palette().ink_3;
    let mut badges: Vec<Element<'_, Msg>> = Vec::new();
    if s.bindings.is_empty() {
        badges.push(ui::badge(NOT_BOUND, ui::Tone::Muted));
    } else {
        for b in &s.bindings {
            badges.push(ui::badge(&binding_label(b), ui::Tone::Info));
        }
    }
    match s.active_version {
        Some(v) => badges.push(ui::badge(&format!("v{v} live"), ui::Tone::Ok)),
        None => badges.push(ui::badge(
            &format!("Unpublished, latest v{}", s.latest_version),
            ui::Tone::Warn,
        )),
    }
    if let Some(score) = s.score {
        badges.push(ui::badge(&format!("score {score:.0}"), score_tone(score)));
    }
    if let Some(best) = s.best_version
        && Some(best) != s.active_version
    {
        badges.push(ui::badge(&format!("best: v{best}"), ui::Tone::Muted));
    }
    badges.push(ui::badge(&source_label(&s.source), ui::Tone::Muted));
    if s.archived_ms.is_some() {
        badges.push(ui::badge("Archived", ui::Tone::Muted));
    }

    let mut body = column![
        button(text(s.title.as_str()).size(16.0))
            .on_press(Msg::Open(s.id.clone()))
            .style(button::text),
        ui::sans(s.name.as_str(), 12.0, muted),
        row(badges).spacing(6),
    ]
    .spacing(8);
    if !s.description.is_empty() {
        body = body.push(text(ui::ellipsize(&s.description, 200)).size(14.0));
    }
    if !s.tags.is_empty() {
        body = body.push(text(s.tags.join(" · ")).size(12.0).color(muted));
    }
    body = body.push(
        text(format!("updated {}", ago(s.updated_ms)))
            .size(12.0)
            .color(muted),
    );
    let id = s.id.clone();
    let card = ui::card(&s.name, body.into());
    row![
        checkbox(selected).on_toggle(move |on| Msg::Select(id.clone(), on)),
        container(card).width(Length::Fill),
    ]
    .spacing(12)
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

/// The bindings picker: an Every agent box, then a box per role on the roles list.
fn picker<'a>(target: Target, roles: &'a [Role], picks: &'a Picks) -> Element<'a, Msg> {
    let muted = crate::theme::palette().ink_3;
    let mut boxes: Vec<Element<'a, Msg>> = vec![
        checkbox(picks.every)
            .label("Every agent")
            .on_toggle(move |on| Msg::Every(target, on))
            .into(),
    ];
    for r in roles {
        let id = r.id.clone();
        boxes.push(
            checkbox(picks.roles.contains(&r.id))
                .label(format!("Role: {}", r.title))
                .on_toggle(move |on| Msg::Role(target, id.clone(), on))
                .into(),
        );
    }
    column![
        text("Bindings").size(12.0).color(muted),
        column(boxes).spacing(8)
    ]
    .spacing(4)
    .into()
}

fn new_form(st: &State) -> Element<'_, Msg> {
    let mut col = column![
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
        picker(Target::New, &st.roles, &st.new_picks),
        field(
            "SKILL.md",
            text_editor(&st.new_body)
                .on_action(Msg::NewBody)
                .padding(8)
                .height(Length::Fixed(260.0))
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
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Form(Form::New))),
        ]
        .spacing(8),
    );
    ui::card("New skill", col.into())
}

fn import_panel(st: &State) -> Element<'_, Msg> {
    let body = column![
        text("Choose a .zip or .skill file, or a folder on this computer. Every folder with a SKILL.md is a skill.").size(13.0),
        row![
            ui::btn("Choose a file", None, ui::Kind::Primary, (!st.busy).then_some(Msg::ChooseFile)),
            ui::btn("Choose a folder", None, ui::Kind::Secondary, (!st.busy).then_some(Msg::ChooseFolder)),
        ]
        .spacing(8),
    ]
    .spacing(12);
    ui::card("Import", body.into())
}

fn github_panel(st: &State) -> Element<'_, Msg> {
    let body = column![
        field(
            "GitHub repository",
            text_input(
                "https://github.com/owner/repo, or a folder link inside it",
                &st.github_url
            )
            .on_input(Msg::GithubUrl)
            .on_submit(Msg::GithubPreview)
            .padding(8)
            .style(crate::theme::input)
            .into(),
        ),
        ui::btn(
            "Preview",
            None,
            ui::Kind::Primary,
            (!st.busy && !st.github_url.trim().is_empty()).then_some(Msg::GithubPreview),
        ),
    ]
    .spacing(12);
    ui::card("From GitHub", body.into())
}

fn preview_view<'a>(st: &'a State, p: &'a Preview) -> Element<'a, Msg> {
    let muted = crate::theme::palette().ink_3;
    let mut list = column![].spacing(10);
    for s in &p.skills {
        let mut badges: Vec<Element<'a, Msg>> =
            vec![ui::badge(&format!("{} files", s.files), ui::Tone::Muted)];
        if s.report.errors > 0 {
            badges.push(ui::badge(
                &format!("{} errors", s.report.errors),
                ui::Tone::Danger,
            ));
        }
        if s.report.warnings > 0 {
            badges.push(ui::badge(
                &format!("{} warnings", s.report.warnings),
                ui::Tone::Warn,
            ));
        }
        badges.push(ui::badge(
            &format!("score {:.0}", s.report.score),
            score_tone(s.report.score),
        ));
        if let Some(id) = s.exists.as_deref() {
            badges.push(ui::badge(&format!("Exists as {id}"), ui::Tone::Warn));
        }
        let path = s.path.clone();
        let body = column![
            text(s.name.clone().unwrap_or_else(|| s.title.clone())).size(15.0),
            ui::mono(s.path.as_str(), 12.0, muted),
            row(badges).spacing(6),
        ]
        .spacing(6);
        list = list.push(
            row![
                checkbox(st.picks.contains(&s.path))
                    .on_toggle(move |on| Msg::PreviewPick(path.clone(), on)),
                body,
            ]
            .spacing(12),
        );
    }
    let n = p.skills.len();
    let noun = if n == 1 { "skill" } else { "skills" };
    let count = st.picks.len();
    let label = if st.committing {
        "Importing...".to_string()
    } else {
        format!("Import {count}")
    };
    let bindings = picker(Target::Preview, &st.roles, &st.preview_picks);
    let actions = row![
        ui::btn(
            &label,
            None,
            ui::Kind::Primary,
            (count > 0 && !st.committing).then_some(Msg::Commit)
        ),
        ui::btn("Close", None, ui::Kind::Ghost, Some(Msg::ClosePreview)),
    ]
    .spacing(8);
    let body = column![
        text(format!(
            "{n} {noun} found, from {}",
            source_label(&p.source)
        ))
        .size(14.0),
        list,
        bindings,
        actions,
    ]
    .spacing(12);
    ui::card("Preview", body.into())
}
