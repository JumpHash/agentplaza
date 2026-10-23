//! Cards (PLAN 4.7, 26.2, 26.3; W63, W64): a decision waiting for the owner, shown in the chat it belongs to as a
//! message with its options as buttons. Only the decisions an earlier decision of the owner's says are his become
//! cards: starting a project's plan (W10), a budget, allowance or disk limit reached (W13–W15), an agent-built
//! connector or a tool that uses a secret (W21), promoting a tool to every project (W37), a tool call the project's
//! permission mode sends to him, a review ruling, an agent proposal (`approvals.rs`, `agent_proposals.rs`); everything
//! else is decided by the AIs and reported as a notice.
//!
//! The records live in memory; the hub's saver keeps them in `plaza.db` (tables `card`, `card_options`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// What a card asks (4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Start,
    Budget,
    Allowance,
    Disk,
    Connector,
    Tool,
    Engine,
    Recipe,
    Promote,
    Memory,
    Review,
    Land,
    Adopt,
    Yank,
    Seat,
    Release,
    Deploy,
    ToolCall,
    Incident,
    /// An agent proposal the owner approves or rejects (`agent_proposals.rs`).
    Proposal,
}

/// Where a card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Answered,
    /// What it asked no longer needs an answer (the task ended, the limit was raised elsewhere).
    Withdrawn,
}

/// One button of a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Choice {
    /// What the answer names (`raise_by`, `stop`, `approve`, …).
    pub id: String,
    pub label: String,
    /// One line on what choosing it does.
    pub detail: Option<String>,
    /// Whether choosing it needs an amount (a raise to an amount the owner types).
    pub takes_amount: bool,
}

/// One card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "card")]
pub struct Card {
    /// `c<seq>`.
    #[table(key)]
    pub id: String,
    pub kind: Kind,
    /// The chat it is shown in.
    #[table(index)]
    pub chat: Option<String>,
    pub project: Option<String>,
    pub task: Option<String>,
    pub session: Option<String>,
    /// What it is about, for grouping cards of one subject (a task's budget: one open card at a time).
    pub subject: String,
    pub title: String,
    /// What the owner needs to decide without opening anything else.
    #[table(content)]
    pub body: Option<String>,
    #[table(child)]
    pub choices: Vec<Choice>,
    pub state: State,
    /// The choice the owner made.
    pub answer: Option<String>,
    /// The amount given with the answer (micro-dollars or tokens, as the card says).
    pub amount: Option<u64>,
    pub created_ms: u64,
    pub answered_ms: Option<u64>,
}

/// What a new card says.
#[derive(Debug, Clone)]
pub struct NewCard {
    pub kind: Kind,
    pub chat: Option<String>,
    pub project: Option<String>,
    pub task: Option<String>,
    pub session: Option<String>,
    pub subject: String,
    pub title: String,
    pub body: Option<String>,
    pub choices: Vec<Choice>,
}

/// An answer refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotFound(String),
    NotOpen(String),
    NoSuchChoice(String),
    AmountNeeded(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotFound(id) => write!(f, "no card {id}"),
            Refused::NotOpen(id) => write!(f, "card {id} is already answered or withdrawn"),
            Refused::NoSuchChoice(c) => write!(f, "the card has no choice {c:?}"),
            Refused::AmountNeeded(c) => write!(f, "the choice {c:?} needs an amount"),
        }
    }
}

#[derive(Default)]
struct Inner {
    next: u64,
    cards: BTreeMap<u64, Card>,
}

/// Every card.
#[derive(Default)]
pub struct Cards {
    inner: Mutex<Inner>,
}

fn seq_of(id: &str) -> Option<u64> {
    id.strip_prefix('c')?.parse().ok()
}

impl Cards {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a card. When an open card of the same kind and subject exists, that one is returned instead: one question
    /// per subject waits at a time (a task at its cap asks once, however many calls it refused).
    pub fn open(&self, new: NewCard, now_ms: u64) -> (Card, bool) {
        let mut inner = self.lock();
        if let Some(open) = inner
            .cards
            .values()
            .find(|c| c.state == State::Open && c.kind == new.kind && c.subject == new.subject)
        {
            return (open.clone(), false);
        }
        inner.next += 1;
        let card = Card {
            id: format!("c{}", inner.next),
            kind: new.kind,
            chat: new.chat,
            project: new.project,
            task: new.task,
            session: new.session,
            subject: new.subject,
            title: new.title,
            body: new.body,
            choices: new.choices,
            state: State::Open,
            answer: None,
            amount: None,
            created_ms: now_ms,
            answered_ms: None,
        };
        let seq = inner.next;
        inner.cards.insert(seq, card.clone());
        (card, true)
    }

    pub fn get(&self, id: &str) -> Option<Card> {
        let seq = seq_of(id)?;
        self.lock().cards.get(&seq).cloned()
    }

    /// The cards, newest first; only open ones when `open_only`; of one chat when `chat` is given.
    pub fn list(&self, open_only: bool, chat: Option<&str>) -> Vec<Card> {
        self.lock()
            .cards
            .values()
            .rev()
            .filter(|c| !open_only || c.state == State::Open)
            .filter(|c| chat.is_none_or(|ch| c.chat.as_deref() == Some(ch)))
            .cloned()
            .collect()
    }

    /// Records the owner's answer.
    pub fn answer(
        &self,
        id: &str,
        choice: &str,
        amount: Option<u64>,
        now_ms: u64,
    ) -> Result<Card, Refused> {
        let seq = seq_of(id).ok_or_else(|| Refused::NotFound(id.to_string()))?;
        let mut inner = self.lock();
        let card = inner
            .cards
            .get_mut(&seq)
            .ok_or_else(|| Refused::NotFound(id.to_string()))?;
        if card.state != State::Open {
            return Err(Refused::NotOpen(id.to_string()));
        }
        let picked = card
            .choices
            .iter()
            .find(|c| c.id == choice)
            .ok_or_else(|| Refused::NoSuchChoice(choice.to_string()))?;
        if picked.takes_amount && amount.is_none() {
            return Err(Refused::AmountNeeded(choice.to_string()));
        }
        card.state = State::Answered;
        card.answer = Some(choice.to_string());
        card.amount = amount;
        card.answered_ms = Some(now_ms);
        Ok(card.clone())
    }

    /// Withdraws the open cards of `kind` about `subject` (what they asked was settled another way).
    pub fn withdraw(&self, kind: Kind, subject: &str, now_ms: u64) -> Vec<Card> {
        let mut inner = self.lock();
        let mut out = Vec::new();
        for card in inner.cards.values_mut() {
            if card.state == State::Open && card.kind == kind && card.subject == subject {
                card.state = State::Withdrawn;
                card.answered_ms = Some(now_ms);
                out.push(card.clone());
            }
        }
        out
    }

    /// The cards' rows, for the hub's store.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        apz_store::RowSet::list(def(), inner.cards.values())
    }

    /// Replaces the cards with what the store holds (at start).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Card> = apz_store::table::load_list(conn, def())?;
        let mut inner = Inner::default();
        for card in stored {
            let seq = seq_of(&card.id).unwrap_or(0);
            inner.next = inner.next.max(seq);
            inner.cards.insert(seq, card);
        }
        *self.lock() = inner;
        Ok(())
    }
}

/// The cards' tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Card)
}

/// The choices a budget card offers (10.3): raise by the planner's estimate, raise to an amount, stop, replan.
pub fn budget_choices(estimate: Option<u64>, unit: crate::budgets::Unit) -> Vec<Choice> {
    let mut choices = Vec::new();
    if let Some(estimate) = estimate {
        choices.push(Choice {
            id: "raise_by".into(),
            label: format!(
                "Raise by the estimate ({})",
                crate::work_loop::amount(estimate, unit)
            ),
            detail: Some("the planner's estimate of what finishing needs".into()),
            takes_amount: false,
        });
    }
    choices.extend([
        Choice {
            id: "raise_to".into(),
            label: "Raise to…".into(),
            detail: Some("a new cap you type".into()),
            takes_amount: true,
        },
        Choice {
            id: "stop".into(),
            label: "Stop the task".into(),
            detail: Some("its sessions end with a handover; nothing more is spent".into()),
            takes_amount: false,
        },
        Choice {
            id: "replan".into(),
            label: "Replan".into(),
            detail: Some("the planner splits or shrinks the task".into()),
            takes_amount: false,
        },
    ]);
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget_card(task: &str) -> NewCard {
        NewCard {
            kind: Kind::Budget,
            chat: Some(format!("chat-{task}")),
            project: Some("pr1".into()),
            task: Some(task.into()),
            session: Some("s1".into()),
            subject: format!("task:{task}"),
            title: format!("Task {task} reached its budget"),
            body: Some("Spent $4.00 of $4.00".into()),
            choices: budget_choices(Some(2_000_000), crate::budgets::Unit::Usd),
        }
    }

    #[test]
    fn one_open_card_per_subject_and_an_answer_needs_a_real_choice() {
        let cards = Cards::default();
        let (first, created) = cards.open(budget_card("t1"), 1);
        assert!(created);
        let (again, created) = cards.open(budget_card("t1"), 2);
        assert!(!created);
        assert_eq!(again.id, first.id, "a second refusal does not ask again");
        assert!(matches!(
            cards.answer(&first.id, "nope", None, 3),
            Err(Refused::NoSuchChoice(_))
        ));
        assert!(matches!(
            cards.answer(&first.id, "raise_to", None, 3),
            Err(Refused::AmountNeeded(_))
        ));
        let answered = cards
            .answer(&first.id, "raise_to", Some(9_000_000), 3)
            .unwrap();
        assert_eq!(
            (answered.state, answered.amount),
            (State::Answered, Some(9_000_000))
        );
        assert!(matches!(
            cards.answer(&first.id, "stop", None, 4),
            Err(Refused::NotOpen(_))
        ));
        let (next, created) = cards.open(budget_card("t1"), 5);
        assert!(
            created && next.id != first.id,
            "once answered, a new limit asks again"
        );
    }

    #[test]
    fn withdrawn_cards_leave_the_open_list_and_cards_round_trip() {
        let cards = Cards::default();
        cards.open(budget_card("t1"), 1);
        cards.open(budget_card("t2"), 2);
        assert_eq!(cards.withdraw(Kind::Budget, "task:t1", 3).len(), 1);
        assert_eq!(cards.list(true, None).len(), 1);
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &cards.rows().unwrap()).unwrap();
        let loaded = Cards::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.list(false, None), cards.list(false, None));
        assert_eq!(loaded.open(budget_card("t3"), 4).0.id, "c3");
    }
}
