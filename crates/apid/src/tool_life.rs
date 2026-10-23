//! The lifecycle of agent-built tools past the library's store (PLAN 14.1, 14.2): the planner's approval and the owner's
//! asks (a connector card for a tool that uses a secret, a `promote` card, a retirement card), the notices the project's
//! and the orchestrator's chats get, the glue tier's runs on the hub, and a replacement's shadow, trial and all.
//!
//! The version states, their evidence and the regression rule are in `library.rs`; this module asks and tells.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

use crate::cards::{Card, Choice, Kind, NewCard};
use crate::connectors;
use crate::glue;
use crate::library::{self, Comparison, Entry, PREFIX};
use crate::state::App;

fn now_ms() -> u64 {
    apz_plaza::now_ms()
}

/// The transport a hub-run tool's requests go through: the approved egress, over HTTPS.
fn outbound() -> Arc<dyn glue::Outbound> {
    Arc::new(glue::HttpsOutbound)
}

/// Runs glue-tier version `entry` on the hub with `input`: `(is_error, output, ms)`. The component gets the connector's
/// key-value state and its egress, and its requests carry the vaulted key (`connectors::Store::run_glue`).
pub fn run_candidate(
    connectors: &connectors::Store,
    library: &library::Store,
    entry: &Entry,
    input: &Value,
    outbound: Arc<dyn glue::Outbound>,
) -> (bool, String, u64) {
    let Some(component) = library.component(&entry.name, entry.version) else {
        return (true, format!("{PREFIX}{} has no component", entry.name), 0);
    };
    let started = Instant::now();
    let result = connectors.run_glue(
        &entry.connector,
        &component,
        "run",
        &input.to_string(),
        outbound,
    );
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(out) => (false, out.output, ms),
        Err(why) => (true, why, ms),
    }
}

/// A call of a glue-tier library tool the hub runs itself: `(is_error, output)`, or `None` when the visible tool runs in a
/// fork (or is not visible to the caller).
pub fn glue_call(app: &App, cid: u64, name: &str, input: &Value) -> Option<(bool, String)> {
    let project = crate::projects::project_of(app, cid);
    let entry = app.library.visible(cid, project.as_deref(), name)?;
    if entry.connector.is_empty() {
        return None;
    }
    let (is_error, output, ms) =
        run_candidate(&app.connectors, &app.library, &entry, input, outbound());
    app.library
        .measure_version(&entry.name, entry.version, !is_error, ms);
    app.evals.tool(cid, name, ms, !is_error);
    if let Some(current) = app.library.find(&entry.name, entry.version) {
        check_harm(app, &current);
    }
    Some((is_error, output))
}

/// The outcome of one call a replacement served or was compared on: the comparison to record and the result to return.
/// `builtin` is the built-in's run when it ran (a shadow runs it first; a trial runs it when the candidate failed).
/// A shadow returns the built-in's result; a trial returns the candidate's, or the built-in's when the candidate failed.
pub fn judge(
    shadow: bool,
    builtin: Option<(bool, String, u64)>,
    candidate: (bool, String, u64),
    at_ms: u64,
) -> (Comparison, (bool, String)) {
    let (cand_err, cand_out, cand_ms) = candidate;
    let cand_ok = !cand_err;
    let (builtin_ran, b_err, b_out, b_ms) = match &builtin {
        Some((err, out, ms)) => (true, *err, out.clone(), *ms),
        None => (false, false, String::new(), 0),
    };
    let comparison = Comparison {
        at_ms,
        builtin_ran,
        builtin_ok: builtin_ran && !b_err,
        builtin_ms: b_ms,
        candidate_ok: cand_ok,
        candidate_ms: cand_ms,
        same: builtin_ran && cand_ok && b_out == cand_out,
    };
    let use_builtin = builtin_ran && (shadow || !cand_ok);
    let result = if use_builtin {
        (b_err, b_out)
    } else {
        (cand_err, cand_out)
    };
    (comparison, result)
}

/// A call of built-in host tool `builtin` that a replacement may serve. `None` when no version replaces it for the
/// caller's project (the built-in runs as before). Otherwise the call's result, recorded as a comparison, and a
/// regression switches the replacement back, telling the orchestrator's chat.
pub fn replaced_call(
    app: &App,
    cid: u64,
    builtin: &str,
    input: &Value,
    run_builtin: &dyn Fn() -> (bool, String),
) -> Option<(bool, String)> {
    let project = crate::projects::project_of(app, cid);
    let entry = app.library.replacement_for(builtin, project.as_deref())?;
    let shadow = entry.stage == "shadow";
    let timed_builtin = || {
        let started = Instant::now();
        let (err, out) = run_builtin();
        (err, out, started.elapsed().as_millis() as u64)
    };
    let first = shadow.then(timed_builtin);
    let candidate = run_candidate(&app.connectors, &app.library, &entry, input, outbound());
    let builtin_run = match first {
        Some(run) => Some(run),
        None if candidate.0 => Some(timed_builtin()),
        None => None,
    };
    let now = now_ms();
    let (comparison, result) = judge(shadow, builtin_run, candidate, now);
    app.library.measure_version(
        &entry.name,
        entry.version,
        comparison.candidate_ok,
        comparison.candidate_ms,
    );
    let regressed = app
        .library
        .compare(&entry.name, entry.version, comparison)
        .unwrap_or(false);
    if regressed {
        app.library.switch_back(&entry.name, entry.version);
        let chat = app.chats.main(now).id;
        crate::work_loop::notice(
            app,
            &chat,
            &format!(
                "{PREFIX}{} v{} replaced `{builtin}`, but its calls kept failing where the built-in held: the built-in serves again",
                entry.name, entry.version
            ),
            now,
        );
    }
    Some(result)
}

/// A call of a library tool reported by a VM (its `tool_package` call's tool, or a guest run): counted, and proposed for
/// removal when it has become harmful.
pub fn reported(app: &App, tool: &str, ok: bool, ms: u64) {
    if let Some(entry) = app.library.measure(tool, ok, ms) {
        check_harm(app, &entry);
    }
}

/// Opens a retirement card for a published version that has become harmful.
fn check_harm(app: &App, entry: &Entry) {
    if entry.state == "published" && library::harmful(entry) {
        retire_card(app, entry, "it fails more than half of its calls");
    }
}

/// The planner's verdict on pending version `version` of `name`. A published version is announced to its project; a
/// tool that uses a connector's secret opens the connector's card for the owner.
pub fn review(
    app: &App,
    name: &str,
    version: u32,
    approve: bool,
    note: &str,
) -> Result<String, String> {
    let entry = app
        .library
        .find(name, version)
        .ok_or_else(|| "no such version".to_string())?;
    if approve
        && !entry.connector.is_empty()
        && app.connectors.approved_manifest(&entry.connector).is_none()
    {
        return Err(format!(
            "connector `{}` has no approved manifest: the owner approves it first",
            entry.connector
        ));
    }
    let state = app.library.review(name, version, approve, note)?;
    let now = now_ms();
    match state.as_str() {
        "published" => tell(
            app,
            &entry.project,
            &format!("{PREFIX}{name} v{version} is approved and serves this project"),
            now,
        ),
        "awaiting_owner" => {
            let card = secret_card(app, &entry, now);
            app.bus.publish("card", &card);
        }
        _ => {}
    }
    app.bus.publish(
        "library",
        &json!({ "name": name, "version": version, "state": state }),
    );
    crate::agents::push_profiles(app);
    Ok(state)
}

/// Asks the owner to make a published version available to every project (a `promote` card in the orchestrator's chat).
pub fn request_promotion(app: &App, name: &str, version: u32) -> Result<(), String> {
    let entry = app
        .library
        .find(name, version)
        .ok_or_else(|| "no such version".to_string())?;
    if entry.state != "published" {
        return Err(format!("v{version} is {}, not published", entry.state));
    }
    if entry.promoted {
        return Err(format!("v{version} is already available to every project"));
    }
    let now = now_ms();
    let chat = app.chats.main(now).id;
    let (card, _) = app.cards.open(
        NewCard {
            kind: Kind::Promote,
            chat: Some(chat),
            project: None,
            task: None,
            session: None,
            subject: format!("promote:{name}:{version}"),
            title: format!("Make {PREFIX}{name} v{version} available to every project?"),
            body: Some(format!(
                "Made by {} in {}. It is serving its project now.",
                entry.author,
                if entry.project.is_empty() {
                    "no project"
                } else {
                    &entry.project
                }
            )),
            choices: vec![
                choice("approve", "Make it available", "every project offers it"),
                choice("reject", "Keep it to its project", "nothing changes"),
            ],
        },
        now,
    );
    app.bus.publish("card", &card);
    Ok(())
}

/// Moves a replacement to `stage` (shadow, trial for `trial_project`, or all), telling the orchestrator's chat.
pub fn set_stage(
    app: &App,
    name: &str,
    version: u32,
    stage: &str,
    trial_project: &str,
) -> Result<(), String> {
    let entry = app
        .library
        .find(name, version)
        .ok_or_else(|| "no such version".to_string())?;
    let side_effects = connectors::risk_of(&entry.replaces) != Some("read");
    app.library
        .set_stage(name, version, stage, trial_project, side_effects)?;
    let now = now_ms();
    let chat = app.chats.main(now).id;
    crate::work_loop::notice(
        app,
        &chat,
        &format!(
            "{PREFIX}{name} v{version} replaces `{}`: now at {stage}",
            entry.replaces
        ),
        now,
    );
    Ok(())
}

/// Proposes removing every published version that is harmful or unused at `now`. Returns how many cards it opened.
pub fn sweep(app: &App, now: u64) -> usize {
    let mut opened = 0;
    for entry in app.library.removal_candidates(now) {
        let reason = if library::harmful(&entry) {
            "it fails more than half of its calls"
        } else {
            "nobody has called it in thirty days"
        };
        if retire_card(app, &entry, reason) {
            opened += 1;
        }
    }
    opened
}

/// Does what an answered card of this module decides, and returns it in words: a connector card for a tool's secret, a
/// promotion, or a retirement.
pub fn card_answered(app: &App, card: &Card) -> String {
    let answer = card.answer.as_deref().unwrap_or("");
    let now = now_ms();
    if let Some((name, version)) = subject_of(&card.subject, "tool:") {
        let approve = answer == "approve";
        return match app.library.settle(&name, version, approve) {
            Ok(state) => {
                if state == "published"
                    && let Some(entry) = app.library.find(&name, version)
                {
                    tell(
                        app,
                        &entry.project,
                        &format!("{PREFIX}{name} v{version} is approved and serves this project"),
                        now,
                    );
                }
                crate::agents::push_profiles(app);
                format!("{PREFIX}{name} v{version} {state}")
            }
            Err(why) => why,
        };
    }
    if let Some((name, version)) = subject_of(&card.subject, "promote:") {
        return if answer == "approve" {
            match app.library.promote(&name, version) {
                Ok(()) => {
                    crate::agents::push_profiles(app);
                    format!("{PREFIX}{name} v{version} is available to every project")
                }
                Err(why) => why,
            }
        } else {
            format!("{PREFIX}{name} v{version} stays with its project")
        };
    }
    if let Some((name, version)) = subject_of(&card.subject, "retire:") {
        return if answer == "retire" {
            match app.library.retire(&name, version) {
                Ok(()) => {
                    crate::agents::push_profiles(app);
                    format!("{PREFIX}{name} v{version} is retired")
                }
                Err(why) => why,
            }
        } else {
            format!("{PREFIX}{name} v{version} is kept")
        };
    }
    "recorded".into()
}

/// The name and version a card's subject names after `prefix` (`name:version`).
fn subject_of(subject: &str, prefix: &str) -> Option<(String, u32)> {
    let (name, version) = subject.strip_prefix(prefix)?.rsplit_once(':')?;
    Some((name.to_string(), version.parse().ok()?))
}

fn choice(id: &str, label: &str, detail: &str) -> Choice {
    Choice {
        id: id.into(),
        label: label.into(),
        detail: Some(detail.into()),
        takes_amount: false,
    }
}

/// Posts `text` into the chat of `project` (nothing when the version has no project).
fn tell(app: &App, project: &str, text: &str, now: u64) {
    if project.is_empty() {
        return;
    }
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::notice(app, &chat, text, now);
}

/// The connector card a tool that uses a secret opens: its connector's label, the scopes and hosts it is approved for.
fn secret_card(app: &App, entry: &Entry, now: u64) -> Card {
    let label = connectors::find(&entry.connector).map_or(entry.connector.as_str(), |c| c.label);
    let hosts = app
        .connectors
        .approved_manifest(&entry.connector)
        .map(|m| m.egress.join(", "))
        .unwrap_or_default();
    let chat = if entry.project.is_empty() {
        app.chats.main(now).id
    } else {
        crate::work_loop::project_chat(app, &entry.project, now)
    };
    app.cards
        .open(
        NewCard {
            kind: Kind::Connector,
            chat: Some(chat),
            project: (!entry.project.is_empty()).then(|| entry.project.clone()),
            task: None,
            session: None,
            subject: format!("tool:{}:{}", entry.name, entry.version),
            title: format!(
                "{PREFIX}{} v{} uses {label}'s secret",
                entry.name, entry.version
            ),
            body: Some(format!(
                "Scopes {}; hosts {hosts}. Its component runs on the hub, and the key is added on the way out. Its tests {}.",
                connectors::scopes_of(&entry.connector).join(", "),
                if entry.tests.is_empty() { "have not run" } else { &entry.tests }
            )),
            choices: vec![
                choice("approve", "Approve the tool", "it may use the secret through this connector"),
                choice("reject", "Reject it", "it stays unpublished"),
            ],
        },
        now,
    )
    .0
}

/// Opens the retirement card of `entry`; returns whether it was newly opened.
fn retire_card(app: &App, entry: &Entry, reason: &str) -> bool {
    let now = now_ms();
    let chat = app.chats.main(now).id;
    let (card, opened) = app.cards.open(
        NewCard {
            kind: Kind::Tool,
            chat: Some(chat),
            project: None,
            task: None,
            session: None,
            subject: format!("retire:{}:{}", entry.name, entry.version),
            title: format!("Retire {PREFIX}{} v{}?", entry.name, entry.version),
            body: Some(format!(
                "{} calls, {} failed. Proposed because {reason}.",
                entry.calls, entry.failures
            )),
            choices: vec![
                choice("retire", "Retire it", "no agent is offered it any more"),
                choice("keep", "Keep it", "nothing changes"),
            ],
        },
        now,
    );
    if opened {
        app.bus.publish("card", &card);
    }
    opened
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::{Outgoing, Reply};
    use std::sync::Mutex;

    /// A transport that answers without a network and keeps every request it is asked to send.
    #[derive(Default)]
    struct Canned {
        sent: Mutex<Vec<Outgoing>>,
    }

    impl glue::Outbound for Canned {
        fn send(&self, request: &Outgoing) -> Result<Reply, String> {
            self.sent.lock().unwrap().push(request.clone());
            Ok(Reply {
                status: 200,
                body: r#"{"ok":true}"#.into(),
            })
        }
    }

    fn vault(dir: &std::path::Path) -> connectors::Store {
        let _ = std::fs::remove_dir_all(dir);
        let vault = crate::vault::Store::open_in(dir, &dir.join("vault.key"));
        vault.boot(apz_plaza::now_ms()).expect("boot");
        connectors::Store::new(Arc::new(vault))
    }

    fn entry_for(store: &library::Store, name: &str, connector: &str) -> Entry {
        let package = library::Package {
            name: name.into(),
            description: "a glue tool".into(),
            command: Vec::new(),
            requires: Vec::new(),
            input_schema: json!({ "type": "object" }),
            publish: true,
            connector: connector.into(),
            replaces: String::new(),
            files: vec![library::PackageFile {
                path: library::COMPONENT_FILE.into(),
                content_b64: library::base64_encode(&glue::tests::component()),
                executable: false,
            }],
        };
        store.insert(7, "scout", "p1", package).unwrap()
    }

    #[test]
    fn a_glue_tool_runs_on_the_hub_with_its_vaulted_key_and_the_key_stays_in_the_hub() {
        let dir = std::env::temp_dir().join(format!("apid-tool-life-{}", std::process::id()));
        let connectors = vault(&dir);
        connectors
            .set_secret("firecrawl", None, "fc-SENTINEL-TOOL")
            .unwrap();
        let library = library::Store::open(dir.join("state"));
        let entry = entry_for(&library, "fetcher", "firecrawl");
        let canned = Arc::new(Canned::default());
        let input =
            json!({ "method": "POST", "url": "https://api.firecrawl.dev/v2/scrape", "body": "{}" });
        let (is_error, output, _) =
            run_candidate(&connectors, &library, &entry, &input, canned.clone());
        assert!(!is_error, "{output}");
        assert!(output.contains(r#""status":200"#), "{output}");
        let sent = canned.sent.lock().unwrap();
        assert_eq!(
            sent[0].headers,
            vec![(
                "Authorization".to_string(),
                "Bearer fc-SENTINEL-TOOL".to_string()
            )]
        );
        assert!(!output.contains("SENTINEL"));
        // A host the manifest does not list is refused: the call fails inside the component, nothing is sent.
        let other = json!({ "method": "GET", "url": "https://evil.example.net/" });
        let (refused, why, _) =
            run_candidate(&connectors, &library, &entry, &other, canned.clone());
        assert!(
            !refused,
            "a refused reply is the component's, not a trap: {why}"
        );
        assert!(why.contains("error"), "{why}");
        assert_eq!(sent.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shadow_records_the_built_ins_result_beside_the_candidates_and_a_trial_falls_back() {
        // Shadow: the built-in's result is returned; the candidate's output differs, so the two are not the same.
        let shadow = judge(
            true,
            Some((false, "built-in".into(), 9)),
            (false, "candidate".into(), 4),
            1,
        );
        assert_eq!(shadow.1, (false, "built-in".to_string()));
        assert!(shadow.0.builtin_ran && shadow.0.builtin_ok && shadow.0.candidate_ok);
        assert!(!shadow.0.same);
        assert_eq!((shadow.0.builtin_ms, shadow.0.candidate_ms), (9, 4));

        // Trial, candidate ok: its result is returned and the built-in did not run.
        let trial = judge(false, None, (false, "candidate".into(), 4), 2);
        assert_eq!(trial.1, (false, "candidate".to_string()));
        assert!(!trial.0.builtin_ran && !trial.0.same);

        // Trial, candidate failed: the built-in is the fallback and its result is returned.
        let fallback = judge(
            false,
            Some((false, "built-in".into(), 5)),
            (true, "trap".into(), 2),
            3,
        );
        assert_eq!(fallback.1, (false, "built-in".to_string()));
        assert!(fallback.0.builtin_ran && !fallback.0.candidate_ok);

        // The shadow's comparison is what a replacement keeps; enough failures of the candidate switch it back.
        let store = library::Store::open(
            std::env::temp_dir().join(format!("apid-tool-life-judge-{}", std::process::id())),
        );
        let entry = entry_for(&store, "fetcher", "firecrawl");
        for _ in 0..3 {
            let (cmp, _) = judge(
                true,
                Some((false, "ok".into(), 1)),
                (true, "x".into(), 1),
                4,
            );
            store.compare(&entry.name, entry.version, cmp).unwrap();
        }
        assert!(library::regressed(
            &store.find(&entry.name, entry.version).unwrap().comparisons
        ));
    }

    #[test]
    fn a_card_subject_names_its_tool_and_version() {
        assert_eq!(
            subject_of("tool:fetcher:3", "tool:"),
            Some(("fetcher".to_string(), 3))
        );
        assert_eq!(subject_of("tool:fetcher", "tool:"), None);
        assert_eq!(subject_of("promote:fetcher:1", "tool:"), None);
    }
}
