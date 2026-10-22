//! New project (the web's `ui/new-project.ts`): talk it through with the orchestrator in its main chat (the owner's
//! words are said there and the chat opens, where the orchestrator drafts the project and the owner starts it from its
//! card), open one directly with a title and a goal, or bring in a project that exists (finished, or nearly: its history
//! and environments come with it). The shell lays it over the screen; the top bar's switcher, the Projects screen and
//! the IDE open it.

use std::fmt;

use iced::widget::text_editor::Content;
use iced::widget::{Space, checkbox, column, container, pick_list, row, text_editor, text_input};
use iced::{Alignment, Background, Border, Element, Length, Padding, Task, Theme};
use serde_json::{Value, json};

use super::screens::project_environments::{EnvKind, NAME_RULE, is_web_url, valid_name};
use super::work_api::{self, ChatMessage};
use super::{Ctx, client, ui};
use crate::theme::{self, SANS_MEDIUM, SANS_SEMIBOLD};

/// The dialog's fields, and the call in flight.
#[derive(Debug)]
pub struct Form {
    /// What the owner wants done: the first words said to the orchestrator's main chat.
    pub ask: Content,
    pub title: String,
    pub goal: Content,
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
            ask: Content::new(),
            title: String::new(),
            goal: Content::new(),
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
            ask: Content::with_text(&ask),
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

impl SourceKind {
    const ALL: [SourceKind; 4] = [
        SourceKind::Platform,
        SourceKind::Git,
        SourceKind::Path,
        SourceKind::Nothing,
    ];
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SourceKind::Platform => "A repository on GitHub, GitLab or Forgejo",
            SourceKind::Git => "A git address anyone can read",
            SourceKind::Path => "A folder on the hub's machine",
            SourceKind::Nothing => "Files from this computer",
        })
    }
}

/// The mode the project is brought in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdoptMode {
    #[default]
    Maintenance,
    Build,
}

impl AdoptMode {
    const ALL: [AdoptMode; 2] = [AdoptMode::Maintenance, AdoptMode::Build];

    fn key(self) -> &'static str {
        match self {
            AdoptMode::Maintenance => "maintenance",
            AdoptMode::Build => "build",
        }
    }
}

impl fmt::Display for AdoptMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AdoptMode::Maintenance => "Maintenance",
            AdoptMode::Build => "Build",
        })
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
#[derive(Debug)]
pub struct AdoptForm {
    pub title: String,
    pub goal: Content,
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
    /// The platform's repository is signed with the hub's key on the owner's account (the web's "Sign exports").
    pub sign: bool,
}

impl Default for AdoptForm {
    fn default() -> Self {
        AdoptForm {
            title: String::new(),
            goal: Content::new(),
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
            sign: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Ask(text_editor::Action),
    Title(String),
    Goal(text_editor::Action),
    /// The kind: code (true) or other.
    Code(bool),
    /// Says the goal to the orchestrator's main chat.
    Talk,
    /// Open the project directly.
    Open,
    Cancel,
    Talked(Result<ChatMessage, String>),
    Opened(Result<Value, String>),
    AdoptTitle(String),
    AdoptGoal(text_editor::Action),
    AdoptSign(bool),
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

/// What the shell does after a message: nothing more, close the dialog, open a chat (the main chat after a talk), or
/// scope the console to the new project and open its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Close,
    /// Opens the chat with this id.
    Conversation(String),
    Project(String),
}

pub fn update(form: &mut Form, msg: Msg, ctx: &Ctx) -> (Task<Msg>, Effect) {
    match msg {
        Msg::Ask(action) => form.ask.perform(action),
        Msg::Title(s) => form.title = s,
        Msg::Goal(action) => form.goal.perform(action),
        Msg::Code(code) => form.code = code,
        Msg::Cancel => return (Task::none(), Effect::Close),
        Msg::Talk => {
            let text = form.ask.text().trim().to_string();
            if text.is_empty() {
                form.error = Some("Say what you want done first.".into());
                return (Task::none(), Effect::None);
            }
            form.busy = true;
            form.error = None;
            let c = ctx.client.clone();
            return (
                client::task(move || work_api::say(&c, "main", &text), Msg::Talked),
                Effect::None,
            );
        }
        Msg::Open => {
            let (title, goal) = (
                form.title.trim().to_string(),
                form.goal.text().trim().to_string(),
            );
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
        // The words are in the main chat: it opens there, where the orchestrator answers.
        Msg::Talked(Ok(_)) => {
            form.busy = false;
            return (Task::none(), Effect::Conversation("main".into()));
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
        Msg::AdoptGoal(action) => form.adopt.goal.perform(action),
        Msg::AdoptSign(on) => form.adopt.sign = on,
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
                    kind: EnvKind::Production,
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
    let goal = adopt.goal.text();
    if !goal.trim().is_empty() {
        body["goal"] = json!(goal.trim());
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
            let mut source = json!({ "platform": platform.id, "repo": repo });
            if adopt.sign {
                source["sign"] = json!(true);
            }
            body["source"] = source;
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

/// The sheet's kind choice: a pick list of Code (a repository) or Other, as the web's select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindChoice {
    Code,
    Other,
}

impl KindChoice {
    const ALL: [KindChoice; 2] = [KindChoice::Code, KindChoice::Other];
}

impl fmt::Display for KindChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KindChoice::Code => "Code (it gets a repository)",
            KindChoice::Other => "Other",
        })
    }
}

/// The text area's look: the field's surface, its rule, and the signal ring while focused (the web's `.textarea`).
pub fn editor(_: &Theme, status: text_editor::Status) -> text_editor::Style {
    let p = theme::palette();
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: Background::Color(p.sheet),
        border: Border::default()
            .width(1.0)
            .color(if focused {
                p.signal
            } else if matches!(status, text_editor::Status::Hovered) {
                p.ink_3
            } else {
                p.rule_strong
            })
            .rounded(theme::skin().radius),
        placeholder: p.ink_3,
        value: p.ink,
        selection: p.signal_soft,
    }
}

/// A labelled field, as the web's `.field`: the label above the control, a hint under it.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: Option<&str>) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut body = column![
        ui::sans(label.to_string(), crate::theme::TYPE_UI, p.ink_2).font(SANS_MEDIUM),
        control
    ]
    .spacing(5);
    if let Some(hint) = hint {
        body = body.push(ui::faint(hint.to_string()));
    }
    body.width(Length::Fill).into()
}

/// A single-line field on the sheet.
fn text_box<'a>(
    value: &str,
    placeholder: &str,
    on: impl Fn(String) -> Msg + 'a,
) -> Element<'a, Msg> {
    text_input(placeholder, value)
        .on_input(on)
        .size(crate::theme::TYPE_UI)
        .padding([6, 10])
        .width(Length::Fill)
        .style(theme::input)
        .into()
}

/// A multi-line field: the web's textarea, `height` tall, with the draft kept in the form.
fn area_box<'a>(
    content: &'a Content,
    placeholder: &str,
    height: f32,
    on: fn(text_editor::Action) -> Msg,
) -> Element<'a, Msg> {
    text_editor(content)
        .placeholder(placeholder.to_string())
        .height(Length::Fixed(height))
        .style(editor)
        .on_action(on)
        .into()
}

/// A pick list on the sheet, as the web's select.
fn select_box<'a, T>(
    options: Vec<T>,
    selected: Option<T>,
    on: impl Fn(T) -> Msg + 'a,
) -> Element<'a, Msg>
where
    T: Clone + PartialEq + fmt::Display + 'a,
{
    pick_list(options, selected, on)
        .style(theme::pick_list)
        .text_size(crate::theme::TYPE_UI)
        .width(Length::Fill)
        .into()
}

/// A section of the sheet: its title, what it does, its fields and its call; a rule under it unless it is the last.
fn section<'a>(title: &str, intro: &str, body: Element<'a, Msg>, last: bool) -> Element<'a, Msg> {
    let p = theme::palette();
    let inner = column![
        ui::sans(title.to_string(), 16.0, p.ink).font(SANS_SEMIBOLD),
        ui::faint(intro.to_string()),
        body,
    ]
    .spacing(12)
    .width(Length::Fill);
    if last {
        inner.into()
    } else {
        column![
            inner,
            Space::new().height(24.0),
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(theme::palette().rule.into()),
                    ..Default::default()
                }),
        ]
        .spacing(0)
        .width(Length::Fill)
        .into()
    }
}

/// The call at the foot of a section: right aligned, the one primary.
fn foot<'a>(calls: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut line = row![Space::new().width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Center);
    for call in calls {
        line = line.push(call);
    }
    line.into()
}

/// The dialog, over a dimmed window: the three ways in, one after another, in a sheet that scrolls.
pub fn view(form: &Form) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = |msg: Msg| (!form.busy).then_some(msg);

    let talk = section(
        "Talk it through",
        "The orchestrator asks what it needs and drafts the project beside the conversation: its tasks and its budget. You start it when it looks right.",
        column![
            field(
                "Your goal",
                area_box(
                    &form.ask,
                    "What do you want done? “The scheduler tests are flaky and I want 0.9 out this week.”",
                    96.0,
                    Msg::Ask,
                ),
                None,
            ),
            foot(vec![ui::btn(
                "Start a conversation",
                Some("conversations"),
                ui::Kind::Primary,
                enabled(Msg::Talk),
            )]),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into(),
        false,
    );

    let direct = section(
        "Or open it directly",
        "A project of yours: you put agents on it and give it tasks. Files you add become its first change.",
        column![
            field(
                "Title",
                text_box(&form.title, "Release 0.9", Msg::Title),
                None
            ),
            field(
                "Goal",
                area_box(&form.goal, "What the project delivers", 72.0, Msg::Goal),
                None
            ),
            field(
                "Kind",
                select_box(
                    KindChoice::ALL.to_vec(),
                    Some(if form.code {
                        KindChoice::Code
                    } else {
                        KindChoice::Other
                    }),
                    |kind| Msg::Code(kind == KindChoice::Code),
                ),
                None,
            ),
            foot(vec![ui::btn(
                "Open project",
                None,
                ui::Kind::Primary,
                enabled(Msg::Open),
            )]),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into(),
        false,
    );

    let bring = section(
        "Or bring a project that exists",
        "A product that is finished, or nearly: the plaza imports its history and looks after it.",
        adopt_fields(&form.adopt, form.busy),
        true,
    );

    let mut body = column![talk, direct, bring].spacing(24).width(Length::Fill);
    if let Some(e) = &form.error {
        body = body.push(ui::error_line(e));
    }
    let head = container(
        row![
            ui::sans("New project", 17.0, p.ink)
                .font(SANS_SEMIBOLD)
                .width(Length::Fill),
            ui::btn("", Some("x"), ui::Kind::Ghost, Some(Msg::Cancel)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 16.0,
        right: 14.0,
        bottom: 6.0,
        left: 22.0,
    });
    // Scrolls within the sheet: the three sections together are taller than a window.
    let sheet = container(column![
        head,
        ui::scroll(container(body).padding(Padding {
            top: 8.0,
            right: 22.0,
            bottom: 16.0,
            left: 22.0,
        })),
    ])
    .width(Length::Fixed(560.0))
    .max_height(720.0)
    .style(theme::pop);
    container(sheet)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().scrim.into()),
            ..Default::default()
        })
        .into()
}

/// The source's fields: the authorization and repository, the git address and branch, or the folder and branch.
fn source_fields(adopt: &AdoptForm) -> Element<'_, Msg> {
    let mut out: Vec<Element<'_, Msg>> = Vec::new();
    match adopt.source {
        SourceKind::Platform => match &adopt.platforms {
            None => out.push(ui::faint(if adopt.platforms_loading {
                "Loading the platform authorizations…"
            } else {
                "The platform authorizations did not load. Pick the source again to retry."
            }).into()),
            Some(list) if list.is_empty() => out.push(
                ui::faint("No git platform is set up yet. Connect one first, or bring the project from a git address or a folder.").into(),
            ),
            Some(list) => {
                out.push(field(
                    "Account",
                    select_box(list.clone(), adopt.platform.clone(), Msg::AdoptPlatform),
                    None,
                ));
                out.push(field("Repository", text_box(&adopt.repo, "owner/name", Msg::AdoptRepo), None));
                out.push(
                    checkbox(adopt.sign)
                        .label("Sign exports with the hub's key on my account")
                        .on_toggle(Msg::AdoptSign)
                        .size(14.0)
                        .text_size(crate::theme::TYPE_UI)
                        .into(),
                );
            }
        },
        SourceKind::Git => {
            out.push(field("Git address", text_box(&adopt.git, "https://…", Msg::AdoptGit), None));
            out.push(field("Branch", text_box(&adopt.branch, "main", Msg::AdoptBranch), None));
        }
        SourceKind::Path => {
            out.push(field(
                "Folder on the hub's machine",
                text_box(&adopt.path, "/Users/me/code/shop", Msg::AdoptPath),
                Some("An absolute path, starting with /"),
            ));
            out.push(field("Branch", text_box(&adopt.branch, "main", Msg::AdoptBranch), None));
        }
        SourceKind::Nothing => out.push(
            ui::faint("The files come later: upload them into the project once it is open.").into(),
        ),
    }
    column(out).spacing(12).width(Length::Fill).into()
}

/// The fields of "Or bring a project that exists", as the web lays them out.
fn adopt_fields(adopt: &AdoptForm, busy: bool) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = move |msg: Msg| (!busy).then_some(msg);

    // Environments: up to three rows of a name, a kind and an address, and a remove at the end of each.
    let mut env_rows: Vec<Element<'_, Msg>> = Vec::new();
    for (i, slot) in adopt.envs.iter().enumerate() {
        env_rows.push(
            row![
                text_input("production", &slot.name)
                    .on_input(move |s| Msg::AdoptEnvName(i, s))
                    .padding([6, 10])
                    .size(crate::theme::TYPE_UI)
                    .style(theme::input)
                    .width(Length::FillPortion(1)),
                pick_list(EnvKind::ALL.to_vec(), Some(slot.kind), move |k| {
                    Msg::AdoptEnvKind(i, k)
                })
                .style(theme::pick_list)
                .text_size(crate::theme::TYPE_UI)
                .width(Length::FillPortion(1)),
                text_input("https://…", &slot.url)
                    .on_input(move |s| Msg::AdoptEnvUrl(i, s))
                    .padding([6, 10])
                    .size(crate::theme::TYPE_UI)
                    .style(theme::input)
                    .width(Length::FillPortion(2)),
                ui::btn_sm("", Some("x"), ui::Kind::Ghost, Some(Msg::AdoptEnvRemove(i))),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    let add = (adopt.envs.len() < 3).then_some(Msg::AdoptEnvAdd);
    let environments = column![
        ui::sans("Environments (optional)", crate::theme::TYPE_UI, p.ink_2).font(SANS_MEDIUM),
        ui::faint("Each has a name, a kind and an address. Empty rows are ignored."),
        column(env_rows).spacing(8).width(Length::Fill),
        foot(vec![ui::btn_sm("+ Add", None, ui::Kind::Ghost, add)]),
    ]
    .spacing(8)
    .width(Length::Fill);

    column![
        field(
            "Title",
            text_box(&adopt.title, "Shop", Msg::AdoptTitle),
            None
        ),
        field(
            "Goal (optional)",
            area_box(&adopt.goal, "What it delivers", 56.0, Msg::AdoptGoal),
            None
        ),
        field(
            "Where it is",
            select_box(
                SourceKind::ALL.to_vec(),
                Some(adopt.source),
                Msg::AdoptSource
            ),
            None,
        ),
        source_fields(adopt),
        field(
            "Mode",
            select_box(AdoptMode::ALL.to_vec(), Some(adopt.mode), Msg::AdoptMode),
            Some(match adopt.mode {
                AdoptMode::Maintenance => "Agents watch it and propose; you pick what is done",
                AdoptMode::Build => "Agents keep building it",
            }),
        ),
        checkbox(adopt.finished)
            .label("It is finished")
            .on_toggle(Msg::AdoptFinished)
            .size(14.0)
            .text_size(crate::theme::TYPE_UI),
        ui::faint("On: it opens delivered. Off: it stays in implementation."),
        environments,
        foot(vec![ui::btn(
            "Bring it in",
            None,
            ui::Kind::Primary,
            enabled(Msg::Adopt)
        )]),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
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
    fn a_talk_with_words_waits_for_the_main_chat_to_take_them() {
        let mut form = Form {
            ask: Content::with_text("Build a weather app"),
            ..Default::default()
        };
        let (_, effect) = update(&mut form, Msg::Talk, &ctx());
        assert_eq!(effect, Effect::None);
        assert!(form.busy);
        assert!(form.error.is_none());
    }

    #[test]
    fn an_answer_opens_the_main_chat_or_the_project() {
        let mut form = Form {
            busy: true,
            ..Default::default()
        };
        let (_, effect) = update(&mut form, Msg::Talked(Ok(ChatMessage::default())), &ctx());
        assert_eq!(effect, Effect::Conversation("main".into()));
        assert!(!form.busy);
        let (_, effect) = update(&mut form, Msg::Talked(Err("down".into())), &ctx());
        assert_eq!(effect, Effect::None);
        assert_eq!(form.error.as_deref(), Some("down"));
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
