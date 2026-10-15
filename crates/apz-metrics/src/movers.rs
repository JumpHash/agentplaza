//! "What moved most" between two windows of the same length (the ten minutes before a moment, and the ten minutes
//! around it): the series whose values changed the most, relative to their own size.

use crate::SeriesKey;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mover {
    pub key: SeriesKey,
    pub before: f64,
    pub after: f64,
    /// (after − before) / max(|before|, ε); `f64::INFINITY` is never returned (capped at 100.0, i.e. 100×).
    pub change: f64,
}

/// The `n` series with the largest absolute relative change between `before` and `after` (matched by key; a key
/// missing on one side counts as 0 there). Series below `floor` on both sides are ignored as noise. Ties keep
/// key order.
pub fn top(before: &[(SeriesKey, f64)], after: &[(SeriesKey, f64)], n: usize, floor: f64) -> Vec<Mover> {
    let _ = (before, after, n, floor);
    todo!()
}
