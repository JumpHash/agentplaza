//! A project's Team tab (the web's `project-team.ts`): the agents working on it, the bench to add from, and the
//! agents that held its tasks before it had members, with one step to put them all on it. New agents can be spawned
//! for it; they join at their first hello.

use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::Id;
use super::wizard::{self, WizardMsg};
use crate::console::{Ctx, client, roles, ui};
use crate::theme;

#[derive(Debug, Default)]
pub struct State {
    /// The project's agents; None until the first load.
    pub members: Option<Vec<Value>>,
    /// The agents on no project (only while the project is live).
    pub bench: Vec<Value>,
    /// The agents holding tasks of the project from before membership, each with `tasks`.
    pub suggested: Vec<Value>,
    pub error: Option<String>,
    /// A line for the last move ("Ada joined Release 0.9").
    pub note: Option<String>,
    /// A move is in flight.
    pub busy: bool,
    /// The agent wizard, for new agents of this project.
    pub wizard: wizard::Wizard,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Value, String>),
    /// Puts agents on the project (Add, or Add all).
    Add(Vec<u64>),
    /// Sends an agent back to the bench.
    Remove(u64),
    /// A move went through (the line to show) or failed.
    Moved(Result<String, String>),
    Wizard(WizardMsg),
}

/// The navigation this tab asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Reads the project's members, and while it is `live` the bench and the agents that held its tasks before.
pub fn load(project: &str, live: bool, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let id = project.to_string();
    client::task(
        move || {
            let members: Value = c.get(&format!("/v1/projects/{id}/members"))?;
            let (bench, suggested) = if live {
                (
                    c.get::<Value>("/v1/agents?project=bench")?,
                    c.get::<Value>(&format!("/v1/projects/{id}/suggested-members"))?,
                )
            } else {
                (json!([]), json!([]))
            };
            Ok(json!({ "members": members, "bench": bench, "suggested": suggested }))
        },
        Msg::Loaded,
    )
}

/// `project` and `title` are the project's; `live` whether it still takes agents.
pub fn update(
    state: &mut State,
    msg: Msg,
    project: &str,
    title: &str,
    live: bool,
    ctx: &Ctx,
) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(v)) => {
            let list = |key: &str| v[key].as_array().cloned().unwrap_or_default();
            state.members = Some(list("members"));
            state.bench = list("bench");
            state.suggested = list("suggested");
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Add(cids) => {
            if cids.is_empty() || state.busy {
                return Task::none();
            }
            state.busy = true;
            let c = ctx.client.clone();
            let id = project.to_string();
            let line = match cids.as_slice() {
                [cid] => format!("{} joined {title}", name_of(&state.everyone(), *cid)),
                _ => format!("{} agents joined {title}", cids.len()),
            };
            client::task(
                move || {
                    for cid in cids {
                        c.post::<Value>(
                            &format!("/v1/projects/{id}/members"),
                            &json!({ "cid": cid }),
                        )?;
                    }
                    Ok(line)
                },
                Msg::Moved,
            )
        }
        Msg::Remove(cid) => {
            if state.busy {
                return Task::none();
            }
            state.busy = true;
            let c = ctx.client.clone();
            let path = format!("/v1/projects/{project}/members/{cid}");
            let line = format!("{} is back on the bench", name_of(&state.everyone(), cid));
            client::task(move || c.delete(&path).map(|_| line), Msg::Moved)
        }
        Msg::Moved(res) => {
            state.busy = false;
            match res {
                Ok(line) => {
                    state.note = Some(line);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(project, live, ctx)
        }
        Msg::Wizard(m) => {
            if let WizardMsg::Done(Err(e)) = &m {
                state.error = Some(e.clone());
            }
            let reload = matches!(m, WizardMsg::Done(Ok(())));
            let step = wizard::update(&mut state.wizard, m, ctx).map(Msg::Wizard);
            if reload {
                Task::batch([step, load(project, live, ctx)])
            } else {
                step
            }
        }
    }
}

impl State {
    /// Every agent the tab lists, for their names in the lines it shows.
    fn everyone(&self) -> Vec<&Value> {
        self.members
            .iter()
            .flatten()
            .chain(&self.bench)
            .chain(&self.suggested)
            .collect()
    }
}

/// An agent's name, or "Unnamed" while it has none.
pub fn agent_name(a: &Value) -> String {
    match a["name"].as_str().map(str::trim) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => "Unnamed".into(),
    }
}

fn name_of(list: &[&Value], cid: u64) -> String {
    list.iter()
        .find(|a| a["cid"].as_u64() == Some(cid))
        .map(|a| agent_name(a))
        .unwrap_or_else(|| format!("Agent {cid}"))
}

/// The tab, for the project `project` (`live`: it still takes agents).
pub fn view<'a>(state: &'a State, project: &str, live: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    let Some(members) = &state.members else {
        parts.push(ui::loading());
        return column(parts).spacing(16).into();
    };
    let busy = state.busy;
    // Working on it: each with Back to the bench, and why it waits if it does.
    let mut working: Vec<Element<'a, Msg>> = Vec::new();
    if live {
        working.push(
            row![ui::btn_sm(
                "New agents for this project",
                Some("plus"),
                ui::Kind::Secondary,
                Some(Msg::Wizard(WizardMsg::OpenFor(project.to_string())))
            )]
            .into(),
        );
    }
    if members.is_empty() {
        working.push(
            ui::faint(if live {
                "Nobody yet: add agents from the bench, or spawn new ones for this project."
            } else {
                "Nobody: the project is over, and its agents went back to the bench."
            })
            .into(),
        );
    }
    for a in members {
        let cid = a["cid"].as_u64().unwrap_or(0);
        let action = live.then(|| {
            ui::btn_sm(
                "Back to the bench",
                None,
                ui::Kind::Ghost,
                (!busy).then_some(Msg::Remove(cid)),
            )
        });
        working.push(agent_row(a, action, a["held"].as_str().map(str::to_string)));
    }
    parts.push(ui::card(
        &format!("Working on it ({})", members.len()),
        column(working).spacing(10).into(),
    ));
    // The agents that held its tasks before membership, with Add all.
    if !state.suggested.is_empty() {
        let all: Vec<u64> = state
            .suggested
            .iter()
            .filter_map(|a| a["cid"].as_u64())
            .collect();
        let label = if all.len() > 1 {
            format!("Add all {}", all.len())
        } else {
            "Add".to_string()
        };
        let mut lines: Vec<Element<'a, Msg>> = vec![
            ui::faint("These agents hold tasks of this project from before agents worked on one project at a time. Put them on it, and its tasks run again.").into(),
            row![ui::btn_sm(
                &label,
                Some("plus"),
                ui::Kind::Primary,
                (!busy).then_some(Msg::Add(all))
            )]
            .into(),
        ];
        for a in &state.suggested {
            let cid = a["cid"].as_u64().unwrap_or(0);
            let holds = a["tasks"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            lines.push(agent_row(
                a,
                Some(ui::btn_sm(
                    "Add",
                    Some("plus"),
                    ui::Kind::Secondary,
                    (!busy).then_some(Msg::Add(vec![cid])),
                )),
                Some(format!("holds {holds}")),
            ));
        }
        parts.push(ui::card(
            "Worked on its tasks before",
            column(lines).spacing(10).into(),
        ));
    }
    if live {
        let mut lines: Vec<Element<'a, Msg>> = Vec::new();
        if state.bench.is_empty() {
            lines.push(ui::faint("Nobody is on the bench.").into());
        } else {
            lines.push(ui::faint("Agents on no project do nothing, and sleep. Adding one stops nothing: it has no work.").into());
            for a in &state.bench {
                let cid = a["cid"].as_u64().unwrap_or(0);
                lines.push(agent_row(
                    a,
                    Some(ui::btn_sm(
                        "Add",
                        Some("plus"),
                        ui::Kind::Secondary,
                        (!busy).then_some(Msg::Add(vec![cid])),
                    )),
                    None,
                ));
            }
        }
        parts.push(ui::card("The bench", column(lines).spacing(10).into()));
    }
    column(parts).spacing(16).into()
}

/// One agent's line: its initial, its name (to its page) and role, a note, its status, and the action.
pub fn agent_row<'a, M>(
    a: &Value,
    action: Option<Element<'a, M>>,
    note: Option<String>,
) -> Element<'a, M>
where
    M: Clone + From<Id> + 'a,
{
    let cid = a["cid"].as_u64().unwrap_or(0);
    let name = agent_name(a);
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let tile = container(
        text(initial)
            .font(ui::SANS_SEMIBOLD)
            .size(12.0)
            .color(theme::palette().ink),
    )
    .width(Length::Fixed(28.0))
    .height(Length::Fixed(28.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|_: &Theme| container::Style {
        background: Some(theme::palette().sheet.into()),
        border: Border {
            color: theme::palette().ink,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    });
    let role = roles::label(a["role"].as_str().unwrap_or(""));
    let sub = match note {
        Some(n) if !n.is_empty() => format!("{role} · {n}"),
        _ => role,
    };
    let status = a["status"].as_str().unwrap_or("offline");
    let tone = match status {
        "working" => ui::Tone::Info,
        "idle" => ui::Tone::Ok,
        _ => ui::Tone::Muted,
    };
    let mut line = row![
        tile,
        column![
            ui::text_link(&name, M::from(Id::Agent(cid))),
            ui::faint(sub)
        ]
        .spacing(2)
        .width(Length::Fill),
        ui::badge(status, tone),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if let Some(action) = action {
        line = line.push(action);
    }
    column![line, Space::new().height(2.0)].into()
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

    #[test]
    fn a_load_fills_the_members_the_bench_and_the_agents_from_before() {
        let mut state = State::default();
        let v = json!({
            "members": [{ "cid": 300, "name": "Ada", "role": "builder", "status": "idle" }],
            "bench": [{ "cid": 301, "name": "", "role": "qa" }],
            "suggested": [{ "cid": 302, "name": "Quinn", "tasks": ["t1", "t2"] }],
        });
        let _ = update(
            &mut state,
            Msg::Loaded(Ok(v)),
            "pr2",
            "Release 0.9",
            true,
            &ctx(),
        );
        assert_eq!(state.members.as_ref().map(Vec::len), Some(1));
        assert_eq!((state.bench.len(), state.suggested.len()), (1, 1));
        assert_eq!(name_of(&state.everyone(), 301), "Unnamed");
        assert_eq!(name_of(&state.everyone(), 302), "Quinn");
        assert_eq!(name_of(&state.everyone(), 999), "Agent 999");
    }

    #[test]
    fn a_move_waits_for_the_last_one() {
        let mut state = State {
            busy: true,
            ..Default::default()
        };
        let _ = update(
            &mut state,
            Msg::Remove(300),
            "pr2",
            "Release 0.9",
            true,
            &ctx(),
        );
        assert!(
            state.busy && state.note.is_none(),
            "nothing new starts while a move runs"
        );
        let _ = update(
            &mut state,
            Msg::Moved(Ok("Ada is back on the bench".into())),
            "pr2",
            "Release 0.9",
            true,
            &ctx(),
        );
        assert!(!state.busy);
        assert_eq!(state.note.as_deref(), Some("Ada is back on the bench"));
    }
}
