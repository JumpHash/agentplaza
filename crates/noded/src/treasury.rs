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
//! * `external_spend` is opened lazily, by the first `spend` entry. v0 meters
//!   no real spend, so a running v0 daemon has no `external_spend` account
//!   until one is posted (a test posts one; the lifecycle does not).
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
//! `forbid_change` triggers; v0 has no database, so the guard is the type-level
//! absence of any mutator (entries are handed out as owned snapshots) plus
//! [`Treasury::verify_journal`], which re-checks that the ids are exactly
//! `1..=n` and every posting names a live account. A correction is a new
//! entry, never an edit.
//!
//! # What v0 does not do (stated, not faked)
//!
//! * **No metered real spend.** Nothing yet posts `spend`: the real cost of a
//!   task (model providers, tools, compute) is not metered, so a task that ends
//!   refunds its *whole* escrow. The API says so: the treasury view carries
//!   `spent: 0` and [`SPEND_NOTE`].
//! * **No subcontracts, payments, clawbacks, fines, fees or budget moves.**
//!   Their kinds, accounts and posting rules are all present and enforced, but
//!   nothing posts them at v0 — they belong to the Agora and sanction slices.
//! * **No credit line above zero.** `agent_earnings` may go negative down to
//!   `-credit_limit`, and the rule is enforced, but v0 has no reputation slice
//!   to set a limit, so every lazily opened account keeps the schema's default
//!   of 0 (an account's limit can be set through [`Treasury::open_account`]).
//! * **No database.** Accounts and the journal live in memory, like the task
//!   ledger. The schema's `treasury.balances` and `treasury.chain_margin`
//!   views are the derivations [`Treasury::view`] and [`Treasury::post`]'s
//!   margin check perform.
//!
//! The caps v0 chooses, and refuses past: [`MAX_ENTRIES`] and [`MAX_ACCOUNTS`]
//! (the journal and the accounts table are bounded, like the task ledger), and
//! [`JOURNAL_LIMIT`] for the journal endpoint's response.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use crate::events::{Event, EventBus};
use crate::registry::now_ms;

/// The `owner_ref` of the single `mint` account: the fixed name the schema's
/// tests give it.
pub const MINT_OWNER: &str = "mint";

/// The `posted_by` of every entry: the schema's `posted_by_service` allows only
/// service names, and every entry here is the treasury service's own.
pub const POSTED_BY: &str = "treasury-service";

/// The chain margin cap, as a percentage of the chain's real cost (0111). The
/// schema's `treasury.margin_cap_pct()` returns this value.
pub const MARGIN_CAP_PCT: i64 = 20;

/// v0's honest note on real spend: it meters none.
pub const SPEND_NOTE: &str = "v0 meters no real spend; a done task refunds its whole escrow";

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

/// The kinds of account, the schema's `treasury.account_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}

impl EntryKind {
    /// Every kind, in the schema's enum order.
    pub const ALL: [EntryKind; 9] = [
        EntryKind::Mint,
        EntryKind::Spend,
        EntryKind::Subcontract,
        EntryKind::Payment,
        EntryKind::Refund,
        EntryKind::Clawback,
        EntryKind::Fine,
        EntryKind::Fee,
        EntryKind::BudgetMove,
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
        }
    }

    /// Whether the entry must name a chain: the schema's
    /// `task_money_names_chain` exempts only `fine` and `fee`.
    fn names_a_chain(self) -> bool {
        !matches!(self, EntryKind::Fine | EntryKind::Fee)
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
    (EntryKind::Subcontract, AccountKind::TaskEscrow, Side::Credit),
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
            _ => 500,
        }
    }
}

impl std::fmt::Display for PostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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

/// One account: the schema's `treasury.accounts` row, without a balance (the
/// balance is derived from the postings).
#[derive(Debug)]
struct Account {
    id: u64,
    kind: AccountKind,
    owner_ref: String,
    credit_limit: i64,
    created_at: u64,
}

/// One posting as it is stored: which account, and the amount.
#[derive(Debug)]
struct Posting {
    account_id: u64,
    amount: i64,
}

/// One entry as it is stored: the schema's `treasury.entries` row together with
/// its postings.
#[derive(Debug)]
struct Entry {
    id: u64,
    kind: EntryKind,
    task_id: Option<String>,
    root_task: Option<String>,
    memo: String,
    posted_by: &'static str,
    created_at: u64,
    postings: Vec<Posting>,
}

/// The accounts and the journal, behind one lock.
#[derive(Debug)]
struct Inner {
    accounts: Vec<Account>,
    entries: Vec<Entry>,
    next_account: u64,
    next_entry: u64,
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
                if entry.kind == EntryKind::Spend
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
            if spec.kind == EntryKind::Spend
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

/// The treasury: the accounts, the append-only double-entry journal, and the
/// event bus every posted entry is published to.
#[derive(Debug)]
pub struct Treasury {
    events: Arc<EventBus>,
    inner: Mutex<Inner>,
}

impl Treasury {
    /// A treasury with the single `mint` account and an empty journal, which
    /// publishes every posted entry to `events`.
    pub fn new(events: Arc<EventBus>) -> Self {
        let now = now_ms();
        Self {
            events,
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
            }),
        }
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
        Ok(id)
    }

    /// Mints `amount` credits into `task_id`'s escrow: a `mint` entry, the
    /// schema's `Fund` (mint debit, task_escrow credit). `amount` must be
    /// positive; the ledger only calls this for a task whose budget is.
    pub fn mint_task_budget(&self, task_id: &str, amount: i64) -> Result<u64, PostError> {
        let spec = EntrySpec::new(
            EntryKind::Mint,
            vec![
                PostingSpec::new(AccountKind::Mint, MINT_OWNER, -amount),
                PostingSpec::new(AccountKind::TaskEscrow, task_id, amount),
            ],
        )
        .task(task_id)
        .root(task_id)
        .memo("task budget");
        self.post(&spec)
    }

    /// Refunds what `task_id`'s escrow still holds back to the mint: a `refund`
    /// entry, the schema's `Close` for a root task (task_escrow debit, mint
    /// credit). Returns the entry's id, or `None` when there is nothing to
    /// refund (no escrow, or a balance of zero) — in which case no entry is
    /// posted.
    ///
    /// The balance is read and the entry posted under one lock, so a concurrent
    /// post cannot make the refund overdraw.
    pub fn refund_task_escrow(&self, task_id: &str) -> Result<Option<u64>, PostError> {
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
                PostingSpec::new(AccountKind::Mint, MINT_OWNER, amount),
            ],
        )
        .task(task_id)
        .root(task_id)
        .memo("task budget refunded");
        self.append(&mut inner, &spec).map(Some)
    }

    /// `task_id`'s escrow balance, derived from its postings.
    pub fn escrow_balance(&self, task_id: &str) -> i128 {
        let inner = self.lock();
        inner
            .find_account(AccountKind::TaskEscrow, task_id)
            .map_or(0, |account| inner.balance(account.id))
    }

    /// Every account, in creation order, with its derived balance.
    pub fn accounts(&self) -> Vec<AccountView> {
        let inner = self.lock();
        account_views(&inner)
    }

    /// The treasury as the API reports it: the accounts with their balances,
    /// a totals line, and v0's honest note on real spend.
    pub fn view(&self) -> TreasuryView {
        let inner = self.lock();
        let accounts = account_views(&inner);
        let mut credits = 0i128;
        let mut debits = 0i128;
        for entry in &inner.entries {
            for posting in &entry.postings {
                if posting.amount > 0 {
                    credits += i128::from(posting.amount);
                } else {
                    debits += i128::from(-posting.amount);
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
            spent: 0,
            note: SPEND_NOTE,
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
            .map(|entry| {
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
            })
            .collect();
        JournalView {
            entries,
            total,
            limit,
            truncated: total > limit,
        }
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
            let account_id = match inner.find_account(posting.kind, &posting.owner).map(|a| a.id) {
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
        self.events.publish(Event::Treasury {
            entry: id,
            kind: spec.kind.as_str(),
            amount,
            task: spec.task_id.clone(),
        });
        Ok(id)
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

/// `GET /api/v0/treasury`'s body.
#[derive(Debug, Serialize)]
pub struct TreasuryView {
    /// The accounts, in creation order, each with its derived balance.
    pub accounts: Vec<AccountView>,
    /// The totals line.
    pub totals: TotalsView,
    /// Real money spent (0 at v0: nothing meters spend yet).
    pub spent: i128,
    /// The note on why `spent` is zero.
    pub note: &'static str,
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
            .post(&spec(
                EntryKind::Mint,
                &[
                    (AccountKind::Mint, MINT_OWNER, -amount),
                    (AccountKind::TaskEscrow, task, amount),
                ],
            )
            .task(task)
            .root(task))
            .expect("a mint inside the rules");
    }

    /// The schema's `treasury.posting_rules`, typed out from the INSERT in
    /// `docs/treasury-draft/schema.sql` — the same table [`POSTING_RULES`]
    /// encodes, listed here independently so a transcription error in the
    /// constant is caught.
    const SCHEMA_RULES: [(EntryKind, AccountKind, Side); 25] = [
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
        (EntryKind::Subcontract, AccountKind::TaskEscrow, Side::Credit),
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
    fn a_posting_outside_the_rules_is_refused_naming_the_rule() {
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
        assert_eq!(treasury.view().totals.entries, 0);
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
        assert_eq!(balance("mint", MINT_OWNER), -500);
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
    fn the_mint_and_refund_cycle_returns_the_escrow_to_the_mint() {
        let treasury = new_treasury();
        let mint = treasury.mint_task_budget("t1", 500).expect("a mint");
        assert_eq!(treasury.escrow_balance("t1"), 500);
        assert_eq!(treasury.view().totals.entries, 1);

        let refund = treasury
            .refund_task_escrow("t1")
            .expect("a refund")
            .expect("there was an escrow to refund");
        assert_eq!(refund, mint + 1);
        assert_eq!(treasury.escrow_balance("t1"), 0);

        let view = treasury.view();
        assert_eq!(view.totals.entries, 2);
        assert_eq!(view.spent, 0);
        assert_eq!(view.note, SPEND_NOTE);
        // Both entries balance; the mint is home and nothing is spent.
        assert_eq!(view.totals.credits, 1000);
        assert_eq!(view.totals.debits, 1000);
        let mint_account = view
            .accounts
            .iter()
            .find(|account| account.kind == "mint")
            .expect("the mint account");
        assert_eq!(mint_account.balance, 0);
        // A second refund has nothing to refund and posts nothing.
        assert_eq!(treasury.refund_task_escrow("t1").expect("no-op"), None);
        assert_eq!(treasury.view().totals.entries, 2);
        // A task that never had a budget has no escrow to refund.
        assert_eq!(treasury.refund_task_escrow("t9").expect("no-op"), None);
        assert!(treasury.verify_journal().is_ok());
    }

    #[test]
    fn an_escrow_cannot_overdraw() {
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
        assert!(
            refused.to_string().contains("below its limit"),
            "{refused}"
        );
        // The refusal changed nothing.
        assert_eq!(treasury.escrow_balance("t1"), 100);
        assert_eq!(treasury.view().totals.entries, 1);

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
    fn a_payment_respects_the_chain_margin_cap() {
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

    #[test]
    fn an_entry_must_balance_and_name_a_chain() {
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
        assert_eq!(treasury.view().totals.entries, 0);

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

    #[test]
    fn an_empty_or_degenerate_entry_is_refused() {
        let treasury = new_treasury();
        assert_eq!(
            treasury.post(&EntrySpec::new(EntryKind::Mint, vec![])),
            Err(PostError::Empty)
        );
        assert_eq!(
            treasury.post(
                &spec(
                    EntryKind::Mint,
                    &[(AccountKind::Mint, MINT_OWNER, 0), (AccountKind::TaskEscrow, "t1", 0)]
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
        assert_eq!(treasury.view().totals.entries, 0);
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
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(treasury.journal(JOURNAL_LIMIT).total, 2);

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
            after.as_array().expect("an array")[..2],
            before.as_array().expect("an array")[..]
        );
        assert_eq!(after.as_array().expect("an array").len(), 3);
        assert_eq!(treasury.journal(JOURNAL_LIMIT).entries[2].id, 3);
        assert!(treasury.verify_journal().is_ok());
    }

    #[test]
    fn accounts_are_opened_lazily_only_when_an_entry_needs_them() {
        let treasury = new_treasury();
        // Only the mint exists until something needs more.
        let kinds = |treasury: &Treasury| -> Vec<&'static str> {
            treasury
                .accounts()
                .iter()
                .map(|account| account.kind)
                .collect()
        };
        assert_eq!(kinds(&treasury), vec!["mint"]);

        fund(&treasury, "t1", 100);
        assert_eq!(kinds(&treasury), vec!["mint", "task_escrow"]);
        assert_eq!(treasury.accounts()[1].owner_ref, "t1");
        assert_eq!(treasury.accounts()[1].credit_limit, 0);

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
            vec!["mint", "task_escrow", "external_spend"]
        );
        // v0's lifecycle posts none of the rest, so they are still absent.
        for kind in ["treasury", "town_budget", "agent_earnings"] {
            assert!(!kinds(&treasury).contains(&kind), "{kind} appeared");
        }
    }

    #[test]
    fn the_caps_refuse_honestly() {
        let treasury = new_treasury();
        // The journal's cap: fill it with mint entries to a town budget, which
        // the rules allow and which touches no limited account.
        for _ in 0..MAX_ENTRIES {
            treasury
                .post(&spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -1),
                        (AccountKind::TownBudget, "shop", 1),
                    ],
                )
                .root("shop-oct"))
                .expect("a mint under the cap");
        }
        assert_eq!(treasury.view().totals.entries, MAX_ENTRIES);
        let refused = treasury
            .post(&spec(
                EntryKind::Mint,
                &[
                    (AccountKind::Mint, MINT_OWNER, -1),
                    (AccountKind::TownBudget, "shop", 1),
                ],
            )
            .root("shop-oct"))
            .expect_err("the journal is full");
        assert_eq!(
            refused,
            PostError::JournalFull { max: MAX_ENTRIES }
        );
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            format!("the treasury journal is full ({MAX_ENTRIES} entries)")
        );
        assert_eq!(treasury.view().totals.entries, MAX_ENTRIES);

        // The accounts cap: opening one past the cap is refused, and so is an
        // entry that would open one.
        let fresh = new_treasury();
        for index in 0..MAX_ACCOUNTS - 1 {
            fresh
                .open_account(AccountKind::TownBudget, &format!("t{index}"), 0)
                .expect("an account under the cap");
        }
        // The mint account plus MAX_ACCOUNTS - 1 town budgets is exactly the cap.
        assert_eq!(fresh.accounts().len(), MAX_ACCOUNTS);
        let refused = fresh
            .open_account(AccountKind::TownBudget, "one-too-many", 0)
            .expect_err("the accounts are full");
        assert_eq!(refused, PostError::AccountsFull { max: MAX_ACCOUNTS });
        assert_eq!(refused.status(), 409);
        let refused = fresh
            .post(&spec(
                EntryKind::Mint,
                &[
                    (AccountKind::Mint, MINT_OWNER, -1),
                    (AccountKind::TownBudget, "still-one-too-many", 1),
                ],
            )
            .root("shop-oct"))
            .expect_err("the accounts are full");
        assert_eq!(refused, PostError::AccountsFull { max: MAX_ACCOUNTS });
        assert_eq!(fresh.accounts().len(), MAX_ACCOUNTS);
        assert_eq!(fresh.view().totals.entries, 0);
    }

    #[test]
    fn the_journal_endpoint_is_bounded_and_ordered() {
        let treasury = new_treasury();
        for _ in 0..(JOURNAL_LIMIT + 10) {
            treasury
                .post(&spec(
                    EntryKind::Mint,
                    &[
                        (AccountKind::Mint, MINT_OWNER, -1),
                        (AccountKind::TownBudget, "shop", 1),
                    ],
                )
                .root("shop-oct"))
                .expect("a mint");
        }
        let journal = treasury.journal(JOURNAL_LIMIT);
        assert_eq!(journal.entries.len(), JOURNAL_LIMIT);
        assert_eq!(journal.total, JOURNAL_LIMIT + 10);
        assert_eq!(journal.limit, JOURNAL_LIMIT);
        assert!(journal.truncated);
        // Chronological: the newest entry is last, and the response holds the
        // newest window of the journal.
        assert_eq!(journal.entries[0].id, 11);
        assert_eq!(journal.entries[JOURNAL_LIMIT - 1].id, (JOURNAL_LIMIT + 10) as u64);

        // A short journal is not truncated, and is oldest-first.
        let small = new_treasury();
        small.mint_task_budget("t1", 5).expect("mint");
        let journal = small.journal(JOURNAL_LIMIT);
        assert!(!journal.truncated);
        assert_eq!(journal.total, 1);
        assert_eq!(journal.entries[0].kind, "mint");
        assert_eq!(journal.entries[0].task_id.as_deref(), Some("t1"));
        assert_eq!(journal.entries[0].amount, 5);
        assert_eq!(journal.entries[0].postings.len(), 2);
        assert_eq!(journal.entries[0].posted_by, POSTED_BY);
        assert_eq!(journal.entries[0].memo, "task budget");
        // Every entry is a balanced pair summing to zero.
        let sum: i64 = journal.entries[0]
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

        let mint = treasury.mint_task_budget("t1", 500).expect("a mint");
        let refund = treasury
            .refund_task_escrow("t1")
            .expect("a refund")
            .expect("something to refund");

        let mut frames = Vec::new();
        while let Ok(event) = receiver.recv_timeout(Duration::from_secs(1)) {
            frames.push(event.frame());
        }
        assert_eq!(
            frames,
            vec![
                format!(
                    "event: treasury\ndata: {{\"entry\":{mint},\"kind\":\"mint\",\"amount\":500,\"task\":\"t1\"}}\n\n"
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
    fn a_fine_never_reaches_escrow() {
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

    #[test]
    fn a_budget_move_between_towns_is_balanced_and_bounded() {
        let treasury = new_treasury();
        treasury
            .post(&spec(
                EntryKind::Mint,
                &[
                    (AccountKind::Mint, MINT_OWNER, -100000),
                    (AccountKind::TownBudget, "shop", 100000),
                ],
            )
            .root("shop-oct"))
            .expect("fund the shop");
        treasury
            .post(&spec(
                EntryKind::Mint,
                &[
                    (AccountKind::Mint, MINT_OWNER, -50000),
                    (AccountKind::TownBudget, "lab", 50000),
                ],
            )
            .root("lab-oct"))
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
        assert!(matches!(refused, PostError::Overdrawn { .. }), "{refused:?}");
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
