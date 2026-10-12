// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The agora v0: the plaza's calls, the bids a connected agent makes on them,
//! and the awards that bind a task to its contractor.
//!
//! The model is `docs/treasury-draft/schema.sql`'s `agora.*` (decisions 0073,
//! 0112) restricted to what a v0 task already carries, and the rules it
//! enforces are that schema's `check_bid` and `check_award`, read against the
//! task ledger instead of a `agora.calls` row.
//!
//! # The call is a view over the ledger, not a second registry
//!
//! `agora.calls` is a row of its own in the schema: a task's *offer*, with its
//! own id, requester, capability, spec, ceiling, deadline and invited list. v0
//! makes one choice here, and states it: **the call is derived from the task
//! ledger rather than stored a second time**, because every field a v0 call
//! needs is already the task's:
//!
//! | `agora.calls` | v0 |
//! |---|---|
//! | `id`, `task_id` | the task's id: the offer and the task it offers are one thing, so the notional call and its task share an identity |
//! | `capability` | the task's `role` (what is needed, matched against the agent's role) |
//! | `spec_ref` | the task's `acceptance`, when it named one |
//! | `max_price` | the task's `budget` — the same credits the treasury's journal minted into the task's escrow |
//! | `state` | `awarded` once an award closed the call, the task's own state otherwise |
//! | `created_at` | the task's `created_at` |
//! | `requester`, `deadline`, `invited` | **not modelled at v0**: a v0 task names no requester, has no deadline, and the register check stands in for the invitation list (below) |
//!
//! The board ([`Agora::calls`]) is that derivation over the ledger's own list,
//! so it can never disagree with it. It holds the roled tasks whose offer is
//! still standing (`open`), plus every roled task the agora holds state for —
//! a bid, or an award. A roled task claimed the old way — the dispatcher's own
//! claim, with no bid and no award — is not on the board: nothing about it was
//! ever offered through the agora.
//!
//! # Enforced, with the schema's triggers as the model
//!
//! * **The bidder is invited — as a connected agent of the task's role.** The
//!   schema's `invited text[]` (at most three best-matching candidates, 0112)
//!   has no v0 equivalent: a v0 task carries no invitation list, so the rule
//!   becomes the check the claim already makes — the bid must come from an
//!   agent the node's register shows connected *right now*, and that agent's
//!   role must be the call's `capability`. The register half is the HTTP
//!   edge's, made once with one message ("no live agent has guest CID n"),
//!   shared with the claim; the role half is the agora's.
//! * **The price is at or above zero, and at or below the ceiling.** `price`
//!   is a `u64` on the wire, so the floor is the type (a negative price never
//!   leaves the body: it is a 400 before this code is reached) and the ceiling
//!   is the call's `max_price`, read from the task's budget; a bid above it is
//!   refused in the schema's own terms (`bid price % exceeds max_price %`).
//! * **The call is `open`.** A bid on a task that has been claimed, finished
//!   or withdrawn is refused, as the schema refuses it: `call % is %, not
//!   open`.
//! * **An award matches a bid.** `agora.check_award` demands the award's
//!   contractor and price match a bid row. v0 looks the bid up by the agent's
//!   CID and awards it **at the price it bid** — the award's body carries no
//!   price at all — so an award can never name a price no bidder offered.
//! * **One award per call.** `agora.awards`' primary key is its `call_id`;
//!   v0's awards are keyed by the call's task id, and a second award to a call
//!   that already has one is refused by that key.
//!
//! # Recorded, or deliberately absent (stated, not faked)
//!
//! * **`eta` and `confidence`.** `agora.bids` requires both; a v0 bid carries
//!   neither, so neither is recorded. A later slice that gives a bidder a
//!   promise to make is what would.
//! * **`warranty_until` and `outcome`.** `agora.awards` requires the first and
//!   allows the second; v0 has no warranty or clawback window and the award
//!   precedes any outcome, so neither is recorded — and **no window is
//!   invented**: a retention or expiry window is the operator's call, not this
//!   code's.
//! * **Reputation.** `agora.reputation`'s counters are a later slice; nothing
//!   here writes them.
//!
//! # The award performs the claim
//!
//! This is v0's integration choice, and the cleanest one available: **an award
//! binds the task through the ledger's own claim path** — the same
//! [`Task::claim`] the claim endpoint calls — rather than through a second way
//! to assign a task. One enforcement point, one state machine, one event
//! shape. Two consequences, both deliberate:
//!
//! * an award needs a **connected** agent, because the claim does (the award's
//!   contractor must be live right now, not merely at bid time); and
//! * an awarded task is `claimed`, with its `assignee_id` and `agent_cid` set,
//!   exactly as a claimed one is — a finish, a withdrawal and every later read
//!   see no difference at all.
//!
//! **An awardless task is untouched by every line of this.** The dispatcher's
//! claim path does not cross the agora: nothing here is consulted when a task
//! is claimed by CID, and the ledger's and the dispatcher's tests are
//! unaffected by it.
//!
//! # A re-bid replaces
//!
//! `agora.bids`' primary key is `(call_id, bidder)`: one bid per bidder per
//! call. The schema's trigger fires `BEFORE INSERT`, so a second row at the
//! same key is a duplicate-key conflict rather than a replace. v0 chooses the
//! **replace** reading of that key — the semantics the assignment names — for
//! the reason the key exists: a call is open, and the latest price an agent
//! has offered is the one it is offering. A re-bid therefore overwrites the
//! row at the same key, carrying its own `created_at`, and a call still holds
//! one bid per agent, never two.
//!
//! # Memory, caps, and the stream
//!
//! Like the ledger and the treasury, v0 is in memory: bids and awards live in
//! this process and are lost with it (the schema's tables, triggers and
//! append-only journal are a later slice). What bounds it: [`MAX_BIDS_PER_TASK`]
//! bids per call (a fresh bid over the cap is refused while a re-bid still
//! fits, since it replaces), and through the ledger's own `MAX_TASKS` at most
//! that many calls. [`AGORA_LIMIT`] bounds the board's *response*, exactly as
//! `treasury::JOURNAL_LIMIT` bounds the journal's.
//!
//! Every bid and every award publishes one `agora` event ([`Event::Agora`]) —
//! `{"task":…,"kind":"bid"|"award","agent_cid":…,"price":…}` — so a console
//! watching `GET /v1/events` sees the bidding live. That frame is the only
//! thing a stream is told here: polling `GET /v1/agora` stays the source
//! of truth for the board itself, the way `GET /v1/tasks` does for the
//! ledger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::{now_ms, ConnectedAgent};
use crate::tasks::Task;

/// The most bids one call holds. A fresh bid past the cap is refused (a 409);
/// a re-bid still fits, because it replaces the bidder's own row rather than
/// adding one. The schema invites at most three candidates (0112); v0 lets any
/// connected agent of the task's role bid, so it holds the bound here instead.
pub const MAX_BIDS_PER_TASK: usize = 32;

/// The most calls the board's response carries. The board itself may hold more
/// (a roled task is a call while its offer stands); the endpoint returns the
/// newest [`AGORA_LIMIT`] of them, oldest first within that window, so a
/// response is bounded — the bounding `treasury::JOURNAL_LIMIT` gives the
/// journal, with the same `total`/`limit`/`truncated` line beside it.
pub const AGORA_LIMIT: usize = 256;

/// Why a bid was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BidError {
    /// The task names no role, so there is no call to bid on.
    NotACall {
        /// The task's id.
        task: String,
    },
    /// The call is not open (its task has been claimed, finished or
    /// withdrawn), so it takes no bids.
    NotOpen {
        /// The call (its task's id).
        task: String,
        /// The task's state.
        state: &'static str,
    },
    /// The agent's role is not the call's capability.
    RoleMismatch {
        /// The bidding agent's guest CID.
        cid: u64,
        /// The agent's role, if it has one.
        agent_role: Option<String>,
        /// The call's capability (the task's role).
        call_role: String,
    },
    /// The bid's price is above the call's ceiling.
    PriceAboveMax {
        /// The price the caller asked.
        price: u64,
        /// The call's `max_price` (the task's budget).
        max_price: u64,
    },
    /// The call already holds the cap of bids and this is not a re-bid.
    Full {
        /// The cap.
        max: usize,
    },
}

impl BidError {
    /// The HTTP status this error is reported as. Every one is a conflict with
    /// the node's state — a task that is not a call, a call that is not open,
    /// an agent of another role, a price above the ceiling, a call at its bid
    /// cap — never a malformed request (that is a 400 before this code runs).
    pub fn status(&self) -> u16 {
        409
    }
}

impl std::fmt::Display for BidError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BidError::NotACall { task } => {
                write!(formatter, "task {task} names no role, so it is not a call")
            }
            BidError::NotOpen { task, state } => {
                write!(formatter, "call {task} is {state}, not open")
            }
            BidError::RoleMismatch {
                cid,
                agent_role,
                call_role,
            } => match agent_role {
                Some(role) => write!(
                    formatter,
                    "the agent on guest CID {cid} is role {role}, not {call_role}"
                ),
                None => write!(
                    formatter,
                    "the agent on guest CID {cid} has no role, so it cannot bid on a {call_role} call"
                ),
            },
            BidError::PriceAboveMax { price, max_price } => {
                write!(formatter, "bid price {price} exceeds max_price {max_price}")
            }
            BidError::Full { max } => {
                write!(formatter, "the call already holds its {max} bids")
            }
        }
    }
}

impl std::error::Error for BidError {}

/// Why an award was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwardError {
    /// The task names no role, so there is no call to award.
    NotACall {
        /// The task's id.
        task: String,
    },
    /// The call already has an award (`agora.awards`' primary key is its
    /// call).
    AlreadyAwarded {
        /// The call (its task's id).
        task: String,
        /// Who it was awarded to.
        contractor: String,
    },
    /// The call is not open (its task has already moved on).
    NotOpen {
        /// The call (its task's id).
        task: String,
        /// The task's state.
        state: &'static str,
    },
    /// The agent has no bid on this call, so there is nothing to award it.
    NoBid {
        /// The call (its task's id).
        task: String,
        /// The guest CID the caller asked to award to.
        agent_cid: u64,
    },
    /// The bid's price is above the call's ceiling. The bid rule already
    /// refused this at bid time and the budget never changes, so this guard
    /// fires only on a store that was corrupted some other way — it is the
    /// award's own re-assertion that an award can never name a price the
    /// call's ceiling does not cover.
    PriceAboveMax {
        /// The bid's price.
        price: u64,
        /// The call's `max_price` (the task's budget).
        max_price: u64,
    },
}

impl AwardError {
    /// The HTTP status this error is reported as: every one is a conflict with
    /// the call's state, never a malformed request.
    pub fn status(&self) -> u16 {
        409
    }
}

impl std::fmt::Display for AwardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AwardError::NotACall { task } => {
                write!(formatter, "task {task} names no role, so it is not a call")
            }
            AwardError::AlreadyAwarded { task, contractor } => {
                write!(formatter, "call {task} is already awarded to {contractor}")
            }
            AwardError::NotOpen { task, state } => {
                write!(formatter, "call {task} is {state}, not open")
            }
            AwardError::NoBid { task, agent_cid } => write!(
                formatter,
                "award to guest CID {agent_cid} matches no bid on call {task}"
            ),
            AwardError::PriceAboveMax { price, max_price } => {
                write!(formatter, "bid price {price} exceeds max_price {max_price}")
            }
        }
    }
}

impl std::error::Error for AwardError {}

/// One bid as the API reports it: the schema's `agora.bids` row, minus what
/// v0 does not record (`eta`, `confidence`).
///
/// The row names its call (`task`), as the schema's does (its key is
/// `(call_id, bidder)`), so the bid a `POST …/bid` answers with stands alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BidView {
    /// The call this bid is on (its task's id).
    pub task: String,
    /// The bidding agent's guest CID — the identity a re-bid replaces, and the
    /// one the award matches.
    pub agent_cid: u64,
    /// The schema's `bidder`: the agent's identity in the ledger's own terms
    /// (its role, or `cid:<n>` for a VM the node booted without one).
    pub bidder: String,
    /// The price the agent asks, in credits.
    pub price: u64,
    /// When the bid was made, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// One award as the API reports it: the schema's `agora.awards` row, minus
/// what v0 does not record (`warranty_until`, `outcome`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardView {
    /// The call this award closed (its task's id).
    pub task: String,
    /// The schema's `contractor`: the winning agent's identity in the
    /// ledger's own terms (from the bid).
    pub contractor: String,
    /// The winning agent's guest CID (from the bid).
    pub agent_cid: u64,
    /// The price the award was made at — always the price the agent bid, never
    /// a price the caller named.
    pub price: u64,
    /// The schema's `escrow_entry`: the treasury entry that funded the call's
    /// escrow. At v0 that is the task's own budget mint (a call's task is
    /// funded by the mint, not by a parent task's subcontract), or `null` for
    /// a task with no budget.
    pub escrow_entry: Option<u64>,
    /// When the award was made, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// One call as the API reports it: the offer a roled task makes, derived (see
/// the module note), with the bids it holds and the award that closed it.
#[derive(Debug, Serialize)]
pub struct CallView {
    /// The call's id: the task's id (the offer and the task are one thing at
    /// v0).
    pub task: String,
    /// The schema's `capability`: what is needed (the task's role).
    pub capability: String,
    /// The schema's `spec_ref`: how the work will be judged complete (the
    /// task's acceptance), or `null` when the task named none.
    pub spec_ref: Option<String>,
    /// The schema's `max_price`: the ceiling a bid may not exceed (the task's
    /// budget).
    pub max_price: u64,
    /// The call's state: `awarded` once an award closed it, the task's own
    /// state otherwise (see the module note).
    pub state: &'static str,
    /// When the call opened (the task's creation), in milliseconds since the
    /// Unix epoch.
    pub created_at: u64,
    /// The bids the call holds, in bid order (a re-bid keeps its place).
    pub bids: Vec<BidView>,
    /// The award that closed the call, or `null` while it is open.
    pub award: Option<AwardView>,
}

/// The board endpoint's body: the calls, with the bounding line the journal
/// endpoint carries.
#[derive(Debug, Serialize)]
pub struct CallsView {
    /// Up to the endpoint's limit of calls, oldest first within the window.
    pub calls: Vec<CallView>,
    /// How many calls the board holds.
    pub total: usize,
    /// The limit the response was cut to.
    pub limit: usize,
    /// Whether the board holds more calls than the response carries.
    pub truncated: bool,
}

/// The agora's own state: the bids each call holds, and its award.
#[derive(Debug, Default, Serialize, Deserialize)]
struct AgoraInner {
    /// The bids on each call, keyed by the call's task id, in bid order. A
    /// re-bid replaces the bidder's own row in place, so this never holds two
    /// bids from one agent.
    bids: HashMap<String, Vec<BidView>>,
    /// The award of each call that has one, keyed by the call's task id.
    awards: HashMap<String, AwardView>,
}

/// The agora: the bids and awards of the node's calls, and the bus they are
/// published to.
pub struct Agora {
    events: Arc<EventBus>,
    inner: Mutex<AgoraInner>,
}

impl Agora {
    /// An agora holding no bids or awards that publishes every one it takes to
    /// `events`.
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            events,
            inner: Mutex::new(AgoraInner::default()),
        }
    }

    /// Records `agent`'s bid of `price` on `task`'s call: the schema's
    /// `agora.bids` row, and its `check_bid` trigger's rules.
    ///
    /// The agent's connectedness is the caller's to have established — the
    /// HTTP edge checks the register before it gets here, exactly as it does
    /// for the claim; its *role* is checked here, because the call's
    /// capability is the call's own fact. `price` is a `u64`, so the schema's
    /// floor is the type; the ceiling is the call's `max_price`.
    ///
    /// A bid from an agent that already bid on this call **replaces** its own
    /// row, at the same place in the call's bid order, with this bid's own
    /// time (see the module note). A refused bid changes nothing.
    pub fn bid(
        &self,
        task: &Task,
        agent: &ConnectedAgent,
        price: u64,
    ) -> Result<BidView, BidError> {
        let Some(capability) = task.role() else {
            return Err(BidError::NotACall {
                task: task.id().to_string(),
            });
        };
        // The schema's `invited` check, as v0 can make it: the agent must be
        // an agent of the role the call is for.
        if agent.role.as_deref() != Some(capability) {
            return Err(BidError::RoleMismatch {
                cid: agent.cid,
                agent_role: agent.role.clone(),
                call_role: capability.to_string(),
            });
        }
        let max_price = task.budget();
        if price > max_price {
            return Err(BidError::PriceAboveMax { price, max_price });
        }
        let bid = BidView {
            task: task.id().to_string(),
            agent_cid: agent.cid,
            bidder: agent.assignee_id(),
            price,
            created_at: now_ms(),
        };
        let mut inner = self.lock();
        if !task.is_open() {
            return Err(BidError::NotOpen {
                task: task.id().to_string(),
                state: task.state(),
            });
        }
        let held = inner.bids.get(&bid.task).map_or(0, Vec::len);
        let rebid = inner
            .bids
            .get(&bid.task)
            .is_some_and(|bids| bids.iter().any(|held| held.agent_cid == bid.agent_cid));
        if !rebid && held >= MAX_BIDS_PER_TASK {
            return Err(BidError::Full {
                max: MAX_BIDS_PER_TASK,
            });
        }
        let bids = inner.bids.entry(bid.task.clone()).or_default();
        match bids
            .iter_mut()
            .find(|held| held.agent_cid == bid.agent_cid)
        {
            Some(slot) => *slot = bid.clone(),
            None => bids.push(bid.clone()),
        }
        // Published under the lock, so a subscriber sees the call's bids in
        // the order they were taken — the ledger publishes its transitions the
        // same way.
        self.events.publish(Event::Agora {
            task: bid.task.clone(),
            kind: "bid",
            agent_cid: bid.agent_cid,
            price: bid.price,
        });
        Ok(bid)
    }

    /// Awards `task`'s call to `agent`, at the price `agent` bid — and binds
    /// the task by claiming it, the ledger's one claim path.
    ///
    /// The award's preconditions, in order: the task is a call (it names a
    /// role), the call has no award yet (`agora.awards`' key), the call is
    /// open, the agent has a bid on it (`check_award`: the award matches a
    /// bid), and that bid's price is within the call's ceiling. Only then is
    /// the claim made; a claim the ledger refuses is an award the agora does
    /// not record, so the call and the task never disagree.
    pub fn award(&self, task: &Task, agent: &ConnectedAgent) -> Result<(), AwardError> {
        if task.role().is_none() {
            return Err(AwardError::NotACall {
                task: task.id().to_string(),
            });
        }
        let mut inner = self.lock();
        if let Some(award) = inner.awards.get(task.id()) {
            return Err(AwardError::AlreadyAwarded {
                task: task.id().to_string(),
                contractor: award.contractor.clone(),
            });
        }
        if !task.is_open() {
            return Err(AwardError::NotOpen {
                task: task.id().to_string(),
                state: task.state(),
            });
        }
        let Some(bid) = inner
            .bids
            .get(task.id())
            .and_then(|bids| bids.iter().find(|bid| bid.agent_cid == agent.cid))
            .cloned()
        else {
            return Err(AwardError::NoBid {
                task: task.id().to_string(),
                agent_cid: agent.cid,
            });
        };
        let max_price = task.budget();
        if bid.price > max_price {
            return Err(AwardError::PriceAboveMax {
                price: bid.price,
                max_price,
            });
        }
        // The award performs the claim: the ledger's own transition, its own
        // enforcement point, its own event. A refusal here means the task
        // stopped being open under us, which is the same conflict the state
        // check above reports.
        if let Err(refused) = task.claim(agent) {
            return Err(AwardError::NotOpen {
                task: task.id().to_string(),
                state: refused.from.as_str(),
            });
        }
        let award = AwardView {
            task: task.id().to_string(),
            contractor: bid.bidder,
            agent_cid: bid.agent_cid,
            price: bid.price,
            escrow_entry: task.escrow_entry(),
            created_at: now_ms(),
        };
        inner.awards.insert(award.task.clone(), award);
        self.events.publish(Event::Agora {
            task: task.id().to_string(),
            kind: "award",
            agent_cid: bid.agent_cid,
            price: bid.price,
        });
        Ok(())
    }

    /// The board: `tasks` (the ledger's own list, oldest first) as the calls
    /// it derives, each with its bids and its award, bounded to the newest
    /// `limit` of them, oldest first within that window.
    ///
    /// The derivation is one pass over the ledger (see the module note): a
    /// roled task is a call while its offer stands (`open`) or once the agora
    /// holds state for it (a bid, or an award). Nothing is stored here that
    /// the ledger already says.
    pub fn calls(&self, tasks: &[Arc<Task>], limit: usize) -> CallsView {
        let inner = self.lock();
        let calls: Vec<CallView> = tasks
            .iter()
            .filter_map(|task| {
                let capability = task.role()?;
                let award = inner.awards.get(task.id()).cloned();
                let bids = inner.bids.get(task.id()).cloned().unwrap_or_default();
                if award.is_none() && bids.is_empty() && !task.is_open() {
                    return None;
                }
                Some(CallView {
                    task: task.id().to_string(),
                    capability: capability.to_string(),
                    spec_ref: task.acceptance().map(str::to_string),
                    max_price: task.budget(),
                    state: match &award {
                        Some(_) => "awarded",
                        None => task.state(),
                    },
                    created_at: task.created_at(),
                    bids,
                    award,
                })
            })
            .collect();
        let total = calls.len();
        let start = total.saturating_sub(limit);
        let truncated = start > 0;
        CallsView {
            calls: calls.into_iter().skip(start).collect(),
            total,
            limit,
            truncated,
        }
    }

    fn lock(&self) -> MutexGuard<'_, AgoraInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}


impl Agora {
    /// The whole state as JSON: each call's bids and its award. The bus is not
    /// in it, and a restore publishes nothing.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        serde_json::to_string(&*inner).expect("the agora state serializes")
    }

    /// Replaces the state with an [`Agora::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: AgoraInner = serde_json::from_str(json)
            .map_err(|err| format!("the snapshot does not parse: {err}"))?;
        check_agora_snapshot(&restored)?;
        *self.lock() = restored;
        Ok(())
    }
}

/// The invariants an agora snapshot must hold before it may replace one: each
/// bid and award sits under its own call, and no agent holds two bids on one
/// call (a re-bid replaces its row).
fn check_agora_snapshot(inner: &AgoraInner) -> Result<(), String> {
    for (task, bids) in &inner.bids {
        let mut bidders = std::collections::HashSet::new();
        for bid in bids {
            if &bid.task != task {
                return Err(format!("a bid under {task} names the call {}", bid.task));
            }
            if !bidders.insert(bid.agent_cid) {
                return Err(format!("agent {} holds two bids on {task}", bid.agent_cid));
            }
        }
    }
    for (task, award) in &inner.awards {
        if &award.task != task {
            return Err(format!("an award under {task} names the call {}", award.task));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{CreateTask, FinishOutcome, TaskLedger};
    use std::time::Duration;

    #[test]
    fn a_snapshot_restores_the_same_calls_bids_and_awards() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let called = call_task(&ledger, "ship the agora", "builder", 500);
        let other = call_task(&ledger, "review the agora", "reviewer", 300);
        agora.bid(&called, &agent(100, "builder"), 400).expect("a bid");
        agora.bid(&called, &agent(101, "builder"), 350).expect("a bid");
        agora.award(&called, &agent(101, "builder")).expect("an award");
        agora.bid(&other, &agent(102, "reviewer"), 200).expect("a bid");

        let json = agora.snapshot();
        let restored = Agora::new(Arc::clone(&events));
        restored.restore(&json).expect("a snapshot restores");
        assert_eq!(
            serde_json::to_value(board(&restored, &ledger)).expect("json"),
            serde_json::to_value(board(&agora, &ledger)).expect("json"),
        );
        // The bid maps are hash maps, so compare the snapshots as values.
        let again: serde_json::Value = serde_json::from_str(&restored.snapshot()).expect("json");
        assert_eq!(again, serde_json::from_str::<serde_json::Value>(&json).expect("json"));
    }

    #[test]
    fn a_garbled_agora_snapshot_is_refused_and_changes_nothing() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let called = call_task(&ledger, "ship the agora", "builder", 500);
        agora.bid(&called, &agent(100, "builder"), 400).expect("a bid");
        let before = agora.snapshot();

        assert!(agora.restore("not json").is_err());
        assert!(agora.restore("{\"bids\": 7}").is_err());
        // A second bid from the same agent on the same call is refused.
        let mut value: serde_json::Value = serde_json::from_str(&before).expect("json");
        let rows = value["bids"]
            .as_object_mut()
            .expect("bids")
            .values_mut()
            .next()
            .expect("a call");
        let copy = rows[0].clone();
        rows.as_array_mut().expect("rows").push(copy);
        assert!(agora.restore(&value.to_string()).is_err());
        assert_eq!(agora.snapshot(), before);
    }

    /// A connected agent with a role, as the register would report one.
    fn agent(cid: u64, role: &str) -> ConnectedAgent {
        ConnectedAgent {
            cid,
            role: Some(role.to_string()),
            run: "r1".to_string(),
            vm: 0,
        }
    }

    /// A roled, budgeted task: the shape v0's call is derived from.
    fn call_task(ledger: &TaskLedger, goal: &str, role: &str, budget: u64) -> Arc<Task> {
        ledger
            .create(&CreateTask {
                goal: goal.to_string(),
                acceptance: Some("the suite is green".to_string()),
                budget: Some(budget),
                role: Some(role.to_string()),
            })
            .expect("a roled, budgeted task inside the contract")
    }

    /// A ledger and the agora whose bus they share, as the daemon wires them.
    fn lab(events: &Arc<EventBus>) -> (TaskLedger, Agora) {
        let ledger = TaskLedger::new(Arc::clone(events));
        (ledger, Agora::new(Arc::clone(events)))
    }

    /// The calls a board holds, by id, at the endpoint's own limit.
    fn board(agora: &Agora, ledger: &TaskLedger) -> CallsView {
        agora.calls(&ledger.list(), AGORA_LIMIT)
    }

    #[test]
    fn a_call_is_a_roled_task_and_the_board_is_derived_from_the_ledger() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);

        // An unroled task is not a call, and cannot be bid on.
        let plain = ledger
            .create(&CreateTask {
                goal: "just a goal".to_string(),
                acceptance: None,
                budget: Some(5),
                role: None,
            })
            .expect("a task");
        assert_eq!(
            agora.bid(&plain, &agent(100, "builder"), 1).unwrap_err(),
            BidError::NotACall {
                task: "t1".to_string()
            }
        );
        assert_eq!(
            agora.award(&plain, &agent(100, "builder")).unwrap_err(),
            AwardError::NotACall {
                task: "t1".to_string()
            }
        );

        // A roled task is a call, derived: capability, spec, ceiling and
        // creation all read off the ledger's own row.
        let called = call_task(&ledger, "ship the agora", "builder", 500);
        let view = board(&agora, &ledger);
        assert_eq!(view.total, 1);
        assert_eq!(view.limit, AGORA_LIMIT);
        assert!(!view.truncated);
        assert_eq!(view.calls.len(), 1);
        let call = &view.calls[0];
        assert_eq!(call.task, "t2");
        assert_eq!(call.capability, "builder");
        assert_eq!(call.spec_ref.as_deref(), Some("the suite is green"));
        assert_eq!(call.max_price, 500);
        assert_eq!(call.state, "open");
        assert_eq!(call.created_at, called.view().created_at);
        assert!(call.bids.is_empty());
        assert!(call.award.is_none());

        // A roled task whose offer is over, with nothing offered through the
        // agora, is not a call: the board is not a second task registry.
        let claimed = call_task(&ledger, "claimed some other way", "builder", 5);
        claimed.claim(&agent(100, "builder")).expect("claim");
        let view = board(&agora, &ledger);
        assert_eq!(view.total, 1, "the claimed task is not on the board");
        assert_eq!(view.calls[0].task, "t2");
    }

    #[test]
    fn the_bid_rules_are_the_schemas_check_bid() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 500);

        // A bid from an agent of another role is refused: the invited check,
        // as v0 can make it.
        assert_eq!(
            agora.bid(&task, &agent(101, "reviewer"), 100).unwrap_err(),
            BidError::RoleMismatch {
                cid: 101,
                agent_role: Some("reviewer".to_string()),
                call_role: "builder".to_string(),
            }
        );
        assert_eq!(
            BidError::RoleMismatch {
                cid: 101,
                agent_role: Some("reviewer".to_string()),
                call_role: "builder".to_string(),
            }
            .to_string(),
            "the agent on guest CID 101 is role reviewer, not builder"
        );
        // An agent with no role cannot bid on a roled call either.
        let roleless = ConnectedAgent {
            cid: 7,
            role: None,
            run: "r1".to_string(),
            vm: 0,
        };
        assert_eq!(
            agora.bid(&task, &roleless, 100).unwrap_err().to_string(),
            "the agent on guest CID 7 has no role, so it cannot bid on a builder call"
        );

        // The price ceiling is the call's max_price — the task's budget.
        assert_eq!(
            agora.bid(&task, &agent(100, "builder"), 501).unwrap_err(),
            BidError::PriceAboveMax {
                price: 501,
                max_price: 500
            }
        );
        assert_eq!(
            BidError::PriceAboveMax {
                price: 501,
                max_price: 500
            }
            .to_string(),
            "bid price 501 exceeds max_price 500"
        );

        // None of those refusals recorded anything.
        assert!(board(&agora, &ledger).calls[0].bids.is_empty());

        // The edges are inside the contract: the ceiling itself, and zero (the
        // schema's floor is enforced by the type; the ceiling is inclusive).
        let at_ceiling = agora.bid(&task, &agent(100, "builder"), 500).expect("the ceiling");
        assert_eq!(at_ceiling.price, 500);
        assert_eq!(
            agora.bid(&task, &agent(101, "builder"), 0).expect("zero").price,
            0
        );
        let view = board(&agora, &ledger);
        assert_eq!(view.calls[0].bids.len(), 2);
        // And the bid names the call and the agent's ledger identity.
        assert_eq!(view.calls[0].bids[0].task, "t1");
        assert_eq!(view.calls[0].bids[0].bidder, "builder");
        assert_eq!(view.calls[0].bids[0].agent_cid, 100);

        // A call that is not open takes no bids.
        let claimed = call_task(&ledger, "already claimed", "builder", 5);
        claimed.claim(&agent(100, "builder")).expect("claim");
        assert_eq!(
            agora.bid(&claimed, &agent(100, "builder"), 1).unwrap_err(),
            BidError::NotOpen {
                task: "t2".to_string(),
                state: "claimed"
            }
        );
        assert_eq!(
            BidError::NotOpen {
                task: "t2".to_string(),
                state: "claimed"
            }
            .to_string(),
            "call t2 is claimed, not open"
        );
    }

    #[test]
    fn a_rebid_replaces_the_agents_bid_and_the_cap_bounds_the_rest() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 500);

        // One bid per agent: a re-bid replaces the row in place, with its own
        // time, and the call still holds exactly one bid from that agent.
        let first = agora.bid(&task, &agent(100, "builder"), 400).expect("a bid");
        assert_eq!(first.price, 400);
        let first_at = first.created_at;
        let second = agora.bid(&task, &agent(100, "builder"), 300).expect("a re-bid");
        assert_eq!(second.price, 300);
        assert_eq!(second.agent_cid, 100);
        assert!(second.created_at >= first_at, "the re-bid carries its own time");
        let view = board(&agora, &ledger);
        assert_eq!(view.calls[0].bids.len(), 1, "one bid per agent");
        assert_eq!(view.calls[0].bids[0].price, 300);

        // Bids from fresh agents fill the call to its cap, and the next is
        // refused — while a re-bid from an agent already on the call still
        // fits, because it replaces rather than adds.
        for index in 1..MAX_BIDS_PER_TASK {
            let cid = 1000 + index as u64;
            agora.bid(&task, &agent(cid, "builder"), 10).expect("a bid under the cap");
        }
        let cap = agora.bid(&task, &agent(9999, "builder"), 10).unwrap_err();
        assert_eq!(cap, BidError::Full { max: MAX_BIDS_PER_TASK });
        assert_eq!(cap.to_string(), "the call already holds its 32 bids");
        assert_eq!(board(&agora, &ledger).calls[0].bids.len(), MAX_BIDS_PER_TASK);
        assert_eq!(
            agora
                .bid(&task, &agent(100, "builder"), 20)
                .expect("a re-bid still fits")
                .price,
            20
        );
        assert_eq!(board(&agora, &ledger).calls[0].bids.len(), MAX_BIDS_PER_TASK);
    }

    #[test]
    fn an_award_matches_a_bid_at_the_bidders_own_price() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 500);

        // No bid, no award — the schema's `check_award`.
        let no_bid = agora.award(&task, &agent(100, "builder")).unwrap_err();
        assert_eq!(
            no_bid,
            AwardError::NoBid {
                task: "t1".to_string(),
                agent_cid: 100
            }
        );
        assert_eq!(
            no_bid.to_string(),
            "award to guest CID 100 matches no bid on call t1"
        );

        // A bid from another agent does not let this one be awarded.
        agora.bid(&task, &agent(101, "builder"), 200).expect("a bid");
        assert!(matches!(
            agora.award(&task, &agent(100, "builder")),
            Err(AwardError::NoBid { .. })
        ));

        // The award is made at the price the agent bid, and it performs the
        // claim: the task is bound exactly as the claim path binds it.
        agora.bid(&task, &agent(100, "builder"), 300).expect("a bid");
        agora.award(&task, &agent(100, "builder")).expect("an award");
        let view = task.view();
        assert_eq!(view.state, "claimed");
        assert_eq!(view.assignee_id.as_deref(), Some("builder"));
        assert_eq!(view.agent_cid, Some(100));
        assert_eq!(view.outcome, None);

        // And it is recorded: the award names its call, the bidder's identity
        // and CID, the bid's price, and the entry that funded the escrow —
        // the task's own budget mint.
        let call = board(&agora, &ledger).calls.remove(0);
        assert_eq!(call.state, "awarded");
        let award = call.award.expect("the award");
        assert_eq!(award.task, "t1");
        assert_eq!(award.contractor, "builder");
        assert_eq!(award.agent_cid, 100);
        assert_eq!(award.price, 300);
        assert_eq!(award.escrow_entry, Some(2), "the plaza mint is entry 1, the funding entry 2");
        assert_eq!(ledger.treasury().escrow_balance("t1"), 500);

        // A second award is refused by the call's key, in the award's own
        // words, and changes nothing.
        let again = agora.award(&task, &agent(101, "builder")).unwrap_err();
        assert_eq!(
            again,
            AwardError::AlreadyAwarded {
                task: "t1".to_string(),
                contractor: "builder".to_string()
            }
        );
        assert_eq!(
            again.to_string(),
            "call t1 is already awarded to builder"
        );
        assert_eq!(
            board(&agora, &ledger).calls[0]
                .award
                .as_ref()
                .expect("the first award")
                .price,
            300
        );

        // An unbudgeted call has no escrow entry to name — and a zero-price
        // award is inside the contract, the ceiling being inclusive.
        let free = call_task(&ledger, "free work", "builder", 0);
        agora.bid(&free, &agent(100, "builder"), 0).expect("a zero bid");
        agora.award(&free, &agent(100, "builder")).expect("a zero award");
        let call = board(&agora, &ledger)
            .calls
            .into_iter()
            .find(|call| call.task == "t2")
            .expect("the free call");
        assert_eq!(call.award.expect("the award").escrow_entry, None);
    }

    #[test]
    fn the_award_re_asserts_the_ceiling_on_a_corrupted_store() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 500);
        agora.bid(&task, &agent(100, "builder"), 300).expect("a bid");

        // The bid rule refuses a price above the ceiling, so the only way the
        // award's own guard can fire is a store that was corrupted some other
        // way: plant such a bid and prove the award refuses it rather than
        // recording an award the call cannot cover.
        let mut inner = agora.lock();
        inner.bids.get_mut("t1").expect("the call's bids")[0].price = 501;
        drop(inner);

        let refused = agora.award(&task, &agent(100, "builder")).unwrap_err();
        assert_eq!(
            refused,
            AwardError::PriceAboveMax {
                price: 501,
                max_price: 500
            }
        );
        assert_eq!(refused.status(), 409);
        assert!(refused.to_string().contains("exceeds max_price 500"));
        // The refused award recorded nothing and bound nothing.
        assert_eq!(task.view().state, "open");
        assert!(board(&agora, &ledger).calls[0].award.is_none());
    }

    #[test]
    fn the_board_is_bounded_to_the_newest_window_oldest_first() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        for index in 0..5 {
            call_task(&ledger, &format!("call {index}"), "builder", 10);
        }
        // An unroled task is not a call and never counts toward the bound.
        ledger
            .create(&CreateTask {
                goal: "not a call".to_string(),
                acceptance: None,
                budget: None,
                role: None,
            })
            .expect("a task");

        let all = board(&agora, &ledger);
        assert_eq!(all.total, 5);
        assert!(!all.truncated);
        let ids: Vec<&str> = all.calls.iter().map(|call| call.task.as_str()).collect();
        assert_eq!(ids, vec!["t1", "t2", "t3", "t4", "t5"], "oldest first");

        // Bounded: the newest window of them, oldest first inside it — the
        // bounding the journal endpoint gives its entries.
        let window = agora.calls(&ledger.list(), 2);
        assert_eq!(window.total, 5);
        assert_eq!(window.limit, 2);
        assert!(window.truncated);
        let ids: Vec<&str> = window.calls.iter().map(|call| call.task.as_str()).collect();
        assert_eq!(ids, vec!["t4", "t5"]);

        // A limit larger than the board carries everything, untruncated.
        let roomy = agora.calls(&ledger.list(), AGORA_LIMIT);
        assert_eq!(roomy.calls.len(), 5);
        assert!(!roomy.truncated);
    }

    #[test]
    fn a_task_that_ends_keeps_its_call_on_the_board() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);

        // A withdrawn roled task nobody bid on leaves the board.
        let unwanted = call_task(&ledger, "unwanted", "builder", 5);
        unwanted.withdraw().expect("open -> withdrawn");
        assert_eq!(board(&agora, &ledger).total, 0);

        // One that was bid on stays, showing the bid that was made and the
        // state its own ledger reports.
        let bid_on = call_task(&ledger, "bid on then claimed", "builder", 5);
        agora.bid(&bid_on, &agent(100, "builder"), 5).expect("a bid");
        bid_on.claim(&agent(100, "builder")).expect("claim");
        let view = board(&agora, &ledger);
        assert_eq!(view.total, 1);
        assert_eq!(view.calls[0].task, "t2");
        assert_eq!(view.calls[0].state, "claimed");
        assert_eq!(view.calls[0].bids.len(), 1);
        assert!(view.calls[0].award.is_none());

        // An awarded call keeps its award through the task's whole life.
        let awarded = call_task(&ledger, "awarded", "builder", 5);
        agora.bid(&awarded, &agent(100, "builder"), 5).expect("a bid");
        agora.award(&awarded, &agent(100, "builder")).expect("the award");
        awarded
            .finish(FinishOutcome::Done, Some("done".to_string()))
            .expect("claimed -> done");
        let call = board(&agora, &ledger)
            .calls
            .into_iter()
            .find(|call| call.task == "t3")
            .expect("the awarded call");
        assert_eq!(call.state, "awarded");
        assert_eq!(call.award.expect("the award").price, 5);
    }

    #[test]
    fn every_bid_and_award_publishes_its_agora_frame() {
        let events = EventBus::new();
        let receiver = events.subscribe();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 500);

        agora.bid(&task, &agent(100, "builder"), 400).expect("a bid");
        agora.bid(&task, &agent(101, "builder"), 350).expect("another bid");
        agora.bid(&task, &agent(100, "builder"), 300).expect("a re-bid");
        agora.award(&task, &agent(100, "builder")).expect("an award");

        // The bus carries the ledger's and the treasury's frames too; what is
        // under test here is the agora's own, in the order they were taken.
        let mut frames = agora_frames(&receiver);
        assert_eq!(
            frames,
            vec![
                "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"bid\",\"agent_cid\":100,\"price\":400}\n\n",
                "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"bid\",\"agent_cid\":101,\"price\":350}\n\n",
                "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"bid\",\"agent_cid\":100,\"price\":300}\n\n",
                "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"award\",\"agent_cid\":100,\"price\":300}\n\n",
            ],
            "every bid, the re-bid, and the award"
        );

        // The award's frame carries the price it was made at, not the last one
        // the bidder offered: for the 101 bidder the two differ.
        let task2 = call_task(&ledger, "the second call", "reviewer", 90);
        agora.bid(&task2, &agent(201, "reviewer"), 80).expect("a bid");
        agora.bid(&task2, &agent(202, "reviewer"), 70).expect("a bid");
        agora.award(&task2, &agent(201, "reviewer")).expect("an award");
        frames = agora_frames(&receiver);
        assert_eq!(
            frames,
            vec![
                "event: agora\ndata: {\"task\":\"t2\",\"kind\":\"bid\",\"agent_cid\":201,\"price\":80}\n\n",
                "event: agora\ndata: {\"task\":\"t2\",\"kind\":\"bid\",\"agent_cid\":202,\"price\":70}\n\n",
                "event: agora\ndata: {\"task\":\"t2\",\"kind\":\"award\",\"agent_cid\":201,\"price\":80}\n\n",
            ]
        );
    }

    /// The `agora` frames a subscriber has received so far, in the order they
    /// were published.
    fn agora_frames(receiver: &std::sync::mpsc::Receiver<Event>) -> Vec<String> {
        let mut frames = Vec::new();
        while let Ok(event) = receiver.recv_timeout(Duration::from_secs(1)) {
            let frame = event.frame();
            if frame.starts_with("event: agora\n") {
                frames.push(frame);
            }
        }
        frames
    }

    #[test]
    fn an_award_refused_by_the_ledger_is_not_recorded() {
        let events = EventBus::new();
        let (ledger, agora) = lab(&events);
        let task = call_task(&ledger, "ship the agora", "builder", 5);
        agora.bid(&task, &agent(100, "builder"), 5).expect("a bid");

        // The task stops being open under the award: the ledger's claim is the
        // one enforcement point, and the award it refuses is not recorded.
        task.claim(&agent(101, "builder")).expect("a direct claim");
        let refused = agora.award(&task, &agent(100, "builder")).unwrap_err();
        assert_eq!(
            refused,
            AwardError::NotOpen {
                task: "t1".to_string(),
                state: "claimed"
            }
        );
        assert_eq!(refused.status(), 409);
        let call = board(&agora, &ledger).calls.remove(0);
        assert!(call.award.is_none());
        // The direct claim is the one that stands.
        assert_eq!(task.view().agent_cid, Some(101));
    }
}
