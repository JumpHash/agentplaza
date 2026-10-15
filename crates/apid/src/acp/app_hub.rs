//! The plaza as the ACP methods see it: [`super::agent::Hub`] over the hub's [`App`].

use std::sync::mpsc::Receiver;

use apz_proto::wire::WorkKind;
use serde_json::Value;

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
        // The scheduler takes out only the item in flight; a queued item is not cancelled here.
        match self.app.work.current(cid) {
            Some(item) if item.id == work => {
                self.app
                    .work
                    .cancel(self.app, cid, "cancelled from the editor")
            }
            _ => false,
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
