//! A project's Environments tab (the web's `project-environments.ts`): where the project runs. Each environment says
//! what it runs (every landing on main, each signed release, or what is deployed by hand), what was deployed there,
//! and what it has: its variables and the names of its secrets. The owner adds, edits and removes environments,
//! deploys to them and adds secrets; a secret's value is sent once and never shown again.

use std::collections::BTreeMap;
use std::fmt;

use iced::widget::{checkbox, column, pick_list, row, text_editor, text_input};
use iced::{Alignment, Element, Length, Task};
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

/// The add or edit form, inline above the cards.
#[derive(Debug)]
pub struct Form {
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
    pub notes: String,
    pub error: Option<String>,
}

/// The add-a-secret form under an environment's card.
#[derive(Debug, Default)]
pub struct SecretDraft {
    pub env: String,
    pub name: String,
    pub value: Hidden,
    pub error: Option<String>,
}

/// The tab's state: the environments, the open form, and what is in flight.
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
    /// What is typed into each environment's Deploy field (empty: the default version).
    pub versions: BTreeMap<String, String>,
    /// The environment whose removal waits for the owner's confirmation.
    pub confirming: Option<String>,
    pub secret: Option<SecretDraft>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Vec<Environment>, String>),
    Add,
    Edit(String),
    CloseForm,
    Name(String),
    Kind(EnvKind),
    Url(String),
    Track(Track),
    Protected(bool),
    Hook(Hidden),
    RemoveHook(bool),
    Variables(text_editor::Action),
    Notes(String),
    Save,
    Saved(Result<String, String>),
    Version(String, String),
    Deploy(String),
    Deployed(Result<(String, Deploy), String>),
    AskRemove(String),
    CancelRemove,
    Remove(String),
    Removed(Result<String, String>),
    OpenSecret(String),
    CloseSecret,
    SecretName(String),
    SecretValue(Hidden),
    SaveSecret,
    SecretSaved(Result<String, String>),
}

/// The navigation this tab asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The rule for an environment's name, as the form and the adopt dialog say it.
pub const NAME_RULE: &str =
    "An environment's name is lower-case letters, digits and dashes, starting with a letter.";

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
            editing: None,
            name: String::new(),
            kind,
            url: String::new(),
            track: kind.default_track(),
            protected: kind.default_protected(),
            has_hook: false,
            hook: Hidden::default(),
            remove_hook: false,
            variables: text_editor::Content::new(),
            notes: String::new(),
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
            editing: Some(env.name.clone()),
            name: env.name.clone(),
            kind: env.kind,
            url: env.url.clone().unwrap_or_default(),
            track: env.track,
            protected: env.protected,
            has_hook: env.hook,
            hook: Hidden::default(),
            remove_hook: false,
            variables: text_editor::Content::with_text(&variables),
            notes: env.notes.clone(),
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
            "notes": self.notes.trim(),
        });
        if self.remove_hook {
            body["hook"] = Value::Null;
        } else if !hook.is_empty() {
            body["hook"] = json!(hook);
        }
        Ok((name, body))
    }
}

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
        Msg::CloseForm => {
            state.form = None;
            Task::none()
        }
        Msg::Name(s) => edit_form(state, |f| f.name = s),
        // A new kind takes its own track and protection; the owner can change either after.
        Msg::Kind(kind) => edit_form(state, |f| {
            f.kind = kind;
            f.track = kind.default_track();
            f.protected = kind.default_protected();
        }),
        Msg::Url(s) => edit_form(state, |f| f.url = s),
        Msg::Track(track) => edit_form(state, |f| f.track = track),
        Msg::Protected(on) => edit_form(state, |f| f.protected = on),
        Msg::Hook(hook) => edit_form(state, |f| f.hook = hook),
        Msg::RemoveHook(on) => edit_form(state, |f| f.remove_hook = on),
        Msg::Variables(action) => edit_form(state, |f| f.variables.perform(action)),
        Msg::Notes(s) => edit_form(state, |f| f.notes = s),
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
            state.note = Some(deploy_line(&env, &deploy));
            state.error = None;
            load(project, ctx)
        }
        Msg::Deployed(Err(e)) => {
            state.busy = false;
            state.error = Some(e);
            load(project, ctx)
        }
        Msg::AskRemove(env) => {
            state.confirming = Some(env);
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirming = None;
            Task::none()
        }
        Msg::Remove(env) => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            state.confirming = None;
            let c = ctx.client.clone();
            let path = format!(
                "/v1/projects/{}/environments/{}",
                client::enc(project),
                client::enc(&env)
            );
            let line = format!("{env} removed");
            client::task(move || c.delete(&path).map(|_| line), Msg::Removed)
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
                env,
                ..SecretDraft::default()
            });
            Task::none()
        }
        Msg::CloseSecret => {
            state.secret = None;
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
    let line = format!("{name} saved on {env}. Its value is never shown again.");
    client::task(
        move || {
            c.put::<Value>(&path, &json!({ "value": value }))
                .map(|_| line)
        },
        Msg::SecretSaved,
    )
}

/// The line for a deploy the hub answered: "production: v1.4 is sent".
fn deploy_line(env: &str, deploy: &Deploy) -> String {
    let mut line = format!(
        "{env}: {} is {}",
        deploy.version,
        status_label(&deploy.status)
    );
    if !deploy.detail.is_empty() {
        line.push_str(" · ");
        line.push_str(&deploy.detail);
    }
    line
}

/// A deploy's status in words.
fn status_label(status: &str) -> &str {
    match status {
        "waiting" => "waiting for approval",
        other => other,
    }
}

fn status_tone(status: &str) -> ui::Tone {
    match status {
        "sent" => ui::Tone::Ok,
        "failed" | "refused" => ui::Tone::Danger,
        "waiting" => ui::Tone::Warn,
        "sending" => ui::Tone::Info,
        _ => ui::Tone::Muted,
    }
}

/// How long ago an instant was, in the words the cards use ("3h ago").
fn ago(ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// The tab: the intro, the add button, the open form, then one card per environment, production first.
pub fn view<'a>(state: &'a State, _project: &'a str) -> Element<'a, Msg> {
    let now = rich::now_ms();
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::faint("Where this project runs. Each environment says what it runs, and what was deployed there.").into(),
        row![ui::btn(
            "Add an environment",
            Some("plus"),
            ui::Kind::Secondary,
            Some(Msg::Add)
        )]
        .into(),
    ];
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    if let Some(form) = &state.form {
        parts.push(form_card(form, state.busy));
    }
    let Some(envs) = &state.envs else {
        parts.push(ui::loading());
        return column(parts).spacing(16).into();
    };
    if envs.is_empty() && state.form.is_none() {
        parts.push(
            ui::faint("No environments yet. An environment is somewhere the project runs: production, staging, a preview, or a machine you deploy to by hand.")
                .into(),
        );
    }
    for env in envs {
        parts.push(env_card(state, env, now));
    }
    column(parts).spacing(16).into()
}

/// The add or edit form, as a card.
fn form_card(form: &Form, busy: bool) -> Element<'_, Msg> {
    let p = theme::palette();
    let enabled = move |msg: Msg| (!busy).then_some(msg);
    let mut body: Vec<Element<'_, Msg>> = Vec::new();
    match &form.editing {
        None => {
            body.push(ui::label("Name").into());
            body.push(
                text_input("production, staging, pr-42", &form.name)
                    .on_input(Msg::Name)
                    .padding(8)
                    .style(theme::input)
                    .into(),
            );
        }
        Some(name) => {
            body.push(ui::label("Name").into());
            body.push(ui::sans(name.clone(), 13.0, p.ink).into());
        }
    }
    body.push(ui::label("Kind").into());
    body.push(
        pick_list(EnvKind::ALL.to_vec(), Some(form.kind), Msg::Kind)
            .style(theme::pick_list)
            .text_size(13.0)
            .into(),
    );
    body.push(ui::label("URL").into());
    body.push(
        text_input("https://example.com", &form.url)
            .on_input(Msg::Url)
            .padding(8)
            .style(theme::input)
            .into(),
    );
    body.push(ui::label("Runs").into());
    body.push(
        pick_list(Track::ALL.to_vec(), Some(form.track), Msg::Track)
            .style(theme::pick_list)
            .text_size(13.0)
            .into(),
    );
    body.push(
        checkbox(form.protected)
            .label("Protected")
            .on_toggle(Msg::Protected)
            .size(14.0)
            .text_size(13.0)
            .into(),
    );
    body.push(ui::faint("The plaza never deploys here on its own or for an agent: a tracked landing or release waits for the owner.").into());
    body.push(ui::label("Deploy hook URL").into());
    let hook_hint = if form.has_hook {
        "Empty keeps the current hook"
    } else {
        "Empty for none: deploys are recorded only"
    };
    body.push(
        text_input(hook_hint, &form.hook.0)
            .secure(true)
            .on_input(|v| Msg::Hook(Hidden(v)))
            .padding(8)
            .style(theme::input)
            .into(),
    );
    body.push(ui::faint("Kept in the vault. Its URL carries a token, so it is not shown.").into());
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
    body.push(ui::label("Variables").into());
    body.push(
        ui::faint("NAME=value, one per line. A name is letters, digits and underscores.").into(),
    );
    body.push(
        text_editor(&form.variables)
            .placeholder("API_BASE=https://example.com")
            .size(13.0)
            .height(Length::Fixed(120.0))
            .on_action(Msg::Variables)
            .into(),
    );
    body.push(ui::label("Notes").into());
    body.push(
        text_input("What this environment is for", &form.notes)
            .on_input(Msg::Notes)
            .padding(8)
            .style(theme::input)
            .into(),
    );
    if let Some(e) = &form.error {
        body.push(ui::error_line(e));
    }
    body.push(
        row![
            ui::btn(
                "Save environment",
                Some("check"),
                ui::Kind::Primary,
                enabled(Msg::Save)
            ),
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseForm)),
        ]
        .spacing(8)
        .into(),
    );
    let title = match &form.editing {
        Some(name) => format!("Edit {name}"),
        None => "Add an environment".to_string(),
    };
    ui::card(&title, column(body).spacing(10).into())
}

/// One environment's card: what it is and runs, what was deployed, its settings, and its actions.
fn env_card<'a>(state: &'a State, env: &'a Environment, now: u64) -> Element<'a, Msg> {
    let p = theme::palette();
    let busy = state.busy;
    let name = env.name.clone();
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();

    let mut head = row![ui::sans(env.kind.to_string(), 13.0, p.ink_2)]
        .spacing(10)
        .align_y(Alignment::Center);
    if env.protected {
        head = head.push(ui::badge("Protected", ui::Tone::Info));
    }
    lines.push(head.into());
    if let Some(url) = env.url.as_deref().filter(|u| !u.is_empty()) {
        lines.push(ui::mono(url.to_string(), 12.0, p.ink_2).into());
    }
    lines.push(ui::sans(format!("Runs: {}", env.track.runs()), 13.0, p.ink).into());
    let now_line = match env.version.as_deref().filter(|v| !v.is_empty()) {
        Some(version) if env.deployed_ms > 0 => {
            format!("Now: {version} · {}", ago(env.deployed_ms, now))
        }
        _ => "Nothing deployed yet".to_string(),
    };
    lines.push(ui::sans(now_line, 13.0, p.ink).into());
    lines.push(
        ui::faint(if env.hook {
            "Deploy hook: set"
        } else {
            "No deploy hook: deploys are recorded only"
        })
        .into(),
    );
    lines.push(
        ui::faint(if env.secrets.is_empty() {
            "No secrets".to_string()
        } else {
            format!("Secrets: {}", env.secrets.join(", "))
        })
        .into(),
    );
    if env.variables.is_empty() {
        lines.push(ui::faint("No variables").into());
    } else {
        let vars = env
            .variables
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(
            column![ui::label("Variables"), ui::mono(vars, 12.0, p.ink_2)]
                .spacing(4)
                .into(),
        );
    }
    if !env.notes.is_empty() {
        lines.push(ui::prose(env.notes.clone()).into());
    }
    if !env.deploys.is_empty() {
        let mut history: Vec<Element<'a, Msg>> = vec![ui::label("Recent deploys").into()];
        // The hub keeps oldest first: the newest five, newest first.
        for deploy in env.deploys.iter().rev().take(5) {
            history.push(deploy_row(deploy, now));
        }
        lines.push(column(history).spacing(8).into());
    }

    let typed = state
        .versions
        .get(&env.name)
        .map(String::as_str)
        .unwrap_or("");
    let version_name = name.clone();
    let deploy_name = name.clone();
    lines.push(
        row![
            text_input("Version, or empty for the default", typed)
                .on_input(move |v| Msg::Version(version_name.clone(), v))
                .padding(8)
                .style(theme::input)
                .width(Length::Fill),
            ui::btn_sm(
                "Deploy",
                Some("play"),
                ui::Kind::Primary,
                (!busy).then_some(Msg::Deploy(deploy_name))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );

    let mut actions = row![ui::btn_sm(
        "Edit",
        Some("edit"),
        ui::Kind::Secondary,
        Some(Msg::Edit(name.clone()))
    )]
    .spacing(8)
    .align_y(Alignment::Center);
    let confirming = state.confirming.as_deref() == Some(env.name.as_str());
    if !confirming {
        actions = actions.push(ui::btn_sm(
            "Remove",
            Some("trash"),
            ui::Kind::Ghost,
            Some(Msg::AskRemove(name.clone())),
        ));
    }
    let secret_open = state.secret.as_ref().filter(|d| d.env == env.name);
    if secret_open.is_none() {
        actions = actions.push(ui::btn_sm(
            "Add a secret",
            Some("key"),
            ui::Kind::Secondary,
            Some(Msg::OpenSecret(name.clone())),
        ));
    }
    lines.push(actions.into());
    if confirming {
        lines.push(
            column![
                ui::faint(format!(
                    "Remove {}? Its settings, deploy record, secrets and deploy hook go with it.",
                    env.name
                )),
                row![
                    ui::btn_sm(
                        "Remove",
                        Some("trash"),
                        ui::Kind::Danger,
                        (!busy).then(|| Msg::Remove(name.clone()))
                    ),
                    ui::btn_sm("Keep it", None, ui::Kind::Ghost, Some(Msg::CancelRemove)),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .into(),
        );
    }
    if let Some(draft) = secret_open {
        lines.push(secret_form(draft, busy));
    }
    ui::card(&env.name, column(lines).spacing(10).into())
}

/// One deploy in an environment's history: its version and status, who and when, and its detail.
fn deploy_row<'a>(deploy: &'a Deploy, now: u64) -> Element<'a, Msg> {
    let mut meta = ago(deploy.at_ms, now);
    if !deploy.by.is_empty() {
        meta = format!("{} · {meta}", deploy.by);
    }
    let mut lines = column![
        row![
            ui::sans(deploy.version.clone(), 13.0, theme::palette().ink),
            ui::badge(status_label(&deploy.status), status_tone(&deploy.status)),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        ui::faint(meta),
    ]
    .spacing(2);
    if !deploy.detail.is_empty() {
        lines = lines.push(ui::faint(deploy.detail.clone()));
    }
    lines.into()
}

/// The add-a-secret form: a name, a secure value, and the promise that the value is not shown again.
fn secret_form(draft: &SecretDraft, busy: bool) -> Element<'_, Msg> {
    let mut body: Vec<Element<'_, Msg>> = vec![
        ui::label("Add a secret").into(),
        ui::faint("Its value goes to the vault and is never shown again. Its name is letters, digits and underscores, starting with a letter.")
            .into(),
        text_input("API_TOKEN", &draft.name)
            .on_input(Msg::SecretName)
            .padding(8)
            .style(theme::input)
            .into(),
        text_input("Value", &draft.value.0)
            .secure(true)
            .on_input(|v| Msg::SecretValue(Hidden(v)))
            .on_submit(Msg::SaveSecret)
            .padding(8)
            .style(theme::input)
            .into(),
    ];
    if let Some(e) = &draft.error {
        body.push(ui::error_line(e));
    }
    body.push(
        row![
            ui::btn_sm(
                "Save secret",
                Some("key"),
                ui::Kind::Primary,
                (!busy).then_some(Msg::SaveSecret)
            ),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseSecret)),
        ]
        .spacing(8)
        .into(),
    );
    column(body).spacing(8).into()
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
}
