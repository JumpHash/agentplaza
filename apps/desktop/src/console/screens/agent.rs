//! One agent's page (parity with the web console's agent detail): profile, transcript and chat,
//! model + reasoning effort, harness, permissions, memory, restart / wipe / retire.

use iced::Element;
use iced::Task;

use super::Id;
use crate::console::{ui, Ctx};

#[derive(Debug, Default)]
pub struct State {
    pub cid: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
    }
}

pub fn load(_state: &mut State, _ctx: &Ctx) -> Task<Msg> {
    Task::none() // contract: W-agent
}

pub fn update(_state: &mut State, _msg: Msg, _ctx: &Ctx) -> Task<Msg> {
    Task::none() // contract: W-agent
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    ui::sheet(ui::view_head(&format!("Agent {}", state.cid), vec![]), ui::empty("agents", "Not ported yet", "This screen is being brought to parity with the web console."))
}
