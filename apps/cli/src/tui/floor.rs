//! The plaza drawn in the terminal: the rooms of the floor seen from above, each agent a glyph in
//! its room (coloured by role, its state shown), and the plaza's traffic — messages, model calls,
//! tool runs — as packets travelling between them, live from the hub's events.

use ratatui::Frame;
use ratatui::layout::Rect;
use serde_json::Value;

/// The floor's state. `tui::app` owns one, feeds it the agents list and every live event, ticks it
/// at the frame rate and renders it into the Floor tab.
#[derive(Debug, Default)]
pub struct FloorView {}

impl FloorView {
    pub fn new() -> FloorView {
        FloorView::default()
    }

    /// The agents (the `GET /v1/agents` answer), on load and on every poll.
    pub fn on_agents(&mut self, agents: &Value) {
        let _ = agents;
    }

    /// One live event from `/v1/ws` (`kind`, `data`), as they arrive.
    pub fn on_event(&mut self, kind: &str, data: &Value) {
        let _ = (kind, data);
    }

    /// Advances the animation by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let _ = dt;
    }

    /// Draws the floor into `area`.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let _ = (frame, area);
    }
}
