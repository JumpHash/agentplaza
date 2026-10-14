//! Setup: the checks behind this console (the hub, the node, the model keys, and this machine's
//! requirements from the first-run checks), and the guided setup and the tour again.

use iced::widget::{column, row, Space};
use iced::{Alignment, Element, Length, Task};

use super::Id;
use crate::console::onboard::{self, Status};
use crate::console::setup::Check;
use crate::console::ui::{self, Kind};
use crate::console::Ctx;

#[derive(Debug, Default)]
pub struct State {
    pub status: Option<Status>,
    /// This machine's requirements; read once on the first open, then on "Check again".
    pub local: Option<Vec<Check>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Check,
    Status(Status),
    Local(Vec<Check>),
    /// Run the setup wizard again (the console opens it).
    Wizard,
    /// Replay the tour (the console starts it).
    Tour,
}

pub fn go(_: &Msg) -> Option<Id> {
    None
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let status = onboard::status_task(ctx).map(Msg::Status);
    if state.local.is_none() {
        Task::batch([status, onboard::local_task().map(Msg::Local)])
    } else {
        status
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Check => Task::batch([onboard::status_task(ctx).map(Msg::Status), onboard::local_task().map(Msg::Local)]),
        Msg::Status(status) => {
            state.status = Some(status);
            Task::none()
        }
        Msg::Local(checks) => {
            state.local = Some(checks);
            Task::none()
        }
        // The console answers these (it owns the wizard and the tour).
        Msg::Wizard | Msg::Tour => Task::none(),
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(
        onboard::copy_text("setup.title"),
        vec![ui::btn(onboard::copy_text("setup.check_again"), None, Kind::Ghost, Some(Msg::Check))],
    );
    let intro = ui::faint(onboard::copy_text("setup.intro"));
    let rows = onboard::rows(state.status.as_ref(), state.local.as_deref()).into_iter().map(|r| {
        row![ui::mono(r.label, 12.0, ui::INK_2), Space::new().width(Length::Fill), ui::badge(&r.detail, r.tone)]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    });
    let checks = column(rows).spacing(10);
    let guided = row![
        ui::btn(onboard::copy_text("replay.setup"), None, Kind::Primary, Some(Msg::Wizard)),
        ui::btn(onboard::copy_text("replay.tour"), None, Kind::Secondary, Some(Msg::Tour)),
    ]
    .spacing(8);
    ui::sheet(head, column![intro, ui::card(onboard::copy_text("wizard.steps.3.title"), checks.into()), guided].spacing(16).into())
}
