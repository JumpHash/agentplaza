//! Allowances (PLAN 10.4, 11.2; Engine 13.3): a subscription's windows as a usage provider reports them, and what they do
//! to the calls that use a subscription. A model the model policy marks `subscription` draws on the allowances of its
//! vendor. While one of its windows is used up and not yet reset, the call is not sent: the session waits, paused by
//! its allowance, and an `allowance` card tells the owner. At the reset the session runs again: [`tick`] (run by the
//! ticker every second) resumes it, and the next call it makes is sent.
//!
//! The readings come from the usage providers (`usage_providers.rs`): each provider's usage answer carries an
//! `allowances` list, one entry per window (see [`parse`]). The ticker reads every provider again every five minutes.
//! Every rule takes the time as an argument, so a test runs them on a fake clock.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cards::{self, Card, Choice, NewCard};
use crate::chats::{Author, MessageKind, Post};
use crate::sessions::{Filter, PausedBy, Session, State};
use crate::settings::ModelPolicy;
use crate::state::App;

/// How often the ticker resumes the sessions an allowance paused.
const TICK: Duration = Duration::from_secs(1);
/// How often the ticker reads every usage provider again.
const READ_EVERY_MS: u64 = 5 * 60_000;
/// The subject prefix of an allowance card: `allowance:<vendor>`.
const SUBJECT: &str = "allowance:";

/// One window of a subscription's allowance, as the last reading of its usage provider gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Allowance {
    /// The usage provider that reported it (its id).
    pub provider: String,
    /// The vendor the window is for (see [`vendor_of`]).
    pub vendor: String,
    /// The account or plan the provider names, else the provider's own name for the vendor.
    pub name: String,
    /// The window as the provider names it (`5h`, `weekly`, …).
    pub window: String,
    /// What is used, in the unit the provider counts in (requests, or tokens).
    pub used: u64,
    pub limit: u64,
    /// When the window resets, in epoch milliseconds; none when the provider does not say.
    pub resets_ms: Option<u64>,
    /// When the reading was taken, in epoch milliseconds.
    pub checked_ms: u64,
}

impl Allowance {
    /// Used up: at or past its limit.
    pub fn exhausted(&self) -> bool {
        self.limit > 0 && self.used >= self.limit
    }

    /// Whether the window blocks a call at `now`: used up, and not yet reset. A window with no reset time blocks until a
    /// later reading says otherwise.
    pub fn blocks(&self, now: u64) -> bool {
        self.exhausted() && self.resets_ms.is_none_or(|reset| reset > now)
    }
}

/// The vendor a provider's name stands for. CLIProxyAPI names its providers after the vendors' CLIs, and the model
/// policy names models `provider/model`; both map to one vendor here.
pub fn vendor_of(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => "anthropic".into(),
        "openai" | "codex" | "chatgpt" => "openai".into(),
        "google" | "gemini" | "gemini-cli" => "google".into(),
        other => other.to_string(),
    }
}

/// The windows in a usage answer. This is the one place the allowance field names are read. They are read from the
/// `usage` object that `usage_providers.rs` already reads the totals from (the answer itself when it has none), as its
/// `allowances` list. ASSUMED, not seen in CLIProxyAPI's answer: the list, and each entry's `provider` (or `vendor`),
/// `window`, `limit`, `used` (0 when absent), `account` (optional) and `resets_at` (epoch milliseconds, or an RFC 3339
/// UTC time). An entry with no provider, window or limit is skipped. The test fixture in `tests/subscriptions.rs`
/// answers this same shape.
pub fn parse(answer: &Value, checked_ms: u64) -> Vec<Allowance> {
    let usage = answer.get("usage").unwrap_or(answer);
    let Some(items) = usage.get("allowances").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let provider = item["provider"].as_str().or(item["vendor"].as_str())?;
            let window = item["window"].as_str()?.to_string();
            let limit = item["limit"].as_u64()?;
            Some(Allowance {
                provider: String::new(),
                vendor: vendor_of(provider),
                name: item["account"].as_str().unwrap_or(provider).to_string(),
                window,
                used: item["used"].as_u64().unwrap_or(0),
                limit,
                resets_ms: reset_of(&item["resets_at"]),
                checked_ms,
            })
        })
        .collect()
}

/// A reset time: epoch milliseconds, or an RFC 3339 time in UTC.
fn reset_of(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| rfc3339_ms(value.as_str()?))
}

/// An RFC 3339 UTC time (`2026-10-24T18:00:00Z`, a fraction of seconds allowed) in epoch milliseconds.
pub fn rfc3339_ms(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut t = time.split('.').next()?.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t.next()?.parse().ok()?;
    if t.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|s| s * 1_000)
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The window that blocks a call of `vendor` at `now`, if any.
pub fn blocking(allowances: &[Allowance], vendor: &str, now: u64) -> Option<Allowance> {
    allowances
        .iter()
        .find(|a| a.vendor == vendor && a.blocks(now))
        .cloned()
}

/// A model name without its effort (`anthropic/claude-x@low` → `anthropic/claude-x`).
fn bare(model: &str) -> &str {
    model.split('@').next().unwrap_or(model)
}

/// Whether the policy marks `model` as a subscription.
pub fn is_subscription(policy: &ModelPolicy, model: &str) -> bool {
    let model = bare(model);
    policy.subscription.iter().any(|m| bare(m) == model)
}

/// What the allowances hold in memory: the sessions an allowance paused, each with the vendor it waits on (a session
/// paused before a restart waits on no vendor and resumes once no allowance blocks at all), and the projects whose
/// owner chose the API-key route for their subscription calls.
#[derive(Default)]
pub struct Waits {
    waiting: Mutex<BTreeMap<String, String>>,
    api_key: Mutex<BTreeSet<String>>,
}

impl Waits {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, String>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn api_key(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.api_key.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An allowance card's subject: `allowance:<vendor>:<project>`, one card per vendor and project.
fn subject(vendor: &str, project: &str) -> String {
    format!("{SUBJECT}{vendor}:{project}")
}

/// The (vendor, project) of an allowance card's subject.
fn of_subject(subject: &str) -> Option<(&str, &str)> {
    subject.strip_prefix(SUBJECT)?.split_once(':')
}

/// The policy's fallback for a subscription model: its first worker model that is not a subscription. The API-key
/// route a project switches to goes there.
pub fn fallback(policy: &ModelPolicy) -> Option<String> {
    policy
        .worker
        .iter()
        .map(|m| bare(m).to_string())
        .find(|m| !is_subscription(policy, m))
}

/// The model a session's call really goes to. A subscription model of a project whose owner chose the API-key route
/// goes to the policy's fallback instead; any other call keeps the model it asked for. The gateway's engine calls are
/// not rerouted: their model is the engine's own.
pub fn route(app: &App, cid: u64, model: &str) -> String {
    let policy = app.settings.get().model_policy;
    if !is_subscription(&policy, model) {
        return model.to_string();
    }
    let Some(session) = app.sessions.by_cid(cid) else {
        return model.to_string();
    };
    if !app.allowance_waits.api_key().contains(&session.project) {
        return model.to_string();
    }
    fallback(&policy).unwrap_or_else(|| model.to_string())
}

/// Before a model call of agent `cid` is held or sent: a call of a subscription model waits while its vendor's
/// allowance is used up. Then the session waits too (paused by its allowance), and the owner is asked with a card, and
/// the `Err` is the harness's paused answer. A call that may go resumes a session its allowance paused. A model that is
/// not a subscription is not checked here.
pub fn gate(app: &App, cid: u64, model: &str, now: u64) -> Result<(), String> {
    if !is_subscription(&app.settings.get().model_policy, model) {
        return Ok(());
    }
    let vendor = vendor_of(model.split('/').next().unwrap_or(model));
    let session = app.sessions.by_cid(cid);
    let Some(window) = blocking(&app.usage_providers.allowances(), &vendor, now) else {
        if let Some(s) = &session
            && s.paused_by == Some(PausedBy::Allowance)
            && s.state == State::Paused
        {
            resume(app, &s.id, now);
        }
        return Ok(());
    };
    if let Some(s) = &session
        && s.state != State::Paused
    {
        let _ = app.sessions.pause(&s.id, PausedBy::Allowance, now);
        app.bus.publish("session", &app.sessions.get(&s.id));
        lock_waits(app).insert(s.id.clone(), vendor.clone());
    }
    ask(app, &vendor, &window, session.as_ref(), now);
    Err(format!(
        "{}{}",
        apz_proto::wire::MODEL_PAUSED,
        sentence(&vendor, &window, now)
    ))
}

fn lock_waits(app: &App) -> MutexGuard<'_, BTreeMap<String, String>> {
    app.allowance_waits.lock()
}

/// Resumes the sessions whose allowance has reset (a session waiting on no vendor resumes once no window blocks), and
/// withdraws the allowance cards whose vendor is no longer blocked. The ticker runs it every [`TICK`].
pub fn tick(app: &App, now: u64) {
    let allowances = app.usage_providers.allowances();
    let paused: Vec<Session> = app
        .sessions
        .list(&Filter {
            state: Some(State::Paused),
            ..Default::default()
        })
        .into_iter()
        .filter(|s| s.paused_by == Some(PausedBy::Allowance))
        .collect();
    let waits = lock_waits(app).clone();
    for s in &paused {
        let still = match waits.get(&s.id) {
            Some(vendor) => blocking(&allowances, vendor, now).is_some(),
            None => allowances.iter().any(|a| a.blocks(now)),
        };
        if !still {
            resume(app, &s.id, now);
        }
    }
    lock_waits(app).retain(|id, _| paused.iter().any(|s| &s.id == id));
    for card in app
        .cards
        .list(true, None)
        .into_iter()
        .filter(|c| c.kind == cards::Kind::Allowance)
    {
        let Some((vendor, _)) = of_subject(&card.subject) else {
            continue;
        };
        if blocking(&allowances, vendor, now).is_none() {
            for done in app
                .cards
                .withdraw(cards::Kind::Allowance, &card.subject, now)
            {
                app.bus.publish("card", &done);
            }
        }
    }
}

fn resume(app: &App, id: &str, now: u64) {
    lock_waits(app).remove(id);
    if app.sessions.set_state(id, State::Running, now).is_ok() {
        app.bus.publish("session", &app.sessions.get(id));
    }
}

/// The owner's card about a vendor's used-up window: one per vendor at a time, in the chat of the call's task (or the
/// main chat), with the window's numbers and the reset.
fn ask(app: &App, vendor: &str, window: &Allowance, session: Option<&Session>, now: u64) {
    let task = session.and_then(|s| s.task.clone());
    let project = session.map_or_else(|| "plaza".to_string(), |s| s.project.clone());
    let chat = match &task {
        Some(task) => crate::work_loop::task_chat(app, task, &project, now),
        None => app.chats.main(now).id,
    };
    let title = format!("The {vendor} allowance is used up");
    let body = format!(
        "The {} window: {} of {} used. {}",
        window.window,
        window.used,
        window.limit,
        reset_words(window, now)
    );
    let key = subject(vendor, &project);
    let (card, created) = app.cards.open(
        NewCard {
            kind: cards::Kind::Allowance,
            chat: Some(chat.clone()),
            project: Some(project),
            task,
            session: None,
            subject: key,
            title: title.clone(),
            body: Some(body.clone()),
            choices: choices(),
        },
        now,
    );
    if created {
        let posted = app.chats.post(
            &chat,
            Post {
                author: Author::System,
                session: None,
                seat: None,
                kind: MessageKind::Question,
                text: format!("{title}. {body}"),
                card: Some(card.id.clone()),
                attachments: Vec::new(),
            },
            now,
        );
        app.bus.publish("card", &card);
        if let Some(message) = posted {
            app.bus
                .publish("chat", &json!({ "chat": chat, "message": message }));
        }
    } else {
        app.bus.publish("card", &card);
    }
}

fn choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "wait".into(),
            label: "Wait for the reset".into(),
            detail: Some("the sessions resume by themselves when the window resets".into()),
            takes_amount: false,
        },
        Choice {
            id: "api_key".into(),
            label: "Use an API key for this project".into(),
            detail: Some(
                "this project's subscription calls go to the policy's first model that is not a subscription, \
                 and its waiting sessions resume now"
                    .into(),
            ),
            takes_amount: false,
        },
        Choice {
            id: "stop".into(),
            label: "Stop the waiting tasks".into(),
            detail: Some("their tasks end with a handover; nothing more is spent".into()),
            takes_amount: false,
        },
    ]
}

fn reset_words(window: &Allowance, now: u64) -> String {
    match window.resets_ms {
        Some(reset) if reset > now => format!(
            "It resets in {} min; the waiting sessions resume then.",
            (reset - now).div_ceil(60_000)
        ),
        Some(_) => "It has reset; the waiting sessions resume at the next tick.".into(),
        None => "The provider does not say when it resets.".into(),
    }
}

fn sentence(vendor: &str, window: &Allowance, now: u64) -> String {
    format!(
        "the {vendor} allowance ({} window) is used up. {}",
        window.window,
        reset_words(window, now)
    )
}

/// The sessions of `project` that an allowance of `vendor` paused.
fn waiting_on(app: &App, vendor: &str, project: &str) -> Vec<String> {
    let waits = lock_waits(app).clone();
    waits
        .into_iter()
        .filter(|(_, waiting)| waiting.as_str() == vendor)
        .map(|(id, _)| id)
        .filter(|id| app.sessions.get(id).is_some_and(|s| s.project == project))
        .collect()
}

/// The owner's answer to an allowance card. `wait` leaves the sessions to resume at the reset. `api_key` switches the
/// project's subscription calls to the policy's fallback (see [`route`]) and resumes its waiting sessions now. `stop`
/// ends the tasks whose sessions wait on the vendor.
pub fn card_answered(app: &App, card: &Card) -> String {
    let now = apz_plaza::now_ms();
    let Some((vendor, project)) = of_subject(&card.subject) else {
        return "recorded".into();
    };
    match card.answer.as_deref().unwrap_or("") {
        "api_key" => {
            app.allowance_waits.api_key().insert(project.to_string());
            let waiting = waiting_on(app, vendor, project);
            for id in &waiting {
                resume(app, id, now);
            }
            format!(
                "{project}'s {vendor} calls use the policy's API-key model; {} sessions resumed",
                waiting.len()
            )
        }
        "stop" => {
            let mut stopped = 0;
            for id in waiting_on(app, vendor, project) {
                let Some(task) = app.sessions.get(&id).and_then(|s| s.task) else {
                    continue;
                };
                crate::work_loop::stop_task(
                    app,
                    &task,
                    "stopped by the owner while its allowance is used up",
                    now,
                );
                stopped += 1;
            }
            format!("stopped {stopped} tasks waiting on the {vendor} allowance")
        }
        _ => "the sessions wait for the reset".into(),
    }
}

/// The allowances' thread: reads every usage provider now and then every [`READ_EVERY_MS`], and runs [`tick`] every
/// [`TICK`].
pub fn spawn_ticker(app: Arc<App>) {
    std::thread::Builder::new()
        .name("allowances".into())
        .spawn(move || {
            let mut read_at: Option<u64> = None;
            loop {
                let now = apz_plaza::now_ms();
                if read_at.is_none_or(|at| now.saturating_sub(at) >= READ_EVERY_MS) {
                    read_at = Some(now);
                    for provider in app.usage_providers.providers() {
                        let _ = crate::usage_providers::read(&app, &provider);
                    }
                }
                tick(&app, apz_plaza::now_ms());
                std::thread::sleep(TICK);
            }
        })
        .expect("spawn the allowances thread");
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/allowances", list_route)
        .doc(RouteDoc::new(
            "List every subscription allowance window the usage providers report, with its reset and the subscription models that draw on it",
        ))
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let now = apz_plaza::now_ms();
    let policy = app.settings.get().model_policy;
    let rows: Vec<Value> = app
        .usage_providers
        .allowances()
        .iter()
        .map(|a| {
            let models: Vec<&str> = policy
                .subscription
                .iter()
                .map(|m| bare(m))
                .filter(|m| vendor_of(m.split('/').next().unwrap_or("")) == a.vendor)
                .collect();
            json!({
                "provider": a.provider,
                "vendor": a.vendor,
                "name": a.name,
                "window": a.window,
                "used": a.used,
                "limit": a.limit,
                "left": a.limit.saturating_sub(a.used),
                "resets_ms": a.resets_ms,
                "checked_ms": a.checked_ms,
                "blocking": a.blocks(now),
                "models": models,
            })
        })
        .collect();
    Response::json(200, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_times_parse_as_epoch_milliseconds_or_utc_text() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_ms("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(
            rfc3339_ms("2024-02-29T12:00:00.250Z"),
            Some(1_709_208_000_000)
        );
        assert_eq!(rfc3339_ms("2024-02-29T12:00:00+01:00"), None);
        assert_eq!(rfc3339_ms("2024-13-01T00:00:00Z"), None);
        assert_eq!(reset_of(&json!(123_456)), Some(123_456));
    }

    #[test]
    fn a_usage_answer_yields_its_windows_by_vendor() {
        let answer = json!({
            "usage": { "total_requests": 9, "allowances": [
                { "provider": "claude", "account": "me@example.com", "window": "5h", "used": 40, "limit": 40,
                  "resets_at": "2026-10-24T18:00:00Z" },
                { "vendor": "codex", "window": "weekly", "limit": 500, "resets_at": 1_800_000_000_000u64 },
                { "provider": "gemini", "window": "daily", "used": 3 }
            ] }
        });
        let windows = parse(&answer, 77);
        assert_eq!(windows.len(), 2, "the entry with no limit is skipped");
        assert_eq!(windows[0].vendor, "anthropic");
        assert_eq!(windows[0].name, "me@example.com");
        assert!(windows[0].exhausted());
        assert_eq!(windows[0].resets_ms, rfc3339_ms("2026-10-24T18:00:00Z"));
        assert_eq!(windows[0].checked_ms, 77);
        assert_eq!(windows[1].vendor, "openai");
        assert_eq!(windows[1].used, 0);
        assert_eq!(windows[1].resets_ms, Some(1_800_000_000_000));
        assert!(parse(&json!({ "usage": {} }), 1).is_empty());
        assert!(
            parse(
                &json!({ "usage": {}, "allowances": answer["usage"]["allowances"] }),
                1
            )
            .is_empty(),
            "the usage object is read, as the totals are: a root list is read only when there is none"
        );
    }

    #[test]
    fn a_window_blocks_only_until_its_reset_on_a_fake_clock() {
        let window = |used, resets_ms| Allowance {
            provider: "hub".into(),
            vendor: "anthropic".into(),
            name: "me".into(),
            window: "5h".into(),
            used,
            limit: 10,
            resets_ms,
            checked_ms: 0,
        };
        let windows = [window(10, Some(5_000))];
        assert!(blocking(&windows, "anthropic", 4_999).is_some());
        assert!(
            blocking(&windows, "anthropic", 5_000).is_none(),
            "at the reset it runs again"
        );
        assert!(
            blocking(&windows, "openai", 4_999).is_none(),
            "another vendor is not held"
        );
        assert!(
            blocking(&[window(9, Some(5_000))], "anthropic", 1).is_none(),
            "a window with room is open"
        );
        assert!(
            blocking(&[window(10, None)], "anthropic", 1).is_some(),
            "no reset time blocks until a reading says otherwise"
        );
    }

    #[test]
    fn the_fallback_is_the_first_worker_model_that_is_not_a_subscription() {
        let policy = ModelPolicy {
            orchestrator: vec![],
            worker: vec![
                "anthropic/claude-haiku-5-5@low".into(),
                "openai/gpt-6-luna".into(),
            ],
            subscription: vec!["anthropic/claude-haiku-5-5".into()],
        };
        assert_eq!(fallback(&policy).as_deref(), Some("openai/gpt-6-luna"));
        let only_subscriptions = ModelPolicy {
            worker: vec!["anthropic/claude-haiku-5-5".into()],
            ..policy
        };
        assert_eq!(fallback(&only_subscriptions), None);
    }

    #[test]
    fn a_subscription_is_a_policy_model_with_or_without_its_effort() {
        let policy = ModelPolicy {
            orchestrator: vec![],
            worker: vec![],
            subscription: vec!["anthropic/claude-haiku-5-5".into()],
        };
        assert!(is_subscription(&policy, "anthropic/claude-haiku-5-5"));
        assert!(is_subscription(&policy, "anthropic/claude-haiku-5-5@low"));
        assert!(!is_subscription(&policy, "openai/gpt-6-luna"));
    }
}
