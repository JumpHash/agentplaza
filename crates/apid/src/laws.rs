//! The plaza's laws, enforced on every tool call an agent makes (JHRL, `crates/jhrl`).
//!
//! The law set is the built-in statutes (`LAW_FILES`) and the owner's rule sets (`RuleSet`), less the
//! built-ins the owner has disabled (`RuleSet::overrides`) and the laws of disabled rules. It is compiled
//! as one program and swapped whole on every change, so a check always reads one consistent set. A rule
//! source that does not compile is never applied.
//!
//! `check` decides one tool call against the laws that cover `tool.call`. An applying forbid
//! refuses the call. A call that no law permits is refused too (default deny): a missing permission.
//! `permits` answers for a role and tool alone; it decides which tools an agent is offered.
//! With strict permits on, an operation that has permit laws is refused unless one of them allows it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};

use jhrl::Program;
use jhrl::ast::{Effect, Expr, ExprKind, Item, Law, PathSeg, Scope, TypeExpr};
use jhrl::authz::{Decision, Entity, EntityStore, Request, authorize_except, check_authz};
use jhrl::span::{Diag, SourceMap};
use jhrl::value::Value;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::settings::{Tier, tier_of};
use crate::tools::{Runs, runs};

/// The law files, compiled into the binary. `cases.jhrl` is the test fixture set, not law.
pub(crate) const LAW_FILES: [(&str, &str); 4] = [
    (
        "operations.jhrl",
        include_str!("../../jhrl/specs/laws/operations.jhrl"),
    ),
    (
        "constitution.jhrl",
        include_str!("../../jhrl/specs/laws/constitution.jhrl"),
    ),
    (
        "statutes.jhrl",
        include_str!("../../jhrl/specs/laws/statutes.jhrl"),
    ),
    (
        "tools.jhrl",
        include_str!("../../jhrl/specs/laws/tools.jhrl"),
    ),
];
/// Tool calls kept for the dry run of a rule source ("would have refused").
const CALLS_KEPT: usize = 500;
/// The largest input (as JSON text) a call keeps for the dry run; a bigger one is not replayed.
const INPUT_KEPT: usize = 16_000;
/// The most recent refusals a dry run lists.
const SAMPLES: usize = 50;
/// The id a dry run compiles its source under; never stored.
const DRY_RUN_ID: &str = "DRY";
/// The tier of the constitution: its laws are switched off only with a typed confirmation.
pub const CONSTITUTION: &str = "constitution";
/// Changes to constitutional laws kept in the history; the oldest go first.
const HISTORY_KEPT: usize = 500;
/// The author of every owner rule and override: the owner is the only one who may write them.
pub const OWNER: &str = "owner";

/// The outcome of checking one tool call.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Ids of the applying forbid laws; non-empty means the call is refused.
    pub forbid: Vec<String>,
    /// True when no law permitted the call and none forbade it (default deny: refused, not recorded).
    pub unpermitted: bool,
    /// Every operation the call was checked as (`tool.call` first), in request order.
    pub requested: Vec<String>,
    /// Ids of the permit laws that applied to some requested operation.
    pub permit: Vec<String>,
    /// Operations that have permit laws, none of which allowed them, with strict permits on.
    pub denied_ops: Vec<String>,
}

/// The context one operation's laws read: the flags, and what the operation adds to them.
type Context = BTreeMap<String, Value>;

/// One law as the Rules view shows it.
#[derive(Clone, Debug, Serialize)]
pub struct LawInfo {
    pub id: String,
    pub title: String,
    /// "constitution" | "statute" | "operations" (the law's tier, or its file's); "owner" for an owner rule.
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
    /// The actions it covers, e.g. "tool.exec", "message.send".
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
    /// "built-in" | "owner".
    pub origin: String,
    /// Whether the law applies: its rule is enabled (or it is built in) and no override disables it.
    pub enabled: bool,
    /// The owner rule the law came from ("" for a built-in).
    pub rule_id: String,
    /// The owner's override that disables this built-in, if any.
    pub overridden: Option<Override>,
}

/// An owner rule set: JHRL source with one or more `law` blocks, applied while it is enabled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct OwnerRule {
    pub id: String,
    pub title: String,
    pub source: String,
    pub enabled: bool,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub author: String,
}

/// The owner's disabling of one built-in statute, with who did it and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Override {
    pub law: String,
    pub disabled_by: String,
    pub reason: String,
    pub at_ms: u64,
}

/// One change of state of a constitutional law, kept in the rules' history: who made it, when, and
/// whether the owner confirmed it by typing the law's id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LawChange {
    pub law: String,
    pub title: String,
    pub tier: String,
    /// True when the change switched the law off; false when it switched it back on.
    pub disabled: bool,
    pub by: String,
    pub reason: String,
    pub at_ms: u64,
    /// True when the switch-off carried the typed confirmation (`confirm` equal to the law's id).
    pub confirmed: bool,
}

/// The owner's rules and overrides: the state that is persisted (the `rules` tables, saved by the store's saver).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
#[table(name = "rules")]
pub struct RuleSet {
    #[table(child = "owner_rule")]
    pub rules: Vec<OwnerRule>,
    #[table(child = "law_override")]
    pub overrides: Vec<Override>,
    /// The changes made to constitutional laws, oldest first; the newest [`HISTORY_KEPT`] are kept.
    #[table(child = "law_change")]
    pub history: Vec<LawChange>,
}

/// One compile error: the file it is in, where, and what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// The file: a rule's `owner/<id>.jhrl`, or a built-in law file.
    pub file: String,
    /// Line and column, from 1; 0 when the error has no place in a file.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// Why a change to the laws was refused. Nothing is applied when one is returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The source does not compile.
    Diagnostics(Vec<Diagnostic>),
    /// No rule or law has that id.
    NotFound(String),
    /// The change is not allowed, and why.
    Invalid(String),
    /// A constitutional law was not switched off with its typed confirmation (409).
    Unconfirmed(String),
}

impl Failure {
    /// The failure as one line of text.
    pub fn message(&self) -> String {
        match self {
            Failure::Diagnostics(list) => list
                .iter()
                .map(|d| format!("{}:{}: {}", d.line, d.column, d.message))
                .collect::<Vec<_>>()
                .join("; "),
            Failure::NotFound(what) => format!("not found: {what}"),
            Failure::Invalid(why) | Failure::Unconfirmed(why) => why.clone(),
        }
    }
}

/// What a dry run found: the laws its source declares, and the recent calls those laws would refuse.
#[derive(Debug, Serialize)]
pub struct DryRun {
    pub laws: Vec<String>,
    pub would_refuse: Vec<Json>,
}

/// The compiled live set: what every check reads. Built whole, then swapped in.
struct Compiled {
    program: Program,
    /// Law id to its title, read from the law sources.
    titles: BTreeMap<String, String>,
    /// Laws left out of the program: overridden built-ins and the laws of disabled rules.
    off: HashSet<String>,
    /// Every law in the program, enabled or not, for the Rules view.
    catalog: Vec<LawInfo>,
    /// The operations some enabled permit law covers ("any" covers all).
    permit_ops: HashSet<String>,
}

impl Compiled {
    /// Whether an enabled permit law covers `op`, so strict permits apply to it.
    fn has_permit(&self, op: &str) -> bool {
        self.permit_ops.contains(op) || self.permit_ops.contains("any")
    }
}

/// A tool call the laws were asked about, kept for the dry run of a new rule.
#[derive(Clone, Debug)]
struct Seen {
    cid: u64,
    role: String,
    tool: String,
    input: Json,
    at_ms: u64,
}

pub struct Laws {
    /// The built-in law sources: `LAW_FILES`, or the sources given to `from_files`.
    base: Vec<(String, String)>,
    /// The live set, swapped whole on every change; a check reads one `Arc` of it.
    live: RwLock<Arc<Compiled>>,
    /// The owner's rules and overrides. Changes are made under this lock, then committed.
    rules: Mutex<RuleSet>,
    /// Memoized [`Laws::permits`] answers, keyed by (role, tool). Cleared whenever the laws change.
    permitted: Mutex<HashMap<(String, String), bool>>,
    /// Whether an operation with permit laws must be allowed by one of them (`settings.laws`).
    strict: AtomicBool,
    /// The laws' generation: bumped by every change to the live set or to strictness. The laws are process-wide
    /// (`global`), so the generation is theirs rather than the hub's (see `sources`).
    generation: AtomicU64,
    /// The most recent tool calls checked, for the dry run.
    seen: Mutex<VecDeque<Seen>>,
    /// Why the stored owner rules were not applied at start, until a change applies them.
    not_applied: Mutex<Option<String>>,
}

impl Laws {
    /// The built-in laws, compiled, with no owner rules yet (the store's load adds them).
    /// Panics at startup if the built-in laws do not compile: an unenforced law is worse than no start.
    pub fn builtin() -> Laws {
        let files: Vec<(String, String)> = LAW_FILES
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect();
        Laws::from_files(files)
    }

    /// [`Laws::builtin`] for the given built-in law sources (`(file name, text)`). Tests use it to put a
    /// rule in or leave one out.
    pub fn from_files(files: Vec<(String, String)>) -> Laws {
        let compiled = match compile(&files, &RuleSet::default()) {
            Ok(compiled) => compiled,
            Err(failure) => panic!("the laws do not compile: {}", failure.message()),
        };
        Laws {
            base: files,
            live: RwLock::new(Arc::new(compiled)),
            rules: Mutex::new(RuleSet::default()),
            permitted: Mutex::new(HashMap::new()),
            strict: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            seen: Mutex::new(VecDeque::new()),
            not_applied: Mutex::new(None),
        }
    }

    /// Decides one tool call. `input` is the call's JSON input, scanned for the flags the laws test.
    ///
    /// Every call is a `tool.call`; a command or file change in the agent's own VM is also a
    /// `tool.exec` on its sandbox, and a direct message also a `message.send` to its recipient, so
    /// the laws written for those operations apply too. Only `tool.call`'s default deny is
    /// reported as `unpermitted`.
    pub fn check(&self, agent_cid: &str, role: &str, tool: &str, input: &Json) -> Verdict {
        self.check_with(&self.live(), agent_cid, role, tool, input)
    }

    fn check_with(
        &self,
        live: &Compiled,
        agent_cid: &str,
        role: &str,
        tool: &str,
        input: &Json,
    ) -> Verdict {
        let flags = detect_flags(tool, input);
        let agent = Entity {
            ty: "Agent".into(),
            id: agent_cid.into(),
        };
        let tool_entity = Entity {
            ty: "Tool".into(),
            id: tool.into(),
        };
        let sandbox = Entity {
            ty: "Sandbox".into(),
            id: format!("vm-{agent_cid}"),
        };
        let mut store = EntityStore::default();
        store.insert(&agent, member_attrs_of(agent_cid, role));
        store.insert(
            &tool_entity,
            BTreeMap::from([
                ("name".to_string(), Value::str(tool)),
                ("kind".to_string(), Value::str(tool_kind(tool))),
                ("risk_class".to_string(), Value::str(risk_class(tool))),
            ]),
        );
        store.insert(
            &sandbox,
            BTreeMap::from([("agent".to_string(), jhrl::authz::entity_value(&agent))]),
        );
        let flag_set = Value::set(flags.iter().map(|flag| Value::str(flag)));
        let base: Context = BTreeMap::from([("flags".to_string(), flag_set)]);
        let mut requests: Vec<(&str, Entity, Context)> =
            vec![("tool.call", tool_entity, Context::new())];
        if GUEST_EXEC.contains(&tool) || tool.starts_with(crate::library::PREFIX) {
            requests.push(("tool.exec", sandbox, Context::new()));
        }
        // A message is a message.send to its recipient, and a board post one to the whole board
        // (no recipient). The recipient is checked even when its CID is missing or malformed: the
        // message laws then see an empty recipient instead of being skipped.
        if MESSAGE_TOOLS.contains(&tool) {
            // A seat message's recipient is its seat address (`T12/reviewer`, `planner`).
            let to = input
                .get("to_cid")
                .and_then(Json::as_u64)
                .map(|n| n.to_string())
                .or_else(|| input.get("to").and_then(Json::as_str).map(str::to_string))
                .unwrap_or_default();
            let peer = Entity {
                ty: "Agent".into(),
                id: to,
            };
            store.insert(&peer, member_attrs_of(&peer.id, ""));
            requests.push(("message.send", peer, Context::new()));
        }
        // An artifact tool is an artifact operation: a file shared or delivered to the plaza is published, an
        // artifact the agent makes or changes is created (permitted to every agent), and asking the owner to share
        // one is a share request (the owner's approval is the gate).
        let artifact_op = artifact_operation(tool);
        if let Some(operation) = artifact_op {
            let artifact = Entity {
                ty: "Artifact".into(),
                id: tool.into(),
            };
            store.insert(&artifact, BTreeMap::new());
            requests.push((operation, artifact, Context::new()));
        }
        requests.extend(plaza_operations(tool, input, role, &mut store));
        let strict = self.strict.load(Ordering::Relaxed);
        let mut verdict = Verdict::default();
        for (i, (operation, resource, extra)) in requests.into_iter().enumerate() {
            verdict.requested.push(operation.to_string());
            let mut context = base.clone();
            context.extend(extra);
            let request = Request {
                principal: agent.clone(),
                operation: operation.into(),
                resource,
                context: Value::record(context),
            };
            match authorize_except(&live.program, &store, &request, &live.off).decision {
                Decision::Deny { by, .. } if !by.is_empty() => {
                    for id in by {
                        if !verdict.forbid.contains(&id) {
                            verdict.forbid.push(id);
                        }
                    }
                }
                Decision::Deny { .. } if i == 0 => verdict.unpermitted = true,
                Decision::Deny { .. } if strict && live.has_permit(operation) => {
                    verdict.denied_ops.push(operation.to_string())
                }
                Decision::Allow { by } => {
                    for id in by {
                        if !verdict.permit.contains(&id) {
                            verdict.permit.push(id);
                        }
                    }
                }
                _ => {}
            }
        }
        verdict
    }

    /// Whether the laws let `role` use `tool` at all: no forbid applies and a permit covers it.
    /// Decided on the role and tier alone (an empty input), and memoized.
    pub fn permits(&self, role: &str, tool: &str) -> bool {
        let key = (role.to_string(), tool.to_string());
        if let Some(answer) = lock(&self.permitted).get(&key) {
            return *answer;
        }
        let live = self.live();
        let verdict = self.check_with(&live, "0", role, tool, &Json::Object(Default::default()));
        let answer =
            verdict.forbid.is_empty() && !verdict.unpermitted && verdict.denied_ops.is_empty();
        let mut memo = lock(&self.permitted);
        // Remembered only while the laws it was decided under are still the live ones.
        if Arc::ptr_eq(&live, &self.live()) {
            memo.insert(key, answer);
        }
        answer
    }

    /// Whether a call of `tool` by agent `cid` (of `role`) with no input at all would pass [`Laws::refusal`]: no law
    /// forbids it, one permits it, and strict permits (if on) are met. The policy snapshot asks this per agent, so it
    /// is not memoized (see [`Laws::permits`], which answers for a role alone).
    pub fn permits_for(&self, cid: u64, role: &str, tool: &str) -> bool {
        let verdict = self.check(
            &cid.to_string(),
            role,
            tool,
            &Json::Object(Default::default()),
        );
        verdict.forbid.is_empty() && !verdict.unpermitted && verdict.denied_ops.is_empty()
    }

    /// Whether a call of `tool` can be decided differently for different inputs: an enabled law that can decide it
    /// reads the input (its flags, or a law that expires), or the tool's recipient is read from the input (a message).
    /// Such a tool is not decided by its name alone.
    pub fn tests_input(&self, tool: &str) -> bool {
        if MESSAGE_TOOLS.contains(&tool) {
            return true;
        }
        let live = self.live();
        let ops = operations_of(tool);
        live.program
            .modules
            .iter()
            .flat_map(|module| module.items.iter())
            .any(|item| match item {
                Item::Law(law) => {
                    !live.off.contains(&law.id)
                        && law
                            .ops
                            .iter()
                            .any(|op| op == "any" || ops.contains(&op.as_str()))
                        && law_reads_input(law)
                }
                _ => false,
            })
    }

    /// The refusal reason when a law forbids the call, or no law permits it (default deny); `None`
    /// when it may run.
    pub fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Json) -> Option<String> {
        self.remember(cid, role, tool, input);
        let verdict = self.check(&cid.to_string(), role, tool, input);
        if verdict.forbid.is_empty() {
            if verdict.unpermitted {
                return Some(format!(
                    "refused: no law permits {tool} for the {role} role"
                ));
            }
            if let Some(op) = verdict.denied_ops.first() {
                return Some(format!("refused: no law permits {op} for the {role} role"));
            }
            return None;
        }
        let live = self.live();
        let titles: Vec<&str> = verdict
            .forbid
            .iter()
            .filter_map(|id| live.titles.get(id))
            .map(String::as_str)
            .collect();
        Some(format!(
            "refused by law {}: {}",
            verdict.forbid.join(", "),
            titles.join("; ")
        ))
    }

    /// The refusal reason when a law forbids an operation an agent's VM performs straight at a hub
    /// route, outside any tool call (a git push, a file publish), `None` when no law forbids it. Only
    /// forbids are decided here (and, with strict permits, an operation no permit allows), since these
    /// routes have no tool to be offered.
    pub fn refuse_op(
        &self,
        cid: u64,
        role: &str,
        operation: &str,
        resource: Entity,
        attrs: BTreeMap<String, Value>,
        input: &Json,
    ) -> Option<String> {
        self.refuse_op_with(
            cid,
            role,
            operation,
            resource,
            attrs,
            input,
            BTreeMap::new(),
        )
    }

    /// As `refuse_op`, with extra request context fields (`training.submit` reads `gpu_hours`, S-006c/d).
    #[allow(clippy::too_many_arguments)]
    pub fn refuse_op_with(
        &self,
        cid: u64,
        role: &str,
        operation: &str,
        resource: Entity,
        attrs: BTreeMap<String, Value>,
        input: &Json,
        extra: BTreeMap<String, Value>,
    ) -> Option<String> {
        let flags = detect_flags("", input);
        let agent = Entity {
            ty: "Agent".into(),
            id: cid.to_string(),
        };
        let mut store = EntityStore::default();
        store.insert(&agent, member_attrs_of(&cid.to_string(), role));
        store.insert(&resource, attrs);
        let mut fields = BTreeMap::from([(
            "flags".to_string(),
            Value::set(flags.iter().map(|flag| Value::str(flag))),
        )]);
        fields.extend(extra);
        let context = Value::record(fields);
        let request = Request {
            principal: agent,
            operation: operation.into(),
            resource,
            context,
        };
        let live = self.live();
        let decision = authorize_except(&live.program, &store, &request, &live.off).decision;
        let by = match &decision {
            Decision::Deny { by, .. } => by.clone(),
            _ => Vec::new(),
        };
        if by.is_empty() {
            let strict = self.strict.load(Ordering::Relaxed);
            if matches!(decision, Decision::Deny { .. }) && strict && live.has_permit(operation) {
                return Some(format!(
                    "refused: no law permits {operation} for the {role} role"
                ));
            }
            return None;
        }
        let titles: Vec<&str> = by
            .iter()
            .filter_map(|id| live.titles.get(id))
            .map(String::as_str)
            .collect();
        Some(format!(
            "refused by law {}: {}",
            by.join(", "),
            titles.join("; ")
        ))
    }

    /// Every law, in file then source order, built-in and owner, enabled or not, for the Rules view.
    pub fn catalog(&self) -> Vec<LawInfo> {
        self.live().catalog.clone()
    }

    /// Whether strict permits are on: an operation with permit laws is refused unless one allows it.
    pub fn strict(&self) -> bool {
        self.strict.load(Ordering::Relaxed)
    }

    /// Turns strict permits on or off (`settings.laws.strict_permits`). Memoized answers are dropped.
    pub fn set_strict(&self, on: bool) {
        let mut memo = lock(&self.permitted);
        self.strict.store(on, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::AcqRel);
        memo.clear();
    }

    /// The laws' generation now: it moves with every change to what the laws permit.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The owner's rules and overrides.
    pub fn rules(&self) -> RuleSet {
        lock(&self.rules).clone()
    }

    /// Adds a rule set and applies it when its source compiles (and it is enabled).
    pub fn add_rule(
        &self,
        title: &str,
        source: &str,
        enabled: bool,
        now_ms: u64,
    ) -> Result<OwnerRule, Failure> {
        let mut slot = lock(&self.rules);
        let mut next = slot.clone();
        let rule = OwnerRule {
            id: format!("R-{:03}", next_number(&next)),
            title: title_of(title, source),
            source: source.to_string(),
            enabled,
            created_ms: now_ms,
            updated_ms: now_ms,
            author: OWNER.to_string(),
        };
        Self::check_source_size(&rule.id, source)?;
        next.rules.push(rule.clone());
        let compiled = compile(&self.base, &next)?;
        self.commit(&mut slot, next, compiled);
        Ok(rule)
    }

    /// Refuses a rule source over 64 KiB before it is parsed, as a compile error of that rule.
    fn check_source_size(id: &str, source: &str) -> Result<(), Failure> {
        if source.len() > 64 * 1024 {
            return Err(Failure::Diagnostics(vec![Diagnostic {
                file: rule_file(id),
                line: 1,
                column: 1,
                message: "a rule's source is limited to 64 KiB".to_string(),
            }]));
        }
        Ok(())
    }

    /// Changes a rule's title (when `title` is not blank) and/or its source, and applies the result.
    pub fn update_rule(
        &self,
        id: &str,
        title: &str,
        source: Option<&str>,
        now_ms: u64,
    ) -> Result<OwnerRule, Failure> {
        if let Some(source) = source {
            Self::check_source_size(id, source)?;
        }
        self.edit_rule(id, now_ms, |rule| {
            if !title.trim().is_empty() {
                rule.title = title.trim().to_string();
            }
            if let Some(source) = source {
                rule.source = source.to_string();
            }
        })
    }

    /// Enables or disables a rule set: its laws apply, or stop applying, from now on.
    pub fn set_rule_enabled(
        &self,
        id: &str,
        enabled: bool,
        now_ms: u64,
    ) -> Result<OwnerRule, Failure> {
        self.edit_rule(id, now_ms, |rule| rule.enabled = enabled)
    }

    /// Deletes a rule set and its laws.
    pub fn delete_rule(&self, id: &str) -> Result<(), Failure> {
        let mut slot = lock(&self.rules);
        if !slot.rules.iter().any(|rule| rule.id == id) {
            return Err(Failure::NotFound(format!("rule {id}")));
        }
        let mut next = slot.clone();
        next.rules.retain(|rule| rule.id != id);
        let compiled = compile(&self.base, &next)?;
        self.commit(&mut slot, next, compiled);
        Ok(())
    }

    /// Disables a built-in statute (with the owner's reason, recorded) or enables it again.
    ///
    /// A constitutional law is switched off only when `confirm` is its exact id; otherwise the
    /// change is refused with [`Failure::Unconfirmed`]. Switching it on needs no confirmation. Every
    /// change of state of a constitutional law is kept in the history, with who made it and when.
    pub fn set_override(
        &self,
        law: &str,
        disabled: bool,
        reason: &str,
        confirm: &str,
        now_ms: u64,
    ) -> Result<Option<Override>, Failure> {
        let mut slot = lock(&self.rules);
        let info = match self.live().catalog.iter().find(|info| info.id == law) {
            None => return Err(Failure::NotFound(format!("law {law}"))),
            Some(info) if info.origin != "built-in" => {
                return Err(Failure::Invalid(format!(
                    "{law} is an owner rule; disable its rule instead"
                )));
            }
            Some(info) => info.clone(),
        };
        let constitutional = info.tier == CONSTITUTION;
        if disabled && constitutional && confirm != law {
            let asked =
                format!("{law} is a constitutional law; to switch it off, send confirm: \"{law}\"");
            let why = if confirm.is_empty() {
                asked
            } else {
                format!("confirm \"{confirm}\" does not match. {asked}")
            };
            return Err(Failure::Unconfirmed(why));
        }
        if disabled && reason.trim().is_empty() {
            return Err(Failure::Invalid("say why the statute is disabled".into()));
        }
        let mut next = slot.clone();
        next.overrides.retain(|o| o.law != law);
        let made = disabled.then(|| Override {
            law: law.to_string(),
            disabled_by: OWNER.to_string(),
            reason: reason.trim().to_string(),
            at_ms: now_ms,
        });
        next.overrides.extend(made.clone());
        let compiled = compile(&self.base, &next)?;
        // A change of state: enabled and now disabled, or disabled and now enabled.
        if constitutional && info.enabled == disabled {
            next.history.push(LawChange {
                law: law.to_string(),
                title: info.title.clone(),
                tier: info.tier.clone(),
                disabled,
                by: OWNER.to_string(),
                reason: reason.trim().to_string(),
                at_ms: now_ms,
                confirmed: disabled,
            });
            let excess = next.history.len().saturating_sub(HISTORY_KEPT);
            next.history.drain(..excess);
        }
        self.commit(&mut slot, next, compiled);
        Ok(made)
    }

    /// The constitutional laws that are switched off now, by id, in catalog order.
    pub fn constitutional_off(&self) -> Vec<String> {
        constitutional_off(&self.live().catalog)
    }

    /// Compiles `source` as an enabled rule set on top of the owner's rules, without applying it, and
    /// replays the recent calls its laws would refuse. `replaces` names the rule being edited: its
    /// source is what is compiled in its place, so its own laws are not declared twice.
    pub fn dry_run(&self, source: &str, replaces: Option<&str>) -> Result<DryRun, Failure> {
        Self::check_source_size(replaces.unwrap_or(DRY_RUN_ID), source)?;
        let mut next = lock(&self.rules).clone();
        let id = match next
            .rules
            .iter_mut()
            .find(|rule| Some(rule.id.as_str()) == replaces)
        {
            Some(rule) => {
                rule.source = source.to_string();
                rule.enabled = true;
                rule.id.clone()
            }
            None => {
                next.rules.retain(|rule| rule.id != DRY_RUN_ID);
                next.rules.push(OwnerRule {
                    id: DRY_RUN_ID.to_string(),
                    title: title_of("", source),
                    source: source.to_string(),
                    enabled: true,
                    created_ms: 0,
                    updated_ms: 0,
                    author: OWNER.to_string(),
                });
                DRY_RUN_ID.to_string()
            }
        };
        let compiled = compile(&self.base, &next)?;
        let mut laws: Vec<String> = compiled
            .catalog
            .iter()
            .filter(|info| info.rule_id == id)
            .map(|info| info.id.clone())
            .collect();
        laws.sort();
        let calls: Vec<Seen> = lock(&self.seen).iter().cloned().collect();
        let mut would_refuse = Vec::new();
        for call in calls.iter().rev() {
            let verdict = self.check_with(
                &compiled,
                &call.cid.to_string(),
                &call.role,
                &call.tool,
                &call.input,
            );
            let by: Vec<&String> = verdict
                .forbid
                .iter()
                .filter(|id| laws.contains(*id))
                .collect();
            if !by.is_empty() {
                would_refuse.push(json!({ "cid": call.cid, "role": call.role, "tool": call.tool, "at_ms": call.at_ms, "laws": by }));
                if would_refuse.len() == SAMPLES {
                    break;
                }
            }
        }
        Ok(DryRun { laws, would_refuse })
    }

    /// The owner's rules and overrides as rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        apz_store::RowSet::one(def(), &*lock(&self.rules))
    }

    /// Replaces the owner's rules with what plaza.db holds (at start), and applies them when they compile. Stored
    /// rules that no longer compile are kept as they are and not applied: only the built-in laws apply until the
    /// owner fixes or removes them (nothing is lost meanwhile).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: RuleSet = apz_store::table::load_one(conn, def())?.unwrap_or_default();
        let mut slot = lock(&self.rules);
        match compile(&self.base, &stored) {
            Ok(compiled) => self.swap(compiled),
            Err(failure) => {
                eprintln!(
                    "laws: the owner's rules are not applied: {}",
                    failure.message()
                );
                *lock(&self.not_applied) = Some(failure.message());
            }
        }
        *slot = stored;
        Ok(())
    }

    /// Why the owner's stored rules are not applied (they did not compile at start), until a change applies them.
    pub fn not_applied(&self) -> Option<String> {
        lock(&self.not_applied).clone()
    }

    /// Applies a change to one rule: the change is compiled first, and nothing is applied unless it compiles.
    fn edit_rule(
        &self,
        id: &str,
        now_ms: u64,
        change: impl FnOnce(&mut OwnerRule),
    ) -> Result<OwnerRule, Failure> {
        let mut slot = lock(&self.rules);
        let mut next = slot.clone();
        let rule = next
            .rules
            .iter_mut()
            .find(|rule| rule.id == id)
            .ok_or_else(|| Failure::NotFound(format!("rule {id}")))?;
        change(rule);
        rule.updated_ms = now_ms;
        let edited = rule.clone();
        let compiled = compile(&self.base, &next)?;
        self.commit(&mut slot, next, compiled);
        Ok(edited)
    }

    /// Stores `next` as the owner's rules and swaps in its compiled set. The saver writes it (see [`Laws::rows`]).
    fn commit(&self, slot: &mut MutexGuard<'_, RuleSet>, next: RuleSet, compiled: Compiled) {
        **slot = next;
        self.swap(compiled);
    }

    /// Swaps in a compiled set. The memo lock is held so that no answer computed under the old set
    /// is remembered after the swap.
    fn swap(&self, next: Compiled) {
        *lock(&self.not_applied) = None;
        let mut memo = lock(&self.permitted);
        *self.live.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        self.generation.fetch_add(1, Ordering::AcqRel);
        memo.clear();
    }

    fn live(&self) -> Arc<Compiled> {
        self.live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Keeps the call for the dry run. A call whose input is too large to replay is not kept.
    fn remember(&self, cid: u64, role: &str, tool: &str, input: &Json) {
        if input.to_string().len() > INPUT_KEPT {
            return;
        }
        let mut seen = lock(&self.seen);
        if seen.len() == CALLS_KEPT {
            seen.pop_front();
        }
        seen.push_back(Seen {
            cid,
            role: role.to_string(),
            tool: tool.to_string(),
            input: input.clone(),
            at_ms: apz_plaza::now_ms(),
        });
    }
}

/// Locks a mutex, ignoring poisoning: the guarded data is always left whole by its writers.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The name a rule's source is compiled under, so its diagnostics and its laws can be traced to it.
fn rule_file(id: &str) -> String {
    format!("owner/{id}.jhrl")
}

/// The next free id of an owner rule (`R-001`, `R-002`, …).
fn next_number(set: &RuleSet) -> u32 {
    set.rules
        .iter()
        .filter_map(|rule| rule.id.strip_prefix("R-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

/// A rule's title when none was given: the first law's title in its source.
fn title_of(title: &str, source: &str) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    let mut titles = BTreeMap::new();
    titles_of(source, &mut titles);
    titles
        .into_values()
        .next()
        .unwrap_or_else(|| "Untitled rule".to_string())
}

/// Compiles the built-in sources with the owner's rules as one program. Every error is returned with
/// its place; a rule set with any error is refused whole.
fn compile(base: &[(String, String)], state: &RuleSet) -> Result<Compiled, Failure> {
    let mut files = base.to_vec();
    files.extend(
        state
            .rules
            .iter()
            .map(|rule| (rule_file(&rule.id), rule.source.clone())),
    );
    let (program, diags) = Program::load(files.clone());
    let mut errors: Vec<Diagnostic> = diags
        .iter()
        .filter(|d| !d.warning)
        .map(|d| diagnostic(&program.sources, d))
        .collect();
    errors.extend(
        check_authz(&program)
            .iter()
            .filter(|d| !d.warning)
            .map(|d| diagnostic(&program.sources, d)),
    );
    for rule in &state.rules {
        let file = rule_file(&rule.id);
        let declares = program
            .modules
            .iter()
            .filter(|module| program.sources.files[module.file as usize].path == file)
            .any(|module| module.items.iter().any(|item| matches!(item, Item::Law(_))));
        if !declares && !errors.iter().any(|error| error.file == file) {
            errors.push(Diagnostic {
                file,
                line: 1,
                column: 1,
                message: "the source declares no law".to_string(),
            });
        }
    }
    if !errors.is_empty() {
        return Err(Failure::Diagnostics(errors));
    }
    let mut titles = BTreeMap::new();
    for (_, text) in &files {
        titles_of(text, &mut titles);
    }
    let mut catalog = Vec::new();
    let mut off = HashSet::new();
    for module in &program.modules {
        let file = program.sources.files[module.file as usize].path.clone();
        let rule = state.rules.iter().find(|rule| rule_file(&rule.id) == file);
        for item in &module.items {
            let Item::Law(law) = item else { continue };
            let overridden = state.overrides.iter().find(|o| o.law == law.id).cloned();
            let enabled = rule.is_none_or(|rule| rule.enabled) && overridden.is_none();
            if !enabled {
                off.insert(law.id.clone());
            }
            let mut info = law_info(law, &file, &program.sources);
            info.origin = if rule.is_some() { "owner" } else { "built-in" }.to_string();
            info.enabled = enabled;
            info.rule_id = rule.map(|rule| rule.id.clone()).unwrap_or_default();
            info.overridden = overridden;
            catalog.push(info);
        }
    }
    let permit_ops = catalog
        .iter()
        .filter(|info| info.enabled && info.effect == "permit")
        .flat_map(|info| info.ops.iter().cloned())
        .collect();
    Ok(Compiled {
        program,
        titles,
        off,
        catalog,
        permit_ops,
    })
}

/// A diagnostic with the place it is in, when it has one.
fn diagnostic(sources: &SourceMap, diag: &Diag) -> Diagnostic {
    match diag.span {
        Some(span) => {
            let source = &sources.files[span.file as usize];
            let (line, column) = source.line_col(span.lo as usize);
            Diagnostic {
                file: source.path.clone(),
                line,
                column,
                message: diag.msg.clone(),
            }
        }
        None => Diagnostic {
            file: String::new(),
            line: 0,
            column: 0,
            message: diag.msg.clone(),
        },
    }
}

/// The owner's rules' tables: `rules`, with each rule, override and history change as a child table.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(RuleSet)
}

/// The plaza's one law set (the state folder's rules are loaded into it at start, see [`Laws::load`]). Only
/// [`Laws::permits`] is answered from the built-in set without an `App`, and that keeps nothing.
pub fn global() -> &'static Laws {
    static LAWS: OnceLock<Laws> = OnceLock::new();
    LAWS.get_or_init(Laws::builtin)
}

/// The attributes of an agent as the laws read them: [`member_attrs`] plus its kind, `model` for a
/// CID in the model range (`foundry::registry`) and `llm` for every other agent.
fn member_attrs_of(cid: &str, role: &str) -> BTreeMap<String, Value> {
    let mut attrs = member_attrs(role);
    let model = cid
        .parse::<u64>()
        .is_ok_and(crate::foundry::registry::is_model_cid);
    attrs.insert(
        "kind".to_string(),
        Value::str(if model { "model" } else { "llm" }),
    );
    attrs
}

/// The attributes of an agent with `role` as the laws read them: its role and tier.
fn member_attrs(role: &str) -> BTreeMap<String, Value> {
    let tier = match tier_of(role) {
        Tier::Orchestrator => "orchestrator",
        Tier::Worker => "worker",
    };
    BTreeMap::from([
        ("role".to_string(), Value::str(role)),
        ("tier".to_string(), Value::str(tier)),
    ])
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
    if apz_secrets::has_secret(&text) || reads_credential_store(&lower) {
        flags.insert("credential_seeking");
    }
    if lower.contains("impersonat")
        || lower.contains("pretend to be the owner")
        || lower.contains("pretend to be the operator")
    {
        flags.insert("impersonation");
    }
    if matches!(tool, "run" | "run_tests" | "run_script") && longest_base64_run(&text) > 400 {
        flags.insert("obfuscation");
    }
    flags
}

/// Whether a command reaches for a credential store.
fn reads_credential_store(lower: &str) -> bool {
    [
        "/.ssh/",
        "id_rsa",
        "id_ed25519",
        "/etc/shadow",
        ".aws/credentials",
        ".netrc",
        ".git-credentials",
        ".docker/config.json",
    ]
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
    "run",
    "run_tests",
    "run_script",
    "edit",
    "download",
    "fetch_url",
    "install_packages",
    "code_engine",
    "tool_package",
    "deliver_file",
    "checkpoint_restore",
    // An ACP engine's own edits, commands and fetches act in the VM as `code_engine` does;
    // `engine.read` only looks, so it is checked as a tool call alone.
    "engine.edit",
    "engine.exec",
    "engine.fetch",
    "engine.other",
];

/// The tools that send a message to a recipient (or the board): their laws read the recipient from the input.
const MESSAGE_TOOLS: &[&str] = &[
    "plaza_send_message",
    "plaza_message_seat",
    "plaza_board_post",
    "plaza_board_comment",
];

/// The operation an artifact tool performs, if it is one (see `Laws::check`).
fn artifact_operation(tool: &str) -> Option<&'static str> {
    match tool {
        "share_file" | "deliver_file" => Some("artifact.publish"),
        "artifact_create" | "artifact_update" | "artifact_attach_data" => Some("artifact.create"),
        "artifact_share_request" => Some("artifact.share"),
        _ => None,
    }
}

/// The operations a call of `tool` is checked as, `tool.call` first, as `Laws::check` requests them.
fn operations_of(tool: &str) -> Vec<&'static str> {
    let mut ops = vec!["tool.call"];
    if GUEST_EXEC.contains(&tool) || tool.starts_with(crate::library::PREFIX) {
        ops.push("tool.exec");
    }
    if MESSAGE_TOOLS.contains(&tool) {
        ops.push("message.send");
    }
    ops.extend(artifact_operation(tool));
    ops
}

/// Whether a law's conditions or scopes read the input of a call, or it expires. A tool's call carries its input's
/// flags in the context (see [`reads_input`]); a law that expires changes with the time, not with a change the hub
/// would push, so it counts too.
fn law_reads_input(law: &Law) -> bool {
    law.expires.is_some()
        || law.when.iter().chain(&law.unless).any(reads_input)
        || [&law.principal, &law.resource]
            .into_iter()
            .flatten()
            .any(|scope| match scope {
                Scope::Is(_) => false,
                Scope::Eq(expr) | Scope::In(expr) => reads_input(expr),
            })
}

/// Whether an expression reads a call's input through its context: the flags the input's text sets, or the whole
/// context. Any other field of the context is absent from a tool's call, so it reads the same for every input.
fn reads_input(expr: &Expr) -> bool {
    let any = |exprs: &[Expr]| exprs.iter().any(reads_input);
    match &expr.kind {
        ExprKind::Name(name) | ExprKind::Primed(name) => name == "context",
        ExprKind::Num(..) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::NoneLit => false,
        ExprKind::Field(base, field) => match &base.kind {
            ExprKind::Name(name) if name == "context" => field == "flags",
            _ => reads_input(base),
        },
        ExprKind::Unary(_, inner) | ExprKind::Is(inner, _) => reads_input(inner),
        ExprKind::Binary(_, left, right) | ExprKind::Index(left, right) => {
            reads_input(left) || reads_input(right)
        }
        ExprKind::If(cond, then, otherwise) => {
            reads_input(cond) || reads_input(then) || reads_input(otherwise)
        }
        ExprKind::Call(callee, args) => {
            reads_input(callee) || args.iter().any(|arg| reads_input(&arg.value))
        }
        ExprKind::List(items) | ExprKind::Set(items) | ExprKind::Tuple(items) => any(items),
        ExprKind::Map(pairs) => pairs
            .iter()
            .any(|(key, value)| reads_input(key) || reads_input(value)),
        ExprKind::Record(fields) => fields.iter().any(|(_, value)| reads_input(value)),
        ExprKind::Comp {
            gens,
            filter,
            key,
            val,
            ..
        } => {
            gens.iter().any(|generator| reads_input(&generator.src))
                || filter.as_deref().is_some_and(reads_input)
                || reads_input(key)
                || val.as_deref().is_some_and(reads_input)
        }
        ExprKind::Quant(_, gens, body) => {
            gens.iter().any(|generator| reads_input(&generator.src)) || reads_input(body)
        }
        ExprKind::With(base, path, value) => {
            reads_input(base)
                || reads_input(value)
                || path
                    .iter()
                    .any(|seg| matches!(seg, PathSeg::Index(index) if reads_input(index)))
        }
    }
}

/// The plaza, as the one `Plaza` entity the laws name for an operation on the plaza as a whole.
const PLAZA: &str = "plaza";

/// The operations a plaza tool performs beyond its `tool.call`, each with the resource and the
/// context its laws read. A forbid whose condition reads a missing attribute applies (fail closed),
/// so every attribute a forbid reads is supplied here. A tool performing several operations
/// requests all of them.
fn plaza_operations(
    tool: &str,
    input: &Json,
    role: &str,
    store: &mut EntityStore,
) -> Vec<(&'static str, Entity, Context)> {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match tool {
        // The plaza is named by its `name` attribute (a bare resource is its `$type`/`$id` record).
        // Posting a plan's whole list creates tasks as posting one does.
        "plaza_post_task" | "plaza_tasks_post" => {
            let plaza = Entity {
                ty: "Plaza".into(),
                id: PLAZA.into(),
            };
            store.insert(
                &plaza,
                BTreeMap::from([("name".to_string(), Value::str(PLAZA))]),
            );
            vec![("task.create", plaza, Context::new())]
        }
        "plaza_assign" => {
            let cid = input
                .get("agent")
                .and_then(Json::as_u64)
                .map(|n| n.to_string())
                .unwrap_or_default();
            let assignee = Entity {
                ty: "Agent".into(),
                id: cid,
            };
            store.insert(&assignee, member_attrs_of(&assignee.id, ""));
            let context =
                Context::from([("assignee".to_string(), jhrl::authz::entity_value(&assignee))]);
            vec![("task.assign", task_resource(store, text("task")), context)]
        }
        "plaza_verdict" => vec![(
            "review.submit",
            task_resource(store, text("task")),
            Context::new(),
        )],
        "submit_claim" => vec![(
            "claim.submit",
            task_resource(store, text("task")),
            Context::new(),
        )],
        // The plan of a project is approved as the task it stands for; its budget is not in the call.
        "plaza_project_approve" => vec![(
            "plan.approve",
            task_resource(store, text("id")),
            Context::new(),
        )],
        // No artifact is marked hidden from a role yet: the hidden set is empty, so S-010d does not apply.
        "plaza_get_file" => {
            let artifact = Entity {
                ty: "Artifact".into(),
                id: text("id"),
            };
            store.insert(
                &artifact,
                BTreeMap::from([(
                    "hidden_from".to_string(),
                    Value::set(std::iter::empty::<Value>()),
                )]),
            );
            vec![("artifact.read", artifact, Context::new())]
        }
        // A training job is checked for its GPU-hours: a breeder's over 8 is forbidden (S-006d).
        "foundry_submit_job" => vec![(
            "training.submit",
            Entity {
                ty: "Plaza".into(),
                id: PLAZA.into(),
            },
            Context::from([(
                "gpu_hours".to_string(),
                Value::int(crate::foundry::breeder::budget_hours(input)),
            )]),
        )],
        // A model runs only where its card allows the caller's role (S-006e).
        "invoke_model" => {
            let id = text("model");
            let model = Entity {
                ty: "Model".into(),
                id: id.clone(),
            };
            store.insert(&model, BTreeMap::new());
            let allowed = crate::foundry::registry::allowed_roles(&id)
                .iter()
                .any(|allowed| allowed == role);
            vec![(
                "model.invoke",
                model,
                Context::from([("role_allowed".to_string(), Value::Bool(allowed))]),
            )]
        }
        // An MCP call is `mcp.call` on its server, with the tool and the risk class the owner's rules may limit.
        t if t.starts_with(crate::mcp::PREFIX) => match crate::mcp::law_operation(t) {
            Some((server, tool, risk)) => {
                let resource = Entity {
                    ty: "McpServer".into(),
                    id: server.clone(),
                };
                store.insert(&resource, BTreeMap::new());
                let context = Context::from([
                    ("server".to_string(), Value::str(&server)),
                    ("tool".to_string(), Value::str(&tool)),
                    ("risk".to_string(), Value::str(risk)),
                ]);
                vec![("mcp.call", resource, context)]
            }
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The task a call names, as an entity of the plaza.
fn task_resource(store: &mut EntityStore, id: String) -> Entity {
    let task = Entity {
        ty: "Task".into(),
        id,
    };
    store.insert(&task, BTreeMap::new());
    task
}

/// Where a tool comes from, as the tool laws (`tools.jhrl`) read it: a connector's action, a
/// library tool another agent built, a host tool apid runs, or a guest tool in the agent's VM.
fn tool_kind(tool: &str) -> &'static str {
    if crate::connectors::by_tool(tool).is_some() {
        "connector"
    } else if tool.starts_with(crate::mcp::PREFIX) {
        "mcp"
    } else if tool.starts_with(crate::library::PREFIX) {
        "library"
    } else if runs(tool) == Some(Runs::Host) {
        "host"
    } else {
        "guest"
    }
}

/// The catalog's risk class for a tool; the constitution's irreversible-action law (C-004a) reads
/// it. Connector actions carry their own class (`connectors::risk_of`: e.g. a DNS record or a droplet
/// created is `irreversible`), and C-004a refuses them, since owner approvals are not threaded through
/// tool calls yet.
pub(crate) fn risk_class(tool: &str) -> &'static str {
    match tool {
        "read" | "find" | "list_files" | "search" | "symbols" | "definition" | "references"
        | "diagnostics" | "outline" | "plaza_tasks" | "plaza_agents" | "transcript_search"
        | "todo_write" | "checkpoint_diff" | "checkpoint_list" | "foundry_hf_search"
        | "station_list" | "station_app_list" | "engine.read" => "read",
        t if t.starts_with("plaza_") && t.ends_with("_read") => "read",
        t if crate::connectors::risk_of(t).is_some() => {
            crate::connectors::risk_of(t).unwrap_or("write")
        }
        t if crate::mcp::risk_of(t).is_some() => crate::mcp::risk_of(t).unwrap_or("write"),
        _ => "write",
    }
}

/// One law as the Rules view shows it: its fields, its source text and a generated sentence.
fn law_info(law: &Law, file: &str, sources: &SourceMap) -> LawInfo {
    let principal = law
        .principal
        .as_ref()
        .map(|s| scope_text("principal", s, sources))
        .unwrap_or_default();
    let resource = law
        .resource
        .as_ref()
        .map(|s| scope_text("resource", s, sources))
        .unwrap_or_default();
    let conditions: Vec<String> = law
        .when
        .iter()
        .map(|e| sources.snippet(e.span).trim().to_string())
        .collect();
    let tier = law
        .tier
        .clone()
        .unwrap_or_else(|| file_tier(file).to_string());
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
        origin: String::new(),
        enabled: true,
        rule_id: String::new(),
        overridden: None,
    }
}

/// The ids of the constitutional laws that are switched off, in catalog order.
pub fn constitutional_off(catalog: &[LawInfo]) -> Vec<String> {
    catalog
        .iter()
        .filter(|info| info.tier == CONSTITUTION && !info.enabled)
        .map(|info| info.id.clone())
        .collect()
}

/// The tier a law gets from its file when it names none.
fn file_tier(file: &str) -> &'static str {
    match file {
        "constitution.jhrl" => "constitution",
        "statutes.jhrl" => "statute",
        "operations.jhrl" => "operations",
        file if file.starts_with("owner/") => "owner",
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
        law.ops
            .iter()
            .map(|op| verb_of(op))
            .collect::<Vec<_>>()
            .join(" or ")
    };
    let own = conditions
        .iter()
        .any(|c| c.contains("resource.agent == principal"));
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
    let tail = if conditions.is_empty() {
        ""
    } else {
        " under the conditions below"
    };
    match law.effect {
        Effect::Permit => format!("{} may {verb}{object}{tail}.", capitalized(&who)),
        Effect::Forbid if who == "any agent" => format!("No agent may {verb}{object}{tail}."),
        Effect::Forbid => format!("{} may not {verb}{object}{tail}.", capitalized(&who)),
    }
}

/// Who the law applies to, as a noun phrase: a role when one is named, else the principal's type.
fn who_of(law: &Law, principal: &str, conditions: &[String]) -> String {
    let role = std::iter::once(principal)
        .chain(conditions.iter().map(String::as_str))
        .find_map(role_named);
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
    let start = ["principal.role == \"", "Role::\""]
        .iter()
        .find_map(|marker| text.find(marker).map(|i| i + marker.len()))?;
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
        "artifact.create" => "create".to_string(),
        "artifact.share" => "share".to_string(),
        "device.actuate" => "operate".to_string(),
        "device.read" => "read".to_string(),
        "audit.modify" => "change the audit log".to_string(),
        "audit.read" => "read the audit log".to_string(),
        "gateway.configure" => "reconfigure the gateway".to_string(),
        "policy.modify" => "change the laws".to_string(),
        "vault.list" => "list".to_string(),
        "sandbox.event" => "report sandbox events for".to_string(),
        "approval.request" => "request".to_string(),
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
        "audit_sample.get" => "draw audit samples from".to_string(),
        "any" => "act on".to_string(),
        other => match other.split_once('.') {
            // `thing.verb` reads as "verb the thing".
            Some((thing, verb)) => {
                format!("{} the {}", verb.replace('_', " "), thing.replace('_', " "))
            }
            None => other.replace('_', " "),
        },
    }
}

fn with_article(noun: &str) -> String {
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
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

    #[test]
    fn the_catalog_explains_each_law() {
        let laws = Laws::builtin();
        let catalog = laws.catalog();
        assert!(!catalog.is_empty());
        assert!(
            catalog
                .iter()
                .all(|l| !l.id.is_empty() && !l.title.is_empty() && !l.effect.is_empty())
        );
        assert!(catalog.iter().any(|l| l.effect == "permit"));
        assert!(catalog.iter().any(|l| l.effect == "forbid"));
        let s000a = catalog
            .iter()
            .find(|l| l.id == "S-000a")
            .expect("S-000a is in the catalog");
        assert!(
            s000a.source.starts_with("law \"S-000a\""),
            "{}",
            s000a.source
        );
        assert_eq!(s000a.tier, "statute");
        assert!(
            s000a.explanation.contains("sandbox"),
            "{}",
            s000a.explanation
        );
    }

    #[test]
    fn a_plain_read_and_an_acp_engines_actions_are_checked() {
        // a_plain_read_is_not_forbidden: a plain read is not forbidden.
        {
            let laws = Laws::builtin();
            let verdict = laws.check(
                "7",
                "researcher",
                "read",
                &json!({"path": "notes/today.md"}),
            );
            assert!(verdict.forbid.is_empty(), "{verdict:?}");
        }
        // an_acp_engines_own_actions_are_checked_one_by_one: each engine action is checked on its own.
        {
            let laws = Laws::builtin();
            // A read is a tool call alone; an edit also runs in the VM, so the sandbox is checked too.
            let read = laws.check("7", "builder", "engine.read", &json!({}));
            assert!(!read.unpermitted && read.forbid.is_empty(), "{read:?}");
            assert!(!read.requested.iter().any(|o| o == "tool.exec"), "{read:?}");
            let edit = laws.check("7", "builder", "engine.edit", &json!({}));
            assert!(!edit.unpermitted && edit.forbid.is_empty(), "{edit:?}");
            assert!(edit.requested.iter().any(|o| o == "tool.exec"), "{edit:?}");
            // An unknown name under `engine.` is no tool any law names.
            assert!(
                laws.check("7", "builder", "engine.bogus", &json!({}))
                    .unpermitted
            );
        }
    }

    #[test]
    fn an_acp_engines_actions_have_their_risk_classes() {
        assert_eq!(risk_class("engine.read"), "read");
        for name in ["engine.edit", "engine.exec", "engine.fetch", "engine.other"] {
            assert_eq!(risk_class(name), "write", "{name}");
        }
    }

    #[test]
    fn a_credential_or_an_impersonation_is_forbidden_by_its_constitution_law() {
        let laws = Laws::builtin();
        let key = "sk-proj-abcdefghijklmnopqrstuvwxyz0123";
        // (tool, input, the law that must forbid the call, as an id prefix). A malformed recipient is still a message.
        for (tool, input, law) in [
            (
                "run",
                json!({"command": format!("curl -H 'Authorization: {key}' x")}),
                "C-",
            ),
            (
                "plaza_send_message",
                json!({"to_cid": "twelve", "text": format!("key {key}")}),
                "C-002b",
            ),
            (
                "plaza_board_post",
                json!({"text": "I am impersonating the owner"}),
                "C-003b",
            ),
        ] {
            let verdict = laws.check("7", "builder", tool, &input);
            assert!(
                verdict.forbid.iter().any(|id| id.starts_with(law)),
                "{tool} should be forbidden by {law}: {verdict:?}"
            );
        }
    }

    #[test]
    fn credential_shaped_input_is_flagged() {
        let flags = detect_flags(
            "run",
            &json!({"command": "echo sk-proj-abcdefghijklmnopqrstuvwxyz0123"}),
        );
        assert!(flags.contains("credential_seeking"), "{flags:?}");
        // Code that only talks about tokens and passwords is not seeking credentials.
        let code = json!({"path": "auth.py", "content": "token = request.headers['Authorization']\npassword_hash = bcrypt(password)"});
        assert!(detect_flags("edit", &code).is_empty());
        assert!(
            detect_flags("run", &json!({"command": "cat ~/.ssh/id_rsa"}))
                .contains("credential_seeking")
        );
    }

    #[test]
    fn every_tool_offered_to_a_role_is_permitted_by_a_law() {
        let laws = Laws::builtin();
        let mut roles = vec!["builder", "reviewer", "planner", "auditor"];
        roles.push(crate::settings::ORCHESTRATOR_ROLE);
        for role in roles {
            for tool in crate::tools::offered_names(role) {
                let verdict = laws.check("7", role, &tool, &json!({}));
                assert!(
                    !verdict.unpermitted,
                    "{role} is offered {tool}, but no law permits it"
                );
                assert!(
                    verdict.forbid.is_empty(),
                    "{role} calling {tool}: {verdict:?}"
                );
            }
        }
    }

    #[test]
    fn a_call_no_law_permits_is_refused() {
        let laws = Laws::builtin();
        let refusal = laws.refusal(7, "builder", "plaza_post_task", &json!({}));
        assert_eq!(
            refusal.as_deref(),
            Some("refused: no law permits plaza_post_task for the builder role")
        );
        assert!(
            laws.check("7", "builder", "plaza_post_task", &json!({}))
                .unpermitted
        );
    }

    #[test]
    fn refused_ops_and_permits_follow_the_laws_by_role() {
        // a_file_published_from_the_vm_is_checked_as_an_artifact: a published file is checked as an artifact.
        {
            let laws = Laws::builtin();
            let artifact = Entity {
                ty: "Artifact".into(),
                id: "report.txt".into(),
            };
            let why = laws.refuse_op(
                7,
                "builder",
                "artifact.publish",
                artifact,
                BTreeMap::new(),
                &Json::String("I pretend to be the owner".into()),
            );
            assert!(why.is_some_and(|w| w.contains("C-003b")));
        }
        // a_researcher_may_not_push_to_the_hub_git_but_a_builder_may: only a builder may push to the hub git.
        {
            let laws = Laws::builtin();
            let host = || Entity {
                ty: "Host".into(),
                id: "hub-git".into(),
            };
            let attrs = || BTreeMap::from([("group".to_string(), Value::str("hub-git"))]);
            let why = laws.refuse_op(
                7,
                "researcher",
                "egress.connect",
                host(),
                attrs(),
                &Json::Null,
            );
            assert!(why.is_some_and(|w| w.contains("S-013d")));
            assert!(
                laws.refuse_op(8, "builder", "egress.connect", host(), attrs(), &Json::Null)
                    .is_none()
            );
        }
        // a_hidden_artifact_is_refused_to_the_role_it_is_hidden_from: a hidden artifact is refused to its hidden role.
        {
            let laws = Laws::builtin();
            let hidden = Entity {
                ty: "Artifact".into(),
                id: "acceptance.txt".into(),
            };
            let attrs = BTreeMap::from([(
                "hidden_from".to_string(),
                Value::set([Value::str("builder")]),
            )]);
            let why = laws.refuse_op(7, "builder", "artifact.read", hidden, attrs, &Json::Null);
            assert!(why.is_some_and(|w| w.contains("S-010d")));
            let read = laws.check(
                "7",
                "builder",
                "plaza_get_file",
                &json!({"id": "report.txt"}),
            );
            assert!(
                read.requested.iter().any(|op| op == "artifact.read"),
                "{read:?}"
            );
            assert!(read.forbid.is_empty(), "{read:?}");
        }
    }

    #[test]
    fn permits_follows_the_tier_and_connector_laws() {
        // an_irreversible_connector_action_is_withheld_from_every_role: no role is offered an irreversible action.
        {
            let laws = Laws::builtin();
            assert_eq!(
                crate::connectors::risk_of("cloudflare_dns_create"),
                Some("irreversible")
            );
            // C-004a: refused without an owner approval, so no role is offered it (tools.rs specs_for).
            for role in ["builder", "orchestrator"] {
                assert!(!laws.permits(role, "cloudflare_dns_create"), "{role}");
            }
        }
        // permits_follows_the_tier_laws: permits follow the tier laws.
        {
            let laws = Laws::builtin();
            assert!(laws.permits(crate::settings::ORCHESTRATOR_ROLE, "plaza_project_approve"));
            assert!(!laws.permits("builder", "plaza_project_approve"));
            assert!(!laws.permits("builder", "plaza_post_task"));
            assert!(laws.permits(crate::settings::ORCHESTRATOR_ROLE, "plaza_post_task"));
            assert!(laws.permits("builder", "run"));
            // A second ask is answered from the memo, the same way.
            assert!(laws.permits(crate::settings::ORCHESTRATOR_ROLE, "plaza_project_approve"));
        }
    }

    #[test]
    fn a_tool_is_checked_as_the_operation_it_performs() {
        // (role, tool, input, operation the laws see, permit that allows it, must be permitted outright)
        let cases: [(&str, &str, Json, &str, Option<&str>, bool); 6] = [
            (
                crate::settings::ORCHESTRATOR_ROLE,
                "plaza_post_task",
                json!({"title": "t", "body": "b"}),
                "task.create",
                None,
                true,
            ),
            (
                crate::settings::ORCHESTRATOR_ROLE,
                "plaza_tasks_post",
                json!({"project": "pr1", "tasks": [{"key": "1.1", "goal": "g"}]}),
                "task.create",
                Some("S-003c"),
                true,
            ),
            (
                "reviewer",
                "plaza_verdict",
                json!({"task": "t-1", "pass": true, "notes": "ok"}),
                "review.submit",
                Some("S-002c"),
                false,
            ),
            (
                crate::settings::ORCHESTRATOR_ROLE,
                "plaza_project_approve",
                json!({"id": "p-1"}),
                "plan.approve",
                None,
                true,
            ),
            (
                "builder",
                "submit_claim",
                json!({"task": "t-1", "summary": "done"}),
                "claim.submit",
                Some("S-001e"),
                false,
            ),
            (
                crate::settings::ORCHESTRATOR_ROLE,
                "plaza_assign",
                json!({"task": "t-1", "agent": 9}),
                "task.assign",
                Some("S-004a"),
                true,
            ),
        ];
        for (role, tool, input, op, permit, allowed) in cases {
            let laws = Laws::builtin();
            let verdict = laws.check("7", role, tool, &input);
            assert!(
                verdict.requested.iter().any(|o| o == op),
                "{tool}: {verdict:?}"
            );
            assert!(verdict.forbid.is_empty(), "{tool}: {verdict:?}");
            if allowed {
                assert!(!verdict.unpermitted, "{tool}: {verdict:?}");
            }
            if let Some(id) = permit {
                assert!(
                    verdict.permit.iter().any(|p| p == id),
                    "{tool}: {verdict:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod owner_rules_tests {
    use super::*;
    use serde_json::json;

    /// A rule an owner might add: no reading files, for any agent.
    const NO_READS: &str = "law \"O-001\" \"No reading files\" {
  tier statute
  enforcement preventive
  severity info
  statement \"Test rule: no agent may read a file.\"
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == \"read\"
}
";

    /// A permit an owner might add: only a sensor may read a device.
    const SENSORS_ONLY: &str = "law \"O-020\" \"Only sensors read devices\" {
  tier statute
  enforcement preventive
  severity info
  statement \"Test rule: only a sensor may read a device.\"
  permit device.read
  principal is Agent
  when principal.role == \"sensor\"
}
";

    fn now() -> u64 {
        apz_plaza::now_ms()
    }

    #[test]
    fn a_rule_nested_past_the_parser_budget_is_refused() {
        let laws = Laws::builtin();
        let source = format!(
            "law \"O-900\" \"Deep nesting\" {{
  tier statute
  enforcement preventive
  severity info
  statement \"Test rule: deeply nested.\"
  forbid tool.call
  principal is Agent
  resource is Tool
  when {open}true{close}
}}
",
            open = "(".repeat(20_000),
            close = ")".repeat(20_000)
        );
        let err = laws
            .add_rule("Deep nesting", &source, true, now())
            .expect_err("a rule nested past the parser budget is refused");
        assert!(
            matches!(&err, Failure::Diagnostics(d) if d.iter().any(|x| x.message.contains("nested deeper than 32 levels"))),
            "the refusal names the nesting limit"
        );
    }

    #[test]
    fn a_rule_source_over_64_kib_is_refused() {
        let laws = Laws::builtin();
        let source = "x".repeat(64 * 1024 + 1);
        let err = laws
            .add_rule("Too large", &source, true, now())
            .expect_err("a source over 64 KiB is refused");
        assert!(
            matches!(&err, Failure::Diagnostics(d) if d.iter().any(|x| x.message == "a rule's source is limited to 64 KiB")),
            "the refusal names the size limit"
        );
    }

    /// The gate every tool call passes: `Ok` with where it runs, or the refusal.
    fn gate(laws: &Laws, tool: &str, input: &Json) -> Result<Runs, String> {
        let granted = crate::tools::offered_names("builder");
        crate::internal::gate_call(laws, 7, "builder", &granted, tool, input)
    }

    #[test]
    fn a_rule_that_does_not_compile_is_rejected_with_its_place_and_not_applied() {
        let laws = Laws::builtin();
        let broken = "law \"O-900\" \"Broken\" {\n  tier statute\n  frobnicate everything\n}\n";
        match laws.add_rule("", broken, true, now()) {
            Err(Failure::Diagnostics(list)) => assert!(
                list.iter().any(|d| d.line >= 1 && d.column >= 1),
                "{list:?}"
            ),
            other => panic!("expected diagnostics, got {other:?}"),
        }
        assert!(laws.rules().rules.is_empty());
        assert!(!laws.catalog().iter().any(|info| info.id == "O-900"));
        // A source that declares no law is refused too.
        assert!(matches!(
            laws.add_rule("", "// nothing here\n", true, now()),
            Err(Failure::Diagnostics(_))
        ));
    }

    #[test]
    fn an_added_forbid_refuses_the_call_at_once_and_disabling_it_stops_that() {
        let laws = Laws::builtin();
        let input = json!({"path": "a.md"});
        assert_eq!(gate(&laws, "read", &input), Ok(Runs::Guest));
        let rule = laws
            .add_rule("No reading files", NO_READS, true, now())
            .expect("compiles");
        assert_eq!(rule.id, "R-001");
        let reason = gate(&laws, "read", &input).unwrap_err();
        assert!(reason.contains("O-001"), "{reason}");
        assert!(
            laws.catalog().iter().any(|info| info.id == "O-001"
                && info.origin == "owner"
                && info.rule_id == "R-001")
        );
        laws.set_rule_enabled(&rule.id, false, now()).unwrap();
        assert_eq!(gate(&laws, "read", &input), Ok(Runs::Guest));
        laws.set_rule_enabled(&rule.id, true, now()).unwrap();
        assert!(gate(&laws, "read", &input).is_err());
        laws.delete_rule(&rule.id).unwrap();
        assert_eq!(gate(&laws, "read", &input), Ok(Runs::Guest));
        assert!(matches!(
            laws.delete_rule("R-001"),
            Err(Failure::NotFound(_))
        ));
    }

    #[test]
    fn an_owner_disables_one_builtin_statute_with_a_reason_on_record() {
        let laws = Laws::builtin();
        let input = json!({"to_cid": 9, "text": "key sk-proj-abcdefghijklmnopqrstuvwxyz0123"});
        let forbids = |laws: &Laws| {
            laws.check("7", "builder", "plaza_send_message", &input)
                .forbid
        };
        assert!(forbids(&laws).iter().any(|id| id == "C-002b"));
        assert!(
            matches!(
                laws.set_override("C-002b", true, "  ", "C-002b", now()),
                Err(Failure::Invalid(_))
            ),
            "a reason is required"
        );
        let made = laws
            .set_override("C-002b", true, "testing the override", "C-002b", now())
            .unwrap()
            .expect("disabled");
        assert_eq!(made.disabled_by, OWNER);
        assert_eq!(made.reason, "testing the override");
        assert!(!forbids(&laws).iter().any(|id| id == "C-002b"));
        let info = laws
            .catalog()
            .into_iter()
            .find(|info| info.id == "C-002b")
            .unwrap();
        assert!(!info.enabled && info.overridden.is_some());
        assert_eq!(laws.rules().overrides.len(), 1);
        laws.set_override("C-002b", false, "", "", now()).unwrap();
        assert!(forbids(&laws).iter().any(|id| id == "C-002b"));
        // Only built-in statutes take an override; an owner rule is disabled as a rule.
        laws.add_rule("No reading files", NO_READS, true, now())
            .unwrap();
        assert!(matches!(
            laws.set_override("O-001", true, "why", "", now()),
            Err(Failure::Invalid(_))
        ));
        assert!(matches!(
            laws.set_override("Z-999", true, "why", "", now()),
            Err(Failure::NotFound(_))
        ));
    }

    #[test]
    fn strict_permits_refuse_an_operation_no_permit_allows_only_when_on() {
        let laws = Laws::builtin();
        laws.add_rule("Only sensors read devices", SENSORS_ONLY, true, now())
            .expect("compiles");
        let device = || Entity {
            ty: "Device".into(),
            id: "d1".into(),
        };
        let read = |laws: &Laws, role: &str| {
            laws.refuse_op(
                7,
                role,
                "device.read",
                device(),
                BTreeMap::new(),
                &Json::Null,
            )
        };
        assert!(!laws.strict());
        assert!(read(&laws, "builder").is_none(), "strict permits are off");
        laws.set_strict(true);
        assert!(read(&laws, "builder").is_some_and(|why| why.contains("device.read")));
        assert!(read(&laws, "sensor").is_none());
        // The orchestrator still posts tasks under strict permits: S-003c grants it task.create.
        let verdict = laws.check(
            "7",
            crate::settings::ORCHESTRATOR_ROLE,
            "plaza_post_task",
            &json!({"title": "t", "body": "b"}),
        );
        assert!(
            verdict.forbid.is_empty() && !verdict.unpermitted && verdict.denied_ops.is_empty(),
            "{verdict:?}"
        );
        laws.set_strict(false);
        assert!(read(&laws, "builder").is_none());
    }

    /// The laws as plaza.db keeps them, loaded into a fresh set: the owner's rules and overrides go through their
    /// tables, as the saver writes them and the start loads them.
    fn restored(laws: &Laws) -> Laws {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &laws.rows().unwrap()).unwrap();
        let fresh = Laws::builtin();
        fresh.load(&conn).unwrap();
        fresh
    }

    #[test]
    fn owner_rules_and_overrides_survive_a_restart() {
        let laws = Laws::builtin();
        laws.add_rule("No reading files", NO_READS, true, now())
            .unwrap();
        laws.set_override("C-002b", true, "testing", "C-002b", now())
            .unwrap();
        let before = laws.rules();
        let again = restored(&laws);
        assert_eq!(again.rules(), before);
        assert!(gate(&again, "read", &json!({"path": "a.md"})).is_err());
        assert!(
            again
                .catalog()
                .iter()
                .any(|info| info.id == "O-001" && info.enabled)
        );
    }

    /// Stored rules that no longer compile are kept as they are and not applied: nothing is lost, and the built-in
    /// laws alone decide until the owner fixes or removes them.
    #[test]
    fn stored_rules_that_do_not_compile_are_kept_and_not_applied() {
        let laws = Laws::builtin();
        laws.add_rule("No reading files", NO_READS, true, now())
            .unwrap();
        let mut bad = laws.rules();
        bad.rules[0].source = "law \"O-901\" \"Bad\" {".into();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        let rows = apz_store::RowSet::one(def(), &bad).unwrap();
        apz_store::table::replace(&conn, def(), &rows).unwrap();
        let loaded = Laws::builtin();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.rules(), bad, "kept as stored");
        assert!(
            !loaded.catalog().iter().any(|info| info.id == "O-001"),
            "not applied"
        );
        assert!(
            loaded.not_applied().is_some(),
            "and the Rules view says why"
        );
    }

    #[test]
    fn a_dry_run_lists_the_recent_calls_a_rule_would_refuse_and_applies_nothing() {
        let laws = Laws::builtin();
        assert!(
            laws.refusal(7, "builder", "read", &json!({"path": "a.md"}))
                .is_none()
        );
        let run = laws.dry_run(NO_READS, None).expect("compiles");
        assert_eq!(run.laws, vec!["O-001".to_string()]);
        assert_eq!(run.would_refuse.len(), 1, "{:?}", run.would_refuse);
        assert_eq!(run.would_refuse[0]["tool"], "read");
        assert!(laws.rules().rules.is_empty());
        assert!(matches!(
            laws.dry_run("law \"O-902\" \"x\" {", None),
            Err(Failure::Diagnostics(_))
        ));
    }

    #[test]
    fn a_constitutional_law_switches_off_only_with_its_typed_id_and_its_changes_are_kept() {
        let laws = Laws::builtin();
        let law = "C-002b";
        let input = json!({"to_cid": 9, "text": "key sk-proj-abcdefghijklmnopqrstuvwxyz0123"});
        let forbids = |laws: &Laws| {
            laws.check("7", "builder", "plaza_send_message", &input)
                .forbid
        };
        let asked =
            format!("{law} is a constitutional law; to switch it off, send confirm: \"{law}\"");
        // Without the typed id, or with another one, the switch-off is refused and nothing is recorded.
        match laws.set_override(law, true, "testing", "", now()) {
            Err(Failure::Unconfirmed(why)) => assert_eq!(why, asked),
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
        match laws.set_override(law, true, "testing", "C-002", now()) {
            Err(Failure::Unconfirmed(why)) => {
                assert_eq!(why, format!("confirm \"C-002\" does not match. {asked}"))
            }
            other => panic!("expected Unconfirmed, got {other:?}"),
        }
        assert!(forbids(&laws).iter().any(|id| id == law));
        assert!(laws.constitutional_off().is_empty());
        assert!(laws.rules().history.is_empty());
        // With the id, the law is off, listed as off, and the change is on record.
        let made = laws
            .set_override(law, true, "testing", law, 1_000)
            .unwrap()
            .expect("disabled");
        assert_eq!(made.disabled_by, OWNER);
        assert!(!forbids(&laws).iter().any(|id| id == law));
        assert_eq!(laws.constitutional_off(), vec![law.to_string()]);
        let title = laws
            .catalog()
            .into_iter()
            .find(|info| info.id == law)
            .unwrap()
            .title;
        let history = laws.rules().history;
        assert_eq!(
            history,
            vec![LawChange {
                law: law.into(),
                title: title.clone(),
                tier: CONSTITUTION.into(),
                disabled: true,
                by: OWNER.into(),
                reason: "testing".into(),
                at_ms: 1_000,
                confirmed: true,
            }]
        );
        // Switching it back on needs no confirmation, and is recorded as well.
        laws.set_override(law, false, "", "", 2_000).unwrap();
        assert!(forbids(&laws).iter().any(|id| id == law));
        assert!(laws.constitutional_off().is_empty());
        let history = laws.rules().history;
        assert_eq!(history.len(), 2);
        assert!(
            !history[1].disabled
                && !history[1].confirmed
                && history[1].at_ms == 2_000
                && history[1].title == title
        );
        // The history is part of the rules: it survives a restart.
        assert_eq!(restored(&laws).rules().history, history);
        // A statute of another tier is switched off with no confirmation and no entry in the history.
        laws.set_override("S-013d", true, "researchers may commit", "", 3_000)
            .unwrap();
        assert!(laws.rules().overrides.iter().any(|o| o.law == "S-013d"));
        assert!(laws.constitutional_off().is_empty());
        assert_eq!(laws.rules().history.len(), 2);
    }
}
