//! The other Metrics figures, as the web's figures.ts: the latency heatmap, treemaps, gauges, lifecycle swimlanes and
//! sparklines (canvas programs).

use apz_metrics_view::{GaugeItem, Heatmap, Swimlanes, Tone, TreeItem, Unit};
use iced::Element;

use super::Msg;

pub fn heatmap<'a>(panel: &'a str, h: &'a Heatmap, compact: bool, tz: i32) -> Element<'a, Msg> {
    let _ = (panel, h, compact, tz);
    iced::widget::Space::new().into()
}

pub fn treemap<'a>(unit: Unit, items: &'a [TreeItem]) -> Element<'a, Msg> {
    let _ = (unit, items);
    iced::widget::Space::new().into()
}

pub fn gauges<'a>(items: &'a [GaugeItem]) -> Element<'a, Msg> {
    let _ = items;
    iced::widget::Space::new().into()
}

pub fn swimlanes<'a>(s: &'a Swimlanes, tz: i32) -> Element<'a, Msg> {
    let _ = (s, tz);
    iced::widget::Space::new().into()
}

/// A small sparkline (filled under the line), `width` × `height`.
pub fn spark<'a>(values: &'a [f64], tone: Tone, width: f32, height: f32) -> Element<'a, Msg> {
    let _ = (values, tone, width, height);
    iced::widget::Space::new().into()
}
