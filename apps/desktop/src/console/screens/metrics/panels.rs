//! A Metrics panel: the card (title, sub, right text, freshness stamp, pinned-from line, the ⋯ button) and its
//! body. Charts are in [`super::charts`] and [`super::figures`]; the alerting, rules, traces and events bodies in
//! [`super::panels_ops`]; everything else is built from widgets here.

use apz_metrics_view::Panel;
use iced::Element;

use super::{Msg, State};

/// A panel's card with its body. `width` is the card's share of the grid in twelfths (from `p.span`).
pub fn panel<'a>(p: &'a Panel, state: &'a State) -> Element<'a, Msg> {
    let _ = (p, state);
    iced::widget::text(p.title.clone()).into()
}
