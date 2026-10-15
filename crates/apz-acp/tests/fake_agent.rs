//! End to end: the `apz-acp-fake-agent` binary, driven by [`FakeClient`] over its stdio.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use apz_acp::rpc;
use apz_acp::testing::{ClientConfig, FakeClient, PermissionAnswer};
use apz_acp::types::{PermissionOptionKind, ProtocolVersion, SessionUpdate, StopReason};
use serde_json::{Value, json};

/// Kills the agent when a test ends, whatever happened, so no fake agent outlives its test.
struct Running {
    child: Child,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Writes `script` for the test `name` and starts the fake agent on it. The answers it logs go to a file of
/// the same name. The files live in cargo's per-target temp directory, so parallel tests do not collide.
fn start(name: &str, script: &Value, config: ClientConfig) -> (FakeClient, Running, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let script_path = dir.join(format!("{name}.script.json"));
    let log_path = dir.join(format!("{name}.log.json"));
    fs::write(&script_path, serde_json::to_vec_pretty(script).unwrap()).unwrap();
    let _ = fs::remove_file(&log_path);
    // Under cargo's runner for this target (qemu in CI's arm64 job), the agent runs under it too.
    let argv = apz_acp::testing::helper_argv(Path::new(env!("CARGO_BIN_EXE_apz-acp-fake-agent")));
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .arg(&script_path)
        .env("APZ_ACP_FAKE_LOG", &log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the fake agent starts");
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let client = FakeClient::spawn(stdout, stdin, config);
    (client, Running { child }, log_path)
}

/// The answers the fake agent logged, in the order it received them.
fn logged_answers(log_path: &Path) -> Vec<Value> {
    let bytes = fs::read(log_path).expect("the agent wrote its log");
    serde_json::from_slice::<Vec<Value>>(&bytes).expect("the log is a JSON array")
}

fn text_chunk(text: &str) -> Value {
    json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})
}

fn permission_request(call: &str) -> Value {
    json!({
        "permission": {
            "toolCall": {"toolCallId": call, "title": "Edit a.txt", "kind": "edit"},
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        }
    })
}

#[test]
fn initialize_and_new_session_are_answered() {
    let (client, _running, _log) =
        start("initialize", &json!({"steps": []}), ClientConfig::default());
    let initialized = client.initialize().expect("initialize");
    assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
    assert!(initialized.agent_capabilities.load_session);
    let session = client.new_session("/work").expect("session/new");
    assert_eq!(session.to_string(), "fake-1");
}

#[test]
fn a_turn_streams_updates_and_answers_every_client_request() {
    let script = json!({
        "steps": [
            {"update": text_chunk("Hello")},
            {"update": {"sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "Edit a.txt", "kind": "edit", "status": "pending"}},
            permission_request("call-1"),
            {"read": {"path": "/work/a.txt"}},
            {"write": {"path": "/work/b.txt", "content": "new text"}},
            {"terminal": {"command": "echo", "args": ["hi"]}},
            {"update": {"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed"}},
        ],
        "stopReason": "end_turn",
    });
    let config = ClientConfig {
        files: HashMap::from([("/work/a.txt".to_owned(), "original".to_owned())]),
        terminal_output: "hi\n".to_owned(),
        ..ClientConfig::default()
    };
    let (client, _running, log_path) = start("full_turn", &script, config);
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");

    let stop = client.prompt(&session, "do it").expect("session/prompt");
    assert_eq!(stop, StopReason::EndTurn);

    // Updates arrive in the order the agent sent them, all before the prompt returns.
    let updates = client.updates();
    assert_eq!(updates.len(), 3);
    assert!(matches!(
        &updates[0].update,
        SessionUpdate::AgentMessageChunk(_)
    ));
    assert!(matches!(&updates[1].update, SessionUpdate::ToolCall(_)));
    assert!(matches!(
        &updates[2].update,
        SessionUpdate::ToolCallUpdate(_)
    ));

    // The client saw the agent's requests in order and holds the written file.
    let methods: Vec<String> = client
        .agent_requests()
        .into_iter()
        .map(|(method, _)| method)
        .collect();
    assert_eq!(
        methods,
        [
            "session/request_permission",
            "fs/read_text_file",
            "fs/write_text_file",
            "terminal/create",
            "terminal/wait_for_exit",
            "terminal/output",
            "terminal/release",
        ]
    );
    assert_eq!(client.file("/work/b.txt").as_deref(), Some("new text"));

    // The agent's log has the client's answers: the permission outcome, the file, the terminal output.
    let answers = logged_answers(&log_path);
    assert_eq!(answers.len(), 7);
    assert_eq!(
        answers[0]["answer"]["result"]["outcome"],
        json!({"outcome": "selected", "optionId": "allow"})
    );
    assert_eq!(answers[1]["answer"]["result"]["content"], json!("original"));
    assert_eq!(answers[5]["answer"]["result"]["output"], json!("hi\n"));
}

#[test]
fn permission_answers_select_the_reject_option_or_cancel() {
    // A rejecting client selects the reject option.
    {
        let script = json!({"steps": [permission_request("call-2")], "stopReason": "end_turn"});
        let config = ClientConfig {
            permission: PermissionAnswer::Kind(PermissionOptionKind::RejectOnce),
            ..ClientConfig::default()
        };
        let (client, _running, log_path) = start("reject", &script, config);
        client.initialize().expect("initialize");
        let session = client.new_session("/work").expect("session/new");
        assert_eq!(
            client.prompt(&session, "edit").expect("prompt"),
            StopReason::EndTurn
        );
        let answers = logged_answers(&log_path);
        assert_eq!(
            answers[0]["answer"]["result"]["outcome"],
            json!({"outcome": "selected", "optionId": "reject"})
        );
    }

    // A client that cancels the permission gives a cancelled outcome.
    {
        let script = json!({"steps": [permission_request("call-3")], "stopReason": "end_turn"});
        let config = ClientConfig {
            permission: PermissionAnswer::Cancelled,
            ..ClientConfig::default()
        };
        let (client, _running, log_path) = start("permission_cancelled", &script, config);
        client.initialize().expect("initialize");
        let session = client.new_session("/work").expect("session/new");
        client.prompt(&session, "edit").expect("prompt");
        assert_eq!(
            logged_answers(&log_path)[0]["answer"]["result"]["outcome"],
            json!({"outcome": "cancelled"})
        );
    }
}

#[test]
fn a_cancel_during_a_sleep_ends_the_turn_with_cancelled() {
    let script = json!({
        "steps": [
            {"update": text_chunk("before")},
            {"sleep_ms": 10000},
            {"update": text_chunk("after")},
        ],
        "stopReason": "end_turn",
    });
    let (client, _running, _log) = start("cancel", &script, ClientConfig::default());
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");

    let started = Instant::now();
    let prompting = client.clone();
    let turn_session = session.clone();
    let turn = thread::spawn(move || prompting.prompt(&turn_session, "wait"));
    thread::sleep(Duration::from_millis(300));
    client.cancel(&session).expect("session/cancel");

    assert_eq!(turn.join().unwrap().expect("prompt"), StopReason::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the sleep was cut short"
    );
    let texts: Vec<String> = client
        .updates()
        .into_iter()
        .filter_map(|note| match note.update {
            SessionUpdate::AgentMessageChunk(chunk) => Some(format!("{:?}", chunk.content)),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 1, "the update after the sleep was not sent");
}

#[test]
fn a_turn_that_ignores_cancel_runs_to_its_own_stop_reason() {
    let script = json!({
        "steps": [
            {"ignore_cancel": true},
            {"sleep_ms": 400},
            {"update": text_chunk("after")},
        ],
        "stopReason": "end_turn",
    });
    let (client, _running, _log) = start("ignore_cancel", &script, ClientConfig::default());
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");

    let prompting = client.clone();
    let turn_session = session.clone();
    let turn = thread::spawn(move || prompting.prompt(&turn_session, "stubborn"));
    thread::sleep(Duration::from_millis(100));
    client.cancel(&session).expect("session/cancel");

    assert_eq!(turn.join().unwrap().expect("prompt"), StopReason::EndTurn);
    assert_eq!(client.updates().len(), 1);
}

#[test]
fn successive_prompts_play_successive_turns_and_the_last_repeats() {
    let script = json!({
        "turns": [
            {"steps": [{"update": text_chunk("one")}], "stopReason": "end_turn"},
            {"steps": [], "stopReason": "max_tokens"},
        ],
    });
    let (client, _running, _log) = start("turns", &script, ClientConfig::default());
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");
    assert_eq!(
        client.prompt(&session, "a").expect("first"),
        StopReason::EndTurn
    );
    assert_eq!(
        client.prompt(&session, "b").expect("second"),
        StopReason::MaxTokens
    );
    assert_eq!(
        client.prompt(&session, "c").expect("third"),
        StopReason::MaxTokens
    );
    assert_eq!(client.updates().len(), 1);
}

#[test]
fn an_agent_that_crashes_mid_turn_fails_the_prompt_as_closed() {
    let script = json!({"steps": [{"update": text_chunk("partial")}, {"crash": true}]});
    let (client, _running, _log) = start("crash", &script, ClientConfig::default());
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");
    assert!(matches!(
        client.prompt(&session, "boom"),
        Err(rpc::Error::Closed)
    ));
}

#[test]
fn a_step_that_is_not_a_valid_update_fails_the_turn_with_invalid_params() {
    let script = json!({"steps": [{"update": {"sessionUpdate": "not_a_real_update"}}]});
    let (client, _running, _log) = start("bad_update", &script, ClientConfig::default());
    client.initialize().expect("initialize");
    let session = client.new_session("/work").expect("session/new");
    match client.prompt(&session, "bad") {
        Err(rpc::Error::Remote(error)) => assert_eq!(i32::from(error.code), -32602),
        other => panic!("expected an invalid params error, got {other:?}"),
    }
}
