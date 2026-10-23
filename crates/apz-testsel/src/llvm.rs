//! Recording the coverage map from llvm-cov's per-test exports (PLAN 15.4).
//!
//! A Rust project's map is recorded on `main` with one run per test: each test runs alone with its own profile, and
//! `llvm-cov export` says which functions that run executed (a function whose `count` is above zero). The functions of
//! every export are the map's definitions; the tests that executed one are its covering tests. The export read here is
//! llvm's JSON (`"type": "llvm.coverage.json.export"`): `data[].functions[]` with `name`, `count`, `filenames` and
//! `regions`, each region `[line_start, col_start, line_end, col_end, count, file_id, expanded_file_id, kind]`.
//!
//! The recording itself needs `cargo llvm-cov` (or `llvm-cov` with `llvm-profdata`) on the machine that records; this
//! module reads what it writes and never runs it.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::Error;
use crate::map::{CoverageMap, Definition, FORMAT, Package, SOURCE_LLVM_COV, TestEntry};

#[derive(Deserialize)]
struct Export {
    data: Vec<Data>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(default)]
    functions: Vec<Function>,
}

#[derive(Deserialize)]
struct Function {
    #[serde(default)]
    name: String,
    count: u64,
    #[serde(default)]
    filenames: Vec<String>,
    #[serde(default)]
    regions: Vec<Vec<u64>>,
}

/// One function an export names: its file (from the root), its name, its lines, and whether the run executed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    pub file: String,
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub executed: bool,
}

/// One test's coverage run: the test's id and the export its run wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub test: String,
    pub export: String,
}

/// Reads one export. `root` is the repository's root (absolute, as llvm-cov writes the paths); files are named from it.
/// A function without regions names no file and is left out.
pub fn parse_export(json: &str, root: &str) -> Result<Vec<Executed>, Error> {
    let export: Export = serde_json::from_str(json)
        .map_err(|e| Error::Export(format!("not an llvm-cov export: {e}")))?;
    let mut out = Vec::new();
    for data in &export.data {
        for function in &data.functions {
            let Some(first) = function.regions.first() else {
                continue;
            };
            let file_id = region_field(first, 5, "file id")?;
            let Some(filename) = function.filenames.get(file_id as usize) else {
                return Err(Error::Export(format!("file id {file_id} names no file")));
            };
            // The function's lines in its own file: the regions that sit in that file (expansions elsewhere excluded).
            let mut start = u64::MAX;
            let mut end = 0u64;
            for region in &function.regions {
                if region_field(region, 5, "file id")? != file_id {
                    continue;
                }
                start = start.min(region_field(region, 0, "line")?);
                end = end.max(region_field(region, 2, "line")?);
            }
            if start == u64::MAX {
                continue;
            }
            out.push(Executed {
                file: relative(filename, root),
                // The export's name is the mangled symbol; it is kept as written, for display only (the lines identify).
                name: function.name.clone(),
                start: to_u32(start)?,
                end: to_u32(end)?,
                executed: function.count > 0,
            });
        }
    }
    Ok(out)
}

fn region_field(region: &[u64], index: usize, what: &str) -> Result<u64, Error> {
    region
        .get(index)
        .copied()
        .ok_or_else(|| Error::Export(format!("a region has no {what} field")))
}

fn to_u32(value: u64) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Export(format!("line {value} is out of range")))
}

/// `path` relative to `root` (both `/`-separated), or `path` as it is when it lies outside the root.
pub fn relative(path: &str, root: &str) -> String {
    let root = root.trim_end_matches('/');
    match path.strip_prefix(root) {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => path.to_string(),
    }
}

/// Records a map from the tests' runs. `packages` and `tests` are the build graph and the inventory (every test that
/// exists, with its package and smoke flag); each run names one of them. A run of a test the inventory does not know
/// is refused. The definitions are the union over the runs: a definition no test executed is kept, with no covering test.
pub fn record(
    project: &str,
    commit: &str,
    root: &str,
    runner: &str,
    packages: Vec<Package>,
    tests: Vec<TestEntry>,
    runs: &[TestRun],
    recorded_ms: u64,
) -> Result<CoverageMap, Error> {
    let known: BTreeSet<&str> = tests.iter().map(|t| t.id.as_str()).collect();
    // (file, name, start, end) -> the tests that executed it.
    let mut defs: BTreeMap<(String, String, u32, u32), BTreeSet<String>> = BTreeMap::new();
    for run in runs {
        if !known.contains(run.test.as_str()) {
            return Err(Error::UnknownTest(run.test.clone()));
        }
        for executed in parse_export(&run.export, root)? {
            let entry = defs
                .entry((executed.file, executed.name, executed.start, executed.end))
                .or_default();
            if executed.executed {
                entry.insert(run.test.clone());
            }
        }
    }
    let files: BTreeSet<String> = defs.keys().map(|(file, ..)| file.clone()).collect();
    let definitions = defs
        .into_iter()
        .map(|((file, name, start, end), covered)| Definition {
            file,
            name,
            start,
            end,
            covered_by: covered.into_iter().collect(),
        })
        .collect();
    Ok(CoverageMap {
        id: CoverageMap::key(project, commit),
        format: FORMAT,
        project: project.to_string(),
        commit: commit.to_string(),
        source: SOURCE_LLVM_COV.to_string(),
        runner: runner.to_string(),
        recorded_ms,
        packages,
        tests,
        definitions,
        files: files.into_iter().collect(),
    })
}
