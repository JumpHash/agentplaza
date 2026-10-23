//! What an import made of a source file (PLAN 22.6): the topology, and one item per thing in the source that was not
//! carried over as it was. Nothing is dropped without an item: a key the importer does not know is an item too.

use serde::Serialize;

use crate::spec::Topology;

/// What became of one item of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Carried over with the same meaning.
    Converted,
    /// Carried over with a different meaning, which the detail says.
    Approximated,
    /// Not carried over at all; the detail says why.
    NotConverted,
}

/// One item of the source, where it is (`<service>.<key>`, or a manifest's `<kind>/<name>`), and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub at: String,
    pub outcome: Outcome,
    pub detail: String,
}

/// The topology an import made, with its report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conversion {
    pub topology: Topology,
    pub report: Vec<Item>,
}

/// Appends an item to a report.
pub fn note(
    report: &mut Vec<Item>,
    at: impl Into<String>,
    outcome: Outcome,
    detail: impl Into<String>,
) {
    report.push(Item {
        at: at.into(),
        outcome,
        detail: detail.into(),
    });
}
