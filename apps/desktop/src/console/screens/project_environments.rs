//! A project's Environments tab (the web's `project-environments.ts`): where the project runs. Each environment is a
//! card: its address, what it runs, what is deployed there now, whether it has a deploy hook, its secrets (names only),
//! its variables, its notes and its last five deploys, with Remove, Edit and Deploy under it. The cards sit two to a
//! row when the window is wide enough. Adding, editing, deploying, the secrets and the confirmations are the web's
//! dialogs. A secret's value is sent once and never shown again.

use std::collections::BTreeMap;
use std::fmt;

use iced::widget::text_editor::Content;
use iced::widget::{
    Space, checkbox, column, container, pick_list, responsive, row, text, text_editor, text_input,
};
use iced::{Alignment, Element, Length, Size, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, rich, ui};
use crate::theme;

/// What an environment is for. The tab lists them in this order, production first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
    Production,
    Staging,
    Preview,
    Development,
    #[default]
    Other,
}

impl EnvKind {
    pub const ALL: [EnvKind; 5] = [
        EnvKind::Production,
        EnvKind::Staging,
        EnvKind::Preview,
        EnvKind::Development,
        EnvKind::Other,
    ];

    /// What an environment of this kind runs, unless the owner says otherwise: production the signed releases,
    /// staging and previews every landing on main, the rest what is deployed by hand.
    pub fn default_track(self) -> Track {
        match self {
            EnvKind::Production => Track::Releases,
            EnvKind::Staging | EnvKind::Preview => Track::Main,
            EnvKind::Development | EnvKind::Other => Track::Manual,
        }
    }

    /// Production is protected by default: the plaza never deploys there on its own.
    pub fn default_protected(self) -> bool {
        self == EnvKind::Production
    }

    /// The kind's name as the cards and the form show it.
    fn label(self) -> &'static str {
        match self {
            EnvKind::Production => "Production",
            EnvKind::Staging => "Staging",
            EnvKind::Preview => "Preview",
            EnvKind::Development => "Development",
            EnvKind::Other => "Other",
        }
    }
}

impl fmt::Display for EnvKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EnvKind::Production => "production",
            EnvKind::Staging => "staging",
            EnvKind::Preview => "preview",
            EnvKind::Development => "development",
            EnvKind::Other => "other",
        })
    }
}

/// What an environment runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Main,
    Releases,
    #[default]
    Manual,
}

impl Track {
    pub const ALL: [Track; 3] = [Track::Main, Track::Releases, Track::Manual];

    /// The card's words for it: "Runs: every landing on main".
    fn runs(self) -> &'static str {
        match self {
            Track::Main => "every landing on main",
            Track::Releases => "each signed release",
            Track::Manual => "what you deploy by hand",
        }
    }
}

impl fmt::Display for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Track::Main => "Every landing on main",
            Track::Releases => "Each signed release",
            Track::Manual => "What you deploy by hand",
        })
    }
}

/// A deploy of an environment, as the hub records it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Deploy {
    pub version: String,
    pub by: String,
    pub at_ms: u64,
    /// recorded, sending, sent, failed, waiting (for the owner's approval) or refused.
    pub status: String,
    pub detail: String,
}

/// One environment of the project, with its deploys (oldest first) and the names of its secrets.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Environment {
    pub name: String,
    pub kind: EnvKind,
    pub url: Option<String>,
    pub track: Track,
    pub protected: bool,
    pub variables: BTreeMap<String, String>,
    pub notes: String,
    pub version: Option<String>,
    pub deployed_ms: u64,
    pub deploys: Vec<Deploy>,
    pub secrets: Vec<String>,
    pub hook: bool,
}

/// A value the tab keeps out of logs and debug output: a secret's value, or a deploy hook (its URL carries a token).
#[derive(Clone, Default)]
pub struct Hidden(pub String);

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Hidden")
    }
}

/// The add or edit form, in its dialog.
#[derive(Debug)]
pub struct Form {
    /// "Add an environment", or "Edit" and its name.
    pub title: String,
    /// The environment being edited; None while a new one is added.
    pub editing: Option<String>,
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
    pub track: Track,
    pub protected: bool,
    /// Whether the environment has a deploy hook (edit only): then "Remove the hook" shows.
    pub has_hook: bool,
    /// A new deploy hook URL; empty keeps the current one.
    pub hook: Hidden,
    /// Removes the deploy hook (the hub gets `hook: null`).
    pub remove_hook: bool,
    /// One NAME=value per line.
    pub variables: text_editor::Content,
    pub notes: text_editor::Content,
    pub error: Option<String>,
}

/// The dialog that asks for a deploy: the environment, and its title.
#[derive(Debug, Clone, Default)]
pub struct Deploying {
    pub env: String,
    pub title: String,
}

/// The add-a-secret dialog under an environment.
#[derive(Debug, Default)]
pub struct SecretDraft {
    pub env: String,
    pub title: String,
    pub name: String,
    pub value: Hidden,
    pub error: Option<String>,
}

/// A removal the owner has to confirm.
#[derive(Debug)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub action: Remove,
}

#[derive(Debug, Clone)]
pub enum Remove {
    Env(String),
    Secret { env: String, name: String },
}

/// The tab's state: the environments, the dialog open, and what is in flight.
#[derive(Debug, Default)]
pub struct State {
    /// The environments, sorted; None until the first load.
    pub envs: Option<Vec<Environment>>,
    pub error: Option<String>,
    /// A line for the last change ("production: v1.4 is sent").
    pub note: Option<String>,
    /// A request is in flight: nothing else starts until it answers.
    pub busy: bool,
    pub form: Option<Form>,
    /// What is typed into each environment's Deploy dialog (empty: the default version).
    pub versions: BTreeMap<String, String>,
    pub deploying: Option<Deploying>,
    /// The removal waiting for the owner's confirmation.
    pub confirming: Option<Confirm>,
    pub secret: Option<SecretDraft>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Vec<Environment>, String>),
    Add,
    Edit(String),
    /// Closes whichever dialog is open over the tab.
    CloseDialog,
    Name(String),
    Kind(EnvKind),
    Url(String),
    Track(Track),
    Protected(bool),
    Hook(Hidden),
    RemoveHook(bool),
    Variables(text_editor::Action),
    Notes(text_editor::Action),
    Save,
    Saved(Result<String, String>),
    Version(String, String),
    OpenDeploy(String),
    Deploy(String),
    Deployed(Result<(String, Deploy), String>),
    AskRemove(String),
    CancelRemove,
    Confirm,
    Removed(Result<String, String>),
    OpenSecret(String),
    SecretName(String),
    SecretValue(Hidden),
    SaveSecret,
    SecretSaved(Result<String, String>),
    AskRemoveSecret(String, String),
    /// Opens a link in the browser.
    OpenUrl(String),
}

/// The navigation this tab asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The rule for an environment's name, as the form says it.
pub const NAME_RULE: &str =
    "An environment's name is lower-case letters, digits and dashes, starting with a letter.";
/// The rule for a variable's or a secret's name, as the dialogs say it.
const VAR_RULE: &str = "Letters, digits and underscores, starting with a letter.";

/// An environment's name: lower-case letters, digits and dashes, starting with a letter, 40 characters at most.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A variable's or a secret's name: letters, digits and underscores, starting with a letter, 64 characters at most.
pub fn valid_key(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// A web address: http or https, 2000 characters at most. Callers trim it first.
pub fn is_web_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://")) && url.len() <= 2000
}

/// The variables the form's text gives: one NAME=value per line, blank lines skipped.
pub fn parse_variables(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut vars = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((name, value)) = line.split_once('=') else {
            return Err("Each variable is NAME=value, one per line.".into());
        };
        let name = name.trim();
        if !valid_key(name) {
            return Err(format!(
                "“{name}” is not a variable name: letters, digits and underscores, starting with a letter."
            ));
        }
        if vars.contains_key(name) {
            return Err(format!("{name} is set twice."));
        }
        let value = value.trim();
        if value.chars().count() > 4000 {
            return Err(format!("{name} is longer than 4000 characters."));
        }
        vars.insert(name.to_string(), value.to_string());
    }
    if vars.len() > 100 {
        return Err("An environment has at most 100 variables.".into());
    }
    Ok(vars)
}

impl Form {
    /// A new environment's form: staging, as the first guess (its track and protection follow its kind).
    fn add() -> Form {
        let kind = EnvKind::Staging;
        Form {
            title: "Add an environment".into(),
            editing: None,
            name: String::new(),
            kind,
            url: String::new(),
            track: kind.default_track(),
            protected: kind.default_protected(),
            has_hook: false,
            hook: Hidden::default(),
            remove_hook: false,
            variables: Content::new(),
            notes: Content::new(),
            error: None,
        }
    }

    /// The form for an environment, as it is now.
    fn edit(env: &Environment) -> Form {
        let variables = env
            .variables
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        Form {
            title: format!("Edit {}", env.name),
            editing: Some(env.name.clone()),
            name: env.name.clone(),
            kind: env.kind,
            url: env.url.clone().unwrap_or_default(),
            track: env.track,
            protected: env.protected,
            has_hook: env.hook,
            hook: Hidden::default(),
            remove_hook: false,
            variables: Content::with_text(&variables),
            notes: Content::with_text(&env.notes),
            error: None,
        }
    }

    /// The environment's name and its request body (a PUT), or why the form cannot be saved yet.
    fn body(&self) -> Result<(String, Value), String> {
        let name = match &self.editing {
            Some(name) => name.clone(),
            None => {
                let name = self.name.trim().to_string();
                if !valid_name(&name) {
                    return Err(NAME_RULE.into());
                }
                name
            }
        };
        let url = self.url.trim();
        if !url.is_empty() && !is_web_url(url) {
            return Err("The URL starts with http:// or https://.".into());
        }
        let hook = self.hook.0.trim();
        if self.remove_hook && !hook.is_empty() {
            return Err("Remove the hook, or give a new one, not both.".into());
        }
        if !hook.is_empty() && !is_web_url(hook) {
            return Err("The deploy hook starts with http:// or https://.".into());
        }
        let variables = parse_variables(&self.variables.text())?;
        // The hub replaces the URL, the variables and the notes on every PUT, so they are always sent. An absent
        // track would default by kind, so it is sent too. The hook is the one field the hub keeps when absent.
        let mut body = json!({
            "kind": self.kind,
            "url": url,
            "track": self.track,
            "protected": self.protected,
            "variables": variables,
            "notes": self.notes.text().trim(),
        });
        if self.remove_hook {
            body["hook"] = Value::Null;
        } else if !hook.is_empty() {
            body["hook"] = json!(hook);
        }
        Ok((name, body))
    }
}

// ---- The hub: reads and writes -------------------------------------------------------------------------------------

/// Reads the project's environments.
pub fn load(project: &str, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = format!("/v1/projects/{}/environments", client::enc(project));
    client::task(move || c.get::<Vec<Environment>>(&path), Msg::Loaded)
}

/// `project` is the project's id; `ctx` the hub.
pub fn update(state: &mut State, msg: Msg, project: &str, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(mut envs)) => {
            envs.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
            state.envs = Some(envs);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Add => {
            state.form = Some(Form::add());
            Task::none()
        }
        Msg::Edit(name) => {
            state.form = state
                .envs
                .iter()
                .flatten()
                .find(|e| e.name == name)
                .map(Form::edit);
            Task::none()
        }
        Msg::CloseDialog => {
            state.form = None;
            state.deploying = None;
            state.secret = None;
            state.confirming = None;
            state.error = None;
            Task::none()
        }
        Msg::Name(s) => edit_form(state, |f| f.name = s),
        // A new kind takes its own track and protection, when the owner has kept the last kind's; else theirs stay.
        Msg::Kind(kind) => edit_form(state, |f| {
            let prev = f.kind;
            if f.track == prev.default_track() {
                f.track = kind.default_track();
            }
            if f.protected == prev.default_protected() {
                f.protected = kind.default_protected();
            }
            f.kind = kind;
        }),
        Msg::Url(s) => edit_form(state, |f| f.url = s),
        Msg::Track(track) => edit_form(state, |f| f.track = track),
        Msg::Protected(on) => edit_form(state, |f| f.protected = on),
        Msg::Hook(hook) => edit_form(state, |f| f.hook = hook),
        Msg::RemoveHook(on) => edit_form(state, |f| f.remove_hook = on),
        Msg::Variables(action) => edit_form(state, |f| f.variables.perform(action)),
        Msg::Notes(action) => edit_form(state, |f| f.notes.perform(action)),
        Msg::Save => save(state, project, ctx),
        Msg::Saved(Ok(line)) => {
            state.busy = false;
            state.form = None;
            state.note = Some(line);
            state.error = None;
            load(project, ctx)
        }
        Msg::Saved(Err(e)) => {
            state.busy = false;
            match state.form.as_mut() {
                Some(form) => form.error = Some(e),
                None => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Version(env, text) => {
            state.versions.insert(env, text);
            Task::none()
        }
        Msg::OpenDeploy(env) => {
            state.deploying = Some(Deploying {
                title: format!("Deploy {env}"),
                env,
            });
            state.error = None;
            Task::none()
        }
        Msg::Deploy(env) => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            let version = state
                .versions
                .get(&env)
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            // Empty: the hub deploys the default (the newest signed release, or main's head).
            let body = if version.is_empty() {
                json!({})
            } else {
                json!({ "version": version })
            };
            let c = ctx.client.clone();
            let path = format!(
                "/v1/projects/{}/environments/{}/deploy",
                client::enc(project),
                client::enc(&env)
            );
            client::task(
                move || c.post::<Deploy>(&path, &body).map(|d| (env, d)),
                Msg::Deployed,
            )
        }
        Msg::Deployed(Ok((env, deploy))) => {
            state.busy = false;
            state.versions.remove(&env);
            state.deploying = None;
            let (line, failed) = deploy_outcome(&env, &deploy.status);
            if failed {
                state.error = Some(line);
                state.note = None;
            } else {
                state.note = Some(line);
                state.error = None;
            }
            load(project, ctx)
        }
        Msg::Deployed(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            load(project, ctx)
        }
        Msg::AskRemove(env) => {
            state.confirming = Some(Confirm {
                title: format!("Remove {env} and its secrets?"),
                text: "This cannot be undone.".into(),
                action: Remove::Env(env),
            });
            Task::none()
        }
        Msg::AskRemoveSecret(env, name) => {
            state.secret = None;
            state.confirming = Some(Confirm {
                title: format!("Remove {name} from the vault?"),
                text: format!("{name} is taken out of {env}. Its value is removed too."),
                action: Remove::Secret { env, name },
            });
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirming = None;
            Task::none()
        }
        Msg::Confirm => {
            if state.busy {
                return Task::none();
            }
            let Some(confirm) = state.confirming.take() else {
                return Task::none();
            };
            state.busy = true;
            state.error = None;
            let c = ctx.client.clone();
            match confirm.action {
                Remove::Env(env) => {
                    let path = format!(
                        "/v1/projects/{}/environments/{}",
                        client::enc(project),
                        client::enc(&env)
                    );
                    let line = format!("{env} removed");
                    client::task(move || c.delete(&path).map(|_| line), Msg::Removed)
                }
                Remove::Secret { env, name } => {
                    let path = format!(
                        "/v1/projects/{}/environments/{}/secrets/{}",
                        client::enc(project),
                        client::enc(&env),
                        client::enc(&name)
                    );
                    let line = format!("{name} removed from {env}");
                    client::task(move || c.delete(&path).map(|_| line), Msg::Removed)
                }
            }
        }
        Msg::Removed(Ok(line)) => {
            state.busy = false;
            state.note = Some(line);
            state.error = None;
            load(project, ctx)
        }
        Msg::Removed(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            load(project, ctx)
        }
        Msg::OpenSecret(env) => {
            state.secret = Some(SecretDraft {
                title: format!("A secret for {env}"),
                env,
                ..SecretDraft::default()
            });
            Task::none()
        }
        Msg::SecretName(s) => {
            if let Some(draft) = state.secret.as_mut() {
                draft.name = s;
            }
            Task::none()
        }
        Msg::SecretValue(value) => {
            if let Some(draft) = state.secret.as_mut() {
                draft.value = value;
            }
            Task::none()
        }
        Msg::SaveSecret => save_secret(state, project, ctx),
        Msg::SecretSaved(Ok(line)) => {
            state.busy = false;
            state.secret = None;
            state.note = Some(line);
            state.error = None;
            load(project, ctx)
        }
        Msg::SecretSaved(Err(e)) => {
            state.busy = false;
            match state.secret.as_mut() {
                Some(draft) => draft.error = Some(e),
                None => state.error = Some(e),
            }
            Task::none()
        }
        Msg::OpenUrl(url) => {
            if let Err(e) = ui::open_url(&url) {
                state.error = Some(e);
            }
            Task::none()
        }
    }
}

/// Changes the open form, if there is one.
fn edit_form(state: &mut State, change: impl FnOnce(&mut Form)) -> Task<Msg> {
    if let Some(form) = state.form.as_mut() {
        change(form);
    }
    Task::none()
}

/// Saves the open form: refused in place when it is wrong, else a PUT, then a reload.
fn save(state: &mut State, project: &str, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    let Some(form) = state.form.as_mut() else {
        return Task::none();
    };
    let (name, body) = match form.body() {
        Ok(ok) => ok,
        Err(e) => {
            form.error = Some(e);
            return Task::none();
        }
    };
    form.error = None;
    state.busy = true;
    let c = ctx.client.clone();
    let path = format!(
        "/v1/projects/{}/environments/{}",
        client::enc(project),
        client::enc(&name)
    );
    let line = format!("{name} saved");
    client::task(
        move || c.put::<Value>(&path, &body).map(|_| line),
        Msg::Saved,
    )
}

/// Sends the secret under the open draft. Its value leaves the form with the request and is not kept.
fn save_secret(state: &mut State, project: &str, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    let Some(draft) = state.secret.as_mut() else {
        return Task::none();
    };
    let name = draft.name.trim().to_string();
    if !valid_key(&name) {
        draft.error = Some(
            "A secret's name is letters, digits and underscores, starting with a letter.".into(),
        );
        return Task::none();
    }
    if draft.value.0.is_empty() {
        draft.error = Some("A secret needs a value.".into());
        return Task::none();
    }
    let value = std::mem::take(&mut draft.value.0);
    let env = draft.env.clone();
    draft.error = None;
    state.busy = true;
    let c = ctx.client.clone();
    let path = format!(
        "/v1/projects/{}/environments/{}/secrets/{}",
        client::enc(project),
        client::enc(&env),
        client::enc(&name)
    );
    let line = format!("{name} is in the vault");
    client::task(
        move || {
            c.put::<Value>(&path, &json!({ "value": value }))
                .map(|_| line)
        },
        Msg::SecretSaved,
    )
}

/// What a deploy that has just been asked for says, and whether it is a failure to show as one (the web's words).
fn deploy_outcome(env: &str, status: &str) -> (String, bool) {
    match status {
        "sent" => (format!("Sent to {env}."), false),
        "recorded" => (format!("Recorded for {env}."), false),
        "sending" => (format!("Sending to {env}."), false),
        "waiting" => (
            format!("Waiting for an approval before it goes to {env}."),
            false,
        ),
        "failed" => (format!("The deploy to {env} failed."), true),
        "refused" => (format!("The deploy to {env} was refused."), true),
        other => (format!("Deploy to {env}: {other}."), false),
    }
}

/// A deploy's status in words, as its badge says it.
fn status_text(status: &str) -> &str {
    match status {
        "recorded" => "Recorded",
        "sending" => "Sending",
        "sent" => "Sent",
        "failed" => "Failed",
        "waiting" => "Waiting",
        "refused" => "Refused",
        other => other,
    }
}

/// A deploy's badge tone: done is ok, still going or waiting is warn, failed or refused is danger.
fn status_tone(status: &str) -> ui::Tone {
    match status {
        "recorded" | "sent" => ui::Tone::Ok,
        "failed" | "refused" => ui::Tone::Danger,
        "sending" | "waiting" => ui::Tone::Warn,
        _ => ui::Tone::Muted,
    }
}

/// How long ago an instant was, in the words the web uses ("just now", "5 min ago", "3 h ago", "2 d ago").
fn ago(ms: u64, now: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let s = (now.saturating_sub(ms) as f64 / 1000.0).round() as u64;
    match s {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", ((s as f64) / 60.0).round().max(1.0) as u64),
        3600..=86_399 => format!("{} h ago", ((s as f64) / 3600.0).round() as u64),
        _ => format!("{} d ago", ((s as f64) / 86_400.0).round() as u64),
    }
}

// ---- The view ------------------------------------------------------------------------------------------------------

/// A card's width below which the cards stack in one column (two of them at 360 and a gap fit a row).
const CARD_MIN: f32 = 360.0;
const GAP: f32 = 16.0;

/// The tab: the intro and the add button, one card per environment (production first), two to a row when wide.
pub fn view<'a>(state: &'a State, _project: &'a str) -> Element<'a, Msg> {
    let now = rich::now_ms();
    let mut parts: Vec<Element<'a, Msg>> = vec![
        row![
            ui::prose("Where this project runs. Each environment says what it runs, and what was deployed there."),
            Space::new().width(Length::Fill),
            ui::btn("Add an environment", Some("plus"), ui::Kind::Primary, Some(Msg::Add)),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .wrap()
        .into(),
    ];
    let dialog = dialog_view(state);
    if let (None, Some(e)) = (&dialog, &state.error) {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    match &state.envs {
        None => parts.push(ui::loading()),
        Some(envs) if envs.is_empty() => parts.push(ui::empty(
            "box",
            "No environments yet",
            "Add production, staging or a preview: its address, what it runs, and how it is deployed.",
        )),
        Some(envs) => parts.push(grid(envs, now)),
    }
    ui::with_modal(column(parts).spacing(16).into(), dialog)
}

/// The cards: two to a row when the tab is wide enough for two, else one under another.
fn grid<'a>(envs: &'a [Environment], now: u64) -> Element<'a, Msg> {
    responsive(move |size: Size| {
        let cards: Vec<Element<'a, Msg>> = envs.iter().map(|e| env_card(e, now)).collect();
        if size.width < 2.0 * CARD_MIN + GAP {
            return column(cards).spacing(GAP).into();
        }
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        let mut it = cards.into_iter();
        while let Some(a) = it.next() {
            let b: Element<'a, Msg> = match it.next() {
                Some(b) => b,
                None => Space::new().width(Length::Fill).into(),
            };
            rows.push(row![a, b].spacing(GAP).into());
        }
        column(rows).spacing(GAP).into()
    })
    .into()
}

/// The dialog open over the tab, if one is: the confirmation, the secret, the deploy, or the form, in that order.
fn dialog_view<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    if let Some(c) = &state.confirming {
        return Some(modal_of(
            state,
            &c.title,
            vec![ui::sans(c.text.clone(), theme::TYPE_UI, theme::palette().ink).into()],
            vec![
                ui::btn("Keep", None, ui::Kind::Ghost, Some(Msg::CancelRemove)),
                ui::btn(
                    "Remove",
                    None,
                    ui::Kind::Danger,
                    (!state.busy).then_some(Msg::Confirm),
                ),
            ],
        ));
    }
    if let Some(s) = &state.secret {
        return Some(secret_dialog(state, s));
    }
    if let Some(d) = &state.deploying {
        return Some(deploy_dialog(state, d));
    }
    state.form.as_ref().map(|f| form_dialog(state, f))
}

/// The dialog's frame: the title, the body with the open error above it, and the buttons in the foot.
fn modal_of<'a>(
    state: &'a State,
    title: &'a str,
    mut body: Vec<Element<'a, Msg>>,
    foot: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    if let Some(e) = &state.error {
        body.insert(0, ui::error_line(e));
    }
    ui::modal(
        title,
        column(body).spacing(14).into(),
        foot,
        Msg::CloseDialog,
        520.0,
    )
}

/// A dialog's field: its label, the control, and its hint under it.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: Option<&str>) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), control];
    if let Some(h) = hint {
        lines.push(ui::faint(h.to_string()).into());
    }
    column(lines).spacing(6).into()
}

/// The add or edit form: name, kind, URL, what it runs, protection, the deploy hook, variables and notes.
fn form_dialog<'a>(state: &'a State, form: &'a Form) -> Element<'a, Msg> {
    let adding = form.editing.is_none();
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    body.push(field(
        "Name",
        match &form.editing {
            None => text_input("production, staging, pr-42", &form.name)
                .on_input(Msg::Name)
                .padding(8)
                .style(theme::input)
                .into(),
            Some(name) => text_input("", name).padding(8).style(theme::input).into(),
        },
        Some(if adding {
            NAME_RULE
        } else {
            "A name is fixed once it is made."
        }),
    ));
    body.push(field(
        "Kind",
        pick_list(EnvKind::ALL.to_vec(), Some(form.kind), Msg::Kind)
            .style(theme::pick_list)
            .text_size(13.0)
            .into(),
        None,
    ));
    body.push(field(
        "URL",
        text_input("https://", &form.url)
            .on_input(Msg::Url)
            .padding(8)
            .style(theme::input)
            .into(),
        Some("Where it is reached. Optional."),
    ));
    body.push(field(
        "Runs",
        pick_list(Track::ALL.to_vec(), Some(form.track), Msg::Track)
            .style(theme::pick_list)
            .text_size(13.0)
            .into(),
        Some("What lands here: each landing on main, each signed release, or what you deploy by hand."),
    ));
    body.push(
        checkbox(form.protected)
            .label("Protected. A deploy here waits for an approval, unless you start it yourself.")
            .on_toggle(Msg::Protected)
            .size(14.0)
            .text_size(13.0)
            .into(),
    );
    body.push(field(
        "Deploy hook URL",
        text_input(
            if form.has_hook {
                "Set — leave empty to keep it"
            } else {
                "https://"
            },
            &form.hook.0,
        )
        .secure(true)
        .on_input(|v| Msg::Hook(Hidden(v)))
        .padding(8)
        .style(theme::input)
        .into(),
        Some("Where a deploy is sent. It is kept in the vault: the console shows only that it is set."),
    ));
    if form.has_hook {
        body.push(
            checkbox(form.remove_hook)
                .label("Remove the hook")
                .on_toggle(Msg::RemoveHook)
                .size(14.0)
                .text_size(13.0)
                .into(),
        );
    }
    body.push(field(
        "Variables",
        text_editor(&form.variables)
            .style(crate::theme::editor)
            .placeholder("NAME=value")
            .size(13.0)
            .height(Length::Fixed(100.0))
            .on_action(Msg::Variables)
            .into(),
        Some("One NAME=value per line. Letters, digits and underscores, starting with a letter. Variables show on the card: keep private values in a secret."),
    ));
    body.push(field(
        "Notes",
        text_editor(&form.notes)
            .style(crate::theme::editor)
            .size(13.0)
            .height(Length::Fixed(60.0))
            .on_action(Msg::Notes)
            .into(),
        None,
    ));
    if let Some(e) = &form.error {
        body.push(ui::error_line(e));
    }
    modal_of(
        state,
        &form.title,
        body,
        vec![ui::btn(
            "Save",
            None,
            ui::Kind::Primary,
            (!state.busy).then_some(Msg::Save),
        )],
    )
}

/// The deploy dialog: what the deploy does (at once for a protected one), and the version to deploy.
fn deploy_dialog<'a>(state: &'a State, d: &'a Deploying) -> Element<'a, Msg> {
    let protected = state
        .envs
        .iter()
        .flatten()
        .find(|e| e.name == d.env)
        .is_some_and(|e| e.protected);
    let line = if protected {
        "You are the owner: this deploys at once."
    } else {
        "It goes to its deploy hook when it has one, and is only recorded when it has none."
    };
    let version = state
        .versions
        .get(&d.env)
        .map(String::as_str)
        .unwrap_or_default();
    let env = d.env.clone();
    modal_of(
        state,
        &d.title,
        vec![
            ui::prose(line).into(),
            field(
                "Version",
                text_input("Empty: the newest signed release or main’s head", version)
                    .on_input(move |v| Msg::Version(env.clone(), v))
                    .padding(8)
                    .style(theme::input)
                    .into(),
                None,
            ),
        ],
        vec![ui::btn(
            "Deploy",
            Some("play"),
            ui::Kind::Primary,
            (!state.busy).then(|| Msg::Deploy(d.env.clone())),
        )],
    )
}

/// The secrets of one environment: names are listed (each removable), and a value is typed in once, never read back.
fn secret_dialog<'a>(state: &'a State, s: &'a SecretDraft) -> Element<'a, Msg> {
    let pal = theme::palette();
    let mut body: Vec<Element<'a, Msg>> =
        vec![ui::prose("The value goes into the vault and is never shown again.").into()];
    let names: Vec<String> = state
        .envs
        .iter()
        .flatten()
        .find(|e| e.name == s.env)
        .map(|e| e.secrets.clone())
        .unwrap_or_default();
    if !names.is_empty() {
        let chips: Vec<Element<'a, Msg>> = names
            .into_iter()
            .map(|n| {
                row![
                    ui::mono(n.clone(), theme::TYPE_CODE, pal.ink_2),
                    ui::btn_sm(
                        "Remove…",
                        None,
                        ui::Kind::Ghost,
                        Some(Msg::AskRemoveSecret(s.env.clone(), n)),
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            })
            .collect();
        body.push(
            row![ui::faint("In the vault"), row(chips).spacing(12).wrap()]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    body.push(field(
        "Name",
        text_input("STRIPE_KEY", &s.name)
            .on_input(Msg::SecretName)
            .padding(8)
            .style(theme::input)
            .into(),
        Some(VAR_RULE),
    ));
    body.push(field(
        "Value",
        text_input("", &s.value.0)
            .secure(true)
            .on_input(|v| Msg::SecretValue(Hidden(v)))
            .on_submit(Msg::SaveSecret)
            .padding(8)
            .style(theme::input)
            .into(),
        None,
    ));
    if let Some(e) = &s.error {
        body.push(ui::error_line(e));
    }
    modal_of(
        state,
        &s.title,
        body,
        vec![ui::btn(
            "Save secret",
            None,
            ui::Kind::Primary,
            (!state.busy).then_some(Msg::SaveSecret),
        )],
    )
}

/// One environment's card: its name and kind, its address, what it runs and what is deployed there, its deploy hook,
/// its secrets, variables and notes, its last five deploys, and Remove, Edit and Deploy.
fn env_card<'a>(e: &'a Environment, now: u64) -> Element<'a, Msg> {
    let pal = theme::palette();
    let mut chips: Vec<Element<'a, Msg>> = vec![ui::badge::<Msg>(e.kind.label(), ui::Tone::Muted)];
    if e.protected {
        chips.push(ui::badge::<Msg>("Protected", ui::Tone::Info));
    }
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row![
            text(e.name.clone())
                .font(ui::SANS_SEMIBOLD)
                .size(15.0)
                .color(pal.ink),
            Space::new().width(Length::Fill),
            row(chips).spacing(8).align_y(Alignment::Center),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap()
        .into(),
    ];
    if let Some(url) = e.url.as_deref().filter(|u| !u.is_empty()) {
        lines.push(if is_web_url(url) {
            ui::link(url, Msg::OpenUrl(url.to_string()), true)
        } else {
            ui::mono(url.to_string(), theme::TYPE_META, pal.ink_2).into()
        });
    }
    let version = match &e.version {
        Some(v) if !v.is_empty() => format!("{v} · {}", ago(e.deployed_ms, now)),
        _ => "Nothing deployed yet".into(),
    };
    lines.push(
        column![
            kv("Runs", e.track.runs().to_string()),
            kv("Now", version),
            kv(
                "Deploy hook",
                if e.hook {
                    "set".into()
                } else {
                    "not set: deploys are recorded only".into()
                }
            ),
        ]
        .spacing(8)
        .into(),
    );
    let mut secrets: Vec<Element<'a, Msg>> = vec![ui::faint("Secrets").into()];
    if e.secrets.is_empty() {
        secrets.push(ui::faint("None").into());
    }
    for n in &e.secrets {
        secrets.push(
            container(ui::mono(n.clone(), theme::TYPE_BADGE, pal.ink_2))
                .padding([2, 6])
                .style(ui::frame(Some(pal.sheet_2), pal.sheet_2))
                .into(),
        );
    }
    secrets.push(ui::btn_sm(
        "Secret…",
        Some("key-round"),
        ui::Kind::Ghost,
        Some(Msg::OpenSecret(e.name.clone())),
    ));
    lines.push(
        row(secrets)
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap()
            .into(),
    );
    if !e.variables.is_empty() {
        let rows: Vec<Element<'a, Msg>> =
            e.variables.iter().map(|(k, v)| kv(k, v.clone())).collect();
        lines.push(column(rows).spacing(8).into());
    }
    if !e.notes.is_empty() {
        lines.push(ui::sans(e.notes.clone(), theme::TYPE_UI, pal.ink_2).into());
    }
    lines.push(deploys(e, now));
    lines.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn_sm(
                "Remove…",
                Some("trash"),
                ui::Kind::Ghost,
                Some(Msg::AskRemove(e.name.clone())),
            ),
            ui::btn_sm(
                "Edit…",
                Some("edit"),
                ui::Kind::Secondary,
                Some(Msg::Edit(e.name.clone())),
            ),
            ui::btn_sm(
                "Deploy…",
                Some("play"),
                ui::Kind::Primary,
                Some(Msg::OpenDeploy(e.name.clone())),
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap()
        .into(),
    );
    container(column(lines).spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(ui::frame(Some(pal.raised), pal.rule))
        .into()
}

/// A label and its value on one line: the label in the mono caps, the value beside it.
fn kv<'a>(label: &str, value: String) -> Element<'a, Msg> {
    let pal = theme::palette();
    row![
        ui::mono(label.to_uppercase(), 11.0, pal.ink_3).width(Length::Fixed(110.0)),
        ui::sans(value, theme::TYPE_UI, pal.ink).width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Start)
    .into()
}

/// The last five deploys, newest first, each with its version, its status, who and when, and its detail.
fn deploys<'a>(e: &'a Environment, now: u64) -> Element<'a, Msg> {
    let pal = theme::palette();
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::faint("Recent deploys").into()];
    if e.deploys.is_empty() {
        lines.push(ui::faint("No deploys yet").into());
    }
    for (i, d) in e.deploys.iter().rev().take(5).enumerate() {
        if i > 0 {
            lines.push(ui::hairline());
        }
        let head: Vec<Element<'a, Msg>> = vec![
            container(ui::mono(
                if d.version.is_empty() {
                    "—".into()
                } else {
                    d.version.clone()
                },
                theme::TYPE_CODE,
                pal.ink_2,
            ))
            .padding([2, 8])
            .style(ui::frame(Some(pal.sheet_2), pal.sheet_2))
            .into(),
            ui::badge::<Msg>(status_text(&d.status), status_tone(&d.status)),
            ui::faint(format!("{} · {}", d.by, ago(d.at_ms, now))).into(),
        ];
        let mut block: Vec<Element<'a, Msg>> = vec![
            row(head)
                .spacing(8)
                .align_y(Alignment::Center)
                .wrap()
                .into(),
        ];
        if !d.detail.is_empty() {
            block.push(ui::faint(d.detail.clone()).into());
        }
        lines.push(column(block).spacing(4).padding([8, 0]).into());
    }
    column(lines).spacing(4).into()
}

impl From<Id> for Msg {
    fn from(id: Id) -> Msg {
        Msg::Go(id)
    }
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

    fn env(name: &str, kind: EnvKind) -> Environment {
        Environment {
            name: name.into(),
            kind,
            ..Environment::default()
        }
    }

    #[test]
    fn a_load_lists_production_first_then_by_name() {
        let mut state = State::default();
        let envs = vec![
            env("zz-dev", EnvKind::Development),
            env("beta", EnvKind::Staging),
            env("prod-eu", EnvKind::Production),
            env("misc", EnvKind::Other),
            env("alpha", EnvKind::Staging),
            env("pr-42", EnvKind::Preview),
            env("prod", EnvKind::Production),
        ];
        let _ = update(&mut state, Msg::Loaded(Ok(envs)), "pr2", &ctx());
        let order: Vec<&str> = state
            .envs
            .iter()
            .flatten()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            order,
            [
                "prod", "prod-eu", "alpha", "beta", "pr-42", "zz-dev", "misc"
            ]
        );
    }

    #[test]
    fn a_kind_brings_its_default_track_and_protection() {
        assert_eq!(EnvKind::Production.default_track(), Track::Releases);
        assert!(EnvKind::Production.default_protected());
        assert_eq!(EnvKind::Staging.default_track(), Track::Main);
        assert_eq!(EnvKind::Preview.default_track(), Track::Main);
        assert!(!EnvKind::Staging.default_protected());
        assert_eq!(EnvKind::Development.default_track(), Track::Manual);
        assert_eq!(EnvKind::Other.default_track(), Track::Manual);
        assert!(!EnvKind::Other.default_protected());
    }

    #[test]
    fn a_new_kind_in_the_form_takes_its_defaults() {
        let mut state = State::default();
        let _ = update(&mut state, Msg::Add, "pr2", &ctx());
        let _ = update(&mut state, Msg::Kind(EnvKind::Production), "pr2", &ctx());
        let form = state.form.as_ref().expect("the form is open");
        assert_eq!((form.track, form.protected), (Track::Releases, true));
        let _ = update(&mut state, Msg::Kind(EnvKind::Staging), "pr2", &ctx());
        let form = state.form.as_ref().expect("the form is open");
        assert_eq!((form.track, form.protected), (Track::Main, false));
    }

    #[test]
    fn variables_are_name_value_lines_with_checked_names() {
        let vars = parse_variables("API_BASE = https://example.com\n\nDEBUG=0\nEMPTY=\n")
            .expect("valid lines");
        assert_eq!(
            vars.get("API_BASE").map(String::as_str),
            Some("https://example.com")
        );
        assert_eq!(vars.get("DEBUG").map(String::as_str), Some("0"));
        assert_eq!(vars.get("EMPTY").map(String::as_str), Some(""));
        assert!(parse_variables("1BAD=x").is_err());
        assert!(parse_variables("GOOD-NAME=x").is_err());
        assert!(parse_variables("NO_EQUALS").is_err());
        assert!(parse_variables("A=1\nA=2").is_err());
    }

    #[test]
    fn environment_names_are_lower_case_and_start_with_a_letter() {
        let long = "a".repeat(40);
        let too_long = "a".repeat(41);
        for ok in ["production", "pr-42", "a", long.as_str()] {
            assert!(valid_name(ok), "{ok} is a name");
        }
        for bad in [
            "",
            "Prod",
            "1st",
            "-x",
            "under_score",
            "a b",
            too_long.as_str(),
        ] {
            assert!(!valid_name(bad), "{bad} is not a name");
        }
        assert!(valid_key("API_TOKEN") && valid_key("x1"));
        assert!(!valid_key("1X") && !valid_key("A-B") && !valid_key(""));
    }

    #[test]
    fn a_form_refuses_a_bad_name_or_url_before_it_saves() {
        let mut state = State::default();
        let _ = update(&mut state, Msg::Add, "pr2", &ctx());
        let _ = update(&mut state, Msg::Name("Staging".into()), "pr2", &ctx());
        let _ = update(&mut state, Msg::Save, "pr2", &ctx());
        assert!(!state.busy, "nothing is sent for a bad name");
        assert_eq!(
            state.form.as_ref().and_then(|f| f.error.as_deref()),
            Some(NAME_RULE)
        );
        let _ = update(&mut state, Msg::Name("staging".into()), "pr2", &ctx());
        let _ = update(
            &mut state,
            Msg::Url("ftp://example.com".into()),
            "pr2",
            &ctx(),
        );
        let _ = update(&mut state, Msg::Save, "pr2", &ctx());
        assert!(!state.busy);
        assert_eq!(
            state.form.as_ref().and_then(|f| f.error.as_deref()),
            Some("The URL starts with http:// or https://.")
        );
    }

    #[test]
    fn the_hook_is_kept_when_absent_and_nulled_when_removed() {
        let mut existing = env("production", EnvKind::Production);
        existing.track = Track::Releases;
        existing.protected = true;
        existing.hook = true;
        let form = Form::edit(&existing);
        let (name, body) = form.body().expect("a valid form");
        assert_eq!(name, "production");
        assert!(
            body.get("hook").is_none(),
            "an empty hook keeps the current one"
        );
        assert_eq!(body["track"], "releases");
        assert_eq!(body["protected"], true);

        let mut form = Form::edit(&existing);
        form.remove_hook = true;
        let (_, body) = form.body().expect("a valid form");
        assert!(body["hook"].is_null(), "the toggle sends hook: null");
    }

    #[test]
    fn a_deploy_says_what_became_of_it_and_a_failure_is_one() {
        assert_eq!(
            deploy_outcome("production", "sent"),
            ("Sent to production.".to_string(), false)
        );
        assert_eq!(
            deploy_outcome("staging", "refused"),
            ("The deploy to staging was refused.".to_string(), true)
        );
    }

    #[test]
    fn the_cards_say_how_long_ago_in_the_web_words() {
        let now: u64 = 1_800_000_000_000;
        assert_eq!(ago(now - 20_000, now), "just now");
        assert_eq!(ago(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3 h ago");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2 d ago");
        assert_eq!(ago(0, now), "—");
    }
}
