//! Authorization over laws (spec 9.13-9.14).
//!
//! `authorize` decides one request against the laws that cover its operation. Conditions are
//! three-valued: a condition that reads a missing attribute is `Unknown` and names the missing
//! path. Fail closed: a forbid whose condition is unknown applies; a permit whose condition is
//! unknown does not. Any applying forbid denies; otherwise any applying permit allows; otherwise
//! the request is denied by default.

use crate::ast::*;
use crate::eval::binop;
use crate::program::Program;
use crate::span::{Diag, Span};
use crate::value::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A reference to an entity by declared type and id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entity {
    pub ty: String,
    pub id: String,
}

/// One access request: who does what to which resource, with a context record.
#[derive(Clone, Debug)]
pub struct Request {
    pub principal: Entity,
    pub operation: String,
    pub resource: Entity,
    pub context: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow { by: Vec<String> },
    Deny { by: Vec<String>, missing: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub decision: Decision,
    /// Ids of applying forbids whose enforcement is `detective` or `judgment`.
    pub violations: Vec<String>,
}

/// The entities of one fixture: attributes keyed by (type, id). A `parent` attribute holding
/// an entity reference links the ancestry used by `in`.
#[derive(Clone, Debug, Default)]
pub struct EntityStore {
    attrs: BTreeMap<(String, String), BTreeMap<String, Value>>,
}

impl EntityStore {
    /// Adds (or replaces) one entity and its attributes.
    pub fn insert(&mut self, e: &Entity, attrs: BTreeMap<String, Value>) {
        self.attrs.insert((e.ty.clone(), e.id.clone()), attrs);
    }

    pub fn from_fixture(fx: &Fixture) -> EntityStore {
        let mut store = EntityStore::default();
        for e in &fx.entities {
            let attrs: BTreeMap<String, Value> = e.fields.iter().map(|(n, x)| (n.clone(), konst(x))).collect();
            store.attrs.insert((e.ty.clone(), e.id.clone()), attrs);
        }
        store
    }

    fn attr(&self, e: &Entity, name: &str) -> Option<&Value> {
        self.attrs.get(&(e.ty.clone(), e.id.clone()))?.get(name)
    }

    fn parent(&self, e: &Entity) -> Option<Entity> {
        self.attr(e, "parent").and_then(entity_of)
    }
}

/// The named fixture's entities, if the program declares it.
pub fn store_for(program: &Program, fixture: &str) -> Option<EntityStore> {
    program.items().find_map(|(_, _, it)| match it {
        Item::Fixture(f) if f.name == fixture => Some(EntityStore::from_fixture(f)),
        _ => None,
    })
}

/// The runtime value of an entity reference: a record carrying its type and id.
pub fn entity_value(e: &Entity) -> Value {
    let mut m = BTreeMap::new();
    m.insert("$type".to_string(), Value::str(&e.ty));
    m.insert("$id".to_string(), Value::str(&e.id));
    Value::record(m)
}

fn entity_of(v: &Value) -> Option<Entity> {
    let Value::Record(m) = v else { return None };
    let (Some(Value::Str(t)), Some(Value::Str(i))) = (m.get("$type"), m.get("$id")) else { return None };
    Some(Entity { ty: t.to_string(), id: i.to_string() })
}

fn path_of(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Name(n) => n.clone(),
        ExprKind::Field(b, f) => format!("{}.{f}", path_of(b)),
        _ => "<expr>".to_string(),
    }
}

/// Evaluates a constant fixture expression (no request, no store lookups).
fn konst(e: &Expr) -> Value {
    let store = EntityStore::default();
    let nobody = Entity::default();
    let ctx = Value::record(BTreeMap::new());
    Eval { store: &store, principal: &nobody, resource: &nobody, context: &ctx }.eval(e)
}

fn not(v: Value) -> Value {
    match v {
        Value::Bool(b) => Value::Bool(!b),
        Value::Unknown(p) => Value::Unknown(p),
        _ => Value::Unknown("not a boolean".into()),
    }
}

fn and(x: Value, y: Value) -> Value {
    match (x, y) {
        (Value::Bool(false), _) | (_, Value::Bool(false)) => Value::Bool(false),
        (Value::Bool(true), Value::Bool(true)) => Value::Bool(true),
        (Value::Unknown(p), _) | (_, Value::Unknown(p)) => Value::Unknown(p),
        _ => Value::Unknown("not a boolean".into()),
    }
}

fn or(x: Value, y: Value) -> Value {
    match (x, y) {
        (Value::Bool(true), _) | (_, Value::Bool(true)) => Value::Bool(true),
        (Value::Bool(false), Value::Bool(false)) => Value::Bool(false),
        (Value::Unknown(p), _) | (_, Value::Unknown(p)) => Value::Unknown(p),
        _ => Value::Unknown("not a boolean".into()),
    }
}

struct Eval<'a> {
    store: &'a EntityStore,
    principal: &'a Entity,
    resource: &'a Entity,
    context: &'a Value,
}

impl Eval<'_> {
    fn is_in(&self, from: &Entity, target: &Entity) -> bool {
        let mut cur = Some(from.clone());
        for _ in 0..64 {
            let Some(c) = cur else { return false };
            if &c == target {
                return true;
            }
            cur = self.store.parent(&c);
        }
        false
    }

    fn eval(&self, e: &Expr) -> Value {
        match &e.kind {
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Str(s) => Value::Str(s.clone()),
            ExprKind::NoneLit => Value::None,
            ExprKind::Num(n, d) => Value::Num(n.clone(), d.clone()),
            ExprKind::Name(n) => match n.as_str() {
                "principal" => entity_value(self.principal),
                "resource" => entity_value(self.resource),
                "context" => self.context.clone(),
                _ => Value::Unknown(n.clone()),
            },
            ExprKind::Field(base, f) => {
                let path = format!("{}.{f}", path_of(base));
                match self.eval(base) {
                    Value::Unknown(p) => Value::Unknown(p),
                    v => {
                        if let Some(ent) = entity_of(&v) {
                            return self.store.attr(&ent, f).cloned().unwrap_or(Value::Unknown(path));
                        }
                        match &v {
                            Value::Record(m) => m.get(f).cloned().unwrap_or(Value::Unknown(path)),
                            _ => Value::Unknown(path),
                        }
                    }
                }
            }
            ExprKind::Is(x, ty) => match self.eval(x) {
                Value::Unknown(p) => Value::Unknown(p),
                v => match entity_of(&v) {
                    Some(ent) => Value::Bool(ent.ty == *ty),
                    None => Value::Unknown(format!("{} is not an entity", path_of(x))),
                },
            },
            ExprKind::Unary(op, x) => match (op, self.eval(x)) {
                (UnOp::Not, v) => not(v),
                (_, Value::Unknown(p)) => Value::Unknown(p),
                _ => Value::Unknown("unsupported operator".into()),
            },
            ExprKind::Binary(op, a, b) => self.binary(op, a, b, e.span),
            ExprKind::If(c, t, f) => match self.truth(c) {
                Value::Bool(true) => self.eval(t),
                Value::Bool(false) => self.eval(f),
                u => u,
            },
            ExprKind::Call(callee, args) => self.call(callee, args),
            ExprKind::Record(fields) => {
                Value::record(fields.iter().map(|(n, x)| (n.clone(), self.eval(x))).collect())
            }
            ExprKind::Set(xs) => Value::set(xs.iter().map(|x| self.eval(x)).collect::<Vec<_>>()),
            ExprKind::List(xs) => Value::list(xs.iter().map(|x| self.eval(x)).collect()),
            ExprKind::Tuple(xs) => Value::tuple(xs.iter().map(|x| self.eval(x)).collect()),
            _ => Value::Unknown("unsupported expression".into()),
        }
    }

    /// A boolean view of an expression: `Bool` or `Unknown`, never anything else.
    fn truth(&self, e: &Expr) -> Value {
        match self.eval(e) {
            v @ (Value::Bool(_) | Value::Unknown(_)) => v,
            _ => Value::Unknown(format!("{} is not a boolean", path_of(e))),
        }
    }

    fn binary(&self, op: &BinOp, a: &Expr, b: &Expr, span: Span) -> Value {
        match op {
            BinOp::And => and(self.truth(a), self.truth(b)),
            BinOp::Or => or(self.truth(a), self.truth(b)),
            BinOp::Implies => or(not(self.truth(a)), self.truth(b)),
            BinOp::Coalesce => match self.eval(a) {
                Value::None => self.eval(b),
                v => v,
            },
            _ => {
                let x = self.eval(a);
                let y = self.eval(b);
                if let Value::Unknown(p) = &x {
                    return Value::Unknown(p.clone());
                }
                if let Value::Unknown(p) = &y {
                    return Value::Unknown(p.clone());
                }
                if matches!(op, BinOp::In) {
                    if let (Some(from), Some(anc)) = (entity_of(&x), entity_of(&y)) {
                        return Value::Bool(self.is_in(&from, &anc));
                    }
                }
                match binop(*op, x, y, span) {
                    Ok(v) => v,
                    Err(d) => Value::Unknown(d.msg),
                }
            }
        }
    }

    fn call(&self, callee: &Expr, args: &[Arg]) -> Value {
        match (&callee.kind, args) {
            (ExprKind::Name(f), [arg]) if f == "has" => match self.eval(&arg.value) {
                Value::Unknown(_) | Value::None => Value::Bool(false),
                _ => Value::Bool(true),
            },
            (ExprKind::Name(ty), [arg]) => match &arg.value.kind {
                ExprKind::Str(id) => entity_value(&Entity { ty: ty.clone(), id: id.to_string() }),
                _ => Value::Unknown(format!("{ty}(...) needs a string id")),
            },
            _ => Value::Unknown("unsupported call".into()),
        }
    }

    fn scope(&self, s: &Scope, me: &Entity) -> Value {
        match s {
            Scope::Is(t) => Value::Bool(t.name() == Some(me.ty.as_str())),
            Scope::Eq(x) => match self.eval(x) {
                Value::Unknown(p) => Value::Unknown(p),
                v => match entity_of(&v) {
                    Some(e) => Value::Bool(e == *me),
                    None => Value::Unknown(format!("{} is not an entity", path_of(x))),
                },
            },
            Scope::In(x) => match self.eval(x) {
                Value::Unknown(p) => Value::Unknown(p),
                v => match entity_of(&v) {
                    Some(anc) => Value::Bool(self.is_in(me, &anc)),
                    None => Value::Unknown(format!("{} is not an entity", path_of(x))),
                },
            },
        }
    }

    fn law_applies(&self, law: &Law) -> Value {
        let mut acc = Value::Bool(true);
        if let Some(s) = &law.principal {
            acc = and(acc, self.scope(s, self.principal));
        }
        if let Some(s) = &law.resource {
            acc = and(acc, self.scope(s, self.resource));
        }
        for w in &law.when {
            acc = and(acc, self.truth(w));
        }
        for u in &law.unless {
            acc = and(acc, not(self.truth(u)));
        }
        acc
    }
}

/// Decides one request. See the module docs for the three-valued rules.
pub fn authorize(program: &Program, entities: &EntityStore, req: &Request) -> Outcome {
    authorize_except(program, entities, req, &HashSet::new())
}

/// [`authorize`] with the laws whose ids are in `off` left out: a law that is off applies to nothing.
pub fn authorize_except(program: &Program, entities: &EntityStore, req: &Request, off: &HashSet<String>) -> Outcome {
    let ev = Eval { store: entities, principal: &req.principal, resource: &req.resource, context: &req.context };
    let mut allow_by = Vec::new();
    let mut deny_by = Vec::new();
    let mut missing = Vec::new();
    let mut violations = Vec::new();
    for (_, _, item) in program.items() {
        let Item::Law(law) = item else { continue };
        if off.contains(&law.id) || !law.ops.iter().any(|o| o == "any" || *o == req.operation) {
            continue;
        }
        let judgment = matches!(law.enforcement.as_deref(), Some("detective") | Some("judgment"));
        match (law.effect, ev.law_applies(law)) {
            (Effect::Forbid, applies) if !matches!(applies, Value::Bool(false)) => {
                deny_by.push(law.id.clone());
                if let Value::Unknown(path) = applies {
                    missing.push(path);
                }
                if judgment {
                    violations.push(law.id.clone());
                }
            }
            (Effect::Permit, Value::Bool(true)) => allow_by.push(law.id.clone()),
            _ => {}
        }
    }
    let decision = if !deny_by.is_empty() {
        Decision::Deny { by: deny_by, missing }
    } else if !allow_by.is_empty() {
        Decision::Allow { by: allow_by }
    } else {
        Decision::Deny { by: Vec::new(), missing: Vec::new() }
    };
    Outcome { decision, violations }
}

/// Static checks of the authorization items: laws name declared operations, law ids are unique,
/// authz blocks name a declared fixture, and each case's `by` names a declared law.
pub fn check_authz(program: &Program) -> Vec<Diag> {
    let mut ops = HashSet::new();
    let mut law_ids = HashSet::new();
    let mut fixtures = HashSet::new();
    for (_, _, it) in program.items() {
        match it {
            Item::Operation(o) => {
                ops.insert(o.name.clone());
            }
            Item::Law(l) => {
                law_ids.insert(l.id.clone());
            }
            Item::Fixture(f) => {
                fixtures.insert(f.name.clone());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_, _, it) in program.items() {
        match it {
            Item::Law(l) => {
                if !seen.insert(l.id.clone()) {
                    out.push(Diag::error(l.span, format!("law `{}` is declared more than once", l.id)));
                }
                for op in &l.ops {
                    if op != "any" && !ops.contains(op) {
                        out.push(Diag::error(l.span, format!("law `{}`: operation `{op}` is not declared", l.id)));
                    }
                }
            }
            Item::Authz(az) => {
                if !fixtures.contains(&az.fixture) {
                    out.push(Diag::error(az.span, format!("authz `{}`: fixture `{}` is not declared", az.name, az.fixture)));
                }
                for c in &az.cases {
                    if let Some(by) = &c.law {
                        if !law_ids.contains(by) {
                            out.push(Diag::error(c.span, format!("authz case `{}`: law `{by}` is not declared", c.label)));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn describe(d: &Decision) -> String {
    match d {
        Decision::Allow { by } => format!("allow by {by:?}"),
        Decision::Deny { by, missing } => format!("deny by {by:?}, missing {missing:?}"),
    }
}

/// Runs every authz case against its fixture. Each row is (label, pass, detail).
pub fn run_authz(program: &Program) -> Vec<(String, bool, String)> {
    let mut fixtures: HashMap<String, EntityStore> = HashMap::new();
    for (_, _, it) in program.items() {
        if let Item::Fixture(f) = it {
            fixtures.insert(f.name.clone(), EntityStore::from_fixture(f));
        }
    }
    let mut out = Vec::new();
    for (_, _, it) in program.items() {
        let Item::Authz(az) = it else { continue };
        let Some(store) = fixtures.get(&az.fixture) else {
            for c in &az.cases {
                out.push((c.label.clone(), false, format!("fixture `{}` is not declared", az.fixture)));
            }
            continue;
        };
        for c in &az.cases {
            let mut req = Request {
                principal: Entity { ty: c.subject.ty.clone(), id: c.subject.id.clone() },
                operation: c.op.clone(),
                resource: Entity { ty: c.object.ty.clone(), id: c.object.id.clone() },
                context: Value::record(BTreeMap::new()),
            };
            let ctx = Eval { store, principal: &req.principal, resource: &req.resource, context: &req.context }.eval(&c.record);
            req.context = ctx;
            let outcome = authorize(program, store, &req);
            let allowed = matches!(outcome.decision, Decision::Allow { .. });
            let by = match &outcome.decision {
                Decision::Allow { by } | Decision::Deny { by, .. } => by.clone(),
            };
            let law_ok = c.law.as_ref().map_or(true, |l| by.contains(l));
            let pass = allowed == c.allow && law_ok;
            let detail = if pass {
                describe(&outcome.decision)
            } else {
                format!(
                    "expected {}, got {}{}",
                    if c.allow { "allow" } else { "deny" },
                    describe(&outcome.decision),
                    if law_ok { "" } else { " (named law not in by)" }
                )
            };
            out.push((c.label.clone(), pass, detail));
        }
    }
    out
}
