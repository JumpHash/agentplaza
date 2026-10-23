//! The thin adapters of the six engines pinned in 5.6 (Amp, Crush, Junie, Droid, Aider, OpenHands): an ACP agent that
//! runs the engine's non-interactive command for each prompt turn in the session's folder, and sends the command's
//! output back as the turn's text. It is the adapter path (PLAN 12.5) for an engine whose ACP mode is not verified.
//!
//! The relay speaks ACP over stdio as JSON-RPC 2.0, one message per line: `initialize`, `session/new`, `session/prompt`
//! (answered after one `session/update` notification with the output), and ignores notifications such as
//! `session/cancel`. The flags of each engine are in [`SHIMS`]; where the docs read did not settle one, it is marked
//! unverified there.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The protocol version the relay answers with.
const PROTOCOL: u64 = 1;
/// The JSON-RPC error codes the relay uses (`-32601` method not found, `-32000` a turn that failed).
const NOT_FOUND: i64 = -32601;
const TURN_FAILED: i64 = -32000;
/// The word in an argument that the turn's text replaces.
const PROMPT: &str = "{prompt}";

/// One engine's thin adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shim {
    /// The catalogue id.
    pub engine: &'static str,
    /// The engine's command, its binary first, with [`PROMPT`] where the turn's text goes.
    pub argv: &'static [&'static str],
    /// Whether the docs read settle these flags. None of them is settled: each is the safest form the docs suggest,
    /// or a guess, and the source says which.
    pub verified: bool,
    /// Where the flags come from.
    pub source: &'static str,
}

/// The six thin adapters. The docs read (metadata only) settle none of the flags; each is marked unverified.
pub const SHIMS: &[Shim] = &[
    Shim {
        engine: "amp",
        argv: &["amp", "-x", PROMPT],
        verified: false,
        source: "Amp's execute mode `-x`, from memory: the readme of the pinned npm package is only an alias for @ampcode/cli, which was not read",
    },
    Shim {
        engine: "crush",
        argv: &["crush", "run", PROMPT],
        verified: false,
        source: "a guess: the Crush readme read documents its MCP servers and --yolo, but no non-interactive command",
    },
    Shim {
        engine: "junie",
        argv: &["junie", "--task", PROMPT],
        verified: false,
        source: "a guess: the npm package has no readme (npm view returns none), so no docs were read",
    },
    Shim {
        engine: "droid",
        argv: &["droid", "exec", PROMPT],
        verified: false,
        source: "a guess: the Factory readme names the `droid` command and documents no non-interactive mode",
    },
    Shim {
        engine: "aider",
        argv: &[
            "aider",
            "--message",
            PROMPT,
            "--yes-always",
            "--no-auto-commits",
        ],
        verified: false,
        source: "from memory: the PyPI description shows --model and --api-key only; --message and --yes-always are not in it",
    },
    Shim {
        engine: "openhands",
        argv: &["openhands", "--task", PROMPT],
        verified: false,
        source: "a guess: the PyPI description documents OpenHands as an ACP client of other agents, not a command-line mode",
    },
];

/// The shim named `engine`.
pub fn shim(engine: &str) -> Option<&'static Shim> {
    SHIMS.iter().find(|shim| shim.engine == engine)
}

/// The turn's text: the text blocks of an ACP prompt, joined by a blank line.
fn prompt_text(prompt: &Value) -> String {
    prompt
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

/// The command for `text`: `argv` with [`PROMPT`] replaced by the text, run in `cwd`. Its output (stdout, then stderr)
/// on success, or why it failed.
pub fn run(argv: &[String], text: &str, cwd: &Path) -> Result<String, String> {
    let args: Vec<String> = argv
        .iter()
        .map(|arg| {
            if arg == PROMPT {
                text.to_string()
            } else {
                arg.clone()
            }
        })
        .collect();
    let (program, rest) = args.split_first().ok_or("the shim has no command")?;
    let out = Command::new(program)
        .args(rest)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot start {program}: {err}"))?;
    let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
    output.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "{program} exited with {}: {}",
            out.status,
            output.trim()
        ))
    }
}

/// Serves ACP for one shim over `input` and `output` until `input` ends. `argv` is the command the turns run (the
/// shim's own [`Shim::argv`], or the same with another binary in a test). A `session/new` opens a session in the folder
/// it names; a `session/prompt` runs the command there and answers `end_turn` after its output is sent.
pub fn serve(argv: &[String], input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut sessions: Vec<(String, String)> = Vec::new();
    for line in input.lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned();
        let params = &message["params"];
        let reply: Result<Value, (i64, String)> = match message["method"].as_str() {
            Some("initialize") => Ok(json!({
                "protocolVersion": PROTOCOL,
                "agentCapabilities": {},
                "authMethods": [],
            })),
            Some("session/new") => {
                let cwd = params["cwd"].as_str().unwrap_or(".").to_string();
                let session = format!("shim-{}", sessions.len() + 1);
                sessions.push((session.clone(), cwd));
                Ok(json!({ "sessionId": session }))
            }
            Some("session/prompt") => {
                let session = params["sessionId"].as_str().unwrap_or("");
                match sessions.iter().find(|(s, _)| s == session) {
                    None => Err((TURN_FAILED, format!("no session {session}"))),
                    Some((_, cwd)) => {
                        let text = prompt_text(&params["prompt"]);
                        match run(argv, &text, Path::new(cwd)) {
                            Ok(reply) => {
                                notify(&mut output, session, &reply)?;
                                Ok(json!({ "stopReason": "end_turn" }))
                            }
                            Err(why) => Err((TURN_FAILED, why)),
                        }
                    }
                }
            }
            // A notification (no id) that the relay does not act on is dropped; a request it does not know is refused.
            Some(_) if id.is_none() => continue,
            Some(method) => Err((NOT_FOUND, format!("method not found: {method}"))),
            None => continue,
        };
        if let Some(id) = id {
            let message = match reply {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
                }
            };
            writeln!(output, "{message}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// The turn's output as one `session/update` notification of agent text.
fn notify(output: &mut impl Write, session: &str, text: &str) -> io::Result<()> {
    let message = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text },
            },
        },
    });
    writeln!(output, "{message}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The stand-in engine: it performs the 12.5 scripted steps in its folder, and stands in for the real binary.
    const STAND_IN: &str = "#!/bin/sh
echo \"start: ok\"
echo \"read: ok\"
echo edited > edited.txt
echo \"edit: ok\"
echo \"test: ok\"
echo \"mcp: plaza tool called\"
echo handover > HANDOVER.md
echo \"handover: written\"
echo \"end: ok\"
";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apz-harness-acp-shim-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    /// A stand-in engine script in `dir`, with `body` as its commands.
    fn stand_in(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("the stand-in");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        path.display().to_string()
    }

    /// The shim's command with its binary replaced by `program`: the flags stay as the docs give them.
    fn argv_with(shim: &Shim, program: &str) -> Vec<String> {
        let mut argv: Vec<String> = shim.argv.iter().map(|arg| arg.to_string()).collect();
        argv[0] = program.to_string();
        argv
    }

    /// The scripted session: initialize, open a session in `cwd`, one prompt. Returns the reply lines, parsed.
    fn session(argv: &[String], cwd: &Path) -> Vec<Value> {
        let input = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": cwd.display().to_string()}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
                "sessionId": "shim-1",
                "prompt": [{"type": "text", "text": "fix the failing test"}],
            }}),
        ]
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let mut output = Vec::new();
        serve(argv, input.as_bytes(), &mut output).expect("the relay serves");
        String::from_utf8(output)
            .expect("utf-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON message"))
            .collect()
    }

    fn reply(lines: &[Value], id: u64) -> &Value {
        lines
            .iter()
            .find(|m| m["id"] == json!(id))
            .unwrap_or_else(|| panic!("a reply to {id}: {lines:?}"))
    }

    #[test]
    fn each_engine_has_one_thin_adapter_with_its_flags_marked_unverified() {
        let engines: Vec<&str> = SHIMS.iter().map(|s| s.engine).collect();
        assert_eq!(
            engines,
            ["amp", "crush", "junie", "droid", "aider", "openhands"]
        );
        for shim in SHIMS {
            assert!(
                !shim.verified,
                "{}: no flag is settled by the docs read",
                shim.engine
            );
            assert!(
                shim.argv.contains(&PROMPT),
                "{} takes the turn's text",
                shim.engine
            );
            assert!(
                !shim.source.is_empty(),
                "{} says where its flags come from",
                shim.engine
            );
            assert_eq!(super::shim(shim.engine), Some(shim));
        }
        assert_eq!(super::shim("claude"), None);
    }

    #[test]
    fn a_scripted_session_runs_each_engine_through_its_stand_in() {
        // The 12.5 steps: start, read, edit, run a test, call a plaza MCP tool, write a handover, end. Each engine's
        // flags are exercised; only its binary is the stand-in.
        for shim in SHIMS {
            let dir = scratch(shim.engine);
            let workspace = dir.join("workspace");
            std::fs::create_dir_all(&workspace).expect("the session folder");
            let program = stand_in(&dir, &format!("{}-stand-in", shim.engine), STAND_IN);
            let lines = session(&argv_with(shim, &program), &workspace);

            assert_eq!(
                reply(&lines, 1)["result"]["protocolVersion"],
                json!(1),
                "{}",
                shim.engine
            );
            assert_eq!(
                reply(&lines, 2)["result"]["sessionId"],
                json!("shim-1"),
                "{}",
                shim.engine
            );
            assert_eq!(
                reply(&lines, 3)["result"]["stopReason"],
                json!("end_turn"),
                "{}",
                shim.engine
            );
            let text: String = lines
                .iter()
                .filter(|m| m["method"] == json!("session/update"))
                .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
                .collect();
            for step in [
                "start: ok",
                "read: ok",
                "edit: ok",
                "test: ok",
                "mcp: plaza tool called",
                "handover: written",
                "end: ok",
            ] {
                assert!(
                    text.contains(step),
                    "{} reports `{step}`: {text}",
                    shim.engine
                );
            }
            assert!(
                workspace.join("edited.txt").exists(),
                "{} edited in the session folder",
                shim.engine
            );
            assert!(
                workspace.join("HANDOVER.md").exists(),
                "{} wrote a handover",
                shim.engine
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_failing_engine_is_an_error_reply_and_an_unknown_request_is_refused() {
        let dir = scratch("failing");
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).expect("the session folder");
        let program = stand_in(&dir, "broken", "#!/bin/sh\necho boom\nexit 3\n");
        let lines = session(&argv_with(&SHIMS[0], &program), &workspace);
        let error = &reply(&lines, 3)["error"];
        assert_eq!(error["code"], json!(TURN_FAILED));
        assert!(
            error["message"].as_str().unwrap().contains("boom"),
            "{error}"
        );

        let mut output = Vec::new();
        let request = json!({"jsonrpc": "2.0", "id": 9, "method": "fs/unknown"}).to_string();
        serve(&[], request.as_bytes(), &mut output).expect("the relay serves");
        let refused: Value = serde_json::from_slice(&output).expect("a reply");
        assert_eq!(refused["error"]["code"], json!(NOT_FOUND));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
