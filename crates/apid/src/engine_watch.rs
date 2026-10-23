//! The engine watch (PLAN 12.6): the `engine-watch` recipe, run once a day. It reads the ACP registry and each pinned
//! engine's newest release (npm, PyPI, or a GitHub release of a binary), and each provider's model list. For a newer
//! version it runs the engine's adapter tests and opens a card in the plaza's chat with the result; for a model a
//! provider newly lists it opens a card too. Approving an engine bump opens a Weft change on the plaza's repository
//! that moves the pin in `agent-harness/src/acp/catalog.rs`, so the review chain decides it (PLAN 12.6: a bump that
//! passes its tests and the review chain is adopted).
//!
//! The adapter tests run a scripted session in an agent VM, which the hub does not start yet, so the production run
//! reports them as not run. A bump is therefore proposed, and adopted only by the owner's approval and the review.
//! Cursor's CLI has no pinnable release, so the watch reports it as a candidate and never pins it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use apz_blobs::Hash;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cards::{Card, Choice, Kind, NewCard};
use crate::engine_instances::{BUILTINS, Builtin, REGISTRY_URL, newer};
use crate::recipes::Report;
use crate::recipes::runs::Runs;
use crate::state::App;
use crate::work_loop;

/// The project the watch belongs to: its recipe run and its cards go to the plaza's chat, and its repository takes
/// the pin changes.
pub const PLAZA: &str = "plaza";
/// The subject of the watch's run (one run at a time per subject).
const SUBJECT: &str = "engines";
/// The file that pins the engines, in the plaza's repository.
pub const PIN_FILE: &str = "crates/agent-harness/src/acp/catalog.rs";
/// The watch's own record (the models each provider listed last, and the bumps awaiting an answer), in the hub's state.
const STATE_FILE: &str = "engine-watch.json";
/// How long the loop waits after a start before its first run, then how long between runs.
const FIRST_RUN: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(24 * 3600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DOCUMENT_LIMIT: u64 = 16 << 20;

/// The GitHub repository of each `release` engine (the hub's list names only its binary).
const RELEASE_REPOS: &[(&str, &str)] = &[("goose", "aaif-goose/goose")];

/// Engines the watch reports as candidates it cannot pin, with the reason.
pub const CANDIDATES: &[(&str, &str)] = &[(
    "cursor",
    "no pinnable release: the vendor ships Cursor's CLI only through an install script, and the npm `cursor-agent` \
     package belongs to an unrelated publisher, so it is never used",
)];

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

/// A newer version of a pinned engine, and where it was seen. An npm bump carries the integrity of that version, which
/// the pin needs; the other sources carry none, and their bumps are not adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    pub engine: &'static str,
    pub from: &'static str,
    pub to: String,
    /// `the ACP registry`, `npm`, `PyPI` or `GitHub release`.
    pub source: &'static str,
    pub integrity: Option<String>,
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

/// The providers' model lists, as the watch reads them.
pub trait ModelLists {
    /// The providers whose lists are read, by id.
    fn providers(&self) -> Vec<String>;
    /// The model ids provider `provider` lists now.
    fn list(&self, provider: &str) -> Result<Vec<String>, String>;
}

/// The hub's own providers: each one's listing, refreshed (`providers::listed_model_ids`).
pub struct Providers<'a>(pub &'a App);

impl ModelLists for Providers<'_> {
    fn providers(&self) -> Vec<String> {
        self.0.providers.provider_ids()
    }

    fn list(&self, provider: &str) -> Result<Vec<String>, String> {
        self.0.providers.listed_model_ids(provider)
    }
}

/// What the watch read: the newer versions, the candidates it cannot pin, and the sources it could not read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub bumps: Vec<Bump>,
    pub unpinnable: Vec<(String, String)>,
    pub unreachable: Vec<String>,
}

/// Reads the newest version of every engine in `engines` and of the ACP registry, and keeps those newer than the pin.
pub fn look(src: &dyn Source, bases: &Bases, engines: &[&Builtin]) -> Look {
    let mut look = Look {
        unpinnable: CANDIDATES
            .iter()
            .map(|(name, why)| (name.to_string(), why.to_string()))
            .collect(),
        ..Look::default()
    };
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
            // The integrity of the version the bump names, which the pin is written with.
            let integrity = (engine.source == "npm")
                .then(|| {
                    document(
                        src,
                        &format!("{}/{}/{to}", bases.npm, engine.package.replace('/', "%2F")),
                    )
                    .ok()
                    .and_then(|doc| {
                        doc.pointer("/dist/integrity")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                })
                .flatten();
            look.bumps.push(Bump {
                engine: engine.id,
                from: pinned,
                to,
                source,
                integrity,
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

/// The models each provider lists that the watch has not recorded. A provider with no record yet is baselined by the
/// caller: nothing is new on its first listing.
pub fn new_models(
    known: &BTreeMap<String, Vec<String>>,
    listed: &[(String, Vec<String>)],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (provider, ids) in listed {
        let Some(held) = known.get(provider) else {
            continue;
        };
        for id in ids {
            if !held.contains(id) {
                out.push((provider.clone(), id.clone()));
            }
        }
    }
    out
}

/// A bump awaiting the owner's answer: what its approval writes into the pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub engine: String,
    pub from: String,
    pub to: String,
    pub integrity: Option<String>,
}

/// The watch's record: the models each provider listed last, and the bumps awaiting an answer (by card subject).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub models: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub bumps: BTreeMap<String, Pending>,
}

/// The record at `path`, or an empty one when there is none (or it does not read).
pub fn load(path: &Path) -> State {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the record to `path`.
pub fn save(path: &Path, state: &State) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The catalogue text with `engine`'s pin moved from `from` to `to`, and its integrity set to `integrity`. An error
/// when the entry is not there or is not pinned at `from` (the catalogue has moved on).
pub fn pin_edit(
    text: &str,
    engine: &str,
    from: &str,
    to: &str,
    integrity: &str,
) -> Result<String, String> {
    let start = text
        .find(&format!("id: \"{engine}\","))
        .ok_or_else(|| format!("{engine} has no entry in {PIN_FILE}"))?;
    let end = start
        + text[start..]
            .find("\n    },")
            .ok_or_else(|| format!("the entry of {engine} does not end"))?;
    let mut entry = text[start..end].to_string();
    let pin = format!("version: \"{from}\",");
    let at = entry
        .find(&pin)
        .ok_or_else(|| format!("{engine} is not pinned at {from}"))?;
    entry.replace_range(at..at + pin.len(), &format!("version: \"{to}\","));
    let key = "integrity: \"";
    let at = entry
        .find(key)
        .ok_or_else(|| format!("{engine} has no integrity to move"))?;
    let close = at
        + key.len()
        + entry[at + key.len()..]
            .find("\",")
            .ok_or_else(|| format!("the integrity of {engine} does not end"))?;
    entry.replace_range(at..close + 2, &format!("integrity: \"{integrity}\","));
    Ok(format!("{}{entry}{}", &text[..start], &text[end..]))
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
            "{} has {} on {}; the pin is {}. Adapter tests: {}. Approving opens a change to {PIN_FILE} that moves the pin, \
             and the review chain decides it.",
            bump.engine,
            bump.to,
            bump.source,
            bump.from,
            tests.line()
        )),
        choices: answers(
            "Open the change",
            "The pin moves by a change that the review chain decides.",
        ),
    }
}

/// The card that tells the owner a provider lists a model the watch has not seen.
pub fn model_proposal(provider: &str, id: &str, chat: &str) -> NewCard {
    NewCard {
        kind: Kind::Engine,
        chat: Some(chat.to_string()),
        project: Some(PLAZA.to_string()),
        task: None,
        session: None,
        subject: format!("model:{provider}:{id}"),
        title: format!("New model {id} on {provider}"),
        body: Some(format!(
            "{provider} lists {id} now. Nothing changes until a model list names it; the watch does not add models itself."
        )),
        choices: answers("Note it", "Keeps the model out of every list."),
    }
}

fn answers(approve: &str, detail: &str) -> Vec<Choice> {
    vec![
        Choice {
            id: "approve".into(),
            label: approve.into(),
            detail: Some(detail.into()),
            takes_amount: false,
        },
        Choice {
            id: "dismiss".into(),
            label: "Not now".into(),
            detail: Some("Changes nothing; the watch asks again only for something new.".into()),
            takes_amount: false,
        },
    ]
}

/// Settles the watch's run: the `watch` step passed with `findings` cards, and the `propose` step runs (and passes)
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

/// One watch: reads the engines, the registry and the providers' model lists, runs the tests of each bump, records the
/// run and opens a card per bump and per new model in the plaza's chat. Returns how many cards were opened.
pub fn run(
    app: &App,
    src: &dyn Source,
    bases: &Bases,
    tests: &dyn AdapterTests,
    models: &dyn ModelLists,
) -> usize {
    let now = apz_plaza::now_ms();
    let engines: Vec<&Builtin> = BUILTINS.iter().collect();
    let found = look(src, bases, &engines);
    let chat = work_loop::project_chat(app, PLAZA, now);
    let path = app.config.state_dir.join(STATE_FILE);
    let mut state = load(&path);
    let mut cards: Vec<NewCard> = Vec::new();
    for bump in &found.bumps {
        let card = proposal(bump, &tests.run(bump), &chat);
        state.bumps.insert(
            card.subject.clone(),
            Pending {
                engine: bump.engine.into(),
                from: bump.from.into(),
                to: bump.to.clone(),
                integrity: bump.integrity.clone(),
            },
        );
        cards.push(card);
    }
    let mut failed = Vec::new();
    let mut new_model_count = 0;
    for provider in models.providers() {
        match models.list(&provider) {
            Ok(ids) => {
                for (p, id) in new_models(&state.models, &[(provider.clone(), ids.clone())]) {
                    cards.push(model_proposal(&p, &id, &chat));
                    new_model_count += 1;
                }
                state.models.insert(provider, ids);
            }
            Err(why) => failed.push(format!("{provider} models: {why}")),
        }
    }
    let count = cards.len();
    let outcome = format!(
        "{} newer engine version(s); {new_model_count} new model(s); {} candidate(s) with no pinnable release; {} source(s) not read",
        found.bumps.len(),
        found.unpinnable.len(),
        found.unreachable.len() + failed.len(),
    );
    if let Some(run) = app.recipe_runs.start_engine(
        "engine-watch",
        "schedule",
        None,
        Some(PLAZA),
        Some(SUBJECT),
        now,
    ) {
        settle(&app.recipe_runs, &run, count, &outcome, now);
    }
    if let Err(why) = save(&path, &state) {
        eprintln!("apid: the engine watch: {why}");
    }
    for card in cards {
        work_loop::open_card(app, card, now);
    }
    for why in found.unreachable.iter().chain(&failed) {
        eprintln!("apid: the engine watch could not read {why}");
    }
    for (name, why) in &found.unpinnable {
        eprintln!("apid: the engine watch: {name} is a candidate, {why}");
    }
    count
}

/// The owner's answer to an engine card. An approved bump opens the pin's change; a model card records nothing.
pub fn card_answered(app: &App, card: &Card) -> String {
    let path = app.config.state_dir.join(STATE_FILE);
    let mut state = load(&path);
    let pending = state.bumps.remove(&card.subject);
    if let Err(why) = save(&path, &state) {
        eprintln!("apid: the engine watch: {why}");
    }
    match (pending, card.answer.as_deref()) {
        (None, _) => "recorded".into(),
        (Some(pending), Some("approve")) => match adopt(app, &pending) {
            Ok(change) => format!(
                "opened change {change} to move {} to {}",
                pending.engine, pending.to
            ),
            Err(why) => format!("not adopted: {why}"),
        },
        (Some(pending), _) => format!("kept {} at {}", pending.engine, pending.from),
    }
}

/// Opens the Weft change that moves an approved bump's pin: the catalogue of `main` with the entry rewritten, as a
/// change on the plaza's repository, drafted, sealed, given its intent and queued (the review chain then decides).
fn adopt(app: &App, pending: &Pending) -> Result<String, String> {
    let integrity = pending
        .integrity
        .as_deref()
        .ok_or("its npm integrity is unknown, so the pin cannot be written")?;
    let repo = app
        .projects
        .get(PLAZA)
        .and_then(|project| project.repo)
        .ok_or_else(|| format!("{PLAZA} has no repo yet"))?;
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?
        .to_string();
    let client = app.weft.client("hub")?;
    let text =
        String::from_utf8(client.get_bytes(&format!("/revisions/{rev}/file?path={PIN_FILE}"))?)
            .map_err(|e| format!("{PIN_FILE} is not UTF-8: {e}"))?;
    let edited = pin_edit(
        &text,
        &pending.engine,
        &pending.from,
        &pending.to,
        integrity,
    )?;
    let bytes = edited.into_bytes();
    let hash = Hash::of(&bytes);
    let bytes_of = |h: &Hash| {
        if *h == hash {
            Ok(bytes.clone())
        } else {
            Err(format!("no bytes for {}", h.hex()))
        }
    };
    client.upload(std::slice::from_ref(&hash), &bytes_of)?;
    let title = format!("Bump {} to {}", pending.engine, pending.to);
    let opened = app.weft.post(
        "/lanes",
        &json!({ "repo": repo, "title": title, "author": { "kind": "hub" } }),
    )?;
    let change = opened["change"]
        .as_str()
        .ok_or("Weft opened no lane for the bump")?
        .to_string();
    let deltas = json!([{ "path": PIN_FILE, "after": hash, "mode": 0o100644 }]);
    app.weft.hub(
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["engine-watch"] }),
    )?;
    app.weft.hub(
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": deltas }),
    )?;
    app.weft.hub(
        "intent.set",
        json!({ "change": change, "n": 1, "intents": [{ "title": title, "kind": "change", "covers": [PIN_FILE] }] }),
    )?;
    app.weft.hub(
        "queue.add",
        json!({ "repo": repo, "ref": "main", "change": change, "n": 1 }),
    )?;
    Ok(change)
}

/// Starts the watch's loop: a run a minute after the start, then one a day.
pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("engine-watch".into())
        .spawn(move || {
            std::thread::sleep(FIRST_RUN);
            loop {
                run(
                    &app,
                    &Net { plain_http: false },
                    &Bases::default(),
                    &Unrun,
                    &Providers(&app),
                );
                std::thread::sleep(EVERY);
            }
        })
        .expect("spawn the engine watch thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Cards, State as CardState};
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

    /// The routes of the three engines the tests watch: claude (npm, with a newer version in the ACP registry, whose
    /// integrity is the newer version's), goose (a GitHub release, newer) and vibe (PyPI, the same as its pin).
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
                "/@agentclientprotocol%2Fclaude-agent-acp/0.85.0",
                r#"{"version":"0.85.0","dist":{"integrity":"sha512-NEW"}}"#,
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

    /// The model lists read from fake providers: each answers `/models` under its base, in the OpenAI list shape.
    struct FakeLists<'a> {
        src: &'a dyn Source,
        bases: Vec<(&'static str, String)>,
    }

    impl ModelLists for FakeLists<'_> {
        fn providers(&self) -> Vec<String> {
            self.bases.iter().map(|(id, _)| id.to_string()).collect()
        }

        fn list(&self, provider: &str) -> Result<Vec<String>, String> {
            let base = &self.bases.iter().find(|(id, _)| *id == provider).unwrap().1;
            let doc = document(self.src, &format!("{base}/models"))?;
            Ok(doc["data"]
                .as_array()
                .ok_or("no data list")?
                .iter()
                .filter_map(|m| m["id"].as_str().map(str::to_string))
                .collect())
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
                    integrity: Some("sha512-NEW".into()),
                },
                Bump {
                    engine: "goose",
                    from: "1.52.0",
                    to: "1.53.0".into(),
                    source: "GitHub release",
                    integrity: None,
                },
            ],
            "vibe is at its pin, so it is no bump"
        );
        assert!(found.unreachable.is_empty(), "{:?}", found.unreachable);
        let asked = seen.lock().unwrap().clone();
        for path in [
            "/registry.json",
            "/@agentclientprotocol%2Fclaude-agent-acp/latest",
            "/@agentclientprotocol%2Fclaude-agent-acp/0.85.0",
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
    fn cursor_is_reported_as_a_candidate_with_no_pinnable_release() {
        let (bases, _) = fake_hub(Vec::new());
        let found = look(&Net { plain_http: true }, &bases, &[]);
        let cursor = found
            .unpinnable
            .iter()
            .find(|(name, _)| name == "cursor")
            .expect("cursor is a candidate");
        assert!(cursor.1.starts_with("no pinnable release"), "{}", cursor.1);
        assert!(found.bumps.is_empty());
    }

    #[test]
    fn a_provider_list_is_read_from_its_fake_server_and_only_new_models_are_carded() {
        let (bases, seen) = fake_hub(vec![(
            "/models",
            r#"{"data":[{"id":"grok-4.7"},{"id":"grok-5"}]}"#,
        )]);
        let src = Net { plain_http: true };
        let lists = FakeLists {
            src: &src,
            bases: vec![("xai", bases.npm.clone()), ("deepseek", bases.npm.clone())],
        };
        let listed: Vec<(String, Vec<String>)> = lists
            .providers()
            .into_iter()
            .map(|p| {
                let ids = lists.list(&p).unwrap();
                (p, ids)
            })
            .collect();
        assert_eq!(listed[0].1, ["grok-4.7", "grok-5"]);
        assert!(seen.lock().unwrap().iter().any(|p| p == "/models"));

        // xai was listed before with grok-4.7 only: grok-5 is new. deepseek has no record yet: nothing is new.
        let known = BTreeMap::from([("xai".to_string(), vec!["grok-4.7".to_string()])]);
        assert_eq!(
            new_models(&known, &listed),
            vec![("xai".to_string(), "grok-5".to_string())]
        );
        let card = model_proposal("xai", "grok-5", "chat-plaza");
        assert_eq!(card.kind, Kind::Engine);
        assert_eq!(card.subject, "model:xai:grok-5");
        assert_eq!(card.title, "New model grok-5 on xai");
    }

    #[test]
    fn the_watch_record_survives_a_save_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("apz-engine-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE);
        assert_eq!(load(&path), State::default());
        let mut state = State::default();
        state.models.insert("xai".into(), vec!["grok-4.7".into()]);
        state.bumps.insert(
            "engine-bump:claude:0.85.0".into(),
            Pending {
                engine: "claude".into(),
                from: "0.84.0".into(),
                to: "0.85.0".into(),
                integrity: Some("sha512-NEW".into()),
            },
        );
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_is_moved_in_its_own_entry_and_a_stale_pin_is_refused() {
        const SAMPLE: &str = "pub const ENGINES: &[Entry] = &[\n    Entry {\n        id: \"claude\",\n        install: Install::Npm {\n            package: \"@x/claude\",\n            version: \"0.84.0\",\n            integrity: \"sha512-AAA\",\n        },\n    },\n    Entry {\n        id: \"codex\",\n        install: Install::Npm {\n            package: \"@x/codex\",\n            version: \"2.0.1\",\n            integrity: \"sha512-BBB\",\n        },\n    },\n];\n";
        let edited = pin_edit(SAMPLE, "claude", "0.84.0", "0.85.0", "sha512-NEW").unwrap();
        assert!(
            edited.contains("version: \"0.85.0\",\n            integrity: \"sha512-NEW\","),
            "{edited}"
        );
        assert!(
            edited.contains("version: \"2.0.1\",\n            integrity: \"sha512-BBB\","),
            "{edited}"
        );
        assert!(pin_edit(SAMPLE, "claude", "0.1.0", "0.85.0", "x").is_err());
        assert!(pin_edit(SAMPLE, "nope", "0.84.0", "0.85.0", "x").is_err());
    }

    #[test]
    fn a_bump_is_proposed_with_its_test_result_and_the_run_walks_to_propose() {
        let bump = Bump {
            engine: "claude",
            from: "0.84.0",
            to: "0.85.0".into(),
            source: "the ACP registry",
            integrity: Some("sha512-NEW".into()),
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
        assert!(new && opened.state == CardState::Open);

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
            integrity: None,
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
