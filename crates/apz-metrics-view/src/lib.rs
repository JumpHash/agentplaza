//! The Metrics view model.
//!
//! The hub composes every tab of the Metrics view as a [`View`]: a list of [`Panel`]s, each a typed [`Body`]
//! (tiles, a chart, a table, a heatmap …) with its numbers already chosen, coloured by [`Tone`] tokens and
//! labelled. The web console and the desktop console are renderers of these panels: neither decides what a tab
//! shows, so the two cannot drift apart. Interaction (zoom, compare, the "what changed" drawer, the panel menu,
//! pins, the find palette) is the consoles' own, driven by the same fields.
//!
//! * [`model`] — the types (serde; the web mirrors them in `apps/web/src/metrics/model.ts`).
//! * [`format`] — value formatting by unit, axis ticks and time labels; `tests/format_golden.json` pins it and
//!   the web test suite checks its own formatter against the same file.
//! * [`tabs`] — the tab list, groups and which tabs apply to a scope kind.
//! * [`compare`] — comparison overlays and change percentages.

#![forbid(unsafe_code)]

pub mod compare;
pub mod format;
pub mod model;
pub mod tabs;

pub use apz_metrics::catalog::Unit;
pub use model::*;
