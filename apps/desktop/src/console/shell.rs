//! The shell around every screen: the sidebar (brand, sections, an icon + label per screen, counts;
//! resizable, collapsible to a rail, hideable), the top bar (sidebar toggle, the screen title, hub
//! health, STOP ALL / RESUME) and the held banner — as on the web console.

use iced::widget::{column, container, row, text};
use iced::{Element, Length, Subscription, Task};

use super::screens::Id;
use super::{ui, Ctx};

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
    pub counts: std::collections::BTreeMap<&'static str, u32>,
    pub hub_ok: bool,
}

impl Default for State {
    fn default() -> Self {
        State { current: Id::Overview, mode: Mode::Open, width: 248.0, paused: false, held_note: String::new(), counts: Default::default(), hub_ok: false }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    ToggleSidebar,
    Resize(f32),
    StopAll,
    Resume,
    /// `GET /v1/plaza/state` + counts answered.
    Loaded(Result<serde_json::Value, String>),
    Done(Result<serde_json::Value, String>),
}

/// The shell's own data: the plaza's held state and the sidebar counts.
pub fn load(_ctx: &Ctx) -> Task<Msg> {
    Task::none() // contract: D3
}

/// Returns a task and, when the user picked a screen, where to go.
pub fn update(state: &mut State, msg: Msg, _ctx: &Ctx) -> (Task<Msg>, Option<Id>) {
    match msg {
        Msg::Go(id) => {
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        _ => (Task::none(), None), // contract: D3
    }
}

/// The shell around `screen`; the shell's own messages are lifted into the app's with `wrap`.
pub fn view<'a, M: Clone + 'a>(state: &'a State, screen: Element<'a, M>, wrap: fn(Msg) -> M) -> Element<'a, M> {
    // contract: D3 — sidebar + top bar + banner around `screen`. Until then: the screen alone,
    // under a one-line title.
    let _ = (state, wrap);
    container(column![text("AgentPlaza").font(crate::theme::MONO).size(13), screen].spacing(12))
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::paper)
        .into()
}

pub fn subscription(_state: &State) -> Subscription<Msg> {
    Subscription::none()
}

#[allow(dead_code)]
fn _row_keepalive<'a, M: 'a>() -> iced::widget::Row<'a, M> {
    row![]
}
