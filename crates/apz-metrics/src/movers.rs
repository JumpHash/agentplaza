//! "What moved most" between two windows of the same length (the ten minutes before a moment, and the ten minutes
//! around it): the series whose values changed the most, relative to their own size.

use crate::SeriesKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
pub fn top(
    before: &[(SeriesKey, f64)],
    after: &[(SeriesKey, f64)],
    n: usize,
    floor: f64,
) -> Vec<Mover> {
    // Keyed by series, in key order, so ties come out in key order.
    let mut sides: BTreeMap<SeriesKey, (f64, f64)> = BTreeMap::new();
    for (key, value) in before {
        sides.entry(key.clone()).or_insert((0.0, 0.0)).0 = *value;
    }
    for (key, value) in after {
        sides.entry(key.clone()).or_insert((0.0, 0.0)).1 = *value;
    }

    let mut movers: Vec<Mover> = sides
        .into_iter()
        .filter(|(_, (b, a))| b.abs() >= floor || a.abs() >= floor)
        .map(|(key, (b, a))| {
            let change = ((a - b) / b.abs().max(1e-9)).clamp(-100.0, 100.0);
            Mover {
                key,
                before: b,
                after: a,
                change,
            }
        })
        .collect();

    movers.sort_by(|x, y| {
        y.change
            .abs()
            .total_cmp(&x.change.abs())
            .then_with(|| x.key.cmp(&y.key))
    });
    movers.truncate(n);
    movers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityKind;

    fn key(metric: &str) -> SeriesKey {
        SeriesKey::new(metric, EntityKind::Node, metric)
    }

    #[test]
    fn the_largest_relative_change_comes_first_and_a_missing_side_counts_as_zero() {
        let before = vec![
            (key("steady"), 10.0),
            (key("doubled"), 1.0),
            (key("fading"), 3.0),
        ];
        let after = vec![
            (key("steady"), 11.0),
            (key("doubled"), 3.0),
            (key("fading"), 0.0),
            (key("appeared"), 4.0),
        ];
        let movers = top(&before, &after, 3, 0.5);

        let names: Vec<&str> = movers.iter().map(|m| m.key.metric.as_str()).collect();
        assert_eq!(names, vec!["appeared", "doubled", "fading"]);
        // A series missing before counts as 0, so 0 -> 4 is capped at 100x; 1 -> 3 is a 200% rise.
        assert_eq!(movers[0].change, 100.0);
        assert!((movers[1].change - 2.0).abs() < 1e-9);
        assert!((movers[2].change + 1.0).abs() < 1e-9);
    }

    #[test]
    fn noise_below_the_floor_is_ignored() {
        let before = vec![(key("noise"), 0.001), (key("signal"), 2.0)];
        let after = vec![(key("noise"), 0.002), (key("signal"), 2.5)];
        let movers = top(&before, &after, 10, 0.1);

        assert_eq!(movers.len(), 1);
        assert_eq!(movers[0].key.metric, "signal");
    }
}
