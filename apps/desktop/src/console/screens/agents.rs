//! The Agents screen (parity with the web console's `agents` view).

use iced::Element;
use iced::Task;

use super::Id;
use crate::console::{ui, Ctx};

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, _ctx: &Ctx) -> Task<Msg> {
    Task::none() // contract: W-agents
}

pub fn update(_state: &mut State, _msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    Task::none() // contract: W-agents
}

pub fn view<'a>(_state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    // contract: W-agents
    ui::sheet(ui::view_head("Agents", vec![]), ui::empty("agents", "Not ported yet", "This screen is being brought to parity with the web console."))
}
