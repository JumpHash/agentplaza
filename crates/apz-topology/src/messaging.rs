//! The messaging stage (PLAN 22.8): delivery, ordering, retries, dead letters and duplicates, judged from what a consumer
//! recorded. The stage produces `messages` keyed messages, runs them through a broker and a consumer, and records every
//! delivery. The broker here is the in-process stand-in (a queue with redelivery: a failed delivery is retried first, so
//! a key's order holds across retries; a message that fails `max_attempts` times is dead-lettered). Where the image has
//! a real broker it is named in the topology and the stand-in is replaced by it; none is installed for the stage.
//!
//! The faults are the broker's (duplicates, transient and poison failures of the consumer) and the consumer's own code
//! is what is judged: [`idempotent`] is the consumer a service should have (it applies each message once, by id); a
//! consumer that drops or repeats effects fails the judgments. The stage records the deliveries ([`Record`]) and the
//! effects the consumer applied, in order; the numbers the hub judges are [`names`].

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::compare::Metric;
use crate::stage::{UNREADABLE, not_run, value};

/// The most messages a stage may produce (a lab VM is small).
const MAX_MESSAGES: u32 = 1_000_000;

/// The messaging stage of a topology: the `[messaging]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessagingStage {
    /// The topic's name, a label.
    pub topic: String,
    pub messages: u32,
    /// The keys the messages are spread over (message `id` has key `id % keys` and sequence `id / keys`).
    pub keys: u32,
    /// The order the consumer must apply a key's messages in: `none`, or `per-key` (by sequence).
    #[serde(default)]
    pub ordering: Ordering,
    /// The broker redelivers every nth message once more (0: never).
    #[serde(default)]
    pub duplicate_every: u32,
    /// Every nth message fails its first delivery, and is retried (0: never).
    #[serde(default)]
    pub transient_every: u32,
    /// Every nth message fails every delivery, and is dead-lettered (0: never).
    #[serde(default)]
    pub poison_every: u32,
    /// The deliveries a message gets before it is dead-lettered.
    pub max_attempts: u32,
    /// The duplicate applications the consumer may make (0: none).
    #[serde(default)]
    pub max_duplicates: u32,
}

/// The order a consumer must keep: none, or per key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ordering {
    #[default]
    None,
    PerKey,
}

impl MessagingStage {
    /// What TOML cannot say: the topic is a label, the counts are bounded and consistent, and a retry needs a second
    /// attempt.
    pub fn check(&self) -> Result<(), String> {
        if !crate::spec::is_label(&self.topic) {
            return Err(format!("messaging topic {:?} is not a label", self.topic));
        }
        if self.messages == 0 || self.messages > MAX_MESSAGES {
            return Err(format!(
                "messaging messages {} is not 1 to {MAX_MESSAGES}",
                self.messages
            ));
        }
        if self.keys == 0 || self.keys > self.messages {
            return Err(format!(
                "messaging keys {} is not 1 to the {} messages",
                self.keys, self.messages
            ));
        }
        if self.max_attempts == 0 {
            return Err("messaging max_attempts is 0: a message is delivered at least once".into());
        }
        if self.transient_every > 0 && self.max_attempts < 2 {
            return Err(
                "messaging has transient failures but max_attempts 1: nothing is retried".into(),
            );
        }
        Ok(())
    }
}

/// One delivery of a message to the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub id: u64,
    pub key: u64,
    pub seq: u64,
    pub attempt: u32,
}

/// What the consumer recorded for one delivery: its status (`applied`, `acked` (no effect), `failed` (retried) or
/// `dead-letter`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    pub id: u64,
    pub key: u64,
    pub seq: u64,
    pub attempt: u32,
    pub status: &'static str,
}

/// What a run recorded: the numbers, the deliveries in order, and the effects the consumer applied in order (message
/// ids).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub metrics: Vec<Metric>,
    pub records: Vec<Record>,
    pub effects: Vec<u64>,
}

/// The consumer: handles a delivery, pushing the id of each effect it applies; an error is a failed delivery.
pub type Handler<'a> = dyn FnMut(&Delivery, &mut Vec<u64>) -> Result<(), String> + 'a;

/// The consumer a service should have: each message is applied once, by its id, and a redelivery is acknowledged without
/// a second effect.
pub fn idempotent() -> impl FnMut(&Delivery, &mut Vec<u64>) -> Result<(), String> {
    let mut seen: HashSet<u64> = HashSet::new();
    move |delivery, effects| {
        if seen.insert(delivery.id) {
            effects.push(delivery.id);
        }
        Ok(())
    }
}

/// The metric names the messaging stage records at a run.
pub fn names() -> Vec<String> {
    [
        "messaging.produced",
        "messaging.deliveries",
        "messaging.delivered",
        "messaging.lost",
        "messaging.duplicates",
        "messaging.out_of_order",
        "messaging.retries",
        "messaging.dead_letters",
        "messaging.poisoned",
        "messaging.max_attempts",
    ]
    .map(String::from)
    .to_vec()
}

fn poisoned(stage: &MessagingStage, id: u64) -> bool {
    stage.poison_every > 0 && id.is_multiple_of(u64::from(stage.poison_every))
}

fn transient(stage: &MessagingStage, id: u64) -> bool {
    stage.transient_every > 0
        && id.is_multiple_of(u64::from(stage.transient_every))
        && !poisoned(stage, id)
}

/// Produces the messages, runs them through the broker and `handler`, and records what happened.
pub fn run(stage: &MessagingStage, handler: &mut Handler<'_>) -> Run {
    let keys = u64::from(stage.keys);
    let mut queue: VecDeque<Delivery> = (1..=u64::from(stage.messages))
        .map(|id| Delivery {
            id,
            key: id % keys,
            seq: id / keys,
            attempt: 1,
        })
        .collect();
    let mut duplicated: HashSet<u64> = HashSet::new();
    let mut run = Run::default();
    let mut dead_letters = 0_u64;
    while let Some(delivery) = queue.pop_front() {
        // The broker redelivers a message once more, at the back of the queue.
        if stage.duplicate_every > 0
            && delivery.attempt == 1
            && delivery.id.is_multiple_of(u64::from(stage.duplicate_every))
            && !poisoned(stage, delivery.id)
            && duplicated.insert(delivery.id)
        {
            queue.push_back(delivery);
        }
        // An injected failure is a consumer that crashed before its effect: the handler does not see the message.
        let failed = poisoned(stage, delivery.id)
            || (transient(stage, delivery.id) && delivery.attempt == 1);
        let before = run.effects.len();
        let result = if failed {
            Err("injected failure".to_string())
        } else {
            handler(&delivery, &mut run.effects)
        };
        let status = match result {
            Ok(()) if run.effects.len() > before => "applied",
            Ok(()) => "acked",
            Err(_) if delivery.attempt < stage.max_attempts => {
                // A retry goes to the front, so the key's next message waits for it.
                queue.push_front(Delivery {
                    attempt: delivery.attempt + 1,
                    ..delivery
                });
                "failed"
            }
            Err(_) => {
                dead_letters += 1;
                "dead-letter"
            }
        };
        run.records.push(Record {
            id: delivery.id,
            key: delivery.key,
            seq: delivery.seq,
            attempt: delivery.attempt,
            status,
        });
    }
    run.metrics = measure(stage, &run, dead_letters);
    run
}

/// The numbers of a run, under [`names`].
fn measure(stage: &MessagingStage, run: &Run, dead_letters: u64) -> Vec<Metric> {
    let mut applied: HashMap<u64, u64> = HashMap::new();
    for id in &run.effects {
        *applied.entry(*id).or_default() += 1;
    }
    let total = u64::from(stage.messages);
    let poison_count = (1..=total).filter(|id| poisoned(stage, *id)).count() as u64;
    let lost = (1..=total)
        .filter(|id| !poisoned(stage, *id) && !applied.contains_key(id))
        .count() as u64;
    let duplicates: u64 = applied.values().map(|n| n.saturating_sub(1)).sum();
    // Per key, an effect whose sequence does not grow is out of order (a repeat counts too).
    let keys = u64::from(stage.keys);
    let mut last: HashMap<u64, u64> = HashMap::new();
    let mut out_of_order = 0_u64;
    for id in &run.effects {
        let (key, seq) = (id % keys, id / keys);
        if last.get(&key).is_some_and(|previous| seq <= *previous) {
            out_of_order += 1;
        }
        last.insert(key, seq);
    }
    let retries = run.records.iter().filter(|r| r.attempt > 1).count() as u64;
    let max_attempts = run.records.iter().map(|r| r.attempt).max().unwrap_or(0);
    let metric = |name: &str, value: u64| Metric {
        name: name.to_string(),
        value: value as f64,
    };
    vec![
        metric("messaging.produced", total),
        metric("messaging.deliveries", run.records.len() as u64),
        metric("messaging.delivered", applied.len() as u64),
        metric("messaging.lost", lost),
        metric("messaging.duplicates", duplicates),
        metric("messaging.out_of_order", out_of_order),
        metric("messaging.retries", retries),
        metric("messaging.dead_letters", dead_letters),
        metric("messaging.poisoned", poison_count),
        metric("messaging.max_attempts", u64::from(max_attempts)),
    ]
}

/// The judgments of a messaging stage over the values the run recorded: each item as `(id, passed, detail, measured)`.
pub fn judge(stage: &MessagingStage, values: &[Metric]) -> Vec<(String, bool, String, bool)> {
    let get = |name: &str| value(values, name);
    let mut out = Vec::new();
    match (get("messaging.produced"), get("messaging.delivered"), get("messaging.lost")) {
        (Some(produced), Some(delivered), Some(lost)) => out.push((
            "delivery".into(),
            lost == 0.0,
            format!(
                "{delivered:.0} of {produced:.0} messages were applied at least once ({lost:.0} lost)"
            ),
            true,
        )),
        _ => out.push(not_run("delivery", "messaging delivery")),
    }
    match get("messaging.duplicates") {
        Some(duplicates) => out.push((
            "duplicates".into(),
            duplicates != UNREADABLE && duplicates <= f64::from(stage.max_duplicates),
            format!(
                "{duplicates:.0} messages were applied more than once (at most {})",
                stage.max_duplicates
            ),
            true,
        )),
        None => out.push(not_run("duplicates", "messaging duplicates")),
    }
    match get("messaging.out_of_order") {
        Some(out_of_order) => {
            let declared = stage.ordering == Ordering::PerKey;
            let passed = !declared || out_of_order == 0.0;
            let detail = if declared {
                format!("{out_of_order:.0} effects were applied out of their key's order")
            } else {
                "no order is declared for this topic".into()
            };
            out.push(("ordering".into(), passed, detail, true));
        }
        None => out.push(not_run("ordering", "messaging ordering")),
    }
    match (get("messaging.dead_letters"), get("messaging.poisoned")) {
        (Some(dead), Some(poisoned)) => out.push((
            "dead-letters".into(),
            dead == poisoned,
            format!("{dead:.0} messages were dead-lettered; {poisoned:.0} failed every delivery"),
            true,
        )),
        _ => out.push(not_run("dead-letters", "messaging dead letters")),
    }
    match (get("messaging.max_attempts"), get("messaging.retries")) {
        (Some(attempts), Some(retries)) => out.push((
            "retries".into(),
            attempts <= f64::from(stage.max_attempts),
            format!(
                "{retries:.0} deliveries were retries; the most attempts for a message was {attempts:.0} (at most {})",
                stage.max_attempts
            ),
            true,
        )),
        _ => out.push(not_run("retries", "messaging retries")),
    }
    out
}
