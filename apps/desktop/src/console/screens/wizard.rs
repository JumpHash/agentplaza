//! The new-agent wizard (`agent-wizard.ts`): define a role (or pick one), its model and effort,
//! its tools and its prompt, then spawn — a modal over the Agents screen.

use iced::{Element, Task};

use crate::console::{ui, Ctx};

#[derive(Debug, Default)]
pub struct Wizard {
    pub open: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WizardMsg {
    Open,
    Close,
    /// Spawned (or failed): the Agents screen reloads.
    Done(Result<(), String>),
}

pub fn update(wizard: &mut Wizard, msg: WizardMsg, ctx: &Ctx) -> Task<WizardMsg> {
    let _ = ctx;
    match msg {
        WizardMsg::Open => wizard.open = true,
        WizardMsg::Close | WizardMsg::Done(_) => wizard.open = false,
    }
    Task::none() // contract: WIZ
}

/// The modal (the caller stacks it over its screen while `open`).
pub fn view<'a>(wizard: &'a Wizard, ctx: &'a Ctx) -> Element<'a, WizardMsg> {
    let _ = (wizard, ctx);
    ui::card("New agent", ui::faint("…").into()) // contract: WIZ
}
