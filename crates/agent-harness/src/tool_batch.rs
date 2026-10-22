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
    /// A call that names nothing the harness can order: it runs alone, in order.
    Other,
}

impl Touch {
    /// Whether the call only reads a path.
    pub fn reads_only(&self) -> bool {
        matches!(self, Touch::Path { write: false, .. })
    }
}

/// What the call `name` with `input` touches: the workspace path a file tool names, or the directory a shell command
/// runs in. A file tool whose path does not resolve touches [`Touch::Other`].
pub fn touch(workspace: &Workspace, name: &str, input: &Value) -> Touch {
    // The call's `path`: required, or the workspace root when it may be absent.
    let at = |optional: bool, write: bool| {
        let raw = match input.get("path") {
            Some(Value::String(raw)) => raw.as_str(),
            None | Some(Value::Null) if optional => "",
            _ => return Touch::Other,
        };
        match workspace.resolve(raw) {
            Ok((path, _)) => Touch::Path { path, write },
            Err(_) => Touch::Other,
        }
    };
    match name {
        "read_file" => at(false, false),
        "list" | "search" | "glob" | "grep" => at(true, false),
        "edit_file" | "multi_edit" | "write_file" | "append_file" => at(false, true),
        "bash" => Touch::Command(workspace.dir().to_path_buf()),
        _ => Touch::Other,
    }
}

/// Whether two calls must keep their order (see the module's rules).
pub fn conflicts(a: &Touch, b: &Touch) -> bool {
    match (a, b) {
        (Touch::Other, _) | (_, Touch::Other) => true,
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
                "edit_file",
                &json!({"path": "a.txt", "old": "x", "new": "y"})
            ),
            Touch::Path {
                path: file.clone(),
                write: true
            }
        );
        assert_eq!(
            touch(&ws, "read_file", &json!({"path": "a.txt"})),
            Touch::Path {
                path: file,
                write: false
            }
        );
        // A directory tool with no path reads the whole workspace.
        assert_eq!(
            touch(&ws, "grep", &json!({"pattern": "x"})),
            Touch::Path {
                path: root,
                write: false
            }
        );
        assert_eq!(
            touch(&ws, "bash", &json!({"command": "ls"})),
            Touch::Command(ws.dir().to_path_buf())
        );
        // A file tool without its path, a path outside the workspace, and a tool the harness does not order.
        assert_eq!(touch(&ws, "read_file", &json!({})), Touch::Other);
        assert_eq!(
            touch(&ws, "read_file", &json!({"path": "../outside"})),
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
            call(0, "r1", "read_file", "{\"path\": \"a.txt\"}"),
        );
        assert_eq!(
            ahead,
            vec![Whole {
                id: "r1".into(),
                name: "read_file".into(),
                input: json!({"path": "a.txt"}),
            }]
        );
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(
                    1,
                    "w1",
                    "edit_file",
                    "{\"path\": \"a.txt\", \"old\": \"x\", \"new\": \"y\"}"
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
                    "edit_file",
                    "{\"path\": \"a.txt\", \"old\": \"x\", \"new\": \"y\"}"
                ),
            )
            .is_empty()
        );
        // The write before it touches the same file: the read waits for it.
        assert!(
            feed(
                &mut streamed,
                &ws,
                call(1, "r1", "read_file", "{\"path\": \"a.txt\"}")
            )
            .is_empty()
        );
        // Another file is read ahead, whatever came before it.
        assert_eq!(
            feed(
                &mut streamed,
                &ws,
                call(2, "r2", "read_file", "{\"path\": \"b.txt\"}")
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
                    name: "read_file".into(),
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
                call(1, "r1", "read_file", "{\"path\": \"a.txt\"}")
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
                call(0, "", "read_file", "{\"path\": \"a.txt\"}")
            )
            .is_empty()
        );
        assert!(
            feed(
                &mut Streamed::default(),
                &ws,
                call(0, "r1", "read_file", "[\"a.txt\"]")
            )
            .is_empty()
        );
        assert!(
            feed(
                &mut Streamed::default(),
                &ws,
                call(0, "r1", "read_file", "{\"path\": \"a.txt\"")
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
                    name: "read_file".into(),
                    index: 3,
                    id: "r3".into(),
                },
                Delta::ToolInput {
                    index: 3,
                    json: "{\"path\":".into(),
                },
                Delta::Text { text: "…".into() },
                Delta::ToolInput {
                    index: 3,
                    json: " \"a.txt\"}".into(),
                },
                Delta::BlockEnd { index: 3 },
            ],
        );
        assert_eq!(ahead[0].input, json!({"path": "a.txt"}));
        let listing = feed(
            &mut streamed,
            &ws,
            vec![
                Delta::ToolUse {
                    name: "list".into(),
                    index: 4,
                    id: "l4".into(),
                },
                Delta::BlockEnd { index: 4 },
            ],
        );
        assert_eq!(listing[0].input, json!({}));
    }
}
