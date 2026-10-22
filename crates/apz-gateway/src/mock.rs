//! The `mock` provider: deterministic and offline, so the plaza's whole loop
//! runs without a model API.
//!
//! * `echo` replies with the last user text.
//! * `agent` (the default) acts like a tiny competent agent: given a new
//!   prompt it calls the first sensible tool it was offered — `plaza_vote`
//!   when the prompt names a motion (`m3`), else `edit` (a note under
//!   `notes/`, created), else `run` (`ls -la /workspace`) — and once it sees that
//!   tool's result it ends the turn with a summary of what it did.
//! * `script` and `script-review` answer from a script: the file the
//!   `APZ_MOCK_SCRIPT` variable names (the eval suite's tasks, see
//!   `crates/apz-evals`). They are read on each call, so a run may rewrite them.

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) const MODELS: [&str; 4] = ["agent", "echo", "script", "script-review"];

pub(crate) fn complete(
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> ChatResponse {
    let model = match model {
        "echo" | "script" | "script-review" => model,
        _ => "agent",
    };
    let (text, call) = match model {
        "echo" => (last_user_text(&req.messages), None),
        "script" | "script-review" => scripted(req),
        _ => agent(req),
    };
    for chunk in text.split_inclusive(' ') {
        sink(Delta::Text {
            text: chunk.to_string(),
        });
    }
    let mut content = Vec::new();
    let mut output_chars = text.len();
    if !text.is_empty() {
        content.push(Block::Text { text });
    }
    let stop = match call {
        Some((name, input)) => {
            output_chars += name.len() + input.to_string().len();
            let id = format!("mock_{}", crate::tool_use_count(&req.messages));
            // The call comes whole: its start, its input and its end go out together.
            let index = content.len();
            sink(Delta::ToolUse {
                name: name.to_string(),
                index,
                id: id.clone(),
            });
            sink(Delta::ToolInput {
                index,
                json: input.to_string(),
            });
            sink(Delta::BlockEnd { index });
            content.push(Block::ToolUse {
                id,
                name: name.to_string(),
                input,
                meta: None,
            });
            StopReason::ToolUse
        }
        None => StopReason::EndTurn,
    };
    let usage = Usage {
        input_tokens: tokens(input_chars(req)),
        output_tokens: tokens(output_chars),
        ..Usage::default()
    };
    ChatResponse {
        model: model.to_string(),
        content,
        stop,
        usage,
    }
}

/// The agent's reply text and tool call.
fn agent(req: &ChatRequest) -> (String, Option<(String, Value)>) {
    let last = req.messages.last();
    let result = last.filter(|m| m.role == Role::User).and_then(|m| {
        m.content.iter().find_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id, content, *is_error)),
            _ => None,
        })
    });
    if let Some((id, output, failed)) = result {
        return (summary(&req.messages, id, output, failed), None);
    }
    let prompt = last_user_text(&req.messages);
    let offered = |name: &str| req.tools.iter().any(|t| t.name == name);
    let motion = motion_id(&prompt);
    if let (Some(motion), true) = (&motion, offered("plaza_vote")) {
        let reason = "The mock agent supports it: the motion is clear and cheap to try.";
        let input = json!({"motion": motion, "vote": "yes", "reason": reason});
        return (
            format!("I'll vote yes on {motion}."),
            Some(("plaza_vote".into(), input)),
        );
    }
    if offered("edit") {
        let path = format!("notes/{}.md", slug(&prompt));
        let content = format!(
            "# Notes\n\nTask: {}\n\nThe mock agent wrote this note.\n",
            excerpt(&prompt, 400)
        );
        return (
            format!("I'll write a note to {path}."),
            Some((
                "edit".into(),
                json!({"create": [{"path": path, "content": content}]}),
            )),
        );
    }
    if offered("run") {
        return (
            "I'll look at the workspace.".into(),
            Some(("run".into(), json!({"command": "ls -la /workspace"}))),
        );
    }
    (
        format!(
            "Mock agent here; no tools to use. You asked: {}",
            excerpt(&prompt, 200)
        ),
        None,
    )
}

/// A one-line account of the call `id` and how it went.
fn summary(messages: &[Message], id: &str, output: &str, failed: bool) -> String {
    let call = messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            Block::ToolUse {
                id: call_id,
                name,
                input,
                ..
            } if call_id == id => Some((name.as_str(), input)),
            _ => None,
        });
    let did = match call {
        Some(("edit", input)) => {
            format!(
                "wrote {}",
                input["create"][0]["path"].as_str().unwrap_or("a note")
            )
        }
        Some(("plaza_vote", input)) => {
            format!(
                "voted {} on {}",
                input["vote"].as_str().unwrap_or("yes"),
                input["motion"].as_str().unwrap_or("the motion")
            )
        }
        Some(("run", input)) => {
            format!("ran `{}`", input["command"].as_str().unwrap_or("a command"))
        }
        Some((name, _)) => format!("used {name}"),
        None => "used a tool".into(),
    };
    if failed {
        let first = output.lines().next().unwrap_or("");
        format!(
            "I tried: {did}, but it failed ({}). Stopping here.",
            excerpt(first, 120)
        )
    } else {
        format!("Done: I {did}.")
    }
}

/// One entry of a script: it answers a conversation whose user text holds every `when` text, turn by turn.
#[derive(Debug, Deserialize)]
pub struct Entry {
    when: Vec<String>,
    turns: Vec<Turn>,
}

/// One answer of a script: a tool call, or the text that ends the turn.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Turn {
    Call { name: String, input: Value },
    Text { text: String },
}

/// The scripted models' answer: the script `APZ_MOCK_SCRIPT` names, read now.
fn scripted(req: &ChatRequest) -> (String, Option<(String, Value)>) {
    match read_script() {
        Ok(entries) => answer(&entries, req),
        Err(err) => (format!("The scripted model has no script: {err}"), None),
    }
}

fn read_script() -> Result<Vec<Entry>, String> {
    let path =
        std::env::var("APZ_MOCK_SCRIPT").map_err(|_| "APZ_MOCK_SCRIPT is not set".to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?;
    serde_json::from_str(&text).map_err(|err| format!("{path}: {err}"))
}

/// The answer to the model's next turn: the first entry whose texts all appear in the user's text answers, with its
/// turn in order (the turns the model has taken so far are the assistant messages).
fn answer(entries: &[Entry], req: &ChatRequest) -> (String, Option<(String, Value)>) {
    let user: String = req
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .map(Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    let Some(entry) = entries
        .iter()
        .find(|e| e.when.iter().all(|text| user.contains(text.as_str())))
    else {
        return (
            "The scripted model has no entry for this conversation.".into(),
            None,
        );
    };
    let turn = req
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    let task = task_of(&user).unwrap_or_default();
    match entry.turns.get(turn) {
        Some(Turn::Call { name, input }) => {
            (String::new(), Some((name.clone(), named_task(input, task))))
        }
        Some(Turn::Text { text }) => (text.clone(), None),
        None => ("Done.".into(), None),
    }
}

/// The task a conversation is about: the id its prompt names (`Task <id>: …`).
fn task_of(user: &str) -> Option<&str> {
    user.lines()
        .find_map(|line| line.strip_prefix("Task "))?
        .split(':')
        .next()
}

/// A scripted call's input with each `$TASK` string replaced by the task's id: a script does not know the ids the hub
/// gives its tasks.
fn named_task(input: &Value, task: &str) -> Value {
    match input {
        Value::String(text) if text == "$TASK" => Value::String(task.to_string()),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| named_task(item, task)).collect())
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), named_task(value, task)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn last_user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(Message::text)
        .unwrap_or_default()
}

/// The first motion id (`m` + digits) mentioned in `text`.
fn motion_id(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| {
            w.len() > 1 && w.starts_with(['m', 'M']) && w[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .map(|w| w.to_ascii_lowercase())
}

/// A file-name slug of the first words of `text`.
fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(6)
        .map(str::to_lowercase)
        .collect();
    let slug = crate::error::truncate(&words.join("-"), 48)
        .trim_end_matches('…')
        .trim_end_matches('-')
        .to_string();
    if slug.is_empty() { "note".into() } else { slug }
}

fn excerpt(text: &str, max: usize) -> String {
    crate::error::truncate(text.trim(), max)
}

fn input_chars(req: &ChatRequest) -> usize {
    let blocks: usize = req
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .map(|b| match b {
            Block::Text { text } | Block::Thinking { text } => text.len(),
            Block::ToolResult { content, .. } => content.len(),
            Block::ToolUse { input, .. } => input.to_string().len(),
            Block::Opaque { raw, .. } => raw.to_string().len(),
        })
        .sum();
    let tools: usize = req
        .tools
        .iter()
        .map(|t| t.name.len() + t.description.len() + t.input_schema.to_string().len())
        .sum();
    blocks + tools + req.system.as_ref().map_or(0, String::len)
}

/// ≈ chars / 4, at least 1.
fn tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::request;
    use crate::{Gateway, ProviderConfig, ProviderKind};
    use apz_proto::chat::ToolSpec;

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("the {name} tool"),
            input_schema: json!({"type": "object"}),
        }
    }

    fn mock() -> Gateway {
        Gateway::new(
            vec![ProviderConfig::new("mock", ProviderKind::Mock)],
            |_: &str, _: &crate::SecretRead<'_>| None,
        )
    }

    #[test]
    fn mock_agent_runs_a_tool_then_summarises() {
        let gw = mock();
        let mut req = ChatRequest {
            model: "mock/agent".into(),
            tools: vec![tool("run"), tool("edit")],
            ..request("Summarise the Q3 plan for the council")
        };
        let mut deltas = Vec::new();
        let first = gw.complete(&req, &mut |d| deltas.push(d)).unwrap();
        assert_eq!(first.model, "mock/agent");
        assert_eq!(first.stop, StopReason::ToolUse);
        let (id, name, input) = first.tool_uses().next().unwrap();
        assert_eq!(name, "edit");
        assert_eq!(
            input["create"][0]["path"],
            "notes/summarise-the-q3-plan-for-the.md"
        );
        assert!(first.usage.input_tokens > 0 && first.usage.output_tokens > 0);
        assert!(
            deltas.iter().any(|d| matches!(d, Delta::Text { .. }))
                && deltas.iter().any(|d| matches!(
                    d,
                    Delta::ToolUse { name, id: started, .. }
                        if name == "edit" && started.as_str() == id
                ))
        );

        let id = id.to_string();
        req.messages.push(Message::assistant(first.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: id,
                content: "ok".into(),
                is_error: false,
            }],
        });
        let second = gw.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(second.stop, StopReason::EndTurn);
        assert_eq!(
            second.text(),
            "Done: I wrote notes/summarise-the-q3-plan-for-the.md."
        );

        let vote = ChatRequest {
            tools: vec![tool("edit"), tool("plaza_vote")],
            ..request("Council: vote on m3 please")
        };
        let vote = gw
            .complete(
                &ChatRequest {
                    model: "mock/agent".into(),
                    ..vote
                },
                &mut |_| {},
            )
            .unwrap();
        let (_, name, input) = vote.tool_uses().next().unwrap();
        assert_eq!(
            (name, &input["motion"], &input["vote"]),
            ("plaza_vote", &json!("m3"), &json!("yes"))
        );

        let echo = gw
            .complete(
                &ChatRequest {
                    model: "mock/echo".into(),
                    ..request("hello there")
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(
            (echo.text().as_str(), echo.stop),
            ("hello there", StopReason::EndTurn)
        );
        assert_eq!(gw.list_models("mock").unwrap().len(), MODELS.len());
        let report = gw.test("mock", None).unwrap();
        assert!(report.ok && report.model == "mock/agent", "{report:?}");
    }

    #[test]
    fn a_script_answers_each_turn_of_the_first_entry_whose_texts_all_appear() {
        let entries: Vec<Entry> = serde_json::from_str(
            r#"[
                {"when": ["Task t9:", "this task's reviewer"], "turns": [
                    {"name": "plaza_verdict", "input": {"pass": true}}, {"text": "Passed."}]},
                {"when": ["Task t9:"], "turns": [{"name": "run", "input": {"command": "ls"}}]}
            ]"#,
        )
        .unwrap();
        let said = |text: &str, role: Role| Message {
            role,
            content: vec![Block::Text { text: text.into() }],
        };
        let mut reviewer = request("Task t9: Fix it.\n\nYou are this task's reviewer.");
        let (text, call) = answer(&entries, &reviewer);
        assert_eq!(text, "");
        assert_eq!(call, Some(("plaza_verdict".into(), json!({"pass": true}))));
        reviewer.messages.push(said("", Role::Assistant));
        assert_eq!(answer(&entries, &reviewer), ("Passed.".into(), None));
        reviewer.messages.push(said("Passed.", Role::Assistant));
        assert_eq!(
            answer(&entries, &reviewer).0,
            "Done.",
            "past its turns, it is done"
        );
        // A builder's conversation matches only the entry without the reviewer's text.
        let builder = request("Task t9: Fix it.\n\nYou are this task's builder.");
        assert_eq!(
            answer(&entries, &builder),
            (
                String::new(),
                Some(("run".into(), json!({"command": "ls"})))
            )
        );
        let unscripted = request("Task t8: nothing scripted");
        assert!(answer(&entries, &unscripted).0.contains("no entry"));
        // A `$TASK` in a call's input is the id of the task the prompt names.
        let verdict: Vec<Entry> = serde_json::from_str(
            r#"[{"when": ["Task t7:"], "turns": [{"name": "plaza_verdict", "input": {"task": "$TASK", "pass": true}}]}]"#,
        )
        .unwrap();
        assert_eq!(
            answer(&verdict, &request("Task t7: Check it.")).1,
            Some(("plaza_verdict".into(), json!({"task": "t7", "pass": true})))
        );
    }
}
