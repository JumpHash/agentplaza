//! The plaza's domain state (apz-plaza) and its REST routes: tasks, treasury,
//! agora, board, conversations, council (motions) and contacts.
//!
//! OWNER: slice "plaza-api". The handlers port noded's former ones — see
//! `git show 82d6286:crates/noded/src/lib.rs` (fn list_tasks … fn withdraw_task).

use std::sync::Arc;

use apz_http::{Request, Response, Router};
use apz_plaza::agora::{self, Agora};
use apz_plaza::council::{self, Council, DecideRequest, ProposeRequest, VoteRequest};
use apz_plaza::events::EventBus;
use apz_plaza::social::{self, ContactRequest, Presence, PostRequest, ReadRequest, Social};
use apz_plaza::tasks::{
    resolve_budget, ClaimError, ClaimRequest, CreateTask, FinishRequest, TaskLedger, TasksView,
};
use apz_plaza::treasury::{self, Treasury};
use apz_plaza::ConnectedAgent;
use apz_proto::wire::WorkKind;
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::agents::{self, Status};
use crate::scheduler::Origin;
use crate::state::App;

/// The smallest and largest guest CIDs the plaza accepts in a path segment.
/// Mirrors `vmm::spawner::{MIN_GUEST_CID, MAX_GUEST_CID}`; apid does not link vmm.
const MIN_GUEST_CID: u64 = 3;
const MAX_GUEST_CID: u64 = u32::MAX as u64 - 1;

pub struct Plaza {
    pub events: Arc<EventBus>,
    pub tasks: TaskLedger,
    pub treasury: Arc<Treasury>,
    pub agora: Agora,
    pub social: Social,
    pub council: Council,
    /// Whether the treasury came back from `treasury.json`.
    pub restored: bool,
}

impl Plaza {
    /// A plaza whose treasury is restored from `snapshot` (a `treasury.json`) when
    /// one is given and holds to the ledger's invariants. The plaza budget is seeded
    /// only when nothing was restored.
    pub fn new(snapshot: Option<&str>) -> Self {
        let events = EventBus::new();
        let restored = snapshot.and_then(|json| {
            let treasury = Treasury::empty(Arc::clone(&events));
            match treasury.restore(json) {
                Ok(()) => Some(treasury),
                Err(err) => {
                    eprintln!("apid: treasury.json refused, starting a fresh treasury: {err}");
                    None
                }
            }
        });
        let was_restored = restored.is_some();
        let treasury = Arc::new(restored.unwrap_or_else(|| Treasury::new(Arc::clone(&events))));
        let tasks = TaskLedger::with_treasury(Arc::clone(&events), Arc::clone(&treasury));
        Self {
            agora: Agora::new(Arc::clone(&events)),
            social: Social::new(Arc::clone(&events)),
            council: Council::new(Arc::clone(&events)),
            tasks,
            treasury,
            events,
            restored: was_restored,
        }
    }
}

/// `/v1/tasks…`, `/v1/treasury…`, `/v1/agora`, `/v1/board…`,
/// `/v1/conversations…`, `/v1/motions…`, `/v1/agents/{cid}/contacts`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/tasks", |app, _, _| list_tasks(app))
        .post("/v1/tasks", |app, req, _| create_task(app, &req.body))
        .get("/v1/tasks/{id}", |app, _, p| get_task(app, p.get("id")))
        .delete("/v1/tasks/{id}", |app, _, p| withdraw_task(app, p.get("id")))
        .post("/v1/tasks/{id}/claim", |app, req, p| claim_task(app, p.get("id"), &req.body))
        .post("/v1/tasks/{id}/finish", |app, req, p| finish_task(app, p.get("id"), &req.body))
        .post("/v1/tasks/{id}/bid", |app, req, p| bid_task(app, p.get("id"), &req.body))
        .post("/v1/tasks/{id}/award", |app, req, p| award_task(app, p.get("id"), &req.body))
        .get("/v1/treasury", |app, _, _| treasury_view(app))
        .get("/v1/treasury/journal", |app, req, _| treasury_journal(app, req))
        .get("/v1/treasury/series", |app, req, _| treasury_series(app, req))
        .post("/v1/treasury/mint", |app, req, _| treasury_mint(app, &req.body))
        .post("/v1/treasury/fine", |app, req, _| treasury_fine(app, &req.body))
        .get("/v1/treasury/wallets", |app, _, _| treasury_wallets(app))
        .get("/v1/agora", |app, _, _| agora_view(app))
        .get("/v1/board", |app, _, _| board_view(app))
        .post("/v1/board", |app, req, _| board_post(app, &req.body))
        .post("/v1/board/{id}/comments", |app, req, p| board_comment(app, p.get("id"), &req.body))
        .post("/v1/board/read", |app, req, _| board_read(app, &req.body))
        .get("/v1/conversations", |app, req, _| conversations_view(app, req))
        .get("/v1/conversations/{a}/{b}", |app, req, p| {
            conversation_thread(app, p.get("a"), p.get("b"), req)
        })
        .get("/v1/motions", |app, _, _| motions_view(app))
        .post("/v1/motions", |app, req, _| motion_propose(app, &req.body))
        .get("/v1/motions/{id}", |app, _, p| motion_get(app, p.get("id")))
        .post("/v1/motions/{id}/vote", |app, req, p| motion_vote(app, p.get("id"), &req.body))
        .post("/v1/motions/{id}/decide", |app, req, p| motion_decide(app, p.get("id"), &req.body))
        .get("/v1/agents/{cid}/contacts", |app, _, p| agent_contacts(app, p.get("cid")))
        .post("/v1/agents/{cid}/contacts", |app, req, p| {
            agent_contact_add(app, p.get("cid"), &req.body)
        })
}

/// Parses a JSON body into `T`, or answers 400 naming the request kind.
fn parse<T: DeserializeOwned>(body: &[u8], what: &str) -> Result<T, Response> {
    serde_json::from_slice(body)
        .map_err(|err| Response::error(400, &format!("invalid {what} request: {err}")))
}

/// A guest CID from a path segment: a number within the plaza's bounds.
fn guest_cid(segment: &str) -> Result<u64, Response> {
    let cid: u64 = segment
        .parse()
        .map_err(|_| Response::error(400, &format!("{segment:?} is not a guest CID")))?;
    if (MIN_GUEST_CID..=MAX_GUEST_CID).contains(&cid) {
        Ok(cid)
    } else {
        Err(Response::error(
            400,
            &format!("guest CID {cid} is outside {MIN_GUEST_CID}..={MAX_GUEST_CID}"),
        ))
    }
}

/// The connected agent for a guest CID. Unknown is a 404; a known agent that
/// is offline is the ledger's own `AgentNotConnected` (a 409), so an
/// unreachable agent is refused before any task is touched.
fn connected(app: &App, cid: u64) -> Result<ConnectedAgent, Response> {
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(404, &format!("no agent with guest CID {cid}")));
    };
    if agent.status == Status::Offline {
        let refused = ClaimError::AgentNotConnected { cid };
        return Err(Response::error(refused.status(), &refused.to_string()));
    }
    Ok(agents::connected_agent(&agent))
}

/// The prompt an assigned task is worked from: its goal, and its acceptance
/// line when one is stated.
fn task_prompt(goal: &str, acceptance: Option<&str>) -> String {
    match acceptance {
        Some(acceptance) if !acceptance.trim().is_empty() => {
            format!("{goal}\n\nAcceptance: {acceptance}")
        }
        _ => goal.to_string(),
    }
}

/// `POST /v1/tasks` may name an `assignee`: the guest CID the task is claimed
/// for and queued to. Absent, the task is created open, as before.
#[derive(Debug, Deserialize)]
struct Assignee {
    #[serde(default)]
    assignee: Option<u64>,
}

fn list_tasks(app: &App) -> Response {
    let tasks = app.plaza.tasks.list();
    let view = TasksView {
        tasks: tasks.iter().map(|task| task.view()).collect(),
    };
    Response::json(200, &view)
}

/// `GET /v1/treasury`: balances (derived from the journal), totals and v0's
/// note on real spend.
fn treasury_view(app: &App) -> Response {
    let mut view = serde_json::to_value(app.plaza.treasury.view()).unwrap_or(serde_json::Value::Null);
    if let Some(fields) = view.as_object_mut() {
        fields.insert("plaza_budget".to_string(), serde_json::json!(app.plaza.treasury.plaza_budget_balance()));
    }
    Response::json(200, &view)
}

/// The longest `task` a journal filter may name, in bytes.
const JOURNAL_TASK_MAX: usize = 256;

/// The span a treasury series covers when the request names no `from`: 30 days.
const SERIES_SPAN_MS: u64 = 30 * 86_400_000;

/// `GET /v1/treasury/journal?before=&limit=&kind=&wallet=&task=`: the double-entry
/// journal a page at a time, newest first. `before` is an entry id (the page is
/// the entries below it), `limit` is `1..=256` (default 50), `kind` an entry
/// kind, `wallet` an agent's cid (the entries posting to its wallet) and `task` a
/// task id. The body is `{entries, total, limit, truncated, next_before}`:
/// `total` counts the entries the filters match, `truncated` says older matches
/// remain, and `next_before` is the `before` for the next page.
fn treasury_journal(app: &App, req: &Request) -> Response {
    let query = match journal_query(req) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let limit = query.limit;
    let page = app.plaza.treasury.journal_page(&query);
    let truncated = page.next_before.is_some();
    Response::json(
        200,
        &serde_json::json!({
            "entries": page.entries,
            "total": page.total,
            "limit": limit,
            "truncated": truncated,
            "next_before": page.next_before,
        }),
    )
}

/// The paging and filters a journal read asks for. An empty `kind`, `wallet` or
/// `task` names no filter.
fn journal_query(req: &Request) -> Result<treasury::JournalQuery, Response> {
    let limit = match req.query_param("limit") {
        None => treasury::JOURNAL_PAGE,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if (1..=treasury::JOURNAL_LIMIT).contains(&n) => n,
            _ => {
                return Err(Response::error(
                    400,
                    &format!("limit must be 1..={}, not {raw:?}", treasury::JOURNAL_LIMIT),
                ))
            }
        },
    };
    let before = match req.query_param("before") {
        None => None,
        Some(raw) => match raw.parse::<u64>() {
            Ok(id) => Some(id),
            Err(_) => return Err(Response::error(400, &format!("before must name an entry id, not {raw:?}"))),
        },
    };
    let kind = match req.query_param("kind").map(percent_decode) {
        Some(kind) if !kind.is_empty() => {
            if !treasury::EntryKind::ALL.iter().any(|known| known.as_str() == kind) {
                let kinds: Vec<&str> = treasury::EntryKind::ALL.iter().map(|known| known.as_str()).collect();
                return Err(Response::error(400, &format!("kind must be one of {}, not {kind:?}", kinds.join(", "))));
            }
            Some(kind)
        }
        _ => None,
    };
    let wallet = match req.query_param("wallet") {
        Some(raw) if !raw.is_empty() => match raw.parse::<u64>() {
            Ok(cid) => Some(cid.to_string()),
            Err(_) => return Err(Response::error(400, &format!("wallet must name an agent cid, not {raw:?}"))),
        },
        _ => None,
    };
    let task = match req.query_param("task").map(percent_decode) {
        Some(task) if task.len() > JOURNAL_TASK_MAX => {
            return Err(Response::error(400, &format!("task is longer than {JOURNAL_TASK_MAX} bytes")))
        }
        Some(task) if !task.is_empty() => Some(task),
        _ => None,
    };
    Ok(treasury::JournalQuery { before, limit, kind, wallet, task })
}

/// `GET /v1/treasury/series?from=&to=&bucket=hour|day`: the treasury over time,
/// one point per bucket of `[from, to)`, in milliseconds since the Unix epoch.
/// `to` defaults to now, `from` to 30 days before `to`, and `bucket` to `day`
/// (hours and days are UTC). A range of more than 2000 buckets is refused.
fn treasury_series(app: &App, req: &Request) -> Response {
    const HOUR_MS: u64 = 3_600_000;
    const DAY_MS: u64 = 86_400_000;
    let bucket_ms = match req.query_param("bucket").unwrap_or("day") {
        "hour" => HOUR_MS,
        "day" => DAY_MS,
        other => return Response::error(400, &format!("bucket must be hour or day, not {other:?}")),
    };
    let to = match instant_param(req, "to") {
        Ok(to) => to.unwrap_or_else(apz_plaza::now_ms),
        Err(response) => return response,
    };
    let from = match instant_param(req, "from") {
        Ok(from) => from.unwrap_or_else(|| to.saturating_sub(SERIES_SPAN_MS)),
        Err(response) => return response,
    };
    match app.plaza.treasury.series(from, to, bucket_ms) {
        Ok(series) => Response::json(200, &series),
        Err(reason) => Response::error(400, &reason),
    }
}

/// A query parameter naming an instant, in milliseconds since the Unix epoch.
fn instant_param(req: &Request, name: &str) -> Result<Option<u64>, Response> {
    match req.query_param(name) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| Response::error(400, &format!("{name} must be milliseconds since the epoch, not {raw:?}"))),
    }
}

/// `POST /v1/treasury/mint` `{amount}`: mints credits into the plaza's budget.
fn treasury_mint(app: &App, body: &[u8]) -> Response {
    #[derive(serde::Deserialize)]
    struct Mint {
        amount: i64,
    }
    let request: Mint = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match app.plaza.treasury.mint_plaza_budget(request.amount) {
        Ok(entry) => Response::json(
            200,
            &serde_json::json!({ "entry": entry, "plaza_budget": app.plaza.treasury.plaza_budget_balance() }),
        ),
        Err(e) => Response::error(e.status(), &format!("{e:?}")),
    }
}

/// `POST /v1/treasury/fine` `{cid, amount, memo}`: fines an agent's wallet into the plaza's treasury.
fn treasury_fine(app: &App, body: &[u8]) -> Response {
    #[derive(serde::Deserialize)]
    struct Fine {
        cid: u64,
        amount: i64,
        memo: String,
    }
    let request: Fine = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match app.plaza.treasury.fine(request.cid, request.amount, &request.memo) {
        Ok(entry) => Response::json(200, &serde_json::json!({ "entry": entry })),
        Err(e) => Response::error(e.status(), &format!("{e:?}")),
    }
}

/// `GET /v1/treasury/wallets`: every agent wallet as `{cid, name, balance}`.
fn treasury_wallets(app: &App) -> Response {
    let rows: Vec<serde_json::Value> = app
        .plaza
        .treasury
        .wallets()
        .into_iter()
        .map(|(cid, balance)| {
            let name = app.agents.get(cid).map_or_else(String::new, |a| a.name);
            serde_json::json!({ "cid": cid, "name": name, "balance": balance })
        })
        .collect();
    Response::json(200, &rows)
}

/// The request with its budget resolved: the default when none is named, and a
/// 400 (naming the minimum) when the named budget is below it.
fn with_budget(mut request: CreateTask) -> Result<CreateTask, Response> {
    let budget = resolve_budget(request.budget).map_err(|refusal| Response::error(400, &refusal))?;
    request.budget = Some(budget);
    Ok(request)
}

/// `POST /v1/tasks`: creates a task in `open`. With an `assignee`, the
/// assignee is checked first (so a refusal leaves the ledger untouched), the
/// task is claimed for it, and the assignee's work queue gets the task. An
/// absent `budget` gets `DEFAULT_TASK_BUDGET`; one under `MIN_TASK_BUDGET` is a 400.
pub(crate) fn create_task(app: &App, body: &[u8]) -> Response {
    let request: CreateTask = match parse(body, "create").and_then(with_budget) {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let assignee = match parse::<Assignee>(body, "create") {
        Ok(assignee) => assignee.assignee,
        Err(refused) => return refused,
    };
    let assignee = match assignee {
        Some(cid) => match connected(app, cid) {
            Ok(agent) => Some((cid, agent)),
            Err(refused) => return refused,
        },
        None => None,
    };
    let task = match app.plaza.tasks.create(&request) {
        Ok(task) => task,
        Err(err) => return Response::error(err.status(), &err.to_string()),
    };
    if let Some((cid, agent)) = assignee {
        if let Err(refused) = task.claim(&agent) {
            let refused = ClaimError::Refused(refused);
            return Response::error(refused.status(), &refused.to_string());
        }
        let id = task.id().to_string();
        let prompt = task_prompt(task.goal(), task.acceptance());
        let origin = Origin {
            task: Some(id.clone()),
            ..Default::default()
        };
        app.work.enqueue(app, cid, WorkKind::Task, format!("task-{id}"), prompt, origin);
    }
    Response::json(201, &task.view())
}

/// `GET /v1/tasks/{id}`: one task.
fn get_task(app: &App, id: &str) -> Response {
    match app.plaza.tasks.find(id) {
        Some(task) => Response::json(200, &task.view()),
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// `POST /v1/tasks/{id}/claim`: binds an open task to a connected agent. The
/// register check is made here; the ledger refuses the claim itself when the
/// task is not open.
fn claim_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: ClaimRequest = match parse(body, "claim") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let agent = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match task.claim(&agent) {
        Ok(()) => Response::json(200, &task.view()),
        Err(refused) => {
            let refused = ClaimError::Refused(refused);
            Response::error(refused.status(), &refused.to_string())
        }
    }
}

/// `POST /v1/tasks/{id}/finish`: ends a claimed task as `done` or `failed`,
/// refunding the escrow it still holds.
fn finish_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: FinishRequest = match parse(body, "finish") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    if let Err(reason) = request.checked() {
        return Response::error(400, &reason);
    }
    match task.finish(request.outcome, request.note) {
        Ok(()) => Response::json(200, &task.view()),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `DELETE /v1/tasks/{id}`: withdraws an open task.
fn withdraw_task(app: &App, id: &str) -> Response {
    match app.plaza.tasks.find(id) {
        Some(task) => match task.withdraw() {
            Ok(()) => Response::json(200, &task.view()),
            Err(refused) => Response::error(refused.status(), &refused.to_string()),
        },
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// The `POST /v1/tasks/{id}/bid` body: who bids, and what they ask.
#[derive(Debug, Deserialize)]
struct BidRequest {
    /// The guest CID of the agent making the bid.
    agent_cid: u64,
    /// The price the agent asks, in credits.
    price: u64,
}

/// The `POST /v1/tasks/{id}/award` body. No price: the agora awards at the
/// bidder's own bid.
#[derive(Debug, Deserialize)]
struct AwardRequest {
    /// The guest CID of the bidder the call is awarded to.
    agent_cid: u64,
    /// The guest CID of the employer. Present and connected, the award also
    /// posts the hiring note and links the contacts; absent, it binds the task
    /// alone.
    #[serde(default)]
    awarder_cid: Option<u64>,
}

/// `POST /v1/tasks/{id}/bid`: a connected agent bids on a roled open task's
/// call. The agora decides whether the bid stands.
fn bid_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: BidRequest = match parse(body, "bid") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let agent = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match app.plaza.agora.bid(&task, &agent, request.price) {
        Ok(bid) => Response::json(200, &bid),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `POST /v1/tasks/{id}/award`: awards a roled task's call to one of its
/// bidders and binds the task through the ledger's own claim.
fn award_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: AwardRequest = match parse(body, "award") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let agent = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    // Checked before the award, so a request the daemon can see is not
    // servable moves nothing.
    let awarder = match request.awarder_cid {
        Some(cid) => match connected(app, cid) {
            Ok(awarder) => Some(awarder),
            Err(refused) => return refused,
        },
        None => None,
    };
    match app.plaza.agora.award(&task, &agent) {
        Ok(()) => {
            // The award is made; the hiring note and contact links are
            // best-effort and report themselves in the log, never in this answer.
            if let Some(awarder) = awarder {
                let seed = app.plaza.social.seed_award(&task, &awarder, &agent);
                if let Ok(note) = &seed.note {
                    eprintln!("apid: award on {id} posted the hiring note {}", note.id);
                }
                if seed.linked {
                    eprintln!("apid: award on {id} linked guest CIDs {} and {}", awarder.cid, agent.cid);
                }
            }
            Response::json(200, &task.view())
        }
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `GET /v1/agora`: the calls the roled tasks offer, each with its bids and
/// the award that closed it.
fn agora_view(app: &App) -> Response {
    let tasks = app.plaza.tasks.list();
    Response::json(200, &app.plaza.agora.calls(&tasks, agora::AGORA_LIMIT))
}

/// `GET /v1/board`: the board, with presence taken from the roster.
fn board_view(app: &App) -> Response {
    let connected: Vec<u64> = app
        .agents
        .list()
        .iter()
        .filter(|agent| agent.status != Status::Offline)
        .map(|agent| agent.cid)
        .collect();
    Response::json(200, &app.plaza.social.board(&connected, social::BOARD_LIMIT))
}

/// `POST /v1/board`: a connected agent posts. The author is always an agent;
/// the driver defaults to the caller.
fn board_post(app: &App, body: &[u8]) -> Response {
    let request: PostRequest = match parse(body, "post") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let post = match request.checked() {
        Ok(post) => post,
        Err(reason) => return Response::error(400, &reason),
    };
    if let Some(task) = &post.task
        && app.plaza.tasks.find(task).is_none()
    {
        return Response::error(400, &format!("no task with id {task}"));
    }
    let author = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match app.plaza.social.post(&author, post) {
        Ok(post) => Response::json(201, &post),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// `POST /v1/board/{id}/comments` `{text}`: the owner comments on a post. The
/// post's author is told through its work queue, unless the owner wrote it.
fn board_comment(app: &App, post: &str, body: &[u8]) -> Response {
    #[derive(Deserialize)]
    struct CommentRequest {
        text: String,
    }
    let request: CommentRequest = match parse(body, "comment") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    match app.plaza.social.comment(post, None, "owner", &request.text) {
        Ok(comment) => {
            notify_post_author(app, post, None, "the owner", &request.text);
            Response::json(201, &comment)
        }
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// Tells a post's author that someone commented on it, through the author's
/// work queue. The author is not told of its own comments.
pub(crate) fn notify_post_author(app: &App, post: &str, commenter: Option<u64>, who: &str, text: &str) {
    let Some(found) = app.plaza.social.board(&[], 200).posts.into_iter().find(|p| p.id == post) else {
        return;
    };
    let (author_cid, subject) = (found.agent_cid, found.subject);
    if Some(author_cid) == commenter {
        return;
    }
    app.work.enqueue(
        app,
        author_cid,
        WorkKind::Message,
        format!("board-{post}"),
        format!(
            "[plaza board] {who} commented on your post {post} \"{subject}\": {text}\nIf it needs an answer, reply on the thread with plaza_board_comment."
        ),
        Origin::default(),
    );
}

/// `POST /v1/board/read`: a connected agent marks the board read up to a point.
fn board_read(app: &App, body: &[u8]) -> Response {
    let request: ReadRequest = match parse(body, "read") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let reader = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match app.plaza.social.read(&reader, &request.up_to) {
        Ok(read) => Response::json(200, &read),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// `GET /v1/conversations`: the pairs that have spoken, and one page of their
/// exchanges, newest first (`?limit=`, `?before=`, `?q=`).
fn conversations_view(app: &App, req: &Request) -> Response {
    match conversation_query(app, req) {
        Ok(query) => Response::json(200, &app.plaza.social.conversations(&query)),
        Err(refused) => refused,
    }
}

/// `GET /v1/conversations/{a}/{b}`: one page of the thread between two guest
/// CIDs, oldest first within the page (`?limit=`, `?before=`, `?q=`).
fn conversation_thread(app: &App, a: &str, b: &str, req: &Request) -> Response {
    let a = match guest_cid(a) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    let b = match guest_cid(b) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    match conversation_query(app, req) {
        Ok(query) => Response::json(200, &app.plaza.social.thread(a, b, &query)),
        Err(refused) => refused,
    }
}

/// The paging and search a conversations read asks for. `limit` is `1..=256`
/// (default [`social::CONVERSATIONS_PAGE`]); `before` is an exchange id (`c<n>`
/// or `<n>`), and the page is the exchanges older than it; `q` is a search over
/// the exchanges' text and roles, and over the exchanges of any agent whose
/// name or role contains it.
fn conversation_query(app: &App, req: &Request) -> Result<social::ConversationQuery, Response> {
    let limit = match req.query_param("limit") {
        None => social::CONVERSATIONS_PAGE,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n >= 1 => n.min(social::CONVERSATIONS_LIMIT),
            _ => {
                return Err(Response::error(
                    400,
                    &format!("limit must be 1..={}, not {raw:?}", social::CONVERSATIONS_LIMIT),
                ))
            }
        },
    };
    let before = match req.query_param("before") {
        None => None,
        Some(raw) => match raw.strip_prefix('c').unwrap_or(raw).parse::<u64>() {
            Ok(seq) => Some(seq),
            Err(_) => {
                return Err(Response::error(400, &format!("before must name an exchange (c<n>), not {raw:?}")))
            }
        },
    };
    let text = match req.query_param("q").map(percent_decode) {
        Some(q) if q.trim().is_empty() => None,
        Some(q) if q.trim().len() > social::MAX_QUERY => {
            return Err(Response::error(400, &format!("q is longer than {} bytes", social::MAX_QUERY)))
        }
        Some(q) => Some(q.trim().to_string()),
        None => None,
    };
    let cids = match &text {
        Some(needle) => {
            let needle = needle.to_lowercase();
            app.agents
                .list()
                .iter()
                .filter(|agent| {
                    agent.role.to_lowercase().contains(&needle)
                        || app.names.get(agent.cid).is_some_and(|name| name.to_lowercase().contains(&needle))
                })
                .map(|agent| agent.cid)
                .collect()
        }
        None => Vec::new(),
    };
    Ok(social::ConversationQuery { text, cids, before, limit })
}

/// A query-string value, percent-decoded (`+` is a space). A malformed escape
/// is kept as it is.
pub(crate) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'+' {
            out.push(b' ');
        } else if byte == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok());
            match hex {
                Some(value) => {
                    out.push(value);
                    index += 2;
                }
                None => out.push(byte),
            }
        } else {
            out.push(byte);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The seats a live agent holds right now: a seat is filled when some agent that
/// is not offline holds its role. The council's quorum is a majority of these.
pub fn filled_seats(agents: &[agents::Agent]) -> usize {
    council::SEATS
        .iter()
        .filter(|seat| {
            agents
                .iter()
                .any(|agent| agent.status != Status::Offline && agent.role == **seat)
        })
        .count()
}

/// The council, read against the seats the roster fills right now. Every council
/// route goes through this before it reads or acts on the council, so no count
/// waits for a seat that no live agent holds.
pub fn council_now(app: &App) -> &Council {
    app.plaza.council.set_filled(filled_seats(&app.agents.list()));
    &app.plaza.council
}

/// Applies the owner's retention to the conversations and the board: the count
/// caps, then the age limits. Runs on the scheduler's tick, so a change to the
/// retention takes effect within a tick. It also reads the seats for the council,
/// so the scheduler's motion offers (made before this runs on each tick) see a
/// roster at most one tick old.
pub fn retain(app: &App) {
    app.plaza.social.prune(app.settings.get().retention.policy());
    council_now(app);
}

/// `GET /v1/motions`: the council's motions.
fn motions_view(app: &App) -> Response {
    Response::json(200, &council_now(app).motions(council::MOTIONS_LIMIT))
}

/// `GET /v1/motions/{id}`: one motion.
fn motion_get(app: &App, id: &str) -> Response {
    match council_now(app).motion(id) {
        Some(motion) => Response::json(200, &motion),
        None => Response::error(404, &format!("no motion with id {id}")),
    }
}

/// `POST /v1/motions`: a seated agent proposes a motion. The minted total the
/// budget gate is measured against is read from the treasury at propose time.
fn motion_propose(app: &App, body: &[u8]) -> Response {
    let request: ProposeRequest = match parse(body, "motion") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let minted_total = app.plaza.treasury.minted_total();
    let proposal = match request.checked(minted_total) {
        Ok(proposal) => proposal,
        Err(reason) => return Response::error(400, &reason),
    };
    let proposer = match connected(app, request.proposer_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match council_now(app).propose(&proposer, proposal) {
        Ok(motion) => Response::json(201, &motion),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `POST /v1/motions/{id}/vote`: a seated agent votes.
fn motion_vote(app: &App, id: &str, body: &[u8]) -> Response {
    let request: VoteRequest = match parse(body, "vote") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let cast = match request.checked() {
        Ok(cast) => cast,
        Err(reason) => return Response::error(400, &reason),
    };
    let voter = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match council_now(app).vote(id, &voter, cast) {
        Ok(motion) => Response::json(200, &motion),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `POST /v1/motions/{id}/decide`: the decision on a motion.
fn motion_decide(app: &App, id: &str, body: &[u8]) -> Response {
    let request: DecideRequest = match parse(body, "decide") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let decision = match request.checked() {
        Ok(decision) => decision,
        Err(reason) => return Response::error(400, &reason),
    };
    match council_now(app).decide(id, decision) {
        Ok(motion) => Response::json(200, &motion),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `GET /v1/agents/{cid}/contacts`: an agent's contacts, with presence from the roster.
fn agent_contacts(app: &App, segment: &str) -> Response {
    let cid = match guest_cid(segment) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    let presence = match app.agents.get(cid) {
        Some(agent) if agent.status != Status::Offline => Presence::Live,
        _ => Presence::Absent,
    };
    match app.plaza.social.contacts(cid, presence) {
        Ok(contacts) => Response::json(200, &contacts),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// `POST /v1/agents/{cid}/contacts`: links a connected agent with a connected peer.
fn agent_contact_add(app: &App, segment: &str, body: &[u8]) -> Response {
    let cid = match guest_cid(segment) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    let request: ContactRequest = match parse(body, "contact") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let agent = match connected(app, cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    let peer = match connected(app, request.peer_cid) {
        Ok(peer) => peer,
        Err(refused) => return refused,
    };
    match app.plaza.social.add_contact(&agent, &peer) {
        Ok(contact) => Response::json(200, &contact),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A roster entry for `role` in `status`, as the register would hold it.
    fn seated(cid: u64, role: &str, status: Status) -> agents::Agent {
        agents::Agent {
            cid,
            name: String::new(),
            role: role.to_owned(),
            model_override: None,
            status,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: apz_proto::chat::Usage::default(),
            species: String::new(),
            kind: "llm".into(),
        }
    }

    #[test]
    fn a_seat_is_filled_by_a_live_agent_holding_its_role() {
        use apz_plaza::council::{FINANCE, INCIDENT, LEGAL_AGENTS_COUNSEL, MAYOR_OFFICE};
        assert_eq!(filled_seats(&[]), 0);
        // The legal-counsel seat has no live holder, so three of four are filled:
        // an offline holder does not fill its seat, a role with no seat fills none,
        // and a judge (excluded from every seat) fills none.
        let roster = [
            seated(1, FINANCE, Status::Idle),
            seated(2, INCIDENT, Status::Working),
            seated(3, MAYOR_OFFICE, Status::Offline),
            seated(4, "legal-gov-judge", Status::Idle),
            seated(5, "engineer", Status::Idle),
        ];
        assert_eq!(filled_seats(&roster), 2);
        let roster = [
            seated(1, FINANCE, Status::Idle),
            seated(2, INCIDENT, Status::Working),
            seated(3, MAYOR_OFFICE, Status::Idle),
            seated(4, LEGAL_AGENTS_COUNSEL, Status::Idle),
        ];
        assert_eq!(filled_seats(&roster), 4);
        // Two live agents holding one seat fill it once.
        let roster = [seated(6, FINANCE, Status::Idle), seated(7, FINANCE, Status::Working)];
        assert_eq!(filled_seats(&roster), 1);
    }

    #[test]
    fn prompt_carries_acceptance_only_when_stated() {
        assert_eq!(task_prompt("write the note", None), "write the note");
        assert_eq!(task_prompt("write the note", Some("  ")), "write the note");
        assert_eq!(
            task_prompt("write the note", Some("under 100 words")),
            "write the note\n\nAcceptance: under 100 words"
        );
    }

    #[test]
    fn assignee_is_optional_and_must_be_a_number() {
        assert_eq!(parse::<Assignee>(b"{}", "create").ok().and_then(|a| a.assignee), None);
        assert_eq!(
            parse::<Assignee>(br#"{"assignee": 7}"#, "create").ok().and_then(|a| a.assignee),
            Some(7)
        );
        assert!(parse::<Assignee>(br#"{"assignee": "seven"}"#, "create").is_err());
    }

    #[test]
    fn an_absent_task_budget_gets_the_default_and_a_low_one_is_a_400() {
        use apz_plaza::tasks::{DEFAULT_TASK_BUDGET, MIN_TASK_BUDGET};
        let resolve = |body: &str| {
            let request: CreateTask = parse(body.as_bytes(), "create").expect("a valid create body");
            with_budget(request)
        };
        assert_eq!(resolve(r#"{"goal": "work"}"#).ok().and_then(|r| r.budget), Some(DEFAULT_TASK_BUDGET as u64));
        assert_eq!(resolve(r#"{"goal": "work", "budget": 200}"#).ok().and_then(|r| r.budget), Some(MIN_TASK_BUDGET as u64));
        for body in [r#"{"goal": "work", "budget": 100}"#, r#"{"goal": "work", "budget": 0}"#] {
            let refusal = resolve(body).err().expect("below the minimum is refused");
            assert_eq!(refusal.status, 400);
            assert!(String::from_utf8_lossy(refusal.bytes()).contains("minimum of 200"));
        }
    }

    #[test]
    fn guest_cid_is_bounded_and_numeric() {
        assert_eq!(guest_cid("3").ok(), Some(3));
        assert!(guest_cid("2").is_err());
        assert!(guest_cid("abc").is_err());
    }
}

#[cfg(test)]
mod conversation_paging_tests {
    use super::percent_decode;

    #[test]
    fn a_search_value_decodes_escapes_and_plus_and_keeps_malformed_ones() {
        assert_eq!(percent_decode("budget+report"), "budget report");
        assert_eq!(percent_decode("a%20b%3Fc"), "a b?c");
        assert_eq!(percent_decode("100%"), "100%", "a trailing % is kept");
        assert_eq!(percent_decode("%zz"), "%zz", "a non-hex escape is kept");
    }
}
