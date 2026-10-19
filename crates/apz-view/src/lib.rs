//! The panel model.
//!
//! A [`model::Panel`] is a title and a typed [`model::Body`] (tiles, a chart, a table, a heatmap … an image, a
//! graph, a dataset, a document, code or live HTML) with its numbers already chosen and coloured by
//! [`model::Tone`] tokens. The web console and the desktop console are renderers of these panels, so the two
//! cannot drift apart. The hub composes the Metrics tabs from them (`apz-metrics-view`), and an agent's artifact
//! is one or more of them ([`artifact`]).
//!
//! * [`model`] — the types (serde; the web mirrors them in `apps/web/src/view/model.ts`).
//! * [`format`] — value formatting by unit, axis ticks and time labels; `tests/format_golden.json` pins it.
//! * [`artifact`] — an artifact's spec, its caps and the validator the hub runs on every create and update.
//! * [`libs`] — the vendored libraries an HTML artifact may import, at pinned versions.

#![forbid(unsafe_code)]

pub mod artifact;
pub mod format;
pub mod highlight;
pub mod libs;
pub mod model;
#[cfg(feature = "raster")]
pub mod raster;

pub use apz_metrics::catalog::Unit;
pub use model::*;
