//! The link an engine session runs over, for one `code_engine` call: the hub decides each action
//! the engine asks for, and hears the engine's updates, through the agent's shared writer.
//!
//! A decision comes back to the tool that waits for it through the nested calls
//! ([`crate::lane::Nested`]): the agent loop is blocked on that same tool, so it cannot take it.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use apz_proto::wire::{Decision, GuestMessage, HostMessage};
use serde_json::Value;

use super::link::{EngineLink, Verdict};
use crate::lane::Shared;

/// How long a decision wait sleeps between checks of the cancel flag.
const DECISION_POLL: Duration = Duration::from_millis(50);

/// One `code_engine` call's path to the hub. Its requests carry ids `<call>.e<n>`, and its
/// updates go out under `call`.
pub struct HubLink<'a, 'l, S, W> {
    shared: &'a Shared<'l, S, W>,
    /// The work item the call runs for.
    work: u64,
    /// The `code_engine` call's id.
    call: String,
    /// The requests sent so far: the next one is `<call>.e<next>`.
    next: AtomicU64,
    /// The updates sent so far; each carries the next number, from 1.
    seq: AtomicU64,
    /// Set when the work item is cancelled or the agent is stopping.
    cancel: &'a AtomicBool,
}

impl<'a, 'l, S: Write, W: Write> HubLink<'a, 'l, S, W> {
    pub fn new(
        shared: &'a Shared<'l, S, W>,
        work: u64,
        call: String,
        cancel: &'a AtomicBool,
    ) -> Self {
        Self {
            shared,
            work,
            call,
            next: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            cancel,
        }
    }

    /// Waits for the hub's final answer to request `id`. A pending answer keeps the wait open
    /// until the owner's `permission_decision` arrives.
    fn wait(&self, id: &str, inbox: &Receiver<HostMessage>, deadline: Instant) -> Verdict {
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return Verdict::Cancelled;
            }
            let now = Instant::now();
            if now >= deadline {
                return Verdict::Deny("no decision before the deadline".to_string());
            }
            match inbox.recv_timeout(DECISION_POLL.min(deadline.saturating_duration_since(now))) {
                Ok(HostMessage::ToolDecision {
                    decision,
                    output,
                    approval,
                    ..
                }) => match decision {
                    Decision::Allow | Decision::Done => return Verdict::Allow,
                    Decision::Deny if output.is_empty() => {
                        return Verdict::Deny("denied by the hub".to_string());
                    }
                    Decision::Deny => return Verdict::Deny(output),
                    Decision::Pending => self.shared.note(&format!(
                        "agent-harness: {id} is held for the owner's approval {}",
                        approval.as_deref().unwrap_or("(none named)")
                    )),
                },
                Ok(HostMessage::PermissionDecision { allow: true, .. }) => return Verdict::Allow,
                Ok(HostMessage::PermissionDecision {
                    allow: false,
                    reason,
                    ..
                }) => {
                    return Verdict::Deny(
                        reason.unwrap_or_else(|| "the owner denied it".to_string()),
                    );
                }
                // Only decisions are routed to a waiting request; anything else is not for it.
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                // The request was unregistered under the wait, which only a dropped link does.
                Err(RecvTimeoutError::Disconnected) => return Verdict::Cancelled,
            }
        }
    }
}

impl<S: Write + Send, W: Write + Send> EngineLink for HubLink<'_, '_, S, W> {
    fn decide(&self, name: &str, input: &Value, deadline: Instant) -> Verdict {
        if self.cancelled() {
            return Verdict::Cancelled;
        }
        let n = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("{}.e{n}", self.call);
        // Registered before the request goes out, so the hub's answer always finds its taker.
        let inbox = self.shared.nested().register(&id);
        let verdict = match self.shared.say(&GuestMessage::ToolRequest {
            work: self.work,
            call: id.clone(),
            name: name.to_string(),
            input: input.clone(),
        }) {
            Ok(()) => self.wait(&id, &inbox, deadline),
            Err(err) => Verdict::Deny(format!("cannot reach the hub: {err}")),
        };
        self.shared.nested().unregister(&id);
        verdict
    }

    fn update(&self, engine: &str, session: &str, update: &Value) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        // Best effort, as the trait says: a lost update must not fail the engine's turn.
        let _ = self.shared.say(&GuestMessage::EngineUpdate {
            work: self.work,
            call: self.call.clone(),
            engine: engine.to_string(),
            session: session.to_string(),
            seq,
            update: update.clone(),
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::{next, pair};
    use apz_proto::line::Lines;
    use serde_json::json;
    use std::thread;

    fn decision(call: &str, decision: Decision) -> HostMessage {
        HostMessage::ToolDecision {
            call: call.to_string(),
            decision,
            output: String::new(),
            is_error: false,
            program: None,
            approval: None,
        }
    }

    #[test]
    fn an_allowed_request_is_sent_under_its_own_id_and_its_decision_returns_to_the_link() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let cancel = AtomicBool::new(false);
        let link = HubLink::new(&shared, 7, "toolu_1".to_string(), &cancel);
        let verdict = thread::scope(|scope| {
            scope.spawn(|| {
                let request = next(&mut host);
                assert!(request.contains(r#""call":"toolu_1.e1""#), "{request}");
                assert!(request.contains(r#""name":"engine.edit""#), "{request}");
                assert!(request.contains(r#""work":7"#), "{request}");
                let routed = shared
                    .nested()
                    .route(decision("toolu_1.e1", Decision::Allow));
                assert!(routed.is_none(), "the link is waiting for it");
            });
            link.decide(
                "engine.edit",
                &json!({"engine": "claude"}),
                Instant::now() + Duration::from_secs(5),
            )
        });
        assert_eq!(verdict, Verdict::Allow);
    }

    #[test]
    fn a_held_request_waits_for_the_owners_permission_decision() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let cancel = AtomicBool::new(false);
        let link = HubLink::new(&shared, 7, "toolu_1".to_string(), &cancel);
        let verdict = thread::scope(|scope| {
            scope.spawn(|| {
                let _ = next(&mut host);
                let held = HostMessage::ToolDecision {
                    call: "toolu_1.e1".to_string(),
                    decision: Decision::Pending,
                    output: String::new(),
                    is_error: false,
                    program: None,
                    approval: Some("a4".to_string()),
                };
                shared.nested().route(held);
                shared.nested().route(HostMessage::PermissionDecision {
                    work: 7,
                    call: "toolu_1.e1".to_string(),
                    allow: false,
                    reason: Some("the owner keeps this file".to_string()),
                });
            });
            link.decide(
                "engine.edit",
                &json!({}),
                Instant::now() + Duration::from_secs(5),
            )
        });
        assert_eq!(
            verdict,
            Verdict::Deny("the owner keeps this file".to_string())
        );
    }

    #[test]
    fn a_cancel_while_the_decision_is_open_ends_the_wait_as_cancelled() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let cancel = AtomicBool::new(false);
        let link = HubLink::new(&shared, 7, "toolu_1".to_string(), &cancel);
        let verdict = thread::scope(|scope| {
            scope.spawn(|| {
                let _ = next(&mut host);
                cancel.store(true, Ordering::SeqCst);
            });
            link.decide(
                "engine.exec",
                &json!({}),
                Instant::now() + Duration::from_secs(5),
            )
        });
        assert_eq!(verdict, Verdict::Cancelled);
        // A link already cancelled does not even ask.
        assert_eq!(
            link.decide(
                "engine.exec",
                &json!({}),
                Instant::now() + Duration::from_secs(5)
            ),
            Verdict::Cancelled
        );
    }

    #[test]
    fn no_decision_by_the_deadline_is_a_denial_and_the_request_is_forgotten() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let cancel = AtomicBool::new(false);
        let link = HubLink::new(&shared, 7, "toolu_1".to_string(), &cancel);
        let verdict = link.decide(
            "engine.fetch",
            &json!({}),
            Instant::now() + Duration::from_millis(120),
        );
        assert_eq!(
            verdict,
            Verdict::Deny("no decision before the deadline".to_string())
        );
        assert!(next(&mut host).contains(r#""call":"toolu_1.e1""#));
        // The id is unregistered, so a late decision goes to the agent loop instead.
        let late = decision("toolu_1.e1", Decision::Allow);
        assert_eq!(shared.nested().route(late.clone()), Some(late));
        // Each request takes the next id.
        let verdict = thread::scope(|scope| {
            scope.spawn(|| {
                let request = next(&mut host);
                assert!(request.contains(r#""call":"toolu_1.e2""#), "{request}");
                shared
                    .nested()
                    .route(decision("toolu_1.e2", Decision::Deny));
            });
            link.decide(
                "engine.read",
                &json!({}),
                Instant::now() + Duration::from_secs(5),
            )
        });
        assert_eq!(verdict, Verdict::Deny("denied by the hub".to_string()));
    }

    #[test]
    fn updates_are_numbered_per_link_and_carry_the_engine_and_session() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let cancel = AtomicBool::new(false);
        let link = HubLink::new(&shared, 7, "toolu_1".to_string(), &cancel);
        let update = json!({"sessionUpdate": "agent_message_chunk"});
        link.update("claude", "s1", &update);
        link.update("claude", "s1", &update);
        let first = next(&mut host);
        assert!(first.contains(r#""type":"engine_update""#), "{first}");
        assert!(first.contains(r#""seq":1"#), "{first}");
        assert!(first.contains(r#""session":"s1""#), "{first}");
        assert!(first.contains(r#""call":"toolu_1""#), "{first}");
        assert!(next(&mut host).contains(r#""seq":2"#));
    }
}
