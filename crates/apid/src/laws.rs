//! The plaza's laws, enforced on every tool call an agent makes (JHRL, `crates/jhrl`).
//!
//! `check` decides one tool call against the laws that cover `tool.call`. An applying forbid
//! refuses the call and is recorded as a violation on the agent's profile. A call that no law
//! permits is refused too (default deny), but not recorded: a missing permission, not misconduct.
//! `permits` answers for a role and tool alone; it decides which tools an agent is offered.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use apz_plaza::council;
use jhrl::ast::{Effect, Item, Law, Scope, TypeExpr};
use jhrl::authz::{authorize, Decision, Entity, EntityStore, Request};
use jhrl::span::SourceMap;
use jhrl::value::Value;
use jhrl::Program;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::settings::{tier_of, Tier};
use crate::tools::{runs, Runs};

/// The law files, compiled into the binary. `cases.jhrl` is the test fixture set, not law.
const LAW_FILES: [(&str, &str); 4] = [
    ("operations.jhrl", include_str!("../../jhrl/specs/laws/operations.jhrl")),
    ("constitution.jhrl", include_str!("../../jhrl/specs/laws/constitution.jhrl")),
    ("statutes.jhrl", include_str!("../../jhrl/specs/laws/statutes.jhrl")),
    ("tools.jhrl", include_str!("../../jhrl/specs/laws/tools.jhrl")),
];
/// Violations kept per agent; the oldest go first.
const KEEP: usize = 200;
/// The window the profile shows, in milliseconds.
const WINDOW_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// One recorded violation: the law, when it was refused, and what happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub law: String,
    pub at_ms: u64,
    pub detail: String,
}

/// The outcome of checking one tool call.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Ids of the applying forbid laws; non-empty means the call is refused.
    pub forbid: Vec<String>,
    /// True when no law permitted the call and none forbade it (default deny: refused, not recorded).
    pub unpermitted: bool,
}

/// One law as the console's Rules view shows it.
#[derive(Clone, Debug, Serialize)]
pub struct LawInfo {
    pub id: String,
    pub title: String,
    /// "constitution" | "statute" | "operations" (the law's tier, or its file's).
    pub tier: String,
    /// "preventive" (a refusal before the act) | "detective" (recorded after) | "".
    pub enforcement: String,
    /// "info" | "minor" | "major" | "critical" | "".
    pub severity: String,
    /// The law in one or two plain sentences, as its author wrote it.
    pub statement: String,
    /// Why the law exists, when the author said.
    pub rationale: String,
    /// "permit" | "forbid".
    pub effect: String,
    /// The actions it covers, e.g. "tool.exec", "plaza.pay".
    pub ops: Vec<String>,
    /// Who it applies to (`principal is Agent`, `principal in Role::"builder"`), as source text.
    pub principal: String,
    /// What it applies to, as source text.
    pub resource: String,
    /// Its `when` conditions, each as source text.
    pub conditions: Vec<String>,
    /// A generated plain-language sentence: who may/may not do what, on what, and when.
    pub explanation: String,
    /// The file it comes from, and the law's whole source.
    pub file: String,
    pub source: String,
}

/// Where each recorded violation is also written (Postgres): (cid, violation, tool).
pub type InfractionSink = Box<dyn Fn(u64, &Violation, &str) + Send + Sync>;

pub struct Laws {
    program: Program,
    /// Law id to its title, read from the law source.
    titles: BTreeMap<String, String>,
    path: PathBuf,
    violations: Mutex<BTreeMap<String, Vec<Violation>>>,
    sink: Mutex<Option<InfractionSink>>,
    /// Memoized [`Laws::permits`] answers, keyed by (role, tool). The seats and tiers a role has
    /// are fixed constants, so an answer never changes while apid runs.
    permitted: Mutex<HashMap<(String, String), bool>>,
}

impl Laws {
    /// Compiles the laws and loads the recorded violations from `<state_dir>/violations.json`.
    /// Panics at startup if the laws do not compile: an unenforced law is worse than no start.
    pub fn load(state_dir: &Path) -> Laws {
        let files: Vec<(String, String)> =
            LAW_FILES.iter().map(|(name, text)| (name.to_string(), text.to_string())).collect();
        let mut titles = BTreeMap::new();
        for (_, text) in LAW_FILES {
            titles_of(text, &mut titles);
        }
        let (program, diags) = Program::load(files);
        assert!(diags.is_empty(), "the laws do not compile: {} diagnostics", diags.len());
        let path = state_dir.join("violations.json");
        let violations = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Laws {
            program,
            titles,
            path,
            violations: Mutex::new(violations),
            sink: Mutex::new(None),
            permitted: Mutex::new(HashMap::new()),
        }
    }

    /// Decides one tool call. `input` is the call's JSON input, scanned for the flags the laws test.
    ///
    /// Every call is a `tool.call`; a command or file change in the agent's own VM is also a
    /// `tool.exec` on its sandbox, and a direct message also a `message.send` to its recipient, so
    /// the laws written for those operations apply too. Only `tool.call`'s default deny is
    /// reported as `unpermitted`.
    pub fn check(&self, agent_cid: &str, role: &str, tool: &str, input: &Json) -> Verdict {
        let flags = detect_flags(tool, input);
        let agent = Entity { ty: "Agent".into(), id: agent_cid.into() };
        let tool_entity = Entity { ty: "Tool".into(), id: tool.into() };
        let sandbox = Entity { ty: "Sandbox".into(), id: format!("vm-{agent_cid}") };
        let mut store = EntityStore::default();
        let member = |role: &str| {
            let tier = match tier_of(role) {
                Tier::Orchestrator => "orchestrator",
                Tier::Worker => "worker",
            };
            BTreeMap::from([
                ("role".to_string(), Value::str(role)),
                ("town".to_string(), Value::str("plaza")),
                ("tier".to_string(), Value::str(tier)),
                ("seated".to_string(), Value::Bool(council::is_seated(role))),
            ])
        };
        store.insert(&agent, member(role));
        store.insert(
            &tool_entity,
            BTreeMap::from([
                ("name".to_string(), Value::str(tool)),
                ("kind".to_string(), Value::str(tool_kind(tool))),
                ("risk_class".to_string(), Value::str(risk_class(tool))),
            ]),
        );
        store.insert(&sandbox, BTreeMap::from([("agent".to_string(), jhrl::authz::entity_value(&agent))]));
        let context = Value::record(BTreeMap::from([(
            "flags".to_string(),
            Value::set(flags.iter().map(|flag| Value::str(flag))),
        )]));
        let mut requests = vec![("tool.call", tool_entity)];
        if GUEST_EXEC.contains(&tool) || tool.starts_with(crate::library::PREFIX) {
            requests.push(("tool.exec", sandbox));
        }
        if tool == "plaza_send_message" {
            if let Some(to) = input.get("to_cid").and_then(Json::as_u64) {
                let peer = Entity { ty: "Agent".into(), id: to.to_string() };
                store.insert(&peer, member(""));
                requests.push(("message.send", peer));
            }
        }
        let mut verdict = Verdict::default();
        for (i, (operation, resource)) in requests.into_iter().enumerate() {
            let request = Request { principal: agent.clone(), operation: operation.into(), resource, context: context.clone() };
            match authorize(&self.program, &store, &request).decision {
                Decision::Deny { by, .. } if !by.is_empty() => {
                    for id in by {
                        if !verdict.forbid.contains(&id) {
                            verdict.forbid.push(id);
                        }
                    }
                }
                Decision::Deny { .. } if i == 0 => verdict.unpermitted = true,
                _ => {}
            }
        }
        verdict
    }

    /// Whether the laws let `role` use `tool` at all: no forbid applies and a permit covers it.
    /// Decided on the role, tier and seat alone (an empty input), and memoized.
    pub fn permits(&self, role: &str, tool: &str) -> bool {
        let key = (role.to_string(), tool.to_string());
        if let Some(answer) = self.permitted.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return *answer;
        }
        let verdict = self.check("0", role, tool, &Json::Object(Default::default()));
        let answer = verdict.forbid.is_empty() && !verdict.unpermitted;
        self.permitted.lock().unwrap_or_else(PoisonError::into_inner).insert(key, answer);
        answer
    }

    /// The refusal reason when a law forbids the call (recorded as a violation) or no law permits
    /// it (default deny, not recorded); `None` when it may run.
    pub fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Json) -> Option<String> {
        let verdict = self.check(&cid.to_string(), role, tool, input);
        if verdict.forbid.is_empty() {
            if verdict.unpermitted {
                return Some(format!("refused: no law permits {tool} for the {role} role"));
            }
            return None;
        }
        self.record(cid, &verdict.forbid, tool, apz_plaza::now_ms());
        let titles: Vec<&str> =
            verdict.forbid.iter().filter_map(|id| self.titles.get(id)).map(String::as_str).collect();
        Some(format!("refused by law {}: {}", verdict.forbid.join(", "), titles.join("; ")))
    }

    /// Every law, in file then source order, for the Rules view.
    pub fn catalog(&self) -> Vec<LawInfo> {
        let sources = &self.program.sources;
        let mut out = Vec::new();
        for module in &self.program.modules {
            let file = sources.files[module.file as usize].path.clone();
            for item in &module.items {
                if let Item::Law(law) = item {
                    out.push(law_info(law, &file, sources));
                }
            }
        }
        out
    }

    /// Every violation recorded from now on is also handed to `sink`.
    pub fn set_sink(&self, sink: InfractionSink) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Every recorded violation, as (cid, violation), for the Rules view and the one-time import.
    pub fn all_violations(&self) -> Vec<(u64, Violation)> {
        let all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
        all.iter()
            .flat_map(|(cid, list)| {
                let cid = cid.parse::<u64>().unwrap_or(0);
                list.iter().map(move |v| (cid, v.clone()))
            })
            .collect()
    }

    /// The agent's violations within the last 30 days, newest first, as profile JSON.
    pub fn recent(&self, cid: u64, now_ms: u64) -> Vec<Json> {
        let all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(list) = all.get(&cid.to_string()) else { return Vec::new() };
        let mut found: Vec<&Violation> = list.iter().filter(|v| v.at_ms + WINDOW_MS >= now_ms).collect();
        found.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
        found.into_iter().map(|v| serde_json::to_value(v).unwrap_or(Json::Null)).collect()
    }

    fn record(&self, cid: u64, laws: &[String], tool: &str, at_ms: u64) {
        let recorded: Vec<Violation> = laws
            .iter()
            .map(|law| Violation { law: law.clone(), at_ms, detail: format!("tool {tool} refused") })
            .collect();
        {
            let mut all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
            let list = all.entry(cid.to_string()).or_default();
            list.extend(recorded.iter().cloned());
            let excess = list.len().saturating_sub(KEEP);
            list.drain(..excess);
            if let Err(e) = save(&self.path, &all) {
                eprintln!("laws: could not save violations: {e}");
            }
        }
        // The sink runs outside the violations lock: it writes to Postgres.
        let guard = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sink) = guard.as_ref() {
            for violation in &recorded {
                sink(cid, violation, tool);
            }
        }
    }
}

/// The compiled-in laws for questions asked outside an `App` (tool offers), compiled once.
/// Only [`Laws::permits`] is answered from it, which records nothing, so its violations path is never written.
pub fn global() -> &'static Laws {
    static LAWS: OnceLock<Laws> = OnceLock::new();
    LAWS.get_or_init(|| Laws::load(&std::env::temp_dir().join("apz-laws-gate")))
}

/// Writes the violations atomically: a temporary file, then a rename over the old one.
fn save(path: &Path, all: &BTreeMap<String, Vec<Violation>>) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(all).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Reads `law "<id>" "<title>" {` headers from a law source.
fn titles_of(text: &str, titles: &mut BTreeMap<String, String>) {
    for line in text.lines().filter(|line| line.starts_with("law \"")) {
        let quoted: Vec<&str> = line.split('"').collect();
        if let (Some(id), Some(title)) = (quoted.get(1), quoted.get(3)) {
            titles.insert(id.to_string(), title.to_string());
        }
    }
}

/// The flags the laws test, read from a tool call: an actual secret in the input (not the
/// mere words "token" or "password", which ordinary code is full of), a read of a credential
/// store, an attempt to pass as the owner, and — for shell commands only — a long encoded
/// blob that hides what a command does.
pub fn detect_flags(tool: &str, input: &Json) -> BTreeSet<&'static str> {
    let text = input.to_string();
    let lower = text.to_lowercase();
    let mut flags = BTreeSet::new();
    if has_secret(&text) || reads_credential_store(&lower) {
        flags.insert("credential_seeking");
    }
    if lower.contains("impersonat") || lower.contains("pretend to be the owner") || lower.contains("pretend to be the operator") {
        flags.insert("impersonation");
    }
    if matches!(tool, "bash" | "bash_start") && longest_base64_run(&text) > 400 {
        flags.insert("obfuscation");
    }
    flags
}

/// Whether `text` carries a secret by its well-known shape: provider API keys and tokens,
/// cloud access keys, chat-platform tokens and private-key blocks.
fn has_secret(text: &str) -> bool {
    if text.contains("PRIVATE KEY-----") {
        return true;
    }
    // (prefix, minimum length of the token-character run after it)
    const SHAPES: [(&str, usize); 9] = [
        ("sk-ant-", 20), ("sk-proj-", 20), ("sk-", 32), ("ghp_", 30), ("github_pat_", 30),
        ("xoxb-", 20), ("xoxp-", 20), ("AIza", 35), ("AKIA", 16),
    ];
    let token_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    for (prefix, min) in SHAPES {
        let mut from = 0;
        while let Some(at) = text[from..].find(prefix) {
            let start = from + at;
            // The prefix must start a token, not sit inside a longer word.
            let boundary = text[..start].chars().next_back().is_none_or(|c| !token_char(c));
            let run = text[start + prefix.len()..].chars().take_while(|c| token_char(*c)).count();
            if boundary && run >= min {
                return true;
            }
            from = start + prefix.len();
        }
    }
    false
}

/// Whether a command reaches for a credential store.
fn reads_credential_store(lower: &str) -> bool {
    ["/.ssh/", "id_rsa", "id_ed25519", "/etc/shadow", ".aws/credentials", ".netrc", ".git-credentials", ".docker/config.json"]
        .iter()
        .any(|p| lower.contains(p))
}

/// The longest run of base64 alphabet characters in `text`.
fn longest_base64_run(text: &str) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Guest tools that act in the agent's own VM: `tool.exec` on its sandbox as well.
const GUEST_EXEC: &[&str] = &[
    "bash", "bash_start", "write_file", "edit_file", "multi_edit", "download", "fetch_url", "install_packages", "code_engine", "tool_package", "deliver_file",
    "checkpoint_restore",
];

/// Where a tool comes from, as the tool laws (`tools.jhrl`) read it: a connector's action, a
/// library tool another agent built, a host tool apid runs, or a guest tool in the agent's VM.
fn tool_kind(tool: &str) -> &'static str {
    if crate::connectors::by_tool(tool).is_some() {
        "connector"
    } else if tool.starts_with(crate::library::PREFIX) {
        "library"
    } else if runs(tool) == Some(Runs::Host) {
        "host"
    } else {
        "guest"
    }
}

/// The catalog's risk class for a tool; the constitution's irreversible-action law (C-004a) reads
/// it. No tool is `irreversible` yet: payments are capped transfers and verdicts are reviewable, and
/// owner approvals are not threaded through tool calls, so marking them would refuse them outright.
pub(crate) fn risk_class(tool: &str) -> &'static str {
    match tool {
        "read" | "read_file" | "list" | "list_files" | "search" | "plaza_tasks" | "plaza_agents" | "memory_read"
        | "memory_search" | "memory_list" | "transcript_search" | "plaza_contacts" | "glob" | "grep" | "bash_output"
        | "todo_write" | "checkpoint_diff" | "checkpoint_list" => "read",
        t if t.starts_with("plaza_") && t.ends_with("_read") => "read",
        t if crate::connectors::risk_of(t).is_some() => crate::connectors::risk_of(t).unwrap_or("write"),
        _ => "write",
    }
}


impl Laws {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
        serde_json::to_string_pretty(&*all).unwrap_or_default()
    }

    /// Replaces the state with a [`Laws::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let violations: BTreeMap<String, Vec<Violation>> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
        save(&self.path, &violations).map_err(|e| e.to_string())?;
        *all = violations;
        Ok(())
    }
}

/// One law as the Rules view shows it: its fields, its source text and a generated sentence.
fn law_info(law: &Law, file: &str, sources: &SourceMap) -> LawInfo {
    let principal = law.principal.as_ref().map(|s| scope_text("principal", s, sources)).unwrap_or_default();
    let resource = law.resource.as_ref().map(|s| scope_text("resource", s, sources)).unwrap_or_default();
    let conditions: Vec<String> = law.when.iter().map(|e| sources.snippet(e.span).trim().to_string()).collect();
    let tier = law.tier.clone().unwrap_or_else(|| file_tier(file).to_string());
    let explanation = explain(law, &principal, &conditions);
    LawInfo {
        id: law.id.clone(),
        title: law.title.clone(),
        tier,
        enforcement: law.enforcement.clone().unwrap_or_default(),
        severity: law.severity.clone().unwrap_or_default(),
        statement: law.statement.clone().unwrap_or_default(),
        rationale: law.rationale.clone().unwrap_or_default(),
        effect: match law.effect {
            Effect::Permit => "permit",
            Effect::Forbid => "forbid",
        }
        .to_string(),
        ops: law.ops.clone(),
        principal,
        resource,
        conditions,
        explanation,
        file: file.to_string(),
        source: sources.snippet(law.span).to_string(),
    }
}

/// The tier a law gets from its file when it names none.
fn file_tier(file: &str) -> &'static str {
    match file {
        "constitution.jhrl" => "constitution",
        "statutes.jhrl" => "statute",
        "operations.jhrl" => "operations",
        _ => "",
    }
}

/// A scope as source text, e.g. `principal is Agent` or `resource == x`.
fn scope_text(subject: &str, scope: &Scope, sources: &SourceMap) -> String {
    match scope {
        Scope::Is(t) => format!("{subject} is {}", type_name(t)),
        Scope::Eq(e) => format!("{subject} == {}", sources.snippet(e.span).trim()),
        Scope::In(e) => format!("{subject} in {}", sources.snippet(e.span).trim()),
    }
}

fn type_name(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named(name, _, _) => name.clone(),
        _ => String::new(),
    }
}

/// The plain sentence: who may (not) do what, on what, and under which conditions.
fn explain(law: &Law, principal: &str, conditions: &[String]) -> String {
    let verb = if law.ops.is_empty() {
        "act".to_string()
    } else {
        law.ops.iter().map(|op| verb_of(op)).collect::<Vec<_>>().join(" or ")
    };
    let own = conditions.iter().any(|c| c.contains("resource.agent == principal"));
    let object = match &law.resource {
        Some(Scope::Is(t)) => {
            let name = type_name(t).to_lowercase();
            if own {
                format!(" its own {name}")
            } else {
                format!(" {}", with_article(&name))
            }
        }
        _ => String::new(),
    };
    let who = who_of(law, principal, conditions);
    let tail = if conditions.is_empty() { "" } else { " under the conditions below" };
    match law.effect {
        Effect::Permit => format!("{} may {verb}{object}{tail}.", capitalized(&who)),
        Effect::Forbid if who == "any agent" => format!("No agent may {verb}{object}{tail}."),
        Effect::Forbid => format!("{} may not {verb}{object}{tail}.", capitalized(&who)),
    }
}

/// Who the law applies to, as a noun phrase: a role when one is named, else the principal's type.
fn who_of(law: &Law, principal: &str, conditions: &[String]) -> String {
    let role = std::iter::once(principal).chain(conditions.iter().map(String::as_str)).find_map(role_named);
    if let Some(role) = role {
        return with_article(&role.to_lowercase());
    }
    match &law.principal {
        None => "anyone".to_string(),
        Some(Scope::Is(t)) => match type_name(t).as_str() {
            "Agent" => "any agent".to_string(),
            "Owner" => "the owner".to_string(),
            other => with_article(&other.to_lowercase()),
        },
        Some(_) => "anyone".to_string(),
    }
}

/// The role a scope or condition names: `principal.role == "builder"` or `Role::"builder"`.
fn role_named(text: &str) -> Option<String> {
    let start = ["principal.role == \"", "Role::\""].iter().find_map(|marker| text.find(marker).map(|i| i + marker.len()))?;
    let rest = &text[start..];
    Some(rest[..rest.find('"')?].to_string())
}

/// The plain verb for an operation name.
fn verb_of(op: &str) -> String {
    match op {
        "tool.exec" => "run commands in".to_string(),
        "tool.call" => "call".to_string(),
        "message.send" => "message".to_string(),
        "plan.approve" => "approve plans for".to_string(),
        "vault.read" => "read".to_string(),
        "egress.connect" => "connect out to".to_string(),
        "artifact.publish" => "publish".to_string(),
        "memory.change" => "change the memory of".to_string(),
        "device.actuate" => "operate".to_string(),
        "device.read" => "read".to_string(),
        "audit.modify" => "change the audit log".to_string(),
        "audit.read" => "read the audit log".to_string(),
        "gateway.configure" => "reconfigure the gateway".to_string(),
        "policy.modify" => "change the laws".to_string(),
        "vault.list" => "list".to_string(),
        "sandbox.event" => "report sandbox events for".to_string(),
        "approval.request" => "request".to_string(),
        "registry.set_name" => "set its name in".to_string(),
        "task.create" => "create tasks in".to_string(),
        "task.assign" => "assign".to_string(),
        "merge.approve" => "approve merges for".to_string(),
        "review.submit" => "submit reviews of".to_string(),
        "agent.spawn" => "start agents in".to_string(),
        "agent.hibernate" => "put agents to sleep in".to_string(),
        "agent.quarantine" => "quarantine".to_string(),
        "usage.read" => "read the usage ledger".to_string(),
        "claim.submit" => "claim work on".to_string(),
        "plan.submit" => "submit plans to".to_string(),
        "report.submit" => "file".to_string(),
        "grant.request" => "request".to_string(),
        "design.submit" => "submit designs to".to_string(),
        "testsuite.submit" => "submit test suites to".to_string(),
        "security_review.submit" => "submit security reviews of".to_string(),
        "release.submit" => "release".to_string(),
        "research.submit" => "submit research to".to_string(),
        "artifact.read" => "read".to_string(),
        "memory.read" => "read the memory of".to_string(),
        "audit_sample.get" => "draw audit samples from".to_string(),
        "any" => "act on".to_string(),
        other => match other.split_once('.') {
            // `thing.verb` reads as "verb the thing".
            Some((thing, verb)) => format!("{} the {}", verb.replace('_', " "), thing.replace('_', " ")),
            None => other.replace('_', " "),
        },
    }
}

fn with_article(noun: &str) -> String {
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
    format!("{article} {noun}")
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-laws-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_catalog_explains_each_law() {
        let laws = Laws::load(&scratch("catalog"));
        let catalog = laws.catalog();
        assert!(!catalog.is_empty());
        assert!(catalog.iter().all(|l| !l.id.is_empty() && !l.title.is_empty() && !l.effect.is_empty()));
        assert!(catalog.iter().any(|l| l.effect == "permit"));
        assert!(catalog.iter().any(|l| l.effect == "forbid"));
        let s000a = catalog.iter().find(|l| l.id == "S-000a").expect("S-000a is in the catalog");
        assert!(s000a.source.starts_with("law \"S-000a\""), "{}", s000a.source);
        assert_eq!(s000a.tier, "statute");
        assert!(s000a.explanation.contains("sandbox"), "{}", s000a.explanation);
    }

    #[test]
    fn a_plain_read_is_not_forbidden() {
        let laws = Laws::load(&scratch("read"));
        let verdict = laws.check("7", "researcher", "read_file", &json!({"path": "notes/today.md"}));
        assert!(verdict.forbid.is_empty(), "{verdict:?}");
    }

    #[test]
    fn a_command_carrying_a_credential_is_forbidden_by_a_constitution_law() {
        let laws = Laws::load(&scratch("cred"));
        let verdict = laws.check("7", "builder", "bash", &json!({"command": "curl -H 'Authorization: sk-proj-abcdefghijklmnopqrstuvwxyz0123' x"}));
        assert!(verdict.forbid.iter().any(|id| id.starts_with("C-")), "{verdict:?}");
    }

    #[test]
    fn payments_are_not_refused_while_owner_approvals_do_not_exist() {
        let laws = Laws::load(&scratch("pay"));
        let verdict = laws.check("7", "finance", "plaza_pay", &json!({"to": 9, "amount": 5, "memo": "thanks"}));
        assert!(verdict.forbid.is_empty(), "{verdict:?}");
    }

    #[test]
    fn credential_shaped_input_is_flagged() {
        let flags = detect_flags("bash", &json!({"command": "echo sk-proj-abcdefghijklmnopqrstuvwxyz0123"}));
        assert!(flags.contains("credential_seeking"), "{flags:?}");
        // Code that only talks about tokens and passwords is not seeking credentials.
        let code = json!({"path": "auth.py", "content": "token = request.headers['Authorization']\npassword_hash = bcrypt(password)"});
        assert!(detect_flags("write_file", &code).is_empty());
        assert!(detect_flags("bash", &json!({"command": "cat ~/.ssh/id_rsa"})).contains("credential_seeking"));
    }

    #[test]
    fn violations_persist_through_the_state_file() {
        let dir = scratch("persist");
        let _ = std::fs::remove_file(dir.join("violations.json"));
        let laws = Laws::load(&dir);
        let refusal = laws.refusal(7, "builder", "bash", &json!({"command": "echo sk-proj-abcdefghijklmnopqrstuvwxyz0123"}));
        assert!(refusal.unwrap_or_default().starts_with("refused by law C-"));
        let again = Laws::load(&dir);
        let found = again.recent(7, apz_plaza::now_ms());
        assert!(!found.is_empty());
        assert!(found[0]["law"].as_str().unwrap_or_default().starts_with("C-"));
    }

    #[test]
    fn every_tool_offered_to_a_role_is_permitted_by_a_law() {
        let laws = Laws::load(&scratch("offered"));
        let mut roles = vec!["builder", "reviewer", "mayor", "planner", "finance"];
        roles.push(crate::settings::ORCHESTRATOR_ROLE);
        for role in roles {
            for tool in crate::tools::offered_names(role) {
                let verdict = laws.check("7", role, &tool, &json!({}));
                assert!(!verdict.unpermitted, "{role} is offered {tool}, but no law permits it");
                assert!(verdict.forbid.is_empty(), "{role} calling {tool}: {verdict:?}");
            }
        }
    }

    #[test]
    fn a_builder_may_not_post_a_task() {
        let laws = Laws::load(&scratch("builder-post"));
        let verdict = laws.check("7", "builder", "plaza_post_task", &json!({}));
        assert!(verdict.unpermitted, "{verdict:?}");
    }

    #[test]
    fn a_call_no_law_permits_is_refused_and_not_recorded() {
        let dir = scratch("default-deny");
        let _ = std::fs::remove_file(dir.join("violations.json"));
        let laws = Laws::load(&dir);
        let refusal = laws.refusal(7, "builder", "plaza_post_task", &json!({}));
        assert_eq!(refusal.as_deref(), Some("refused: no law permits plaza_post_task for the builder role"));
        assert!(laws.all_violations().is_empty());
        assert!(laws.recent(7, apz_plaza::now_ms()).is_empty());
    }

    #[test]
    fn permits_follows_the_tier_and_seat_laws() {
        let laws = Laws::load(&scratch("permits"));
        assert!(laws.permits("mayor", "plaza_project_approve"));
        assert!(!laws.permits("builder", "plaza_project_approve"));
        assert!(!laws.permits("builder", "plaza_post_task"));
        assert!(laws.permits("builder", "plaza_bid"));
        assert!(laws.permits(crate::settings::ORCHESTRATOR_ROLE, "plaza_post_task"));
        assert!(laws.permits("builder", "bash"));
        // A second ask is answered from the memo, the same way.
        assert!(laws.permits("mayor", "plaza_project_approve"));
    }
}
