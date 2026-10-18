//! The plaza as the ACP methods see it: [`super::agent::Hub`] over the hub's [`App`].

use std::sync::mpsc::Receiver;

use apz_proto::wire::{Control, WorkKind, WorkLane};
use serde_json::{Value, json};

use super::agent::{Hub, RepoFile, SessionChange};
use super::store::RepoLink;
use crate::agents::{Agent, Status};
use crate::approvals::{Mode, State};
use crate::events::Frame;
use crate::scheduler::Origin;
use crate::settings::{Tier, tier_of};
use crate::state::App;
use crate::transcript::Entry;

/// How many transcript entries one read takes from the log.
const ENTRY_PAGE: usize = 1000;
/// How many changed files one session change reports.
const PULL_FILES: usize = 50;
/// The largest text read from either side of a changed file; a bigger file is not reported.
const PULL_TEXT: usize = 256 * 1024;

/// The hub's [`Hub`].
pub struct AppHub<'a> {
    pub app: &'a App,
}

impl Hub for AppHub<'_> {
    fn agent(&self, name: Option<&str>) -> Result<u64, String> {
        let agents = self.app.agents.list();
        let found = match name {
            None => agents
                .iter()
                .find(|a| tier_of(&a.role) == Tier::Orchestrator),
            Some(name) => match name.parse::<u64>() {
                Ok(cid) => agents.iter().find(|a| a.cid == cid),
                Err(_) => agents.iter().find(|a| a.name.eq_ignore_ascii_case(name)),
            },
        };
        let Some(agent) = found else {
            return Err(match name {
                None => "there is no orchestrator".to_string(),
                Some(name) => format!("no agent named {name}"),
            });
        };
        match chat_refusal(agent) {
            Some(why) => Err(why.to_string()),
            None => Ok(agent.cid),
        }
    }

    fn subscribe(&self) -> Receiver<Frame> {
        self.app.bus.subscribe()
    }

    fn chat(
        &self,
        cid: u64,
        thread: &str,
        text: &str,
        lane: Option<WorkLane>,
    ) -> Result<u64, String> {
        let agent = self
            .app
            .agents
            .get(cid)
            .ok_or_else(|| "no such agent".to_string())?;
        if let Some(why) = chat_refusal(&agent) {
            return Err(why.to_string());
        }
        check_prompt_text(text)?;
        let origin = Origin {
            owner: true,
            acp: true,
            lane,
            ..Origin::default()
        };
        Ok(self.app.work.enqueue(
            self.app,
            cid,
            WorkKind::Chat,
            thread.to_string(),
            text.to_string(),
            origin,
        ))
    }

    fn cancel(&self, cid: u64, work: u64) -> bool {
        // The item in flight is stopped on the agent; a queued one only leaves its queue.
        let in_flight = self
            .app
            .work
            .current(cid)
            .is_some_and(|item| item.id == work);
        if in_flight {
            self.app
                .work
                .cancel(self.app, cid, "cancelled from the editor")
        } else {
            self.app
                .work
                .cancel_queued(self.app, cid, work, "cancelled from the editor")
        }
    }

    fn entries(&self, cid: u64, works: &[u64]) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut after = 0;
        loop {
            let page = self.app.transcripts.after(cid, after, ENTRY_PAGE);
            let full = page.len() == ENTRY_PAGE;
            if let Some(last) = page.last() {
                after = last.seq;
            }
            out.extend(page.into_iter().filter(|e| works.contains(&e.work)));
            if !full {
                return out;
            }
        }
    }

    fn pending_approvals(&self, cid: u64, thread: &str) -> Vec<Value> {
        self.app
            .approvals
            .list()
            .into_iter()
            .filter(|a| {
                a.cid == cid && a.state == State::Pending && a.thread.as_deref() == Some(thread)
            })
            .filter_map(|a| serde_json::to_value(a).ok())
            .collect()
    }

    fn decide(&self, id: &str, approve: bool, reason: &str) -> Result<(), String> {
        crate::approvals::decide_approval(self.app, id, approve, Some(reason.to_string()), None)
            .map(|_| ())
            .map_err(|(_, message)| message)
    }

    fn mode(&self, cid: u64) -> String {
        mode_name(self.app.approvals.mode(cid)).to_string()
    }

    fn set_mode(&self, cid: u64, mode: &str) -> Result<(), String> {
        let mode = parse_mode(mode)
            .ok_or_else(|| format!("no mode {mode}: use autonomous, ask or read_only"))?;
        self.app
            .approvals
            .set_mode(cid, mode)
            .map_err(|e| format!("could not save the permission mode: {e}"))
    }

    fn usage(&self, cid: u64, work: u64) -> Option<Value> {
        // The harness records each model call of a work as a `model_response` entry with its usage.
        let mut input = 0;
        let mut output = 0;
        let mut calls = 0;
        for entry in self.entries(cid, &[work]) {
            if entry.kind != "model_response" || entry.data["usage"].is_null() {
                continue;
            }
            input += entry.data["usage"]["input_tokens"].as_u64().unwrap_or(0);
            output += entry.data["usage"]["output_tokens"].as_u64().unwrap_or(0);
            calls += 1;
        }
        // The gateway's engine calls of the work are in the usage ledger, not the transcript: they
        // are filed under no work there, so the feed never shows them as the agent's own text.
        let engine = self.app.usage.work_totals(cid, work);
        combined_usage(
            (input, output, calls),
            (engine.tokens.input, engine.tokens.output, engine.calls),
        )
    }

    fn control(
        &self,
        cid: u64,
        work: Option<u64>,
        command: &str,
        arg: &str,
        from: &str,
    ) -> Result<String, String> {
        let running = self.app.work.current(cid);
        if let Some(work) = work
            && running.as_ref().map(|item| item.id) != Some(work)
        {
            return Err(format!("work {work} is not running"));
        }
        match command {
            "compress" => crate::harness_control::control(self.app, cid, Control::Compress)
                .map(|w| format!("compression queued for work {w}")),
            "steer" => {
                let text = arg.trim();
                if text.is_empty() {
                    return Err("steer needs what to change: /steer <text>".to_string());
                }
                if text.chars().count() > 2000 {
                    return Err("steer text is over 2000 characters".to_string());
                }
                let note = Control::Steer {
                    from: from.to_string(),
                    text: text.to_string(),
                };
                crate::harness_control::control(self.app, cid, note)
                    .map(|w| format!("steer queued for work {w}"))
            }
            "extend-turns" => {
                let turns = arg
                    .parse::<u32>()
                    .ok()
                    .filter(|t| (1..=200).contains(t))
                    .ok_or("extend-turns needs how many turns, from 1 to 200")?;
                crate::harness_control::control(self.app, cid, Control::ExtendTurns { turns })
                    .map(|w| format!("work {w} may take {turns} more turns"))
            }
            "stop" => {
                let item = running.ok_or_else(|| format!("agent {cid} has no running work"))?;
                if self
                    .app
                    .work
                    .cancel(self.app, cid, "stopped from the editor")
                {
                    Ok(format!("stopping work {}", item.id))
                } else {
                    Err(format!("agent {cid} did not take the stop"))
                }
            }
            other => Err(format!("no command {other}")),
        }
    }

    fn models(&self, cid: u64) -> (String, Vec<String>) {
        let Some(agent) = self.app.agents.get(cid) else {
            return (String::new(), Vec::new());
        };
        let current = self
            .app
            .settings
            .resolve(&agent.role, agent.model_override.as_deref());
        let policy = self.app.settings.get().model_policy;
        let mut models = match tier_of(&agent.role) {
            Tier::Orchestrator => policy.orchestrator,
            Tier::Worker => policy.worker,
        };
        // The model the agent runs now is offered even when the policy's list does not name it.
        if !models.contains(&current) {
            models.insert(0, current.clone());
        }
        (current, models)
    }

    fn context_window(&self, cid: u64) -> Option<u64> {
        let agent = self.app.agents.get(cid)?;
        let model = crate::agents::model_of(self.app, &agent);
        crate::providers::context_window(self.app, &model)
    }

    fn set_model(&self, cid: u64, model: &str) -> Result<(), String> {
        let agent = self
            .app
            .agents
            .get(cid)
            .ok_or_else(|| "no such agent".to_string())?;
        self.app.settings.allowed(tier_of(&agent.role), model)?;
        // The same override the console's PATCH sets: the hub resolves it on the agent's next model call.
        self.app
            .spawned
            .set_model(cid, Some(model))
            .map_err(|e| format!("could not save the model: {e}"))?;
        self.app
            .agents
            .update(cid, |a| a.model_override = Some(model.to_string()));
        // As the console's PATCH does: the agent gets its new profile now (best effort; its next
        // hello carries it anyway), and the console sees the change.
        if let Some(updated) = self.app.agents.get(cid) {
            let profile = crate::agents::profile(self.app, &updated);
            let push =
                serde_json::json!({ "message": apz_proto::wire::HostMessage::Profile { profile } });
            let _ = crate::fleet::send(self.app, cid, &push);
            self.app
                .bus
                .publish("agent", &crate::agents::view(self.app, &updated));
        }
        Ok(())
    }

    fn write_back(&self) -> bool {
        self.app.settings.get().acp_write_back
    }

    fn session_lane(&self, cid: u64, session: &str, repo: &RepoLink) -> Option<WorkLane> {
        crate::weft_hub::session_lane(self.app, &repo.name, &repo.branch, session, cid)
    }

    fn session_change(&self, name: &str, head: &str) -> Option<SessionChange> {
        // The session's lane in Weft (`acp/<session>`, the agent's, opened on the editor's lane): its latest
        // draft against the revision that draft started from.
        let c = self.app.weft.client("owner").ok()?;
        let enc = weft::server::percent_encode;
        let open = c
            .get(&format!(
                "/repos/{}/log?q={}&limit=200",
                enc(name),
                enc("open()")
            ))
            .ok()?;
        let lane = open["changes"]
            .as_array()?
            .iter()
            .find(|ch| ch["title"].as_str() == Some(head))?
            .clone();
        let change = lane["change"].as_str()?.to_string();
        let detail = c.get(&format!("/changes/{change}")).ok()?;
        let rev = detail["drafts"].as_array()?.last()?.as_str()?.to_string();
        let base = c.get(&format!("/revisions/{rev}")).ok()?["base"]
            .as_str()?
            .to_string();
        // Weft that cannot list the files still leaves the lane reported, without diffs.
        let changed = c
            .get(&format!("/revisions/{rev}/diff"))
            .unwrap_or(Value::Null);
        let mut files = Vec::new();
        for f in changed["files"].as_array().into_iter().flatten() {
            if files.len() == PULL_FILES {
                break;
            }
            let Some(path) = f["path"].as_str() else {
                continue;
            };
            if f["binary"] == true {
                continue;
            }
            let (old, new) = (side(&c, &base, path), side(&c, &rev, path));
            let (old_text, new_text) = match (old, new) {
                (Side::Skip, _) | (_, Side::Skip) | (Side::Absent, Side::Absent) => continue,
                (old, new) => (old.text(), new.text()),
            };
            files.push(RepoFile {
                path: path.to_string(),
                old_text,
                new_text,
            });
        }
        Some(SessionChange {
            url: format!(
                "{}/#/history/{}/change/{change}",
                self.app.config.public_url.trim_end_matches('/'),
                enc(name)
            ),
            number: weft::model::parse_change(&change).ok()?,
            files,
        })
    }

    fn now_ms(&self) -> u64 {
        apz_plaza::now_ms()
    }
}

/// What a changed file holds on one side of its diff.
enum Side {
    /// Not on that side: the file was added or removed.
    Absent,
    Text(String),
    /// Not reported: a directory, binary or too big, or Weft did not answer.
    Skip,
}

impl Side {
    fn text(self) -> Option<String> {
        match self {
            Side::Text(text) => Some(text),
            Side::Absent | Side::Skip => None,
        }
    }
}

/// `path` at revision `rev`, from Weft.
fn side(c: &weft::client::Client, rev: &str, path: &str) -> Side {
    if rev == "root" {
        return Side::Absent;
    }
    let bytes = match c.get_bytes(&format!(
        "/revisions/{rev}/file?path={}",
        weft::server::percent_encode(path)
    )) {
        Ok(b) => b,
        Err(e) if e.contains("(404)") => return Side::Absent,
        Err(_) => return Side::Skip,
    };
    if bytes.len() > PULL_TEXT {
        return Side::Skip;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Side::Text(text),
        Err(_) => Side::Skip,
    }
}

/// The most an editor's prompt may hold, in bytes: the attached files ([`super::agent::PROMPT_BYTES`])
/// plus room for the editor's own text and the repository note. The console's chat keeps its own,
/// smaller limit (`agents::check_chat_text`); a prompt with attached files is far longer than a typed one.
const PROMPT_TEXT_BYTES: usize = super::agent::PROMPT_BYTES + 64 * 1024;

/// Whether an editor's prompt text may be queued: not blank, and within [`PROMPT_TEXT_BYTES`].
fn check_prompt_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("text must not be empty".into());
    }
    if text.len() > PROMPT_TEXT_BYTES {
        return Err(format!(
            "the prompt is {} bytes; the limit is {PROMPT_TEXT_BYTES}",
            text.len()
        ));
    }
    Ok(())
}

/// Why `agent` takes no chat: a model agent has none, and an offline one cannot take it. The
/// messages are the chat route's.
fn chat_refusal(agent: &Agent) -> Option<&'static str> {
    if agent.kind == "model" {
        return Some("a model agent has no chat");
    }
    if agent.status == Status::Offline {
        return Some("agent is offline");
    }
    None
}

/// A mode's name on the ACP side: the approvals API's own spelling.
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Autonomous => "autonomous",
        Mode::Ask => "ask",
        Mode::ReadOnly => "read_only",
    }
}

fn parse_mode(name: &str) -> Option<Mode> {
    match name {
        "autonomous" => Some(Mode::Autonomous),
        "ask" => Some(Mode::Ask),
        "read_only" => Some(Mode::ReadOnly),
        _ => None,
    }
}

/// The usage an editor is told for a work, from the harness's model calls and the gateway's engine
/// calls, each as (input, output, calls). `input_tokens` and `output_tokens` are both parts; the
/// engine part is also reported apart. `None` when neither made a call.
fn combined_usage(harness: (u64, u64, u64), engine: (u64, u64, u64)) -> Option<Value> {
    let (h_input, h_output, h_calls) = harness;
    let (e_input, e_output, e_calls) = engine;
    (h_calls + e_calls > 0).then(|| {
        json!({
            "input_tokens": h_input + e_input,
            "output_tokens": h_output + e_output,
            "engine_input_tokens": e_input,
            "engine_output_tokens": e_output,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usage_combines_the_harness_calls_and_the_engine_calls_of_a_work() {
        let both = combined_usage((100, 20, 2), (30, 5, 1)).unwrap();
        assert_eq!(both["input_tokens"], 130);
        assert_eq!(both["output_tokens"], 25);
        assert_eq!(both["engine_input_tokens"], 30);
        assert_eq!(both["engine_output_tokens"], 5);

        let engine_only = combined_usage((0, 0, 0), (30, 5, 1)).unwrap();
        assert_eq!(engine_only["input_tokens"], 30);
        assert_eq!(engine_only["engine_output_tokens"], 5);

        let harness_only = combined_usage((100, 20, 2), (0, 0, 0)).unwrap();
        assert_eq!(harness_only["input_tokens"], 100);
        assert_eq!(harness_only["engine_input_tokens"], 0);

        assert_eq!(combined_usage((0, 0, 0), (0, 0, 0)), None);
    }

    #[test]
    fn an_editor_prompt_may_carry_its_attached_files() {
        let attached = "x".repeat(super::super::agent::PROMPT_BYTES);
        assert!(check_prompt_text(&attached).is_ok());
        assert!(check_prompt_text("  ").is_err());
        assert!(check_prompt_text(&"x".repeat(PROMPT_TEXT_BYTES + 1)).is_err());
    }

    #[test]
    fn modes_use_the_approvals_api_spelling() {
        for mode in [Mode::Autonomous, Mode::Ask, Mode::ReadOnly] {
            assert_eq!(parse_mode(mode_name(mode)), Some(mode));
            assert_eq!(
                serde_json::to_value(mode).unwrap(),
                Value::String(mode_name(mode).to_string())
            );
        }
        assert_eq!(parse_mode("read-only"), None);
        assert_eq!(parse_mode("Ask"), None);
    }
}
