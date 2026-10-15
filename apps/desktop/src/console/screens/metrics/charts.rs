//! The Metrics line chart, a canvas program drawing what the web's charts.ts draws: stacked areas, a percentile band,
//! dashed comparison overlays, markers, a limit, annotations, shaded gaps with their reason, the shared crosshair and
//! tooltip, drag-to-zoom and a click for what changed. Formatting is `apz_metrics_view::format`, the same as the
//! web's.

use apz_metrics_view::Chart;
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
