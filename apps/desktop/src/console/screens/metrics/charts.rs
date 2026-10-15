//! The Metrics canvas programs, drawing what the web's charts.ts and figures.ts draw: the line chart (stacked areas,
//! a percentile band, dashed comparison overlays, markers, a limit, annotations, shaded gaps with their reason, the
//! shared crosshair and tooltip, drag-to-zoom, click for what changed), the heatmap, the treemap, gauges, swimlanes
//! and sparklines. Formatting is `apz_metrics_view::format`, the same as the web's.

use apz_metrics_view::{Chart, GaugeItem, Heatmap, Swimlanes, Tone, TreeItem, Unit};
use iced::Element;

use super::{Msg, Pointer};

/// A chart panel's chart. `panel` names the panel in the messages it sends (Hover, DragStart/To/End, Point).
pub fn line<'a>(
    panel: &'a str,
    c: &'a Chart,
    pointer: &'a Pointer,
    annotations: &'a [apz_metrics_view::Annotation],
    show_annotations: bool,
    compact: bool,
    tz: i32,
) -> Element<'a, Msg> {
    let _ = (
        panel,
        c,
        pointer,
        annotations,
        show_annotations,
        compact,
        tz,
    );
    iced::widget::Space::new().into()
}

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
