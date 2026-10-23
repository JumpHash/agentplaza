//! The engine watch (PLAN 12.6): the `engine-watch` recipe, run once a day. It reads the ACP registry and each pinned
//! engine's newest release (npm, PyPI, or a GitHub release of a binary), and for a newer version it runs the engine's
//! adapter tests and opens a card with the result in the plaza's chat. A pin is code (`agent-harness/src/acp/catalog.rs`),
//! so a bump is made by a reviewed change to the catalogue: the card records the owner's decision and changes no pin.
//!
//! The adapter tests run a scripted session in an agent VM, which the hub does not start yet, so the production run
//! reports them as not run. A bump is therefore proposed, never adopted, by the watch.
//!
//! Not read yet: the providers' model lists (they need each provider's key, which the watch does not hold).

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::cards::{Choice, Kind, NewCard};
use crate::engine_instances::{BUILTINS, Builtin, REGISTRY_URL, newer};
use crate::recipes::Report;
use crate::recipes::runs::Runs;
use crate::state::App;
use crate::work_loop;

/// The project the watch belongs to: its recipe run and its card go to the plaza's chat.
pub const PLAZA: &str = "plaza";
/// The subject of the watch's run (one run at a time per subject).
const SUBJECT: &str = "engines";
/// How long the loop waits after a start before its first run, then how long between runs.
const FIRST_RUN: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(24 * 3600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DOCUMENT_LIMIT: u64 = 16 << 20;

/// The GitHub repository of each `release` engine (the hub's list names only its binary).
const RELEASE_REPOS: &[(&str, &str)] = &[("goose", "aaif-goose/goose")];

/// Where the watch reads each source. The production values are the public services; a test points them at a fake server.
#[derive(Debug, Clone)]
pub struct Bases {
    /// The ACP registry document (the one `/v1/acp/registry` serves).
    pub registry: String,
    pub npm: String,
    pub pypi: String,
    pub github: String,
}

impl Default for Bases {
    fn default() -> Self {
        Bases {
            registry: REGISTRY_URL.to_string(),
            npm: "https://registry.npmjs.org".to_string(),
            pypi: "https://pypi.org".to_string(),
            github: "https://api.github.com".to_string(),
        }
    }
}

/// Reads one JSON document.
pub trait Source: Send + Sync {
    fn text(&self, url: &str) -> Result<String, String>;
}

/// The network source. HTTPS only; plain HTTP is allowed where a test says so (a fake server on the loopback).
pub struct Net {
    pub plain_http: bool,
}

impl Source for Net {
    fn text(&self, url: &str) -> Result<String, String> {
        if !(url.starts_with("https://") || (self.plain_http && url.starts_with("http://"))) {
            return Err(format!("{url} is not an https address"));
        }
        let request = ureq::http::Request::builder()
            .method("GET")
            .uri(url)
            .header("user-agent", "agentplaza")
            .header("accept", "application/json")
            .body(Vec::new())
            .map_err(|e| format!("bad request: {e}"))?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(FETCH_TIMEOUT))
            .http_status_as_error(false)
            .build();
        let mut response = ureq::Agent::new_with_config(config)
            .run(request)
            .map_err(|e| format!("{url} did not answer: {e}"))?;
        if response.status().as_u16() != 200 {
            return Err(format!("{url} answered {}", response.status().as_u16()));
        }
        response
            .body_mut()
            .with_config()
            .limit(DOCUMENT_LIMIT)
            .read_to_string()
            .map_err(|e| format!("reading {url}: {e}"))
    }
}

/// A newer version of a pinned engine, and where it was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    pub engine: &'static str,
    pub from: &'static str,
    pub to: String,
    /// `the ACP registry`, `npm`, `PyPI` or `GitHub release`.
    pub source: &'static str,
}

/// What the adapter's tests said about a bump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tests {
    // The VM runner that reports these is not built yet; only the tests construct them until it is.
    #[cfg_attr(not(test), allow(dead_code))]
    Passed,
    #[cfg_attr(not(test), allow(dead_code))]
    Failed(String),
    NotRun(String),
}

impl Tests {
    /// The result in a sentence, for the card.
    pub fn line(&self) -> String {
        match self {
            Tests::Passed => "passed".to_string(),
            Tests::Failed(why) => format!("failed ({why})"),
            Tests::NotRun(why) => format!("not run ({why})"),
        }
    }
}

/// Runs the adapter's tests for a bump.
pub trait AdapterTests: Send + Sync {
    fn run(&self, bump: &Bump) -> Tests;
}

/// The production adapter tests: the scripted session needs an agent VM, which the hub does not start yet.
pub struct Unrun;

impl AdapterTests for Unrun {
    fn run(&self, _: &Bump) -> Tests {
        Tests::NotRun(
            "the scripted session needs an agent VM, which the watch does not start yet".into(),
        )
    }
}

/// What the watch read: the newer versions, and the sources it could not read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub bumps: Vec<Bump>,
    pub unreachable: Vec<String>,
}

/// Reads the newest version of every engine in `engines` and of the ACP registry, and keeps those newer than the pin.
pub fn look(src: &dyn Source, bases: &Bases, engines: &[&Builtin]) -> Look {
    let mut look = Look::default();
    let registry = match document(src, &bases.registry) {
        Ok(doc) => Some(doc),
        Err(why) => {
            look.unreachable.push(format!("the ACP registry: {why}"));
            None
        }
    };
    for engine in engines {
        let pinned = engine.pinned;
        let (source, latest) = match engine.source {
            "npm" => (
                "npm",
                document(
                    src,
                    &format!(
                        "{}/{}/latest",
                        bases.npm,
                        engine.package.replace('/', "%2F")
                    ),
                )
                .and_then(|doc| version_at(&doc, "/version")),
            ),
            "pip" => (
                "PyPI",
                document(src, &format!("{}/pypi/{}/json", bases.pypi, engine.package))
                    .and_then(|doc| version_at(&doc, "/info/version")),
            ),
            "release" => (
                "GitHub release",
                match RELEASE_REPOS.iter().find(|(id, _)| *id == engine.id) {
                    Some((_, repo)) => document(
                        src,
                        &format!("{}/repos/{repo}/releases/latest", bases.github),
                    )
                    .and_then(|doc| version_at(&doc, "/tag_name"))
                    .map(|tag| tag.trim_start_matches('v').to_string()),
                    None => Err("no GitHub repository is known for it".to_string()),
                },
            ),
            other => (
                "",
                Err(format!("the hub does not know the source `{other}`")),
            ),
        };
        let mut best: Option<(&'static str, String)> = None;
        if engine.source == "npm"
            && let Some(version) = registry
                .as_ref()
                .and_then(|doc| registry_version(doc, engine.package))
            && newer(&version, pinned)
        {
            best = Some(("the ACP registry", version));
        }
        match latest {
            Ok(version) => {
                if newer(&version, pinned)
                    && best.as_ref().is_none_or(|(_, held)| newer(&version, held))
                {
                    best = Some((source, version));
                }
            }
            Err(why) => look.unreachable.push(format!("{}: {why}", engine.id)),
        }
        if let Some((source, to)) = best {
            look.bumps.push(Bump {
                engine: engine.id,
                from: pinned,
                to,
                source,
            });
        }
    }
    look
}

/// One JSON document from `src`, parsed.
fn document(src: &dyn Source, url: &str) -> Result<Value, String> {
    let text = src.text(url)?;
    serde_json::from_str(&text).map_err(|e| format!("{url} is not JSON: {e}"))
}

/// The version a document names at `pointer`.
fn version_at(doc: &Value, pointer: &str) -> Result<String, String> {
    doc.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("no version at {pointer}"))
}

/// The version the ACP registry gives the agent whose npx distribution is `package`.
fn registry_version(doc: &Value, package: &str) -> Option<String> {
    doc["agents"].as_array()?.iter().find_map(|agent| {
        let spec = agent["distribution"]["npx"]["package"].as_str()?;
        // `name@version`; a scoped name (`@scope/name`) has its `@` first, which is not a version separator.
        let name = spec
            .rsplit_once('@')
            .filter(|(name, _)| !name.is_empty())
            .map_or(spec, |(name, _)| name);
        (name == package)
            .then(|| agent["version"].as_str().map(str::to_string))
            .flatten()
    })
}

/// The card that asks the owner about a bump, with the adapter tests' result.
pub fn proposal(bump: &Bump, tests: &Tests, chat: &str) -> NewCard {
    NewCard {
        kind: Kind::Engine,
        chat: Some(chat.to_string()),
        project: Some(PLAZA.to_string()),
        task: None,
        session: None,
        subject: format!("engine-bump:{}:{}", bump.engine, bump.to),
        title: format!("Bump {} from {} to {}", bump.engine, bump.from, bump.to),
        body: Some(format!(
            "{} has {} on {}; the pin is {}. Adapter tests: {}. Approving records the owner's decision; the pin itself \
             changes by a reviewed change to the catalogue.",
            bump.engine,
            bump.to,
            bump.source,
            bump.from,
            tests.line()
        )),
        choices: vec![
            Choice {
                id: "approve".into(),
                label: "Record the bump".into(),
                detail: Some("The owner's decision; the pin changes by a reviewed change.".into()),
                takes_amount: false,
            },
            Choice {
                id: "dismiss".into(),
                label: "Not now".into(),
                detail: Some(
                    "Keeps the pin; the watch proposes again only a newer version.".into(),
                ),
                takes_amount: false,
            },
        ],
    }
}

/// Settles the watch's run: the `watch` step passed with `findings` bumps, and the `propose` step runs (and passes)
/// when there are some. The run's outcome is `outcome`.
pub fn settle(runs: &Runs, run: &str, findings: usize, outcome: &str, now: u64) {
    runs.finish_subject(
        "schedule",
        SUBJECT,
        &Report {
            passed: true,
            findings: Some(u32::try_from(findings).unwrap_or(u32::MAX)),
            ..Report::default()
        },
        outcome,
        false,
        now,
    );
    if findings > 0 {
        runs.started(run, "propose", now);
        runs.settle(run, "propose", &Report::verdict(true), now);
    }
}

/// One watch: reads the sources, runs the tests of each bump, records the run and opens a card per bump in the plaza's
/// chat. Returns how many bumps were proposed.
pub fn run(app: &App, src: &dyn Source, bases: &Bases, tests: &dyn AdapterTests) -> usize {
    let now = apz_plaza::now_ms();
    let engines: Vec<&Builtin> = BUILTINS.iter().collect();
    let found = look(src, bases, &engines);
    let chat = work_loop::project_chat(app, PLAZA, now);
    let cards: Vec<NewCard> = found
        .bumps
        .iter()
        .map(|bump| proposal(bump, &tests.run(bump), &chat))
        .collect();
    let outcome = format!(
        "{} newer engine version(s); {} source(s) not read",
        cards.len(),
        found.unreachable.len()
    );
    if let Some(run) = app.recipe_runs.start_engine(
        "engine-watch",
        "schedule",
        None,
        Some(PLAZA),
        Some(SUBJECT),
        now,
    ) {
        settle(&app.recipe_runs, &run, cards.len(), &outcome, now);
    }
    for card in &cards {
        work_loop::open_card(app, card.clone(), now);
    }
    for why in &found.unreachable {
        eprintln!("apid: the engine watch could not read {why}");
    }
    cards.len()
}

/// Starts the watch's loop: a run a minute after the start, then one a day.
pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("engine-watch".into())
        .spawn(move || {
            std::thread::sleep(FIRST_RUN);
            loop {
                run(&app, &Net { plain_http: false }, &Bases::default(), &Unrun);
                std::thread::sleep(EVERY);
            }
        })
        .expect("spawn the engine watch thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Cards, State};
    use crate::recipes::graph::StepState;
    use crate::recipes::runs::RunState;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// A fake HTTP server on the loopback: it answers `routes` (by path) with 200 and a JSON body, anything else with
    /// 404, and records every path it is asked for.
    fn fake_hub(routes: Vec<(&'static str, &'static str)>) -> (Bases, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let Ok(clone) = stream.try_clone() else {
                    continue;
                };
                let mut reader = BufReader::new(clone);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = line.split(' ').nth(1).unwrap_or("").to_string();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                }
                log.lock().unwrap().push(path.clone());
                let (status, body) = match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => (200, *body),
                    None => (404, "{}"),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let bases = Bases {
            registry: format!("{base}/registry.json"),
            npm: base.clone(),
            pypi: base.clone(),
            github: base,
        };
        (bases, seen)
    }

    /// The routes of the three engines the tests watch: claude (npm, with a newer version in the ACP registry), goose
    /// (a GitHub release, newer) and vibe (PyPI, the same as its pin).
    fn routes() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "/registry.json",
                r#"{"agents":[{"id":"claude-acp","version":"0.85.0","distribution":{"npx":{"package":"@agentclientprotocol/claude-agent-acp@0.85.0"}}}]}"#,
            ),
            (
                "/@agentclientprotocol%2Fclaude-agent-acp/latest",
                r#"{"version":"0.84.0"}"#,
            ),
            (
                "/repos/aaif-goose/goose/releases/latest",
                r#"{"tag_name":"v1.53.0"}"#,
            ),
            (
                "/pypi/mistral-vibe/json",
                r#"{"info":{"version":"2.25.0"}}"#,
            ),
        ]
    }

    fn watched() -> Vec<&'static Builtin> {
        BUILTINS
            .iter()
            .filter(|b| ["claude", "goose", "vibe"].contains(&b.id))
            .collect()
    }

    struct Passing;

    impl AdapterTests for Passing {
        fn run(&self, _: &Bump) -> Tests {
            Tests::Passed
        }
    }

    #[test]
    fn a_newer_version_in_the_registry_or_a_release_is_a_bump_and_the_calls_are_recorded() {
        let (bases, seen) = fake_hub(routes());
        let found = look(&Net { plain_http: true }, &bases, &watched());
        assert_eq!(
            found.bumps,
            vec![
                Bump {
                    engine: "claude",
                    from: "0.84.0",
                    to: "0.85.0".into(),
                    source: "the ACP registry",
                },
                Bump {
                    engine: "goose",
                    from: "1.52.0",
                    to: "1.53.0".into(),
                    source: "GitHub release",
                },
            ],
            "vibe is at its pin, so it is no bump"
        );
        assert!(found.unreachable.is_empty(), "{:?}", found.unreachable);
        let asked = seen.lock().unwrap().clone();
        for path in [
            "/registry.json",
            "/@agentclientprotocol%2Fclaude-agent-acp/latest",
            "/repos/aaif-goose/goose/releases/latest",
            "/pypi/mistral-vibe/json",
        ] {
            assert!(
                asked.iter().any(|p| p == path),
                "{path} was asked: {asked:?}"
            );
        }
    }

    #[test]
    fn a_source_that_does_not_answer_is_counted_and_the_rest_is_still_read() {
        // No route for vibe: its lookup fails, and the others are read as before.
        let routes: Vec<_> = routes()
            .into_iter()
            .filter(|(p, _)| *p != "/pypi/mistral-vibe/json")
            .collect();
        let (bases, _) = fake_hub(routes);
        let found = look(&Net { plain_http: true }, &bases, &watched());
        assert_eq!(found.bumps.len(), 2);
        assert_eq!(found.unreachable.len(), 1, "{:?}", found.unreachable);
        assert!(
            found.unreachable[0].starts_with("vibe:"),
            "{:?}",
            found.unreachable
        );
    }

    #[test]
    fn a_bump_is_proposed_with_its_test_result_and_the_run_walks_to_propose() {
        let bump = Bump {
            engine: "claude",
            from: "0.84.0",
            to: "0.85.0".into(),
            source: "the ACP registry",
        };
        let result = Passing.run(&bump);
        let card = proposal(&bump, &result, "chat-plaza");
        assert_eq!(card.kind, Kind::Engine);
        assert_eq!(card.title, "Bump claude from 0.84.0 to 0.85.0");
        assert_eq!(card.subject, "engine-bump:claude:0.85.0");
        assert!(
            card.body
                .as_deref()
                .unwrap()
                .contains("Adapter tests: passed")
        );
        let cards = Cards::default();
        let (opened, new) = cards.open(card, 3);
        assert!(new && opened.state == State::Open);

        let runs = Runs::default();
        let run = runs
            .start_engine(
                "engine-watch",
                "schedule",
                None,
                Some(PLAZA),
                Some(SUBJECT),
                1,
            )
            .expect("the engine-watch recipe is built in");
        settle(
            &runs,
            &run,
            1,
            "1 newer engine version(s); 0 source(s) not read",
            2,
        );
        let view = runs.view(&run).expect("the run");
        assert_eq!(view.run.state, RunState::Passed);
        assert_eq!(view.walk.steps["watch"].state, StepState::Passed);
        assert_eq!(view.walk.steps["propose"].state, StepState::Passed);
        assert_eq!(
            view.run.outcome.as_deref(),
            Some("1 newer engine version(s); 0 source(s) not read")
        );
    }

    #[test]
    fn a_watch_with_no_bump_ends_after_the_watch_step() {
        let runs = Runs::default();
        let run = runs
            .start_engine(
                "engine-watch",
                "schedule",
                None,
                Some(PLAZA),
                Some(SUBJECT),
                1,
            )
            .expect("the engine-watch recipe is built in");
        settle(
            &runs,
            &run,
            0,
            "0 newer engine version(s); 0 source(s) not read",
            2,
        );
        let view = runs.view(&run).expect("the run");
        assert_eq!(view.run.state, RunState::Passed);
        assert_eq!(view.walk.steps["watch"].state, StepState::Passed);
        assert_ne!(view.walk.steps["propose"].state, StepState::Passed);
    }

    #[test]
    fn the_engine_watch_recipe_is_a_real_recipe_on_a_schedule() {
        let recipe = Runs::default()
            .recipe("engine-watch", None)
            .expect("the engine-watch recipe is built in");
        assert!(
            !recipe.description.contains("Stub"),
            "{}",
            recipe.description
        );
        assert_eq!(recipe.triggers, ["schedule"]);
        let steps: Vec<&str> = recipe.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(steps, ["watch", "propose"]);
    }

    #[test]
    fn the_production_adapter_tests_say_why_they_did_not_run() {
        let bump = Bump {
            engine: "claude",
            from: "0.84.0",
            to: "0.85.0".into(),
            source: "npm",
        };
        match Unrun.run(&bump) {
            Tests::NotRun(why) => assert!(why.contains("agent VM"), "{why}"),
            other => panic!("not run, got {other:?}"),
        }
        assert_eq!(Tests::Failed("x".into()).line(), "failed (x)");
    }

    #[test]
    fn the_registry_names_a_scoped_package_with_its_version_after_the_last_at() {
        let doc: Value = serde_json::from_str(
            r#"{"agents":[{"id":"a","version":"9.0.0","distribution":{"npx":{"package":"@scope/pkg"}}},
                          {"id":"b","version":"2.0.0","distribution":{"npx":{"package":"@scope/other@2.0.0"}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            registry_version(&doc, "@scope/pkg").as_deref(),
            Some("9.0.0")
        );
        assert_eq!(
            registry_version(&doc, "@scope/other").as_deref(),
            Some("2.0.0")
        );
        assert_eq!(registry_version(&doc, "@scope/missing"), None);
    }
}
