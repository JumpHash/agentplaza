//! Misses (PLAN 15.4): a test the selection skipped that fails in the scheduled full run on `main` (or in a batch of the
//! merge queue). Each miss is recorded and shown per area (the package), so the owner sees where selection cannot be
//! trusted yet.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::map::{CoverageMap, Miss};
use crate::select::Selection;

/// The tests that failed in a full run and that `selection` did not choose. A full selection misses nothing.
pub fn missed(selection: &Selection, failed_in_full_run: &[String]) -> Vec<String> {
    if selection.full {
        return Vec::new();
    }
    let chosen: BTreeSet<&str> = selection.chosen.iter().map(|c| c.test.as_str()).collect();
    let mut out: Vec<String> = failed_in_full_run
        .iter()
        .filter(|test| !chosen.contains(test.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One package's tests and its misses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AreaSummary {
    pub package: String,
    /// The tests the map knows in it.
    pub tests: usize,
    /// The misses recorded in it.
    pub misses: usize,
}

/// Every package the map or the misses name, with its test count and miss count, sorted by package.
pub fn per_area(map: &CoverageMap, misses: &[Miss]) -> Vec<AreaSummary> {
    let mut tests: BTreeMap<&str, usize> = BTreeMap::new();
    for test in &map.tests {
        *tests.entry(test.package.as_str()).or_default() += 1;
    }
    let mut counted: BTreeMap<&str, usize> = BTreeMap::new();
    for miss in misses {
        *counted.entry(miss.package.as_str()).or_default() += 1;
    }
    let areas: BTreeSet<&str> = tests.keys().chain(counted.keys()).copied().collect();
    areas
        .into_iter()
        .map(|package| AreaSummary {
            package: package.to_string(),
            tests: tests.get(package).copied().unwrap_or_default(),
            misses: counted.get(package).copied().unwrap_or_default(),
        })
        .collect()
}
