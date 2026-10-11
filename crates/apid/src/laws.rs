//! The plaza's laws, enforced on every tool call an agent makes (JHRL, `crates/jhrl`).
//!
//! `check` decides one tool call against the laws that cover `tool.call`. An applying forbid
//! refuses the call and is recorded as a violation on the agent's profile. A call that no law
//! permits is allowed and logged, because the role bundles do not yet permit every tool.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use jhrl::authz::{authorize, Decision, Entity, EntityStore, Request};
use jhrl::value::Value;
use jhrl::Program;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The law files, compiled into the binary. `cases.jhrl` is the test fixture set, not law.
const LAW_FILES: [(&str, &str); 3] = [
    ("operations.jhrl", include_str!("../../jhrl/specs/laws/operations.jhrl")),
    ("constitution.jhrl", include_str!("../../jhrl/specs/laws/constitution.jhrl")),
    ("statutes.jhrl", include_str!("../../jhrl/specs/laws/statutes.jhrl")),
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
    /// True when no law permitted the call and none forbade it (default deny, not yet enforced).
    pub unpermitted: bool,
}

pub struct Laws {
    program: Program,
    /// Law id to its title, read from the law source.
    titles: BTreeMap<String, String>,
    path: PathBuf,
    violations: Mutex<BTreeMap<String, Vec<Violation>>>,
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
        Laws { program, titles, path, violations: Mutex::new(violations) }
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
            BTreeMap::from([("role".to_string(), Value::str(role)), ("town".to_string(), Value::str("plaza"))])
        };
        store.insert(&agent, member(role));
        store.insert(&tool_entity, BTreeMap::from([("risk_class".to_string(), Value::str(risk_class(tool)))]));
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

    /// The refusal reason when a law forbids the call (recorded as a violation); `None` when it may run.
    pub fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Json) -> Option<String> {
        let verdict = self.check(&cid.to_string(), role, tool, input);
        if verdict.forbid.is_empty() {
            if verdict.unpermitted {
                eprintln!("laws: agent {cid} calls {tool}; no law permits it yet, allowed");
            }
            return None;
        }
        self.record(cid, &verdict.forbid, tool, apz_plaza::now_ms());
        let titles: Vec<&str> =
            verdict.forbid.iter().filter_map(|id| self.titles.get(id)).map(String::as_str).collect();
        Some(format!("refused by law {}: {}", verdict.forbid.join(", "), titles.join("; ")))
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
        let mut all = self.violations.lock().unwrap_or_else(PoisonError::into_inner);
        let list = all.entry(cid.to_string()).or_default();
        for law in laws {
            list.push(Violation { law: law.clone(), at_ms, detail: format!("tool {tool} refused") });
        }
        let excess = list.len().saturating_sub(KEEP);
        list.drain(..excess);
        if let Err(e) = save(&self.path, &all) {
            eprintln!("laws: could not save violations: {e}");
        }
    }
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
    "bash", "bash_start", "write_file", "edit_file", "multi_edit", "download", "fetch_url", "install_packages", "code_engine", "tool_package",
    "checkpoint_restore",
];

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
}
