//! The work engine's calls to apid: sessions (one step of a task, a project's planner, the main orchestrator, or a
//! helper), a task's seats, the budgets of a project and of a task with their ledgers, the cards waiting for the owner,
//! and the chats. The structs are the JSON the web console reads (`apps/web/src/api-work.ts`, whose shapes apid's
//! `sessions.rs`, `seats.rs`, `budgets.rs`, `cards.rs`, `chats.rs`, `transcript.rs` and `context_track.rs` serialize).
//! Every function takes the console's [`Client`]; a non-2xx answer is an `Err` with the hub's `error` text. Amounts are
//! micro-dollars for a `usd` budget and tokens for a `tokens` one. Every field a view can do without defaults, so one
//! missing field does not fail a whole list.

use serde::Deserialize;
use serde_json::{Value, json};

use super::client::{Client, enc};

/// `?a=1&b=x` from the parameters that have a value; empty when none does.
fn query(params: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .filter_map(|(key, value)| value.as_ref().map(|v| format!("{key}={}", enc(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

// ---- Sessions ---------------------------------------------------------------

/// What a session is for: one step of a task, a project's planner or the main orchestrator (long), or a helper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Step,
    Long,
    Helper,
}

/// Where a session is in its life. `Ended` and `Failed` are final.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    #[default]
    Queued,
    Placing,
    Forking,
    Running,
    Paused,
    Compacting,
    HandingOver,
    Ending,
    Ended,
    Failed,
}

/// Why a session is paused: its budget, its allowance, the disk, a card waiting for the owner, or its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PausedBy {
    Budget,
    Allowance,
    Disk,
    Card,
    Node,
}

/// What a session has used. Costs are micro-dollars.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    pub cost_micros: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
}

/// One session. A session has no name: its seat, task and project say what it is for.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Session {
    /// `s<seq>`.
    pub id: String,
    /// The project it works for (`plaza` for the main orchestrator).
    pub project: String,
    /// The task it works on; none for a long-lived session.
    pub task: Option<String>,
    /// The seat it fills (`builder`, `reviewer:1`, …).
    pub seat: Option<String>,
    pub kind: SessionKind,
    /// For a helper, the session that started it.
    pub parent: Option<String>,
    /// The seat template (role) it runs as.
    pub role: String,
    /// `harness` (the hub's own loop) or a vendor engine's id.
    pub engine: String,
    pub model: Option<String>,
    pub state: SessionState,
    pub paused_by: Option<PausedBy>,
    /// Its fork: the node it runs on, its VM's CID and noded's run.
    pub node: Option<String>,
    pub cid: Option<u64>,
    pub run: Option<String>,
    /// The work item in flight on its CID's lane.
    pub work: Option<u64>,
    pub usage: Usage,
    pub context_bytes: u64,
    pub compactions: u32,
    /// What it handed over to the next session of its seat when it ended.
    pub handover: Option<String>,
    /// Why it ended or failed, in a sentence.
    pub outcome: Option<String>,
    /// Which try of its seat this is (1, 2, …).
    pub attempt: u32,
    /// A reviewer's verdict, once given (true: passed).
    pub verdict: Option<bool>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

/// One entry of a session's transcript: `kind` names what happened, `data` holds its fields as the hub wrote them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TranscriptEntry {
    pub cid: u64,
    pub seq: u64,
    pub session: Option<String>,
    /// The work item it ran under (0 for none).
    pub work: u64,
    pub at_ms: u64,
    pub kind: String,
    pub data: Value,
}

/// One part of a session's context and the tokens it took.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContextPart {
    pub part: String,
    pub tokens: u64,
}

/// One tool's calls in a session's context.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContextTool {
    pub tool: String,
    pub calls: u64,
    pub errors: u64,
    pub result_tokens: u64,
}

/// A session's context: what its last model call sent (`used`) against its model's `window`, by part and by tool. A
/// session that has made no call yet has no CID and no parts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Context {
    pub session: String,
    pub cid: Option<u64>,
    pub work: Option<u64>,
    pub model: Option<String>,
    pub window: Option<u64>,
    pub used: u64,
    pub output: u64,
    /// `used` over `window`, at most 1; none without a window.
    pub fill: Option<f64>,
    pub parts: Vec<ContextPart>,
    pub tools: Vec<ContextTool>,
    pub calls: u64,
    pub compactions: u32,
    pub updated_ms: u64,
}

// ---- Seats ------------------------------------------------------------------

/// Where a seat is: waiting for an earlier step, ready for a session, filled by one, done, or failed for good.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatState {
    #[default]
    Waiting,
    Ready,
    Filled,
    Done,
    Failed,
}

/// Which models may fill a seat: any the project allows, or one no other seat of the task used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicy {
    #[default]
    Any,
    Differ,
}

/// One seat of a task: a role in its recipe, filled by one session at a time.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Seat {
    pub task: String,
    /// The seat's name in its task (`builder`, `reviewer`, …).
    pub name: String,
    pub recipe: String,
    pub step: String,
    pub role: String,
    pub model_policy: ModelPolicy,
    pub state: SeatState,
    /// The session filling it now.
    pub session: Option<String>,
    /// The model of the last session that filled it.
    pub last_model: Option<String>,
    /// Sessions started for it in this round.
    pub attempts: u32,
    /// The recipe's loop round it is in (1 at the start).
    pub round: u32,
    /// What the step that sent the task back said (the findings).
    pub returned_with: Option<String>,
    pub updated_ms: u64,
}

// ---- Budgets ----------------------------------------------------------------

/// A budget's unit: dollars (`usd`, micro-dollars on the wire) or tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetUnit {
    Usd,
    Tokens,
}

impl BudgetUnit {
    /// The unit as the hub writes it in a budget's body.
    pub fn key(self) -> &'static str {
        match self {
            BudgetUnit::Usd => "usd",
            BudgetUnit::Tokens => "tokens",
        }
    }
}

/// Whose budget it is: a project's or a task's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Project,
    Task,
}

impl OwnerKind {
    /// The kind as the hub writes it (`owner_kind` in a budget, and the key a frame names its owner by).
    pub fn key(self) -> &'static str {
        match self {
            OwnerKind::Project => "project",
            OwnerKind::Task => "task",
        }
    }
}

/// A budget as the hub reads it: its cap, what is spent, held and reserved, and the rest the hub adds. Before a budget
/// is set the hub answers only the owner with no unit, so every other field may be missing. A project's answer carries
/// its tasks' budgets in `tasks`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub owner: String,
    pub owner_kind: Option<OwnerKind>,
    /// For a task, its project; for a project, itself.
    pub project: Option<String>,
    pub unit: Option<BudgetUnit>,
    /// In the unit (micro-dollars or tokens); none: no cap.
    pub cap: Option<i64>,
    /// The share of the cap at which the owner is told (80 by default).
    pub warn_at_pct: Option<u32>,
    /// A project's cap per month for its long-lived sessions; none: none.
    pub monthly_cap: Option<i64>,
    /// The month (`yyyymm`) `month_spent` counts.
    pub month: Option<u32>,
    pub month_spent: i64,
    /// A project: what its running tasks may still spend. A task: what it reserved from its project.
    pub reserved: i64,
    /// A task: how much of its reservation it has drawn.
    pub drawn: i64,
    pub spent: i64,
    /// The worst case of the calls in flight.
    pub held: i64,
    /// The owner paused it, or it reached its cap and waits for a raise.
    pub paused: bool,
    pub warned: bool,
    /// What is left to spend (cap less spent, held and reserved); none when uncapped.
    pub left: Option<i64>,
    /// The share of the cap spent, in percent; none when uncapped.
    pub spent_pct: Option<f64>,
    /// A project's tasks' budgets.
    pub tasks: Vec<Budget>,
}

/// One ledger entry, newest first. `amount` is in the owner's unit; `micros` and `tokens` carry both.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LedgerEntry {
    pub owner: String,
    pub owner_kind: Option<OwnerKind>,
    /// What it records: `set`, `reserve`, `hold`, `charge`, `release` or `raise`.
    pub kind: String,
    pub amount: i64,
    pub micros: i64,
    pub tokens: i64,
    pub session: Option<String>,
    pub model: Option<String>,
    pub at_ms: u64,
}

// ---- Cards ------------------------------------------------------------------

/// One button of a card. `takes_amount`: the owner types an amount to go with it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CardChoice {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub takes_amount: bool,
}

/// A decision waiting for the owner, shown in the chat it belongs to.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Card {
    /// `c<seq>`.
    pub id: String,
    /// What it asks: `start`, `budget`, `allowance`, `tool_call`, `incident`, and the rest.
    pub kind: String,
    /// The chat it is shown in.
    pub chat: Option<String>,
    pub project: Option<String>,
    pub task: Option<String>,
    pub session: Option<String>,
    /// What it is about, for grouping cards of one subject.
    pub subject: String,
    pub title: String,
    pub body: Option<String>,
    pub choices: Vec<CardChoice>,
    /// `open`, `answered` or `withdrawn`.
    pub state: String,
    /// The choice the owner made.
    pub answer: Option<String>,
    /// The amount given with the answer (micro-dollars or tokens, as the card says).
    pub amount: Option<i64>,
    pub created_ms: u64,
    pub answered_ms: Option<u64>,
}

/// An answer's reply: the card, and what the answer decided done (empty when nothing else follows).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CardAnswer {
    pub card: Card,
    pub done: String,
}

// ---- Chats ------------------------------------------------------------------

/// A chat. The lists leave `messages` empty; a read has its last messages.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Chat {
    /// `main` for the orchestrator's chat.
    pub id: String,
    /// `main`, `project` or `task`.
    pub kind: String,
    pub project: Option<String>,
    pub task: Option<String>,
    pub title: String,
    pub archived: bool,
    pub next_seq: u64,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub messages: Vec<ChatMessage>,
}

/// One message. `author` is `owner`, `session` or `system`; `kind` is `message`, `question` (whose buttons are its
/// card's choices) or `notice`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatMessage {
    pub seq: u64,
    pub author: String,
    pub session: Option<String>,
    /// The seat the writing session filled.
    pub seat: Option<String>,
    pub kind: String,
    pub text: String,
    /// The card of a question.
    pub card: Option<String>,
    pub at_ms: u64,
}

// ---- The calls --------------------------------------------------------------

/// The sessions, newest first: a project's or a task's when given, only the live ones when `live`.
pub fn sessions(
    client: &Client,
    project: Option<&str>,
    task: Option<&str>,
    live: bool,
) -> Result<Vec<Session>, String> {
    let live = live.then(|| "true".to_string());
    client.get(&format!(
        "/v1/sessions{}",
        query(&[
            ("project", project.map(String::from)),
            ("task", task.map(String::from)),
            ("live", live),
        ])
    ))
}

pub fn session(client: &Client, id: &str) -> Result<Session, String> {
    client.get(&format!("/v1/sessions/{}", enc(id)))
}

/// A session's transcript from entry `after` on, at most `limit` entries (the hub's page is at most 1000).
pub fn transcript(
    client: &Client,
    id: &str,
    after: u64,
    limit: u32,
) -> Result<Vec<TranscriptEntry>, String> {
    client.get(&format!(
        "/v1/sessions/{}/transcript?after={after}&limit={limit}",
        enc(id)
    ))
}

/// The context of a session's last model call.
pub fn context(client: &Client, id: &str) -> Result<Context, String> {
    client.get(&format!("/v1/sessions/{}/context", enc(id)))
}

/// Stops a session: it ends with its handover, and its seat goes again.
pub fn stop_session(client: &Client, id: &str) -> Result<Session, String> {
    client.post(&format!("/v1/sessions/{}/stop", enc(id)), &json!({}))
}

pub fn seats(client: &Client, task: &str) -> Result<Vec<Seat>, String> {
    client.get(&format!("/v1/tasks/{}/seats", enc(task)))
}

pub fn project_budget(client: &Client, id: &str) -> Result<Budget, String> {
    client.get(&format!("/v1/projects/{}/budget", enc(id)))
}

pub fn set_project_budget(client: &Client, id: &str, body: &Value) -> Result<Budget, String> {
    client.put(&format!("/v1/projects/{}/budget", enc(id)), body)
}

pub fn project_ledger(client: &Client, id: &str, limit: u32) -> Result<Vec<LedgerEntry>, String> {
    client.get(&format!(
        "/v1/projects/{}/budget/ledger?limit={limit}",
        enc(id)
    ))
}

pub fn task_budget(client: &Client, id: &str) -> Result<Budget, String> {
    client.get(&format!("/v1/tasks/{}/budget", enc(id)))
}

pub fn set_task_budget(client: &Client, id: &str, body: &Value) -> Result<Budget, String> {
    client.put(&format!("/v1/tasks/{}/budget", enc(id)), body)
}

pub fn task_ledger(client: &Client, id: &str, limit: u32) -> Result<Vec<LedgerEntry>, String> {
    client.get(&format!(
        "/v1/tasks/{}/budget/ledger?limit={limit}",
        enc(id)
    ))
}

/// The cards, newest first: the open ones only when `open`, and those of one chat when `chat` is given.
pub fn cards(client: &Client, open: bool, chat: Option<&str>) -> Result<Vec<Card>, String> {
    let open = open.then(|| "true".to_string());
    client.get(&format!(
        "/v1/cards{}",
        query(&[("open", open), ("chat", chat.map(String::from))])
    ))
}

/// Answers a card with one of its choices; `amount` goes with a choice that takes one.
pub fn answer_card(
    client: &Client,
    id: &str,
    choice: &str,
    amount: Option<i64>,
) -> Result<CardAnswer, String> {
    let body = match amount {
        Some(amount) => json!({ "choice": choice, "amount": amount }),
        None => json!({ "choice": choice }),
    };
    client.post(&format!("/v1/cards/{}", enc(id)), &body)
}

/// The chats, newest activity first: a project's when `project` is given, the archived ones when `archived`.
pub fn chats(client: &Client, project: Option<&str>, archived: bool) -> Result<Vec<Chat>, String> {
    let archived = archived.then(|| "true".to_string());
    client.get(&format!(
        "/v1/chats{}",
        query(&[
            ("project", project.map(String::from)),
            ("archived", archived)
        ])
    ))
}

/// One chat, with its last `limit` messages.
pub fn chat(client: &Client, id: &str, limit: u32) -> Result<Chat, String> {
    client.get(&format!("/v1/chats/{}?limit={limit}", enc(id)))
}

/// A new chat with a project's planner.
pub fn open_chat(client: &Client, project: &str, title: Option<&str>) -> Result<Chat, String> {
    let body = match title {
        Some(title) => json!({ "project": project, "title": title }),
        None => json!({ "project": project }),
    };
    client.post("/v1/chats", &body)
}

/// The owner's message to a chat; it is delivered to the chat's session.
pub fn say(client: &Client, id: &str, text: &str) -> Result<ChatMessage, String> {
    client.post(
        &format!("/v1/chats/{}/messages", enc(id)),
        &json!({ "text": text }),
    )
}

/// Renames a chat, or archives it (or restores it): only the fields given change.
pub fn edit_chat(
    client: &Client,
    id: &str,
    title: Option<&str>,
    archived: Option<bool>,
) -> Result<Chat, String> {
    let mut body = json!({});
    if let Some(title) = title {
        body["title"] = json!(title);
    }
    if let Some(archived) = archived {
        body["archived"] = json!(archived);
    }
    client.patch(&format!("/v1/chats/{}", enc(id)), &body)
}

#[cfg(test)]
mod tests {
    use super::{BudgetUnit, OwnerKind, query};

    #[test]
    fn a_query_keeps_only_the_parameters_that_have_a_value() {
        assert_eq!(query(&[("project", None), ("live", None)]), "");
        assert_eq!(
            query(&[
                ("project", Some("pr 4".into())),
                ("task", None),
                ("live", Some("true".into())),
            ]),
            "?project=pr%204&live=true"
        );
    }

    #[test]
    fn the_hub_names_units_and_owners_in_lower_case() {
        assert_eq!(BudgetUnit::Usd.key(), "usd");
        assert_eq!(BudgetUnit::Tokens.key(), "tokens");
        assert_eq!(OwnerKind::Project.key(), "project");
        assert_eq!(OwnerKind::Task.key(), "task");
        let budget: super::Budget = serde_json::from_value(serde_json::json!({
            "owner": "pr4",
            "owner_kind": "project",
            "unit": "usd",
            "cap": 25_000_000,
            "spent": 1_500_000,
            "tasks": [{ "owner": "t9", "unit": null, "spent": 0 }],
        }))
        .unwrap();
        assert_eq!(budget.unit, Some(BudgetUnit::Usd));
        assert_eq!(budget.owner_kind, Some(OwnerKind::Project));
        assert_eq!(budget.tasks[0].unit, None);
    }
}
