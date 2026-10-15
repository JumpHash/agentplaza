//! The plaza as the ACP methods see it: [`super::agent::Hub`] over the hub's [`App`].

use std::sync::mpsc::Receiver;

use apz_proto::wire::{Control, WorkKind};
use serde_json::{Value, json};

use super::agent::Hub;
use crate::agents::{Agent, Status, check_chat_text};
use crate::approvals::{Mode, State};
use crate::events::Frame;
use crate::scheduler::Origin;
use crate::settings::{Tier, tier_of};
use crate::state::App;
use crate::transcript::Entry;

/// How many transcript entries one read takes from the log.
const ENTRY_PAGE: usize = 1000;

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

    fn chat(&self, cid: u64, thread: &str, text: &str) -> Result<u64, String> {
        let agent = self
            .app
            .agents
            .get(cid)
            .ok_or_else(|| "no such agent".to_string())?;
        if let Some(why) = chat_refusal(&agent) {
            return Err(why.to_string());
        }
        check_chat_text(text)?;
        let origin = Origin {
            owner: true,
            acp: true,
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
        (calls > 0).then(|| json!({"input_tokens": input, "output_tokens": output}))
    }

    fn control(
        &self,
        cid: u64,
        work: Option<u64>,
        command: &str,
        arg: &str,
    ) -> Result<String, String> {
        let running = self.app.work.current(cid);
        if let Some(work) = work {
            if running.as_ref().map(|item| item.id) != Some(work) {
                return Err(format!("work {work} is not running"));
            }
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
                    from: "owner".to_string(),
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

    fn now_ms(&self) -> u64 {
        apz_plaza::now_ms()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
