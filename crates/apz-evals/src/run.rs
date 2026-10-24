//! One task on its own scratch stack: its project and task are made through apid's API as an owner would, its
//! workspace is the task's fixture, and it runs until the task is done (or the time is up). Then the task's check
//! decides its outcome, and its runs and the tools' latencies are read back from the hub.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cli::Options;
use crate::report::{Report, TaskReport, Tokens, ToolLatency};
use crate::stack::{Binaries, Stack};
use crate::tasks::{self, Task};

/// The workspace the harness works in (its `WORKSPACE_ROOT`): cleared and seeded from the fixture before each task.
pub const WORKSPACE: &str = "/workspace";
/// The budget a project and a task get: far above what any task spends, so the budget never pauses a run.
const CAP: u64 = 10_000_000;
/// How long one call to apid may take.
const CALL: Duration = Duration::from_secs(30);

/// Runs the whole suite, one task after another, each on a fresh stack.
pub fn suite(options: &Options, tasks: &[Task], roles: &Path) -> Result<Report, String> {
    let bins = Binaries::in_dir(&options.bin_dir);
    let scratch_root = std::env::temp_dir().join(format!("apz-evals-{}", std::process::id()));
    let mut report = Report {
        model: options.model.clone(),
        tasks: Vec::new(),
    };
    for task in tasks {
        let scratch = scratch_root.join(&task.name);
        let _ = fs::remove_dir_all(&scratch);
        fs::create_dir_all(&scratch).map_err(|err| format!("{}: {err}", scratch.display()))?;
        report
            .tasks
            .push(one(options, &bins, &scratch, roles, task)?);
    }
    Ok(report)
}

/// Runs one task in `scratch`.
fn one(
    options: &Options,
    bins: &Binaries,
    scratch: &Path,
    roles: &Path,
    task: &Task,
) -> Result<TaskReport, String> {
    reset_workspace(&task.fixture)?;
    let script = scratch.join("script.json");
    let mock = options.model == "mock";
    if mock {
        let text = serde_json::to_string_pretty(&tasks::script_for(task)).unwrap_or_default();
        fs::write(&script, text).map_err(|err| format!("{}: {err}", script.display()))?;
    }
    let (builder, reviewer) = if mock {
        ("mock/script".to_string(), "mock/script-review".to_string())
    } else {
        (options.model.clone(), options.model.clone())
    };
    let stack = Stack::start(options, bins, scratch, &script, roles)?;
    let port = stack.apid_port;
    let timeout = Duration::from_secs(options.timeout_s);
    if mock {
        use_mock_models(port)?;
    }

    let project = created(
        port,
        "/v1/projects",
        &json!({
            "title": task.name,
            "goal": task.goal,
            "kind": "other",
        }),
    )?;
    let pid = text_of(&project["id"]);
    expect_ok(
        port,
        "PUT",
        &format!("/v1/projects/{pid}/budget"),
        json!({ "unit": "tokens", "cap": CAP }),
    )?;
    expect_ok(
        port,
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        json!({ "role_models": { "builder": builder, "reviewer": reviewer } }),
    )?;
    let made = created(
        port,
        "/v1/tasks",
        &json!({ "goal": task.goal, "project": pid }),
    )?;
    let tid = text_of(&made["id"]);
    expect_ok(
        port,
        "PUT",
        &format!("/v1/tasks/{tid}/budget"),
        json!({ "unit": "tokens", "cap": CAP }),
    )?;

    let done = wait_until_true(timeout, || state_of(port, &tid) == "done");
    let state = state_of(port, &tid);
    // A reviewer's session records its run when it ends, a little after the task is done.
    let _ = wait_until_true(Duration::from_secs(30), || all_sessions_ended(port, &tid));

    let check_exit = run_check(&task.check)?;
    let outcome = match (done, check_exit) {
        (false, _) => "timeout",
        (true, Some(0)) => "pass",
        (true, _) => "fail",
    };
    let runs = runs_of(port, &pid);
    let tools = tool_latency(port);
    drop(stack);
    Ok(summarise(task, outcome, state, check_exit, &runs, tools))
}

/// Makes the scripted models the only ones the hub picks for a seat that names none. The recipe's sceptics are on
/// models other than the builder's, so the second one falls through the hub's worker list to a provider the suite has
/// no key for; the hub's default model and its worker policy are its settings, so they are set here, on this stack.
fn use_mock_models(port: u16) -> Result<(), String> {
    let (status, mut settings) = call(port, "GET", "/v1/settings", None)?;
    if status != 200 {
        return Err(format!("GET /v1/settings: {status} {settings}"));
    }
    let object = settings
        .as_object_mut()
        .ok_or("GET /v1/settings: not an object")?;
    object.remove("dev_models");
    object.insert("default_model".into(), json!("mock/script-review"));
    let policy = object
        .get_mut("model_policy")
        .and_then(Value::as_object_mut)
        .ok_or("GET /v1/settings: no model_policy")?;
    policy.insert(
        "worker".into(),
        json!(["mock/script", "mock/script-review"]),
    );
    expect_ok(port, "PUT", "/v1/settings", settings)
}

/// The task's report from its runs (the eval records of its project's step sessions) and its tools' latencies.
fn summarise(
    task: &Task,
    outcome: &str,
    state: String,
    check_exit: Option<i32>,
    runs: &[Value],
    tools: Vec<ToolLatency>,
) -> TaskReport {
    let sum = |field: &str| {
        runs.iter()
            .map(|run| run[field].as_u64().unwrap_or(0))
            .sum::<u64>()
    };
    TaskReport {
        task: task.name.clone(),
        outcome: outcome.to_string(),
        state,
        check_exit,
        runs: runs.len() as u64,
        tool_calls: sum("tool_calls"),
        model_turns: sum("model_calls"),
        tokens: Tokens {
            input: sum("tokens_in"),
            output: sum("tokens_out"),
            cached: sum("tokens_cached"),
        },
        dollars: sum("cost_micros") as f64 / 1_000_000.0,
        wall_ms: sum("wall_ms"),
        tool_ms: sum("tool_ms"),
        tools,
    }
}

/// Clears the workspace and copies the fixture into it.
fn reset_workspace(fixture: &Path) -> Result<(), String> {
    let workspace = Path::new(WORKSPACE);
    if workspace.exists() {
        for entry in fs::read_dir(workspace).map_err(|err| format!("{WORKSPACE}: {err}"))? {
            let path = entry.map_err(|err| format!("{WORKSPACE}: {err}"))?.path();
            let removed = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            removed.map_err(|err| format!("{}: {err}", path.display()))?;
        }
    }
    fs::create_dir_all(workspace).map_err(|err| format!("{WORKSPACE}: {err}"))?;
    copy_tree(fixture, workspace)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    for entry in fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))? {
        let entry = entry.map_err(|err| format!("{}: {err}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            fs::create_dir_all(&target).map_err(|err| format!("{}: {err}", target.display()))?;
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|err| format!("{}: {err}", target.display()))?;
        }
    }
    Ok(())
}

/// Runs the task's check in the workspace; its exit status, or None when it was killed by a signal.
fn run_check(check: &str) -> Result<Option<i32>, String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(check)
        .current_dir(WORKSPACE)
        .status()
        .map_err(|err| format!("the check: {err}"))?;
    Ok(status.code())
}

/// Calls apid as the owner and returns the reply's body, or an error naming the call when it fails.
fn call(port: u16, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value), String> {
    let bytes = body.map(|body| body.to_string());
    let reply = apz_http::client::send_with(
        port,
        method,
        path,
        &[],
        bytes.as_deref().map(str::as_bytes),
        CALL,
    )
    .map_err(|err| format!("{method} {path}: {err}"))?;
    Ok((
        reply.status,
        serde_json::from_slice(&reply.body).unwrap_or(Value::Null),
    ))
}

fn created(port: u16, path: &str, body: &Value) -> Result<Value, String> {
    let (status, value) = call(port, "POST", path, Some(body.clone()))?;
    if status != 201 {
        return Err(format!("POST {path}: {status} {value}"));
    }
    Ok(value)
}

fn expect_ok(port: u16, method: &str, path: &str, body: Value) -> Result<(), String> {
    let (status, value) = call(port, method, path, Some(body))?;
    if status != 200 {
        return Err(format!("{method} {path}: {status} {value}"));
    }
    Ok(())
}

fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn state_of(port: u16, tid: &str) -> String {
    call(port, "GET", &format!("/v1/tasks/{tid}"), None)
        .map(|(_, task)| text_of(&task["state"]))
        .unwrap_or_default()
}

/// Whether every session of the task has ended or failed.
fn all_sessions_ended(port: u16, tid: &str) -> bool {
    call(port, "GET", &format!("/v1/sessions?task={tid}"), None)
        .map(|(_, list)| {
            list.as_array().is_some_and(|sessions| {
                sessions
                    .iter()
                    .all(|s| matches!(s["state"].as_str(), Some("ended" | "failed")))
            })
        })
        .unwrap_or(false)
}

fn runs_of(port: u16, pid: &str) -> Vec<Value> {
    call(
        port,
        "GET",
        &format!("/v1/evals/runs?project={pid}&limit=200"),
        None,
    )
    .ok()
    .and_then(|(_, runs)| runs.as_array().cloned())
    .unwrap_or_default()
}

fn tool_latency(port: u16) -> Vec<ToolLatency> {
    call(port, "GET", "/v1/evals/tools", None)
        .ok()
        .and_then(|(_, tools)| serde_json::from_value(tools).ok())
        .unwrap_or_default()
}

/// Polls `ready` for at most `timeout`; whether it held.
fn wait_until_true(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if ready() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
