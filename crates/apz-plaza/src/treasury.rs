// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The treasury v0: the plaza's accounts, its double-entry journal, and the
//! task budgets that become real escrow.
//!
//! The model is `docs/treasury-draft/schema.sql` (`treasury.*`) restricted to
//! what noded v0 needs, and the moves it implements are the ones that schema
//! and `docs/formal/Economy.tla` already describe: a task budget is minted into
//! the task's escrow, and a task that ends refunds what its escrow still holds.
//! Nothing here invents a transition those two do not have.
//!
//! # Accounts
//!
//! An account is the schema's `treasury.accounts` row: a [`AccountKind`], an
//! owner (`owner_ref` — a task id, an agent id, or a fixed name) and a
//! `credit_limit`. The kinds exist for every account the schema's enum lists,
//! so the type is complete, but **an account is opened only when something
//! needs it**:
//!
//! * `mint` is opened once, when the treasury is created: it is the source of
//!   all credits and every mint and refund names it, so it always exists.
//! * `task_escrow` is opened lazily, one per task, by the first entry that
//!   names it (a task's budget mint).
//! * `external_spend` is opened lazily, by the first `spend` or `model` entry:
//!   the provider's account is the one every metered real cost is credited to.
//! * `treasury`, `town_budget` and `agent_earnings` are likewise opened lazily
//!   by the first entry that names them. v0's task lifecycle posts none of
//!   them, so they are reachable only through [`Treasury::post`] and
//!   [`Treasury::open_account`]; the posting rules for them (`fine`, `fee`,
//!   `payment`, `clawback`, `budget_move`, `subcontract`) are enforced all the
//!   same, and covered by the rule matrix test.
//!
//! # The journal
//!
//! The journal is the schema's `treasury.entries` and `treasury.postings`
//! together: one [`Entry`] per move, each carrying its postings. An entry's
//! [`EntryKind`] decides which `(account kind, side)` pairs its postings may
//! touch — [`POSTING_RULES`] is the schema's `treasury.posting_rules` table
//! verbatim, and [`Treasury::post`] refuses any posting outside it with a
//! [`PostError::Rule`] that names the rule, in the schema's own words
//! (`fine entry may not debit a task_escrow account`). On top of the rules, an
//! entry is held to the rest of the schema's `treasury.check_entry`, in
//! miniature:
//!
//! 1. it has at least one posting, and no posting is zero (`amount <> 0`);
//! 2. one posting per account (the schema's `postings` primary key);
//! 3. money that moves for a task names its chain: every kind but `fine` and
//!    `fee` needs a `root_task` (the schema's `task_money_names_chain`);
//! 4. its postings balance to zero (the schema's `does not balance`);
//! 5. no touched account falls below `-credit_limit` — for `task_escrow`,
//!    `agent_earnings`, `treasury` and `town_budget`, whose default limit is 0,
//!    that is the schema's "never negative", and it is why **an escrow can
//!    never go negative** (see [`Treasury::post`]);
//! 6. a `payment` respects the chain's 20 % margin cap over real cost (0111).
//!
//! Balances are **derived, never stored**: [`Treasury::accounts`] and
//! [`Treasury::view`] compute each account's balance by summing its postings,
//! exactly as the schema's `treasury.balances` view does. There is no field
//! holding a balance and no API that sets one.
//!
//! The journal is **append-only**: an id is assigned once, in order, and no
//! method updates or deletes an entry. The schema enforces this with its
//! `forbid_change` triggers; this crate has no database of its own, so the guard
//! is the type-level absence of any mutator (entries are handed out as owned snapshots) plus
//! [`Treasury::verify_journal`], which re-checks that the ids are exactly
//! `1..=n` and every posting names a live account. A correction is a new
//! entry, never an edit.
//!
//! # What v0 does not do (stated, not faked)
//!
//! * **Real spend is metered for model calls and `spend` entries only.** A
//!   task's real cost is what its `model` and `spend` entries credit to
//!   `external_spend`; tools and compute are not metered yet, so a task that
//!   ends refunds what its escrow still holds, not a figure net of them. The
//!   treasury view's `spent` is the sum of those credits.
//! * **No subcontracts, payments, clawbacks, fines, fees or budget moves.**
//!   Their kinds, accounts and posting rules are all present and enforced, but
//!   nothing posts them at v0 — they belong to the Agora and sanction slices.
//! * **No credit line above zero.** `agent_earnings` may go negative down to
//!   `-credit_limit`, and the rule is enforced, but v0 has no reputation slice
//!   to set a limit, so every lazily opened account keeps the schema's default
//!   of 0 (an account's limit can be set through [`Treasury::open_account`]).
//! * **Persistence is apid's job.** Accounts and the journal are held in memory
//!   by this crate, and apid snapshots them (`snapshot` / `restore`): into
//!   Postgres (`plaza_state`) when it is configured, else `treasury.json` in the
//!   state dir. The schema's `treasury.balances` and `treasury.chain_margin`
//!   views are the derivations [`Treasury::view`] and [`Treasury::post`]'s
//!   margin check perform.
//!
//! The caps v0 chooses, and refuses past: [`MAX_ENTRIES`] and [`MAX_ACCOUNTS`]
//! (the journal and the accounts table are bounded, like the task ledger), and
//! [`JOURNAL_LIMIT`] for the journal endpoint's response.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::now_ms;

/// The `owner_ref` of the single `mint` account: the fixed name the schema's
/// tests give it.
pub const MINT_OWNER: &str = "mint";

/// The `owner_ref` of the plaza's `town_budget` account: the pool every task is
/// funded from and every refund returns to.
pub const PLAZA_OWNER: &str = "plaza";

/// The credits the plaza's budget is minted with when the treasury is created.
pub const PLAZA_BUDGET: i64 = 100_000;

/// The most credits one agent may transfer to others per UTC day.
pub const TRANSFER_DAILY_CAP: i64 = 1_000;

/// One UTC day, in the clock's milliseconds.
const DAY_MS: u64 = 86_400_000;

/// The `posted_by` of every entry: the schema's `posted_by_service` allows only
/// service names, and every entry here is the treasury service's own.
pub const POSTED_BY: &str = "treasury-service";

/// The chain margin cap, as a percentage of the chain's real cost (0111). The
/// schema's `treasury.margin_cap_pct()` returns this value.
pub const MARGIN_CAP_PCT: i64 = 20;

/// The most entries the journal holds. v0 never removes an entry (the journal
/// is append-only), so the cap is absolute: past it [`Treasury::post`] refuses
/// with an honest error rather than dropping the oldest entry. The task
/// lifecycle posts at most two entries per task (a mint and a refund) and the
/// ledger holds at most [`crate::tasks::MAX_TASKS`] tasks, so the lifecycle
/// alone can never fill it.
pub const MAX_ENTRIES: usize = 4096;

/// The most accounts the treasury holds. Each task opens at most one escrow
/// account and the rest are opened by the entry that needs them, so the
/// lifecycle alone stays well under this; past it an entry that would open a
/// new account is refused.
pub const MAX_ACCOUNTS: usize = 4 * crate::tasks::MAX_TASKS;

/// The most entries the journal endpoint returns in one response. The journal
/// itself may be longer; the endpoint returns the newest [`JOURNAL_LIMIT`] of
/// them (oldest of those first), so a response is bounded.
pub const JOURNAL_LIMIT: usize = 256;

/// The page size the journal endpoint serves when a request names no `limit`.
pub const JOURNAL_PAGE: usize = 50;

/// The most points one treasury series returns; a wider range with the same
/// bucket is refused, so the response stays bounded.
pub const SERIES_MAX_POINTS: usize = 2000;

/// The kinds of account, the schema's `treasury.account_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// The governance source of all credits; may go arbitrarily negative.
    Mint,
    /// Fines, filing fees, reclaimed balances.
    Treasury,
    /// One per town, funded monthly by the mint.
    TownBudget,
    /// One per task: its budget; operating funds only.
    TaskEscrow,
    /// One per agent: accumulated margin; buys autonomy, never real money.
    AgentEarnings,
    /// Real money leaving: model providers, tools, compute.
    ExternalSpend,
}

impl AccountKind {
    /// Every kind, in the schema's enum order.
    pub const ALL: [AccountKind; 6] = [
        AccountKind::Mint,
        AccountKind::Treasury,
        AccountKind::TownBudget,
        AccountKind::TaskEscrow,
        AccountKind::AgentEarnings,
        AccountKind::ExternalSpend,
    ];

    /// The kind's name, exactly as the schema's enum spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Mint => "mint",
            AccountKind::Treasury => "treasury",
            AccountKind::TownBudget => "town_budget",
            AccountKind::TaskEscrow => "task_escrow",
            AccountKind::AgentEarnings => "agent_earnings",
            AccountKind::ExternalSpend => "external_spend",
        }
    }

    /// Whether the schema's `check_entry` bounds this kind below by
    /// `-credit_limit` (`task_escrow`, `agent_earnings`, `treasury`,
    /// `town_budget`). `mint` and `external_spend` may go freely negative and
    /// positive.
    fn is_limited(self) -> bool {
        matches!(
            self,
            AccountKind::TaskEscrow
                | AccountKind::AgentEarnings
                | AccountKind::Treasury
                | AccountKind::TownBudget
        )
    }
}

/// The kinds of entry, the schema's `treasury.entry_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Owner or council funds a task budget.
    Mint,
    /// Metered real cost: escrow -> external_spend.
    Spend,
    /// Award: parent escrow locks the price in the child task's escrow.
    Subcontract,
    /// Verified work: child escrow -> contractor earnings (the margin).
    Payment,
    /// Unused escrow back to the parent escrow or mint.
    Refund,
    /// Warranty claim: contractor earnings -> escrow/treasury.
    Clawback,
    /// Sanction: earnings -> treasury (from earnings only, never escrow).
    Fine,
    /// Appeal filing fee (earnings -> treasury), refunded on success.
    Fee,
    /// Owner or council moves budget between towns (0137).
    BudgetMove,
    /// Agent-to-agent move of earnings: no task money, so it names no chain.
    Transfer,
    /// A model call's metered cost: the task's escrow (or, with no task, the
    /// agent's wallet) to `external_spend`. Names a chain only when it has a task.
    Model,
}

impl EntryKind {
    /// Every kind, in the schema's enum order.
    pub const ALL: [EntryKind; 11] = [
        EntryKind::Mint,
        EntryKind::Spend,
        EntryKind::Subcontract,
        EntryKind::Payment,
        EntryKind::Refund,
        EntryKind::Clawback,
        EntryKind::Fine,
        EntryKind::Fee,
        EntryKind::BudgetMove,
        EntryKind::Transfer,
        EntryKind::Model,
    ];

    /// The kind's name, exactly as the schema's enum spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Mint => "mint",
            EntryKind::Spend => "spend",
            EntryKind::Subcontract => "subcontract",
            EntryKind::Payment => "payment",
            EntryKind::Refund => "refund",
            EntryKind::Clawback => "clawback",
            EntryKind::Fine => "fine",
            EntryKind::Fee => "fee",
            EntryKind::BudgetMove => "budget_move",
            EntryKind::Transfer => "transfer",
            EntryKind::Model => "model",
        }
    }

    /// Whether the entry must name a chain: the schema's
    /// `task_money_names_chain` exempts `fine` and `fee`, and `transfer`, which
    /// moves earnings and no task money. A `model` entry names one only when
    /// it has a task (see [`Treasury::charge_model`]).
    fn names_a_chain(self) -> bool {
        !matches!(
            self,
            EntryKind::Fine | EntryKind::Fee | EntryKind::Transfer | EntryKind::Model
        )
    }

    /// Whether an entry of this kind is metered real cost: its credits to `external_spend`
    /// are what the plaza has paid providers. Both the agent loop's and the gateway's model
    /// charges post `model`, and a `spend` is the other metered cost; the Spent total, a
    /// chain's margin cap and a task's spent figure all count these two kinds and no others.
    pub fn is_real_cost(self) -> bool {
        matches!(self, EntryKind::Spend | EntryKind::Model)
    }
}

/// Which side of an entry a posting is on. The schema stores this as
/// `text CHECK (side IN ('debit', 'credit'))`; a posting's sign is its side
/// (`amount < 0` is a debit, `amount > 0` a credit), exactly as the schema's
/// `check_entry` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// `amount < 0`.
    Debit,
    /// `amount > 0`.
    Credit,
}

impl Side {
    /// The side of a posting of `amount` (nonzero by the time this is called).
    fn of(amount: i64) -> Side {
        if amount < 0 {
            Side::Debit
        } else {
            Side::Credit
        }
    }

    /// The side's name, as the schema's error message spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Debit => "debit",
            Side::Credit => "credit",
        }
    }
}

/// The schema's `treasury.posting_rules`, verbatim: which account kinds each
/// entry kind may debit (`amount < 0`) and credit (`amount > 0`). Anything not
/// listed is refused, so e.g. a fine can never touch escrow.
pub const POSTING_RULES: &[(EntryKind, AccountKind, Side)] = &[
    (EntryKind::Mint, AccountKind::Mint, Side::Debit),
    (EntryKind::Mint, AccountKind::TaskEscrow, Side::Credit),
    (EntryKind::Mint, AccountKind::TownBudget, Side::Credit),
    (EntryKind::Subcontract, AccountKind::TownBudget, Side::Debit),
    (EntryKind::Refund, AccountKind::TownBudget, Side::Credit),
    (EntryKind::BudgetMove, AccountKind::TownBudget, Side::Debit),
    (EntryKind::BudgetMove, AccountKind::TownBudget, Side::Credit),
    (EntryKind::Spend, AccountKind::TaskEscrow, Side::Debit),
    (EntryKind::Spend, AccountKind::ExternalSpend, Side::Credit),
    (EntryKind::Subcontract, AccountKind::TaskEscrow, Side::Debit),
    (
        EntryKind::Subcontract,
        AccountKind::TaskEscrow,
        Side::Credit,
    ),
    (EntryKind::Payment, AccountKind::TaskEscrow, Side::Debit),
    (EntryKind::Payment, AccountKind::AgentEarnings, Side::Credit),
    (EntryKind::Refund, AccountKind::TaskEscrow, Side::Debit),
    (EntryKind::Refund, AccountKind::TaskEscrow, Side::Credit),
    (EntryKind::Refund, AccountKind::Mint, Side::Credit),
    (EntryKind::Clawback, AccountKind::AgentEarnings, Side::Debit),
    (EntryKind::Clawback, AccountKind::TaskEscrow, Side::Credit),
    (EntryKind::Clawback, AccountKind::Treasury, Side::Credit),
    (EntryKind::Fine, AccountKind::AgentEarnings, Side::Debit),
    (EntryKind::Fine, AccountKind::Treasury, Side::Credit),
    (EntryKind::Fee, AccountKind::AgentEarnings, Side::Debit),
    (EntryKind::Fee, AccountKind::Treasury, Side::Credit),
    (EntryKind::Fee, AccountKind::Treasury, Side::Debit),
    (EntryKind::Fee, AccountKind::AgentEarnings, Side::Credit),
    (EntryKind::Spend, AccountKind::TownBudget, Side::Debit),
    (EntryKind::Transfer, AccountKind::AgentEarnings, Side::Debit),
    (
        EntryKind::Transfer,
        AccountKind::AgentEarnings,
        Side::Credit,
    ),
    (EntryKind::Model, AccountKind::TaskEscrow, Side::Debit),
    (EntryKind::Model, AccountKind::AgentEarnings, Side::Debit),
    (EntryKind::Model, AccountKind::ExternalSpend, Side::Credit),
];

/// Whether the schema's `posting_rules` allow `entry_kind` to post `side` to
/// `account_kind`.
fn rule_allows(entry_kind: EntryKind, account_kind: AccountKind, side: Side) -> bool {
    POSTING_RULES.contains(&(entry_kind, account_kind, side))
}

/// One posting to an entry: an account named by kind and owner, and an amount
/// (negative = debit, positive = credit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingSpec {
    /// The account's kind.
    pub kind: AccountKind,
    /// The account's owner (`owner_ref`).
    pub owner: String,
    /// The amount, in whole credits; negative debits, positive credits, never 0.
    pub amount: i64,
}

impl PostingSpec {
    /// A posting of `amount` to the account of `kind` owned by `owner`.
    pub fn new(kind: AccountKind, owner: &str, amount: i64) -> Self {
        Self {
            kind,
            owner: owner.to_string(),
            amount,
        }
    }
}

/// One entry to post, as a caller describes it before it is validated and
/// committed. The fields mirror the schema's `treasury.entries` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySpec {
    /// The entry's kind, which decides its posting rules.
    pub kind: EntryKind,
    /// The task this entry belongs to, if any (`task_id`).
    pub task_id: Option<String>,
    /// The root of the task chain (`root_task`); required for every kind but
    /// `fine` and `fee`.
    pub root_task: Option<String>,
    /// A short note (`memo`).
    pub memo: String,
    /// The entry's postings; at least one, all nonzero, to distinct accounts,
    /// allowed by [`POSTING_RULES`], summing to zero.
    pub postings: Vec<PostingSpec>,
}

impl EntrySpec {
    /// An entry of `kind` with `postings` (no task, no root, no memo).
    pub fn new(kind: EntryKind, postings: Vec<PostingSpec>) -> Self {
        Self {
            kind,
            task_id: None,
            root_task: None,
            memo: String::new(),
            postings,
        }
    }

    /// The same entry, belonging to `task_id`.
    pub fn task(mut self, task_id: &str) -> Self {
        self.task_id = Some(task_id.to_string());
        self
    }

    /// The same entry, naming `root_task` as its chain.
    pub fn root(mut self, root_task: &str) -> Self {
        self.root_task = Some(root_task.to_string());
        self
    }

    /// The same entry, with `memo`.
    pub fn memo(mut self, memo: &str) -> Self {
        self.memo = memo.to_string();
        self
    }
}

/// Why an entry could not be posted. Every one is a rule of the schema's
/// `treasury.check_entry` (or an insert-time constraint) refused; the message
/// uses the schema's own wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostError {
    /// The entry had no postings at all.
    Empty,
    /// A posting's amount was zero (the schema's `amount <> 0`).
    ZeroPosting {
        /// The kind of the account the zero posting named.
        account: AccountKind,
        /// Its owner.
        owner_ref: String,
    },
    /// Two postings in one entry named the same account (the schema's
    /// `postings` primary key).
    DuplicatePosting {
        /// The kind of the repeated account.
        account: AccountKind,
        /// Its owner.
        owner_ref: String,
    },
    /// The entry kind must name a root task and none was given (the schema's
    /// `task_money_names_chain`).
    RootTaskRequired {
        /// The entry kind that needs a chain.
        entry_kind: EntryKind,
    },
    /// A posting touched a `(account kind, side)` pair its entry kind may not
    /// (the schema's `posting_rules`).
    Rule {
        /// The entry kind.
        entry_kind: EntryKind,
        /// The account kind the posting named.
        account_kind: AccountKind,
        /// The side the posting was on.
        side: Side,
    },
    /// The entry's postings did not balance to zero (the schema's
    /// `does not balance`).
    Unbalanced {
        /// The sum of the postings.
        sum: i128,
    },
    /// An account would fall below `-credit_limit` (the schema's
    /// `below its limit`).
    Overdrawn {
        /// The account's kind.
        account: AccountKind,
        /// Its owner.
        owner_ref: String,
        /// The balance the entry would leave it at.
        balance: i128,
        /// The account's credit limit.
        credit_limit: i64,
    },
    /// A payment would break the chain's margin cap over real cost (0111).
    MarginCap {
        /// The chain the payment was on.
        root_task: String,
        /// What the chain had paid agents, including this payment.
        paid_to_agents: i128,
        /// The chain's real cost (metered spend).
        real_cost: i128,
    },
    /// The journal is full ([`MAX_ENTRIES`]).
    JournalFull {
        /// The cap.
        max: usize,
    },
    /// The entry would open an account past the accounts cap ([`MAX_ACCOUNTS`]).
    AccountsFull {
        /// The cap.
        max: usize,
    },
    /// An account of that kind and owner already exists.
    DuplicateAccount {
        /// The account's kind.
        account: AccountKind,
        /// Its owner.
        owner_ref: String,
    },
    /// A transfer named the same agent as sender and receiver.
    SelfTransfer {
        /// The agent.
        cid: u64,
    },
    /// An amount that must be positive was zero or negative.
    NonPositive {
        /// The amount.
        amount: i64,
    },
    /// A sender's transfers this UTC day would pass [`TRANSFER_DAILY_CAP`].
    TransferCap {
        /// The sender.
        cid: u64,
        /// What it has already sent today.
        sent_today: i64,
        /// The transfer that was refused.
        amount: i64,
        /// The daily cap.
        cap: i64,
    },
}

impl PostError {
    /// The HTTP status this error is reported as when it reaches the task
    /// lifecycle's request path: a full journal or a duplicated/over-cap
    /// account is a conflict with the treasury's state; the rest are rules a
    /// service-constructed entry cannot break, so reaching one over HTTP would
    /// be a daemon bug.
    pub fn status(&self) -> u16 {
        match self {
            PostError::JournalFull { .. } | PostError::AccountsFull { .. } => 409,
            PostError::SelfTransfer { .. } | PostError::NonPositive { .. } => 400,
            PostError::TransferCap { .. } => 409,
            _ => 500,
        }
    }
}

impl std::fmt::Display for PostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostError::SelfTransfer { cid } => {
                write!(formatter, "agent cid:{cid} may not transfer to itself")
            }
            PostError::NonPositive { amount } => {
                write!(formatter, "an amount must be positive, got {amount}")
            }
            PostError::TransferCap {
                cid,
                sent_today,
                amount,
                cap,
            } => write!(
                formatter,
                "agent cid:{cid} has sent {sent_today} today; {amount} more passes the {cap} daily cap"
            ),
            PostError::Empty => write!(formatter, "an entry must have at least one posting"),
            PostError::ZeroPosting { account, owner_ref } => write!(
                formatter,
                "a posting of zero is not allowed: {} {}",
                account.as_str(),
                owner_ref
            ),
            PostError::DuplicatePosting { account, owner_ref } => write!(
                formatter,
                "an entry may post to {} {} once",
                account.as_str(),
                owner_ref
            ),
            PostError::RootTaskRequired { entry_kind } => write!(
                formatter,
                "a {} entry must name its root task",
                entry_kind.as_str()
            ),
            PostError::Rule {
                entry_kind,
                account_kind,
                side,
            } => write!(
                formatter,
                "{} entry may not {} a {} account",
                entry_kind.as_str(),
                side.as_str(),
                account_kind.as_str()
            ),
            PostError::Unbalanced { sum } => {
                write!(formatter, "the entry does not balance (sum = {sum})")
            }
            PostError::Overdrawn {
                account,
                owner_ref,
                balance,
                credit_limit,
            } => write!(
                formatter,
                "account {} {} would be at {balance}, below its limit of -{credit_limit}",
                account.as_str(),
                owner_ref
            ),
            PostError::MarginCap {
                root_task,
                paid_to_agents,
                real_cost,
            } => write!(
                formatter,
                "the payment breaks the chain margin cap: {paid_to_agents} paid against \
                 {real_cost} real cost in chain {root_task} (cap {MARGIN_CAP_PCT}%)"
            ),
            PostError::JournalFull { max } => {
                write!(formatter, "the treasury journal is full ({max} entries)")
            }
            PostError::AccountsFull { max } => {
                write!(formatter, "the treasury holds too many accounts ({max})")
            }
            PostError::DuplicateAccount { account, owner_ref } => write!(
                formatter,
                "an account of {} {} already exists",
                account.as_str(),
                owner_ref
            ),
        }
    }
}

impl std::error::Error for PostError {}

/// One agent's wallet as `plaza_money` reports it (see [`Treasury::wallet_money`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletMoney {
    pub balance: i128,
    pub sent_today: i64,
    pub earned: i128,
}

/// One task's money as `plaza_money` reports it (see [`Treasury::task_money`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMoney {
    pub minted: i128,
    pub escrow: i128,
    pub spent: i128,
    /// (cid, credits) per agent paid from the escrow.
    pub paid: Vec<(u64, i128)>,
    pub refunded: i128,
}

/// One account: the schema's `treasury.accounts` row, without a balance (the
/// balance is derived from the postings).
#[derive(Debug, Serialize, Deserialize)]
struct Account {
    id: u64,
    kind: AccountKind,
    owner_ref: String,
    credit_limit: i64,
    created_at: u64,
}

/// One posting as it is stored: which account, and the amount.
#[derive(Debug, Serialize, Deserialize)]
struct Posting {
    account_id: u64,
    amount: i64,
}

/// One entry as it is stored: the schema's `treasury.entries` row together with
/// its postings.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    id: u64,
    kind: EntryKind,
    task_id: Option<String>,
    root_task: Option<String>,
    memo: String,
    #[serde(skip_deserializing, default = "posted_by_default")]
    posted_by: &'static str,
    created_at: u64,
    postings: Vec<Posting>,
}

/// The accounts and the journal, behind one lock.
#[derive(Debug, Serialize, Deserialize)]
struct Inner {
    accounts: Vec<Account>,
    entries: Vec<Entry>,
    next_account: u64,
    next_entry: u64,
    /// What each agent has transferred on each UTC day, keyed by (cid, day).
    #[serde(
        default,
        serialize_with = "ser_transferred",
        deserialize_with = "de_transferred"
    )]
    transferred: BTreeMap<(u64, u64), i64>,
}

impl Inner {
    /// The account of `kind` owned by `owner`, if it exists.
    fn find_account(&self, kind: AccountKind, owner: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|account| account.kind == kind && account.owner_ref == owner)
    }

    /// The kind of the account with that id, if it exists.
    fn account_kind(&self, id: u64) -> Option<AccountKind> {
        self.accounts
            .iter()
            .find(|account| account.id == id)
            .map(|account| account.kind)
    }

    /// An account's balance: the sum of its postings. Derived every time, never
    /// stored (the schema's `treasury.balances`).
    fn balance(&self, account_id: u64) -> i128 {
        self.entries
            .iter()
            .flat_map(|entry| &entry.postings)
            .filter(|posting| posting.account_id == account_id)
            .map(|posting| i128::from(posting.amount))
            .sum()
    }

    /// The schema's `treasury.chain_margin` for `root`, with `spec`'s postings
    /// counted as if they were already committed (the schema's `check_entry`
    /// runs at commit, when the new postings are in the table): the chain's
    /// real cost (positive `spend` credits to `external_spend`) and what it has
    /// paid agents (`payment` and `clawback` postings on `agent_earnings`).
    fn chain_margin(&self, root: &str, spec: &EntrySpec) -> (i128, i128) {
        let mut real_cost = 0i128;
        let mut paid_to_agents = 0i128;
        for entry in &self.entries {
            if entry.root_task.as_deref() != Some(root) {
                continue;
            }
            for posting in &entry.postings {
                let Some(kind) = self.account_kind(posting.account_id) else {
                    continue;
                };
                if entry.kind.is_real_cost()
                    && kind == AccountKind::ExternalSpend
                    && posting.amount > 0
                {
                    real_cost += i128::from(posting.amount);
                }
                if matches!(entry.kind, EntryKind::Payment | EntryKind::Clawback)
                    && kind == AccountKind::AgentEarnings
                {
                    paid_to_agents += i128::from(posting.amount);
                }
            }
        }
        for posting in &spec.postings {
            if spec.kind.is_real_cost()
                && posting.kind == AccountKind::ExternalSpend
                && posting.amount > 0
            {
                real_cost += i128::from(posting.amount);
            }
            if matches!(spec.kind, EntryKind::Payment | EntryKind::Clawback)
                && posting.kind == AccountKind::AgentEarnings
            {
                paid_to_agents += i128::from(posting.amount);
            }
        }
        (real_cost, paid_to_agents)
    }
}

/// The `posted_by` an entry read back from a snapshot carries: the service's name,
/// which every entry is posted under.
fn posted_by_default() -> &'static str {
    POSTED_BY
}

/// The transfer allowances as `(cid, day, sent)` rows: a JSON object key must be a
/// string, and these are keyed by a pair.
fn ser_transferred<S: serde::Serializer>(
    transferred: &BTreeMap<(u64, u64), i64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let rows: Vec<(u64, u64, i64)> = transferred
        .iter()
        .map(|(&(cid, day), &sent)| (cid, day, sent))
        .collect();
    serde::Serialize::serialize(&rows, serializer)
}

fn de_transferred<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<(u64, u64), i64>, D::Error> {
    let rows = <Vec<(u64, u64, i64)> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(rows
        .into_iter()
        .map(|(cid, day, sent)| ((cid, day), sent))
        .collect())
}

/// The invariants a snapshot must hold before it may replace a treasury: the ones
/// [`Treasury::post`] enforces, replayed in journal order, so a snapshot the ledger
/// could not have built is refused.
fn check_snapshot(inner: &Inner) -> Result<(), String> {
    if inner.entries.len() > MAX_ENTRIES {
        return Err(format!(
            "the journal holds {} entries, past the cap of {MAX_ENTRIES}",
            inner.entries.len()
        ));
    }
    if inner.accounts.len() > MAX_ACCOUNTS {
        return Err(format!(
            "the treasury holds {} accounts, past the cap of {MAX_ACCOUNTS}",
            inner.accounts.len()
        ));
    }
    for (index, account) in inner.accounts.iter().enumerate() {
        if account.id > inner.next_account {
            return Err(format!(
                "account {} is past the account counter ({})",
                account.id, inner.next_account
            ));
        }
        let repeated = inner.accounts[..index].iter().any(|other| {
            other.id == account.id
                || (other.kind == account.kind && other.owner_ref == account.owner_ref)
        });
        if repeated {
            return Err(format!(
                "account {} {} is listed twice",
                account.kind.as_str(),
                account.owner_ref
            ));
        }
    }
    if inner.next_entry != u64::try_from(inner.entries.len()).unwrap_or(u64::MAX) {
        return Err(format!(
            "the next entry id ({}) does not follow the journal's length ({})",
            inner.next_entry,
            inner.entries.len()
        ));
    }
    // The balances and the chain margins, replayed in journal order: each entry is
    // checked against the state the entries before it leave behind.
    let mut balances: BTreeMap<u64, i128> = BTreeMap::new();
    let mut margins: BTreeMap<String, (i128, i128)> = BTreeMap::new();
    for (index, entry) in inner.entries.iter().enumerate() {
        let expected = u64::try_from(index).unwrap_or(u64::MAX) + 1;
        if entry.id != expected {
            return Err(format!(
                "entry {} is at position {index}, expected id {expected}",
                entry.id
            ));
        }
        if entry.postings.is_empty() {
            return Err(format!("entry {} has no postings", entry.id));
        }
        if entry.kind.names_a_chain() && entry.root_task.is_none() {
            return Err(format!("entry {} must name its root task", entry.id));
        }
        let mut sum = 0i128;
        for (position, posting) in entry.postings.iter().enumerate() {
            if posting.amount == 0 {
                return Err(format!("entry {} has a zero posting", entry.id));
            }
            if entry.postings[..position]
                .iter()
                .any(|earlier| earlier.account_id == posting.account_id)
            {
                return Err(format!(
                    "entry {} posts to account {} twice",
                    entry.id, posting.account_id
                ));
            }
            let Some(account) = inner
                .accounts
                .iter()
                .find(|account| account.id == posting.account_id)
            else {
                return Err(format!(
                    "entry {} posts to unknown account {}",
                    entry.id, posting.account_id
                ));
            };
            let side = Side::of(posting.amount);
            if !rule_allows(entry.kind, account.kind, side) {
                return Err(format!(
                    "entry {} breaks the posting rules: {} entry may not {} a {} account",
                    entry.id,
                    entry.kind.as_str(),
                    side.as_str(),
                    account.kind.as_str()
                ));
            }
            sum += i128::from(posting.amount);
            let balance = balances.entry(posting.account_id).or_insert(0);
            *balance += i128::from(posting.amount);
            if account.kind.is_limited() && *balance < -i128::from(account.credit_limit) {
                return Err(format!(
                    "entry {} overdraws account {} {}: {} is below its limit of -{}",
                    entry.id,
                    account.kind.as_str(),
                    account.owner_ref,
                    *balance,
                    account.credit_limit
                ));
            }
            if let Some(root) = entry.root_task.as_deref() {
                let (cost, paid) = margins.entry(root.to_string()).or_insert((0, 0));
                if entry.kind.is_real_cost()
                    && account.kind == AccountKind::ExternalSpend
                    && posting.amount > 0
                {
                    *cost += i128::from(posting.amount);
                }
                if matches!(entry.kind, EntryKind::Payment | EntryKind::Clawback)
                    && account.kind == AccountKind::AgentEarnings
                {
                    *paid += i128::from(posting.amount);
                }
            }
        }
        if sum != 0 {
            return Err(format!("entry {} does not balance (sum = {sum})", entry.id));
        }
        if entry.kind == EntryKind::Payment
            && let Some(root) = entry.root_task.as_deref()
        {
            let (cost, paid) = margins.get(root).copied().unwrap_or((0, 0));
            if paid * 100 > cost * i128::from(MARGIN_CAP_PCT) {
                return Err(format!(
                    "entry {} breaks the chain margin cap: {paid} paid against {cost} real cost in chain {root}",
                    entry.id
                ));
            }
        }
    }
    Ok(())
}

/// The `owner_ref` of agent `cid`'s `agent_earnings` account (its wallet).
fn agent_owner(cid: u64) -> String {
    format!("cid:{cid}")
}

/// The treasury: the accounts, the append-only double-entry journal, and the
/// event bus every posted entry is published to.
#[derive(Debug)]
pub struct Treasury {
    events: Arc<EventBus>,
    /// Bumped on every change to the accounts or the journal.
    version: AtomicU64,
    inner: Mutex<Inner>,
}

impl Treasury {
    /// A treasury with only the single `mint` account and an empty journal, which
    /// publishes every posted entry to `events`. [`Treasury::restore`] starts here.
    pub fn empty(events: Arc<EventBus>) -> Self {
        let now = now_ms();
        Self {
            events,
            version: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                accounts: vec![Account {
                    id: 1,
                    kind: AccountKind::Mint,
                    owner_ref: MINT_OWNER.to_string(),
                    credit_limit: 0,
                    created_at: now,
                }],
                entries: Vec::new(),
                next_account: 1,
                next_entry: 0,
                transferred: BTreeMap::new(),
            }),
        }
    }

    /// A treasury with the plaza's budget minted into it: the one entry a fresh
    /// treasury holds.
    pub fn new(events: Arc<EventBus>) -> Self {
        let treasury = Self::empty(events);
        treasury
            .mint_plaza_budget(PLAZA_BUDGET)
            .expect("a fresh journal takes the plaza's first mint");
        treasury
    }

    /// Posts one entry, or says which rule refused it. Nothing is mutated until
    /// every check has passed, so a refused entry leaves the journal exactly as
    /// it was.
    ///
    /// The checks, in the schema's `treasury.check_entry` order (see the module
    /// note for the mapping):
    ///
    /// * the journal is not full ([`MAX_ENTRIES`]);
    /// * the entry has postings, none of them zero, one per account;
    /// * the entry names a chain when its kind must ([`EntryKind::names_a_chain`]);
    /// * every posting is allowed by [`POSTING_RULES`] — **this is the rule a
    ///   call outside the schema's table is refused by**, with the schema's own
    ///   wording;
    /// * the postings balance to zero;
    /// * **no touched `task_escrow` (or `agent_earnings`/`treasury`/`town_budget`)
    ///   account falls below its credit limit** — the mechanism that keeps an
    ///   escrow from ever going negative: the new balance is computed as
    ///   `existing + this entry's posting` and compared to `-credit_limit`
    ///   (0 by default) before anything is written, and a refusal changes
    ///   nothing;
    /// * a `payment` respects the chain's margin cap;
    /// * the accounts the entry would open fit under [`MAX_ACCOUNTS`].
    ///
    /// Accounts the entry names are opened on demand (with the schema's default
    /// credit limit of 0), the entry is appended with the next id, and an
    /// `event: treasury` frame is published — all while the journal's lock is
    /// held, so ids and frames are in the same order.
    pub fn post(&self, spec: &EntrySpec) -> Result<u64, PostError> {
        let mut inner = self.lock();
        self.append(&mut inner, spec)
    }

    /// Opens an account of `kind` owned by `owner_ref` with `credit_limit`, as
    /// the schema's `INSERT INTO treasury.accounts` does. v0 opens accounts
    /// lazily with a limit of 0 on the entry that needs them; this is the way
    /// to give an account a credit line (the reputation slice that would is
    /// later), and the way a test sets one up.
    pub fn open_account(
        &self,
        kind: AccountKind,
        owner_ref: &str,
        credit_limit: i64,
    ) -> Result<u64, PostError> {
        let mut inner = self.lock();
        if inner.find_account(kind, owner_ref).is_some() {
            return Err(PostError::DuplicateAccount {
                account: kind,
                owner_ref: owner_ref.to_string(),
            });
        }
        if inner.accounts.len() >= MAX_ACCOUNTS {
            return Err(PostError::AccountsFull { max: MAX_ACCOUNTS });
        }
        inner.next_account += 1;
        let id = inner.next_account;
        inner.accounts.push(Account {
            id,
            kind,
            owner_ref: owner_ref.to_string(),
            credit_limit,
            created_at: now_ms(),
        });
        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(id)
    }

    /// Mints `amount` credits into the plaza's budget: a `mint` entry, the mint
    /// debited and the plaza's `town_budget` credited. Every task is funded from
    /// this pool and every refund returns to it.
    pub fn mint_plaza_budget(&self, amount: i64) -> Result<u64, PostError> {
        let spec = EntrySpec::new(
            EntryKind::Mint,
            vec![
                PostingSpec::new(AccountKind::Mint, MINT_OWNER, -amount),
                PostingSpec::new(AccountKind::TownBudget, PLAZA_OWNER, amount),
            ],
        )
        .root(PLAZA_OWNER)
        .memo("plaza budget minted");
        self.post(&spec)
    }

    /// The plaza's budget balance, derived from its postings.
    pub fn plaza_budget_balance(&self) -> i128 {
        let inner = self.lock();
        inner
            .find_account(AccountKind::TownBudget, PLAZA_OWNER)
            .map_or(0, |account| inner.balance(account.id))
    }

    /// Funds `task_id`'s escrow from the plaza's budget: a `subcontract` entry
    /// (the plaza's `town_budget` debited, the task's escrow credited). Refused
    /// when the budget would go negative.
    pub fn fund_task(&self, task_id: &str, amount: i64) -> Result<u64, PostError> {
        if amount <= 0 {
            return Err(PostError::NonPositive { amount });
        }
        let spec = EntrySpec::new(
            EntryKind::Subcontract,
            vec![
                PostingSpec::new(AccountKind::TownBudget, PLAZA_OWNER, -amount),
                PostingSpec::new(AccountKind::TaskEscrow, task_id, amount),
            ],
        )
        .task(task_id)
        .root(task_id)
        .memo("task budget funded from the plaza");
        self.post(&spec)
    }

    /// Charges `credits` of real spend: a `spend` entry, from the task's escrow
    /// (or from the plaza's budget when `task_id` is `None`) to
    /// `external_spend`. Refused when the escrow or budget would go negative.
    pub fn charge_spend(
        &self,
        task_id: Option<&str>,
        credits: i64,
        memo: &str,
    ) -> Result<u64, PostError> {
        if credits <= 0 {
            return Err(PostError::NonPositive { amount: credits });
        }
        let (kind, owner) = match task_id {
            Some(task) => (AccountKind::TaskEscrow, task),
            None => (AccountKind::TownBudget, PLAZA_OWNER),
        };
        let mut spec = EntrySpec::new(
            EntryKind::Spend,
            vec![
                PostingSpec::new(kind, owner, -credits),
                PostingSpec::new(AccountKind::ExternalSpend, "providers", credits),
            ],
        )
        .root(owner)
        .memo(memo);
        if let Some(task) = task_id {
            spec = spec.task(task);
        }
        self.post(&spec)
    }

    /// Charges `credits` of model cost: a `model` entry to `external_spend`
    /// (the provider is credited). With `task`, the task's escrow pays and the
    /// entry names that chain; without one, agent `cid`'s wallet pays. Refused
    /// when the payer would go below its limit.
    pub fn charge_model(
        &self,
        cid: u64,
        task: Option<&str>,
        credits: i64,
        memo: &str,
    ) -> Result<u64, PostError> {
        if credits <= 0 {
            return Err(PostError::NonPositive { amount: credits });
        }
        let spec = match task {
            Some(task) => EntrySpec::new(
                EntryKind::Model,
                vec![
                    PostingSpec::new(AccountKind::TaskEscrow, task, -credits),
                    PostingSpec::new(AccountKind::ExternalSpend, "providers", credits),
                ],
            )
            .task(task)
            .root(task),
            None => EntrySpec::new(
                EntryKind::Model,
                vec![
                    PostingSpec::new(AccountKind::AgentEarnings, &agent_owner(cid), -credits),
                    PostingSpec::new(AccountKind::ExternalSpend, "providers", credits),
                ],
            ),
        }
        .memo(memo);
        self.post(&spec)
    }

    /// Pays `amount` of a task's escrow to agent `cid`'s wallet: a `payment`
    /// entry. The chain's margin cap applies: a payment may not exceed 20 % of
    /// the task's real cost so far.
    pub fn pay_agent(&self, task_id: &str, cid: u64, amount: i64) -> Result<u64, PostError> {
        if amount <= 0 {
            return Err(PostError::NonPositive { amount });
        }
        let owner = agent_owner(cid);
        let spec = EntrySpec::new(
            EntryKind::Payment,
            vec![
                PostingSpec::new(AccountKind::TaskEscrow, task_id, -amount),
                PostingSpec::new(AccountKind::AgentEarnings, &owner, amount),
            ],
        )
        .task(task_id)
        .root(task_id)
        .memo("verified work");
        self.post(&spec)
    }

    /// Refunds what `task_id`'s escrow still holds back to the plaza's budget: a
    /// `refund` entry (task_escrow debited, the plaza's `town_budget` credited).
    /// Returns the entry's id, or `None` when there is nothing to refund (no
    /// escrow, or a balance of zero), in which case no entry is posted.
    ///
    /// The balance is read and the entry posted under one lock, so a concurrent
    /// post cannot make the refund overdraw.
    pub fn refund_task_to_budget(&self, task_id: &str) -> Result<Option<u64>, PostError> {
        let mut inner = self.lock();
        let Some(account) = inner.find_account(AccountKind::TaskEscrow, task_id) else {
            return Ok(None);
        };
        let balance = inner.balance(account.id);
        if balance <= 0 {
            return Ok(None);
        }
        let Ok(amount) = i64::try_from(balance) else {
            // A balance that does not fit the schema's bigint could only come
            // from a journal built outside the ledger's own paths.
            return Err(PostError::Overdrawn {
                account: AccountKind::TaskEscrow,
                owner_ref: task_id.to_string(),
                balance,
                credit_limit: account.credit_limit,
            });
        };
        let spec = EntrySpec::new(
            EntryKind::Refund,
            vec![
                PostingSpec::new(AccountKind::TaskEscrow, task_id, -amount),
                PostingSpec::new(AccountKind::TownBudget, PLAZA_OWNER, amount),
            ],
        )
        .task(task_id)
        .root(task_id)
        .memo("task budget returned to the plaza");
        self.append(&mut inner, &spec).map(Some)
    }

    /// Moves `amount` of earnings from agent `from_cid` to agent `to_cid`: a
    /// `transfer` entry. Refused for a self-transfer, a non-positive amount, an
    /// overdraw, or when the sender's transfers this UTC day would pass
    /// [`TRANSFER_DAILY_CAP`]. The check, the post and the day's sum all happen
    /// under one lock.
    pub fn transfer(
        &self,
        from_cid: u64,
        to_cid: u64,
        amount: i64,
        memo: &str,
    ) -> Result<u64, PostError> {
        if from_cid == to_cid {
            return Err(PostError::SelfTransfer { cid: from_cid });
        }
        if amount <= 0 {
            return Err(PostError::NonPositive { amount });
        }
        let mut inner = self.lock();
        let day = now_ms() / DAY_MS;
        let sent_today = inner
            .transferred
            .get(&(from_cid, day))
            .copied()
            .unwrap_or(0);
        if sent_today.saturating_add(amount) > TRANSFER_DAILY_CAP {
            return Err(PostError::TransferCap {
                cid: from_cid,
                sent_today,
                amount,
                cap: TRANSFER_DAILY_CAP,
            });
        }
        let from = agent_owner(from_cid);
        let to = agent_owner(to_cid);
        let spec = EntrySpec::new(
            EntryKind::Transfer,
            vec![
                PostingSpec::new(AccountKind::AgentEarnings, &from, -amount),
                PostingSpec::new(AccountKind::AgentEarnings, &to, amount),
            ],
        )
        .memo(memo);
        let id = self.append(&mut inner, &spec)?;
        // Only the current day's sums are kept, so the map stays small.
        inner.transferred.retain(|&(_, sent_on), _| sent_on == day);
        *inner.transferred.entry((from_cid, day)).or_insert(0) += amount;
        Ok(id)
    }

    /// Fines agent `cid`: earnings -> the plaza's `treasury`, a `fine` entry.
    /// Refused when it would take the agent's wallet below zero.
    pub fn fine(&self, cid: u64, amount: i64, memo: &str) -> Result<u64, PostError> {
        if amount <= 0 {
            return Err(PostError::NonPositive { amount });
        }
        let owner = agent_owner(cid);
        let spec = EntrySpec::new(
            EntryKind::Fine,
            vec![
                PostingSpec::new(AccountKind::AgentEarnings, &owner, -amount),
                PostingSpec::new(AccountKind::Treasury, PLAZA_OWNER, amount),
            ],
        )
        .memo(memo);
        self.post(&spec)
    }

    /// Agent `cid`'s wallet (its earnings balance), or 0 when it has none yet.
    pub fn wallet(&self, cid: u64) -> i128 {
        let inner = self.lock();
        inner
            .find_account(AccountKind::AgentEarnings, &agent_owner(cid))
            .map_or(0, |account| inner.balance(account.id))
    }

    /// Every agent wallet as (cid, balance), in account-creation order.
    pub fn wallets(&self) -> Vec<(u64, i128)> {
        let inner = self.lock();
        inner
            .accounts
            .iter()
            .filter(|account| account.kind == AccountKind::AgentEarnings)
            .filter_map(|account| {
                let cid = account
                    .owner_ref
                    .strip_prefix("cid:")?
                    .parse::<u64>()
                    .ok()?;
                Some((cid, inner.balance(account.id)))
            })
            .collect()
    }

    /// `task_id`'s escrow balance, derived from its postings.
    pub fn escrow_balance(&self, task_id: &str) -> i128 {
        let inner = self.lock();
        inner
            .find_account(AccountKind::TaskEscrow, task_id)
            .map_or(0, |account| inner.balance(account.id))
    }

    /// Agent `cid`'s money as `plaza_money` reads it: the wallet balance, what
    /// it has sent this UTC day (against [`TRANSFER_DAILY_CAP`]), and what task
    /// payments have earned it in total.
    pub fn wallet_money(&self, cid: u64) -> WalletMoney {
        let inner = self.lock();
        let day = now_ms() / DAY_MS;
        let wallet = inner.find_account(AccountKind::AgentEarnings, &agent_owner(cid));
        let wallet_id = wallet.map(|account| account.id);
        let balance = wallet.map_or(0, |account| inner.balance(account.id));
        let earned = inner
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Payment)
            .flat_map(|entry| &entry.postings)
            .filter(|posting| Some(posting.account_id) == wallet_id && posting.amount > 0)
            .map(|posting| i128::from(posting.amount))
            .sum();
        WalletMoney {
            balance,
            sent_today: inner.transferred.get(&(cid, day)).copied().unwrap_or(0),
            earned,
        }
    }

    /// `task_id`'s money, read from its journal entries: the budget minted into
    /// its escrow, the escrow balance now, the real cost charged to it, what it
    /// paid each agent, and what went back to the plaza's budget.
    pub fn task_money(&self, task_id: &str) -> TaskMoney {
        let inner = self.lock();
        let mut money = TaskMoney {
            minted: 0,
            escrow: inner
                .find_account(AccountKind::TaskEscrow, task_id)
                .map_or(0, |account| inner.balance(account.id)),
            spent: 0,
            paid: Vec::new(),
            refunded: 0,
        };
        for entry in inner
            .entries
            .iter()
            .filter(|entry| entry.task_id.as_deref() == Some(task_id))
        {
            for posting in &entry.postings {
                let Some(account) = inner
                    .accounts
                    .iter()
                    .find(|account| account.id == posting.account_id)
                else {
                    continue;
                };
                let amount = i128::from(posting.amount);
                match (entry.kind, account.kind, posting.amount > 0) {
                    // A task's budget arrives in its escrow by a fund entry (a subcontract from the plaza's budget).
                    (EntryKind::Mint | EntryKind::Subcontract, AccountKind::TaskEscrow, true) => {
                        money.minted += amount
                    }
                    // A spend debits the escrow; a negative posting adds to the spend.
                    (kind, AccountKind::TaskEscrow, false) if kind.is_real_cost() => {
                        money.spent -= amount
                    }
                    (EntryKind::Refund, AccountKind::TownBudget, true) => money.refunded += amount,
                    (EntryKind::Payment, AccountKind::AgentEarnings, true) => {
                        let cid = account
                            .owner_ref
                            .strip_prefix("cid:")
                            .and_then(|cid| cid.parse::<u64>().ok())
                            .unwrap_or(0);
                        match money.paid.iter_mut().find(|(paid_cid, _)| *paid_cid == cid) {
                            Some((_, total)) => *total += amount,
                            None => money.paid.push((cid, amount)),
                        }
                    }
                    _ => {}
                }
            }
        }
        money
    }

    /// Every account, in creation order, with its derived balance.
    pub fn accounts(&self) -> Vec<AccountView> {
        let inner = self.lock();
        account_views(&inner)
    }

    /// The credits the mint has issued into the ledger over the journal's life:
    /// the sum of every `mint` entry's credited total, derived from the
    /// journal's postings and never stored.
    ///
    /// This is the minted budget total the council's `budget_move` gate (0067,
    /// over 10%) is measured against — the honest stand-in for a town's monthly
    /// budget in a port with one budget pool. It is **read afresh** by every
    /// caller: nothing caches it, so it follows the journal as tasks are
    /// created. A refund credits the mint back but does not change what the
    /// mint *issued*, so refunds do not move this figure (which is why the
    /// journal's `credits` total, refunds and all, is not the right source).
    pub fn minted_total(&self) -> i128 {
        let inner = self.lock();
        inner
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Mint)
            .flat_map(|entry| &entry.postings)
            .filter(|posting| posting.amount > 0)
            .map(|posting| i128::from(posting.amount))
            .sum()
    }

    /// The treasury as the API reports it: the accounts with their balances,
    /// a totals line, and v0's honest note on real spend.
    pub fn view(&self) -> TreasuryView {
        let inner = self.lock();
        let accounts = account_views(&inner);
        let mut credits = 0i128;
        let mut debits = 0i128;
        let mut spent = 0i128;
        for entry in &inner.entries {
            for posting in &entry.postings {
                if posting.amount > 0 {
                    credits += i128::from(posting.amount);
                } else {
                    debits += i128::from(-posting.amount);
                }
                if entry.kind.is_real_cost()
                    && posting.amount > 0
                    && inner.account_kind(posting.account_id) == Some(AccountKind::ExternalSpend)
                {
                    spent += i128::from(posting.amount);
                }
            }
        }
        TreasuryView {
            accounts,
            totals: TotalsView {
                accounts: inner.accounts.len(),
                entries: inner.entries.len(),
                credits,
                debits,
                balance: credits - debits,
            },
            spent,
        }
    }

    /// The journal as the API reports it: up to `limit` entries, the newest
    /// last (chronological order, like the task list), with the journal's total
    /// and whether the response was cut short.
    pub fn journal(&self, limit: usize) -> JournalView {
        let inner = self.lock();
        let total = inner.entries.len();
        let start = total.saturating_sub(limit);
        let entries = inner.entries[start..]
            .iter()
            .map(|entry| entry_view(&inner, entry))
            .collect();
        JournalView {
            entries,
            total,
            limit,
            truncated: total > limit,
        }
    }

    /// One page of the journal, newest first, filtered by `query`.
    ///
    /// `total` counts every entry the filters match, whatever the cursor, so a
    /// caller can say how many there are. `next_before` is the id of the page's
    /// oldest entry when older matches remain: pass it as `before` for the next
    /// page. Pages are disjoint and consecutive, so walking `next_before` visits
    /// every match exactly once.
    pub fn journal_page(&self, query: &JournalQuery) -> JournalPage {
        let inner = self.lock();
        let limit = query.limit.clamp(1, JOURNAL_LIMIT);
        // `Some(None)`: the wallet names no account, so no entry can match it.
        let wallet: Option<Option<u64>> = query.wallet.as_deref().map(|raw| {
            raw.parse::<u64>().ok().and_then(|cid| {
                inner
                    .find_account(AccountKind::AgentEarnings, &agent_owner(cid))
                    .map(|account| account.id)
            })
        });
        let matches = |entry: &Entry| {
            query
                .kind
                .as_deref()
                .is_none_or(|kind| entry.kind.as_str() == kind)
                && wallet.is_none_or(|account| {
                    account.is_some_and(|id| {
                        entry
                            .postings
                            .iter()
                            .any(|posting| posting.account_id == id)
                    })
                })
                && query
                    .task
                    .as_deref()
                    .is_none_or(|task| entry.task_id.as_deref() == Some(task))
        };
        let mut total = 0;
        let mut page: Vec<&Entry> = Vec::new();
        let mut more = false;
        for entry in inner.entries.iter().rev().filter(|entry| matches(entry)) {
            total += 1;
            if query.before.is_some_and(|before| entry.id >= before) {
                continue;
            }
            if page.len() < limit {
                page.push(entry);
            } else {
                more = true;
            }
        }
        let next_before = if more {
            page.last().map(|entry| entry.id)
        } else {
            None
        };
        JournalPage {
            entries: page
                .into_iter()
                .map(|entry| entry_view(&inner, entry))
                .collect(),
            total,
            next_before,
        }
    }

    /// The treasury over time: the flows posted in each bucket of `[from_ms,
    /// to_ms)`, and the balances at the bucket's end. See [`TreasurySeries`].
    ///
    /// Buckets are `bucket_ms` wide and aligned to multiples of it (a day bucket
    /// starts at UTC midnight); the first and last are clipped to the range.
    /// Every bucket is present, empty ones included. An entry belongs to the
    /// bucket its `created_at` falls in. Balances are replayed from the postings
    /// (nothing is stored): the opening balance is every entry before `from_ms`,
    /// so the figures are exact whatever the range. Refused for an empty range, a
    /// zero bucket, or more than [`SERIES_MAX_POINTS`] buckets.
    pub fn series(
        &self,
        from_ms: u64,
        to_ms: u64,
        bucket_ms: u64,
    ) -> Result<TreasurySeries, String> {
        if bucket_ms == 0 {
            return Err("the bucket must be a positive number of milliseconds".to_string());
        }
        if from_ms >= to_ms {
            return Err(format!("from ({from_ms}) must be before to ({to_ms})"));
        }
        let start = from_ms - from_ms % bucket_ms;
        let count = usize::try_from((to_ms - start).div_ceil(bucket_ms)).unwrap_or(usize::MAX);
        if count > SERIES_MAX_POINTS {
            return Err(format!(
                "the range holds {count} buckets, past the cap of {SERIES_MAX_POINTS}; use a wider bucket"
            ));
        }
        let inner = self.lock();
        let accounts: BTreeMap<u64, (AccountKind, &str)> = inner
            .accounts
            .iter()
            .map(|account| (account.id, (account.kind, account.owner_ref.as_str())))
            .collect();
        let mut points: Vec<SeriesPoint> = (0..count)
            .map(|index| {
                let bucket_start = start.saturating_add((index as u64).saturating_mul(bucket_ms));
                SeriesPoint {
                    at_ms: bucket_start.max(from_ms),
                    end_ms: bucket_start.saturating_add(bucket_ms).min(to_ms),
                    entries: 0,
                    minted: 0,
                    escrowed: 0,
                    paid: 0,
                    refunded: 0,
                    fined: 0,
                    spent: 0,
                    by_kind: EntryKind::ALL
                        .iter()
                        .map(|kind| (kind.as_str(), 0))
                        .collect(),
                    plaza_budget_end: 0,
                    circulating_end: 0,
                    treasury_end: 0,
                }
            })
            .collect();
        // What each bucket moves in the three balances, in the order
        // [plaza budget, circulating, treasury], and what was there before `from_ms`.
        let mut moves = vec![[0i128; 3]; count];
        let mut opening = [0i128; 3];
        for entry in &inner.entries {
            let mut moved = [0i128; 3];
            let mut size = 0i128;
            for posting in &entry.postings {
                let amount = i128::from(posting.amount);
                if amount > 0 {
                    size += amount;
                }
                match accounts.get(&posting.account_id) {
                    Some((AccountKind::TownBudget, owner)) if *owner == PLAZA_OWNER => {
                        moved[0] += amount
                    }
                    Some((AccountKind::TaskEscrow | AccountKind::AgentEarnings, _)) => {
                        moved[1] += amount
                    }
                    Some((AccountKind::Treasury, _)) => moved[2] += amount,
                    _ => {}
                }
            }
            if entry.created_at < from_ms {
                for (total, amount) in opening.iter_mut().zip(moved) {
                    *total += amount;
                }
                continue;
            }
            if entry.created_at >= to_ms {
                continue;
            }
            // `created_at` is in `[from_ms, to_ms)`, so the index is below `count`.
            let index =
                usize::try_from((entry.created_at - start) / bucket_ms).unwrap_or(usize::MAX);
            let point = &mut points[index];
            point.entries += 1;
            *point.by_kind.entry(entry.kind.as_str()).or_insert(0) += size;
            match entry.kind {
                EntryKind::Mint => point.minted += size,
                EntryKind::Subcontract => point.escrowed += size,
                EntryKind::Payment => point.paid += size,
                EntryKind::Refund => point.refunded += size,
                EntryKind::Fine => point.fined += size,
                kind if kind.is_real_cost() => point.spent += size,
                _ => {}
            }
            for (total, amount) in moves[index].iter_mut().zip(moved) {
                *total += amount;
            }
        }
        let mut balances = opening;
        for (point, moved) in points.iter_mut().zip(&moves) {
            for (balance, amount) in balances.iter_mut().zip(moved) {
                *balance += amount;
            }
            point.plaza_budget_end = balances[0];
            point.circulating_end = balances[1];
            point.treasury_end = balances[2];
        }
        Ok(TreasurySeries {
            from_ms,
            to_ms,
            bucket_ms,
            points,
        })
    }

    /// The `n` agent wallets with the largest balances, richest first (ties by
    /// cid). Each carries what it has earned (its `payment` credits) and what has
    /// left it (every debit: transfers out, fines, fees, clawbacks), over the
    /// journal's life.
    pub fn top_wallets(&self, n: usize) -> Vec<WalletRank> {
        let inner = self.lock();
        let mut flows: BTreeMap<u64, (i128, i128)> = BTreeMap::new();
        for entry in &inner.entries {
            for posting in &entry.postings {
                let amount = i128::from(posting.amount);
                let (earned, spent) = flows.entry(posting.account_id).or_insert((0, 0));
                if entry.kind == EntryKind::Payment && amount > 0 {
                    *earned += amount;
                }
                if amount < 0 {
                    *spent -= amount;
                }
            }
        }
        let mut ranks: Vec<WalletRank> = inner
            .accounts
            .iter()
            .filter(|account| account.kind == AccountKind::AgentEarnings)
            .filter_map(|account| {
                let cid = account
                    .owner_ref
                    .strip_prefix("cid:")?
                    .parse::<u64>()
                    .ok()?;
                let (earned, spent) = flows.get(&account.id).copied().unwrap_or((0, 0));
                Some(WalletRank {
                    cid,
                    balance: inner.balance(account.id),
                    earned,
                    spent,
                })
            })
            .collect();
        ranks.sort_by(|a, b| b.balance.cmp(&a.balance).then(a.cid.cmp(&b.cid)));
        ranks.truncate(n);
        ranks
    }

    /// The append-only guard: checks that the journal has only ever been
    /// appended to — entry ids are exactly `1..=n` in order, the next id is
    /// one past the end, and every posting names a live account. Returns the
    /// reason it fails, which would be a bug here.
    pub fn verify_journal(&self) -> Result<(), String> {
        let inner = self.lock();
        for (index, entry) in inner.entries.iter().enumerate() {
            let expected = u64::try_from(index).unwrap_or(u64::MAX) + 1;
            if entry.id != expected {
                return Err(format!(
                    "entry {} is at position {}, expected id {}",
                    entry.id, index, expected
                ));
            }
            for posting in &entry.postings {
                if inner.account_kind(posting.account_id).is_none() {
                    return Err(format!(
                        "entry {} posts to unknown account {}",
                        entry.id, posting.account_id
                    ));
                }
            }
        }
        if inner.next_entry != u64::try_from(inner.entries.len()).unwrap_or(u64::MAX) {
            return Err(format!(
                "the next entry id ({}) does not follow the journal's length ({})",
                inner.next_entry,
                inner.entries.len()
            ));
        }
        Ok(())
    }

    /// Appends one entry to the journal, publishing its frame. The caller holds
    /// the lock; nothing is mutated unless every check passes.
    fn append(&self, inner: &mut Inner, spec: &EntrySpec) -> Result<u64, PostError> {
        if inner.entries.len() >= MAX_ENTRIES {
            return Err(PostError::JournalFull { max: MAX_ENTRIES });
        }
        if spec.postings.is_empty() {
            return Err(PostError::Empty);
        }
        for posting in &spec.postings {
            if posting.amount == 0 {
                return Err(PostError::ZeroPosting {
                    account: posting.kind,
                    owner_ref: posting.owner.clone(),
                });
            }
        }
        if spec.kind.names_a_chain() && spec.root_task.is_none() {
            return Err(PostError::RootTaskRequired {
                entry_kind: spec.kind,
            });
        }
        for (index, posting) in spec.postings.iter().enumerate() {
            if spec.postings[..index]
                .iter()
                .any(|earlier| earlier.kind == posting.kind && earlier.owner == posting.owner)
            {
                return Err(PostError::DuplicatePosting {
                    account: posting.kind,
                    owner_ref: posting.owner.clone(),
                });
            }
        }
        for posting in &spec.postings {
            let side = Side::of(posting.amount);
            if !rule_allows(spec.kind, posting.kind, side) {
                return Err(PostError::Rule {
                    entry_kind: spec.kind,
                    account_kind: posting.kind,
                    side,
                });
            }
        }
        let sum: i128 = spec
            .postings
            .iter()
            .map(|posting| i128::from(posting.amount))
            .sum();
        if sum != 0 {
            return Err(PostError::Unbalanced { sum });
        }
        for posting in &spec.postings {
            let existing = inner.find_account(posting.kind, &posting.owner);
            let before = existing.map_or(0, |account| inner.balance(account.id));
            let after = before + i128::from(posting.amount);
            let credit_limit = existing.map_or(0, |account| account.credit_limit);
            if posting.kind.is_limited() && after < -i128::from(credit_limit) {
                return Err(PostError::Overdrawn {
                    account: posting.kind,
                    owner_ref: posting.owner.clone(),
                    balance: after,
                    credit_limit,
                });
            }
        }
        if spec.kind == EntryKind::Payment {
            let root = spec.root_task.as_deref().unwrap_or_default();
            let (real_cost, paid_to_agents) = inner.chain_margin(root, spec);
            if paid_to_agents * 100 > real_cost * i128::from(MARGIN_CAP_PCT) {
                return Err(PostError::MarginCap {
                    root_task: root.to_string(),
                    paid_to_agents,
                    real_cost,
                });
            }
        }
        let opening = spec
            .postings
            .iter()
            .filter(|posting| inner.find_account(posting.kind, &posting.owner).is_none())
            .count();
        if inner.accounts.len() + opening > MAX_ACCOUNTS {
            return Err(PostError::AccountsFull { max: MAX_ACCOUNTS });
        }

        let now = now_ms();
        let mut postings = Vec::with_capacity(spec.postings.len());
        for posting in &spec.postings {
            let account_id = match inner
                .find_account(posting.kind, &posting.owner)
                .map(|a| a.id)
            {
                Some(id) => id,
                None => {
                    inner.next_account += 1;
                    let id = inner.next_account;
                    inner.accounts.push(Account {
                        id,
                        kind: posting.kind,
                        owner_ref: posting.owner.clone(),
                        credit_limit: 0,
                        created_at: now,
                    });
                    id
                }
            };
            postings.push(Posting {
                account_id,
                amount: posting.amount,
            });
        }
        inner.next_entry += 1;
        let id = inner.next_entry;
        let amount: i128 = postings
            .iter()
            .filter(|posting| posting.amount > 0)
            .map(|posting| i128::from(posting.amount))
            .sum();
        inner.entries.push(Entry {
            id,
            kind: spec.kind,
            task_id: spec.task_id.clone(),
            root_task: spec.root_task.clone(),
            memo: spec.memo.clone(),
            posted_by: POSTED_BY,
            created_at: now,
            postings,
        });
        self.version.fetch_add(1, Ordering::SeqCst);
        self.events.publish(Event::Treasury {
            entry: id,
            kind: spec.kind.as_str(),
            amount,
            task: spec.task_id.clone(),
        });
        Ok(id)
    }

    /// The treasury's whole state as JSON: the accounts, the journal and the
    /// transfer allowances. No balance is in it; restore derives them again.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        serde_json::to_string(&*inner).expect("the treasury state serializes")
    }

    /// Counts every change to the accounts or the journal (a posting, an account
    /// opened, a restore). A saver compares it to know when there is to save.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Replaces the whole state with `json`, a [`Treasury::snapshot`]. The
    /// snapshot must hold to the invariants a live post does; a refusal changes
    /// nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: Inner = serde_json::from_str(json)
            .map_err(|err| format!("the snapshot does not parse: {err}"))?;
        check_snapshot(&restored)?;
        *self.lock() = restored;
        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An account as the API reports it: the schema's row plus its derived balance.
#[derive(Debug, Serialize)]
pub struct AccountView {
    /// The account's id.
    pub id: u64,
    /// The account's kind.
    pub kind: &'static str,
    /// The account's owner (`owner_ref`).
    pub owner_ref: String,
    /// The account's credit limit (0 at v0).
    pub credit_limit: i64,
    /// The balance, the sum of the account's postings.
    pub balance: i128,
    /// When the account was opened, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// Every account of `inner`, in creation order, each with its derived balance.
fn account_views(inner: &Inner) -> Vec<AccountView> {
    inner
        .accounts
        .iter()
        .map(|account| AccountView {
            id: account.id,
            kind: account.kind.as_str(),
            owner_ref: account.owner_ref.clone(),
            credit_limit: account.credit_limit,
            balance: inner.balance(account.id),
            created_at: account.created_at,
        })
        .collect()
}

/// One entry as the journal endpoints report it, its postings named by account kind and owner.
fn entry_view(inner: &Inner, entry: &Entry) -> EntryView {
    let postings = entry
        .postings
        .iter()
        .map(|posting| PostingView {
            account: posting.account_id,
            kind: inner
                .account_kind(posting.account_id)
                .map_or("unknown", AccountKind::as_str),
            owner_ref: inner
                .accounts
                .iter()
                .find(|account| account.id == posting.account_id)
                .map_or_else(String::new, |account| account.owner_ref.clone()),
            amount: posting.amount,
        })
        .collect();
    EntryView {
        id: entry.id,
        kind: entry.kind.as_str(),
        task_id: entry.task_id.clone(),
        root_task: entry.root_task.clone(),
        memo: entry.memo.clone(),
        posted_by: entry.posted_by,
        created_at: entry.created_at,
        amount: entry
            .postings
            .iter()
            .filter(|posting| posting.amount > 0)
            .map(|posting| i128::from(posting.amount))
            .sum(),
        postings,
    }
}

/// One posting as the API reports it.
#[derive(Debug, Serialize)]
pub struct PostingView {
    /// The account's id.
    pub account: u64,
    /// The account's kind.
    pub kind: &'static str,
    /// The account's owner.
    pub owner_ref: String,
    /// The amount, negative for a debit, positive for a credit.
    pub amount: i64,
}

/// One entry as the API reports it.
#[derive(Debug, Serialize)]
pub struct EntryView {
    /// The entry's id.
    pub id: u64,
    /// The entry's kind.
    pub kind: &'static str,
    /// The task the entry belongs to, or `null`.
    pub task_id: Option<String>,
    /// The entry's chain root, or `null` (only `fine`/`fee` entries have none).
    pub root_task: Option<String>,
    /// The entry's memo.
    pub memo: String,
    /// Who posted it (always `treasury-service`).
    pub posted_by: &'static str,
    /// When it was posted, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The entry's size: the total of its credit-side postings (equal to the
    /// total of its debits, since every entry balances).
    pub amount: i128,
    /// The entry's postings, which sum to zero.
    pub postings: Vec<PostingView>,
}

/// The journal endpoint's body.
#[derive(Debug, Serialize)]
pub struct JournalView {
    /// Up to the endpoint's limit of entries, in chronological order (newest
    /// last).
    pub entries: Vec<EntryView>,
    /// How many entries the journal holds.
    pub total: usize,
    /// The limit the response was cut to.
    pub limit: usize,
    /// Whether the journal holds more entries than the response carries.
    pub truncated: bool,
}

/// The treasury view's totals line.
#[derive(Debug, Serialize)]
pub struct TotalsView {
    /// How many accounts the treasury holds.
    pub accounts: usize,
    /// How many entries the journal holds.
    pub entries: usize,
    /// The total credited across the journal.
    pub credits: i128,
    /// The total debited across the journal.
    pub debits: i128,
    /// The sum of every account's balance: zero while the journal balances.
    pub balance: i128,
}

/// `GET /v1/treasury`'s body.
#[derive(Debug, Serialize)]
pub struct TreasuryView {
    /// The accounts, in creation order, each with its derived balance.
    pub accounts: Vec<AccountView>,
    /// The totals line.
    pub totals: TotalsView,
    /// Real money the plaza has paid providers: every metered `spend` and `model` entry's
    /// credits to `external_spend`.
    pub spent: i128,
}

/// What a journal page asks for (see [`Treasury::journal_page`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalQuery {
    /// Only entries with an id below this: the page after a cursor.
    pub before: Option<u64>,
    /// The most entries the page holds, clamped to `1..=`[`JOURNAL_LIMIT`].
    pub limit: usize,
    /// Only entries of this kind, spelled as [`EntryKind::as_str`] spells it.
    pub kind: Option<String>,
    /// Only entries that post to this agent's wallet (its `agent_earnings`
    /// account), named by the agent's cid in decimal. A cid with no wallet
    /// matches nothing.
    pub wallet: Option<String>,
    /// Only entries of this task (`task_id`).
    pub task: Option<String>,
}

/// One page of the journal (see [`Treasury::journal_page`]).
#[derive(Debug, Serialize)]
pub struct JournalPage {
    /// The page's entries, newest first.
    pub entries: Vec<EntryView>,
    /// How many entries of the journal the filters match, the cursor not counted.
    pub total: usize,
    /// The `before` for the next, older page, or `None` when this page is the last.
    pub next_before: Option<u64>,
}

/// One bucket of the treasury series (see [`Treasury::series`]). Each flow is
/// the entries of its kind posted in the bucket, measured as an entry's size
/// (the total of its credit-side postings, which equals its debits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeriesPoint {
    /// The bucket's start, in milliseconds since the Unix epoch (clipped to the range's start).
    pub at_ms: u64,
    /// The bucket's end, exclusive (clipped to the range's end).
    pub end_ms: u64,
    /// How many entries were posted in the bucket.
    pub entries: usize,
    /// Credits minted (`mint` entries).
    pub minted: i128,
    /// Credits locked into task escrow from the plaza's budget (`subcontract` entries).
    pub escrowed: i128,
    /// Credits paid to agents for verified work (`payment` entries).
    pub paid: i128,
    /// Credits refunded to the plaza's budget (`refund` entries).
    pub refunded: i128,
    /// Credits fined from agent wallets into the treasury (`fine` entries).
    pub fined: i128,
    /// Metered model spend (`spend` entries). Real money, in credits.
    pub spent: i128,
    /// The size of every entry kind posted in the bucket, zero for kinds that did not
    /// occur; `clawback`, `fee`, `budget_move` and `transfer` appear here only.
    pub by_kind: BTreeMap<&'static str, i128>,
    /// The plaza's budget (its `town_budget` account) at the bucket's end.
    pub plaza_budget_end: i128,
    /// The credits in play at the bucket's end: task escrows plus agent wallets.
    pub circulating_end: i128,
    /// The treasury's balance (collected fines and fees) at the bucket's end.
    pub treasury_end: i128,
}

/// The treasury over `[from_ms, to_ms)` (see [`Treasury::series`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreasurySeries {
    /// The range's start, in milliseconds since the Unix epoch.
    pub from_ms: u64,
    /// The range's end, exclusive.
    pub to_ms: u64,
    /// The bucket width in milliseconds.
    pub bucket_ms: u64,
    /// Every bucket in the range, oldest first.
    pub points: Vec<SeriesPoint>,
}

/// One agent wallet as the top-wallets query reports it (see [`Treasury::top_wallets`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WalletRank {
    /// The agent's cid.
    pub cid: u64,
    /// The wallet's balance.
    pub balance: i128,
    /// Credits the wallet has earned from task payments, over the journal's life.
    pub earned: i128,
    /// Credits debited from the wallet over the journal's life (transfers out, fines, fees, clawbacks).
    pub spent: i128,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A treasury with no subscribers.
    fn new_treasury() -> Treasury {
        Treasury::new(EventBus::new())
    }

    /// An entry spec from `(kind, owner, amount)` posting triples.
    fn spec(kind: EntryKind, postings: &[(AccountKind, &str, i64)]) -> EntrySpec {
        EntrySpec::new(
            kind,
            postings
                .iter()
                .map(|(kind, owner, amount)| PostingSpec::new(*kind, owner, *amount))
                .collect(),
        )
    }

    /// Mints `amount` into `task`'s escrow through the plain `post` path (the
    /// ledger's own `mint_task_budget` is exercised separately).
    fn fund(treasury: &Treasury, task: &str, amount: i64) {
        treasury
            .post(
                &spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -amount),
                        (AccountKind::TaskEscrow, task, amount),
                    ],
                )
                .task(task)
                .root(task),
            )
            .expect("a mint inside the rules");
    }

    /// The schema's `treasury.posting_rules`, typed out from the INSERT in
    /// `docs/treasury-draft/schema.sql` — the same table [`POSTING_RULES`]
    /// encodes, listed here independently so a transcription error in the
    /// constant is caught.
    const SCHEMA_RULES: [(EntryKind, AccountKind, Side); 31] = [
        (EntryKind::Mint, AccountKind::Mint, Side::Debit),
        (EntryKind::Mint, AccountKind::TaskEscrow, Side::Credit),
        (EntryKind::Mint, AccountKind::TownBudget, Side::Credit),
        (EntryKind::Subcontract, AccountKind::TownBudget, Side::Debit),
        (EntryKind::Refund, AccountKind::TownBudget, Side::Credit),
        (EntryKind::BudgetMove, AccountKind::TownBudget, Side::Debit),
        (EntryKind::BudgetMove, AccountKind::TownBudget, Side::Credit),
        (EntryKind::Spend, AccountKind::TaskEscrow, Side::Debit),
        (EntryKind::Spend, AccountKind::ExternalSpend, Side::Credit),
        (EntryKind::Subcontract, AccountKind::TaskEscrow, Side::Debit),
        (
            EntryKind::Subcontract,
            AccountKind::TaskEscrow,
            Side::Credit,
        ),
        (EntryKind::Payment, AccountKind::TaskEscrow, Side::Debit),
        (EntryKind::Payment, AccountKind::AgentEarnings, Side::Credit),
        (EntryKind::Refund, AccountKind::TaskEscrow, Side::Debit),
        (EntryKind::Refund, AccountKind::TaskEscrow, Side::Credit),
        (EntryKind::Refund, AccountKind::Mint, Side::Credit),
        (EntryKind::Clawback, AccountKind::AgentEarnings, Side::Debit),
        (EntryKind::Clawback, AccountKind::TaskEscrow, Side::Credit),
        (EntryKind::Clawback, AccountKind::Treasury, Side::Credit),
        (EntryKind::Fine, AccountKind::AgentEarnings, Side::Debit),
        (EntryKind::Fine, AccountKind::Treasury, Side::Credit),
        (EntryKind::Fee, AccountKind::AgentEarnings, Side::Debit),
        (EntryKind::Fee, AccountKind::Treasury, Side::Credit),
        (EntryKind::Fee, AccountKind::Treasury, Side::Debit),
        (EntryKind::Fee, AccountKind::AgentEarnings, Side::Credit),
        (EntryKind::Spend, AccountKind::TownBudget, Side::Debit),
        (EntryKind::Transfer, AccountKind::AgentEarnings, Side::Debit),
        (
            EntryKind::Transfer,
            AccountKind::AgentEarnings,
            Side::Credit,
        ),
        (EntryKind::Model, AccountKind::TaskEscrow, Side::Debit),
        (EntryKind::Model, AccountKind::AgentEarnings, Side::Debit),
        (EntryKind::Model, AccountKind::ExternalSpend, Side::Credit),
    ];

    #[test]
    fn the_posting_rule_matrix_is_the_schemas() {
        // Every (entry kind, account kind, side) pair: allowed exactly when the
        // schema's table lists it, and nothing else.
        for entry_kind in EntryKind::ALL {
            for account_kind in AccountKind::ALL {
                for side in [Side::Debit, Side::Credit] {
                    assert_eq!(
                        rule_allows(entry_kind, account_kind, side),
                        SCHEMA_RULES.contains(&(entry_kind, account_kind, side)),
                        "{} may not be compared for {} {}",
                        entry_kind.as_str(),
                        side.as_str(),
                        account_kind.as_str()
                    );
                }
            }
        }
        // The constant itself is the schema's table, with no extra or missing row.
        assert_eq!(POSTING_RULES.len(), SCHEMA_RULES.len());
        for rule in SCHEMA_RULES {
            assert!(POSTING_RULES.contains(&rule), "{rule:?} is missing");
        }
    }

    #[test]
    fn postings_outside_the_rules_are_refused() {
        // Each forbidden (entry kind, account kind, side) is refused by the posting rules,
        // naming the rule, and none of them touches the journal.
        {
            let treasury = new_treasury();
            // One forbidden (entry kind, account kind, side) per entry kind: a pair
            // the schema's `posting_rules` table does not list.
            let forbidden = [
                (
                    EntryKind::Mint,
                    AccountKind::Mint,
                    Side::Credit,
                    "mint entry may not credit a mint account",
                ),
                (
                    EntryKind::Spend,
                    AccountKind::TaskEscrow,
                    Side::Credit,
                    "spend entry may not credit a task_escrow account",
                ),
                (
                    EntryKind::Subcontract,
                    AccountKind::Mint,
                    Side::Debit,
                    "subcontract entry may not debit a mint account",
                ),
                (
                    EntryKind::Payment,
                    AccountKind::ExternalSpend,
                    Side::Credit,
                    "payment entry may not credit a external_spend account",
                ),
                (
                    EntryKind::Refund,
                    AccountKind::Treasury,
                    Side::Credit,
                    "refund entry may not credit a treasury account",
                ),
                (
                    EntryKind::Clawback,
                    AccountKind::TownBudget,
                    Side::Credit,
                    "clawback entry may not credit a town_budget account",
                ),
                (
                    EntryKind::Fine,
                    AccountKind::TaskEscrow,
                    Side::Debit,
                    "fine entry may not debit a task_escrow account",
                ),
                (
                    EntryKind::Fee,
                    AccountKind::Mint,
                    Side::Credit,
                    "fee entry may not credit a mint account",
                ),
                (
                    EntryKind::BudgetMove,
                    AccountKind::AgentEarnings,
                    Side::Credit,
                    "budget_move entry may not credit a agent_earnings account",
                ),
                (
                    EntryKind::Transfer,
                    AccountKind::TaskEscrow,
                    Side::Debit,
                    "transfer entry may not debit a task_escrow account",
                ),
            ];
            for (entry_kind, account_kind, side, message) in forbidden {
                // The offending posting is first, so it is the one the rules
                // refuse; the counter-posting is a different account entirely (so
                // nothing before the rule check objects) and the entry is
                // deliberately left unbalanced — the rule must fire before the
                // balance check.
                let amount = if side == Side::Debit { -10 } else { 10 };
                let spec = spec(
                    entry_kind,
                    &[
                        (account_kind, "subject", amount),
                        (AccountKind::Mint, "counter", -amount),
                    ],
                )
                .root("t1");
                let refused = treasury
                    .post(&spec)
                    .expect_err(&format!("{entry_kind:?} {account_kind:?} {side:?}"));
                assert!(
                    matches!(
                        refused,
                        PostError::Rule {
                            entry_kind: got_kind,
                            account_kind: got_account,
                            side: got_side,
                        } if got_kind == entry_kind && got_account == account_kind && got_side == side
                    ),
                    "{entry_kind:?} {account_kind:?} {side:?}: {refused:?}"
                );
                assert_eq!(refused.to_string(), message);
            }
            // None of these touched the journal.
            assert_eq!(treasury.view().totals.entries, 1);
        }

        // A fine may never debit escrow: the rule refuses it even in an otherwise balanced entry.
        {
            // The schema's own example: a `fine` posting on a `task_escrow` account
            // is refused by the rule, even in an otherwise balanced entry.
            let treasury = new_treasury();
            fund(&treasury, "t1", 100);
            let refused = treasury
                .post(
                    &spec(
                        EntryKind::Fine,
                        &[
                            (AccountKind::TaskEscrow, "t1", -10),
                            (AccountKind::Treasury, "treasury", 10),
                        ],
                    )
                    .task("t1")
                    .root("t1"),
                )
                .expect_err("a fine may not come from escrow");
            assert_eq!(
                refused.to_string(),
                "fine entry may not debit a task_escrow account"
            );
            assert_eq!(treasury.escrow_balance("t1"), 100);
        }
    }

    #[test]
    fn balances_are_derived_from_the_postings() {
        let treasury = new_treasury();
        fund(&treasury, "t1", 500);
        treasury
            .post(
                &spec(
                    EntryKind::Spend,
                    &[
                        (AccountKind::TaskEscrow, "t1", -200),
                        (AccountKind::ExternalSpend, "providers", 200),
                    ],
                )
                .task("t1")
                .root("t1"),
            )
            .expect("a spend inside the rules");

        let accounts = treasury.accounts();
        let balance = |kind: &str, owner: &str| {
            accounts
                .iter()
                .find(|account| account.kind == kind && account.owner_ref == owner)
                .unwrap_or_else(|| panic!("{kind} {owner} exists"))
                .balance
        };
        assert_eq!(balance("mint", MINT_OWNER), -i128::from(PLAZA_BUDGET) - 500);
        assert_eq!(balance("task_escrow", "t1"), 300);
        assert_eq!(balance("external_spend", "providers"), 200);

        // The derived balances are exactly the sums over the journal's
        // postings — the same arithmetic `treasury.balances` does.
        let journal = treasury.journal(JOURNAL_LIMIT);
        let sum = |kind: &str, owner: &str| -> i128 {
            journal
                .entries
                .iter()
                .flat_map(|entry| &entry.postings)
                .filter(|posting| posting.kind == kind && posting.owner_ref == owner)
                .map(|posting| i128::from(posting.amount))
                .sum()
        };
        assert_eq!(sum("mint", MINT_OWNER), balance("mint", MINT_OWNER));
        assert_eq!(sum("task_escrow", "t1"), balance("task_escrow", "t1"));
        assert_eq!(
            sum("external_spend", "providers"),
            balance("external_spend", "providers")
        );

        // Every account's balance sums to zero: the journal balances.
        let total: i128 = accounts.iter().map(|account| account.balance).sum();
        assert_eq!(total, 0);
        assert_eq!(treasury.view().totals.balance, 0);
    }

    #[test]
    fn funding_and_refund_cycles_keep_every_balance_consistent() {
        // A mint funds a task, and refunding the escrow returns what is left to the mint.
        {
            let treasury = new_treasury();
            let mint = treasury.fund_task("t1", 500).expect("a funding");
            assert_eq!(treasury.escrow_balance("t1"), 500);
            assert_eq!(
                treasury.plaza_budget_balance(),
                i128::from(PLAZA_BUDGET) - 500
            );
            assert_eq!(treasury.view().totals.entries, 2);

            let refund = treasury
                .refund_task_to_budget("t1")
                .expect("a refund")
                .expect("there was an escrow to refund");
            assert_eq!(refund, mint + 1);
            assert_eq!(treasury.escrow_balance("t1"), 0);

            let view = treasury.view();
            assert_eq!(view.totals.entries, 3);
            assert_eq!(view.spent, 0, "a funding and a refund are not spend");
            // Every entry balances, and the refund brought the budget home.
            assert_eq!(view.totals.balance, 0);
            let budget = view
                .accounts
                .iter()
                .find(|account| account.kind == "town_budget")
                .expect("the plaza's budget account");
            assert_eq!(budget.balance, i128::from(PLAZA_BUDGET));
            // A second refund has nothing to refund and posts nothing.
            assert_eq!(treasury.refund_task_to_budget("t1").expect("no-op"), None);
            assert_eq!(treasury.view().totals.entries, 3);
            // A task that never had a budget has no escrow to refund.
            assert_eq!(treasury.refund_task_to_budget("t9").expect("no-op"), None);
            assert!(treasury.verify_journal().is_ok());
        }

        // Funding, spending and refunding keep every balance consistent with the journal.
        {
            let treasury = Treasury::new(EventBus::new());
            assert_eq!(treasury.plaza_budget_balance(), i128::from(PLAZA_BUDGET));
            treasury.fund_task("t1", 300).expect("fund");
            assert_eq!(
                treasury.plaza_budget_balance(),
                i128::from(PLAZA_BUDGET) - 300
            );
            assert_eq!(treasury.escrow_balance("t1"), 300);
            treasury
                .charge_spend(Some("t1"), 120, "model call")
                .expect("spend");
            assert_eq!(treasury.escrow_balance("t1"), 180);
            assert!(
                treasury
                    .refund_task_to_budget("t1")
                    .expect("refund")
                    .is_some()
            );
            assert_eq!(treasury.escrow_balance("t1"), 0);
            // The 120 credits spent are gone; the rest came home to the plaza.
            assert_eq!(
                treasury.plaza_budget_balance(),
                i128::from(PLAZA_BUDGET) - 120
            );
            assert_eq!(
                treasury.refund_task_to_budget("t1").expect("nothing left"),
                None
            );
            assert!(treasury.verify_journal().is_ok());
            assert_eq!(treasury.view().totals.balance, 0);
        }
    }
    #[test]
    fn the_minted_total_counts_what_the_mint_issued_not_what_came_back() {
        let treasury = new_treasury();
        // The plaza's opening mint is the only issue so far.
        assert_eq!(treasury.minted_total(), i128::from(PLAZA_BUDGET));
        treasury.mint_plaza_budget(250).expect("a mint");
        assert_eq!(
            treasury.minted_total(),
            i128::from(PLAZA_BUDGET) + 250,
            "the mint raised the total"
        );
        // Funding and refunding move credits around the plaza but issue none,
        // so the total is what the mint issued, not what it holds.
        treasury.fund_task("t1", 500).expect("a funding");
        treasury.refund_task_to_budget("t1").expect("a refund");
        assert_eq!(treasury.minted_total(), i128::from(PLAZA_BUDGET) + 250);
    }
    #[test]
    fn spends_cannot_overdraw_an_escrow_or_the_budget() {
        // An escrow may not go negative: an overdraw is refused and changes nothing,
        // but draining it to zero is allowed.
        {
            let treasury = new_treasury();
            fund(&treasury, "t1", 100);
            assert_eq!(treasury.escrow_balance("t1"), 100);

            // 101 out of an escrow of 100 is refused, naming the limit.
            let refused = treasury
                .post(
                    &spec(
                        EntryKind::Spend,
                        &[
                            (AccountKind::TaskEscrow, "t1", -101),
                            (AccountKind::ExternalSpend, "providers", 101),
                        ],
                    )
                    .task("t1")
                    .root("t1"),
                )
                .expect_err("an escrow may not go negative");
            assert!(
                matches!(
                    refused,
                    PostError::Overdrawn {
                        account: AccountKind::TaskEscrow,
                        ref owner_ref,
                        balance: -1,
                        credit_limit: 0,
                    } if owner_ref == "t1"
                ),
                "{refused:?}"
            );
            assert!(refused.to_string().contains("below its limit"), "{refused}");
            // The refusal changed nothing.
            assert_eq!(treasury.escrow_balance("t1"), 100);
            assert_eq!(treasury.view().totals.entries, 2);

            // Draining it exactly is allowed.
            treasury
                .post(
                    &spec(
                        EntryKind::Spend,
                        &[
                            (AccountKind::TaskEscrow, "t1", -100),
                            (AccountKind::ExternalSpend, "providers", 100),
                        ],
                    )
                    .task("t1")
                    .root("t1"),
                )
                .expect("an escrow may be drained to zero");
            assert_eq!(treasury.escrow_balance("t1"), 0);
        }

        // A spend past the escrow or the plaza budget, and a funding past the budget, are refused.
        {
            let treasury = Treasury::new(EventBus::new());
            treasury.fund_task("t1", 50).expect("fund");
            let refused = treasury
                .charge_spend(Some("t1"), 51, "too much")
                .expect_err("past the escrow");
            assert!(
                matches!(refused, PostError::Overdrawn { .. }),
                "{refused:?}"
            );
            assert_eq!(treasury.escrow_balance("t1"), 50);
            // A spend from the plaza budget cannot overdraw it either, and funding
            // past the budget is refused the same way.
            let refused = treasury
                .charge_spend(None, PLAZA_BUDGET + 1, "plaza")
                .expect_err("past the budget");
            assert!(
                matches!(refused, PostError::Overdrawn { .. }),
                "{refused:?}"
            );
            let refused = treasury
                .fund_task("t2", PLAZA_BUDGET)
                .expect_err("past the budget");
            assert!(
                matches!(refused, PostError::Overdrawn { .. }),
                "{refused:?}"
            );
            assert_eq!(
                treasury.plaza_budget_balance(),
                i128::from(PLAZA_BUDGET) - 50
            );
            assert!(treasury.verify_journal().is_ok());
        }
    }

    #[test]
    fn an_earnings_account_may_go_negative_only_to_its_credit_line() {
        let treasury = new_treasury();
        treasury
            .open_account(AccountKind::AgentEarnings, "bob", 50)
            .expect("open bob's earnings with a credit line");
        // A fine may spend down to the credit line...
        treasury
            .post(&spec(
                EntryKind::Fine,
                &[
                    (AccountKind::AgentEarnings, "bob", -50),
                    (AccountKind::Treasury, "treasury", 50),
                ],
            ))
            .expect("a fine within the credit line");
        assert_eq!(
            treasury
                .accounts()
                .iter()
                .find(|account| account.kind == "agent_earnings" && account.owner_ref == "bob")
                .expect("bob's account")
                .balance,
            -50
        );
        // ...but not past it.
        let refused = treasury
            .post(&spec(
                EntryKind::Fine,
                &[
                    (AccountKind::AgentEarnings, "bob", -1),
                    (AccountKind::Treasury, "treasury", 1),
                ],
            ))
            .expect_err("a fine past the credit line");
        assert!(
            matches!(
                refused,
                PostError::Overdrawn {
                    account: AccountKind::AgentEarnings,
                    balance: -51,
                    credit_limit: 50,
                    ..
                }
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn payments_respect_the_chain_margin_cap() {
        // A payment may not exceed 20 % of the chain's real cost, and a clawback does not
        // give the room back.
        {
            let treasury = new_treasury();
            fund(&treasury, "root", 1000);
            let post = |kind, postings: &[(AccountKind, &str, i64)]| {
                let entry = spec(kind, postings).task("sub1").root("root");
                treasury.post(&entry)
            };
            post(
                EntryKind::Subcontract,
                &[
                    (AccountKind::TaskEscrow, "root", -400),
                    (AccountKind::TaskEscrow, "sub1", 400),
                ],
            )
            .expect("a subcontract");
            post(
                EntryKind::Spend,
                &[
                    (AccountKind::TaskEscrow, "sub1", -100),
                    (AccountKind::ExternalSpend, "providers", 100),
                ],
            )
            .expect("a spend");
            // 20% of the chain's 100 real cost is 20: one more is refused...
            let refused = post(
                EntryKind::Payment,
                &[
                    (AccountKind::TaskEscrow, "sub1", -21),
                    (AccountKind::AgentEarnings, "alice", 21),
                ],
            )
            .expect_err("a payment past the cap");
            assert!(
                matches!(
                    refused,
                    PostError::MarginCap {
                        ref root_task,
                        paid_to_agents: 21,
                        real_cost: 100,
                    } if root_task == "root"
                ),
                "{refused:?}"
            );
            assert!(refused.to_string().contains("margin cap"), "{refused}");
            // ...and 20 exactly is paid.
            post(
                EntryKind::Payment,
                &[
                    (AccountKind::TaskEscrow, "sub1", -20),
                    (AccountKind::AgentEarnings, "alice", 20),
                ],
            )
            .expect("a payment at the cap");
            // Clawbacks net against the cap, so the room does not come back: a
            // clawback lowers what the chain has paid, and the next payment's net
            // margin (1) is still within the cap.
            post(
                EntryKind::Clawback,
                &[
                    (AccountKind::AgentEarnings, "alice", -20),
                    (AccountKind::TaskEscrow, "sub1", 20),
                ],
            )
            .expect("a clawback");
            post(
                EntryKind::Payment,
                &[
                    (AccountKind::TaskEscrow, "sub1", -1),
                    (AccountKind::AgentEarnings, "bob", 1),
                ],
            )
            .expect("a payment after a clawback still fits (net margin 1)");
        }

        // A payment lands in the wallet, and none lands before the work has cost anything.
        {
            let treasury = Treasury::new(EventBus::new());
            treasury.fund_task("t1", 200).expect("fund");
            // A payment is capped at 20 % of the chain's real cost, so none lands
            // before the work has cost anything.
            assert!(matches!(
                treasury.pay_agent("t1", 7, 5),
                Err(PostError::MarginCap { .. })
            ));
            treasury
                .charge_spend(Some("t1"), 100, "real cost")
                .expect("spend");
            treasury.pay_agent("t1", 7, 20).expect("pay");
            assert_eq!(treasury.wallet(7), 20);
            assert_eq!(treasury.wallet(8), 0);
            assert_eq!(treasury.escrow_balance("t1"), 80);
            assert_eq!(treasury.wallets(), vec![(7, 20)]);
            assert!(treasury.verify_journal().is_ok());
        }
    }

    #[test]
    fn an_entry_must_be_balanced_and_well_formed() {
        // An entry must balance, and money that moves for a task must name its chain;
        // a fine moves no task's money and need not.
        {
            let treasury = new_treasury();
            let unbalanced = treasury
                .post(
                    &spec(
                        EntryKind::Mint,
                        &[
                            (AccountKind::Mint, MINT_OWNER, -1000),
                            (AccountKind::TaskEscrow, "t1", 999),
                        ],
                    )
                    .task("t1")
                    .root("t1"),
                )
                .expect_err("999 is not 1000");
            assert_eq!(unbalanced, PostError::Unbalanced { sum: -1 });
            assert!(unbalanced.to_string().contains("does not balance"));
            assert_eq!(treasury.view().totals.entries, 1);

            // Money that moves for a task must name its chain...
            let no_chain = treasury
                .post(&spec(
                    EntryKind::Refund,
                    &[
                        (AccountKind::TaskEscrow, "t1", -10),
                        (AccountKind::Mint, MINT_OWNER, 10),
                    ],
                ))
                .expect_err("a refund must name its chain");
            assert_eq!(
                no_chain,
                PostError::RootTaskRequired {
                    entry_kind: EntryKind::Refund
                }
            );
            // ...while a fine, which moves no task's money, need not.
            treasury
                .open_account(AccountKind::AgentEarnings, "alice", 1)
                .expect("open alice's earnings with a credit line");
            treasury
                .post(&spec(
                    EntryKind::Fine,
                    &[
                        (AccountKind::AgentEarnings, "alice", -1),
                        (AccountKind::Treasury, "treasury", 1),
                    ],
                ))
                .expect("a fine needs no chain");
        }

        // An empty, zero-amount, or duplicated-posting entry is refused, and changes nothing.
        {
            let treasury = new_treasury();
            assert_eq!(
                treasury.post(&EntrySpec::new(EntryKind::Mint, vec![])),
                Err(PostError::Empty)
            );
            assert_eq!(
                treasury.post(
                    &spec(
                        EntryKind::Mint,
                        &[
                            (AccountKind::Mint, MINT_OWNER, 0),
                            (AccountKind::TaskEscrow, "t1", 0)
                        ]
                    )
                    .root("t1")
                ),
                Err(PostError::ZeroPosting {
                    account: AccountKind::Mint,
                    owner_ref: MINT_OWNER.to_string()
                })
            );
            assert_eq!(
                treasury.post(
                    &spec(
                        EntryKind::Mint,
                        &[
                            (AccountKind::TaskEscrow, "t1", -5),
                            (AccountKind::TaskEscrow, "t1", 5)
                        ]
                    )
                    .root("t1")
                ),
                Err(PostError::DuplicatePosting {
                    account: AccountKind::TaskEscrow,
                    owner_ref: "t1".to_string()
                })
            );
            assert_eq!(treasury.view().totals.entries, 1);
        }
    }

    #[test]
    fn the_journal_is_append_only() {
        let treasury = new_treasury();
        fund(&treasury, "t1", 100);
        treasury
            .post(
                &spec(
                    EntryKind::Spend,
                    &[
                        (AccountKind::TaskEscrow, "t1", -40),
                        (AccountKind::ExternalSpend, "providers", 40),
                    ],
                )
                .task("t1")
                .root("t1"),
            )
            .expect("a spend");

        // The structural guard holds: ids in order, postings to live accounts.
        assert!(treasury.verify_journal().is_ok());

        // The ids are handed out once, in order.
        let ids: Vec<u64> = treasury
            .journal(JOURNAL_LIMIT)
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(treasury.journal(JOURNAL_LIMIT).total, 3);

        // Entries come out as snapshots: taking the journal twice yields equal
        // data, and a correction is a new entry, never an edit. (The journal
        // has no update or delete in its API at all, so a posted entry can only
        // ever be appended to.)
        let before = serde_json::to_value(treasury.journal(JOURNAL_LIMIT).entries).expect("json");
        treasury
            .post(
                &spec(
                    EntryKind::Refund,
                    &[
                        (AccountKind::TaskEscrow, "t1", -60),
                        (AccountKind::Mint, MINT_OWNER, 60),
                    ],
                )
                .task("t1")
                .root("t1")
                .memo("correction"),
            )
            .expect("a second refund");
        let after = serde_json::to_value(treasury.journal(JOURNAL_LIMIT).entries).expect("json");
        assert_eq!(
            after.as_array().expect("an array")[..3],
            before.as_array().expect("an array")[..]
        );
        assert_eq!(after.as_array().expect("an array").len(), 4);
        assert_eq!(treasury.journal(JOURNAL_LIMIT).entries[3].id, 4);
        assert!(treasury.verify_journal().is_ok());
    }

    #[test]
    fn accounts_are_opened_lazily_only_when_an_entry_needs_them() {
        let treasury = new_treasury();
        // Only the mint and the plaza's budget exist until something needs more.
        let kinds = |treasury: &Treasury| -> Vec<&'static str> {
            treasury
                .accounts()
                .iter()
                .map(|account| account.kind)
                .collect()
        };
        assert_eq!(kinds(&treasury), vec!["mint", "town_budget"]);

        fund(&treasury, "t1", 100);
        assert_eq!(kinds(&treasury), vec!["mint", "town_budget", "task_escrow"]);
        assert_eq!(treasury.accounts()[2].owner_ref, "t1");
        assert_eq!(treasury.accounts()[2].credit_limit, 0);

        // The other kinds appear only when an entry names them.
        treasury
            .post(
                &spec(
                    EntryKind::Spend,
                    &[
                        (AccountKind::TaskEscrow, "t1", -10),
                        (AccountKind::ExternalSpend, "providers", 10),
                    ],
                )
                .task("t1")
                .root("t1"),
            )
            .expect("a spend opens the spend counterparty");
        assert_eq!(
            kinds(&treasury),
            vec!["mint", "town_budget", "task_escrow", "external_spend"]
        );
        // v0's lifecycle posts none of the rest, so they are still absent.
        for kind in ["treasury", "agent_earnings"] {
            assert!(!kinds(&treasury).contains(&kind), "{kind} appeared");
        }
    }
    #[test]
    fn the_caps_refuse_honestly() {
        let treasury = new_treasury();
        // The journal's cap: fill it with mint entries to a town budget, which
        // the rules allow and which touches no limited account.
        for _ in 0..MAX_ENTRIES - 1 {
            treasury
                .post(
                    &spec(
                        EntryKind::Mint,
                        &[
                            (AccountKind::Mint, MINT_OWNER, -1),
                            (AccountKind::TownBudget, "shop", 1),
                        ],
                    )
                    .root("shop-oct"),
                )
                .expect("a mint under the cap");
        }
        assert_eq!(treasury.view().totals.entries, MAX_ENTRIES);
        let refused = treasury
            .post(
                &spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -1),
                        (AccountKind::TownBudget, "shop", 1),
                    ],
                )
                .root("shop-oct"),
            )
            .expect_err("the journal is full");
        assert_eq!(refused, PostError::JournalFull { max: MAX_ENTRIES });
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            format!("the treasury journal is full ({MAX_ENTRIES} entries)")
        );
        assert_eq!(treasury.view().totals.entries, MAX_ENTRIES);

        // The accounts cap: opening one past the cap is refused, and so is an
        // entry that would open one.
        let fresh = new_treasury();
        for index in 0..MAX_ACCOUNTS - 2 {
            fresh
                .open_account(AccountKind::TownBudget, &format!("t{index}"), 0)
                .expect("an account under the cap");
        }
        // The mint and plaza accounts plus MAX_ACCOUNTS - 2 town budgets are exactly the cap.
        assert_eq!(fresh.accounts().len(), MAX_ACCOUNTS);
        let refused = fresh
            .open_account(AccountKind::TownBudget, "one-too-many", 0)
            .expect_err("the accounts are full");
        assert_eq!(refused, PostError::AccountsFull { max: MAX_ACCOUNTS });
        assert_eq!(refused.status(), 409);
        let refused = fresh
            .post(
                &spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -1),
                        (AccountKind::TownBudget, "still-one-too-many", 1),
                    ],
                )
                .root("shop-oct"),
            )
            .expect_err("the accounts are full");
        assert_eq!(refused, PostError::AccountsFull { max: MAX_ACCOUNTS });
        assert_eq!(fresh.accounts().len(), MAX_ACCOUNTS);
        assert_eq!(fresh.view().totals.entries, 1);
    }

    #[test]
    fn the_journal_endpoint_is_bounded_and_ordered() {
        let treasury = new_treasury();
        for _ in 0..(JOURNAL_LIMIT + 10) {
            treasury
                .post(
                    &spec(
                        EntryKind::Mint,
                        &[
                            (AccountKind::Mint, MINT_OWNER, -1),
                            (AccountKind::TownBudget, "shop", 1),
                        ],
                    )
                    .root("shop-oct"),
                )
                .expect("a mint");
        }
        let journal = treasury.journal(JOURNAL_LIMIT);
        assert_eq!(journal.entries.len(), JOURNAL_LIMIT);
        assert_eq!(journal.total, JOURNAL_LIMIT + 11);
        assert_eq!(journal.limit, JOURNAL_LIMIT);
        assert!(journal.truncated);
        // Chronological: the newest entry is last, and the response holds the
        // newest window of the journal.
        assert_eq!(journal.entries[0].id, 12);
        assert_eq!(
            journal.entries[JOURNAL_LIMIT - 1].id,
            (JOURNAL_LIMIT + 11) as u64
        );

        // A short journal is not truncated, and is oldest-first.
        let small = new_treasury();
        small.fund_task("t1", 5).expect("mint");
        let journal = small.journal(JOURNAL_LIMIT);
        assert!(!journal.truncated);
        // The plaza's mint is entry 1; the funding is entry 2.
        assert_eq!(journal.total, 2);
        assert_eq!(journal.entries[1].kind, "subcontract");
        assert_eq!(journal.entries[1].task_id.as_deref(), Some("t1"));
        assert_eq!(journal.entries[1].amount, 5);
        assert_eq!(journal.entries[1].postings.len(), 2);
        assert_eq!(journal.entries[1].posted_by, POSTED_BY);
        assert_eq!(journal.entries[1].memo, "task budget funded from the plaza");
        // Every entry is a balanced pair summing to zero.
        let sum: i64 = journal.entries[1]
            .postings
            .iter()
            .map(|posting| posting.amount)
            .sum();
        assert_eq!(sum, 0);
    }

    #[test]
    fn every_entry_publishes_a_treasury_frame() {
        let events = EventBus::new();
        let receiver = events.subscribe();
        let treasury = Treasury::new(Arc::clone(&events));

        let mint = treasury.fund_task("t1", 500).expect("a funding");
        let refund = treasury
            .refund_task_to_budget("t1")
            .expect("a refund")
            .expect("something to refund");

        let mut frames = Vec::new();
        while let Ok(event) = receiver.recv_timeout(Duration::from_secs(1)) {
            frames.push(event.frame());
        }
        assert_eq!(
            frames,
            vec![
                "event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":100000,\"task\":null}\n\n"
                    .to_string(),
                format!(
                    "event: treasury\ndata: {{\"entry\":{mint},\"kind\":\"subcontract\",\"amount\":500,\"task\":\"t1\"}}\n\n"
                ),
                format!(
                    "event: treasury\ndata: {{\"entry\":{refund},\"kind\":\"refund\",\"amount\":500,\"task\":\"t1\"}}\n\n"
                ),
            ]
        );
    }
    #[test]
    fn the_append_only_guard_catches_a_tampered_journal() {
        let treasury = new_treasury();
        fund(&treasury, "t1", 100);
        assert!(treasury.verify_journal().is_ok());

        // A next id that does not follow the journal's length is not a journal
        // that was only appended to.
        treasury.lock().next_entry = 41;
        assert!(treasury.verify_journal().is_err());
        treasury.lock().next_entry = 1;

        // An entry whose id is not its position (a gap, a rewrite) is caught,
        // even when the next id and the length agree.
        treasury.lock().entries.push(Entry {
            id: 5,
            kind: EntryKind::Mint,
            task_id: None,
            root_task: Some("t1".to_string()),
            memo: String::new(),
            posted_by: POSTED_BY,
            created_at: 0,
            postings: vec![Posting {
                account_id: 1,
                amount: -1,
            }],
        });
        treasury.lock().next_entry = 2;
        assert!(treasury.verify_journal().is_err());
        treasury.lock().next_entry = 1;

        // A posting to an account that does not exist is caught.
        treasury.lock().entries.pop();
        treasury.lock().entries[0].postings.push(Posting {
            account_id: 999,
            amount: 0,
        });
        assert!(treasury.verify_journal().is_err());
    }

    #[test]
    fn a_budget_move_between_towns_is_balanced_and_bounded() {
        let treasury = new_treasury();
        treasury
            .post(
                &spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -100000),
                        (AccountKind::TownBudget, "shop", 100000),
                    ],
                )
                .root("shop-oct"),
            )
            .expect("fund the shop");
        treasury
            .post(
                &spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -50000),
                        (AccountKind::TownBudget, "lab", 50000),
                    ],
                )
                .root("lab-oct"),
            )
            .expect("fund the lab");
        treasury
            .post(
                &spec(
                    EntryKind::BudgetMove,
                    &[
                        (AccountKind::TownBudget, "lab", -20000),
                        (AccountKind::TownBudget, "shop", 20000),
                    ],
                )
                .root("move-1"),
            )
            .expect("a budget move");
        // A town budget is limited: it may not go negative either.
        let refused = treasury
            .post(
                &spec(
                    EntryKind::BudgetMove,
                    &[
                        (AccountKind::TownBudget, "lab", -30001),
                        (AccountKind::TownBudget, "shop", 30001),
                    ],
                )
                .root("move-2"),
            )
            .expect_err("the lab has only 30000 left");
        assert!(
            matches!(refused, PostError::Overdrawn { .. }),
            "{refused:?}"
        );
        let balance = |owner: &str| {
            treasury
                .accounts()
                .iter()
                .find(|account| account.kind == "town_budget" && account.owner_ref == owner)
                .expect("a town budget")
                .balance
        };
        assert_eq!(balance("shop"), 120000);
        assert_eq!(balance("lab"), 30000);
    }
}

#[cfg(test)]
mod economy_tests {
    use super::*;

    fn treasury() -> Treasury {
        Treasury::new(EventBus::new())
    }

    #[test]
    fn a_snapshot_restores_the_same_wallets_budget_and_journal() {
        let original = treasury();
        original.fund_task("t1", 300).expect("fund");
        original
            .charge_spend(Some("t1"), 100, "real cost")
            .expect("spend");
        original.pay_agent("t1", 7, 20).expect("pay");
        original.fine(7, 5, "late").expect("fine");
        original.transfer(7, 8, 3, "share").expect("transfer");
        let json = original.snapshot();

        let restored = Treasury::empty(EventBus::new());
        restored
            .restore(&json)
            .expect("a snapshot of a ledger restores");
        assert_eq!(restored.wallets(), original.wallets());
        assert_eq!(restored.wallets(), vec![(7, 12), (8, 3)]);
        assert_eq!(
            restored.plaza_budget_balance(),
            original.plaza_budget_balance()
        );
        assert_eq!(restored.journal(JOURNAL_LIMIT).total, 6);
        assert_eq!(
            restored.journal(JOURNAL_LIMIT).total,
            original.journal(JOURNAL_LIMIT).total
        );
        assert_eq!(restored.snapshot(), json);
        assert!(restored.verify_journal().is_ok());
        assert_eq!(restored.view().totals.balance, 0);
        assert_eq!(restored.version(), 1);
    }

    #[test]
    fn a_task_and_a_wallet_read_back_their_money() {
        let ledger = treasury();
        ledger.fund_task("t1", 300).expect("fund");
        ledger
            .charge_spend(Some("t1"), 100, "real cost")
            .expect("spend");
        ledger.pay_agent("t1", 7, 20).expect("pay");
        ledger.transfer(7, 8, 5, "share").expect("transfer");
        ledger.refund_task_to_budget("t1").expect("refund");
        assert_eq!(
            ledger.task_money("t1"),
            TaskMoney {
                minted: 300,
                escrow: 0,
                spent: 100,
                paid: vec![(7, 20)],
                refunded: 180
            }
        );
        assert_eq!(
            ledger.wallet_money(7),
            WalletMoney {
                balance: 15,
                sent_today: 5,
                earned: 20
            }
        );
        assert_eq!(
            ledger.wallet_money(9),
            WalletMoney {
                balance: 0,
                sent_today: 0,
                earned: 0
            }
        );
    }

    #[test]
    fn a_tampered_snapshot_is_refused_and_changes_nothing() {
        let original = treasury();
        original.fund_task("t1", 300).expect("fund");
        let mut value: serde_json::Value =
            serde_json::from_str(&original.snapshot()).expect("the snapshot is JSON");
        // Shave one credit off the plaza's opening mint: its entry no longer balances.
        value["entries"][0]["postings"][0]["amount"] = serde_json::json!(-99_999);
        let fresh = Treasury::empty(EventBus::new());
        let refused = fresh
            .restore(&value.to_string())
            .expect_err("an unbalanced entry is refused");
        assert!(refused.contains("does not balance"), "{refused}");
        assert_eq!(fresh.accounts().len(), 1, "only the mint account");
        assert_eq!(fresh.version(), 0);
    }

    #[test]
    fn a_model_charge_debits_its_payer_and_credits_external_spend() {
        let treasury = treasury();
        treasury.fund_task("t1", 200).expect("fund");
        // With a task, the task's escrow pays and the entry names the chain.
        let with_task = treasury
            .charge_model(7, Some("t1"), 30, "model call")
            .expect("a model call on the task");
        assert_eq!(treasury.escrow_balance("t1"), 170);
        assert_eq!(treasury.wallet(7), 0);
        // Without a task, the agent's wallet pays. Earn 15 first: a spend of
        // 100 makes the chain's real cost, and a payment of 15 is inside the cap.
        treasury
            .charge_spend(Some("t1"), 100, "real cost")
            .expect("spend");
        treasury.pay_agent("t1", 7, 15).expect("pay");
        assert_eq!(treasury.wallet(7), 15);
        let without_task = treasury
            .charge_model(7, None, 10, "model call")
            .expect("a model call from the wallet");
        assert_eq!(treasury.wallet(7), 5);
        assert_eq!(treasury.escrow_balance("t1"), 55);
        // A wallet cannot go below zero, and a non-positive charge is refused.
        assert!(treasury.charge_model(7, None, 6, "too much").is_err());
        assert!(matches!(
            treasury.charge_model(7, None, 0, "nothing"),
            Err(PostError::NonPositive { amount: 0 })
        ));
        {
            let inner = treasury.lock();
            let kind_of = |id: u64| {
                inner
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| (entry.kind, entry.root_task.clone()))
            };
            assert_eq!(
                kind_of(with_task),
                Some((EntryKind::Model, Some("t1".to_string())))
            );
            assert_eq!(kind_of(without_task), Some((EntryKind::Model, None)));
            // The provider is credited by the two model calls and the spend: 30 + 100 + 10.
            let external = inner
                .find_account(AccountKind::ExternalSpend, "providers")
                .expect("the provider account")
                .id;
            assert_eq!(inner.balance(external), 140);
        }
        assert!(treasury.verify_journal().is_ok());
    }

    #[test]
    fn a_transfer_is_capped_per_sender_per_day() {
        let treasury = treasury();
        treasury.fund_task("t1", 6000).expect("fund");
        treasury
            .charge_spend(Some("t1"), 5000, "real cost")
            .expect("spend");
        treasury.pay_agent("t1", 1, 1000).expect("pay");
        assert!(matches!(
            treasury.transfer(1, 1, 10, ""),
            Err(PostError::SelfTransfer { cid: 1 })
        ));
        assert!(matches!(
            treasury.transfer(1, 2, 0, ""),
            Err(PostError::NonPositive { amount: 0 })
        ));
        treasury.transfer(1, 2, 600, "first").expect("first leg");
        let capped = treasury
            .transfer(1, 2, 500, "over")
            .expect_err("over the cap");
        assert!(
            matches!(
                capped,
                PostError::TransferCap {
                    cid: 1,
                    sent_today: 600,
                    amount: 500,
                    cap: 1000
                }
            ),
            "{capped:?}"
        );
        treasury
            .transfer(1, 2, 400, "to the cap")
            .expect("exactly the cap");
        assert_eq!(treasury.wallet(1), 0);
        assert_eq!(treasury.wallet(2), 1000);
        // The allowance is per sender: agent 2 has sent nothing today.
        treasury
            .transfer(2, 1, 1000, "back")
            .expect("a fresh sender");
        assert!(treasury.verify_journal().is_ok());
    }

    #[test]
    fn a_fine_is_refused_below_zero() {
        let treasury = treasury();
        assert!(matches!(
            treasury.fine(3, 10, "no earnings"),
            Err(PostError::Overdrawn { .. })
        ));
        treasury.fund_task("t1", 200).expect("fund");
        treasury
            .charge_spend(Some("t1"), 100, "real cost")
            .expect("spend");
        treasury.pay_agent("t1", 3, 20).expect("pay");
        assert!(matches!(
            treasury.fine(3, 21, "too much"),
            Err(PostError::Overdrawn { .. })
        ));
        assert_eq!(treasury.wallet(3), 20);
        treasury
            .fine(3, 20, "all of it")
            .expect("fine the whole wallet");
        assert_eq!(treasury.wallet(3), 0);
        assert_eq!(
            treasury.plaza_budget_balance(),
            i128::from(PLAZA_BUDGET) - 200
        );
        assert!(treasury.verify_journal().is_ok());
    }
}

#[cfg(test)]
mod paging_tests {
    use super::*;

    /// An empty journal: `new` would post the plaza's starting mint as entry 1 at
    /// the current time, which the ids and dates these tests set do not expect.
    fn treasury() -> Treasury {
        Treasury::empty(EventBus::new())
    }

    fn query(limit: usize) -> JournalQuery {
        JournalQuery {
            before: None,
            limit,
            kind: None,
            wallet: None,
            task: None,
        }
    }

    fn ids(page: &JournalPage) -> Vec<u64> {
        page.entries.iter().map(|entry| entry.id).collect()
    }

    /// Seven entries: the plaza's mint (1), funds for t1, t2 and t3 (2, 3, 5), a
    /// spend on t1 (4), a second plaza mint (6) and a spend on t2 (7).
    fn seven(treasury: &Treasury) {
        treasury.mint_plaza_budget(100).expect("mint");
        treasury.fund_task("t1", 5).expect("fund t1");
        treasury.fund_task("t2", 5).expect("fund t2");
        treasury
            .charge_spend(Some("t1"), 1, "model")
            .expect("spend t1");
        treasury.fund_task("t3", 5).expect("fund t3");
        treasury.mint_plaza_budget(3).expect("mint");
        treasury
            .charge_spend(Some("t2"), 2, "model")
            .expect("spend t2");
    }

    /// Wallets: the plaza's mint (1), t1 funded 200 and spent 100 (so up to 20 can
    /// be paid), cid 1 paid 10 (4) and cid 2 paid 5 (5), cid 1 sends 4 to cid 3 (6),
    /// and cid 2 is fined 1 (7).
    fn wallets(treasury: &Treasury) {
        treasury.mint_plaza_budget(200).expect("mint");
        treasury.fund_task("t1", 200).expect("fund");
        treasury
            .charge_spend(Some("t1"), 100, "model")
            .expect("spend");
        treasury.pay_agent("t1", 1, 10).expect("pay cid 1");
        treasury.pay_agent("t1", 2, 5).expect("pay cid 2");
        treasury.transfer(1, 3, 4, "tip").expect("transfer");
        treasury.fine(2, 1, "late").expect("fine");
    }

    /// The journal with its entries dated: the plaza's mint at 9 000 (before the
    /// ranges the tests use), a fund of 40 at 10 500, a spend of 10 at 10 600, a
    /// payment of 2 to cid 2 at 12 100, a refund of 28 at 13 200 and a fine of 1
    /// on cid 2 at 13 500.
    fn dated(treasury: &Treasury) {
        treasury.mint_plaza_budget(100).expect("mint");
        treasury.fund_task("t1", 40).expect("fund");
        treasury
            .charge_spend(Some("t1"), 10, "model")
            .expect("spend");
        treasury.pay_agent("t1", 2, 2).expect("pay");
        treasury.refund_task_to_budget("t1").expect("refund");
        treasury.fine(2, 1, "late").expect("fine");
        let times = [9_000, 10_500, 10_600, 12_100, 13_200, 13_500];
        let mut inner = treasury.lock();
        for (entry, at) in inner.entries.iter_mut().zip(times) {
            entry.created_at = at;
        }
    }

    #[test]
    fn pages_are_disjoint_consecutive_and_newest_first() {
        let treasury = treasury();
        seven(&treasury);
        let first = treasury.journal_page(&query(3));
        assert_eq!(ids(&first), vec![7, 6, 5]);
        assert_eq!(first.total, 7);
        assert_eq!(first.next_before, Some(5));
        assert_eq!(first.entries[0].kind, "spend");
        assert_eq!(first.entries[0].postings.len(), 2);

        let second = treasury.journal_page(&JournalQuery {
            before: first.next_before,
            ..query(3)
        });
        assert_eq!(ids(&second), vec![4, 3, 2]);
        assert_eq!(second.next_before, Some(2));

        let last = treasury.journal_page(&JournalQuery {
            before: second.next_before,
            ..query(3)
        });
        assert_eq!(ids(&last), vec![1]);
        assert_eq!(last.next_before, None);
        assert_eq!(last.total, 7);

        // A page that fits holds everything, with no cursor.
        let whole = treasury.journal_page(&query(JOURNAL_LIMIT));
        assert_eq!(ids(&whole), vec![7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(whole.next_before, None);
        // The limit clamps to 1..=JOURNAL_LIMIT.
        assert_eq!(ids(&treasury.journal_page(&query(0))), vec![7]);
        assert_eq!(treasury.journal_page(&query(10_000)).entries.len(), 7);
        // Each entry reads exactly as the journal endpoint reads it, in reverse order.
        let chronological =
            serde_json::to_value(treasury.journal(JOURNAL_LIMIT).entries).expect("json");
        let newest_first = serde_json::to_value(whole.entries).expect("json");
        assert_eq!(
            newest_first,
            serde_json::Value::Array(
                chronological
                    .as_array()
                    .expect("array")
                    .iter()
                    .rev()
                    .cloned()
                    .collect()
            )
        );
    }

    #[test]
    fn kind_and_task_filters_narrow_pages_and_total() {
        let treasury = treasury();
        seven(&treasury);
        let subcontracts = treasury.journal_page(&JournalQuery {
            kind: Some("subcontract".into()),
            ..query(2)
        });
        assert_eq!(ids(&subcontracts), vec![5, 3]);
        assert_eq!(subcontracts.total, 3);
        assert_eq!(subcontracts.next_before, Some(3));
        let rest = treasury.journal_page(&JournalQuery {
            kind: Some("subcontract".into()),
            before: subcontracts.next_before,
            ..query(2)
        });
        assert_eq!(ids(&rest), vec![2]);
        assert_eq!(rest.total, 3);
        assert_eq!(rest.next_before, None);

        let task = treasury.journal_page(&JournalQuery {
            task: Some("t1".into()),
            ..query(JOURNAL_LIMIT)
        });
        assert_eq!(ids(&task), vec![4, 2]);
        assert_eq!(task.total, 2);

        let both = treasury.journal_page(&JournalQuery {
            kind: Some("spend".into()),
            task: Some("t2".into()),
            ..query(JOURNAL_LIMIT)
        });
        assert_eq!(ids(&both), vec![7]);

        // A kind no entry has, or a task with no entries, matches nothing.
        let fees = treasury.journal_page(&JournalQuery {
            kind: Some("fee".into()),
            ..query(JOURNAL_LIMIT)
        });
        assert_eq!(
            (fees.total, fees.entries.len(), fees.next_before),
            (0, 0, None)
        );
        let nobody = treasury.journal_page(&JournalQuery {
            task: Some("nobody".into()),
            ..query(JOURNAL_LIMIT)
        });
        assert_eq!(nobody.total, 0);
    }

    #[test]
    fn the_wallet_filter_matches_postings_to_or_from_the_wallet() {
        let treasury = treasury();
        wallets(&treasury);
        let with_wallet = |wallet: &str| JournalQuery {
            wallet: Some(wallet.into()),
            ..query(JOURNAL_LIMIT)
        };
        // Cid 1: its payment (4) and its transfer out (6).
        assert_eq!(ids(&treasury.journal_page(&with_wallet("1"))), vec![6, 4]);
        // Cid 2: its payment (5) and its fine (7).
        assert_eq!(ids(&treasury.journal_page(&with_wallet("2"))), vec![7, 5]);
        // Cid 3 is only the receiver of the transfer.
        assert_eq!(ids(&treasury.journal_page(&with_wallet("3"))), vec![6]);
        // No wallet, or a value that is not a cid, matches nothing.
        assert_eq!(treasury.journal_page(&with_wallet("9")).total, 0);
        assert_eq!(treasury.journal_page(&with_wallet("one")).total, 0);
        let payments = treasury.journal_page(&JournalQuery {
            wallet: Some("1".into()),
            kind: Some("payment".into()),
            ..query(JOURNAL_LIMIT)
        });
        assert_eq!(ids(&payments), vec![4]);
    }

    #[test]
    fn top_wallets_ranks_by_balance_with_earned_and_spent() {
        let treasury = treasury();
        wallets(&treasury);
        let top = treasury.top_wallets(2);
        assert_eq!(
            top,
            vec![
                WalletRank {
                    cid: 1,
                    balance: 6,
                    earned: 10,
                    spent: 4
                },
                WalletRank {
                    cid: 2,
                    balance: 4,
                    earned: 5,
                    spent: 1
                },
            ]
        );
        // Ties break by cid, and a wallet that has only received is ranked too.
        let all = treasury.top_wallets(10);
        assert_eq!(
            all.iter().map(|rank| rank.cid).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            all[2],
            WalletRank {
                cid: 3,
                balance: 4,
                earned: 0,
                spent: 0
            }
        );
    }

    #[test]
    fn series_buckets_the_range_and_carries_the_balances() {
        // Every bucket is listed in order, and each carries the balances it closed with.
        {
            let treasury = treasury();
            dated(&treasury);
            let series = treasury.series(10_000, 14_000, 1_000).expect("a series");
            let spans: Vec<(u64, u64)> = series
                .points
                .iter()
                .map(|point| (point.at_ms, point.end_ms))
                .collect();
            assert_eq!(
                spans,
                vec![
                    (10_000, 11_000),
                    (11_000, 12_000),
                    (12_000, 13_000),
                    (13_000, 14_000)
                ]
            );

            // The fund and the spend.
            let first = &series.points[0];
            assert_eq!((first.entries, first.escrowed, first.spent), (2, 40, 10));
            assert_eq!(
                (
                    first.plaza_budget_end,
                    first.circulating_end,
                    first.treasury_end
                ),
                (60, 30, 0)
            );
            assert_eq!(first.by_kind["subcontract"], 40);
            assert_eq!(first.by_kind["fine"], 0);
            assert_eq!(first.by_kind.len(), EntryKind::ALL.len());

            // An empty bucket, its balances carried from the one before.
            let empty = &series.points[1];
            assert_eq!(
                (empty.entries, empty.spent, empty.escrowed, empty.paid),
                (0, 0, 0, 0)
            );
            assert_eq!((empty.plaza_budget_end, empty.circulating_end), (60, 30));

            // The payment.
            let paid = &series.points[2];
            assert_eq!((paid.entries, paid.paid), (1, 2));
            assert_eq!((paid.plaza_budget_end, paid.circulating_end), (60, 30));

            // The refund and the fine.
            let last = &series.points[3];
            assert_eq!((last.entries, last.refunded, last.fined), (2, 28, 1));
            assert_eq!(
                (
                    last.plaza_budget_end,
                    last.circulating_end,
                    last.treasury_end
                ),
                (88, 1, 1)
            );
            // The last bucket's balances are the live ones, since every entry is in the range.
            assert_eq!(last.plaza_budget_end, treasury.plaza_budget_balance());
            assert_eq!(
                last.circulating_end,
                treasury.wallet(2) + treasury.escrow_balance("t1")
            );
        }

        // The range clips the ends: what came before it counts toward the opening balance,
        // and a payment past the end lands in no bucket.
        {
            let treasury = treasury();
            dated(&treasury);
            // Buckets [10 500, 11 000) and [11 000, 12 000): the first starts at `from`,
            // so the fund at 10 500 counts, and the mint at 9 000 is in the opening balance.
            let series = treasury.series(10_500, 12_000, 1_000).expect("a series");
            let spans: Vec<(u64, u64)> = series
                .points
                .iter()
                .map(|point| (point.at_ms, point.end_ms))
                .collect();
            assert_eq!(spans, vec![(10_500, 11_000), (11_000, 12_000)]);
            assert_eq!(series.points[0].escrowed, 40);
            assert_eq!(series.points[0].spent, 10);
            assert_eq!(series.points[0].plaza_budget_end, 60);
            assert_eq!(series.points[1].circulating_end, 30);
            // The payment (12 100) is past `to`, so it is in no bucket.
            assert_eq!(
                series.points.iter().map(|point| point.paid).sum::<i128>(),
                0
            );
        }
    }

    #[test]
    fn series_refuses_empty_ranges_zero_buckets_and_too_many_points() {
        let treasury = treasury();
        assert!(treasury.series(5, 5, 1).is_err());
        assert!(treasury.series(6, 5, 1).is_err());
        assert!(treasury.series(0, 10, 0).is_err());
        assert!(treasury.series(0, 2_000, 1).is_ok());
        assert!(treasury.series(0, 2_001, 1).is_err());
        // An empty journal reads as zeros.
        let empty = treasury.series(0, 3, 1).expect("a series");
        assert_eq!(empty.points.len(), 3);
        assert!(
            empty
                .points
                .iter()
                .all(|point| point.entries == 0 && point.plaza_budget_end == 0)
        );
    }
}
