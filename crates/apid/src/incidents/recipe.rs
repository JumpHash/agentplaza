//! The incident recipe's four steps (PLAN 24.1, 12.1): triage, act, tell and after. The run is the engine's (`recipes::runs`)
//! and each step is settled as it ends; the steps themselves are plain code, because the work is the hub's own: the
//! deploy path, the task planner, the chat, Weft's docs and project memory. The two outside reads are ports: the logs a
//! source's connector can fetch ([`Logs`]) and the project's notify connector ([`Notify`]). Production answers them from
//! the connectors (Sentry's latest event; Telegram's `send`, when that connector is enabled); a test gives its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{DEPLOY_HEALTH, Incident, RECIPE, Step};
use crate::cards::{Kind, NewCard};
use crate::environments;
use crate::project_memory;
use crate::recipes::graph::{Move, Report};
use crate::state::App;
use crate::task_plan;
use crate::work_loop;

/// What started an incident: a source's firing alert, or a deploy's health breach (11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Alert,
    Breach,
}

/// The logs a source's connector can fetch for an alert: a text excerpt, or none when the connector fetches none.
pub trait Logs: Send + Sync {
    fn fetch(&self, source: &str, fingerprint: &str) -> Result<Option<String>, String>;
}

/// The project's notify connector: a notice sent out of the hub, when one is connected.
pub trait Notify: Send + Sync {
    fn send(&self, project: &str, text: &str) -> Result<(), String>;
}

/// Runs incident `id` through its recipe: triage, act, tell, after. Returns the act's outcome.
pub fn run(app: &Arc<App>, id: &str, trigger: Trigger) -> String {
    let ports = Connected {
        app: Arc::clone(app),
    };
    run_with(app, id, trigger, &ports, &ports)
}

/// [`run`] with the outside reads given (a test's fakes; production passes [`Connected`]).
pub fn run_with(
    app: &Arc<App>,
    id: &str,
    trigger: Trigger,
    logs: &dyn Logs,
    notify: &dyn Notify,
) -> String {
    let Some(incident) = app.incidents.get(id) else {
        return "none".into();
    };
    let trigger_name = match trigger {
        Trigger::Alert => "alert",
        Trigger::Breach => "breach",
    };
    let run = app.recipe_runs.start_engine_items(
        RECIPE,
        &BTreeMap::new(),
        trigger_name,
        None,
        Some(incident.project.as_str()),
        Some(id),
        now(),
    );
    let run = run.as_deref();

    let triaged = triage(app, &incident, logs);
    let suspect = triaged
        .suspect
        .as_ref()
        .map(|s| format!("{}@{}", s.env, s.version));
    app.incidents.update(id, |i| {
        i.suspect = suspect.clone().unwrap_or_default();
        i.logs = triaged.logs.clone().unwrap_or_default();
    });
    record(app, id, run, "triage", true, triaged.notes.join(" "));

    let incident = app.incidents.get(id).unwrap_or(incident);
    let acted = act(app, &incident, &triaged);
    app.incidents.update(id, |i| {
        i.outcome = acted.outcome.clone();
        i.rollback = acted.rollback.clone();
        i.task = acted.task.clone();
    });
    record(app, id, run, "act", acted.ok, acted.detail.clone());

    let incident = app.incidents.get(id).unwrap_or(incident);
    let told = tell(app, &incident, &acted, notify);
    record(app, id, run, "tell", true, told);

    let incident = app.incidents.get(id).unwrap_or(incident);
    let (after_ok, after_text) = after(app, &incident, &acted);
    record(app, id, run, "after", after_ok, after_text);

    app.incidents.get(id).map(|i| i.outcome).unwrap_or_default()
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// Appends a step to incident `id` and settles it in its run; the steps the run then makes ready are started.
fn record(app: &App, id: &str, run: Option<&str>, stage: &str, passed: bool, text: String) {
    let at = now();
    app.incidents.update(id, |i| {
        let step = Step {
            id: format!("{id}.{}", i.steps.len() + 1),
            stage: stage.to_string(),
            at_ms: at,
            outcome: if passed { "passed" } else { "failed" }.to_string(),
            text,
        };
        i.steps.push(step);
    });
    if let Some(run) = run {
        for step in app
            .recipe_runs
            .settle(run, stage, &Report::verdict(passed), at)
        {
            if let Move::Ready(next) = step {
                app.recipe_runs.started(run, &next, at);
            }
        }
    }
}

// ---- Triage ---------------------------------------------------------------------------------------------------------

/// A deploy the alert may come from: its environment, version and time, whether it is still what the environment runs,
/// and whether the health watch rolled it back already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspect {
    pub env: String,
    pub version: String,
    pub at_ms: u64,
    pub live: bool,
    pub rolled_back: bool,
}

/// What the triage found: the suspect deploy, the notes on what was correlated, and the logs fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triage {
    pub suspect: Option<Suspect>,
    pub notes: Vec<String>,
    pub logs: Option<String>,
}

/// Correlates the incident with the deploys, the releases (their landings) and the signals before it started, and reads
/// the logs the source's connector can fetch.
pub fn triage(app: &App, incident: &Incident, logs: &dyn Logs) -> Triage {
    if incident.source == DEPLOY_HEALTH {
        let (env, version) = incident.suspect.split_once('@').unwrap_or(("", ""));
        return Triage {
            suspect: Some(Suspect {
                env: env.to_string(),
                version: version.to_string(),
                at_ms: incident.started_ms,
                live: false,
                rolled_back: true,
            }),
            notes: vec![format!(
                "The health watch rolled {version} back on {env} when it breached ({}).",
                incident.detail
            )],
            logs: None,
        };
    }
    let started = incident.started_ms;
    let from = started.saturating_sub(super::CORRELATE_MS);
    let until = started + super::SLACK_MS;
    let mut notes = Vec::new();
    let mut best: Option<Suspect> = None;
    for env in app.environments.of(&incident.project) {
        if !incident.environment.is_empty() && env.name != incident.environment {
            continue;
        }
        for deploy in &env.deploys {
            let landed = matches!(
                deploy.status.as_str(),
                "passed" | "sent" | "recorded" | "rolling" | "rollback"
            );
            let newer = best.as_ref().is_none_or(|b| deploy.at_ms > b.at_ms);
            if landed && deploy.at_ms >= from && deploy.at_ms <= until && newer {
                best = Some(Suspect {
                    env: env.name.clone(),
                    version: deploy.version.clone(),
                    at_ms: deploy.at_ms,
                    live: env.version.as_deref() == Some(deploy.version.as_str()),
                    rolled_back: false,
                });
            }
        }
    }
    match &best {
        Some(s) => notes.push(format!(
            "Deploy {} to {} was recorded {} minute(s) before the alert started.",
            s.version,
            s.env,
            started.saturating_sub(s.at_ms) / 60_000
        )),
        None => notes.push("No deploy in the hour before the alert.".into()),
    }
    for release in app.releases.list(&incident.project) {
        if release.created_ms >= from && release.created_ms <= until {
            notes.push(format!(
                "Release {} was cut in the hour before the alert, with {} landing(s).",
                release.version,
                release.changes.len()
            ));
        }
    }
    let signals = app
        .maintenance
        .get(&incident.project)
        .signals
        .iter()
        .filter(|s| s.kind == "error" && s.at_ms >= from && s.at_ms <= until)
        .filter(|s| {
            incident.environment.is_empty()
                || s.environment.as_deref() == Some(incident.environment.as_str())
        })
        .count();
    notes.push(format!(
        "{signals} error signal(s) in the hour before the alert."
    ));
    let fetched = match logs.fetch(&incident.source, &incident.fingerprint) {
        Ok(Some(text)) => {
            notes.push("The source's latest event is in the logs.".into());
            Some(text)
        }
        Ok(None) => None,
        Err(why) => {
            notes.push(format!("The logs could not be fetched: {why}."));
            None
        }
    };
    Triage {
        suspect: best,
        notes,
        logs: fetched,
    }
}

// ---- Act ------------------------------------------------------------------------------------------------------------

/// What the act step did: its outcome (one of the incident's outcomes), a line of detail, the version a rollback returns
/// to, the fix task it opened, and whether the step passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acted {
    pub outcome: String,
    pub detail: String,
    pub rollback: String,
    pub task: String,
    pub ok: bool,
}

/// A correlating deploy is rolled back through the deploy path (the previous release is deployed again, with its
/// stages and checks, and a protected environment waits for the owner). Without one, a fix task is opened.
pub fn act(app: &Arc<App>, incident: &Incident, triage: &Triage) -> Acted {
    let Some(suspect) = &triage.suspect else {
        return fix_task(app, incident, triage, "no deploy correlates with the alert");
    };
    if suspect.rolled_back {
        return Acted {
            outcome: "rolled_back".into(),
            detail: format!(
                "{} was rolled back by the health watch already; no further rollback is needed.",
                suspect.version
            ),
            rollback: String::new(),
            task: String::new(),
            ok: true,
        };
    }
    if !suspect.live {
        return fix_task(
            app,
            incident,
            triage,
            "the correlated deploy is no longer live",
        );
    }
    let Some(previous) = previous(app, &incident.project, suspect) else {
        return fix_task(
            app,
            incident,
            triage,
            "no earlier release is there to return to",
        );
    };
    let why = format!(
        "incident {}: {} correlates with the alert \"{}\"",
        incident.id, suspect.version, incident.title
    );
    match environments::request(
        app,
        &incident.project,
        &suspect.env,
        &previous,
        0,
        "incident",
        &why,
    ) {
        Ok(deploy) if deploy.status == "waiting" => Acted {
            outcome: "rollback_waiting".into(),
            detail: format!(
                "Rolling {} back from {} to {previous} waits for the owner's approval in Approvals.",
                suspect.env, suspect.version
            ),
            rollback: previous,
            task: String::new(),
            ok: true,
        },
        Ok(_) => Acted {
            outcome: "rollback".into(),
            detail: format!(
                "Rolling {} back from {} to {previous} through the deploy path.",
                suspect.env, suspect.version
            ),
            rollback: previous,
            task: String::new(),
            ok: true,
        },
        Err(why_refused) => fix_task(
            app,
            incident,
            triage,
            &format!("the rollback was refused: {why_refused}"),
        ),
    }
}

/// The release an environment ran before the suspect deploy, when there is one.
fn previous(app: &App, project: &str, suspect: &Suspect) -> Option<String> {
    app.environments
        .of(project)
        .into_iter()
        .find(|e| e.name == suspect.env)?
        .deploys
        .iter()
        .filter(|d| {
            d.at_ms < suspect.at_ms
                && d.version != suspect.version
                && matches!(
                    d.status.as_str(),
                    "passed" | "sent" | "recorded" | "rollback"
                )
        })
        .max_by_key(|d| d.at_ms)
        .map(|d| d.version.clone())
}

/// Opens a fix task with a spec drawn from the alert, its triage and its logs.
fn fix_task(app: &Arc<App>, incident: &Incident, triage: &Triage, why: &str) -> Acted {
    let logs = triage.logs.as_deref().unwrap_or("(no logs were fetched)");
    let goal = format!(
        "Fix the cause of incident {id}: {title} ({severity}, from {source}).\n\n{detail}\n\nLinks:\n{links}\n\nTriage:\n{notes}\n\nLogs:\n{logs}\n\nNo rollback was made: {why}.",
        id = incident.id,
        title = incident.title,
        severity = incident.severity,
        source = incident.source,
        detail = incident.detail,
        links = incident.links.join("\n"),
        notes = triage.notes.join("\n"),
    );
    let planned = task_plan::Planned {
        key: format!("incident-{}", incident.id),
        goal: clip(&goal, 7000),
        acceptance: Some(
            "The alert's condition no longer holds on the service, a regression test covers the failure in the logs, and the change is landed."
                .into(),
        ),
        role: Some("builder".into()),
        after: Vec::new(),
    };
    match task_plan::post(
        &app.projects,
        &app.plaza.tasks,
        &incident.project,
        &[planned],
    ) {
        Ok(posted) => {
            let task = posted.first().map(|p| p.id.clone()).unwrap_or_default();
            app.work.dispatch_idle(app);
            Acted {
                outcome: "fix_task".into(),
                detail: format!("Opened fix task {task}: {why}."),
                rollback: String::new(),
                task,
                ok: true,
            }
        }
        Err(e) => Acted {
            outcome: "none".into(),
            detail: format!("No fix task could be opened ({e}); {why}."),
            rollback: String::new(),
            task: String::new(),
            ok: false,
        },
    }
}

// ---- Tell -----------------------------------------------------------------------------------------------------------

/// Tells the owner: a notice in the project's chat, an incident card (a breach keeps the card it always had), and the
/// notify connector. Returns the step's text.
fn tell(app: &App, incident: &Incident, acted: &Acted, notify: &dyn Notify) -> String {
    let now = now();
    let text = format!(
        "Incident {}: {} ({}). {}",
        incident.id, incident.title, incident.severity, acted.detail
    );
    let chat = work_loop::project_chat(app, &incident.project, now);
    work_loop::notice(app, &chat, &text, now);
    let (title, body, subject) = if incident.source == DEPLOY_HEALTH {
        (
            incident.title.clone(),
            format!(
                "{}. The deploy was rolled back to the previous release.",
                incident.detail
            ),
            format!("incident:{}", incident.suspect.replace('@', ":")),
        )
    } else {
        (
            format!("{} ({})", incident.title, incident.severity),
            format!("{} Incident {}.", acted.detail, incident.id),
            format!("incident:{}", incident.id),
        )
    };
    work_loop::open_card(
        app,
        NewCard {
            kind: Kind::Incident,
            chat: Some(chat),
            project: Some(incident.project.clone()),
            task: None,
            session: None,
            subject,
            title,
            body: Some(body),
            choices: Vec::new(),
        },
        now,
    );
    match notify.send(&incident.project, &text) {
        Ok(()) => text,
        Err(why) => format!("{text} The notify connector did not send it: {why}."),
    }
}

// ---- After ----------------------------------------------------------------------------------------------------------

/// Writes the postmortem to the project's docs in Weft and proposes its lessons into the project's memory as pitfalls
/// and decisions. Returns whether the postmortem was written, and the step's text.
fn after(app: &App, incident: &Incident, acted: &Acted) -> (bool, String) {
    let path = format!("docs/incidents/{}.md", incident.id);
    let lessons = lessons(incident, acted);
    let text = postmortem(incident, acted, &lessons);
    let written = write_doc(
        app,
        &incident.project,
        &path,
        &text,
        &format!("Postmortem of incident {}: {}", incident.id, incident.title),
    );
    let now = now();
    let ids: Vec<String> = lessons
        .iter()
        .map(|(kind, text)| {
            app.project_memory
                .add_proposal(project_memory::Proposal {
                    id: String::new(),
                    project: incident.project.clone(),
                    kind: kind.clone(),
                    text: text.clone(),
                    anchors: Vec::new(),
                    why: format!("Lesson of incident {}", incident.id),
                    by_cid: 0,
                    by: "incident".into(),
                    created_ms: now,
                })
                .id
        })
        .collect();
    let saved = written.is_ok();
    app.incidents.update(&incident.id, |i| {
        if saved {
            i.postmortem = path.clone();
        }
        i.lessons = ids.clone();
    });
    match written {
        Ok(()) => (
            true,
            format!(
                "Postmortem at {path}; {} lesson(s) proposed to memory.",
                ids.len()
            ),
        ),
        Err(why) => (
            false,
            format!(
                "The postmortem was not written ({why}); {} lesson(s) proposed to memory.",
                ids.len()
            ),
        ),
    }
}

/// The lessons, as (memory type, text): a pitfall for what went wrong, a decision for a rollback's rule.
fn lessons(incident: &Incident, acted: &Acted) -> Vec<(String, String)> {
    let env = if incident.environment.is_empty() {
        "the service".to_string()
    } else {
        incident.environment.clone()
    };
    let mut out = Vec::new();
    match acted.outcome.as_str() {
        "rollback" | "rollback_waiting" => {
            out.push((
                "pitfall".to_string(),
                format!(
                    "{}: {env} ran a deploy that fired the alert; it was rolled back to {} while the cause was found. Check its signals before the next promotion.",
                    incident.title, acted.rollback
                ),
            ));
            out.push((
                "decision".to_string(),
                format!(
                    "An alert that correlates with a deploy is answered by rolling that deploy back first, through the deploy path (incident {}).",
                    incident.id
                ),
            ));
        }
        "rolled_back" => out.push((
            "pitfall".to_string(),
            format!(
                "{}: the health window caught the breach and rolled the release back on its own; the breach numbers are in the incident card.",
                incident.title
            ),
        )),
        "fix_task" => out.push((
            "pitfall".to_string(),
            format!(
                "{} on {env} had no correlating deploy, so its cause is in the code or its configuration; fix task {} carries the alert and the logs.",
                incident.title, acted.task
            ),
        )),
        _ => out.push((
            "pitfall".to_string(),
            format!("{}: {}", incident.title, incident.detail),
        )),
    }
    out
}

/// The postmortem's text: the alert, the timeline of the steps, the correlation, the logs, the action and the lessons.
fn postmortem(incident: &Incident, acted: &Acted, lessons: &[(String, String)]) -> String {
    let mut out = format!("# Incident {}: {}\n\n", incident.id, incident.title);
    out.push_str(&format!("- Severity: {}\n", incident.severity));
    out.push_str(&format!("- Source: {}\n", incident.source));
    if !incident.environment.is_empty() {
        out.push_str(&format!("- Environment: {}\n", incident.environment));
    }
    out.push_str(&format!("- Started: {}\n", stamp(incident.started_ms)));
    out.push_str(&format!("- Last alert: {}\n", stamp(incident.last_ms)));
    out.push_str(&format!("- Alerts folded in: {}\n", incident.seen));
    if incident.resolved_ms > 0 {
        out.push_str(&format!("- Resolved: {}\n", stamp(incident.resolved_ms)));
    }
    out.push_str(&format!("\n## What happened\n\n{}\n", incident.detail));
    if !incident.links.is_empty() {
        out.push('\n');
        for link in &incident.links {
            out.push_str(&format!("- {link}\n"));
        }
    }
    out.push_str("\n## Timeline\n\n");
    for step in &incident.steps {
        out.push_str(&format!(
            "- {} {} ({}): {}\n",
            stamp(step.at_ms),
            step.stage,
            step.outcome,
            step.text
        ));
    }
    if !incident.suspect.is_empty() {
        out.push_str(&format!(
            "\n## Correlation\n\nThe deploy before the alert: {}.\n",
            incident.suspect
        ));
    }
    if !incident.logs.is_empty() {
        out.push_str(&format!("\n## Logs\n\n```\n{}\n```\n", incident.logs));
    }
    out.push_str(&format!(
        "\n## Action\n\n{}: {}\n",
        acted.outcome, acted.detail
    ));
    out.push_str("\n## Lessons\n\n");
    for (kind, text) in lessons {
        out.push_str(&format!("- {kind}: {text}\n"));
    }
    out
}

/// Writes one file to the project's repository on main, as the owner's edit (the upload first, then the edit).
fn write_doc(app: &App, project: &str, path: &str, text: &str, title: &str) -> Result<(), String> {
    if !app.weft.running() {
        return Err("Weft is not running".into());
    }
    let repo = crate::weft_hub::ensure_repo(app, project)?;
    let bytes = text.as_bytes().to_vec();
    let hash = weft::hash::Hash::of(&bytes);
    let client = app.weft.client("owner")?;
    client.upload(&[hash], &|_| Ok(bytes.clone()))?;
    let edit =
        json!({ "title": title, "ref": "main", "files": [{ "path": path, "hash": hash.hex() }] });
    let landed = client.post(&format!("/repos/{repo}/edits"), &edit)?;
    app.bus.publish(
        "weft_land",
        &json!({ "repo": repo, "change": landed["change"], "kind": "edit" }),
    );
    Ok(())
}

// ---- The connected ports ----------------------------------------------------------------------------------------------

/// The production ports: Sentry's latest event through the `sentry` connector, and `send` through the `telegram`
/// connector, each when it is enabled.
pub struct Connected {
    app: Arc<App>,
}

impl Logs for Connected {
    fn fetch(&self, source: &str, fingerprint: &str) -> Result<Option<String>, String> {
        match source {
            "sentry" => sentry_latest(&self.app, fingerprint),
            _ => Ok(None),
        }
    }
}

impl Notify for Connected {
    fn send(&self, _project: &str, text: &str) -> Result<(), String> {
        if crate::connectors::hub_values(&self.app, "telegram").is_err() {
            return Ok(());
        }
        crate::connectors::call(
            &self.app,
            None,
            "incident",
            "telegram",
            "send",
            &json!({ "text": text }),
        )
        .map(|_| ())
    }
}

/// The latest event of a Sentry issue, as an excerpt: its title and each exception's type and value. The connector's
/// token is a bearer header and is never in an error.
fn sentry_latest(app: &App, fingerprint: &str) -> Result<Option<String>, String> {
    let Ok(values) = crate::connectors::hub_values(app, "sentry") else {
        return Ok(None);
    };
    let token = values.get("auth_token").map(String::as_str).unwrap_or("");
    if token.trim().is_empty() {
        return Ok(None);
    }
    let base = values
        .get("url")
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or("https://sentry.io");
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err("the Sentry connector's URL is an http or https address".into());
    }
    let id = fingerprint.strip_prefix("sentry:").unwrap_or(fingerprint);
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("the alert names no Sentry issue".into());
    }
    let headers = vec![("Authorization", format!("Bearer {token}"))];
    let http = crate::platforms::http::Ureq;
    let reply = crate::platforms::http::Http::call(
        &http,
        "GET",
        &format!("{base}/api/0/issues/{id}/events/latest/"),
        &headers,
        None,
    )
    .map_err(|e| e.replace(token, "[redacted]"))?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("Sentry answered {}", reply.status));
    }
    Ok(Some(excerpt(&reply.body)))
}

/// A Sentry event as text: its title or message, then each exception's type and value.
fn excerpt(event: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(title) = event["title"]
        .as_str()
        .or_else(|| event["message"].as_str())
    {
        lines.push(title.to_string());
    }
    for entry in event["entries"].as_array().into_iter().flatten() {
        if entry["type"] != json!("exception") {
            continue;
        }
        for value in entry["data"]["values"].as_array().into_iter().flatten() {
            lines.push(format!(
                "{}: {}",
                value["type"].as_str().unwrap_or("error"),
                value["value"].as_str().unwrap_or("")
            ));
        }
    }
    clip(&lines.join("\n"), 2000)
}

// ---- Text helpers ---------------------------------------------------------------------------------------------------

/// The first `max` characters of `text`.
fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// An RFC 3339 time (UTC, to the second) for a ms count.
fn stamp(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date of `days` since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_stamped_in_utc_to_the_second() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(1_792_835_892_000), "2026-10-24T09:58:12Z");
        assert_eq!(stamp(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn a_sentry_event_reads_its_title_and_exceptions() {
        let event = json!({
            "title": "TypeError: x",
            "entries": [
                { "type": "breadcrumbs", "data": {} },
                { "type": "exception", "data": { "values": [
                    { "type": "TypeError", "value": "Cannot read properties of undefined" }
                ] } }
            ]
        });
        assert_eq!(
            excerpt(&event),
            "TypeError: x\nTypeError: Cannot read properties of undefined"
        );
    }
}
