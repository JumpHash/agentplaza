//! A project's Team tab (the web's `project-team.ts`): the agents working on it, the bench to add from, and the
//! agents that held its tasks before it had members, with one step to put them all on it. New agents can be spawned
//! for it; they join at their first hello.

use iced::widget::{Space, button, column, container, row, stack, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Task, Theme};
use serde_json::{Value, json};

use super::Id;
use super::wizard::{self, WizardMsg};
use crate::console::engine::robot::species_of;
use crate::console::engine::roles::{agent_hue, role_hue};
use crate::console::{Ctx, client, render_card, roles, ui};
use crate::theme;

/// An agent's portrait, as the web's `avatar(a, 28)`.
const AVATAR: f32 = 28.0;

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

/// A capitalised word (the web's `cap`).
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A hairline across, in `color`.
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// An agent's name, bold, to its page: ink, signal-ink under the pointer (the web's `.pj-clip` link).
fn name_link<'a>(name: String, cid: u64) -> Element<'a, Msg> {
    button(text(name).font(ui::SANS_SEMIBOLD).size(13.5))
        .padding(0)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                theme::palette().signal_ink
            } else {
                theme::palette().ink
            },
            border: Border::default(),
            ..Default::default()
        })
        .on_press(Msg::Go(Id::Agent(cid)))
        .into()
}

/// The agent's portrait: its robot over the letter tile, which shows where the GPU cannot draw.
fn avatar<'a>(a: &Value, slot: usize) -> Element<'a, Msg> {
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
            .color(theme::palette().ink_2),
    )
    .width(Length::Fixed(AVATAR))
    .height(Length::Fixed(AVATAR))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|_: &Theme| container::Style {
        background: Some(Background::Color(theme::palette().sheet_2)),
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: (AVATAR / 2.0).into(),
        },
        ..Default::default()
    });
    let orchestrator = a["tier"].as_str() == Some("orchestrator");
    let status = a["status"].as_str().unwrap_or("offline");
    let spec = render_card::PortraitSpec {
        cid,
        orchestrator,
        role_hue: if orchestrator {
            25.0
        } else {
            role_hue(a["role"].as_str().unwrap_or(""))
        },
        body_hue: agent_hue(cid),
        offline: matches!(status, "offline" | "asleep" | "dozing"),
        species: species_of(a["species"].as_str(), cid),
    };
    let mut layers: Vec<Element<'a, Msg>> = vec![tile.into()];
    if let Some(robot) = render_card::robot_portrait(slot, spec, AVATAR) {
        layers.push(robot);
    }
    stack(layers)
        .width(Length::Fixed(AVATAR))
        .height(Length::Fixed(AVATAR))
        .into()
}

/// The status chip (the web's `badge(word, tone, true)`): asleep and dozing say so, the rest their status word.
fn status_chip<'a>(status: &str) -> Element<'a, Msg> {
    let (word, tone) = match status {
        "working" => ("Working".to_string(), ui::Tone::Info),
        "idle" => ("Idle".to_string(), ui::Tone::Ok),
        "asleep" => ("Asleep".to_string(), ui::Tone::Muted),
        "dozing" => ("Asleep (light)".to_string(), ui::Tone::Muted),
        other => (cap(other), ui::Tone::Muted),
    };
    ui::badge_dot(&word, tone)
}

/// One agent's line (the web's `.pj-mrow`): its portrait, its name to its page over its role and note, its status, and
/// its action; a rule above it.
fn person<'a>(
    a: &Value,
    slot: usize,
    note: Option<String>,
    action: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let cid = a["cid"].as_u64().unwrap_or(0);
    let role = roles::label(a["role"].as_str().unwrap_or(""));
    let sub = match note {
        Some(n) if !n.is_empty() => format!("{role} · {n}"),
        _ => role,
    };
    let mut line = row![
        avatar(a, slot),
        column![name_link(agent_name(a), cid), ui::faint(sub)]
            .spacing(4)
            .width(Length::Fill),
        status_chip(a["status"].as_str().unwrap_or("offline")),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if let Some(action) = action {
        line = line.push(action);
    }
    column![
        rule(theme::palette().rule),
        container(line).padding([8, 0]).width(Length::Fill),
    ]
    .into()
}

/// Puts one agent on the project.
fn add_btn<'a>(cid: u64, busy: bool) -> Element<'a, Msg> {
    ui::btn_sm(
        "Add",
        Some("plus"),
        ui::Kind::Secondary,
        (!busy).then_some(Msg::Add(vec![cid])),
    )
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
        return column(parts).spacing(20).into();
    };
    let busy = state.busy;
    // Working on it: each with Back to the bench, and why it waits if it does.
    let working: Vec<Element<'a, Msg>> = members
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let cid = a["cid"].as_u64().unwrap_or(0);
            let action = live.then(|| {
                ui::btn_sm(
                    "Back to the bench",
                    None,
                    ui::Kind::Ghost,
                    (!busy).then_some(Msg::Remove(cid)),
                )
            });
            person(a, i, a["held"].as_str().map(str::to_string), action)
        })
        .collect();
    let body: Element<'a, Msg> = if members.is_empty() {
        ui::faint(if live {
            "Nobody yet: add agents from the bench, or spawn new ones for this project."
        } else {
            "Nobody: the project is over, and its agents went back to the bench."
        })
        .into()
    } else {
        column(working).spacing(12).into()
    };
    let head_actions = if live {
        vec![ui::btn_sm(
            "New agents for this project",
            Some("plus"),
            ui::Kind::Secondary,
            Some(Msg::Wizard(WizardMsg::OpenFor(project.to_string()))),
        )]
    } else {
        Vec::new()
    };
    parts.push(ui::card_with(
        &format!("Working on it ({})", members.len()),
        head_actions,
        body,
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
        ];
        for (i, a) in state.suggested.iter().enumerate() {
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
            lines.push(person(
                a,
                i,
                Some(format!("holds {holds}")),
                Some(add_btn(cid, busy)),
            ));
        }
        let add_all = ui::btn_sm(
            &label,
            Some("plus"),
            ui::Kind::Primary,
            (!busy).then_some(Msg::Add(all)),
        );
        parts.push(ui::card_with(
            "Worked on its tasks before",
            vec![add_all],
            column(lines).spacing(12).into(),
        ));
    }
    // The bench, to add from.
    if live {
        let body: Element<'a, Msg> = if state.bench.is_empty() {
            ui::faint("Nobody is on the bench.").into()
        } else {
            let mut lines: Vec<Element<'a, Msg>> = vec![
                ui::faint("Agents on no project do nothing, and sleep. Adding one stops nothing: it has no work.").into(),
            ];
            for (i, a) in state.bench.iter().enumerate() {
                let cid = a["cid"].as_u64().unwrap_or(0);
                lines.push(person(a, i, None, Some(add_btn(cid, busy))));
            }
            column(lines).spacing(12).into()
        };
        parts.push(ui::card("The bench", body));
    }
    column(parts).spacing(20).into()
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
