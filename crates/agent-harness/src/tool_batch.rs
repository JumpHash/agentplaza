//! How a turn's tool calls run together, and which of them may start before the model's answer ends.
//!
//! A call touches what it names: a path it reads or writes, or the directory a shell command runs in. Two calls keep
//! their order when they touch the same path and one of them writes it, when a command meets a path under its
//! directory, when two commands share a directory, or when a call names nothing ([`Touch::Other`]). Any other pair
//! may run at once. [`waves`] puts a turn's calls in the order they may run in; [`Streamed`] finds the read-only calls
//! that are whole while the answer still streams.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use apz_proto::chat::Delta;
use serde_json::{Value, json};

use crate::files::Workspace;

/// What a tool call touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// A path (a file, or everything under a directory), read or written.
    Path { path: PathBuf, write: bool },
    /// A shell command in this directory: it may read or write anything under it.
    Command(PathBuf),
    /// Several of the above, for a call that names more than one file (a `read` of several, an `edit` of several).
    Many(Vec<Touch>),
    /// A call that names nothing the harness can order: it runs alone, in order.
    Other,
}

impl Touch {
    /// Whether the call only reads paths.
    pub fn reads_only(&self) -> bool {
        match self {
            Touch::Path { write, .. } => !write,
            Touch::Many(touches) => !touches.is_empty() && touches.iter().all(Touch::reads_only),
            Touch::Command(_) | Touch::Other => false,
        }
    }
}

/// What the call `name` with `input` touches: the workspace paths the call names, or the directory a shell command
/// runs in. A call whose path does not resolve touches [`Touch::Other`].
pub fn touch(workspace: &Workspace, name: &str, input: &Value) -> Touch {
    // The call's `path`: required, or the workspace root when it may be absent.
    let at = |optional: bool, write: bool| {
        let raw = match input.get("path") {
            Some(Value::String(raw)) => raw.as_str(),
            None | Some(Value::Null) if optional => "",
            _ => return Touch::Other,
        };
        resolved(workspace, raw, write)
    };
    // The `path` of each entry of the array `key`, when there is an array with entries.
    let entries = |key: &str, write: bool| -> Vec<Touch> {
        input
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|entry| match entry.get("path").and_then(Value::as_str) {
                Some(raw) => resolved(workspace, raw, write),
                None => Touch::Other,
            })
            .collect()
    };
    match name {
        "read" => many(entries("files", false)),
        "edit" => many([entries("edits", true), entries("create", true)].concat()),
        "find" | "search" => at(true, false),
        // Code intelligence reads the files it names (its language server only indexes them).
        "symbols" | "definition" | "references" | "diagnostics" | "outline" => at(true, false),
        // A command runs in its `cwd`, or the workspace root: a `cd` it makes holds for the commands after it.
        // A script may read or write anything in the workspace, and makes its own calls under their decisions: it runs
        // alone, as a command in the workspace root.
        "run_script" => Touch::Command(workspace.dir().to_path_buf()),
        "run" | "run_tests" => match input.get("cwd") {
            None | Some(Value::Null) => Touch::Command(workspace.dir().to_path_buf()),
            Some(Value::String(raw)) => match workspace.resolve(raw) {
                Ok((path, _)) => Touch::Command(path),
                Err(_) => Touch::Other,
            },
            Some(_) => Touch::Other,
        },
        _ => Touch::Other,
    }
}

/// The touch of a workspace path a call names, or [`Touch::Other`] when the path does not resolve.
fn resolved(workspace: &Workspace, raw: &str, write: bool) -> Touch {
    match workspace.resolve(raw) {
        Ok((path, _)) => Touch::Path { path, write },
        Err(_) => Touch::Other,
    }
}

/// The touch of the files a call names: one touch per file. A call that names no file, or one of whose files does not
/// resolve, touches [`Touch::Other`].
fn many(touches: Vec<Touch>) -> Touch {
    if touches.is_empty() || touches.contains(&Touch::Other) {
        Touch::Other
    } else {
        Touch::Many(touches)
    }
}

/// Whether two calls must keep their order (see the module's rules).
pub fn conflicts(a: &Touch, b: &Touch) -> bool {
    match (a, b) {
        (Touch::Other, _) | (_, Touch::Other) => true,
        (Touch::Many(touches), other) | (other, Touch::Many(touches)) => {
            touches.iter().any(|touch| conflicts(touch, other))
        }
        (Touch::Command(dir), Touch::Command(other)) => overlaps(dir, other),
        (Touch::Command(dir), Touch::Path { path, .. })
        | (Touch::Path { path, .. }, Touch::Command(dir)) => overlaps(dir, path),
        (
            Touch::Path { path, write },
            Touch::Path {
                path: other,
                write: other_write,
            },
        ) => (*write || *other_write) && overlaps(path, other),
    }
}

/// Whether one path is the other, or lies under it.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// The calls of a turn in waves, by their position in `touches`: the calls of a wave run at once, and a call waits for
/// the wave after the last earlier call it conflicts with. Each call is in exactly one wave, and the waves run in order.
pub fn waves(touches: &[Touch]) -> Vec<Vec<usize>> {
    let mut wave_of: Vec<usize> = Vec::with_capacity(touches.len());
    let mut waves: Vec<Vec<usize>> = Vec::new();
    for (index, touch) in touches.iter().enumerate() {
        let wave = (0..index)
            .filter(|&earlier| conflicts(&touches[earlier], touch))
            .map(|earlier| wave_of[earlier] + 1)
            .max()
            .unwrap_or(0);
        if wave == waves.len() {
            waves.push(Vec::new());
        }
        waves[wave].push(index);
        wave_of.push(wave);
    }
    waves
}

/// A call that is whole while its answer still streams, and may run before the answer ends: its id (the key its
/// result answers to), its tool and its input.
#[derive(Debug, Clone, PartialEq)]
pub struct Whole {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// The tool calls of a streamed answer as they arrive. A call opens at its [`Delta::ToolUse`], takes its input in
/// [`Delta::ToolInput`] chunks and is whole at its [`Delta::BlockEnd`]. Every call keeps its place in the answer, so a
/// call is checked against the calls before it.
#[derive(Default)]
pub struct Streamed {
    /// The calls still streaming, by block index.
    open: HashMap<usize, Open>,
    /// The answer's calls in order: each call's touch once it is whole, `None` until then.
    calls: Vec<Option<Touch>>,
}

/// A call still streaming: its place in the answer, and what has come of it so far.
struct Open {
    place: usize,
    id: String,
    name: String,
    json: String,
}

impl Streamed {
    /// Takes one fragment of the answer. Returns the call the fragment completes, when that call may run before the
    /// answer ends: it only reads, it has an id, its input is an object, and no call before it in the answer is still
    /// streaming or touches what it touches.
    pub fn push(&mut self, workspace: &Workspace, delta: Delta) -> Option<Whole> {
        match delta {
            Delta::ToolUse { name, index, id } => {
                let place = self.calls.len();
                self.calls.push(None);
                self.open.insert(
                    index,
                    Open {
                        place,
                        id,
                        name,
                        json: String::new(),
                    },
                );
                None
            }
            Delta::ToolInput { index, json } => {
                if let Some(open) = self.open.get_mut(&index) {
                    open.json.push_str(&json);
                }
                None
            }
            Delta::BlockEnd { index } => {
                let open = self.open.remove(&index)?;
                let input = whole_input(&open.json);
                // A call without an id, or whose input is not an object, names nothing the harness can order.
                let touched = match &input {
                    Some(input) if !open.id.is_empty() => touch(workspace, &open.name, input),
                    _ => Touch::Other,
                };
                let after_earlier = self.calls[..open.place].iter().all(|earlier| {
                    earlier
                        .as_ref()
                        .is_some_and(|earlier| !conflicts(earlier, &touched))
                });
                let ahead = touched.reads_only() && after_earlier;
                self.calls[open.place] = Some(touched);
                match input {
                    Some(input) if ahead => Some(Whole {
                        id: open.id,
                        name: open.name,
                        input,
                    }),
                    _ => None,
                }
            }
            Delta::Text { .. } | Delta::Thinking { .. } => None,
        }
    }
}

/// A streamed call's input, read the way the gateway reads a whole call's: no text is an empty object, and anything
/// else must be a JSON object.
fn whole_input(json: &str) -> Option<Value> {
    if json.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(json)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!(
            "apz-harness-tool-batch-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn read(path: &str) -> Touch {
        Touch::Path {
            path: PathBuf::from(path),
            write: false,
        }
    }

    fn write(path: &str) -> Touch {
        Touch::Path {
            path: PathBuf::from(path),
            write: true,
        }
    }

    /// The fragments of one call: its start, its input in one chunk, and its end.
    fn call(index: usize, id: &str, name: &str, input: &str) -> Vec<Delta> {
        vec![
            Delta::ToolUse {
                name: name.into(),
                index,
                id: id.into(),
            },
            Delta::ToolInput {
                index,
                json: input.into(),
            },
            Delta::BlockEnd { index },
        ]
    }

    /// Feeds fragments to `streamed` and returns the calls that may run ahead.
    fn feed(streamed: &mut Streamed, workspace: &Workspace, deltas: Vec<Delta>) -> Vec<Whole> {
        deltas
            .into_iter()
            .filter_map(|delta| streamed.push(workspace, delta))
            .collect()
    }

    #[test]
    fn reads_of_different_files_run_at_once_and_a_write_keeps_its_order_with_its_file() {
        let touches = [read("/w/a"), read("/w/b"), write("/w/a"), read("/w/a")];
        assert_eq!(waves(&touches), vec![vec![0, 1], vec![2], vec![3]]);
    }

    #[test]
    fn commands_keep_their_order_and_a_path_outside_their_directory_runs_with_them() {
        let dir = PathBuf::from("/w");
        let touches = [
            Touch::Command(dir.clone()),
            read("/w/a"),
            Touch::Command(dir),
            read("/elsewhere"),
        ];
        assert_eq!(waves(&touches), vec![vec![0, 3], vec![1], vec![2]]);
    }

    #[test]
    fn a_call_that_names_nothing_runs_alone_and_the_calls_after_it_wait() {
        let touches = [read("/w/a"), Touch::Other, read("/w/b")];
        assert_eq!(waves(&touches), vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn a_tool_call_touches_the_path_it_names_or_the_directory_a_command_runs_in() {
        let ws = scratch("touch");
        let file = ws.resolve("a.txt").expect("a path").0;
        let root = ws.resolve("").expect("the root").0;
        assert_eq!(
            touch(
                &ws,
                "edit",
                &json!({"edits": [{"path": "a.txt", "old": "x", "new": "y"}]})
            ),
            Touch::Many(vec![Touch::Path {
                path: file.clone(),
                write: true
            }])
        );
        assert_eq!(
            touch(&ws, "read", &json!({"files": [{"path": "a.txt"}]})),
            Touch::Many(vec![Touch::Path {
                path: file,
                write: false
            }])
        );
        // A directory tool with no path reads the whole workspace.
        assert_eq!(
            touch(&ws, "search", &json!({"pattern": "x"})),
            Touch::Path {
                path: root,
                write: false
            }
        );
        assert_eq!(
            touch(
                &ws,
                "definition",
                &json!({"path": "a.txt", "line": 1, "column": 1})
            ),
            Touch::Path {
                path: ws.resolve("a.txt").expect("a path").0,
                write: false
            }
        );
        assert_eq!(
            touch(&ws, "run", &json!({"command": "ls"})),
            Touch::Command(ws.dir().to_path_buf())
        );
        // A script touches the workspace as a whole, so no read or write of it runs beside the script.
        assert_eq!(
            touch(
                &ws,
                "run_script",
                &json!({"language": "python", "code": "pass"})
            ),
            Touch::Command(ws.dir().to_path_buf())
        );
        assert!(conflicts(
            &touch(
                &ws,
                "run_script",
                &json!({"language": "python", "code": "pass"})
            ),
            &read(&ws.resolve("a.txt").expect("a path").0.to_string_lossy())
        ));
        // A file tool without its path, a path outside the workspace, and a tool the harness does not order.
        assert_eq!(touch(&ws, "read", &json!({})), Touch::Other);
        assert_eq!(
            touch(&ws, "read", &json!({"files": [{"path": "../outside"}]})),
            Touch::Other
        );
        assert_eq!(
            touch(&ws, "conversation", &json!({"text": "hi"})),
            Touch::Other
        );
    }

    #[test]
    fn a_whole_read_runs_ahead_and_a_write_does_not() {
        let ws = scratch("ahead");
        let mut streamed = Streamed::default();
        let ahead = feed(
            &mut streamed,
            &ws,
            call(0, "r1", "read", "{\"files\": [{\"path\": \"a.txt\"}]}"),
        );
        assert_eq!(
            ahead,
            vec![Whole {
                id: "r1".into(),
                name: "read".into(),
                input: json!({"files": [{"path": "a.txt"}]}),
            }]
        );
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(
                    1,
                    "w1",
                    "edit",
                    "{\"edits\": [{\"path\": \"a.txt\", \"old\": \"x\", \"new\": \"y\"}]}"
                ),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_read_runs_ahead_only_when_no_earlier_call_could_change_what_it_reads() {
        let ws = scratch("order");
        let mut streamed = Streamed::default();
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(
                    0,
                    "w1",
                    "edit",
                    "{\"edits\": [{\"path\": \"a.txt\", \"old\": \"x\", \"new\": \"y\"}]}"
                ),
            )
            .is_empty()
        );
        // The write before it touches the same file: the read waits for it.
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(1, "r1", "read", "{\"files\": [{\"path\": \"a.txt\"}]}")
            )
            .is_empty()
        );
        // Another file is read ahead, whatever came before it.
        assert_eq!(
            feed(
                &mut streamed,
                &ws,
                call(2, "r2", "read", "{\"files\": [{\"path\": \"b.txt\"}]}")
            )
            .iter()
            .map(|whole| whole.id.as_str())
            .collect::<Vec<_>>(),
            vec!["r2"]
        );
    }

    #[test]
    fn a_call_still_streaming_before_a_read_holds_the_read_back() {
        let ws = scratch("open");
        let mut streamed = Streamed::default();
        // The first call never ends in this answer: nothing says what it touches.
        assert!(
            feed(
                &mut streamed,
                &ws,
                vec![Delta::ToolUse {
                    name: "read".into(),
                    index: 0,
                    id: "r0".into(),
                }],
            )
            .is_empty()
        );
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(1, "r1", "read", "{\"files\": [{\"path\": \"a.txt\"}]}")
            )
            .is_empty()
        );
    }

    #[test]
    fn a_call_without_an_id_or_with_input_that_is_not_an_object_does_not_run_ahead() {
        let ws = scratch("refused");
        assert!(
            feed(
                &mut Streamed::default(),
                &ws,
                call(0, "", "read", "{\"files\": [{\"path\": \"a.txt\"}]}")
            )
            .is_empty()
        );
        assert!(
            feed(
                &mut Streamed::default(),
                &ws,
                call(0, "r1", "read", "[\"a.txt\"]")
            )
            .is_empty()
        );
        assert!(
            feed(
                &mut Streamed::default(),
                &ws,
                call(0, "r1", "read", "{\"path\": \"a.txt\"")
            )
            .is_empty()
        );
    }

    #[test]
    fn streamed_input_chunks_join_and_an_empty_input_is_an_empty_object() {
        let ws = scratch("chunks");
        let mut streamed = Streamed::default();
        let ahead = feed(
            &mut streamed,
            &ws,
            vec![
                Delta::ToolUse {
                    name: "read".into(),
                    index: 3,
                    id: "r3".into(),
                },
                Delta::ToolInput {
                    index: 3,
                    json: "{\"files\":".into(),
                },
                Delta::Text { text: "…".into() },
                Delta::ToolInput {
                    index: 3,
                    json: " [{\"path\": \"a.txt\"}]}".into(),
                },
                Delta::BlockEnd { index: 3 },
            ],
        );
        assert_eq!(ahead[0].input, json!({"files": [{"path": "a.txt"}]}));
        let listing = feed(
            &mut streamed,
            &ws,
            vec![
                Delta::ToolUse {
                    name: "find".into(),
                    index: 4,
                    id: "l4".into(),
                },
                Delta::BlockEnd { index: 4 },
            ],
        );
        assert_eq!(listing[0].input, json!({}));
    }
}
