//! The shell around every screen: the sidebar (brand, sections, an icon + label per screen, counts;
//! resizable, collapsible to a rail, hideable), the top bar (sidebar toggle, the screen title, hub
//! health, STOP ALL / RESUME) and the held banner — as on the web console.

use std::collections::BTreeMap;

use iced::widget::{button, column, container, row, scrollable, tooltip, Space};
use iced::{Alignment, Border, Element, Length, Padding, Subscription, Task, Theme};

use super::screens::{Id, NAV};
use super::{ui, Ctx};
use crate::theme::{CRIT, HOVER, OK};

/// The sidebar's display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Open,
    Rail,
    Hidden,
}

#[derive(Debug)]
pub struct State {
    pub current: Id,
    pub mode: Mode,
    /// The open sidebar's width (200–420).
    pub width: f32,
    /// The plaza's held state (STOP ALL), as the hub reports it.
    pub paused: bool,
    pub held_note: String,
    /// Counts per screen key (agents, approvals, …).
    pub counts: BTreeMap<&'static str, u32>,
    pub hub_ok: bool,
    /// The hub's address, shown faint in the sidebar footer.
    pub hub: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            current: Id::Overview,
            mode: Mode::Open,
            width: 248.0,
            paused: false,
            held_note: String::new(),
            counts: Default::default(),
            hub_ok: false,
            hub: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    ToggleSidebar,
    /// Width change in px (negative narrows); clamped to 200–420.
    Resize(f32),
    StopAll,
    Resume,
    /// `GET /v1/overview` + `GET /v1/plaza/state` answered.
    Loaded(Result<serde_json::Value, String>),
    /// `POST /v1/plaza/stop` or `/v1/plaza/resume` answered.
    Done(Result<serde_json::Value, String>),
}

/// The shell's own data: the plaza's held state and the sidebar counts.
pub fn load(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    super::client::task(
        move || {
            let overview: serde_json::Value = client.get("/v1/overview")?;
            let control: serde_json::Value = client.get("/v1/plaza/state")?;
            Ok(serde_json::json!({ "overview": overview, "control": control }))
        },
        Msg::Loaded,
    )
}

fn post(ctx: &Ctx, path: &'static str, body: serde_json::Value) -> Task<Msg> {
    let client = ctx.client.clone();
    super::client::task(move || client.post::<serde_json::Value>(path, &body), Msg::Done)
}

/// Returns a task and, when the user picked a screen, where to go.
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> (Task<Msg>, Option<Id>) {
    match msg {
        Msg::Go(id) => {
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        Msg::ToggleSidebar => {
            state.mode = match state.mode {
                Mode::Open => Mode::Rail,
                Mode::Rail => Mode::Hidden,
                Mode::Hidden => Mode::Open,
            };
            (Task::none(), None)
        }
        Msg::Resize(delta) => {
            state.width = (state.width + delta).clamp(200.0, 420.0);
            (Task::none(), None)
        }
        Msg::StopAll => (post(ctx, "/v1/plaza/stop", serde_json::json!({ "reason": "stop all (desktop)" })), None),
        Msg::Resume => (post(ctx, "/v1/plaza/resume", serde_json::json!({})), None),
        Msg::Done(Ok(_)) => (load(ctx), None),
        Msg::Done(Err(_)) => (Task::none(), None),
        Msg::Loaded(Ok(v)) => {
            state.hub_ok = v["overview"]["hub"].as_bool().unwrap_or(false);
            state.counts.insert("agents", v["overview"]["agents"]["total"].as_u64().unwrap_or(0) as u32);
            state.paused = v["control"]["paused"].as_bool().unwrap_or(false);
            state.held_note = v["control"]["reason"].as_str().unwrap_or("").to_string();
            state.hub = ctx.client.base().to_string();
            (Task::none(), None)
        }
        Msg::Loaded(Err(_)) => {
            state.hub_ok = false;
            (Task::none(), None)
        }
    }
}

/// The shell around `screen`; the shell's own messages are lifted into the app's with `wrap`.
pub fn view<'a, M: Clone + 'a>(state: &'a State, screen: Element<'a, M>, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let mut main = column![top_bar(state, wrap)];
    if state.paused {
        main = main.push(held_banner(state));
    }
    main = main.push(container(screen).padding(16).width(Length::Fill).height(Length::Fill));
    let main = container(main).width(Length::Fill).height(Length::Fill).style(ui::paper);
    match state.mode {
        Mode::Hidden => main.into(),
        _ => row![sidebar(state, wrap), main].height(Length::Fill).into(),
    }
}

pub fn subscription(_state: &State) -> Subscription<Msg> {
    Subscription::none()
}

fn sidebar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let open = state.mode == Mode::Open;
    let mut list = column![].spacing(2);
    let mut section: Option<&str> = None;
    for (id, label, sec, icon_name) in NAV.iter() {
        if section != Some(*sec) {
            section = Some(*sec);
            if open {
                list = list.push(container(ui::label(*sec)).padding(Padding { top: 14.0, right: 12.0, bottom: 4.0, left: 12.0 }));
            }
        }
        list = list.push(nav_item(state, id, label, icon_name, open, wrap));
    }
    let mut col = column![brand(open), scrollable(list).height(Length::Fill)].height(Length::Fill);
    if open {
        col = col.push(footer(state, wrap));
    }
    let width = if open { state.width } else { 60.0 };
    container(col)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(ui::SHEET.into()),
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

fn brand<'a, M: 'a>(open: bool) -> Element<'a, M> {
    let mark = ui::hive(26.0);
    let head: Element<'a, M> = if open {
        row![mark, ui::mono("AgentPlaza", 14.0, ui::INK)].spacing(10).align_y(Alignment::Center).into()
    } else {
        mark
    };
    container(head).padding(Padding { top: 14.0, right: 12.0, bottom: 10.0, left: 12.0 }).width(Length::Fill).align_x(Alignment::Center).into()
}

fn nav_item<'a, M: Clone + 'a>(state: &'a State, id: &Id, label: &str, icon_name: &str, open: bool, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let active = *id == state.current;
    let fg = if active { ui::INK } else { ui::INK_2 };
    let glyph = if active { ui::SIGNAL } else { ui::INK_3 };
    let content: Element<'a, M> = if open {
        let mut r = row![ui::icon(icon_name, 16.0, glyph), ui::mono(label.to_string(), 12.5, fg)].spacing(10).align_y(Alignment::Center);
        r = r.push(Space::new().width(Length::Fill));
        if let Some(n) = state.counts.get(label.to_lowercase().as_str()) {
            r = r.push(count_badge(*n));
        }
        r.into()
    } else {
        container(ui::icon(icon_name, 16.0, glyph)).width(Length::Fill).align_x(Alignment::Center).into()
    };
    let item = button(content)
        .width(Length::Fill)
        .height(Length::Fixed(30.0))
        .padding([0, 10])
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if active || hovered { Some(HOVER.into()) } else { None },
                text_color: fg,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(wrap(Msg::Go(id.clone())));
    let bar = container(Space::new().width(Length::Fixed(3.0)).height(Length::Fixed(30.0))).style(move |_: &Theme| container::Style {
        background: if active { Some(ui::SIGNAL.into()) } else { None },
        ..Default::default()
    });
    let line: Element<'a, M> = row![bar, item].height(Length::Fixed(30.0)).into();
    if open {
        line
    } else {
        tooltip(
            line,
            container(ui::mono(label.to_string(), 11.0, ui::INK)).padding([4, 8]).style(|_: &Theme| container::Style {
                background: Some(ui::SHEET.into()),
                border: Border { color: ui::INK, width: 1.0, radius: 0.0.into() },
                ..Default::default()
            }),
            tooltip::Position::Right,
        )
        .into()
    }
}

fn count_badge<'a, M: 'a>(n: u32) -> Element<'a, M> {
    container(ui::mono(n.to_string(), 11.0, ui::INK_3))
        .padding([1, 5])
        .style(|_: &Theme| container::Style {
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The sidebar's footer: the hub's address, and the width steps.
fn footer<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    column![
        ui::mono(state.hub.clone(), 10.5, ui::INK_3),
        row![
            Space::new().width(Length::Fill),
            ui::btn("-", None, ui::Kind::Ghost, Some(wrap(Msg::Resize(-24.0)))),
            ui::btn("+", None, ui::Kind::Ghost, Some(wrap(Msg::Resize(24.0)))),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .padding(Padding { top: 10.0, right: 12.0, bottom: 12.0, left: 12.0 })
    .into()
}

fn top_bar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let toggle = ui::btn("", Some("panel-left"), ui::Kind::Ghost, Some(wrap(Msg::ToggleSidebar)));
    let title = NAV.iter().find(|e| e.0 == state.current).map(|e| e.1).unwrap_or("Agent");
    let dot = if state.hub_ok { OK } else { CRIT };
    let hub = row![
        container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0))).style(move |_: &Theme| container::Style {
            background: Some(dot.into()),
            ..Default::default()
        }),
        ui::mono("Hub", 11.5, ui::INK_2),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let control = if state.paused {
        ui::btn("Resume", Some("play"), ui::Kind::Primary, Some(wrap(Msg::Resume)))
    } else {
        ui::btn("Stop all", Some("stop"), ui::Kind::Danger, Some(wrap(Msg::StopAll)))
    };
    container(
        row![toggle, ui::mono(title.to_uppercase(), 13.0, ui::INK), Space::new().width(Length::Fill), hub, control]
            .spacing(16)
            .align_y(Alignment::Center),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    })
    .into()
}

fn held_banner<'a, M: 'a>(state: &'a State) -> Element<'a, M> {
    container(
        column![
            ui::mono("Everything is held: no agent works until you resume.", 12.5, CRIT),
            ui::faint(state.held_note.clone()),
        ]
        .spacing(4),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(ui::SHEET.into()),
        border: Border { color: CRIT, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx { client: super::super::client::Client::new("http://127.0.0.1:1", None, None) }
    }

    #[test]
    fn toggle_cycles_the_three_modes() {
        let ctx = ctx();
        let mut s = State::default();
        let mut seen = vec![s.mode];
        for _ in 0..3 {
            let _ = update(&mut s, Msg::ToggleSidebar, &ctx);
            seen.push(s.mode);
        }
        assert_eq!(seen, vec![Mode::Open, Mode::Rail, Mode::Hidden, Mode::Open]);
    }

    #[test]
    fn resize_clamps_to_200_420() {
        let ctx = ctx();
        let mut s = State::default();
        let _ = update(&mut s, Msg::Resize(-1000.0), &ctx);
        assert_eq!(s.width, 200.0);
        let _ = update(&mut s, Msg::Resize(1000.0), &ctx);
        assert_eq!(s.width, 420.0);
    }
}
