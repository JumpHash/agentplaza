//! The agent page's side panels (`agent-detail.ts` side column, `agent-harness.ts`,
//! `memory.ts`): model and reasoning effort (the model picker), harness limits and loops,
//! permissions, and memory.

use iced::{Element, Task};

use crate::console::{ui, Ctx};

#[derive(Debug, Default)]
pub struct Side {
    pub cid: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SideMsg {}

/// Loads the panels' data for agent `cid`.
pub fn load(side: &mut Side, cid: u64, ctx: &Ctx) -> Task<SideMsg> {
    side.cid = cid;
    let _ = ctx;
    Task::none() // contract: SIDE
}

pub fn update(side: &mut Side, msg: SideMsg, ctx: &Ctx) -> Task<SideMsg> {
    let _ = (side, msg, ctx);
    Task::none() // contract: SIDE
}

/// The side column.
pub fn view<'a>(side: &'a Side, ctx: &'a Ctx) -> Element<'a, SideMsg> {
    let _ = (side, ctx);
    ui::card("Model", ui::faint("…").into()) // contract: SIDE
}
