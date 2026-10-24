//! Incidents (PLAN 24.1, 12.1). An alert from a source (Sentry, Alertmanager, Grafana, CloudWatch, Cloudflare, an uptime
//! check) arrives at its connector's webhook (`/v1/hooks/{connector}/{token}`, `triggers::hooks`), is read into typed alerts
//! ([`alert`]) and is matched to a project by the service it names: an environment whose `ALERT_SERVICE` variable (or its
//! Cloudflare Worker or Pages name) is that service. A firing alert opens an incident, unless an open one has its
//! fingerprint: that delivery is folded into it. A resolved alert closes its incident. A deploy's health breach (23, 11.4)
//! opens one too, after the health watch has rolled the deploy back.
//!
//! Every new incident runs the incident recipe (`recipes/builtin/incident.toml`, the engine's run is recorded): triage
//! ([`recipe`]: the deploys and landings before the alert, the signals, the logs the connector can fetch), act (roll back
//! the correlating deploy through the deploy path, else open a fix task with a spec from the alert and its logs), tell (the
//! project chat, an incident card, the notify connector), after (a postmortem in the project's docs and its lessons as
//! memory proposals).
//!
//! Routes: `GET /v1/projects/{id}/incidents`, `GET /v1/projects/{id}/incidents/{iid}` (with its steps and lessons),
//! `POST /v1/projects/{id}/incidents/{iid}/resolve`. The webhook of each source is `triggers::hooks`' one route.

pub mod alert;
mod recipe;
mod sns;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use apz_http::{Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::App;
use crate::vault::OnBehalf;
use alert::{Alert, Parsed};

pub use recipe::Trigger;

/// The recipe an incident runs as, and the trigger each kind of incident names in its run.
pub const RECIPE: &str = "incident";
/// The source a deploy's health breach is recorded under.
pub const DEPLOY_HEALTH: &str = "deploy-health";
/// The environment variable that names the service an environment serves, as the alerts name it.
pub const SERVICE_VAR: &str = "ALERT_SERVICE";
/// The environment variable that names the logs connector an environment's incidents read (`loki`).
pub const LOGS_VAR: &str = "LOGS_CONNECTOR";
/// A deploy is the alert's suspect when it was recorded this long before the alert started (and a minute after).
const CORRELATE_MS: u64 = 60 * 60 * 1000;
const SLACK_MS: u64 = 60 * 1000;

// ---- Data ----------------------------------------------------------------------------------------------------------

/// One incident: an alert (or a breach) the hub is handling, from its opening to its postmortem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "incident")]
pub struct Incident {
    /// `i<n>`.
    #[table(key)]
    pub id: String,
    pub project: String,
    /// The environment the alert is about (empty when its service maps to none).
    #[serde(default)]
    pub environment: String,
    /// The connector the alert came in on, or [`DEPLOY_HEALTH`].
    pub source: String,
    /// The alert's fingerprint: a firing alert with an open incident's fingerprint is folded into it.
    pub fingerprint: String,
    /// `critical`, `high`, `medium` or `low` (see [`alert::Alert::severity`]).
    pub severity: String,
    pub title: String,
    /// The source's description, or the breach's numbers.
    #[serde(default)]
    pub detail: String,
    /// `open`, or `resolved` when the source resolved its alert or the owner closed it.
    pub status: String,
    pub started_ms: u64,
    /// 0 while open.
    #[serde(default)]
    pub resolved_ms: u64,
    /// When the last alert of it arrived.
    pub last_ms: u64,
    /// The deliveries of the alert it has taken (the first one opened it).
    pub seen: u64,
    #[serde(default)]
    #[table(list)]
    pub links: Vec<String>,
    /// What the triage correlated: the deploy before the alert, as `environment@version`, or empty.
    #[serde(default)]
    pub suspect: String,
    /// What the act step did: `rollback`, `rollback_waiting`, `rolled_back` (the health watch did it), `fix_task`, `none`,
    /// or empty before it ran.
    #[serde(default)]
    pub outcome: String,
    /// The version a rollback returns to, when the act step asked for one.
    #[serde(default)]
    pub rollback: String,
    /// The fix task the act step opened, when it did.
    #[serde(default)]
    pub task: String,
    /// The postmortem's file in the project's repository, once written.
    #[serde(default)]
    pub postmortem: String,
    /// The memory proposals the postmortem made (`mp<n>`).
    #[serde(default)]
    #[table(list)]
    pub lessons: Vec<String>,
    /// The log excerpt the triage fetched, when the source's connector can fetch one.
    #[serde(default)]
    pub logs: String,
    #[serde(default)]
    #[table(child = "incident_step")]
    pub steps: Vec<Step>,
}

/// One step of an incident's recipe run, as it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Step {
    /// `<incident>.<n>`.
    pub id: String,
    /// `triage`, `act`, `tell` or `after`.
    pub stage: String,
    pub at_ms: u64,
    /// `passed` or `failed`.
    pub outcome: String,
    pub text: String,
}

/// The table of incidents: `incident`, one row per incident, with its steps as children.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Incident)
}

/// A change that landed on a project's repository (Weft's landing event): what it touched and who made it. The triage
/// correlates an alert with the landings before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "incident_landing")]
pub struct Landing {
    /// `<project>/<change>`.
    #[table(key)]
    pub key: String,
    pub project: String,
    pub change: String,
    pub title: String,
    /// Who made the change, as Weft names its author (an agent's label, or `owner`).
    pub author: String,
    /// The task the change landed for, when there was one.
    #[serde(default)]
    pub task: String,
    /// The session the task was assigned to, when it was.
    #[serde(default)]
    pub session: String,
    pub at_ms: u64,
    /// The files the change touched, as the repository names them.
    #[serde(default)]
    #[table(list)]
    pub files: Vec<String>,
    /// The top-level directories of those files (their areas).
    #[serde(default)]
    #[table(list)]
    pub areas: Vec<String>,
}

/// The table of recorded landings: `incident_landing`, one row per change per project.
pub fn landing_def() -> &'static TableDef {
    apz_store::root_def!(Landing)
}

/// The incidents, in the order they were opened.
#[derive(Default)]
pub struct Store {
    items: Mutex<Vec<Incident>>,
    /// The landings the projects' repositories took.
    landings: Mutex<Vec<Landing>>,
    /// The SNS signing certificates read so far, by URL (`sns`).
    certs: Mutex<HashMap<String, Vec<u8>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    /// The project's incidents, oldest first.
    pub fn of(&self, project: &str) -> Vec<Incident> {
        lock(&self.items)
            .iter()
            .filter(|i| i.project == project)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Incident> {
        lock(&self.items).iter().find(|i| i.id == id).cloned()
    }

    /// The open incident of `project` with `fingerprint`, when there is one.
    fn open_for(&self, project: &str, fingerprint: &str) -> Option<Incident> {
        lock(&self.items)
            .iter()
            .find(|i| i.project == project && i.fingerprint == fingerprint && i.status == "open")
            .cloned()
    }

    /// Stores a new incident under the next `i<n>` id and returns it.
    fn add(&self, mut incident: Incident) -> Incident {
        let mut items = lock(&self.items);
        let next = items
            .iter()
            .filter_map(|i| i.id.strip_prefix('i')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        incident.id = format!("i{next}");
        for (n, step) in incident.steps.iter_mut().enumerate() {
            step.id = format!("{}.{}", incident.id, n + 1);
        }
        items.push(incident.clone());
        incident
    }

    /// Changes incident `id` in place; None when there is no such incident.
    fn update<T>(&self, id: &str, change: impl FnOnce(&mut Incident) -> T) -> Option<T> {
        let mut items = lock(&self.items);
        let incident = items.iter_mut().find(|i| i.id == id)?;
        Some(change(incident))
    }

    /// The rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), lock(&self.items).iter())
    }

    /// The project's recorded landings, newest first.
    pub fn landings_of(&self, project: &str) -> Vec<Landing> {
        let mut out: Vec<Landing> = lock(&self.landings)
            .iter()
            .filter(|l| l.project == project)
            .cloned()
            .collect();
        out.sort_by_key(|l| std::cmp::Reverse(l.at_ms));
        out
    }

    /// Records a landing (a change lands once per project: a repeat replaces the record).
    fn add_landing(&self, landing: Landing) {
        let mut items = lock(&self.landings);
        items.retain(|l| l.key != landing.key);
        items.push(landing);
    }

    /// The landing rows, as they are now (the saver writes what changed).
    pub fn landing_rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(landing_def(), lock(&self.landings).iter())
    }

    /// Replaces the landings with what plaza.db holds (at start).
    pub fn load_landings(&self, conn: &Connection) -> Result<(), StoreError> {
        *lock(&self.landings) = apz_store::table::load_list(conn, landing_def())?;
        Ok(())
    }

    /// Replaces the incidents with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        *lock(&self.items) = apz_store::table::load_list(conn, def())?;
        Ok(())
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

// ---- Matching alerts to projects -----------------------------------------------------------------------------------

/// The project and environment an alert's service maps to: the first environment (of a live project) whose
/// `ALERT_SERVICE`, Cloudflare Worker or Pages name is that service.
pub fn project_of(app: &App, service: &str) -> Option<(String, String)> {
    if service.is_empty() {
        return None;
    }
    for project in app.projects.list() {
        if project.archived {
            continue;
        }
        for env in app.environments.of(&project.id) {
            let names = [SERVICE_VAR, "CLOUDFLARE_WORKER", "CLOUDFLARE_PAGES"];
            if names
                .iter()
                .any(|var| env.variables.get(*var).map(String::as_str) == Some(service))
            {
                return Some((project.id.clone(), env.name.clone()));
            }
        }
    }
    None
}

// ---- Deliveries -----------------------------------------------------------------------------------------------------

/// An alert source's delivery, after the hook route has checked its token: the connector must be enabled, its secret
/// must check out, and each alert is handled. A delivery that names no alert to record is acknowledged as ignored.
pub fn delivery(
    app: &App,
    connector: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
) -> Response {
    let Some(arc) = app.handle().upgrade() else {
        return Response::error(503, "the hub is not ready");
    };
    if crate::connectors::hub_values(app, connector).is_err() {
        return Response::error(404, "no such webhook");
    }
    if connector == "cloudwatch" {
        return sns_delivery(&arc, header, body);
    }
    let Some(secret) = app.vault.value_for(
        &crate::triggers::hooks::secret_name(connector),
        crate::triggers::BINDING,
        &OnBehalf::Hub,
    ) else {
        return Response::error(401, "the webhook has no signing secret");
    };
    if !alert::check(connector, &secret, header, body) {
        return Response::error(401, "the signature does not match the body");
    }
    match alert::parse(connector, header, body) {
        Err(why) => Response::error(400, &why),
        Ok(Parsed::Ignored(why)) => Response::json(202, &json!({ "ignored": why })),
        Ok(Parsed::Alerts(list)) => {
            let now = now();
            let outcomes: Vec<Value> = list.iter().map(|a| handle(&arc, a, now)).collect();
            Response::json(202, &json!({ "alerts": outcomes }))
        }
    }
}

/// A CloudWatch alarm's SNS delivery (`cloudwatch`): the signature is checked against SNS's certificate for the topic's
/// region, a subscription is confirmed through its SubscribeURL, and an alarm is handled as any alert.
fn sns_delivery(app: &Arc<App>, header: &dyn Fn(&str) -> Option<String>, body: &[u8]) -> Response {
    let envelope: Value = match serde_json::from_slice(body) {
        Ok(envelope) => envelope,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let kind = envelope["Type"].as_str().unwrap_or("");
    if header("x-amz-sns-message-type").is_some_and(|sent| sent != kind) {
        return Response::error(400, "the message type header and the body differ");
    }
    let http = crate::platforms::http::Ureq;
    let client = sns::Sns {
        http: &http,
        certs: &app.incidents.certs,
    };
    if kind == "SubscriptionConfirmation" {
        return match client.confirm(&envelope) {
            Ok(()) => Response::json(202, &json!({ "subscription": "confirmed" })),
            Err(why) => Response::error(401, &why),
        };
    }
    if let Err(why) = client.verify(&envelope) {
        return Response::error(401, &why);
    }
    match alert::parse("cloudwatch", header, body) {
        Err(why) => Response::error(400, &why),
        Ok(Parsed::Ignored(why)) => Response::json(202, &json!({ "ignored": why })),
        Ok(Parsed::Alerts(list)) => {
            let now = now();
            let outcomes: Vec<Value> = list.iter().map(|a| handle(app, a, now)).collect();
            Response::json(202, &json!({ "alerts": outcomes }))
        }
    }
}

/// A change landed on `repo` (Weft's landing event): each project on it records the landing, with the files the change
/// touched, their areas, and who made it (Weft's author, the task it landed for and that task's session).
pub fn landed(app: &App, repo: &str, change: &str) {
    let projects: Vec<String> = app
        .projects
        .list()
        .into_iter()
        .filter(|p| p.repo.as_deref() == Some(repo))
        .map(|p| p.id)
        .collect();
    if projects.is_empty() {
        return;
    }
    let detail = match app.weft.get(&format!("/changes/{change}")) {
        Ok(detail) => detail,
        Err(why) => {
            eprintln!("apid: incidents: the landing of {change} was not read: {why}");
            return;
        }
    };
    let files = match crate::project_memory::touched(app, change) {
        Ok((files, _)) => files,
        Err(why) => {
            eprintln!("apid: incidents: the files of {change} were not read: {why}");
            Vec::new()
        }
    };
    let mut areas: Vec<String> = files
        .iter()
        .filter_map(|f| f.split_once('/').map(|(area, _)| area.to_string()))
        .collect();
    areas.sort();
    areas.dedup();
    let task = detail["task"].as_str().unwrap_or("").to_string();
    let session = app
        .plaza
        .tasks
        .find(&task)
        .and_then(|t| t.view().assignee_id.clone())
        .unwrap_or_default();
    let title = detail["title"].as_str().unwrap_or("").to_string();
    let author = detail["author"].as_str().unwrap_or("").to_string();
    for project in projects {
        app.incidents.add_landing(Landing {
            key: format!("{project}/{change}"),
            project,
            change: change.to_string(),
            title: title.clone(),
            author: author.clone(),
            task: task.clone(),
            session: session.clone(),
            at_ms: now(),
            files: files.clone(),
            areas: areas.clone(),
        });
    }
}

/// One alert: folded into its open incident, resolving it, or opening a new incident that runs the recipe.
fn handle(app: &Arc<App>, alert: &Alert, now: u64) -> Value {
    let Some((project, environment)) = project_of(app, &alert.service) else {
        return json!({ "fingerprint": alert.fingerprint, "outcome": "unmapped", "service": alert.service });
    };
    let open = app.incidents.open_for(&project, &alert.fingerprint);
    if !alert.firing {
        let Some(incident) = open else {
            return json!({ "fingerprint": alert.fingerprint, "outcome": "not_open" });
        };
        let at = alert.resolved_ms.unwrap_or(now);
        let text = format!("{} is resolved at {}.", incident.title, incident.source);
        resolve(app, &incident.id, at, &text);
        return json!({ "incident": incident.id, "outcome": "resolved" });
    }
    if let Some(incident) = open {
        app.incidents.update(&incident.id, |i| {
            i.seen += 1;
            i.last_ms = now;
        });
        return json!({ "incident": incident.id, "outcome": "folded", "seen": incident.seen + 1 });
    }
    let started = alert.started_ms.map_or(now, |s| s.min(now));
    let incident = app.incidents.add(Incident {
        id: String::new(),
        project,
        environment,
        source: alert.source.clone(),
        fingerprint: alert.fingerprint.clone(),
        severity: alert.severity.clone(),
        title: alert.title.clone(),
        detail: alert.detail.clone(),
        status: "open".into(),
        started_ms: started,
        resolved_ms: 0,
        last_ms: now,
        seen: 1,
        links: alert.links.clone(),
        suspect: String::new(),
        outcome: String::new(),
        rollback: String::new(),
        task: String::new(),
        postmortem: String::new(),
        lessons: Vec::new(),
        logs: String::new(),
        steps: Vec::new(),
    });
    app.bus.publish(
        "incidents",
        &json!({ "project": incident.project, "what": "opened", "incident": incident.id }),
    );
    let outcome = recipe::run(app, &incident.id, Trigger::Alert);
    json!({ "incident": incident.id, "outcome": outcome })
}

/// A deploy's health breach (11.4): the health watch has rolled `version` back on `env` already. The incident records
/// the breach with its numbers and runs the recipe, which opens the incident card in the project's chat.
pub fn breach(app: &Arc<App>, project: &str, env: &str, version: &str, detail: &str) {
    let now = now();
    let incident = app.incidents.add(Incident {
        id: String::new(),
        project: project.to_string(),
        environment: env.to_string(),
        source: DEPLOY_HEALTH.into(),
        fingerprint: format!("{DEPLOY_HEALTH}:{env}:{version}:{now}"),
        severity: "high".into(),
        title: format!("{version} breached its health window on {env}"),
        detail: detail.to_string(),
        status: "open".into(),
        started_ms: now,
        resolved_ms: 0,
        last_ms: now,
        seen: 1,
        links: Vec::new(),
        suspect: format!("{env}@{version}"),
        outcome: String::new(),
        rollback: String::new(),
        task: String::new(),
        postmortem: String::new(),
        lessons: Vec::new(),
        logs: String::new(),
        steps: Vec::new(),
    });
    recipe::run(app, &incident.id, Trigger::Breach);
}

/// Closes incident `id` at `at_ms` (the owner's or its source's): its status, and a line in the project's chat.
fn resolve(app: &App, id: &str, at_ms: u64, text: &str) -> Option<Incident> {
    let incident = app.incidents.update(id, |i| {
        i.status = "resolved".into();
        i.resolved_ms = at_ms.max(i.started_ms);
        i.clone()
    })?;
    let now = now();
    let chat = crate::work_loop::project_chat(app, &incident.project, now);
    crate::work_loop::notice(app, &chat, &format!("Incident {id}: {text}"), now);
    app.bus.publish(
        "incidents",
        &json!({ "project": incident.project, "what": "resolved", "incident": id }),
    );
    Some(incident)
}

// ---- Routes ---------------------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/landings", |app, _, p| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            let list: Vec<Value> = app
                .incidents
                .landings_of(project)
                .iter()
                .map(|l| serde_json::to_value(l).unwrap_or(Value::Null))
                .collect();
            Response::json(200, &json!({ "landings": list }))
        })
        .doc(RouteDoc::new("A project's recorded landings, newest first: the change, its author and task, and the files and areas it touched").errors(&[404]))
        .get("/v1/projects/{id}/incidents", |app, _, p| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            let list: Vec<Value> = app
                .incidents
                .of(project)
                .iter()
                .map(|i| view(app, i))
                .collect();
            Response::json(200, &json!({ "incidents": list }))
        })
        .doc(RouteDoc::new("A project's incidents, oldest first: the alerts and breaches it is handling or has handled").errors(&[404]))
        .get("/v1/projects/{id}/incidents/{iid}", |app, _, p| {
            match app.incidents.get(p.get("iid")) {
                Some(i) if i.project == p.get("id") => Response::json(200, &view(app, &i)),
                _ => Response::error(404, "no such incident"),
            }
        })
        .doc(RouteDoc::new("One incident: its alert, the correlation, the act taken, its steps and its lessons").errors(&[404]))
        .post("/v1/projects/{id}/incidents/{iid}/resolve", |app, _, p| {
            match app.incidents.get(p.get("iid")) {
                Some(i) if i.project == p.get("id") && i.status == "open" => {
                    let text = format!("{} is closed by the owner.", i.title);
                    let at = now();
                    match resolve(app, &i.id, at, &text) {
                        Some(done) => Response::json(200, &view(app, &done)),
                        None => Response::error(404, "no such incident"),
                    }
                }
                Some(i) if i.project == p.get("id") => Response::error(409, "the incident is resolved already"),
                _ => Response::error(404, "no such incident"),
            }
        })
        .doc(RouteDoc::new("Close an open incident as the owner: its status becomes resolved and the project's chat is told").errors(&[404, 409]))
}

/// An incident as its routes show it: the record, and the text of each lesson the postmortem made.
fn view(app: &App, incident: &Incident) -> Value {
    let mut value = serde_json::to_value(incident).unwrap_or(Value::Null);
    let proposals = app.project_memory.proposals_of(&incident.project);
    value["lesson_texts"] = incident
        .lessons
        .iter()
        .map(|id| {
            proposals
                .iter()
                .find(|p| &p.id == id)
                .map(|p| json!({ "id": p.id, "kind": p.kind, "text": p.text }))
                .unwrap_or_else(|| json!({ "id": id }))
        })
        .collect::<Vec<_>>()
        .into();
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incident(project: &str, fingerprint: &str) -> Incident {
        Incident {
            id: String::new(),
            project: project.into(),
            environment: "production".into(),
            source: "sentry".into(),
            fingerprint: fingerprint.into(),
            severity: "high".into(),
            title: "t".into(),
            detail: String::new(),
            status: "open".into(),
            started_ms: 1,
            resolved_ms: 0,
            last_ms: 1,
            seen: 1,
            links: Vec::new(),
            suspect: String::new(),
            outcome: String::new(),
            rollback: String::new(),
            task: String::new(),
            postmortem: String::new(),
            lessons: Vec::new(),
            logs: String::new(),
            steps: Vec::new(),
        }
    }

    #[test]
    fn incidents_get_ids_in_order_and_an_open_fingerprint_is_found_per_project() {
        let store = Store::default();
        let first = store.add(incident("site", "sentry:1"));
        let second = store.add(incident("api", "sentry:1"));
        assert_eq!((first.id.as_str(), second.id.as_str()), ("i1", "i2"));
        assert_eq!(
            store.open_for("site", "sentry:1").map(|i| i.id),
            Some("i1".into())
        );
        assert!(store.open_for("site", "sentry:2").is_none());
        store.update("i1", |i| i.status = "resolved".into());
        assert!(
            store.open_for("site", "sentry:1").is_none(),
            "a resolved incident is not open"
        );
        assert_eq!(store.of("site").len(), 1);
    }

    #[test]
    fn the_incident_table_keeps_its_steps_as_children() {
        let store = Store::default();
        let mut with_steps = incident("site", "f");
        with_steps.steps.push(Step {
            id: String::new(),
            stage: "triage".into(),
            at_ms: 5,
            outcome: "passed".into(),
            text: "no deploy".into(),
        });
        let saved = store.add(with_steps);
        assert_eq!(saved.steps[0].id, "i1.1");
        assert_eq!(store.get(&saved.id).unwrap().steps.len(), 1);
    }
}
