//! The incident recipe's four steps (PLAN 24.1, 12.1): triage, act, tell and after. The run is the engine's (`recipes::runs`)
//! and each step is settled as it ends; the steps are plain code, because the work is the hub's own: the deploy path, the
//! task planner, the chat, Weft's docs and project memory. The outside reads are ports: the logs a source's connector can
//! fetch ([`Logs`]), the project's notify connector ([`Notify`]) and the recipe's model, which distils the postmortem into
//! memory proposals ([`Model`]; the template of [`template_lessons`] is the fallback when no model answers). Production
//! answers the ports from the connectors (Sentry's latest event, Loki's query_range, Telegram's `send`) where they are
//! connected; a test gives its own.

use std::collections::BTreeMap;
use std::sync::Arc;

use apz_gateway::Gateway;
use apz_proto::chat::{Block, ChatRequest, Message, Role, blocks_text};
use serde_json::{Value, json};

use super::{DEPLOY_HEALTH, Incident, Landing, RECIPE, Step};
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

/// The logs a source's connector can fetch for an incident: a text excerpt, or None when none is connected for it.
pub trait Logs: Send + Sync {
    fn fetch(&self, incident: &Incident) -> Result<Option<String>, String>;
}

/// The project's notify connector: a notice sent out of the hub, when one is connected.
pub trait Notify: Send + Sync {
    fn send(&self, project: &str, text: &str) -> Result<(), String>;
}

/// The recipe's model: asked to distil a postmortem, for `project`. Ok(None) (or an error) means no model answered.
pub trait Model: Send + Sync {
    fn answer(&self, project: &str, prompt: &str) -> Result<Option<String>, String>;
}

/// The most lessons a postmortem proposes.
const MAX_LESSONS: usize = 5;
/// A lesson's text and its why, within the memory's limits.
const LESSON_MAX: usize = 2000;
const WHY_MAX: usize = 500;

/// Runs incident `id` through its recipe: triage, act, tell, after. Returns the act's outcome.
pub fn run(app: &Arc<App>, id: &str, trigger: Trigger) -> String {
    let ports = Connected {
        app: Arc::clone(app),
    };
    run_with(app, id, trigger, &ports, &ports, &ports)
}

/// [`run`] with the outside reads given (a test's fakes; production passes [`Connected`]).
pub fn run_with(
    app: &Arc<App>,
    id: &str,
    trigger: Trigger,
    logs: &dyn Logs,
    notify: &dyn Notify,
    model: &dyn Model,
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
    // The incident traces back to a landing's task: that task's eval records count it (25.1).
    if let Some((landing, _)) = &triaged.landing
        && !landing.task.is_empty()
        && let Err(err) = app.evals.incident(&landing.task)
    {
        eprintln!(
            "apid: evals: incident {id} was not recorded for {}: {err}",
            landing.task
        );
    }
    let suspect = triaged
        .suspect
        .as_ref()
        .map(|s| format!("{}@{}", s.env, s.version))
        .or_else(|| {
            triaged
                .landing
                .as_ref()
                .map(|(l, _)| format!("landing {}", l.change))
        });
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
    let (after_ok, after_text) = after(app, &incident, &acted, model);
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

/// What the triage found: the suspect deploy, the landing that correlates (with why), the notes on what was correlated,
/// and the logs fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triage {
    pub suspect: Option<Suspect>,
    pub landing: Option<(Landing, String)>,
    pub notes: Vec<String>,
    pub logs: Option<String>,
}

/// Correlates the incident with the deploys, the landings (the changes that landed before it, their files and areas), the
/// releases and the signals before it started, and reads the logs the source's connector can fetch.
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
            landing: None,
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
    let fetched = match logs.fetch(incident) {
        Ok(Some(text)) => {
            notes.push(format!(
                "The logs ({}) are in the incident.",
                incident.source
            ));
            Some(text)
        }
        Ok(None) => {
            notes.push("No logs: nothing connected fetches them for this source here.".into());
            None
        }
        Err(why) => {
            notes.push(format!("The logs could not be fetched: {why}."));
            None
        }
    };
    let landing = correlate_landing(app, incident, fetched.as_deref());
    match &landing {
        Some((l, why)) => notes.push(format!(
            "Change {} (\"{}\") by {} landed {} minute(s) before the alert and {why}; it touched {}.",
            l.change,
            l.title,
            l.author,
            started.saturating_sub(l.at_ms) / 60_000,
            if l.files.is_empty() {
                "no files it can read".to_string()
            } else {
                l.files.join(", ")
            }
        )),
        None => notes.push("No landing in the hour before the alert.".into()),
    }
    Triage {
        suspect: best,
        landing,
        notes,
        logs: fetched,
    }
}

/// The landing of the hour before the alert that fits it best: one whose file the alert's text or logs name, then one in
/// an area they name, then the latest of those that landed in the window.
fn correlate_landing(
    app: &App,
    incident: &Incident,
    logs: Option<&str>,
) -> Option<(Landing, String)> {
    let started = incident.started_ms;
    let from = started.saturating_sub(super::CORRELATE_MS);
    let until = started + super::SLACK_MS;
    let text = format!(
        "{} {} {}",
        incident.title,
        incident.detail,
        logs.unwrap_or("")
    );
    app.incidents
        .landings_of(&incident.project)
        .into_iter()
        .filter(|l| l.at_ms >= from && l.at_ms <= until)
        .map(|l| {
            let (rank, why) = match_landing(&text, &l);
            (rank, l, why)
        })
        .max_by(|a, b| (a.0, a.1.at_ms).cmp(&(b.0, b.1.at_ms)))
        .map(|(_, l, why)| (l, why))
}

/// How a landing fits an alert's text: rank 2 when the text names one of its files (or its basename), 1 when it names one
/// of its areas, 0 for a landing in the window alone. The reason says which.
pub fn match_landing(text: &str, landing: &Landing) -> (u8, String) {
    let text = text.to_lowercase();
    if let Some(file) = landing.files.iter().find(|f| {
        let f = f.to_lowercase();
        let base = f.rsplit('/').next().unwrap_or("");
        text.contains(&f) || (base.len() >= 4 && text.contains(base))
    }) {
        return (2, format!("touched {file}, which the alert names"));
    }
    if let Some(area) = landing
        .areas
        .iter()
        .find(|a| !a.is_empty() && text.contains(&a.to_lowercase()))
    {
        return (1, format!("is in the {area} area the alert names"));
    }
    (0, "landed in the window before it".into())
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

/// A correlating deploy is rolled back through the deploy path (the previous release is deployed again, with its stages
/// and checks, and a protected environment waits for the owner). Without one, a fix task is opened: naming the landing
/// that correlates, when there is one.
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

/// Opens a fix task with a spec drawn from the alert, its triage, its logs and the landing that correlates.
fn fix_task(app: &Arc<App>, incident: &Incident, triage: &Triage, why: &str) -> Acted {
    let logs = triage.logs.as_deref().unwrap_or("(no logs were fetched)");
    let suspect_change = match &triage.landing {
        Some((l, reason)) => format!(
            "Suspect change: {} \"{}\" by {} (task {}, session {}), which {reason}. Its files: {}.",
            l.change,
            l.title,
            l.author,
            if l.task.is_empty() { "none" } else { &l.task },
            if l.session.is_empty() {
                "none"
            } else {
                &l.session
            },
            if l.files.is_empty() {
                "none readable".to_string()
            } else {
                l.files.join(", ")
            }
        ),
        None => "Suspect change: none correlates.".to_string(),
    };
    let goal = format!(
        "Fix the cause of incident {id}: {title} ({severity}, from {source}).\n\n{detail}\n\nLinks:\n{links}\n\nTriage:\n{notes}\n\n{suspect_change}\n\nLogs:\n{logs}\n\nNo rollback was made: {why}.",
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
            let named = triage
                .landing
                .as_ref()
                .map(|(l, _)| format!("; suspect change {}", l.change))
                .unwrap_or_default();
            Acted {
                outcome: "fix_task".into(),
                detail: format!("Opened fix task {task}: {why}{named}."),
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

/// A lesson the postmortem proposes into the project's memory: its type (`pitfall` or `decision`), its text and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub kind: String,
    pub text: String,
    pub why: String,
}

/// Writes the postmortem to the project's docs in Weft and proposes its lessons into the project's memory. The lessons
/// are the model's, when it answers with some; otherwise the template's. Returns whether the postmortem was written, and
/// the step's text.
fn after(app: &App, incident: &Incident, acted: &Acted, model: &dyn Model) -> (bool, String) {
    let path = format!("docs/incidents/{}.md", incident.id);
    let draft = postmortem(incident, acted, &[]);
    let (lessons, source) = lessons_for(incident, acted, model, &draft);
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
        .map(|lesson| {
            let why = if lesson.why.is_empty() {
                format!("Lesson of incident {}", incident.id)
            } else {
                lesson.why.clone()
            };
            app.project_memory
                .add_proposal(project_memory::Proposal {
                    id: String::new(),
                    project: incident.project.clone(),
                    kind: lesson.kind.clone(),
                    text: lesson.text.clone(),
                    anchors: Vec::new(),
                    why: format!("{why} (distilled by {source})"),
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
    let proposed = format!(
        "{} lesson(s) proposed to memory, distilled by {source}",
        ids.len()
    );
    match written {
        Ok(()) => (true, format!("Postmortem at {path}; {proposed}.")),
        Err(why) => (
            false,
            format!("The postmortem was not written ({why}); {proposed}."),
        ),
    }
}

/// The lessons of a postmortem: the model's answer when it gives valid ones (the source is `the model`), else the template
/// (`the template`). The prompt is the postmortem's draft and the format the memory takes.
pub fn lessons_for(
    incident: &Incident,
    acted: &Acted,
    model: &dyn Model,
    draft: &str,
) -> (Vec<Lesson>, &'static str) {
    let prompt = format!(
        "ROLE: incident distil\n{draft}\n\nDistil the lessons of this postmortem into at most {MAX_LESSONS} memory proposals for the project. \
         Reply with only a JSON array of objects, each with \"kind\" (\"pitfall\" for what went wrong, \"decision\" for a \
         rule the project follows), \"text\" (one or two sentences) and \"why\" (one sentence)."
    );
    match model.answer(&incident.project, &prompt) {
        Ok(Some(answer)) => match parse_lessons(&answer) {
            Some(lessons) => (lessons, "the model"),
            None => (template_lessons(incident, acted), "the template"),
        },
        _ => (template_lessons(incident, acted), "the template"),
    }
}

/// A model's answer as lessons: the JSON array in it, each item with a kind of `pitfall` or `decision` and a text within
/// the memory's limits. Items that do not fit are dropped; no valid item is None.
pub fn parse_lessons(answer: &str) -> Option<Vec<Lesson>> {
    let start = answer.find('[')?;
    let end = answer.rfind(']')?;
    if end < start {
        return None;
    }
    let items: Vec<Value> = serde_json::from_str(&answer[start..=end]).ok()?;
    let lessons: Vec<Lesson> = items
        .iter()
        .filter_map(|item| {
            let kind = item["kind"].as_str()?.trim();
            if !matches!(kind, "pitfall" | "decision") {
                return None;
            }
            let text = item["text"].as_str()?.trim();
            let why = item["why"].as_str().unwrap_or("").trim();
            if text.is_empty() || text.chars().count() > LESSON_MAX || why.chars().count() > WHY_MAX
            {
                return None;
            }
            Some(Lesson {
                kind: kind.to_string(),
                text: text.to_string(),
                why: why.to_string(),
            })
        })
        .take(MAX_LESSONS)
        .collect();
    (!lessons.is_empty()).then_some(lessons)
}

/// The template's lessons, for when no model answers: a pitfall for what went wrong, and for a rollback the rule it
/// followed.
pub fn template_lessons(incident: &Incident, acted: &Acted) -> Vec<Lesson> {
    let env = if incident.environment.is_empty() {
        "the service".to_string()
    } else {
        incident.environment.clone()
    };
    let pitfall = |text: String| Lesson {
        kind: "pitfall".into(),
        text,
        why: String::new(),
    };
    match acted.outcome.as_str() {
        "rollback" | "rollback_waiting" => vec![
            pitfall(format!(
                "{}: {env} ran a deploy that fired the alert; it was rolled back to {} while the cause was found. Check its signals before the next promotion.",
                incident.title, acted.rollback
            )),
            Lesson {
                kind: "decision".into(),
                text: format!(
                    "An alert that correlates with a deploy is answered by rolling that deploy back first, through the deploy path (incident {}).",
                    incident.id
                ),
                why: String::new(),
            },
        ],
        "rolled_back" => vec![pitfall(format!(
            "{}: the health window caught the breach and rolled the release back on its own; the breach numbers are in the incident card.",
            incident.title
        ))],
        "fix_task" => vec![pitfall(format!(
            "{} on {env} had no correlating deploy, so its cause is in the code or its configuration; fix task {} carries the alert and the logs.",
            incident.title, acted.task
        ))],
        _ => vec![pitfall(format!("{}: {}", incident.title, incident.detail))],
    }
}

/// The postmortem's text: the alert, the timeline of the steps, the correlation, the logs, the action and the lessons.
fn postmortem(incident: &Incident, acted: &Acted, lessons: &[Lesson]) -> String {
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
            "\n## Correlation\n\nThe suspect: {}.\n",
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
    if !lessons.is_empty() {
        out.push_str("\n## Lessons\n\n");
        for lesson in lessons {
            out.push_str(&format!("- {}: {}\n", lesson.kind, lesson.text));
        }
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

/// The production ports. Logs: Sentry's latest event through the `sentry` connector, and Loki's lines for a Grafana or
/// Alertmanager alert when its environment names the `loki` connector. Notify: `send` through the `telegram` connector,
/// when it is enabled. Model: no hub-side model call is wired for one-shot completions yet, so no model answers and the
/// template is written (the port is what a model plugs into; tests give a mock).
pub struct Connected {
    app: Arc<App>,
}

impl Logs for Connected {
    fn fetch(&self, incident: &Incident) -> Result<Option<String>, String> {
        match incident.source.as_str() {
            "sentry" => sentry_latest(&self.app, &incident.fingerprint),
            "grafana" | "alertmanager" => loki_lines(&self.app, incident),
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

impl Model for Connected {
    fn answer(&self, project: &str, prompt: &str) -> Result<Option<String>, String> {
        let Some(model) = distil_model(&self.app, project) else {
            return Ok(None);
        };
        let gateway = self.app.providers.gateway();
        GatewayModel {
            gateway: &gateway,
            model,
        }
        .answer(project, prompt)
    }
}

/// The model a project's incident is distilled on, as `provider/model`: the project's own model for the `incident` role,
/// else the incident recipe's `after` step's model when it names one, else the project's default model. None when none
/// of them names a model.
pub fn distil_model(app: &App, project: &str) -> Option<String> {
    let settings = app.project_settings.get(project);
    let recipe = app.recipe_runs.recipe(RECIPE, None).and_then(|r| {
        r.steps
            .iter()
            .find(|s| s.id == "after")
            .and_then(|s| match &s.model {
                crate::recipes::model::Model::Name(name) => Some(name.clone()),
                crate::recipes::model::Model::List(_) => None,
            })
    });
    choose_model(
        settings.role_models.get("incident").map(String::as_str),
        recipe.as_deref(),
        settings.default_model.as_deref(),
    )
}

/// The first of the three that names a `provider/model`.
pub fn choose_model(
    role: Option<&str>,
    recipe: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    [role, recipe, default]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|m| {
            m.split_once('/')
                .is_some_and(|(p, name)| !p.is_empty() && !name.is_empty())
        })
        .map(str::to_string)
}

/// The system line of the distillation: the answer is a JSON array and nothing else.
const SYSTEM: &str = "You distil the lessons of an incident into a project's memory. Answer with one JSON array and nothing else.";

/// A model reached through the gateway (the hub's providers, as user testing reaches its planner): the prompt as one user
/// message, the answer's text. Its failure is the error (the template then answers).
pub struct GatewayModel<'a> {
    pub gateway: &'a Gateway,
    pub model: String,
}

impl Model for GatewayModel<'_> {
    fn answer(&self, _project: &str, prompt: &str) -> Result<Option<String>, String> {
        let req = ChatRequest {
            model: self.model.clone(),
            system: Some(SYSTEM.to_string()),
            system_parts: Vec::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![Block::Text {
                    text: prompt.to_string(),
                }],
            }],
            tools: Vec::new(),
            cache_last_message: false,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        let reply = self
            .gateway
            .complete(&req, &mut |_| {})
            .map_err(|e| format!("{}: {e}", self.model))?;
        Ok(Some(blocks_text(&reply.content)))
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

/// The lines Loki holds for an alert's service over its window (`query_range`, the 10 minutes before the alert to a
/// minute after it), when the incident's environment names the `loki` connector. The token is a bearer header only.
fn loki_lines(app: &App, incident: &Incident) -> Result<Option<String>, String> {
    let Some(env) = app
        .environments
        .of(&incident.project)
        .into_iter()
        .find(|e| e.name == incident.environment)
    else {
        return Ok(None);
    };
    if env.variables.get(super::LOGS_VAR).map(String::as_str) != Some("loki") {
        return Ok(None);
    }
    let values = crate::connectors::hub_values(app, "loki")?;
    let base = values
        .get("url")
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .ok_or("the loki connector has no API address (LOKI_URL)")?;
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err("the Loki connector's address is an http or https address".into());
    }
    let service = env
        .variables
        .get(super::SERVICE_VAR)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the environment names no service to query (ALERT_SERVICE)")?;
    let token = values.get("token").map(String::as_str).unwrap_or("").trim();
    let query = format!("{{service=\"{service}\"}}");
    let start = incident.started_ms.saturating_sub(10 * 60_000) * 1_000_000;
    let end = (incident.started_ms + super::SLACK_MS) * 1_000_000;
    let url = format!(
        "{base}/loki/api/v1/query_range?query={}&start={start}&end={end}&limit=20&direction=backward",
        percent(&query)
    );
    let headers: Vec<(&str, String)> = if token.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization", format!("Bearer {token}"))]
    };
    let http = crate::platforms::http::Ureq;
    let reply = crate::platforms::http::Http::call(&http, "GET", &url, &headers, None)
        .map_err(|e| e.replace(token, "[redacted]"))?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("Loki answered {}", reply.status));
    }
    let lines = loki_text(&reply.body);
    Ok((!lines.is_empty()).then(|| clip(&lines.join("\n"), 2000)))
}

/// The log lines of a Loki `query_range` answer: each stream's values, as their text.
pub fn loki_text(answer: &Value) -> Vec<String> {
    answer["data"]["result"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|stream| stream["values"].as_array().into_iter().flatten())
        .filter_map(|pair| pair[1].as_str().map(str::to_string))
        .take(20)
        .collect()
}

/// Percent-encodes everything but the unreserved characters.
fn percent(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
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

    /// A model that answers what it was given (or nothing).
    struct Mock(Option<String>);

    impl Model for Mock {
        fn answer(&self, _project: &str, _prompt: &str) -> Result<Option<String>, String> {
            Ok(self.0.clone())
        }
    }

    fn landing(files: &[&str], areas: &[&str]) -> Landing {
        Landing {
            key: "site/c1".into(),
            project: "site".into(),
            change: "c1".into(),
            title: "Checkout".into(),
            author: "owner".into(),
            task: String::new(),
            session: String::new(),
            at_ms: 1,
            files: files.iter().map(|s| s.to_string()).collect(),
            areas: areas.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn incident() -> Incident {
        Incident {
            id: "i1".into(),
            project: "site".into(),
            environment: "production".into(),
            source: "sentry".into(),
            fingerprint: "sentry:1".into(),
            severity: "high".into(),
            title: "TypeError".into(),
            detail: String::new(),
            status: "open".into(),
            started_ms: 1,
            resolved_ms: 0,
            last_ms: 1,
            seen: 1,
            links: Vec::new(),
            suspect: String::new(),
            outcome: "fix_task".into(),
            rollback: String::new(),
            task: "t1".into(),
            postmortem: String::new(),
            lessons: Vec::new(),
            logs: String::new(),
            steps: Vec::new(),
        }
    }

    fn acted() -> Acted {
        Acted {
            outcome: "fix_task".into(),
            detail: "Opened fix task t1".into(),
            rollback: String::new(),
            task: "t1".into(),
            ok: true,
        }
    }

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

    #[test]
    fn a_loki_answer_reads_its_lines_across_streams() {
        let answer = json!({ "status": "success", "data": { "resultType": "streams", "result": [
            { "stream": { "service": "site-web" }, "values": [["1", "placeOrder: stock 0"], ["2", "retry"]] },
            { "stream": { "service": "site-web", "level": "error" }, "values": [["3", "checkout failed"]] }
        ] } });
        assert_eq!(
            loki_text(&answer),
            ["placeOrder: stock 0", "retry", "checkout failed"]
        );
        assert!(loki_text(&json!({ "data": { "result": [] } })).is_empty());
        assert_eq!(
            percent("{service=\"site-web\"}"),
            "%7Bservice%3D%22site-web%22%7D"
        );
    }

    #[test]
    fn a_file_the_alert_names_is_a_stronger_landing_than_an_area_or_the_window() {
        let l = landing(&["src/checkout.ts", "docs/readme.md"], &["src", "docs"]);
        let (rank, why) = match_landing("TypeError in src/checkout.ts in placeOrder", &l);
        assert_eq!(rank, 2);
        assert!(why.contains("src/checkout.ts"), "{why}");
        // The basename is enough when the path is shortened in the text.
        assert_eq!(match_landing("error in checkout.ts", &l).0, 2);
        assert_eq!(match_landing("the docs area is quiet", &l).0, 1);
        assert_eq!(match_landing("unrelated", &l).0, 0);
    }

    #[test]
    fn a_model_answer_becomes_lessons_and_a_bad_one_falls_back_to_the_template() {
        let good = "Here you go: [{\"kind\":\"pitfall\",\"text\":\"Checkout reads a missing stock row.\",\"why\":\"from the logs\"},\
                    {\"kind\":\"decision\",\"text\":\"Roll back before fixing.\",\"why\":\"\"},\
                    {\"kind\":\"opinion\",\"text\":\"dropped: unknown kind\"}]";
        let lessons = parse_lessons(good).expect("two valid lessons");
        assert_eq!(lessons.len(), 2);
        assert_eq!(lessons[1].kind, "decision");
        assert!(parse_lessons("no array here").is_none());
        assert!(
            parse_lessons("[{\"kind\":\"opinion\",\"text\":\"x\"}]").is_none(),
            "no valid item"
        );

        let (from_model, source) =
            lessons_for(&incident(), &acted(), &Mock(Some(good.into())), "draft");
        assert_eq!(source, "the model");
        assert_eq!(from_model, lessons);

        let (garbled, source) = lessons_for(
            &incident(),
            &acted(),
            &Mock(Some("I cannot help".into())),
            "draft",
        );
        assert_eq!(source, "the template", "a garbled answer falls back");
        assert_eq!(garbled, template_lessons(&incident(), &acted()));

        let (none, source) = lessons_for(&incident(), &acted(), &Mock(None), "draft");
        assert_eq!(source, "the template", "no model answering falls back");
        assert_eq!(none, template_lessons(&incident(), &acted()));
        assert!(!none.is_empty());
    }

    /// The gateway with its `mock` provider, answering from the script the apid tests share (user testing's).
    fn gateway() -> Gateway {
        crate::user_test::tests::script();
        Gateway::builder()
            .providers(vec![apz_gateway::ProviderConfig::new(
                "mock",
                apz_gateway::ProviderKind::Mock,
            )])
            .build()
    }

    #[test]
    fn the_production_port_asks_the_gateway_and_its_answer_becomes_the_lessons() {
        let gateway = gateway();
        let port = GatewayModel {
            gateway: &gateway,
            model: "mock/script".into(),
        };
        let (lessons, source) =
            lessons_for(&incident(), &acted(), &port, "# Incident i1: TypeError");
        assert_eq!(
            source, "the model",
            "the gateway's mock model distilled the postmortem"
        );
        assert_eq!(lessons.len(), 2);
        assert_eq!(lessons[0].kind, "pitfall");
        assert!(lessons[0].text.contains("stock row"), "{lessons:?}");
        assert_eq!(lessons[1].kind, "decision");
    }

    #[test]
    fn a_failed_gateway_call_falls_back_to_the_template() {
        let gateway = gateway();
        let port = GatewayModel {
            gateway: &gateway,
            model: "missing/model".into(),
        };
        assert!(
            port.answer("site", "draft").is_err(),
            "an unknown provider is an error"
        );
        let (lessons, source) = lessons_for(&incident(), &acted(), &port, "draft");
        assert_eq!(source, "the template");
        assert_eq!(lessons, template_lessons(&incident(), &acted()));
    }

    #[test]
    fn the_distillation_model_is_the_first_named_provider_model() {
        assert_eq!(
            choose_model(Some("openai/gpt"), Some("mock/script"), Some("x/y")).as_deref(),
            Some("openai/gpt"),
            "the project's incident role first"
        );
        assert_eq!(
            choose_model(None, Some("mock/script"), Some("x/y")).as_deref(),
            Some("mock/script")
        );
        assert_eq!(
            choose_model(Some("bad"), Some("any"), Some("x/y")).as_deref(),
            Some("x/y")
        );
        assert_eq!(
            choose_model(Some("  "), None, None),
            None,
            "nothing named: no model"
        );
    }

    #[test]
    fn the_model_is_asked_for_the_memory_format_with_the_draft() {
        struct Asked(std::sync::Mutex<String>);
        impl Model for Asked {
            fn answer(&self, _project: &str, prompt: &str) -> Result<Option<String>, String> {
                *self.0.lock().unwrap() = prompt.to_string();
                Ok(None)
            }
        }
        let asked = Asked(std::sync::Mutex::new(String::new()));
        let _ = lessons_for(&incident(), &acted(), &asked, "# Incident i1: TypeError");
        let prompt = asked.0.lock().unwrap().clone();
        assert!(
            prompt.starts_with("ROLE: incident distil\n# Incident i1: TypeError"),
            "{prompt}"
        );
        assert!(
            prompt.contains("\"kind\"")
                && prompt.contains("pitfall")
                && prompt.contains("decision")
        );
    }
}
