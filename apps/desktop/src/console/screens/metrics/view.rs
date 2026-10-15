//! The Metrics screen's layout: the head (title with the pin star, breadcrumb with the sibling switcher, the
//! controls), the tier line (window end, storage tier, live text, zoom and compare chips, level), the scope tree,
//! the grouped tabs with the alerts badge, and the grid of panels by span (twelfths), as the web's `.mx-view`.

use iced::Element;

use super::{Msg, State};
use crate::console::Ctx;

pub fn screen<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let _ = (state, ctx);
    iced::widget::text("Metrics").into()
}
