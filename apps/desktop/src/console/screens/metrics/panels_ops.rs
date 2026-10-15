//! The operational Metrics bodies, as the web's bodies-ops.ts: firing alerts (open, mute, acknowledge), the rules
//! table (on/off, new rule), kept traces (filter, list, waterfall, span details) and the events feed (kind filter).

use apz_metrics_view::{AlertRow, EventRow, RuleRow, TracesBody};
use iced::Element;

use super::{Msg, State};

pub fn alerts<'a>(rows: &'a [AlertRow]) -> Element<'a, Msg> {
    let _ = rows;
    iced::widget::Space::new().into()
}

pub fn rules<'a>(rows: &'a [RuleRow]) -> Element<'a, Msg> {
    let _ = rows;
    iced::widget::Space::new().into()
}

pub fn traces<'a>(t: &'a TracesBody, state: &'a State) -> Element<'a, Msg> {
    let _ = (t, state);
    iced::widget::Space::new().into()
}

pub fn events<'a>(kinds: &'a [String], rows: &'a [EventRow], state: &'a State) -> Element<'a, Msg> {
    let _ = (kinds, rows, state);
    iced::widget::Space::new().into()
}
