//! New project (the web's `ui/new-project.ts`): talk it through with the orchestrator (a conversation, opened in the
//! IDE, where it drafts the project beside the chat and the owner starts it), open one directly with a title and
//! a goal, or bring in a project that exists (finished, or nearly: its history and environments come with it). The
//! shell lays it over the screen; the top bar's switcher, the Projects screen and the IDE open it.

use std::fmt;

use iced::widget::{checkbox, column, container, pick_list, row, text_input};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::screens::project_environments::{EnvKind, NAME_RULE, is_web_url, valid_name};
use super::{Ctx, client, ui};
use crate::theme;

/// The dialog's fields, and the call in flight.
#[derive(Debug, Clone)]
pub struct Form {
    /// What the owner wants done: the conversation's first message.
    pub ask: String,
    pub title: String,
    pub goal: String,
    /// A code project gets a repository; the other kind does not.
    pub code: bool,
    /// The bring-in section's fields.
    pub adopt: AdoptForm,
    pub busy: bool,
    pub error: Option<String>,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            ask: String::new(),
            title: String::new(),
            goal: String::new(),
            code: true,
            adopt: AdoptForm::default(),
            busy: false,
            error: None,
        }
    }
}

impl Form {
    /// The dialog with `ask` typed in (a capture hook, `APZ_NEW_PROJECT`).
    pub fn with_goal(ask: String) -> Form {
        Form {
            ask,
            ..Form::default()
        }
    }
}

/// Where the bring-in section's code is: a repository on a platform the owner authorized, a git address, a folder
/// on the hub, or nothing yet (the files come later, by upload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    Platform,
    Git,
    Path,
    #[default]
    Nothing,
}

/// The mode the project is brought in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdoptMode {
    #[default]
    Maintenance,
    Build,
}

impl AdoptMode {
    fn key(self) -> &'static str {
        match self {
            AdoptMode::Maintenance => "maintenance",
            AdoptMode::Build => "build",
        }
    }
}

/// An authorization the owner can take a repository from, as the pick list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPick {
    /// The integration's id, sent as the source's `platform`.
    pub id: String,
    pub label: String,
}

impl fmt::Display for PlatformPick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// One environment row of the bring-in section: a name, a kind, and an optional address.
#[derive(Debug, Clone)]
pub struct AdoptEnv {
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
}

/// The "Or bring a project that exists" section's fields.
#[derive(Debug, Clone)]
pub struct AdoptForm {
    pub title: String,
    pub goal: String,
    pub source: SourceKind,
    pub git: String,
    pub path: String,
    pub branch: String,
    /// The platform authorizations, once loaded; None until the source switches to a platform.
    pub platforms: Option<Vec<PlatformPick>>,
    /// The authorizations' list is on its way.
    pub platforms_loading: bool,
    pub platform: Option<PlatformPick>,
    /// The repository, as `owner/name`.
    pub repo: String,
    pub mode: AdoptMode,
    /// True (the default): it opens delivered; false: still in implementation.
    pub finished: bool,
    /// Up to three environments.
    pub envs: Vec<AdoptEnv>,
}

impl Default for AdoptForm {
    fn default() -> Self {
        AdoptForm {
            title: String::new(),
            goal: String::new(),
            source: SourceKind::default(),
            git: String::new(),
            path: String::new(),
            branch: "main".into(),
            platforms: None,
            platforms_loading: false,
            platform: None,
            repo: String::new(),
            mode: AdoptMode::default(),
            finished: true,
            envs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Ask(String),
    Title(String),
    Goal(String),
    /// The kind: code (true) or other.
    Code(bool),
    /// Start a conversation with the orchestrator, the goal its first message.
    Talk,
    /// Open the project directly.
    Open,
    Cancel,
    Talked(Result<Value, String>),
    Opened(Result<Value, String>),
    AdoptTitle(String),
    AdoptGoal(String),
    AdoptSource(SourceKind),
    AdoptGit(String),
    AdoptPath(String),
    AdoptBranch(String),
    AdoptPlatform(PlatformPick),
    AdoptRepo(String),
    AdoptMode(AdoptMode),
    AdoptFinished(bool),
    AdoptEnvAdd,
    AdoptEnvRemove(usize),
    AdoptEnvName(usize, String),
    AdoptEnvKind(usize, EnvKind),
    AdoptEnvUrl(usize, String),
    /// Brings the project in.
    Adopt,
    Adopted(Result<Value, String>),
    /// The platform authorizations, for the pick list.
    Platforms(Result<Value, String>),
}

/// What the shell does after a message: nothing more, close the dialog, open the new conversation in the IDE, or
/// scope the console to the new project and open its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Close,
    Conversation(String),
    Project(String),
}

pub fn update(form: &mut Form, msg: Msg, ctx: &Ctx) -> (Task<Msg>, Effect) {
    match msg {
        Msg::Ask(s) => form.ask = s,
        Msg::Title(s) => form.title = s,
        Msg::Goal(s) => form.goal = s,
        Msg::Code(code) => form.code = code,
        Msg::Cancel => return (Task::none(), Effect::Close),
        Msg::Talk => {
            let text = form.ask.trim().to_string();
            if text.is_empty() {
                form.error = Some("Say what you want done first.".into());
                return (Task::none(), Effect::None);
            }
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            let body = json!({ "text": text });
            return (
                client::task(
                    move || c.post::<Value>("/v1/owner/conversations", &body),
                    Msg::Talked,
                ),
                Effect::None,
            );
        }
        Msg::Open => {
            let (title, goal) = (form.title.trim().to_string(), form.goal.trim().to_string());
            if title.is_empty() || goal.is_empty() {
                form.error = Some("A project needs a title and a goal.".into());
                return (Task::none(), Effect::None);
            }
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            let body = json!({ "title": title, "goal": goal, "kind": if form.code { "code" } else { "other" } });
            return (
                client::task(move || c.post::<Value>("/v1/projects", &body), Msg::Opened),
                Effect::None,
            );
        }
        Msg::Talked(Err(e)) | Msg::Opened(Err(e)) | Msg::Adopted(Err(e)) => {
            form.busy = false;
            form.error = Some(e);
        }
        Msg::Talked(Ok(conversation)) => {
            form.busy = false;
            return match conversation["id"].as_str() {
                Some(id) => (Task::none(), Effect::Conversation(id.to_string())),
                None => {
                    form.error = Some("The hub answered without a conversation.".into());
                    (Task::none(), Effect::None)
                }
            };
        }
        Msg::Opened(Ok(project)) => {
            form.busy = false;
            return match project["id"].as_str() {
                Some(id) => (Task::none(), Effect::Project(id.to_string())),
                None => {
                    form.error = Some("The hub answered without a project.".into());
                    (Task::none(), Effect::None)
                }
            };
        }
        Msg::AdoptTitle(s) => form.adopt.title = s,
        Msg::AdoptGoal(s) => form.adopt.goal = s,
        Msg::AdoptSource(source) => {
            form.adopt.source = source;
            // The authorizations load the first time the source is a platform.
            if source == SourceKind::Platform
                && form.adopt.platforms.is_none()
                && !form.adopt.platforms_loading
            {
                form.adopt.platforms_loading = true;
                let c = ctx.client.clone();
                return (
                    client::task(move || c.get::<Value>("/v1/platforms"), Msg::Platforms),
                    Effect::None,
                );
            }
        }
        Msg::AdoptGit(s) => form.adopt.git = s,
        Msg::AdoptPath(s) => form.adopt.path = s,
        Msg::AdoptBranch(s) => form.adopt.branch = s,
        Msg::AdoptPlatform(pick) => form.adopt.platform = Some(pick),
        Msg::AdoptRepo(s) => form.adopt.repo = s,
        Msg::AdoptMode(mode) => form.adopt.mode = mode,
        Msg::AdoptFinished(on) => form.adopt.finished = on,
        Msg::AdoptEnvAdd => {
            if form.adopt.envs.len() < 3 {
                form.adopt.envs.push(AdoptEnv {
                    name: String::new(),
                    kind: EnvKind::Staging,
                    url: String::new(),
                });
            }
        }
        Msg::AdoptEnvRemove(i) => {
            if i < form.adopt.envs.len() {
                form.adopt.envs.remove(i);
            }
        }
        Msg::AdoptEnvName(i, s) => {
            if let Some(env) = form.adopt.envs.get_mut(i) {
                env.name = s;
            }
        }
        Msg::AdoptEnvKind(i, kind) => {
            if let Some(env) = form.adopt.envs.get_mut(i) {
                env.kind = kind;
            }
        }
        Msg::AdoptEnvUrl(i, s) => {
            if let Some(env) = form.adopt.envs.get_mut(i) {
                env.url = s;
            }
        }
        Msg::Adopt => {
            let body = match adopt_body(&form.adopt) {
                Ok(body) => body,
                Err(e) => {
                    form.error = Some(e);
                    return (Task::none(), Effect::None);
                }
            };
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            return (
                client::task(
                    move || c.post::<Value>("/v1/projects/adopt", &body),
                    Msg::Adopted,
                ),
                Effect::None,
            );
        }
        Msg::Adopted(Ok(answer)) => {
            form.busy = false;
            return match answer["project"]["id"].as_str() {
                Some(id) => {
                    // The project opens even when the import had problems; they are shown where the dialog was.
                    let problems: Vec<&str> = answer["problems"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    if !problems.is_empty() {
                        form.error = Some(problems.join("; "));
                    }
                    (Task::none(), Effect::Project(id.to_string()))
                }
                None => {
                    form.error = Some("The hub answered without a project.".into());
                    (Task::none(), Effect::None)
                }
            };
        }
        Msg::Platforms(Ok(list)) => {
            form.adopt.platforms_loading = false;
            form.adopt.platforms = Some(platform_picks(&list));
        }
        Msg::Platforms(Err(e)) => {
            form.adopt.platforms_loading = false;
            form.error = Some(e);
        }
    }
    (Task::none(), Effect::None)
}

/// The authorizations of `GET /v1/platforms`, as pick-list rows: "github · acme · ledger".
fn platform_picks(list: &Value) -> Vec<PlatformPick> {
    list["integrations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|integration| {
            let id = integration["id"].as_str()?.to_string();
            let kind = integration["kind"].as_str().unwrap_or("");
            let login = integration["account"]["login"].as_str().unwrap_or("");
            let mut label = format!("{kind} · {login}");
            if let Some(target) = integration["target"].as_str().filter(|t| !t.is_empty()) {
                label.push_str(" · ");
                label.push_str(target);
            }
            Some(PlatformPick { id, label })
        })
        .collect()
}

/// `owner/name`: two non-empty names around one slash, with no spaces.
fn valid_repo(repo: &str) -> bool {
    match repo.split_once('/') {
        Some((owner, name)) => {
            !owner.is_empty()
                && !name.is_empty()
                && !name.contains('/')
                && !repo.contains(char::is_whitespace)
        }
        None => false,
    }
}

/// The adopt request's body, or why it is refused. Checked here, before anything is sent to the hub.
fn adopt_body(adopt: &AdoptForm) -> Result<Value, String> {
    let title = adopt.title.trim();
    if title.is_empty() {
        return Err("Bringing a project in needs a title.".into());
    }
    let branch = match adopt.branch.trim() {
        "" => "main",
        branch => branch,
    };
    let mut body = json!({
        "title": title,
        "kind": "code",
        "mode": adopt.mode.key(),
        "finished": adopt.finished,
    });
    let goal = adopt.goal.trim();
    if !goal.is_empty() {
        body["goal"] = json!(goal);
    }
    match adopt.source {
        SourceKind::Platform => {
            let Some(platform) = &adopt.platform else {
                return Err("Pick the platform authorization the repository is on.".into());
            };
            let repo = adopt.repo.trim();
            if !valid_repo(repo) {
                return Err("The repository is owner/name, as the platform shows it.".into());
            }
            body["source"] = json!({ "platform": platform.id, "repo": repo });
        }
        SourceKind::Git => {
            let git = adopt.git.trim();
            if !is_web_url(git) {
                return Err("A git address starts with http:// or https://.".into());
            }
            body["source"] = json!({ "git": git, "branch": branch });
        }
        SourceKind::Path => {
            let path = adopt.path.trim();
            if !path.starts_with('/') {
                return Err("A folder on the hub is an absolute path, starting with /.".into());
            }
            body["source"] = json!({ "path": path, "branch": branch });
        }
        SourceKind::Nothing => {}
    }
    let mut environments: Vec<Value> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for slot in &adopt.envs {
        let name = slot.name.trim();
        let url = slot.url.trim();
        // A row left blank is skipped; one with anything in it must be whole.
        if name.is_empty() && url.is_empty() {
            continue;
        }
        if !valid_name(name) {
            return Err(NAME_RULE.into());
        }
        if names.contains(&name) {
            return Err("Two environments have the same name.".into());
        }
        if !url.is_empty() && !is_web_url(url) {
            return Err("An environment's URL starts with http:// or https://.".into());
        }
        names.push(name);
        let mut env = json!({ "name": name, "kind": slot.kind });
        if !url.is_empty() {
            env["url"] = json!(url);
        }
        environments.push(env);
    }
    if !environments.is_empty() {
        body["environments"] = Value::Array(environments);
    }
    Ok(body)
}

/// The dialog, over a dimmed window.
pub fn view(form: &Form) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = |msg: Msg| (!form.busy).then_some(msg);
    let talk = column![
        ui::sans("Talk it through", 14.0, p.ink),
        ui::faint("The orchestrator asks what it needs and drafts the project beside the conversation: its tasks, its team and its budget. You start it when it looks right."),
        ui::label("Your goal"),
        text_input(
            "What do you want done? “The scheduler tests are flaky and I want 0.9 out this week.”",
            &form.ask
        )
        .on_input(Msg::Ask)
        .on_submit(Msg::Talk)
        .padding(8)
        .style(theme::input),
        row![ui::btn(
            "Start a conversation",
            Some("conversations"),
            ui::Kind::Primary,
            enabled(Msg::Talk)
        )],
    ]
    .spacing(10);
    let kind = |code: bool, label: &str| {
        ui::btn(
            label,
            None,
            if form.code == code {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            },
            Some(Msg::Code(code)),
        )
    };
    let direct = column![
        ui::sans("Or open it directly", 14.0, p.ink),
        ui::faint("An empty project of yours: you put agents on it and give it tasks."),
        ui::label("Title"),
        text_input("Release 0.9", &form.title)
            .on_input(Msg::Title)
            .padding(8)
            .style(theme::input),
        ui::label("Goal"),
        text_input("What the project delivers", &form.goal)
            .on_input(Msg::Goal)
            .on_submit(Msg::Open)
            .padding(8)
            .style(theme::input),
        ui::label("Kind"),
        row![
            kind(true, "Code (it gets a repository)"),
            kind(false, "Other")
        ]
        .spacing(6),
        row![ui::btn(
            "Open project",
            None,
            ui::Kind::Secondary,
            enabled(Msg::Open)
        )],
    ]
    .spacing(10);
    let adopt = adopt_section(&form.adopt, form.busy);
    let mut body = column![
        ui::sans("New project", 16.0, p.ink),
        talk,
        ui::hairline(),
        direct,
        ui::hairline(),
        adopt,
    ]
    .spacing(16);
    if let Some(e) = &form.error {
        body = body.push(ui::error_line(e));
    }
    body = body.push(row![ui::btn(
        "Cancel",
        None,
        ui::Kind::Ghost,
        Some(Msg::Cancel)
    )]);
    // Scrolls within the panel: the three sections together are taller than a window.
    let panel = container(ui::scroll(body))
        .padding(20)
        .width(Length::Fixed(560.0))
        .max_height(680.0)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.5,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..Default::default()
        })
        .into()
}

/// "Or bring a project that exists": a finished (or nearly finished) product, brought in with its history.
fn adopt_section(adopt: &AdoptForm, busy: bool) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = move |msg: Msg| (!busy).then_some(msg);
    let source = |kind: SourceKind, label: &str| {
        ui::btn_sm(
            label,
            None,
            if adopt.source == kind {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            },
            Some(Msg::AdoptSource(kind)),
        )
    };
    let mode = |m: AdoptMode, label: &str| {
        ui::btn_sm(
            label,
            None,
            if adopt.mode == m {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            },
            Some(Msg::AdoptMode(m)),
        )
    };
    let mut body: Vec<Element<'_, Msg>> = vec![
        ui::sans("Or bring a project that exists", 14.0, p.ink).into(),
        ui::faint("A product that is finished, or nearly: the plaza imports its history and looks after it.").into(),
        ui::label("Title").into(),
        text_input("The product's name", &adopt.title)
            .on_input(Msg::AdoptTitle)
            .padding(8)
            .style(theme::input)
            .into(),
        ui::label("Goal").into(),
        text_input("What it is for", &adopt.goal)
            .on_input(Msg::AdoptGoal)
            .padding(8)
            .style(theme::input)
            .into(),
        ui::label("Where its code is").into(),
        row![
            source(SourceKind::Platform, "Platform"),
            source(SourceKind::Git, "Git address"),
            source(SourceKind::Path, "Folder on the hub"),
            source(SourceKind::Nothing, "Later"),
        ]
        .spacing(6)
        .into(),
    ];
    match adopt.source {
        SourceKind::Platform => body.extend(platform_fields(adopt)),
        SourceKind::Git => {
            body.push(ui::label("Git address").into());
            body.push(
                text_input("https://github.com/owner/product.git", &adopt.git)
                    .on_input(Msg::AdoptGit)
                    .padding(8)
                    .style(theme::input)
                    .into(),
            );
            body.push(branch_field(adopt));
        }
        SourceKind::Path => {
            body.push(ui::label("Folder on the hub").into());
            body.push(
                text_input("/srv/repos/product", &adopt.path)
                    .on_input(Msg::AdoptPath)
                    .padding(8)
                    .style(theme::input)
                    .into(),
            );
            body.push(branch_field(adopt));
        }
        SourceKind::Nothing => body.push(
            ui::faint("The files come later: upload them into the project once it is open.").into(),
        ),
    }
    body.push(ui::label("Mode").into());
    body.push(
        row![
            mode(AdoptMode::Maintenance, "Maintenance"),
            mode(AdoptMode::Build, "Build"),
        ]
        .spacing(6)
        .into(),
    );
    body.push(
        ui::faint("Maintenance: the plaza watches it and proposes changes. Build: it is still being built.")
            .into(),
    );
    body.push(
        checkbox(adopt.finished)
            .label("Finished: it opens delivered")
            .on_toggle(Msg::AdoptFinished)
            .size(14.0)
            .text_size(13.0)
            .into(),
    );
    body.push(ui::label("Environments").into());
    body.push(
        ui::faint("Where it runs, up to three. A name is lower-case letters, digits and dashes.")
            .into(),
    );
    for (i, slot) in adopt.envs.iter().enumerate() {
        body.push(
            row![
                text_input("production", &slot.name)
                    .on_input(move |s| Msg::AdoptEnvName(i, s))
                    .padding(8)
                    .style(theme::input)
                    .width(Length::Fixed(130.0)),
                pick_list(EnvKind::ALL.to_vec(), Some(slot.kind), move |k| {
                    Msg::AdoptEnvKind(i, k)
                })
                .style(theme::pick_list)
                .text_size(13.0),
                text_input("https://example.com", &slot.url)
                    .on_input(move |s| Msg::AdoptEnvUrl(i, s))
                    .padding(8)
                    .style(theme::input)
                    .width(Length::Fill),
                ui::btn_sm(
                    "Remove",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::AdoptEnvRemove(i))
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if adopt.envs.len() < 3 {
        body.push(
            row![ui::btn_sm(
                "Add an environment",
                Some("plus"),
                ui::Kind::Secondary,
                Some(Msg::AdoptEnvAdd)
            )]
            .into(),
        );
    }
    body.push(
        row![ui::btn(
            "Bring it in",
            None,
            ui::Kind::Secondary,
            enabled(Msg::Adopt)
        )]
        .into(),
    );
    column(body).spacing(10).into()
}

/// The branch to bring in (git and folders); "main" when left empty.
fn branch_field(adopt: &AdoptForm) -> Element<'_, Msg> {
    column![
        ui::label("Branch"),
        text_input("main", &adopt.branch)
            .on_input(Msg::AdoptBranch)
            .padding(8)
            .style(theme::input),
    ]
    .spacing(10)
    .into()
}

/// The platform source's fields: the authorization to pick from, and the repository typed as `owner/name`.
fn platform_fields(adopt: &AdoptForm) -> Vec<Element<'_, Msg>> {
    let mut out: Vec<Element<'_, Msg>> = Vec::new();
    match &adopt.platforms {
        None => out.push(
            ui::faint(if adopt.platforms_loading {
                "Loading the platform authorizations…"
            } else {
                "The platform authorizations did not load. Pick the source again to retry."
            })
            .into(),
        ),
        Some(list) if list.is_empty() => out.push(
            ui::faint("No platform is authorized yet. Connect one first, or bring the project from a git address or a folder.")
                .into(),
        ),
        Some(list) => {
            out.push(ui::label("Authorization").into());
            out.push(
                pick_list(list.clone(), adopt.platform.clone(), Msg::AdoptPlatform)
                    .style(theme::pick_list)
                    .text_size(13.0)
                    .into(),
            );
            out.push(ui::label("Repository").into());
            out.push(
                text_input("owner/name", &adopt.repo)
                    .on_input(Msg::AdoptRepo)
                    .padding(8)
                    .style(theme::input)
                    .into(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            client: client::Client::new("http://127.0.0.1:9", None, None),
            project: None,
        }
    }

    #[test]
    fn the_dialog_asks_for_a_goal_or_a_title_and_goal_before_it_calls_the_hub() {
        let mut form = Form::default();
        let (_, effect) = update(&mut form, Msg::Talk, &ctx());
        assert_eq!(effect, Effect::None);
        assert_eq!(form.error.as_deref(), Some("Say what you want done first."));
        assert!(!form.busy);
        let _ = update(&mut form, Msg::Title("Release 0.9".into()), &ctx());
        let _ = update(&mut form, Msg::Open, &ctx());
        assert_eq!(
            form.error.as_deref(),
            Some("A project needs a title and a goal.")
        );
        assert!(!form.busy);
    }

    #[test]
    fn an_answer_opens_the_conversation_or_the_project() {
        let mut form = Form::default();
        let (_, effect) = update(
            &mut form,
            Msg::Talked(Ok(json!({ "id": "conv-4" }))),
            &ctx(),
        );
        assert_eq!(effect, Effect::Conversation("conv-4".into()));
        let (_, effect) = update(&mut form, Msg::Opened(Ok(json!({ "id": "pr7" }))), &ctx());
        assert_eq!(effect, Effect::Project("pr7".into()));
        let (_, effect) = update(&mut form, Msg::Opened(Err("no".into())), &ctx());
        assert_eq!(effect, Effect::None);
        assert_eq!(form.error.as_deref(), Some("no"));
        assert_eq!(update(&mut form, Msg::Cancel, &ctx()).1, Effect::Close);
    }

    /// Runs `Adopt` and returns the refusal shown; asserts nothing was sent.
    fn refused(form: &mut Form) -> String {
        let (_, effect) = update(form, Msg::Adopt, &ctx());
        assert_eq!(effect, Effect::None);
        assert!(!form.busy, "a refused form sends nothing");
        form.error.take().expect("the refusal is shown")
    }

    #[test]
    fn bringing_a_project_in_is_refused_before_any_call() {
        let mut form = Form::default();
        assert_eq!(refused(&mut form), "Bringing a project in needs a title.");

        let _ = update(&mut form, Msg::AdoptTitle("Ledger".into()), &ctx());
        let _ = update(&mut form, Msg::AdoptSource(SourceKind::Git), &ctx());
        let _ = update(
            &mut form,
            Msg::AdoptGit("ftp://example.com/ledger.git".into()),
            &ctx(),
        );
        assert_eq!(
            refused(&mut form),
            "A git address starts with http:// or https://."
        );

        let _ = update(&mut form, Msg::AdoptSource(SourceKind::Path), &ctx());
        let _ = update(&mut form, Msg::AdoptPath("ledger".into()), &ctx());
        assert_eq!(
            refused(&mut form),
            "A folder on the hub is an absolute path, starting with /."
        );

        let _ = update(&mut form, Msg::AdoptSource(SourceKind::Platform), &ctx());
        assert_eq!(
            refused(&mut form),
            "Pick the platform authorization the repository is on."
        );
        let _ = update(
            &mut form,
            Msg::AdoptPlatform(PlatformPick {
                id: "ga1".into(),
                label: "github · acme".into(),
            }),
            &ctx(),
        );
        let _ = update(&mut form, Msg::AdoptRepo("ledger".into()), &ctx());
        assert_eq!(
            refused(&mut form),
            "The repository is owner/name, as the platform shows it."
        );

        let _ = update(&mut form, Msg::AdoptRepo("acme/ledger".into()), &ctx());
        let _ = update(&mut form, Msg::AdoptEnvAdd, &ctx());
        let _ = update(&mut form, Msg::AdoptEnvName(0, "Prod".into()), &ctx());
        assert_eq!(refused(&mut form), NAME_RULE);

        let _ = update(&mut form, Msg::AdoptEnvName(0, "prod".into()), &ctx());
        let _ = update(
            &mut form,
            Msg::AdoptEnvUrl(0, "ledger.example.com".into()),
            &ctx(),
        );
        assert_eq!(
            refused(&mut form),
            "An environment's URL starts with http:// or https://."
        );

        let _ = update(&mut form, Msg::AdoptEnvUrl(0, String::new()), &ctx());
        let _ = update(&mut form, Msg::AdoptEnvAdd, &ctx());
        let _ = update(&mut form, Msg::AdoptEnvName(1, "prod".into()), &ctx());
        assert_eq!(refused(&mut form), "Two environments have the same name.");
    }

    #[test]
    fn a_git_project_is_sent_with_its_branch_mode_and_environments() {
        let mut adopt = AdoptForm {
            title: "Ledger".into(),
            source: SourceKind::Git,
            git: "https://example.com/ledger.git".into(),
            ..AdoptForm::default()
        };
        adopt.envs.push(AdoptEnv {
            name: "prod".into(),
            kind: EnvKind::Production,
            url: "https://ledger.example.com".into(),
        });
        let body = adopt_body(&adopt).expect("a valid form");
        assert_eq!(body["title"], "Ledger");
        assert_eq!(body["kind"], "code");
        assert_eq!(body["mode"], "maintenance");
        assert_eq!(body["finished"], true);
        assert_eq!(
            body["source"],
            json!({ "git": "https://example.com/ledger.git", "branch": "main" })
        );
        assert_eq!(
            body["environments"],
            json!([{ "name": "prod", "kind": "production", "url": "https://ledger.example.com" }])
        );
        assert!(body.get("goal").is_none(), "an empty goal is left out");
    }

    #[test]
    fn an_adopted_project_opens_and_its_problems_are_shown() {
        let mut form = Form::default();
        let (_, effect) = update(
            &mut form,
            Msg::Adopted(Ok(json!({ "project": { "id": "pr9" } }))),
            &ctx(),
        );
        assert_eq!(effect, Effect::Project("pr9".into()));
        assert!(form.error.is_none());

        let (_, effect) = update(
            &mut form,
            Msg::Adopted(Ok(json!({
                "project": { "id": "pr9" },
                "problems": ["importing x: no", "environment prod: bad"],
            }))),
            &ctx(),
        );
        assert_eq!(effect, Effect::Project("pr9".into()));
        assert_eq!(
            form.error.as_deref(),
            Some("importing x: no; environment prod: bad")
        );
    }

    #[test]
    fn the_platform_authorizations_are_listed_with_their_kind_account_and_target() {
        let list = json!({
            "integrations": [
                { "id": "ga1", "kind": "github", "account": { "login": "acme" }, "target": "ledger" },
                { "id": "gl2", "kind": "gitlab", "account": { "login": "irfan" }, "target": null },
            ],
        });
        let picks = platform_picks(&list);
        assert_eq!(
            picks,
            vec![
                PlatformPick {
                    id: "ga1".into(),
                    label: "github · acme · ledger".into()
                },
                PlatformPick {
                    id: "gl2".into(),
                    label: "gitlab · irfan".into()
                },
            ]
        );
    }
}
