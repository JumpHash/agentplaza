//! A project's Settings tab (the web's `project-settings.ts`): every setting its agents get, inherited from the plaza
//! or overridden here, with Change and Reset to plaza; and the models by role. The instructions and skills are the
//! prompt and skills bound to `project:<id>`. Each change is typed in a dialog (`ui::modal`), as the web's `modal()` does.

use iced::widget::{Space, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Background, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, roles, ui};
use crate::theme;

/// The width of a dialog (the web's `.sheet`).
const DIALOG: f32 = 560.0;
/// The widths of a table row's value, source and actions columns; the name takes the rest.
const VALUE_WIDTH: f32 = 200.0;
const FROM_WIDTH: f32 = 170.0;
const ACTIONS_WIDTH: f32 = 200.0;

/// How a setting is entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Model,
    Number,
    Choice(&'static [&'static str]),
}

/// One setting: its key, its label, what it does, and how it is entered.
struct Spec {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    input: Input,
}

const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const MODES: &[&str] = &["autonomous", "ask", "read_only"];

const SPECS: [Spec; 9] = [
    Spec {
        key: "default_model",
        label: "Model",
        hint: "The project's agents' model, for a role with none of its own here",
        input: Input::Model,
    },
    Spec {
        key: "effort",
        label: "Effort",
        hint: "The reasoning effort of their model calls",
        input: Input::Choice(EFFORTS),
    },
    Spec {
        key: "max_tokens",
        label: "Longest answer",
        hint: "Tokens per model call (0 at the plaza: the provider decides)",
        input: Input::Number,
    },
    Spec {
        key: "max_turns",
        label: "Turns per task",
        hint: "How many model turns a work item may take (1 to 500)",
        input: Input::Number,
    },
    Spec {
        key: "compress_at",
        label: "Compress at",
        hint: "The conversation size, in bytes, at which it is compressed (0: never)",
        input: Input::Number,
    },
    Spec {
        key: "keep_recent",
        label: "Turns kept",
        hint: "How many recent turns a compression keeps (1 to 50)",
        input: Input::Number,
    },
    Spec {
        key: "task_budget",
        label: "Task budget",
        hint: "What a task reserves when it names none, in the project budget’s unit",
        input: Input::Number,
    },
    Spec {
        key: "permission_mode",
        label: "Permission mode",
        hint: "For agents with no mode of their own",
        input: Input::Choice(MODES),
    },
    Spec {
        key: "review_timeout_minutes",
        label: "Review timeout",
        hint: "Minutes a finished task waits for a reviewer before you are asked",
        input: Input::Number,
    },
];

/// A change being typed: a setting (by its key) or a role's model, its dialog's title, and the value so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    key: &'static str,
    role: Option<String>,
    value: String,
    title: String,
}

#[derive(Debug, Default)]
pub struct State {
    /// `GET /v1/projects/{id}/settings/effective`; None until the first load.
    pub view: Option<Value>,
    pub error: Option<String>,
    /// A line for the last change ("Effort saved").
    pub note: Option<String>,
    pub edit: Option<Edit>,
    /// The role named for a new role's model, before its model is picked.
    pub new_role: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Value, String>),
    /// Change a setting, by its key.
    Change(&'static str),
    /// Change a role's model.
    ChangeRole(String),
    Value(String),
    Save,
    /// Reset a setting (by its key) or a role's model to the plaza's.
    Reset(&'static str),
    ResetRole(String),
    Cancel,
    /// "Set a role's model": the role's name, then Next.
    NewRole,
    RoleName(String),
    RoleNext,
    /// A change went through (the line to show) or failed.
    Saved(Result<String, String>),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn load(project: &str, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = format!("/v1/projects/{project}/settings/effective");
    client::task(move || c.get::<Value>(&path), Msg::Loaded)
}

fn spec(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

/// A value as the table shows it: a dash for none.
fn shown(v: &Value) -> String {
    match &v["value"] {
        Value::Null => "—".into(),
        Value::String(s) if s.is_empty() => "—".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The value a field holds to begin a change with: the current one, else the first choice.
fn start_value(v: &Value, input: Input) -> String {
    match (&v["value"], input) {
        (Value::Null, Input::Choice(choices)) => choices[0].to_string(),
        (Value::Null, _) => String::new(),
        (Value::String(s), _) => s.clone(),
        (other, _) => other.to_string(),
    }
}

/// The patch a typed value makes, or why it cannot: a number for a number, some text for the rest.
fn patch_of(edit: &Edit) -> Result<Value, String> {
    let raw = edit.value.trim();
    if raw.is_empty() {
        return Err("Give a value, or reset it to the plaza's.".into());
    }
    let value = match spec(edit.key).map(|s| s.input) {
        Some(Input::Number) => match raw.parse::<i64>() {
            Ok(n) => json!(n),
            Err(_) => return Err("A whole number.".into()),
        },
        _ => json!(raw),
    };
    Ok(match &edit.role {
        Some(role) => json!({ "role_models": { role: value } }),
        None => json!({ edit.key: value }),
    })
}

/// `project` is the project's id, `live` whether it can still be changed.
pub fn update(state: &mut State, msg: Msg, project: &str, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(v)) => {
            state.view = Some(v);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Change(key) => {
            let current = state
                .view
                .as_ref()
                .map(|v| v["settings"][key].clone())
                .unwrap_or_default();
            if let Some(s) = spec(key) {
                state.edit = Some(Edit {
                    key: s.key,
                    role: None,
                    value: start_value(&current, s.input),
                    title: s.label.to_string(),
                });
                state.new_role = None;
            }
            Task::none()
        }
        Msg::ChangeRole(role) => {
            let current = state
                .view
                .as_ref()
                .map(|v| v["role_models"][&role].clone())
                .unwrap_or_default();
            state.edit = Some(Edit {
                key: "default_model",
                value: start_value(&current, Input::Model),
                title: format!("{role}’s model"),
                role: Some(role),
            });
            state.new_role = None;
            Task::none()
        }
        Msg::Value(v) => {
            if let Some(edit) = state.edit.as_mut() {
                edit.value = v;
            }
            Task::none()
        }
        Msg::Save => {
            let Some(edit) = state.edit.clone() else {
                return Task::none();
            };
            match patch_of(&edit) {
                Ok(change) => {
                    let what = match &edit.role {
                        Some(role) => format!("{}'s model saved", roles::label(role)),
                        None => format!("{} saved", spec(edit.key).map_or(edit.key, |s| s.label)),
                    };
                    state.edit = None;
                    save(project, change, what, ctx)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::Reset(key) => {
            let what = format!(
                "{} is the plaza's again",
                spec(key).map_or(key, |s| s.label)
            );
            save(project, json!({ key: null }), what, ctx)
        }
        Msg::ResetRole(role) => {
            let what = format!("{}'s model is the plaza's again", roles::label(&role));
            save(project, json!({ "role_models": { role: null } }), what, ctx)
        }
        Msg::Cancel => {
            state.edit = None;
            state.new_role = None;
            Task::none()
        }
        Msg::NewRole => {
            state.new_role = Some(String::new());
            state.edit = None;
            Task::none()
        }
        Msg::RoleName(name) => {
            if let Some(r) = state.new_role.as_mut() {
                *r = name;
            }
            Task::none()
        }
        Msg::RoleNext => {
            let role = state.new_role.take().unwrap_or_default().trim().to_string();
            if role.is_empty() {
                return Task::none();
            }
            state.edit = Some(Edit {
                key: "default_model",
                title: format!("{role}’s model"),
                role: Some(role),
                value: String::new(),
            });
            Task::none()
        }
        Msg::Saved(res) => {
            match res {
                Ok(line) => {
                    state.note = Some(line);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(project, ctx)
        }
    }
}

fn save(project: &str, change: Value, what: String, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = format!("/v1/projects/{project}/settings");
    client::task(
        move || c.patch::<Value>(&path, &change).map(|_| what),
        Msg::Saved,
    )
}

/// Where a value comes from, as a badge: overridden here, or inherited from the plaza.
fn source_badge<'a>(v: &Value) -> Element<'a, Msg> {
    if v["source"].as_str() == Some("project") {
        ui::badge("Overridden", ui::Tone::Signal)
    } else {
        ui::badge("Inherited from the plaza", ui::Tone::Muted)
    }
}

/// A hairline across, in `color` (the table's rules).
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// A table row's cells, as the web's `.tbl` lays them: the name, the value, its source, and the actions at the right.
fn cells<'a>(
    name: Element<'a, Msg>,
    value: Element<'a, Msg>,
    from: Element<'a, Msg>,
    actions: Element<'a, Msg>,
) -> Element<'a, Msg> {
    row![
        container(name).width(Length::Fill),
        container(value).width(Length::Fixed(VALUE_WIDTH)),
        container(from).width(Length::Fixed(FROM_WIDTH)),
        container(actions)
            .width(Length::Fixed(ACTIONS_WIDTH))
            .align_x(Alignment::End),
    ]
    .spacing(24)
    .align_y(Alignment::Center)
    .into()
}

/// The table's head: its labels in the label voice, the rule under it strong.
fn table_head<'a>() -> Element<'a, Msg> {
    column![
        container(cells(
            ui::label("Setting").into(),
            ui::label("Value").into(),
            ui::label("From").into(),
            text("").into(),
        ))
        .padding([8, 0]),
        rule(theme::palette().rule_strong),
    ]
    .into()
}

/// A table of rows, each under a rule but the last (the web's `.tbl`), with an optional head.
fn table<'a>(head: Option<Element<'a, Msg>>, rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(head) = head {
        lines.push(head);
    }
    let last = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        lines.push(r);
        if i + 1 < last {
            lines.push(rule(theme::palette().rule));
        }
    }
    column(lines).into()
}

/// A table row: the name (with its hint, if it has one), the value in mono, where it comes from, and the actions.
fn table_row<'a>(
    name: Element<'a, Msg>,
    value: String,
    v: &Value,
    actions: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    container(cells(
        name,
        ui::mono(value, 12.5, theme::palette().ink).into(),
        source_badge(v),
        row(actions).spacing(8).align_y(Alignment::Center).into(),
    ))
    .padding([9, 0])
    .into()
}

/// A row's name: its label in semibold, its hint under it in small ink-3.
fn named<'a>(label: &str, hint: Option<&str>) -> Element<'a, Msg> {
    let mut name = column![
        text(label.to_string())
            .font(ui::SANS_SEMIBOLD)
            .size(13.5)
            .color(theme::palette().ink)
    ]
    .spacing(4);
    if let Some(hint) = hint {
        name = name.push(ui::faint(hint.to_string()).size(12.0));
    }
    name.into()
}

/// A dialog's field: its label in the label voice, the control, and its hint under it.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    let mut col = column![ui::label(label), control].spacing(6);
    if !hint.is_empty() {
        col = col.push(ui::faint(hint.to_string()));
    }
    col.into()
}

/// The dialog open over the tab, if any (the web's `modal()`): a change being typed or a role's name.
fn dialog<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let error: Option<Element<'a, Msg>> = state.error.as_deref().map(ui::error_line);
    if let Some(edit) = &state.edit {
        let s = spec(edit.key).unwrap_or(&SPECS[0]);
        let (label, control): (&str, Element<'a, Msg>) = match s.input {
            Input::Choice(choices) => (
                if edit.role.is_some() {
                    "Model"
                } else {
                    s.label
                },
                pick_list(
                    choices.to_vec(),
                    choices.iter().copied().find(|c| *c == edit.value),
                    |c: &'static str| Msg::Value(c.to_string()),
                )
                .width(Length::Fill)
                .padding(8)
                .style(theme::pick_list)
                .into(),
            ),
            Input::Model | Input::Number => {
                let placeholder = if s.input == Input::Model {
                    "provider/model, e.g. anthropic/claude-sonnet-5-5"
                } else {
                    ""
                };
                (
                    if edit.role.is_some() {
                        "Model"
                    } else {
                        s.label
                    },
                    text_input(placeholder, &edit.value)
                        .on_input(Msg::Value)
                        .on_submit(Msg::Save)
                        .padding(8)
                        .style(theme::input)
                        .into(),
                )
            }
        };
        let mut body: Vec<Element<'a, Msg>> = vec![field(label, control, s.hint)];
        body.extend(error);
        return Some(ui::modal(
            &edit.title,
            column(body).spacing(12).into(),
            vec![ui::btn("Save", None, ui::Kind::Primary, Some(Msg::Save))],
            Msg::Cancel,
            DIALOG,
        ));
    }
    if let Some(name) = &state.new_role {
        let role: Element<'a, Msg> = text_input("builder", name)
            .on_input(Msg::RoleName)
            .on_submit(Msg::RoleNext)
            .padding(8)
            .style(theme::input)
            .into();
        let mut body: Vec<Element<'a, Msg>> = vec![field(
            "Role",
            role,
            "Its agents on this project run the model you pick next",
        )];
        body.extend(error);
        return Some(ui::modal(
            "A role’s model",
            column(body).spacing(12).into(),
            vec![ui::btn(
                "Next",
                None,
                ui::Kind::Primary,
                Some(Msg::RoleNext),
            )],
            Msg::Cancel,
            DIALOG,
        ));
    }
    None
}

/// The tab for the project `project` (`live`: it can still be changed).
pub fn view<'a>(state: &'a State, project: &'a str, live: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    let Some(view) = &state.view else {
        parts.push(ui::loading());
        return column(parts).spacing(20).into();
    };
    // The settings.
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for s in &SPECS {
        let v = &view["settings"][s.key];
        let mut actions: Vec<Element<'a, Msg>> = Vec::new();
        if live {
            actions.push(ui::btn_sm(
                "Change",
                None,
                ui::Kind::Ghost,
                Some(Msg::Change(s.key)),
            ));
            if v["source"].as_str() == Some("project") {
                actions.push(ui::btn_sm(
                    "Reset to plaza",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Reset(s.key)),
                ));
            }
        }
        rows.push(table_row(
            named(s.label, Some(s.hint)),
            shown(v),
            v,
            actions,
        ));
    }
    parts.push(ui::card("Settings", table(Some(table_head()), rows)));
    // The models by role.
    let mut role_rows: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(models) = view["role_models"].as_object().filter(|m| !m.is_empty()) {
        for (role, v) in models {
            let mut actions: Vec<Element<'a, Msg>> = Vec::new();
            if live {
                actions.push(ui::btn_sm(
                    "Change",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::ChangeRole(role.clone())),
                ));
                if v["source"].as_str() == Some("project") {
                    actions.push(ui::btn_sm(
                        "Reset to plaza",
                        None,
                        ui::Kind::Ghost,
                        Some(Msg::ResetRole(role.clone())),
                    ));
                }
            }
            role_rows.push(table_row(named(role, None), shown(v), v, actions));
        }
    }
    let role_body: Element<'a, Msg> = if role_rows.is_empty() {
        ui::faint(
            "No role has a model of its own here or at the plaza: every role runs the model above.",
        )
        .into()
    } else {
        table(None, role_rows)
    };
    let role_actions = if live {
        vec![ui::btn_sm(
            "Set a role’s model",
            Some("plus"),
            ui::Kind::Secondary,
            Some(Msg::NewRole),
        )]
    } else {
        Vec::new()
    };
    parts.push(ui::card_with("Models by role", role_actions, role_body));
    // Where its instructions and skills bind.
    parts.push(ui::card(
        "Instructions and skills",
        row![
            ui::prose("An AGENTS.md prompt bound to "),
            ui::mono(format!("project:{project}"), 12.5, theme::palette().ink),
            ui::prose(" is added to this project’s agents’ instructions, and skills bound to it are theirs too. Bind them in "),
            ui::text_link("Prompts", Msg::Go(Id::Prompts)),
            ui::prose(" and "),
            ui::text_link("Skills", Msg::Go(Id::Skills)),
            ui::prose("."),
        ]
        .wrap()
        .into(),
    ));
    ui::with_modal(column(parts).spacing(20).into(), dialog(state))
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
    fn a_change_patches_the_setting_or_the_roles_model() {
        let number = Edit {
            key: "max_turns",
            role: None,
            value: " 30 ".into(),
            title: String::new(),
        };
        assert_eq!(patch_of(&number), Ok(json!({ "max_turns": 30 })));
        let words = Edit {
            value: "thirty".into(),
            ..number.clone()
        };
        assert_eq!(patch_of(&words), Err("A whole number.".into()));
        let role = Edit {
            key: "default_model",
            role: Some("builder".into()),
            value: "anthropic/claude-sonnet-5-5".into(),
            title: String::new(),
        };
        assert_eq!(
            patch_of(&role),
            Ok(json!({ "role_models": { "builder": "anthropic/claude-sonnet-5-5" } }))
        );
        let empty = Edit {
            value: "  ".into(),
            ..role
        };
        assert!(patch_of(&empty).is_err());
    }

    #[test]
    fn a_change_starts_from_the_value_in_force() {
        let mut state = State {
            view: Some(json!({
                "settings": { "effort": { "value": null, "source": "plaza" }, "max_turns": { "value": 40, "source": "project" } },
                "role_models": {},
            })),
            ..Default::default()
        };
        let _ = update(&mut state, Msg::Change("effort"), "pr2", &ctx());
        assert_eq!(state.edit.as_ref().map(|e| e.value.as_str()), Some("none"));
        let _ = update(&mut state, Msg::Change("max_turns"), "pr2", &ctx());
        assert_eq!(state.edit.as_ref().map(|e| e.value.as_str()), Some("40"));
    }
}
