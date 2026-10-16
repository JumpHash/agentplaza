//! The Volumes screen: the hub's shared read-only data volumes (`/v1/volumes`), and which roles' agents mount them
//! (`role_volumes` in the hub's settings). An agent mounts a volume at `/data/<name>`. A change applies to agents
//! spawned from now on. The settings are read whole and written whole, so no other field is lost.

use std::collections::BTreeMap;

use iced::widget::{column, row, text_input};
use iced::{Alignment, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::agent;
use super::nodes::Load;
use crate::console::{Ctx, client, ui};

/// The most volumes one role mounts.
pub const MAX_PER_ROLE: usize = 4;
/// The longest volume name.
const MAX_NAME: usize = 20;
const NAME_HINT: &str = "A volume name is 1 to 20 lowercase letters, digits or hyphens, starting with a letter or digit.";

/// A volume, as `GET /v1/volumes` lists it. `in_use` holds the CIDs of the agents that mount it now.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Volume {
    pub name: String,
    pub size_bytes: u64,
    pub created_ms: u64,
    pub source: String,
    pub in_use: Vec<u64>,
}

#[derive(Debug, Default)]
pub struct State {
    pub volumes: Load<Vec<Volume>>,
    /// The hub's settings as read, kept whole so a save sends every field back.
    pub settings: Load<Value>,
    /// The working text of each role's volume list (comma-separated), keyed by role.
    pub drafts: BTreeMap<String, String>,
    /// The new volume's name and source path.
    pub name: String,
    pub source: String,
    /// The new role's name and its volume list.
    pub new_role: String,
    pub new_role_volumes: String,
    /// A write is on its way to the hub.
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The volume waiting for its remove confirmation.
    pub confirm_remove: Option<String>,
}

/// Drops a pending remove confirmation. Called when the owner leaves the screen.
pub fn leave(state: &mut State) {
    state.confirm_remove = None;
}

#[derive(Debug, Clone)]
pub enum Msg {
    VolumesLoaded(Result<Vec<Volume>, String>),
    SettingsLoaded(Result<Value, String>),
    Name(String),
    Source(String),
    Create,
    Created(Result<Volume, String>),
    AskRemove(String),
    CancelRemove,
    Remove(String),
    /// A volume's removal finished: its result and the line to confirm it with.
    Removed(Result<(), String>, String),
    /// A role's volume list changed in the page (its working text).
    Draft(String, String),
    SaveRole(String),
    RemoveRole(String),
    NewRole(String),
    NewRoleVolumes(String),
    AddRole,
    /// A settings write finished: its result and the line to confirm it with.
    Saved(Result<(), String>, String),
}

/// Navigation: the screen links nowhere else.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

fn settle<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(_) => Load::Failed,
    }
}

// ---- Pure helpers -------------------------------------------------------------------------------------------

/// A volume name: `[a-z0-9][a-z0-9-]{0,19}`.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_NAME
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && *b == b'-'))
}

/// A role's name: letters, digits, hyphens or underscores, up to 40 characters.
pub fn valid_role(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 40
        && role
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A comma-separated list of volume names: blanks dropped, repeats kept once, each name checked, at most four.
pub fn parse_names(text: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for raw in text.split(',') {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        if !valid_name(name) {
            return Err(format!("\"{name}\" is not a volume name. {NAME_HINT}"));
        }
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    if names.len() > MAX_PER_ROLE {
        return Err(format!("A role mounts at most {MAX_PER_ROLE} volumes."));
    }
    Ok(names)
}

/// The `role_volumes` of a settings object: each role with its volume names. Missing or odd entries read as none.
pub fn role_volumes(settings: &Value) -> BTreeMap<String, Vec<String>> {
    settings
        .get("role_volumes")
        .and_then(Value::as_object)
        .map(|roles| {
            roles
                .iter()
                .map(|(role, names)| {
                    let names = names
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    (role.clone(), names)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The settings with `role` set to `names`; `None` removes the role. Every other field stays as it was.
pub fn with_role(settings: &Value, role: &str, names: Option<Vec<String>>) -> Value {
    let mut roles = role_volumes(settings);
    match names {
        Some(names) => {
            roles.insert(role.to_string(), names);
        }
        None => {
            roles.remove(role);
        }
    }
    let mut next = settings.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("role_volumes".to_string(), json!(roles));
    }
    next
}

/// The settings with `role` given the volumes `text` names. A role with none is removed.
pub fn role_settings(settings: &Value, role: &str, text: &str) -> Result<Value, String> {
    let names = parse_names(text)?;
    let names = (!names.is_empty()).then_some(names);
    Ok(with_role(settings, role, names))
}

/// A volume's size in whole MiB.
pub fn size_mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// The agents (CIDs) that mount a volume, or the dash when none does.
pub fn mounted_text(cids: &[u64]) -> String {
    if cids.is_empty() {
        "—".to_string()
    } else {
        cids.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The new volume's name and source, checked before the hub is asked.
fn check_new(name: &str, source: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(NAME_HINT.to_string());
    }
    if !source.starts_with('/') {
        return Err("The source is an absolute path on the hub's machine.".to_string());
    }
    Ok(())
}

fn drafts_of(settings: &Value) -> BTreeMap<String, String> {
    role_volumes(settings)
        .into_iter()
        .map(|(role, names)| (role, names.join(", ")))
        .collect()
}

// ---- Loading and writes -----------------------------------------------------------------------------------

fn volumes_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Vec<Volume>>("/v1/volumes"),
        Msg::VolumesLoaded,
    )
}

fn settings_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Value>("/v1/settings"),
        Msg::SettingsLoaded,
    )
}

/// Loads the volumes and the settings (called when the screen opens and after each write).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    Task::batch(vec![volumes_task(ctx), settings_task(ctx)])
}

/// The settings as loaded, or why they are not.
fn loaded_settings(state: &State) -> Result<Value, String> {
    match &state.settings {
        Load::Ready(value) => Ok(value.clone()),
        _ => Err("The settings are not loaded yet.".to_string()),
    }
}

/// One write of the whole settings: refused while another write runs. `notice` confirms it.
fn put_settings(state: &mut State, ctx: &Ctx, settings: Value, notice: String) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    state.busy = true;
    state.error = None;
    state.notice = None;
    let client = ctx.client.clone();
    client::task(
        move || {
            client
                .call("PUT", "/v1/settings", Some(&settings))
                .map(|_| ())
        },
        move |result| Msg::Saved(result, notice.clone()),
    )
}

/// Saves one role's volume list (an empty list removes the role).
fn put_role(state: &mut State, ctx: &Ctx, role: &str, text: &str) -> Task<Msg> {
    let next = loaded_settings(state).and_then(|settings| role_settings(&settings, role, text));
    match next {
        Ok(next) => put_settings(state, ctx, next, format!("Volumes for {role} are saved.")),
        Err(error) => {
            state.error = Some(error);
            Task::none()
        }
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::VolumesLoaded(result) => {
            state.volumes = settle(result);
            Task::none()
        }
        Msg::SettingsLoaded(result) => {
            if let Ok(settings) = &result {
                state.drafts = drafts_of(settings);
            }
            state.settings = settle(result);
            Task::none()
        }
        Msg::Name(text) => {
            state.name = text;
            Task::none()
        }
        Msg::Source(text) => {
            state.source = text;
            Task::none()
        }
        Msg::Create => {
            if state.busy {
                return Task::none();
            }
            let name = state.name.trim().to_string();
            let source = state.source.trim().to_string();
            if let Err(error) = check_new(&name, &source) {
                state.error = Some(error);
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            state.notice = None;
            let body = json!({ "name": name, "source": source });
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Volume>("/v1/volumes", &body),
                Msg::Created,
            )
        }
        Msg::Created(Ok(made)) => {
            state.busy = false;
            state.notice = Some(format!("Volume {} is made.", made.name));
            state.name.clear();
            state.source.clear();
            volumes_task(ctx)
        }
        Msg::Created(Err(error)) => {
            state.busy = false;
            state.error = Some(error);
            Task::none()
        }
        Msg::AskRemove(name) => {
            state.confirm_remove = Some(name);
            state.error = None;
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirm_remove = None;
            Task::none()
        }
        Msg::Remove(name) => {
            if state.busy {
                return Task::none();
            }
            if !valid_name(&name) {
                state.error = Some(NAME_HINT.to_string());
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            state.notice = None;
            state.confirm_remove = None;
            let notice = format!("Volume {name} is removed.");
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("DELETE", &format!("/v1/volumes/{name}"), None)
                        .map(|_| ())
                },
                move |result| Msg::Removed(result, notice.clone()),
            )
        }
        Msg::Removed(result, notice) => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.notice = Some(notice);
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
            volumes_task(ctx)
        }
        Msg::Draft(role, text) => {
            state.drafts.insert(role, text);
            Task::none()
        }
        Msg::SaveRole(role) => {
            let text = state.drafts.get(&role).cloned().unwrap_or_default();
            put_role(state, ctx, &role, &text)
        }
        Msg::RemoveRole(role) => {
            let next = loaded_settings(state).map(|settings| with_role(&settings, &role, None));
            match next {
                Ok(next) => put_settings(state, ctx, next, format!("Role {role} is removed.")),
                Err(error) => {
                    state.error = Some(error);
                    Task::none()
                }
            }
        }
        Msg::NewRole(text) => {
            state.new_role = text;
            Task::none()
        }
        Msg::NewRoleVolumes(text) => {
            state.new_role_volumes = text;
            Task::none()
        }
        Msg::AddRole => {
            if state.busy {
                return Task::none();
            }
            let role = state.new_role.trim().to_string();
            if !valid_role(&role) {
                state.error = Some(
                    "A role is a name of letters, digits, hyphens or underscores, up to 40 characters."
                        .to_string(),
                );
                return Task::none();
            }
            if state.drafts.contains_key(&role) {
                state.error = Some(format!("{role} already has a volume list."));
                return Task::none();
            }
            let text = state.new_role_volumes.clone();
            let task = put_role(state, ctx, &role, &text);
            if state.error.is_none() {
                state.new_role.clear();
                state.new_role_volumes.clear();
            }
            task
        }
        Msg::Saved(result, notice) => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.notice = Some(notice);
                    state.error = None;
                    // The saved settings are read again, so the page shows what the hub kept.
                    settings_task(ctx)
                }
                Err(error) => {
                    state.error = Some(error);
                    Task::none()
                }
            }
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

/// The button's message, unless a write is running.
fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

/// The screen.
pub fn view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = vec![
        ui::faint("Volumes are read-only images that a role's agents mount at /data/<name>. A change applies to agents spawned from now on.")
            .into(),
    ];
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    parts.push(ui::card("New volume", create_view(state)));
    parts.push(ui::card("Volumes", volumes_view(state)));
    parts.push(ui::card("Roles", roles_view(state)));
    let head = ui::view_head("Volumes", vec![]);
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

fn create_view(state: &State) -> Element<'_, Msg> {
    let form = row![
        text_input("corpus", &state.name)
            .on_input(Msg::Name)
            .padding(8.0)
            .style(crate::theme::input)
            .width(Length::FillPortion(1)),
        text_input("/srv/volumes/corpus.ext4", &state.source)
            .on_input(Msg::Source)
            .on_submit(Msg::Create)
            .padding(8.0)
            .style(crate::theme::input)
            .width(Length::FillPortion(3)),
        ui::btn(
            "Create volume",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::Create)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        ui::faint("The name is how the volume is named here. The source is an absolute path on the hub's machine: an ext4 image, a cloned volume, or a directory, packed."),
        form,
    ]
    .spacing(14)
    .width(Length::Fill)
    .into()
}

fn volumes_view(state: &State) -> Element<'_, Msg> {
    match &state.volumes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the volumes.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "machine",
            "No volumes yet",
            "Make one above, then give it to a role below.",
        ),
        Load::Ready(list) => column(
            list.iter()
                .map(|volume| volume_row(state, volume))
                .collect::<Vec<_>>(),
        )
        .spacing(14)
        .width(Length::Fill)
        .into(),
    }
}

fn volume_row<'a>(state: &State, volume: &Volume) -> Element<'a, Msg> {
    let facts = row![
        ui::mono(volume.name.clone(), 12.0, crate::theme::palette().ink),
        ui::faint(format!("{} MiB", size_mib(volume.size_bytes))),
        ui::faint(format!("mounted by {}", mounted_text(&volume.in_use))),
        ui::faint(volume.source.clone()),
        ui::faint(format!("created {}", agent::ago(volume.created_ms))),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let action: Element<'a, Msg> = if state.confirm_remove.as_deref() == Some(volume.name.as_str())
    {
        row![
            ui::faint(format!(
                "Remove {}? Roles stop mounting it from the next agent they spawn.",
                volume.name
            )),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRemove)),
            ui::btn_sm(
                "Remove",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::Remove(volume.name.clone())),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    } else {
        // A volume that a running agent mounts is not removed: the hub refuses it, so the button is off.
        let allowed = if volume.in_use.is_empty() {
            gate(state.busy, Msg::AskRemove(volume.name.clone()))
        } else {
            None
        };
        ui::btn_sm("Remove", None, ui::Kind::Danger, allowed)
    };
    column![facts, action].spacing(8).into()
}

fn roles_view(state: &State) -> Element<'_, Msg> {
    let lines: Element<'_, Msg> = match &state.settings {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the roles.", ""),
        Load::Ready(_) if state.drafts.is_empty() => ui::empty(
            "machine",
            "No role mounts a volume",
            "Add a role below and list the volumes its agents mount.",
        ),
        Load::Ready(_) => column(
            state
                .drafts
                .iter()
                .map(|(role, text)| role_row(state, role, text))
                .collect::<Vec<_>>(),
        )
        .spacing(14)
        .width(Length::Fill)
        .into(),
    };
    let add = row![
        text_input("role", &state.new_role)
            .on_input(Msg::NewRole)
            .padding(8.0)
            .style(crate::theme::input)
            .width(Length::FillPortion(1)),
        text_input("data-1, corpus", &state.new_role_volumes)
            .on_input(Msg::NewRoleVolumes)
            .on_submit(Msg::AddRole)
            .padding(8.0)
            .style(crate::theme::input)
            .width(Length::FillPortion(2)),
        ui::btn(
            "Add role",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::AddRole)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        ui::faint("Each role lists the volumes its agents mount, at most four. A role with no volumes is removed."),
        lines,
        ui::label("Add a role"),
        add,
    ]
    .spacing(14)
    .width(Length::Fill)
    .into()
}

/// One role: its name, its volume list (edited in place), and Save and Remove.
fn role_row<'a>(state: &State, role: &'a str, text: &'a str) -> Element<'a, Msg> {
    column![
        ui::mono(role.to_string(), 12.0, crate::theme::palette().ink),
        row![
            text_input("data-1, corpus", text)
                .on_input(move |value| Msg::Draft(role.to_string(), value))
                .padding(8.0)
                .style(crate::theme::input)
                .width(Length::Fill),
            ui::btn_sm(
                "Save",
                None,
                ui::Kind::Secondary,
                gate(state.busy, Msg::SaveRole(role.to_string()))
            ),
            ui::btn_sm(
                "Remove",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::RemoveRole(role.to_string()))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_a_role_takes_at_most_four() {
        assert_eq!(parse_names("a, b,,c ").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(parse_names("a,a").unwrap(), vec!["a"]);
        assert_eq!(parse_names("  ").unwrap(), Vec::<String>::new());
        assert!(parse_names("a,b,c,d,e").is_err());
        assert!(parse_names("a,Bad").is_err());
    }

    #[test]
    fn volumes_settings_keep_the_other_fields_and_roles() {
        let settings = json!({"idle_sleep_min": 10, "doze_idle_s": 60, "role_volumes": {"dev": ["x"], "ops": ["y"]}});
        let set = role_settings(&settings, "dev", "z, w").unwrap();
        assert_eq!(set["idle_sleep_min"], json!(10));
        assert_eq!(set["doze_idle_s"], json!(60));
        assert_eq!(
            set["role_volumes"],
            json!({"dev": ["z", "w"], "ops": ["y"]})
        );
        let cleared = role_settings(&set, "ops", " ").unwrap();
        assert_eq!(cleared["role_volumes"], json!({"dev": ["z", "w"]}));
        assert!(role_settings(&settings, "dev", "Bad").is_err());
    }
}
