//! Test selection on the hub (PLAN 15.4): the coverage maps recorded on `main` per project and commit, the misses a full
//! run found among the tests a selection skipped, and the `tests_select` tool that answers which tests a change needs.
//! The selecting itself is `apz-testsel`'s; this module holds the maps and misses and answers the agents.
//!
//! The maps and the misses are components of `plaza.db` (`test_map`, `test_miss` and their child tables: typed rows, no
//! JSON), saved by the saver like the other components. The newest map of a project is the one a selection reads. A
//! project with no map gets the full suite, since nothing says which tests a change can skip.
//!
//! The map is recorded by the full run the CI recipe schedules on `main` (4.5): its per-test llvm-cov exports, the build
//! graph and the inventory of the commit, through `apz_testsel::llvm::record` and [`TestSelect::record`]. The schedule
//! itself is not wired yet; until it is, every project answers with the full suite.

use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use apz_testsel::map::{CoverageMap, FORMAT, Miss};
use apz_testsel::{Change, Selection, Touch};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::state::App;

/// The tools this module serves.
pub const TOOLS: &[&str] = &["tests_select"];

/// The most selected tests one answer lists; the rest are counted, not named.
pub const LISTED: usize = 200;

/// The maps' tables (`test_map` with its children), as the saver keeps them.
pub fn map_def() -> &'static TableDef {
    apz_store::root_def!(CoverageMap)
}

/// The misses' table (`test_miss`).
pub fn miss_def() -> &'static TableDef {
    apz_store::root_def!(Miss)
}

/// What the hub holds for test selection: the maps (one per project and commit) and the misses.
#[derive(Default)]
pub struct TestSelect {
    maps: Mutex<Vec<CoverageMap>>,
    misses: Mutex<Vec<Miss>>,
}

impl TestSelect {
    /// Records a map of `main` at a commit. The map of the same project and commit is replaced; the others are kept.
    /// Nothing calls it outside the tests until the scheduled full run on `main` is wired (4.5).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn record(&self, map: CoverageMap) -> Result<(), String> {
        if map.format != FORMAT {
            return Err(format!(
                "a coverage map of format {} (this hub reads {FORMAT})",
                map.format
            ));
        }
        let mut maps = lock(&self.maps);
        maps.retain(|m| m.id != map.id);
        maps.push(map);
        Ok(())
    }

    /// The newest map of a project: the last one recorded for it.
    pub fn latest(&self, project: &str) -> Option<CoverageMap> {
        lock(&self.maps)
            .iter()
            .rev()
            .find(|m| m.project == project)
            .cloned()
    }

    /// Records misses. One already recorded (the same project, commit and test) is kept once.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn record_misses(&self, misses: Vec<Miss>) {
        let mut stored = lock(&self.misses);
        for miss in misses {
            if !stored.iter().any(|m| m.id == miss.id) {
                stored.push(miss);
            }
        }
    }

    /// The misses recorded for a project, oldest first.
    pub fn misses(&self, project: &str) -> Vec<Miss> {
        lock(&self.misses)
            .iter()
            .filter(|m| m.project == project)
            .cloned()
            .collect()
    }

    /// Every map's rows, as they are now (the saver writes what changed).
    pub fn map_rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(map_def(), lock(&self.maps).iter())
    }

    /// Replaces the maps with what plaza.db holds (at start).
    pub fn load_maps(&self, conn: &Connection) -> Result<(), StoreError> {
        *lock(&self.maps) = apz_store::table::load_list(conn, map_def())?;
        Ok(())
    }

    /// Every miss's rows, as they are now.
    pub fn miss_rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(miss_def(), lock(&self.misses).iter())
    }

    /// Replaces the misses with what plaza.db holds (at start).
    pub fn load_misses(&self, conn: &Connection) -> Result<(), StoreError> {
        *lock(&self.misses) = apz_store::table::load_list(conn, miss_def())?;
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The misses of one full run: the tests the selection skipped that failed in it, each recorded with the selection it
/// was skipped for (`selection`, a change's label or its commit).
#[cfg_attr(not(test), allow(dead_code))]
pub fn misses_of(
    map: &CoverageMap,
    selection: &Selection,
    failed_in_full_run: &[String],
    selection_label: &str,
    now_ms: u64,
) -> Vec<Miss> {
    apz_testsel::misses::missed(selection, failed_in_full_run)
        .into_iter()
        .map(|test| {
            let package = map
                .tests
                .iter()
                .find(|t| t.id == test)
                .map(|t| t.package.clone())
                .unwrap_or_default();
            Miss {
                id: Miss::key(&map.project, &map.commit, &test),
                project: map.project.clone(),
                commit: map.commit.clone(),
                test,
                package,
                selection: selection_label.to_string(),
                recorded_ms: now_ms,
            }
        })
        .collect()
}

/// The `tests_select` tool's schema.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    if name != "tests_select" {
        return None;
    }
    let lines = json!({"type": "array", "items": {"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 2, "maxItems": 2}});
    let file = json!({"type": "object", "properties": {
        "path": {"type": "string", "description": "the file, from the repository's root"},
        "lines": {"description": "the lines the change touches in the file as it is after it (omit for the whole file)", "anyOf": [lines, {"type": "null"}]}
    }, "required": ["path"], "additionalProperties": false});
    Some((
        "Which tests a change needs, from the project's coverage map on main: the tests that cover the definitions it touches, those in its packages and their dependents, the tests of the files it changes, and the smoke set. The full suite instead when it changes build, CI or toolchain files, or code the map does not cover. Returns a command for run_tests: pass it as `command`.",
        json!({"type": "object", "properties": {
            "change": {"type": "object", "properties": {"files": {"type": "array", "items": file}}, "required": ["files"], "additionalProperties": false}
        }, "required": ["change"], "additionalProperties": false}),
    ))
}

/// Runs `tests_select` for the agent's project.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid).ok_or("you are not on a project")?;
    let change = change_of(input)?;
    let answer = match app.test_select.latest(&project) {
        Some(map) => {
            let selection = apz_testsel::select(&change, &map);
            answer(
                &map.commit,
                &map.source,
                &map.runner,
                &selection,
                &app.test_select.misses(&project),
            )
        }
        None => json!({
            "full": true,
            "full_because": ["no coverage map is recorded for this project yet"],
            "command": null,
            "tests": [],
        }),
    };
    Ok(answer.to_string())
}

/// The answer's JSON: the selection, the command, the first [`LISTED`] tests, and the misses per area.
fn answer(
    commit: &str,
    source: &str,
    runner: &str,
    selection: &Selection,
    misses: &[Miss],
) -> Value {
    let listed: Vec<Value> = selection
        .chosen
        .iter()
        .take(LISTED)
        .map(|c| json!({"id": c.test, "package": c.package, "why": c.why}))
        .collect();
    json!({
        "commit": commit,
        "source": source,
        "runner": runner,
        "full": selection.full,
        "full_because": selection.full_because,
        "unmapped": selection.unmapped,
        "total": selection.total,
        "selected": selection.chosen.len(),
        "fraction": (selection.fraction() * 1000.0).round() / 1000.0,
        "packages": selection.packages,
        "command": if selection.command.is_empty() { Value::Null } else { json!(selection.command) },
        "tests": listed,
        "tests_listed": selection.chosen.len().min(LISTED),
        "misses": misses.iter().map(|m| json!({"test": m.test, "package": m.package, "selection": m.selection})).collect::<Vec<_>>(),
    })
}

/// The change a call names: `{"change": {"files": [{"path", "lines"?}]}}`.
fn change_of(input: &Value) -> Result<Change, String> {
    let files = input
        .get("change")
        .and_then(|c| c.get("files"))
        .and_then(Value::as_array)
        .ok_or("\"change\" needs a \"files\" list")?;
    let mut out = Vec::new();
    for file in files {
        let path = file
            .get("path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or("each file needs a \"path\"")?;
        let lines = match file.get("lines") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let ranges: Vec<[u32; 2]> =
                    serde_json::from_value(value.clone()).map_err(|e| format!("lines: {e}"))?;
                if ranges.is_empty() {
                    None
                } else {
                    Some(ranges.into_iter().map(|[a, b]| (a, b)).collect())
                }
            }
        };
        out.push(Touch {
            path: path.replace('\\', "/").trim_start_matches("./").to_string(),
            lines,
        });
    }
    Ok(Change { files: out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_testsel::map::{Definition, Package, TestEntry};

    fn map(project: &str, commit: &str) -> CoverageMap {
        CoverageMap {
            id: CoverageMap::key(project, commit),
            format: FORMAT,
            project: project.into(),
            commit: commit.into(),
            source: "build-graph".into(),
            runner: "cargo".into(),
            recorded_ms: 1,
            packages: vec![
                Package {
                    name: "leaf".into(),
                    dir: "crates/leaf".into(),
                    deps: vec![],
                },
                Package {
                    name: "other".into(),
                    dir: "crates/other".into(),
                    deps: vec![],
                },
            ],
            tests: vec![
                TestEntry {
                    id: "crates/leaf/tests/a.rs::one".into(),
                    package: "leaf".into(),
                    file: "crates/leaf/tests/a.rs".into(),
                    smoke: false,
                },
                TestEntry {
                    id: "crates/other/tests/b.rs::two".into(),
                    package: "other".into(),
                    file: "crates/other/tests/b.rs".into(),
                    smoke: false,
                },
            ],
            definitions: Vec::<Definition>::new(),
            files: vec![],
        }
    }

    #[test]
    fn a_new_map_of_a_commit_replaces_the_old_one_and_latest_is_the_newest_for_the_project() {
        let s = TestSelect::default();
        s.record(map("p", "c1")).unwrap();
        s.record(map("q", "c9")).unwrap();
        s.record(map("p", "c2")).unwrap();
        s.record(map("p", "c1")).unwrap();
        // c1 was recorded again after c2: it is the newest of the project now.
        assert_eq!(s.latest("p").unwrap().commit, "c1");
        assert_eq!(s.latest("q").unwrap().commit, "c9");
        assert!(s.latest("nobody").is_none());
    }

    #[test]
    fn a_map_of_another_format_is_refused() {
        let s = TestSelect::default();
        let mut m = map("p", "c1");
        m.format = FORMAT + 1;
        assert!(s.record(m).is_err());
        assert!(s.latest("p").is_none());
    }

    #[test]
    fn a_miss_is_recorded_once_per_project_commit_and_test() {
        let s = TestSelect::default();
        let m = map("p", "c1");
        let selection = apz_testsel::select(
            &Change {
                files: vec![Touch {
                    path: "crates/leaf/tests/a.rs".into(),
                    lines: None,
                }],
            },
            &m,
        );
        let failed = vec!["crates/other/tests/b.rs::two".to_string()];
        let found = misses_of(&m, &selection, &failed, "change t1", 5);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].package, "other");
        s.record_misses(found.clone());
        s.record_misses(found);
        assert_eq!(s.misses("p").len(), 1);
        assert!(s.misses("q").is_empty());
    }

    #[test]
    fn the_answer_names_the_command_the_selection_and_its_misses() {
        let m = map("p", "c1");
        let selection = apz_testsel::select(
            &Change {
                files: vec![Touch {
                    path: "crates/leaf/src/lib.rs".into(),
                    lines: Some(vec![(3, 4)]),
                }],
            },
            &m,
        );
        let answer = answer(&m.commit, &m.source, &m.runner, &selection, &[]);
        assert_eq!(answer["full"], json!(false));
        assert_eq!(answer["command"], json!("cargo test -p leaf"));
        assert_eq!(answer["selected"], json!(1));
        assert_eq!(answer["total"], json!(2));
        assert_eq!(answer["fraction"], json!(0.5));
        assert_eq!(answer["tests"][0]["why"], json!("package"));
    }

    #[test]
    fn a_change_is_read_with_its_lines_and_a_whole_file_when_they_are_empty() {
        let input = json!({"change": {"files": [
            {"path": "./crates/leaf/src/lib.rs", "lines": [[3, 4]]},
            {"path": "crates\\other\\x.rs"},
            {"path": "docs/a.md", "lines": []}
        ]}});
        let change = change_of(&input).unwrap();
        assert_eq!(change.files[0].path, "crates/leaf/src/lib.rs");
        assert_eq!(change.files[0].lines, Some(vec![(3, 4)]));
        assert_eq!(change.files[1].path, "crates/other/x.rs");
        assert_eq!(change.files[1].lines, None);
        assert_eq!(change.files[2].lines, None);
        assert!(change_of(&json!({"change": {}})).is_err());
        assert!(change_of(&json!({"change": {"files": [{"path": ""}]}})).is_err());
    }
}
