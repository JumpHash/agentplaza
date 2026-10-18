//! `projects maintenance`: a project's maintenance (`crates/apid/src/maintenance.rs`). In maintenance its watchers keep it:
//! they read the signals (feedback, reports, analytics, errors) and the map of the product's areas, sweep on a schedule
//! or when asked, and propose what to change. The owner approves a proposal (it becomes a task of the project), declines
//! it or puts it off. The hub's routes are under `/v1/projects/{id}/maintenance`.

use clap::Subcommand;
use serde_json::{Value, json};

use super::{array, dash};
use crate::ctx::{self, Ctx};

/// The kinds a signal takes (the hub's).
const SIGNAL_KINDS: [&str; 6] = ["feedback", "report", "analytics", "error", "usage", "note"];
/// The most characters a focus takes (the hub's).
const FOCUS_MAX: usize = 1000;
/// How many of the newest signals `maintenance ID` lists.
const SHOWN_SIGNALS: usize = 10;
/// The proposal table's columns.
const PROPOSAL_HEAD: [&str; 7] = ["id", "kind", "title", "value", "risk", "size", "state"];

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Sweep now: a watcher reads the signals and the map, and proposes what to change.
    Sweep,
    /// Set the focus the sweeps look at first, in words. `""` clears it.
    Focus {
        /// The focus, at most 1000 characters.
        text: String,
    },
    /// How often the sweeps run on their own, in hours. 0 turns the schedule off, so a sweep runs only when asked.
    Every {
        /// Hours, from 0 to 720.
        #[arg(value_parser = clap::value_parser!(u32).range(0..=720))]
        hours: u32,
    },
    /// Record a signal for the sweeps to read: feedback, a report, analytics, an error or a note.
    Signal {
        /// What the signal says.
        text: String,
        /// feedback, report, analytics, error, usage or note.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SIGNAL_KINDS))]
        kind: Option<String>,
        /// A short title for it.
        #[arg(long)]
        title: Option<String>,
        /// Who it is from.
        #[arg(long)]
        from: Option<String>,
        /// A link it is about.
        #[arg(long)]
        url: Option<String>,
        /// The environment it came from.
        #[arg(long = "env")]
        environment: Option<String>,
    },
    /// The proposals waiting for the owner (proposed or put off for later), or every proposal with --all.
    Proposals {
        /// Every proposal, decided or not.
        #[arg(long)]
        all: bool,
    },
    /// One proposal in full: its impact, value and why, effort, evidence and instrument.
    ShowProposal {
        /// The proposal's id, such as mp7.
        pid: String,
    },
    /// Approve a proposal, which makes it a task of the project. An instrument must first be set up in Connectors or
    /// MCP, so it makes no task yet.
    Approve {
        /// The proposal's id.
        pid: String,
        /// A note for the team.
        #[arg(long)]
        note: Option<String>,
        /// The task's budget in credits, at least 200. The project's default when it is not given.
        #[arg(long, value_parser = clap::value_parser!(u64).range(200..))]
        budget: Option<u64>,
        /// The role that does the task, such as builder.
        #[arg(long)]
        role: Option<String>,
    },
    /// Decline a proposal, with an optional reason.
    Decline {
        /// The proposal's id.
        pid: String,
        /// Why it is declined.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Put a proposal off for later. It keeps waiting for the owner.
    Later {
        /// The proposal's id.
        pid: String,
    },
    /// Open an address that signals can be sent to, shown once (a new one replaces the old). With --stop, turn it off.
    Ingest {
        /// Turn the address off.
        #[arg(long)]
        stop: bool,
    },
}

/// `projects maintenance ID`: what the project's watchers know and propose; the actions sweep it, change its focus,
/// schedule or signals, or decide a proposal.
pub fn run(ctx: &Ctx, id: &str, action: Option<Action>) -> Result<(), String> {
    let base = format!("/v1/projects/{}/maintenance", ctx::seg(id));
    let Some(action) = action else {
        let view = ctx.client.get(&base)?;
        ctx.out(&view, |v| print_view(id, v));
        return Ok(());
    };
    match action {
        Action::Sweep => {
            let started = ctx.client.post(&format!("{base}/sweep"), &json!({}))?;
            ctx.out(&started, |s| {
                println!(
                    "a sweep of project {id} is under way (work {})",
                    ctx::text(s, "work")
                )
            });
        }
        Action::Focus { text } => {
            let text = text.trim().to_string();
            if text.chars().count() > FOCUS_MAX {
                return Err(format!("a focus is at most {FOCUS_MAX} characters"));
            }
            let view = ctx.client.patch(&base, &json!({ "focus": text }))?;
            ctx.out(&view, |_| {
                if text.is_empty() {
                    println!("cleared the focus of project {id}");
                } else {
                    println!("the focus of project {id} is: {text}");
                }
            });
        }
        Action::Every { hours } => {
            let view = ctx.client.patch(&base, &json!({ "every_hours": hours }))?;
            ctx.out(&view, |_| {
                println!("project {id} is swept {}", schedule(u64::from(hours)))
            });
        }
        Action::Signal {
            text,
            kind,
            title,
            from,
            url,
            environment,
        } => {
            let body = signal_body(&text, [kind, title, from, url, environment])?;
            let signal = ctx.client.post(&format!("{base}/signals"), &body)?;
            ctx.out(&signal, |s| {
                println!(
                    "recorded signal {} ({}) on project {id}",
                    ctx::s(s, "id"),
                    ctx::s(s, "kind")
                )
            });
        }
        Action::Proposals { all } => {
            let view = ctx.client.get(&base)?;
            ctx.out(&view, |v| {
                ctx::table(&PROPOSAL_HEAD, &proposal_rows(array(&v["proposals"]), all));
            });
        }
        Action::ShowProposal { pid } => {
            let view = ctx.client.get(&base)?;
            let proposal = find_proposal(&view, id, &pid)?;
            ctx.out(proposal, print_proposal);
        }
        Action::Approve {
            pid,
            note,
            budget,
            role,
        } => {
            let body = approve_body(note.as_deref(), budget, role.as_deref());
            let p = ctx.client.post(
                &format!("{base}/proposals/{}/approve", ctx::seg(&pid)),
                &body,
            )?;
            ctx.out(&p, |p| println!("{}", approved_line(&pid, p)));
        }
        Action::Decline { pid, reason } => {
            let body = json!({ "reason": reason.unwrap_or_default() });
            let p = ctx.client.post(
                &format!("{base}/proposals/{}/decline", ctx::seg(&pid)),
                &body,
            )?;
            ctx.out(&p, |_| println!("declined proposal {pid}"));
        }
        Action::Later { pid } => {
            let p = ctx.client.post(
                &format!("{base}/proposals/{}/later", ctx::seg(&pid)),
                &json!({}),
            )?;
            ctx.out(&p, |_| println!("proposal {pid} is put off for later"));
        }
        Action::Ingest { stop } => {
            if stop {
                let off = ctx.client.delete(&format!("{base}/ingest"))?;
                ctx.out(&off, |_| {
                    println!("the ingest address of project {id} is off")
                });
            } else {
                let made = ctx.client.post(&format!("{base}/ingest"), &json!({}))?;
                ctx.out(&made, |m| print_ingest(ctx.client.base(), id, m));
            }
        }
    }
    Ok(())
}

/// When a project is swept, in words: on a schedule of every so many hours, or only when asked (0).
fn schedule(hours: u64) -> String {
    match hours {
        0 => "only when asked".to_string(),
        1 => "every hour".to_string(),
        n => format!("every {n} hours"),
    }
}

/// The `POST …/signals` body: the text (required), then kind, title, from, url and environment, each when it is given
/// and not blank.
fn signal_body(text: &str, details: [Option<String>; 5]) -> Result<Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("a signal needs text".into());
    }
    let mut body = json!({ "text": text });
    for (key, value) in ["kind", "title", "from", "url", "environment"]
        .into_iter()
        .zip(details)
    {
        if let Some(value) = value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            body[key] = json!(value);
        }
    }
    Ok(body)
}

/// The `POST …/approve` body: the note, budget and role, each when it is given.
fn approve_body(note: Option<&str>, budget: Option<u64>, role: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    if let Some(budget) = budget {
        body["budget"] = json!(budget);
    }
    if let Some(role) = role {
        body["role"] = json!(role);
    }
    body
}

/// What an approval says: the task it made, or the instrument to set up first, or that it is approved.
fn approved_line(pid: &str, p: &Value) -> String {
    let task = ctx::s(p, "task");
    if !task.is_empty() {
        let state = ctx::s(p, "task_state");
        return match state {
            "" => format!("approved proposal {pid}: it is task {task}"),
            state => format!("approved proposal {pid}: it is task {task} ({state})"),
        };
    }
    match ctx::s(&p["instrument"], "name") {
        "" => format!("approved proposal {pid}"),
        name => format!(
            "approved proposal {pid}: set up {name} in Connectors or MCP, and its task follows"
        ),
    }
}

/// Whether a proposal in `state` waits for the owner.
fn waiting(state: &str) -> bool {
    matches!(state, "proposed" | "later")
}

/// The proposals as table rows: id, kind, title, value (1 to 5), risk, size and state. Only the ones waiting for the
/// owner, unless `all`.
fn proposal_rows(proposals: &[Value], all: bool) -> Vec<Vec<String>> {
    proposals
        .iter()
        .filter(|p| all || waiting(ctx::s(p, "state")))
        .map(|p| {
            vec![
                ctx::s(p, "id").to_string(),
                ctx::s(p, "kind").to_string(),
                ctx::s(p, "title").to_string(),
                ctx::text(&p["value"], "score"),
                ctx::text(&p["impact"], "risk"),
                ctx::text(&p["effort"], "size"),
                ctx::s(p, "state").to_string(),
            ]
        })
        .collect()
}

/// The proposal `pid` of project `project`, from the maintenance view.
fn find_proposal<'a>(view: &'a Value, project: &str, pid: &str) -> Result<&'a Value, String> {
    array(&view["proposals"])
        .iter()
        .find(|p| ctx::s(p, "id") == pid)
        .ok_or_else(|| {
            format!("project {project} has no proposal {pid} (`projects maintenance {project} proposals --all` lists them)")
        })
}

/// The newest sweep's time and the first line of what it said; "never" before the first one.
fn last_sweep(sweep: &Value) -> String {
    if sweep["last_ms"].as_u64().unwrap_or(0) == 0 {
        return "never".to_string();
    }
    let ago = ctx::ago(&sweep["last_ms"]);
    match first_line(ctx::s(sweep, "last_summary")) {
        "" => ago,
        line => format!("{ago}: {line}"),
    }
}

/// The first line of a text, trimmed.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// A signal's title, or its text when it has none.
fn signal_title(signal: &Value) -> String {
    match ctx::s(signal, "title") {
        "" => ctx::s(signal, "text").to_string(),
        title => title.to_string(),
    }
}

/// The maintenance view in words: the focus and schedule, the last sweep, the counts, then the waiting proposals, the
/// map's areas and the newest signals.
fn print_view(id: &str, v: &Value) {
    let config = &v["config"];
    let waiting_rows = proposal_rows(array(&v["proposals"]), false);
    let areas = array(&v["map"]["areas"]);
    let mut pairs: Vec<(&str, String)> = vec![
        ("focus", dash(ctx::text(config, "focus"))),
        (
            "schedule",
            format!(
                "swept {}",
                schedule(config["every_hours"].as_u64().unwrap_or(0))
            ),
        ),
        ("last sweep", last_sweep(&v["sweep"])),
        (
            "signals",
            format!(
                "{} in all, {} not read by a sweep",
                ctx::text(v, "signals_total"),
                ctx::text(v, "unseen")
            ),
        ),
        ("proposals", format!("{} waiting", waiting_rows.len())),
        ("areas", areas.len().to_string()),
        (
            "ingest",
            if v["sources"]["ingest"].as_bool() == Some(true) {
                "on"
            } else {
                "off"
            }
            .to_string(),
        ),
    ];
    if v["sweep"]["running"].is_object() {
        pairs.push((
            "sweep",
            format!(
                "under way (work {})",
                ctx::text(&v["sweep"]["running"], "work")
            ),
        ));
    }
    println!("project {id}");
    ctx::fields(&pairs);

    println!("\nWaiting proposals ({})", waiting_rows.len());
    ctx::table(&PROPOSAL_HEAD, &waiting_rows);

    println!("\nAreas ({})", areas.len());
    let area_rows: Vec<Vec<String>> = areas
        .iter()
        .map(|a| {
            vec![
                ctx::s(a, "health").to_string(),
                ctx::s(a, "id").to_string(),
                ctx::s(a, "name").to_string(),
            ]
        })
        .collect();
    ctx::table(&["health", "id", "name"], &area_rows);

    let signals = array(&v["signals"]);
    println!(
        "\nSignals (the newest {} of {})",
        signals.len().min(SHOWN_SIGNALS),
        ctx::text(v, "signals_total")
    );
    let signal_rows: Vec<Vec<String>> = signals
        .iter()
        .take(SHOWN_SIGNALS)
        .map(|s| {
            vec![
                ctx::s(s, "id").to_string(),
                ctx::s(s, "kind").to_string(),
                ctx::when(&s["at_ms"]),
                dash(ctx::text(s, "from")),
                signal_title(s),
            ]
        })
        .collect();
    ctx::table(&["id", "kind", "when", "from", "title"], &signal_rows);
}

/// One proposal in full, as fields and lists.
fn print_proposal(p: &Value) {
    let value = &p["value"];
    let impact = &p["impact"];
    let effort = &p["effort"];
    let areas: Vec<&str> = array(&impact["areas"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let budget = match ctx::text(effort, "budget").as_str() {
        "" => "the project's default".to_string(),
        budget => format!("{budget} credits"),
    };
    let mut pairs: Vec<(&str, String)> = vec![
        ("id", ctx::text(p, "id")),
        ("kind", ctx::text(p, "kind")),
        ("state", ctx::text(p, "state")),
        ("title", ctx::text(p, "title")),
        ("area", dash(ctx::text(p, "area"))),
        (
            "by",
            format!("{} (cid {})", ctx::text(p, "by"), ctx::text(p, "by_cid")),
        ),
        ("summary", dash(ctx::text(p, "summary"))),
        (
            "value",
            format!(
                "{} of 5: {}",
                ctx::text(value, "score"),
                dash(ctx::text(value, "why"))
            ),
        ),
        (
            "impact",
            format!(
                "{} for {}; areas {}",
                dash(ctx::text(impact, "changes")),
                dash(ctx::text(impact, "users")),
                dash(areas.join(", "))
            ),
        ),
        (
            "risk",
            format!(
                "{}: {}",
                ctx::text(impact, "risk"),
                dash(ctx::text(impact, "risk_note"))
            ),
        ),
        (
            "effort",
            format!(
                "{} with {budget}: {}",
                ctx::text(effort, "size"),
                dash(ctx::text(effort, "note"))
            ),
        ),
        ("role", dash(ctx::text(p, "role"))),
        ("acceptance", dash(ctx::text(p, "acceptance"))),
        ("note", dash(ctx::text(p, "note"))),
    ];
    if p["instrument"].is_object() {
        let instrument = &p["instrument"];
        pairs.push((
            "instrument",
            format!(
                "{} {}{}: {}",
                ctx::text(instrument, "kind"),
                ctx::text(instrument, "name"),
                match ctx::s(instrument, "catalog") {
                    "" => String::new(),
                    catalog => format!(" (catalog {catalog})"),
                },
                ctx::text(instrument, "data")
            ),
        ));
    }
    if !ctx::s(p, "task").is_empty() {
        pairs.push((
            "task",
            format!(
                "{} ({})",
                ctx::text(p, "task"),
                dash(ctx::text(p, "task_state"))
            ),
        ));
    }
    ctx::fields(&pairs);

    let evidence = array(&p["evidence"]);
    println!("\nEvidence ({})", evidence.len());
    for e in evidence {
        println!(
            "  {}  {}",
            dash(ctx::text(e, "signal")),
            ctx::text(e, "note")
        );
    }
}

/// The address a signal is sent to: the hub's base URL and the path, and how to send to it.
fn print_ingest(base: &str, id: &str, made: &Value) {
    let url = format!("{base}{}", ctx::s(made, "path"));
    println!("signals for project {id} go to this address (shown once: a new one replaces it):");
    println!("  {url}");
    println!("send one as text, with curl:");
    println!("  curl -X POST --data-binary 'what happened' '{url}'");
    println!(
        "or as JSON, with the text in a text field and any of kind, title, from, url, environment."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_needs_text_and_carries_only_the_details_given() {
        assert!(signal_body("  ", [None, None, None, None, None]).is_err());
        let body = signal_body(
            " checkout fails ",
            [
                Some("error".into()),
                None,
                Some("  ".into()),
                None,
                Some("prod".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            body,
            json!({ "text": "checkout fails", "kind": "error", "environment": "prod" })
        );
    }

    #[test]
    fn a_schedule_reads_in_words() {
        assert_eq!(schedule(0), "only when asked");
        assert_eq!(schedule(1), "every hour");
        assert_eq!(schedule(24), "every 24 hours");
    }

    #[test]
    fn approve_sends_only_what_is_given() {
        assert_eq!(approve_body(None, None, None), json!({}));
        assert_eq!(
            approve_body(Some("ship it"), Some(500), None),
            json!({ "note": "ship it", "budget": 500 })
        );
    }

    #[test]
    fn the_waiting_proposals_are_the_proposed_and_the_later_ones() {
        let proposals = json!([
            { "id": "mp1", "kind": "fix", "title": "a", "state": "proposed", "value": { "score": 4 },
              "impact": { "risk": "low" }, "effort": { "size": "s" } },
            { "id": "mp2", "kind": "chore", "title": "b", "state": "later" },
            { "id": "mp3", "kind": "feature", "title": "c", "state": "approved" }
        ]);
        let list = array(&proposals);
        let waiting_ids: Vec<String> = proposal_rows(list, false)
            .iter()
            .map(|r| r[0].clone())
            .collect();
        assert_eq!(waiting_ids, ["mp1", "mp2"]);
        assert_eq!(proposal_rows(list, true).len(), 3);
        assert_eq!(proposal_rows(list, false)[0][3..6], ["4", "low", "s"]);
    }

    #[test]
    fn an_approval_names_the_task_or_the_instrument_to_set_up() {
        let made = json!({ "id": "mp1", "task": "t42", "task_state": "queued" });
        assert_eq!(
            approved_line("mp1", &made),
            "approved proposal mp1: it is task t42 (queued)"
        );
        let instrument = json!({ "id": "mp2", "task": "", "instrument": { "name": "Sentry" } });
        assert_eq!(
            approved_line("mp2", &instrument),
            "approved proposal mp2: set up Sentry in Connectors or MCP, and its task follows"
        );
    }

    #[test]
    fn the_last_sweep_says_never_before_the_first() {
        assert_eq!(
            last_sweep(&json!({ "last_ms": 0, "last_summary": "" })),
            "never"
        );
        assert_eq!(first_line("first\nsecond"), "first");
    }
}
