// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The council v0: the four seats, the motions they propose and vote on, and
//! the count that decides — `docs/agents/council.md` over what a node of
//! tool-server agents can honestly hold.
//!
//! # The seats (the doctrine, mapped to this fleet's roles)
//!
//! The doctrine is `docs/codex-draft/statutes/S-008-council-seats.cedar` (law
//! `S-008`; clause `S-008a` is the exclusion) read with
//! `docs/codex-draft/playbooks/council-procedure.md`. Seats are held by the
//! heads of Finance, Legal (Agents) — the legislative counsel — Incident
//! response and Mayor Office, and **judges and prosecutors never sit**: Legal
//! (Governance) is excluded so the courts stay independent of rule-making
//! ([`SEATS`], [`EXCLUDED_ROLES`]). Every other role of the fleet holds no
//! seat.
//!
//! # The motion (the schema, as this daemon stores it)
//!
//! [`MotionView`] is the contract's `{id: m<seq>, proposer_cid,
//! proposer_role, kind, text, needs_owner, state, votes[], tally, created_at,
//! updated_at}`. Proposing is a **seat action** (S-008a): the proposer must be
//! a connected agent whose role holds a seat, and any other agent is refused
//! with its role named ([`CouncilError::Unseated`], a 403). The `kind` is the
//! schema's enum, membership-checked by the deserializer, and `text` is
//! non-empty and at most [`MAX_MOTION_TEXT`] bytes.
//!
//! # `needs_owner` is derived, never trusted
//!
//! The daemon computes `needs_owner` from the kind, per decision 0067
//! ([`derived_needs_owner`]): `law_proposal`, `town_proposal` and
//! `mayor_replacement` are always owner-gated; a `budget_move` is owner-gated
//! when the amount it names is over [`OWNER_BUDGET_PERCENT`]% of the **minted
//! budget total** — the treasury's own record of what its mint has issued, read
//! afresh at propose time, never cached ([`crate::treasury::Treasury::minted_total`]) — and
//! the other kinds are the council's majority. A
//! caller-supplied `needs_owner` that **understates** the rule is refused with
//! the rule named (`400`, never silently corrected) and one that **overstates**
//! it is refused too: v0 does not let a proposer set the flag true to route a
//! motion around the majority that would decide it (stated, per the contract).
//!
//! **A narrowing, stated:** the doctrine calls the non-gated kinds "the
//! proposer's call", and this daemon narrows that to "the proposer picks the
//! kind". The flag itself is derived, and any caller-supplied value that
//! disagrees with the derived one is a 400 — so a motion's gate is a fact about
//! the motion, not a claim about it.
//!
//! **An addition, stated:** a `budget_move` must name its amount, as a typed
//! `amount` field, because the 10% rule cannot be applied to free text. The
//! field is echoed in the view only when the motion has one, so every motion
//! that names no amount serialises exactly as the contract's schema does.
//!
//! # The vote
//!
//! [`VoteView`] is one seat's vote: `{seat, agent_cid, vote, reason, at}`.
//! **Only the four seats vote** (a connected agent whose role holds a seat; any
//! other is a 403 with its role named), `vote` is the schema's
//! `yes | no | abstain`, and **`reason` is required and non-empty** — the
//! playbook's rule, never a bare vote — and at most [`MAX_REASON`] bytes. **One
//! vote per seat**: a later vote from the same seat **replaces** the earlier
//! one, so the newest reason stands and a seat can never be double-counted.
//! Voting on a motion that is no longer `open` is refused with the state named
//! (a 409).
//!
//! # The count (0034), and the owner's decision
//!
//! The count's state is a function of the tally and the gate, in one table
//! ([`state_of`]); the owner's decision, once made, is the state:
//!
//! | state | when |
//! |---|---|
//! | `open` | fewer than [`SEAT_COUNT`] seats have voted |
//! | `carried` | a majority of yes over no, below the owner's gate |
//! | `rejected` | a majority of no |
//! | `tie_to_owner` | an equal split (the doctrine's 2–2) — the owner decides |
//! | `awaiting_owner` | carried, but `needs_owner` — the decision waits for ratification |
//! | `ratified` | the owner approved a motion the count gated to them |
//! | `vetoed` | the owner refused a motion the count gated to them |
//!
//! `abstain` counts toward the four seats having voted and never toward the
//! majority. The doctrine names the tie `2–2`; an equal yes/no split with
//! abstentions (say 1–1 with two abstentions) is the same no-majority and the
//! same escalation, which is how this daemon reads it. [`TallyView`] carries
//! `{yes, no, abstain, voted, seats}`.
//!
//! # The owner's decision
//!
//! `POST /v1/motions/{id}/decide {approve, note?}` is the owner's hand
//! ([`Council::decide`]). **The owner is the operator, not an agent**: the
//! request names no agent CID and none is looked up — the owner has no seat,
//! no role and no session, so the seat checks that gate proposing and voting do
//! not apply to them. That is the same modelling choice the agora's award makes
//! for the operator (the caller that knows is the employer, not a connected
//! agent): the operator's act *is* the owner's, recorded as it.
//!
//! The owner decides a motion **once**, and only one the count gated to them
//! ([`State::owner_decides`]: `tie_to_owner` and `awaiting_owner`). A motion in
//! any other state — `open` (the council has not decided), `carried` or
//! `rejected` (the council has, and a majority is not the owner's to re-decide)
//! — is refused with the state named and the council's own decision left
//! standing (a 409); a motion already decided by the owner is refused too (a
//! 409, decided once). A gated motion takes the decision: `approve` true is
//! `ratified`, false is `vetoed`, and the optional `note` (≤
//! [`MAX_DECISION_NOTE`] bytes; absent, or present and non-empty) is recorded
//! with the stamp in [`OwnerDecisionView`] — the `owner` field every view
//! carries, `null` until the owner acts and a Column-style optional thereafter:
//! `null` reads as "the owner has not decided", never as a decision.
//!
//! # Bounds (each stated, none silent)
//!
//! | what | bound |
//! |---|---|
//! | motions the council holds | [`MAX_MOTIONS`] (256) — a fresh motion past it is refused, 409 |
//! | a motion's `text` | [`MAX_MOTION_TEXT`] (4000 bytes, the schema's `maxLength`) — 400 |
//! | a vote's `reason` | [`MAX_REASON`] (2000 bytes) — 400 |
//! | an owner's decision `note` | [`MAX_DECISION_NOTE`] (1000 bytes) — 400 |
//! | a motions response | [`MOTIONS_LIMIT`] (256) newest motions, `truncated` says so |
//!
//! # What v0 narrows (stated, not hidden)
//!
//! * **The store is in this process.** Like the ledger, the treasury, the agora
//!   and the social layer, the motions live in memory and are lost with the
//!   daemon; the schema's tables are a later slice. The views are derived from
//!   that store, never a second copy of a fact held elsewhere.
//! * **No quorum or session windows, no deadlines, no per-case grants (0079)
//!   and no law signing (0049).** Any seat may vote while a motion is `open`.
//!   The owner's decision is here ([`Council::decide`]); grants and law signing
//!   are each their own slice, exactly as the contract states.
//! * **The drivers are not here.** The dispatcher's driving of votes for the
//!   seats is `dispatchd`'s, the way the social pass is: this module is the
//!   state and the view the drivers act on.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::{now_ms, ConnectedAgent};
use crate::tasks::{optional_text, required_text};

/// The seat the head of **Finance** holds (S-008 / 0034).
pub const FINANCE: &str = "finance";

/// The seat the head of **Legal (Agents)** holds — the legislative counsel.
/// Legal (Governance) is the excluded half (see [`EXCLUDED_ROLES`]).
pub const LEGAL_AGENTS_COUNSEL: &str = "legal-agents-counsel";

/// The seat the head of **Incident response** holds.
pub const INCIDENT: &str = "incident";

/// The seat the head of **Mayor Office** holds.
pub const MAYOR_OFFICE: &str = "mayor-office";

/// How many seats the council has. The doctrine's four (0034); every count in
/// this module is expressed against it rather than against a literal.
pub const SEAT_COUNT: usize = 4;

/// The council's four seats, in the order a motion's votes are reported.
///
/// The doctrine is `docs/codex-draft/statutes/S-008-council-seats.cedar`
/// (`@law("S-008")`: "Agents holding a council seat may propose motions and
/// cast votes. Judges and prosecutors never hold seats."), read with
/// `docs/codex-draft/playbooks/council-procedure.md` step 1: "There are four:
/// the heads of Finance, Legal (Agents) (the legislative counsel), Incident
/// response and Mayor Office. Legal (Governance), meaning judges and
/// prosecutors, never sits (S-008a), so courts stay independent of
/// rule-making."
///
/// These are the roles of `docs/agents/initial-roles.json` that hold seats;
/// every other role of that manifest holds none (see [`EXCLUDED_ROLES`] and
/// [`seat_of`]).
pub const SEATS: [&str; SEAT_COUNT] = [FINANCE, LEGAL_AGENTS_COUNSEL, INCIDENT, MAYOR_OFFICE];

/// The roles the doctrine excludes from every seat: Legal (Governance), the
/// judges and the prosecutors. They are named here so the exclusion is a fact
/// of this module, not merely an absence from [`SEATS`] — a later edit that
/// tried to seat one of them must fail this module's own test.
pub const EXCLUDED_ROLES: [&str; 2] = ["legal-gov-judge", "legal-gov-prosecutor"];

/// The seat holding `role`, by index into [`SEATS`], or `None` when the role
/// holds none. Exact equality, like every other role comparison in this daemon:
/// a role that merely looks like a seat is not one.
pub fn seat_of(role: &str) -> Option<usize> {
    SEATS.iter().position(|seat| *seat == role)
}

/// Whether `role` holds a council seat.
pub fn is_seated(role: &str) -> bool {
    seat_of(role).is_some()
}

/// The most motions the council holds. v0 never removes a motion (a motion is
/// the record of something the council did), so the cap is absolute: past it a
/// fresh motion is refused with an honest error rather than the oldest dropped.
pub const MAX_MOTIONS: usize = 256;

/// The most motions a motions *response* carries. The council may hold
/// [`MAX_MOTIONS`]; the endpoint returns the newest [`MOTIONS_LIMIT`] of them,
/// with the same `total`/`limit`/`truncated` line the journal and the board
/// carry.
pub const MOTIONS_LIMIT: usize = 256;

/// The most a motion's `text` may occupy, in bytes — the contract schema's
/// `maxLength`.
pub const MAX_MOTION_TEXT: usize = 4000;

/// The most a vote's `reason` may occupy, in bytes.
pub const MAX_REASON: usize = 2000;

/// The most an owner's decision `note` may occupy, in bytes. The note is
/// optional, but when it is there it is held to the same "not blank" rule as a
/// motion's `text` and a vote's `reason` — a bare note is no note.
pub const MAX_DECISION_NOTE: usize = 1000;

/// The threshold a `budget_move` is owner-gated at, as a percentage of the
/// minted budget total (0067: "budget moves over 10% of a town's monthly
/// budget"). This port has one budget pool, so the stand-in for a town's
/// monthly budget is the treasury's minted total, and the fraction is the
/// doctrine's. A move is gated when `amount * OWNER_BUDGET_PERCENT >
/// minted_total`, computed in integers so no rounding can move the boundary.
pub const OWNER_BUDGET_PERCENT: i128 = 10;

/// The kinds a motion may be (`docs/agents/council.md`'s enum). The
/// deserializer is the membership check: a word outside this enum is a 400
/// before this code runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Set a fleet-wide priority.
    Priority,
    /// Move budget between towns (0067: over 10% of the pool is owner-gated).
    BudgetMove,
    /// Propose a town (0067: always owner-gated).
    TownProposal,
    /// Replace a Mayor (0067: always owner-gated).
    MayorReplacement,
    /// Propose a law (0067 and 0049: always owner-gated).
    LawProposal,
    /// Anything else the council's majority decides.
    Other,
}

impl Kind {
    /// The kind as the wire and the views spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Priority => "priority",
            Kind::BudgetMove => "budget_move",
            Kind::TownProposal => "town_proposal",
            Kind::MayorReplacement => "mayor_replacement",
            Kind::LawProposal => "law_proposal",
            Kind::Other => "other",
        }
    }

    /// Whether this kind is always owner-gated, whatever it names: the 0067
    /// thresholds that do not depend on an amount (`law_proposal`,
    /// `town_proposal`, `mayor_replacement`).
    pub fn always_owner_gated(self) -> bool {
        matches!(
            self,
            Kind::LawProposal | Kind::TownProposal | Kind::MayorReplacement
        )
    }
}

/// One vote word (`yes | no | abstain`), membership-checked by the
/// deserializer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vote {
    /// In favour.
    Yes,
    /// Against.
    No,
    /// Present, and counted as such but never toward the majority.
    Abstain,
}

impl Vote {
    /// The vote as the wire and the views spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Vote::Yes => "yes",
            Vote::No => "no",
            Vote::Abstain => "abstain",
        }
    }
}

/// A motion's state (0034), derived from its tally and its gate — never stored
/// independently, so a state can never disagree with the votes that produced
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Fewer than [`SEAT_COUNT`] seats have voted.
    Open,
    /// A majority of yes over no, below the owner's gate.
    Carried,
    /// A majority of no.
    Rejected,
    /// An equal split (the doctrine's 2–2): the owner decides.
    TieToOwner,
    /// Carried, but `needs_owner`: the decision waits for ratification.
    AwaitingOwner,
    /// The owner approved a motion the count gated to them.
    Ratified,
    /// The owner refused a motion the count gated to them.
    Vetoed,
}

impl State {
    /// The state as the wire and the views spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Carried => "carried",
            State::Rejected => "rejected",
            State::TieToOwner => "tie_to_owner",
            State::AwaitingOwner => "awaiting_owner",
            State::Ratified => "ratified",
            State::Vetoed => "vetoed",
        }
    }

    /// Whether a motion in this state is the owner's to decide: the two the
    /// count gates to them — the 2–2 tie ([`State::TieToOwner`]) and a carry
    /// above the 0067 thresholds ([`State::AwaitingOwner`]). Every other state
    /// is the council's own (a decided motion is not re-decided; a majority is
    /// not routed around).
    pub fn owner_decides(self) -> bool {
        matches!(self, State::TieToOwner | State::AwaitingOwner)
    }
}

/// Why the council's store refused a request.
///
/// Three shapes, one per status: a caller the doctrine does not seat
/// ([`Unseated`](CouncilError::Unseated), a 403 — proposing and voting are seat
/// actions, S-008a), a conflict with the state the store holds
/// ([`Conflict`](CouncilError::Conflict), a 409), and a request that names
/// nothing the daemon holds ([`Unknown`](CouncilError::Unknown), a 404). A
/// request outside the endpoint's contract is a 400 before this code runs, from
/// the request's own checks ([`ProposeRequest::checked`],
/// [`VoteRequest::checked`], [`DecideRequest::checked`]), exactly as the
/// board's and the ledger's are. The
/// message is written where the rule lives, so a refusal names the rule it
/// broke rather than a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CouncilError {
    /// The caller is a connected agent whose role holds no seat.
    Unseated {
        /// The caller's guest CID.
        cid: u64,
        /// The caller's role, when it has one.
        role: Option<String>,
    },
    /// The request conflicts with the state the store holds.
    Conflict(String),
    /// The request names nothing the daemon holds.
    Unknown(String),
}

impl CouncilError {
    /// The HTTP status this refusal is reported as.
    pub fn status(&self) -> u16 {
        match self {
            CouncilError::Unseated { .. } => 403,
            CouncilError::Conflict(_) => 409,
            CouncilError::Unknown(_) => 404,
        }
    }
}

impl std::fmt::Display for CouncilError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CouncilError::Conflict(message) | CouncilError::Unknown(message) => {
                formatter.write_str(message)
            }
            CouncilError::Unseated { cid, role } => match role {
                Some(role) => write!(
                    formatter,
                    "the agent on guest CID {cid} is role {role}, which holds no council seat"
                ),
                None => write!(
                    formatter,
                    "the agent on guest CID {cid} has no role, so it holds no council seat"
                ),
            },
        }
    }
}

impl std::error::Error for CouncilError {}

/// The `POST /v1/motions` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ProposeRequest {
    /// The guest CID of the agent proposing — a connected agent whose role
    /// holds a seat.
    pub proposer_cid: u64,
    /// What kind of motion it is (the schema's enum).
    pub kind: Kind,
    /// The motion itself: the evidence and the concrete proposal.
    pub text: String,
    /// The amount a `budget_move` names, in credits. Required for a
    /// `budget_move` (the 10% rule needs it) and recorded for any other kind
    /// that carries one.
    #[serde(default)]
    pub amount: Option<u64>,
    /// The caller's claim about the owner's gate. v0 derives the flag and
    /// refuses any value that disagrees with the derivation (see the module
    /// note).
    #[serde(default)]
    pub needs_owner: Option<bool>,
}

/// A propose request's fields, checked, with the gate derived.
///
/// Built only by [`ProposeRequest::checked`], so the caps and the derived gate
/// hold for every motion the council takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedProposal {
    /// What kind of motion it is.
    pub kind: Kind,
    /// The motion itself.
    pub text: String,
    /// The amount a `budget_move` names.
    pub amount: Option<u64>,
    /// The gate the daemon derived — never the caller's word for it.
    pub needs_owner: bool,
}

impl ProposeRequest {
    /// Checks the request against the council's caps, derives `needs_owner`
    /// from the kind (and, for a `budget_move`, from `minted_total`), and
    /// refuses a caller-supplied flag that disagrees with the derivation, or
    /// says why the request is outside the endpoint's contract.
    ///
    /// `minted_total` is the treasury's minted budget total, read afresh by
    /// the caller at propose time.
    pub fn checked(&self, minted_total: i128) -> Result<CheckedProposal, String> {
        let text = required_text("text", &self.text, MAX_MOTION_TEXT)?;
        if self.kind == Kind::BudgetMove && self.amount.is_none() {
            return Err(format!(
                "a budget_move must name an amount: 0067 gates a move over \
                 {OWNER_BUDGET_PERCENT}% of the minted budget total, and the amount is what \
                 the rule is applied to (the minted total is {minted_total})"
            ));
        }
        let derived = derived_needs_owner(self.kind, self.amount, minted_total);
        if let Some(asked) = self.needs_owner
            && asked != derived
        {
            return Err(owner_mismatch(self.kind, self.amount, minted_total, asked));
        }
        Ok(CheckedProposal {
            kind: self.kind,
            text,
            amount: self.amount,
            needs_owner: derived,
        })
    }
}

/// The `POST /v1/motions/{id}/vote` body.
#[derive(Debug, Clone, Deserialize)]
pub struct VoteRequest {
    /// The guest CID of the voter — a connected agent whose role holds a seat.
    pub agent_cid: u64,
    /// The vote (the schema's enum).
    pub vote: Vote,
    /// Why, tied to the evidence. Required and non-empty: the playbook's
    /// "always with a reason", never a bare vote.
    pub reason: String,
}

/// A vote request's fields, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedVote {
    /// The vote.
    pub vote: Vote,
    /// Why.
    pub reason: String,
}

impl VoteRequest {
    /// Checks the request against the council's caps, or says why the request
    /// is outside the endpoint's contract.
    pub fn checked(&self) -> Result<CheckedVote, String> {
        Ok(CheckedVote {
            vote: self.vote,
            reason: required_text("reason", &self.reason, MAX_REASON)?,
        })
    }
}

/// The `POST /v1/motions/{id}/decide` body: the owner's decision.
///
/// **The owner is the operator, not an agent**, so this body names no agent
/// CID and none is looked up: the operator is not on the register, holds no
/// seat and has no session, and the seat checks that gate proposing and voting
/// do not apply to them. That is the agora's modelling choice for the operator
/// ([`crate::agora`]'s award takes the employer from the caller that knows it,
/// not from a connected agent): the operator's act *is* the owner's.
#[derive(Debug, Clone, Deserialize)]
pub struct DecideRequest {
    /// Whether the owner approves the motion: `true` ratifies it (`ratified`),
    /// `false` vetoes it (`vetoed`). Required, and never defaulted — an
    /// absent or non-boolean `approve` is outside this endpoint's contract.
    pub approve: bool,
    /// Why the owner decided as they did. Optional; absent is `null`
    /// ([`OwnerDecisionView`]), and when present it must be non-empty and at
    /// most [`MAX_DECISION_NOTE`] bytes.
    #[serde(default)]
    pub note: Option<String>,
}

/// An owner's decision, checked (the note held to its cap).
///
/// Built only by [`DecideRequest::checked`], so the note's cap holds for every
/// decision the council takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedDecision {
    /// Whether the owner approved the motion.
    pub approve: bool,
    /// The owner's note, held to [`MAX_DECISION_NOTE`] bytes, or `None`.
    pub note: Option<String>,
}

impl DecideRequest {
    /// Checks the request against the owner's decision caps, or says why the
    /// request is outside the endpoint's contract.
    ///
    /// `approve` is typed — an absent or non-boolean field never deserializes,
    /// so this is the 400 the endpoint's own contract raises before the store
    /// is reached. The note is optional; when present it is held to the same
    /// non-blank rule a vote's `reason` and a motion's `text` are.
    pub fn checked(&self) -> Result<CheckedDecision, String> {
        Ok(CheckedDecision {
            approve: self.approve,
            note: optional_text("note", &self.note, MAX_DECISION_NOTE)?,
        })
    }
}

/// Whether a motion of `kind`, naming `amount`, is owner-gated per 0067 against
/// a minted budget total of `minted_total`.
///
/// The always-gated kinds are gated whatever they name; a `budget_move` is
/// gated when the amount it names is over `OWNER_BUDGET_PERCENT`% of the
/// minted total (in integer arithmetic: `amount * percent > total`, so a
/// boundary amount that is exactly the fraction is *not* over it); every other
/// kind is the council's majority. With an empty ledger (`minted_total` zero)
/// any positive move is over the fraction of nothing, and a zero move is not —
/// which is the honest reading, not an invented floor.
pub fn derived_needs_owner(kind: Kind, amount: Option<u64>, minted_total: i128) -> bool {
    if kind.always_owner_gated() {
        return true;
    }
    if kind == Kind::BudgetMove {
        let amount = i128::from(amount.unwrap_or(0));
        return amount * OWNER_BUDGET_PERCENT > minted_total;
    }
    false
}

/// The 400 for a caller-supplied `needs_owner` that disagrees with the derived
/// gate, naming the rule the derivation applied.
fn owner_mismatch(kind: Kind, amount: Option<u64>, minted_total: i128, asked: bool) -> String {
    if asked {
        format!(
            "needs_owner is true, but a {} motion is below every 0067 threshold: the council \
             decides it by majority (0034), and v0 refuses setting the flag true to route a \
             motion around that majority",
            kind.as_str()
        )
    } else if kind == Kind::BudgetMove {
        let amount = amount.unwrap_or(0);
        format!(
            "needs_owner is false, but the {} names {amount} credits, which is over \
             {OWNER_BUDGET_PERCENT}% of the minted budget total {minted_total}: 0067 gates it \
             to the owner",
            kind.as_str()
        )
    } else {
        format!(
            "needs_owner is false, but a {} is always owner-gated: 0067 requires the owner to \
             ratify it",
            kind.as_str()
        )
    }
}

/// One vote as the API reports it: which seat cast it, which agent acted for
/// that seat, the word, the reason, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoteView {
    /// The seat that voted (its role in `docs/agents/initial-roles.json`).
    pub seat: &'static str,
    /// The guest CID of the agent that cast the seat's vote.
    pub agent_cid: u64,
    /// `yes`, `no` or `abstain`.
    pub vote: &'static str,
    /// Why, in the voter's own words.
    pub reason: String,
    /// When the vote was cast, in milliseconds since the Unix epoch.
    pub at: u64,
}

/// A motion's tally: the votes cast, against the four seats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TallyView {
    /// How many seats voted yes.
    pub yes: usize,
    /// How many voted no.
    pub no: usize,
    /// How many abstained.
    pub abstain: usize,
    /// How many seats have voted, however they voted.
    pub voted: usize,
    /// How many seats there are.
    pub seats: usize,
}

/// An owner's decision as the API reports it: whether the owner approved the
/// motion, the note they left (or `null`), and when.
///
/// **The owner is the operator**, so this names no agent: it is the record of
/// the operator's act, the way an [`crate::agora::AwardView`] records the
/// employer's. The stamp is the moment the decision was taken, in milliseconds
/// since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerDecisionView {
    /// Whether the owner approved the motion (`true` → `ratified`, `false` →
    /// `vetoed`).
    pub approve: bool,
    /// The owner's note, or `null` when they left none.
    pub note: Option<String>,
    /// When the owner decided, in milliseconds since the Unix epoch.
    pub at: u64,
}

/// One motion as the API reports it: the contract's schema, as this daemon
/// stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MotionView {
    /// The motion's id (`m<seq>`), monotonic and never reused.
    pub id: String,
    /// The guest CID of the agent that proposed it.
    pub proposer_cid: u64,
    /// The proposing seat (its role).
    pub proposer_role: &'static str,
    /// The kind, as the wire spells it.
    pub kind: &'static str,
    /// The motion itself.
    pub text: String,
    /// The amount a `budget_move` names, when the motion names one. Absent
    /// otherwise, so a motion that names no amount serialises exactly as the
    /// contract's schema does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    /// Whether the decision is owner-gated (derived; see the module note).
    pub needs_owner: bool,
    /// The state, as the wire spells it.
    pub state: &'static str,
    /// The seats' votes, in seat order (a re-vote keeps its seat's place).
    pub votes: Vec<VoteView>,
    /// The tally.
    pub tally: TallyView,
    /// The owner's decision, or `null` until the owner makes one. This is a
    /// **Column-style optional** (like the desktop client's `Column<T>`): the
    /// key is always present and reads `null` for "the owner has not decided",
    /// so a caller that distinguishes absent from null is never misled, and a
    /// decided motion carries `{approve, note|null, at}`.
    pub owner: Option<OwnerDecisionView>,
    /// When the motion was proposed, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the motion last changed (its proposal, or its latest vote), in
    /// milliseconds since the Unix epoch.
    pub updated_at: u64,
}

/// The motions endpoint's body: the motions, with the bounding line the journal
/// endpoint carries.
#[derive(Debug, Serialize)]
pub struct MotionsView {
    /// Up to the endpoint's limit of motions, newest first.
    pub motions: Vec<MotionView>,
    /// How many motions the council holds.
    pub total: usize,
    /// The limit the response was cut to.
    pub limit: usize,
    /// Whether the council holds more motions than the response carries.
    pub truncated: bool,
}

/// One seat's cast vote, as the store holds it: the word as the enum (so the
/// tally is exhaustive and typed) and the reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cast {
    agent_cid: u64,
    vote: Vote,
    reason: String,
    at: u64,
}

/// One owner's decision, as the store holds it: whether the owner approved,
/// the note they left (or none), and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Decision {
    approve: bool,
    note: Option<String>,
    at: u64,
}

/// One motion, as the store holds it: the votes keyed by seat, the gate the
/// daemon derived when it was proposed, and the owner's decision once they made
/// one.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Motion {
    id: String,
    proposer_cid: u64,
    /// The seat's name, as the seat table spells it.
    proposer_role: String,
    kind: Kind,
    text: String,
    amount: Option<u64>,
    needs_owner: bool,
    /// One slot per seat of [`SEATS`]; a re-vote replaces its slot, so a seat
    /// can never be counted twice.
    votes: [Option<Cast>; SEAT_COUNT],
    /// The owner's decision, or `None` while the owner has not made one. Once
    /// set it is never changed: the owner decides a motion once.
    decision: Option<Decision>,
    created_at: u64,
    updated_at: u64,
}

impl Motion {
    /// The motion's tally: the votes cast, against the four seats.
    fn tally(&self) -> TallyView {
        let mut yes = 0;
        let mut no = 0;
        let mut abstain = 0;
        for cast in self.votes.iter().flatten() {
            match cast.vote {
                Vote::Yes => yes += 1,
                Vote::No => no += 1,
                Vote::Abstain => abstain += 1,
            }
        }
        TallyView {
            yes,
            no,
            abstain,
            voted: yes + no + abstain,
            seats: SEAT_COUNT,
        }
    }

    /// The motion's state: the owner's decision when they have made one
    /// ([`State::Ratified`] or [`State::Vetoed`]), else the count's own state
    /// derived from its tally and its gate.
    fn state(&self) -> State {
        match &self.decision {
            Some(decision) => {
                if decision.approve {
                    State::Ratified
                } else {
                    State::Vetoed
                }
            }
            None => state_of(self.tally(), self.needs_owner),
        }
    }

    /// The motion as the API reports it.
    fn view(&self) -> MotionView {
        let votes = self
            .votes
            .iter()
            .enumerate()
            .filter_map(|(seat, cast)| {
                cast.as_ref().map(|cast| VoteView {
                    seat: SEATS[seat],
                    agent_cid: cast.agent_cid,
                    vote: cast.vote.as_str(),
                    reason: cast.reason.clone(),
                    at: cast.at,
                })
            })
            .collect();
        MotionView {
            id: self.id.clone(),
            proposer_cid: self.proposer_cid,
            proposer_role: SEATS[seat_of(&self.proposer_role)
                .expect("a motion's proposer holds a seat")],
            kind: self.kind.as_str(),
            text: self.text.clone(),
            amount: self.amount,
            needs_owner: self.needs_owner,
            state: self.state().as_str(),
            votes,
            tally: self.tally(),
            owner: self.decision.as_ref().map(|decision| OwnerDecisionView {
                approve: decision.approve,
                note: decision.note.clone(),
                at: decision.at,
            }),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// The state a motion with `tally` and gate `needs_owner` is in (0034's count).
///
/// The motion stays [`State::Open`] until all [`SEAT_COUNT`] seats have voted
/// (`abstain` counts toward that and never toward the majority). Then a
/// majority of yes over no carries it — to [`State::AwaitingOwner`] when the
/// gate says the owner must ratify, else [`State::Carried`] — a majority of no
/// rejects it, and an equal split is [`State::TieToOwner`]: the owner decides.
///
/// [`State::Ratified`] and [`State::Vetoed`] are never produced here: they are
/// the owner's decision ([`Motion::state`]), which no tally can derive. This
/// function is the count, and the count alone.
pub fn state_of(tally: TallyView, needs_owner: bool) -> State {
    if tally.voted < SEAT_COUNT {
        return State::Open;
    }
    if tally.yes > tally.no {
        return if needs_owner {
            State::AwaitingOwner
        } else {
            State::Carried
        };
    }
    if tally.no > tally.yes {
        return State::Rejected;
    }
    State::TieToOwner
}

/// The council's state: the motions it holds, and the bus every change is
/// published to.
pub struct Council {
    events: Arc<EventBus>,
    inner: Mutex<CouncilInner>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CouncilInner {
    /// The motions, oldest first. Motions are never removed and their ids are
    /// contiguous from `m1`.
    motions: Vec<Motion>,
    /// The number of the newest motion (`0` while the council holds none).
    seq: u64,
}

impl Council {
    /// A council holding no motions that publishes every change it takes to
    /// `events`.
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            events,
            inner: Mutex::new(CouncilInner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, CouncilInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The motions: the newest (bounded to `limit`), with the store's size and
    /// whether the response was cut short.
    pub fn motions(&self, limit: usize) -> MotionsView {
        let inner = self.lock();
        let total = inner.motions.len();
        let motions = inner
            .motions
            .iter()
            .rev()
            .take(limit)
            .map(Motion::view)
            .collect();
        MotionsView {
            motions,
            total,
            limit,
            truncated: total > limit,
        }
    }

    /// One motion by id, or `None` when the council holds none with it.
    pub fn motion(&self, id: &str) -> Option<MotionView> {
        let inner = self.lock();
        inner
            .motions
            .iter()
            .find(|motion| motion.id == id)
            .map(Motion::view)
    }

    /// Records `proposer`'s motion, or refuses it.
    ///
    /// The proposer's connectedness is the caller's to have established — the
    /// HTTP edge checks the register before it gets here, as it does for the
    /// claim and the post — and the motion's own caps and its derived gate were
    /// checked when it was built ([`ProposeRequest::checked`]). What is left is
    /// the council's own: the proposer's role must hold a seat
    /// ([`CouncilError::Unseated`], 403), and the store must have room
    /// ([`CouncilError::Conflict`], 409).
    pub fn propose(
        &self,
        proposer: &ConnectedAgent,
        proposal: CheckedProposal,
    ) -> Result<MotionView, CouncilError> {
        let seat = seated_index(proposer)?;
        let mut inner = self.lock();
        if inner.motions.len() >= MAX_MOTIONS {
            return Err(CouncilError::Conflict(format!(
                "the council holds its cap of {MAX_MOTIONS} motions"
            )));
        }
        inner.seq += 1;
        let at = now_ms();
        let motion = Motion {
            id: format!("m{}", inner.seq),
            proposer_cid: proposer.cid,
            proposer_role: SEATS[seat].to_string(),
            kind: proposal.kind,
            text: proposal.text,
            amount: proposal.amount,
            needs_owner: proposal.needs_owner,
            votes: std::array::from_fn(|_| None),
            decision: None,
            created_at: at,
            updated_at: at,
        };
        let view = motion.view();
        inner.motions.push(motion);
        // Published under the lock, so a stream sees motions in the order they
        // were taken — the ledger, the agora and the social layer publish their
        // transitions the same way.
        self.publish(&view);
        Ok(view)
    }

    /// Casts `voter`'s vote on motion `id`, or refuses it.
    ///
    /// The rules, each with its own refusal: the voter's role must hold a seat
    /// ([`CouncilError::Unseated`], 403); the motion must exist
    /// ([`CouncilError::Unknown`], 404); and the motion must still be `open`
    /// ([`CouncilError::Conflict`], 409, the state named). A vote from a seat
    /// that already voted **replaces** that seat's vote — the newest reason
    /// stands, and no seat is ever counted twice.
    pub fn vote(
        &self,
        id: &str,
        voter: &ConnectedAgent,
        cast: CheckedVote,
    ) -> Result<MotionView, CouncilError> {
        let seat = seated_index(voter)?;
        let mut inner = self.lock();
        let Some(index) = inner.motions.iter().position(|motion| motion.id == id) else {
            return Err(CouncilError::Unknown(format!("no motion with id {id}")));
        };
        let motion = &mut inner.motions[index];
        let state = motion.state();
        if state != State::Open {
            return Err(CouncilError::Conflict(format!(
                "motion {id} is {}, not open: a decided motion takes no further vote",
                state.as_str()
            )));
        }
        let at = now_ms();
        motion.votes[seat] = Some(Cast {
            agent_cid: voter.cid,
            vote: cast.vote,
            reason: cast.reason,
            at,
        });
        motion.updated_at = at;
        let view = motion.view();
        self.publish(&view);
        Ok(view)
    }

    /// Records the **owner's** decision on motion `id`, or refuses it.
    ///
    /// **The owner is the operator, not an agent**: this takes no
    /// [`ConnectedAgent`] and makes no seat check, because the operator holds
    /// no seat, has no role and is not on the register. The caller (the HTTP
    /// edge) has already checked the request's own contract
    /// ([`DecideRequest::checked`]) — `approve` is typed, the note is bounded —
    /// so what is left is the council's own, in order:
    ///
    /// * the motion must exist ([`CouncilError::Unknown`], 404);
    /// * it must be one the count gated to the owner — [`State::owner_decides`]
    ///   (`tie_to_owner` or `awaiting_owner`). A motion in any other state is
    ///   refused with the state named and the council's own decision left
    ///   standing ([`CouncilError::Conflict`], 409): a majority is not the
    ///   owner's to re-decide, and an `open` motion has not been decided at
    ///   all;
    /// * it must not already carry the owner's decision — the owner decides a
    ///   motion **once** ([`CouncilError::Conflict`], 409).
    ///
    /// A decision the store takes is recorded whole: `approve` true is
    /// [`State::Ratified`], false is [`State::Vetoed`], and the optional note
    /// is stored with the stamp. The decision is never changed afterwards and
    /// no later vote touches it (a decided motion takes no further vote, and
    /// such a vote is refused before any decision is consulted).
    pub fn decide(
        &self,
        id: &str,
        decision: CheckedDecision,
    ) -> Result<MotionView, CouncilError> {
        let mut inner = self.lock();
        let Some(index) = inner.motions.iter().position(|motion| motion.id == id) else {
            return Err(CouncilError::Unknown(format!("no motion with id {id}")));
        };
        let at = now_ms();
        let motion = &mut inner.motions[index];
        if motion.decision.is_some() {
            return Err(CouncilError::Conflict(format!(
                "motion {id} is already decided ({}): the owner decides a motion once",
                motion.state().as_str()
            )));
        }
        let state = motion.state();
        if !state.owner_decides() {
            return Err(CouncilError::Conflict(format!(
                "motion {id} is {}: the owner decides only a tie_to_owner or awaiting_owner \
                 motion — the council's own decision stands",
                state.as_str()
            )));
        }
        motion.decision = Some(Decision {
            approve: decision.approve,
            note: decision.note,
            at,
        });
        motion.updated_at = at;
        let view = motion.view();
        self.publish(&view);
        Ok(view)
    }

    /// Publishes one `motion` event: the motion's id, its kind and its state,
    /// and nothing else — a frame carries no text (see `crate::events`).
    fn publish(&self, motion: &MotionView) {
        self.events.publish(Event::Motion {
            id: motion.id.clone(),
            kind: motion.kind,
            state: motion.state,
        });
    }
}

/// The seat index of a connected agent, or the refusal that its role holds no
/// seat — the one seat check every seat action makes.
fn seated_index(agent: &ConnectedAgent) -> Result<usize, CouncilError> {
    match &agent.role {
        Some(role) => seat_of(role).ok_or_else(|| CouncilError::Unseated {
            cid: agent.cid,
            role: Some(role.clone()),
        }),
        None => Err(CouncilError::Unseated {
            cid: agent.cid,
            role: None,
        }),
    }
}


impl Council {
    /// The whole state as JSON: the motions, their votes and owner decisions,
    /// and the counter. The bus is not in it, and a restore publishes nothing.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        serde_json::to_string(&*inner).expect("the council state serializes")
    }

    /// Replaces the state with a [`Council::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: CouncilInner = serde_json::from_str(json)
            .map_err(|err| format!("the snapshot does not parse: {err}"))?;
        check_council_snapshot(&restored)?;
        *self.lock() = restored;
        Ok(())
    }
}

/// The invariants a council snapshot must hold before it may replace one: the
/// motions are the contiguous ids `m1`, `m2`, … in order, and the counter is at
/// least the newest id, so no id is ever handed out twice.
fn check_council_snapshot(inner: &CouncilInner) -> Result<(), String> {
    for (index, motion) in inner.motions.iter().enumerate() {
        let want = format!("m{}", index + 1);
        if motion.id != want {
            return Err(format!("motion {} is where {want} belongs", motion.id));
        }
        if !is_seated(&motion.proposer_role) {
            return Err(format!("motion {} names {} as its proposer seat", motion.id, motion.proposer_role));
        }
    }
    if inner.seq < inner.motions.len() as u64 {
        return Err(format!("the counter {} is behind its newest motion", inner.seq));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_restores_the_same_motions_and_refuses_garbage() {
        let (council, _notes) = lab();
        for (cid, seat, text) in [(100, SEATS[0], "move the budget"), (101, SEATS[1], "move it again")] {
            let mut request = proposal(Kind::BudgetMove, text, Some(100), None);
            request.proposer_cid = cid;
            let checked = request.checked(1000).expect("a proposal inside the contract");
            council.propose(&agent(cid, seat), checked).expect("a motion");
        }
        let json = council.snapshot();

        let (restored, _restored_notes) = lab();
        restored.restore(&json).expect("a snapshot restores");
        assert_eq!(
            serde_json::to_value(restored.motions(10)).expect("json"),
            serde_json::to_value(council.motions(10)).expect("json"),
        );
        assert_eq!(restored.snapshot(), json);

        // Garbage, a broken id sequence and a counter behind its motions are refused.
        assert!(restored.restore("not json").is_err());
        assert!(restored.restore(&json.replace("\"m2\"", "\"m9\"")).is_err());
        assert!(restored.restore(&json.replace("\"seq\":2", "\"seq\":1")).is_err());
        assert_eq!(restored.snapshot(), json);
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

    /// A council and the receiver its bus publishes to.
    fn lab() -> (Council, std::sync::mpsc::Receiver<Event>) {
        let events = EventBus::new();
        let notes = events.subscribe();
        (Council::new(events), notes)
    }

    /// The proposal a test varies: `kind`, `text`, `amount`, `needs_owner`.
    fn proposal(kind: Kind, text: &str, amount: Option<u64>, needs_owner: Option<bool>) -> ProposeRequest {
        ProposeRequest {
            proposer_cid: 0,
            kind,
            text: text.to_string(),
            amount,
            needs_owner,
        }
    }

    /// Proposes a motion as `role` with the fields a test varies, at a minted
    /// total of `minted_total`.
    fn propose(
        council: &Council,
        cid: u64,
        role: &str,
        kind: Kind,
        text: &str,
        minted_total: i128,
    ) -> MotionView {
        let mut request = proposal(kind, text, None, None);
        request.proposer_cid = cid;
        let checked = request
            .checked(minted_total)
            .expect("a proposal inside the contract");
        council.propose(&agent(cid, role), checked).expect("a motion")
    }

    /// The vote a test varies.
    fn vote(cid: u64, word: Vote, reason: &str) -> VoteRequest {
        VoteRequest {
            agent_cid: cid,
            vote: word,
            reason: reason.to_string(),
        }
    }

    /// The CID the seat at `seat` acts through: the cast the existing tests
    /// use, in [`SEATS`] order.
    const SEAT_CIDS: [u64; SEAT_COUNT] = [100, 103, 102, 104];

    /// Casts one vote per seat, in seat order, and answers the final view — the
    /// shortest way a test reaches a counted motion.
    fn vote_all(council: &Council, id: &str, words: [Vote; SEAT_COUNT]) -> MotionView {
        let mut view = council.motion(id).expect("the motion");
        for (seat, word) in words.iter().enumerate() {
            view = council
                .vote(
                    id,
                    &agent(SEAT_CIDS[seat], SEATS[seat]),
                    CheckedVote {
                        vote: *word,
                        reason: format!("the {} seat's reason", SEATS[seat]),
                    },
                )
                .expect("a vote");
        }
        view
    }

    /// The owner's decision a test varies, checked.
    fn decision(approve: bool, note: Option<&str>) -> CheckedDecision {
        DecideRequest {
            approve,
            note: note.map(String::from),
        }
        .checked()
        .expect("a decision inside the contract")
    }

    #[test]
    fn the_seat_table_is_the_doctrines_four_and_excludes_the_courts() {
        assert_eq!(SEATS.len(), SEAT_COUNT);
        assert_eq!(
            SEATS,
            [FINANCE, LEGAL_AGENTS_COUNSEL, INCIDENT, MAYOR_OFFICE]
        );
        for (index, seat) in SEATS.iter().enumerate() {
            assert_eq!(seat_of(seat), Some(index), "{seat} is a seat of its own");
        }
        // The doctrine's exclusion is a fact of this module, not an absence: a
        // judge or a prosecutor holds no seat, and neither does any other role.
        for role in EXCLUDED_ROLES {
            assert!(!is_seated(role), "{role} never sits (S-008a)");
        }
        for role in ["builder", "mayor", "legal-agents-defender", "legal-gov", ""] {
            assert!(!is_seated(role), "{role} holds no seat");
        }
    }

    #[test]
    fn the_derived_gate_follows_the_kind_and_the_budget_fraction() {
        // Always-gated kinds, whatever they name.
        for kind in [Kind::LawProposal, Kind::TownProposal, Kind::MayorReplacement] {
            assert!(derived_needs_owner(kind, None, 0), "{kind:?} is always gated");
            assert!(derived_needs_owner(kind, Some(1), 1000));
        }
        // The proposer's kinds are the council's majority.
        for kind in [Kind::Priority, Kind::Other] {
            assert!(!derived_needs_owner(kind, None, 1000), "{kind:?} is not gated");
            assert!(!derived_needs_owner(kind, Some(999), 1000));
        }
        // A budget_move is gated strictly over 10% of the minted total.
        assert!(!derived_needs_owner(Kind::BudgetMove, Some(100), 1000), "10% exactly is not over");
        assert!(derived_needs_owner(Kind::BudgetMove, Some(101), 1000));
        // An empty ledger gates any positive move and no zero move.
        assert!(derived_needs_owner(Kind::BudgetMove, Some(1), 0));
        assert!(!derived_needs_owner(Kind::BudgetMove, Some(0), 0));
    }

    #[test]
    fn a_caller_flag_that_disagrees_with_the_derivation_is_a_400() {
        // Understating an always-gated kind names the rule.
        let under = proposal(Kind::LawProposal, "a law", None, Some(false))
            .checked(1000)
            .unwrap_err();
        assert!(under.contains("always owner-gated"), "{under}");
        assert!(under.contains("0067"), "{under}");

        // Understating the budget rule names the amount, the fraction and the
        // total it read.
        let under = proposal(Kind::BudgetMove, "move it", Some(500), Some(false))
            .checked(1000)
            .unwrap_err();
        assert!(under.contains("500"), "{under}");
        assert!(under.contains("10%"), "{under}");
        assert!(under.contains("1000"), "{under}");

        // Overstating is refused too: the majority is not routed around.
        let over = proposal(Kind::Priority, "a priority", None, Some(true))
            .checked(1000)
            .unwrap_err();
        assert!(over.contains("needs_owner is true"), "{over}");
        assert!(over.contains("majority"), "{over}");

        // Agreeing with the derivation is accepted, and the derived value is
        // what the motion keeps.
        let agreed = proposal(Kind::LawProposal, "a law", None, Some(true))
            .checked(1000)
            .expect("agreeing is inside the contract");
        assert!(agreed.needs_owner);
        let plain = proposal(Kind::Priority, "a priority", None, Some(false))
            .checked(1000)
            .expect("agreeing is inside the contract");
        assert!(!plain.needs_owner);
    }

    #[test]
    fn a_budget_move_must_name_its_amount() {
        let refused = proposal(Kind::BudgetMove, "move some budget", None, None)
            .checked(1000)
            .unwrap_err();
        assert!(refused.contains("must name an amount"), "{refused}");
        assert!(refused.contains("10%"), "{refused}");
    }

    #[test]
    fn the_text_caps_hold() {
        assert_eq!(
            proposal(Kind::Priority, "  ", None, None).checked(0).unwrap_err(),
            "text must not be empty"
        );
        let long = "x".repeat(MAX_MOTION_TEXT + 1);
        assert_eq!(
            proposal(Kind::Priority, &long, None, None)
                .checked(0)
                .unwrap_err(),
            format!("text must be at most {MAX_MOTION_TEXT} bytes")
        );
        let reason = "y".repeat(MAX_REASON + 1);
        let refused = vote(100, Vote::Yes, &reason).checked().unwrap_err();
        assert_eq!(refused, format!("reason must be at most {MAX_REASON} bytes"));
        assert_eq!(
            vote(100, Vote::Yes, " ").checked().unwrap_err(),
            "reason must not be empty"
        );
    }

    #[test]
    fn only_the_seats_propose_and_vote() {
        let (council, _notes) = lab();
        let checked = proposal(Kind::Priority, "a priority", None, None)
            .checked(0)
            .expect("inside the contract");
        let refused = council
            .propose(&agent(100, "builder"), checked)
            .unwrap_err();
        assert_eq!(
            refused,
            CouncilError::Unseated {
                cid: 100,
                role: Some("builder".to_string())
            }
        );
        assert_eq!(refused.status(), 403);
        assert_eq!(
            refused.to_string(),
            "the agent on guest CID 100 is role builder, which holds no council seat"
        );

        let refused = council
            .propose(
                &ConnectedAgent {
                    cid: 101,
                    role: None,
                    run: "r1".to_string(),
                    vm: 0,
                },
                proposal(Kind::Priority, "a priority", None, None)
                    .checked(0)
                    .expect("inside the contract"),
            )
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the agent on guest CID 101 has no role, so it holds no council seat"
        );

        // A motion proposed by a seat, and a vote from a role that holds none.
        let motion = propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        let refused = council
            .vote("m1", &agent(200, "builder"), CheckedVote {
                vote: Vote::Yes,
                reason: "looks fine".to_string(),
            })
            .unwrap_err();
        assert_eq!(refused.status(), 403);
        assert!(refused.to_string().contains("builder"), "{refused}");
        assert_eq!(motion.state, "open");
    }

    #[test]
    fn the_propose_refusals_are_the_stores_own() {
        let (council, _notes) = lab();
        let motion = propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        assert_eq!(motion.id, "m1");
        assert_eq!(motion.proposer_role, FINANCE);
        assert_eq!(motion.kind, "priority");
        assert!(!motion.needs_owner);
        assert_eq!(motion.state, "open");
        assert_eq!(motion.created_at, motion.updated_at);

        // An unknown id names nothing.
        let refused = council
            .vote("m9", &agent(100, FINANCE), CheckedVote {
                vote: Vote::Yes,
                reason: "a reason".to_string(),
            })
            .unwrap_err();
        assert_eq!(refused.status(), 404);
        assert_eq!(refused.to_string(), "no motion with id m9");
    }

    #[test]
    fn the_count_table_is_exhaustive() {
        // All four yes: carried below the gate, awaiting_owner at it.
        let all_yes = TallyView { yes: 4, no: 0, abstain: 0, voted: 4, seats: 4 };
        assert_eq!(state_of(all_yes, false), State::Carried);
        assert_eq!(state_of(all_yes, true), State::AwaitingOwner);

        // 3 yes, 1 no: carried.
        assert_eq!(
            state_of(TallyView { yes: 3, no: 1, abstain: 0, voted: 4, seats: 4 }, false),
            State::Carried
        );
        // 3 yes, 0 no, 1 abstain: abstain never counts toward the majority.
        assert_eq!(
            state_of(TallyView { yes: 3, no: 0, abstain: 1, voted: 4, seats: 4 }, false),
            State::Carried
        );
        // 2 yes, 1 no, 1 abstain: a relative majority carries.
        assert_eq!(
            state_of(TallyView { yes: 2, no: 1, abstain: 1, voted: 4, seats: 4 }, false),
            State::Carried
        );
        // 2-2: the doctrine's tie.
        assert_eq!(
            state_of(TallyView { yes: 2, no: 2, abstain: 0, voted: 4, seats: 4 }, false),
            State::TieToOwner
        );
        // An equal split with abstentions is the same no-majority.
        assert_eq!(
            state_of(TallyView { yes: 1, no: 1, abstain: 2, voted: 4, seats: 4 }, false),
            State::TieToOwner
        );
        // A majority of no rejects.
        assert_eq!(
            state_of(TallyView { yes: 1, no: 3, abstain: 0, voted: 4, seats: 4 }, false),
            State::Rejected
        );
        assert_eq!(
            state_of(TallyView { yes: 0, no: 2, abstain: 2, voted: 4, seats: 4 }, false),
            State::Rejected
        );
        // Fewer than four voted is open, whatever the split.
        assert_eq!(
            state_of(TallyView { yes: 3, no: 0, abstain: 0, voted: 3, seats: 4 }, false),
            State::Open
        );
        assert_eq!(
            state_of(TallyView { yes: 0, no: 0, abstain: 1, voted: 1, seats: 4 }, true),
            State::Open
        );
    }

    #[test]
    fn a_full_count_decides_and_a_re_vote_replaces() {
        let (council, notes) = lab();
        let motion = propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        assert_eq!(motion.id, "m1");
        // The propose published one frame, open.
        assert_eq!(
            notes.recv().expect("the propose frame"),
            Event::Motion {
                id: "m1".to_string(),
                kind: "priority",
                state: "open"
            }
        );

        // Two yes votes leave it open.
        let yes = |_cid: u64, reason: &str| CheckedVote {
            vote: Vote::Yes,
            reason: reason.to_string(),
        };
        council
            .vote("m1", &agent(100, FINANCE), yes(100, "sound"))
            .expect("a vote");
        council
            .vote("m1", &agent(103, LEGAL_AGENTS_COUNSEL), yes(103, "lawful"))
            .expect("a vote");
        assert_eq!(council.motion("m1").expect("the motion").state, "open");

        // The Finance seat re-votes: the seat still holds exactly one vote, and
        // the newest reason stands — the tally does not double-count it.
        let replaced = council
            .vote(
                "m1",
                &agent(100, FINANCE),
                CheckedVote {
                    vote: Vote::Abstain,
                    reason: "the evidence changed".to_string(),
                },
            )
            .expect("a re-vote");
        assert_eq!(replaced.votes.len(), 2, "one vote per seat");
        assert_eq!(replaced.tally.voted, 2, "a re-vote does not double-count");
        assert_eq!(replaced.tally.abstain, 1);
        assert_eq!(replaced.tally.yes, 1);
        let finance = replaced
            .votes
            .iter()
            .find(|vote| vote.seat == FINANCE)
            .expect("the finance seat's vote");
        assert_eq!(finance.vote, "abstain");
        assert_eq!(finance.reason, "the evidence changed");
        assert_eq!(replaced.state, "open", "two seats still have not voted");

        // The remaining two seats vote, and the third yes carries it: 3 yes of
        // the four cast (one abstained).
        council
            .vote("m1", &agent(100, FINANCE), yes(100, "sound after all"))
            .expect("a re-vote");
        council
            .vote("m1", &agent(102, INCIDENT), yes(102, "no risk"))
            .expect("a vote");
        let decided = council
            .vote("m1", &agent(104, MAYOR_OFFICE), yes(104, "the towns agree"))
            .expect("the deciding vote");
        assert_eq!(decided.state, "carried");
        assert_eq!(decided.tally.yes, 4);
        assert_eq!(decided.tally.voted, 4);
        assert!(decided.updated_at >= decided.created_at);

        // A further vote is refused with the state named.
        let refused = council
            .vote("m1", &agent(100, FINANCE), yes(100, "again"))
            .unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m1 is carried, not open: a decided motion takes no further vote"
        );
        // And nothing about the decided motion changed.
        let after = council.motion("m1").expect("the motion");
        assert_eq!(after, decided);
    }

    #[test]
    fn a_tie_escalates_and_a_gated_majority_waits() {
        let (council, _notes) = lab();
        // A non-gated motion that ties 2-2 escalates to the owner.
        let tie = propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        assert_eq!(tie.id, "m1");
        let cast = |cid: u64, seat: &str, word: Vote, reason: &str| {
            council
                .vote(
                    "m1",
                    &agent(cid, seat),
                    CheckedVote {
                        vote: word,
                        reason: reason.to_string(),
                    },
                )
                .expect("a vote")
        };
        cast(100, FINANCE, Vote::Yes, "yes");
        cast(103, LEGAL_AGENTS_COUNSEL, Vote::No, "no");
        cast(102, INCIDENT, Vote::Yes, "yes");
        let decided = cast(104, MAYOR_OFFICE, Vote::No, "no");
        assert_eq!(decided.state, "tie_to_owner");
        assert_eq!(decided.tally.yes, 2);
        assert_eq!(decided.tally.no, 2);

        // A law proposal is always gated: carried, and still `awaiting_owner`.
        let law = council
            .propose(
                &agent(100, FINANCE),
                proposal(Kind::LawProposal, "a new law", None, None)
                    .checked(0)
                    .expect("inside the contract"),
            )
            .expect("a motion");
        assert_eq!(law.id, "m2");
        assert!(law.needs_owner);
        for (cid, seat, word) in [
            (100, FINANCE, Vote::Yes),
            (103, LEGAL_AGENTS_COUNSEL, Vote::Yes),
            (102, INCIDENT, Vote::Yes),
            (104, MAYOR_OFFICE, Vote::No),
        ] {
            let decided = council
                .vote(
                    "m2",
                    &agent(cid, seat),
                    CheckedVote {
                        vote: word,
                        reason: "the record decides".to_string(),
                    },
                )
                .expect("a vote");
            if decided.state != "open" {
                assert_eq!(decided.state, "awaiting_owner", "carried, but gated");
                assert_eq!(decided.tally.yes, 3);
                assert_eq!(decided.tally.no, 1);
            }
        }
    }

    #[test]
    fn every_transition_publishes_a_motion_frame() {
        let (council, notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        let frames = |notes: &std::sync::mpsc::Receiver<Event>| -> Vec<Event> {
            let mut events = Vec::new();
            while let Ok(event) = notes.try_recv() {
                events.push(event);
            }
            events
        };
        assert_eq!(
            frames(&notes),
            vec![Event::Motion {
                id: "m1".to_string(),
                kind: "priority",
                state: "open"
            }]
        );
        for (cid, seat, word) in [
            (100, FINANCE, Vote::Yes),
            (103, LEGAL_AGENTS_COUNSEL, Vote::Yes),
            (102, INCIDENT, Vote::Yes),
            (104, MAYOR_OFFICE, Vote::Yes),
        ] {
            council
                .vote(
                    "m1",
                    &agent(cid, seat),
                    CheckedVote {
                        vote: word,
                        reason: "the record decides".to_string(),
                    },
                )
                .expect("a vote");
        }
        // Four vote frames, the last carrying the decided state — and no text
        // in any of them.
        let events = frames(&notes);
        assert_eq!(events.len(), 4);
        assert_eq!(
            events.last().expect("the deciding frame"),
            &Event::Motion {
                id: "m1".to_string(),
                kind: "priority",
                state: "carried"
            }
        );
        assert_eq!(
            events.last().expect("the deciding frame").frame(),
            "event: motion\ndata: {\"id\":\"m1\",\"kind\":\"priority\",\"state\":\"carried\"}\n\n"
        );
    }

    #[test]
    fn the_views_are_bounded_newest_first() {
        let (council, _notes) = lab();
        for index in 0..5 {
            propose(
                &council,
                100,
                FINANCE,
                Kind::Priority,
                &format!("agenda {index}"),
                0,
            );
        }
        let all = council.motions(MOTIONS_LIMIT);
        assert_eq!(all.total, 5);
        assert!(!all.truncated);
        let ids: Vec<&str> = all.motions.iter().map(|motion| motion.id.as_str()).collect();
        assert_eq!(ids, vec!["m5", "m4", "m3", "m2", "m1"], "newest first");

        let window = council.motions(2);
        assert_eq!(window.total, 5);
        assert_eq!(window.limit, 2);
        assert!(window.truncated);
        let ids: Vec<&str> = window
            .motions
            .iter()
            .map(|motion| motion.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m5", "m4"]);

        assert!(council.motion("m3").is_some());
        assert!(council.motion("m9").is_none());
    }

    #[test]
    fn the_store_caps_motions() {
        let (council, _notes) = lab();
        for _ in 0..MAX_MOTIONS {
            propose(&council, 100, FINANCE, Kind::Other, "a motion", 0);
        }
        let refused = council
            .propose(
                &agent(100, FINANCE),
                proposal(Kind::Other, "one too many", None, None)
                    .checked(0)
                    .expect("inside the contract"),
            )
            .unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            format!("the council holds its cap of {MAX_MOTIONS} motions")
        );
        assert_eq!(council.motions(MOTIONS_LIMIT).total, MAX_MOTIONS);
    }

    #[test]
    fn the_owner_decides_the_gated_states_and_ratifies_or_vetoes() {
        let (council, _notes) = lab();

        // A 2-2 tie escalates to the owner; their `approve` ratifies it.
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        let tied = vote_all(&council, "m1", [Vote::Yes, Vote::No, Vote::Yes, Vote::No]);
        assert_eq!(tied.state, "tie_to_owner");
        assert_eq!(tied.owner, None, "the owner has not decided");
        let ratified = council
            .decide("m1", decision(true, Some("carry it")))
            .expect("the owner decides");
        assert_eq!(ratified.state, "ratified");
        let owner = ratified.owner.clone().expect("the recorded decision");
        assert!(owner.approve);
        assert_eq!(owner.note.as_deref(), Some("carry it"));
        assert!(owner.at >= ratified.created_at, "the stamp is the decision's own moment");
        assert_eq!(ratified.tally, tied.tally, "the decision touches no tally");
        assert_eq!(ratified.votes, tied.votes, "and no vote");
        assert!(ratified.updated_at >= tied.updated_at);

        // A second tie, vetoed.
        propose(&council, 100, FINANCE, Kind::Priority, "another agenda", 0);
        vote_all(&council, "m2", [Vote::No, Vote::Yes, Vote::No, Vote::Yes]);
        let vetoed = council
            .decide("m2", decision(false, None))
            .expect("the owner decides");
        assert_eq!(vetoed.state, "vetoed");
        let owner = vetoed.owner.clone().expect("the recorded decision");
        assert!(!owner.approve);
        assert_eq!(owner.note, None, "an absent note is null");

        // A gated carry (`awaiting_owner`) is the owner's too: both ways.
        propose(&council, 100, FINANCE, Kind::LawProposal, "a new law", 0);
        let waited = vote_all(&council, "m3", [Vote::Yes, Vote::Yes, Vote::Yes, Vote::No]);
        assert_eq!(waited.state, "awaiting_owner");
        assert!(waited.needs_owner);
        let ratified = council
            .decide("m3", decision(true, None))
            .expect("the owner ratifies");
        assert_eq!(ratified.state, "ratified");
        assert!(ratified.owner.as_ref().expect("the decision").approve);

        propose(&council, 100, FINANCE, Kind::LawProposal, "another law", 0);
        vote_all(&council, "m4", [Vote::Yes, Vote::Yes, Vote::No, Vote::Yes]);
        let vetoed = council
            .decide("m4", decision(false, Some("not yet")))
            .expect("the owner vetoes");
        assert_eq!(vetoed.state, "vetoed");
        assert_eq!(
            vetoed.owner.as_ref().expect("the decision").note.as_deref(),
            Some("not yet")
        );
    }

    #[test]
    fn the_owner_decides_only_a_gated_motion() {
        let (council, _notes) = lab();

        // `open`: the council has not decided — nothing for the owner to take.
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        let refused = council.decide("m1", decision(true, None)).unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m1 is open: the owner decides only a tie_to_owner or awaiting_owner motion — \
             the council's own decision stands"
        );

        // `carried`: the council's majority decided it; a majority is not the
        // owner's to re-decide.
        vote_all(&council, "m1", [Vote::Yes, Vote::Yes, Vote::Yes, Vote::No]);
        let carried = council.motion("m1").expect("the motion");
        assert_eq!(carried.state, "carried");
        let refused = council.decide("m1", decision(true, None)).unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m1 is carried: the owner decides only a tie_to_owner or awaiting_owner motion \
             — the council's own decision stands"
        );

        // `rejected`: the same, the other way.
        propose(&council, 100, FINANCE, Kind::Priority, "a second agenda", 0);
        vote_all(&council, "m2", [Vote::No, Vote::No, Vote::Yes, Vote::No]);
        assert_eq!(council.motion("m2").expect("the motion").state, "rejected");
        let refused = council.decide("m2", decision(false, None)).unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m2 is rejected: the owner decides only a tie_to_owner or awaiting_owner motion \
             — the council's own decision stands"
        );

        // Nothing the owner was refused recorded a decision.
        assert_eq!(council.motion("m1").expect("the motion").owner, None);
        assert_eq!(council.motion("m2").expect("the motion").owner, None);

        // An unknown id names nothing.
        let unknown = council.decide("m9", decision(true, None)).unwrap_err();
        assert_eq!(unknown.status(), 404);
        assert_eq!(unknown.to_string(), "no motion with id m9");
    }

    #[test]
    fn the_owner_decides_a_motion_once() {
        let (council, _notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        vote_all(&council, "m1", [Vote::Yes, Vote::No, Vote::Yes, Vote::No]);
        council
            .decide("m1", decision(true, Some("carry it")))
            .expect("the owner decides");

        // A second decision is refused, and the first stands untouched.
        let refused = council.decide("m1", decision(false, Some("changed my mind"))).unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m1 is already decided (ratified): the owner decides a motion once"
        );
        let after = council.motion("m1").expect("the motion");
        assert_eq!(after.state, "ratified");
        let owner = after.owner.expect("the first decision stands");
        assert!(owner.approve);
        assert_eq!(owner.note.as_deref(), Some("carry it"));
    }

    #[test]
    fn a_decided_motion_takes_no_further_vote() {
        let (council, _notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        vote_all(&council, "m1", [Vote::Yes, Vote::No, Vote::Yes, Vote::No]);
        council
            .decide("m1", decision(false, None))
            .expect("the owner vetoes");

        // The vote path's guard now reads `vetoed`: a decided motion takes no
        // further vote, the state named.
        let refused = council
            .vote(
                "m1",
                &agent(100, FINANCE),
                CheckedVote {
                    vote: Vote::Yes,
                    reason: "again".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "motion m1 is vetoed, not open: a decided motion takes no further vote"
        );
        let after = council.motion("m1").expect("the motion");
        assert_eq!(after.state, "vetoed");
        assert_eq!(after.tally.voted, SEAT_COUNT, "the tally is untouched");
    }

    #[test]
    fn a_decide_body_needs_a_boolean_approve_and_a_bounded_note() {
        // `approve` is required and typed: absent, non-boolean and wrong-typed
        // never deserialize (the 400's first enforcement point).
        for body in [
            r#"{}"#,
            r#"{"note":"a note"}"#,
            r#"{"approve":"yes"}"#,
            r#"{"approve":1}"#,
            r#"{"approve":null}"#,
        ] {
            assert!(
                serde_json::from_str::<DecideRequest>(body).is_err(),
                "{body} is outside the contract"
            );
        }
        let bare: DecideRequest = serde_json::from_str(r#"{"approve":true}"#).expect("a bare approve");
        assert!(bare.approve);
        assert_eq!(bare.note, None);

        // The note is optional; absent is `null` and inside the contract.
        assert_eq!(decision(true, None).note, None);

        // Blank is refused in the shared text rule's words.
        assert_eq!(
            DecideRequest { approve: false, note: Some("   ".to_string()) }
                .checked()
                .unwrap_err(),
            "note must not be empty"
        );

        // The cap is the contract's 1000 bytes, pinned as a literal so widening
        // the constant does not silently re-baseline the test.
        assert_eq!(MAX_DECISION_NOTE, 1000);
        let long = "x".repeat(1001);
        assert_eq!(
            DecideRequest { approve: true, note: Some(long) }.checked().unwrap_err(),
            "note must be at most 1000 bytes".to_string()
        );
        let at_cap = "y".repeat(1000);
        let accepted = DecideRequest { approve: true, note: Some(at_cap.clone()) }
            .checked()
            .expect("at the cap is inside the contract");
        assert_eq!(accepted.note.as_deref(), Some(at_cap.as_str()));
    }

    #[test]
    fn the_owner_field_is_a_column_style_optional() {
        let (council, _notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);

        // The key is always present, and reads `null` until the owner decides —
        // never absent, so a caller that distinguishes absent from null is not
        // misled.
        let before =
            serde_json::to_value(council.motion("m1").expect("the motion")).expect("a view");
        assert!(before.get("owner").is_some(), "the owner key is always present: {before}");
        assert!(before["owner"].is_null(), "and null until decided: {before}");

        vote_all(&council, "m1", [Vote::Yes, Vote::No, Vote::Yes, Vote::No]);
        assert_eq!(
            council.motion("m1").expect("the motion").owner,
            None,
            "a gated motion the owner has not decided carries no decision"
        );

        let decided = council
            .decide("m1", decision(true, Some("carry it")))
            .expect("the owner decides");
        let at = decided.owner.as_ref().expect("the decision").at;
        let after = serde_json::to_value(&decided).expect("a view");
        assert_eq!(
            after["owner"],
            serde_json::json!({"approve": true, "note": "carry it", "at": at}),
            "{after}"
        );
        // An absent note is `null` in the object, not an omitted key.
        propose(&council, 100, FINANCE, Kind::LawProposal, "a law", 0);
        vote_all(&council, "m2", [Vote::Yes, Vote::Yes, Vote::Yes, Vote::Yes]);
        let decided = council
            .decide("m2", decision(false, None))
            .expect("the owner vetoes");
        let after = serde_json::to_value(&decided).expect("a view");
        assert_eq!(after["state"], "vetoed");
        assert!(after["owner"].get("note").is_some(), "{after}");
        assert!(after["owner"]["note"].is_null(), "{after}");
    }

    #[test]
    fn the_owners_decision_publishes_the_motion_frame() {
        let (council, notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        vote_all(&council, "m1", [Vote::Yes, Vote::No, Vote::Yes, Vote::No]);
        // The propose and vote frames are not what is under test here.
        while notes.try_recv().is_ok() {}

        council
            .decide("m1", decision(true, Some("carry it")))
            .expect("the owner decides");
        let frame = notes.try_recv().expect("the decision's frame");
        assert_eq!(
            frame,
            Event::Motion {
                id: "m1".to_string(),
                kind: "priority",
                state: "ratified"
            }
        );
        // The existing frame, byte for byte — and no text.
        assert_eq!(
            frame.frame(),
            "event: motion\ndata: {\"id\":\"m1\",\"kind\":\"priority\",\"state\":\"ratified\"}\n\n"
        );
        assert!(notes.try_recv().is_err(), "exactly one frame on the decision");

        // A veto publishes the same frame, carrying `vetoed`.
        propose(&council, 100, FINANCE, Kind::Priority, "another agenda", 0);
        vote_all(&council, "m2", [Vote::No, Vote::Yes, Vote::No, Vote::Yes]);
        while notes.try_recv().is_ok() {}
        council
            .decide("m2", decision(false, None))
            .expect("the owner vetoes");
        let frame = notes.try_recv().expect("the veto's frame");
        assert_eq!(
            frame.frame(),
            "event: motion\ndata: {\"id\":\"m2\",\"kind\":\"priority\",\"state\":\"vetoed\"}\n\n"
        );
    }

    #[test]
    fn the_decide_refusals_record_nothing() {
        let (council, notes) = lab();
        propose(&council, 100, FINANCE, Kind::Priority, "an agenda", 0);
        while notes.try_recv().is_ok() {}

        // An `open` refusal, an unknown id: neither touches the store nor the bus.
        assert!(council.decide("m1", decision(true, None)).is_err());
        assert!(council.decide("m9", decision(true, None)).is_err());
        assert!(notes.try_recv().is_err(), "a refusal publishes no frame");
        let after = council.motion("m1").expect("the motion");
        assert_eq!(after.state, "open");
        assert_eq!(after.owner, None);
    }
}
