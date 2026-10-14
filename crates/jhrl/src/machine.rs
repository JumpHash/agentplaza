//! Machine instances (docs/jhrl.md section 9.9): constants bound by a model or scenario, the
//! initial states, and the successor relation of each action.
//!
//! - `var x: T in S` gives one initial state per member of S; an `init { … }` block may refine
//!   them with `pick` and `either`.
//! - An action's `when` guards and primed assignments run in order; unprimed names read the
//!   current state, `x'` reads a value assigned earlier in the same action. A variable that is not
//!   assigned keeps its value. Assigning a variable twice in one branch is an error.
//! - Parameters `n: T` range over the model's domain for T, or `a in S` over the set S.

use crate::ast::*;
use crate::eval::{type_dim, Env};
use crate::num::{Dim, Num};
use crate::program::Program;
use crate::span::{Diag, Span};
use crate::value::Value;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub name: String,
    pub ty: TypeExpr,
    pub optional: bool,
    pub init: VarInit,
    pub span: Span,
}

/// A machine with its constants bound.
#[derive(Clone, Debug)]
pub struct MachineCtx {
    pub name: String,
    pub pack: usize,
    pub consts: HashMap<String, Value>,
    pub const_order: Vec<String>,
    pub vars: Vec<VarInfo>,
    pub var_index: HashMap<String, usize>,
    /// Type names that range over finite sets here: model domains, model values, enums, Bool.
    pub sets: HashMap<String, Value>,
    pub model_values: HashMap<String, Value>,
    /// Machine-level definitions: name → (parameters, body).
    pub defs: HashMap<String, (Vec<String>, Expr)>,
    pub decl: MachineDecl,
}

pub type State = Vec<Value>;

/// One way to take an action: its parameter values and the next state.
#[derive(Clone, Debug)]
pub struct Succ {
    pub params: Vec<(String, Value)>,
    pub next: State,
}

#[derive(Clone)]
struct Branch {
    next: Vec<Option<Value>>,
    locals: Vec<(String, Value)>,
}

fn err<T>(span: Span, msg: impl Into<String>) -> Result<T, Diag> {
    Err(Diag::error(span, msg))
}

fn set_elems(v: &Value, span: Span, what: &str) -> Result<Vec<Value>, Diag> {
    match v {
        Value::Set(s) => Ok(s.iter().cloned().collect()),
        Value::List(l) => Ok(l.to_vec()),
        Value::Unknown(r) => err(span, format!("{what} is unknown: {r}")),
        _ => err(
            span,
            format!("{what} must be a set, found {} {v}", v.type_name()),
        ),
    }
}

/// Where a const's value comes from: an expression to evaluate in some pack.
pub struct ConstSource<'a> {
    pub name: String,
    pub expr: &'a Expr,
    pub pack: usize,
}

impl MachineCtx {
    /// Binds a machine for a model (domains, values, consts) or a scenario (consts only).
    pub fn new(
        prog: &Program,
        pack: usize,
        decl: &MachineDecl,
        model: Option<(&ModelDecl, usize)>,
        consts: &[ConstSource],
    ) -> Result<MachineCtx, Diag> {
        if decl.compose.is_some() {
            return err(decl.span, "machine composition (`||`) is not in v0");
        }
        let mut m = MachineCtx {
            name: decl.name.clone(),
            pack,
            consts: HashMap::new(),
            const_order: vec![],
            vars: vec![],
            var_index: HashMap::new(),
            sets: HashMap::new(),
            model_values: HashMap::new(),
            defs: HashMap::new(),
            decl: decl.clone(),
        };
        for it in &decl.items {
            if let MItem::Def {
                name,
                params,
                body,
                span,
            } = it
            {
                if m.defs
                    .insert(name.clone(), (params.clone(), body.clone()))
                    .is_some()
                {
                    return err(*span, format!("`{name}` is defined twice"));
                }
            }
        }
        // Enums visible from the machine's pack range over their members.
        for (_, r, it) in prog.items() {
            if let Item::Enum(e) = it {
                if prog.lookup_decl(pack, &e.name) == Some(r) {
                    m.sets.insert(
                        e.name.clone(),
                        Value::set(e.members.iter().map(|x| Value::str(&x.0))),
                    );
                }
            }
        }
        m.sets.insert(
            "Bool".into(),
            Value::set([Value::Bool(false), Value::Bool(true)]),
        );
        if let Some((md, mpack)) = model {
            for o in &md.opts {
                match o {
                    MOpt::Values(t, e) => {
                        let ExprKind::Set(items) = &e.kind else {
                            return err(
                                e.span,
                                "`values` takes a set of names, like {mayor, owner}",
                            );
                        };
                        let mut set = BTreeSet::new();
                        for it in items {
                            let ExprKind::Name(n) = &it.kind else {
                                return err(it.span, "a model value is a bare name");
                            };
                            let v = Value::Model(n.as_str().into());
                            m.model_values.insert(n.clone(), v.clone());
                            set.insert(v);
                        }
                        m.sets.insert(t.clone(), Value::Set(set.into()));
                    }
                    MOpt::Domain(t, lo, hi) => {
                        let mut env = Env::machine(prog, &m, None);
                        env.pack = mpack;
                        let a = env.eval(lo)?;
                        let b = env.eval(hi)?;
                        let (Value::Num(a, da), Value::Num(b, db)) = (&a, &b) else {
                            return err(lo.span, "a domain is `lo .. hi` over numbers");
                        };
                        if da != db {
                            return err(
                                lo.span.to(hi.span),
                                format!(
                                    "domain ends differ in dimension: {} and {}",
                                    da.describe(),
                                    db.describe()
                                ),
                            );
                        }
                        if let Some(td) = Dim::of_type_name(t) {
                            if td != *da {
                                return err(
                                    lo.span,
                                    format!(
                                        "domain {t} is {}, but the bounds are {}",
                                        td.describe(),
                                        da.describe()
                                    ),
                                );
                            }
                        }
                        let (Some(ai), Some(bi)) = (a.as_int(), b.as_int()) else {
                            return err(
                                lo.span,
                                "domain bounds must be whole base units (e.g. 0 cr .. 3 cr)",
                            );
                        };
                        if bi - ai > 100_000 {
                            return err(lo.span, "domain has more than 100,000 values");
                        }
                        let set: BTreeSet<Value> =
                            (ai..=bi).map(|i| Value::Num(Num::int(i), *da)).collect();
                        m.sets.insert(t.clone(), Value::Set(set.into()));
                    }
                    MOpt::Const(n, e)
                        if !decl
                            .items
                            .iter()
                            .any(|i| matches!(i, MItem::Const { name, .. } if name == n)) =>
                    {
                        return err(e.span, format!("machine {} has no const `{n}`", decl.name));
                    }
                    _ => {}
                }
            }
        }
        // Constants, in declaration order: model or scenario value, else the default.
        for it in &decl.items {
            if let MItem::Const {
                name,
                ty,
                default,
                span,
            } = it
            {
                let given_model = model.and_then(|(md, mpack)| {
                    md.opts.iter().find_map(|o| match o {
                        MOpt::Const(n, e) if n == name => Some((e, mpack)),
                        _ => None,
                    })
                });
                let given = consts
                    .iter()
                    .find(|c| c.name == *name)
                    .map(|c| (c.expr, c.pack))
                    .or(given_model);
                let v = match (given, default) {
                    (Some((e, p)), _) => {
                        let mut env = Env::machine(prog, &m, None);
                        env.pack = p;
                        env.eval(e)?
                    }
                    (None, Some(e)) => Env::machine(prog, &m, None).eval(e)?,
                    (None, None) => Value::Unknown(format!("const {name} is not set by the model")),
                };
                if !matches!(v, Value::Unknown(_)) && !admits(prog, pack, ty, &v, false) {
                    return err(*span, format!("const {name}: {v} is not a {ty}"));
                }
                m.consts.insert(name.clone(), v);
                m.const_order.push(name.clone());
            }
        }
        for c in consts {
            if !m.consts.contains_key(&c.name)
                && !decl
                    .items
                    .iter()
                    .any(|i| matches!(i, MItem::Var { name, .. } if *name == c.name))
            {
                return err(
                    c.expr.span,
                    format!("machine {} has no const or var `{}`", decl.name, c.name),
                );
            }
        }
        for it in &decl.items {
            if let MItem::Var {
                name,
                ty,
                optional,
                init,
                span,
            } = it
            {
                if m.var_index.contains_key(name) || m.consts.contains_key(name) {
                    return err(*span, format!("`{name}` is declared twice"));
                }
                m.var_index.insert(name.clone(), m.vars.len());
                m.vars.push(VarInfo {
                    name: name.clone(),
                    ty: ty.clone(),
                    optional: *optional,
                    init: init.clone(),
                    span: *span,
                });
            }
        }
        Ok(m)
    }

    pub fn action(&self, name: &str) -> Option<&ActionDecl> {
        self.decl.actions().find(|a| a.name == name)
    }

    /// All initial states (deduplicated, in a deterministic order).
    pub fn initial_states(&self, prog: &Program) -> Result<Vec<State>, Diag> {
        let n = self.vars.len();
        let mut partial: Vec<State> = vec![vec![Value::Unknown("not yet initialised".into()); n]];
        let has_init = self.decl.items.iter().any(|i| matches!(i, MItem::Init(..)));
        for (i, v) in self.vars.iter().enumerate() {
            let mut out = Vec::new();
            for s in &partial {
                let mut env = Env::machine(prog, self, Some(s));
                let choices = match &v.init {
                    VarInit::Eq(e) => vec![env.eval(e)?],
                    VarInit::In(e) => {
                        let set = env.eval(e)?;
                        let xs = set_elems(&set, e.span, &format!("the initial set of `{}`", v.name))?;
                        if xs.is_empty() {
                            return err(e.span, format!("the initial set of `{}` is empty", v.name));
                        }
                        xs
                    }
                    VarInit::None if v.optional => vec![Value::None],
                    VarInit::None if has_init => vec![Value::Unknown(format!("{} (set it in init)", v.name))],
                    VarInit::None => return err(v.span, format!("`{}` has no initial value (`= e`, `in S`, `optional` or an init block)", v.name)),
                };
                for c in choices {
                    let mut t = s.clone();
                    t[i] = c;
                    out.push(t);
                }
            }
            partial = out;
        }
        let mut states = partial;
        for it in &self.decl.items {
            if let MItem::Init(body, _) = it {
                let mut out = Vec::new();
                for s in &states {
                    for b in self.exec(prog, body, s, vec![])? {
                        out.push(self.apply(s, &b));
                    }
                }
                states = out;
            }
        }
        let mut seen = BTreeSet::new();
        states.retain(|s| seen.insert(s.clone()));
        for s in &states {
            if let Some((i, Value::Unknown(r))) = s
                .iter()
                .enumerate()
                .find(|(_, v)| matches!(v, Value::Unknown(_)))
            {
                return err(
                    self.vars[i].span,
                    format!("initial value of `{}` is unknown: {r}", self.vars[i].name),
                );
            }
        }
        Ok(states)
    }

    fn apply(&self, cur: &[Value], b: &Branch) -> State {
        cur.iter()
            .zip(&b.next)
            .map(|(c, n)| n.clone().unwrap_or_else(|| c.clone()))
            .collect()
    }

    /// The values each parameter of `a` ranges over in state `cur`.
    pub fn param_values(
        &self,
        prog: &Program,
        a: &ActionDecl,
        cur: &[Value],
    ) -> Result<Vec<Vec<Value>>, Diag> {
        let mut out = Vec::new();
        for p in &a.params {
            let vals = match &p.dom {
                ADom::In(e) => {
                    let mut env = Env::machine(prog, self, Some(cur));
                    let set = env.eval(e)?;
                    set_elems(
                        &set,
                        e.span,
                        &format!("the domain of parameter `{}`", p.name),
                    )?
                }
                ADom::Type(t) => {
                    let n = t.name().unwrap_or("");
                    match self.sets.get(n) {
                        Some(s) => set_elems(s, p.span, "a domain")?,
                        None => {
                            return err(
                                p.span,
                                format!("parameter `{}`: type {t} has no finite domain; the model needs `domain {t} = lo .. hi` or `values {t} = {{…}}`", p.name),
                            )
                        }
                    }
                }
            };
            out.push(vals);
        }
        Ok(out)
    }

    /// All successors of one action from `cur`, over every parameter combination.
    pub fn successors(
        &self,
        prog: &Program,
        a: &ActionDecl,
        cur: &[Value],
    ) -> Result<Vec<Succ>, Diag> {
        let domains = self.param_values(prog, a, cur)?;
        let mut combos: Vec<Vec<(String, Value)>> = vec![vec![]];
        for (p, vals) in a.params.iter().zip(&domains) {
            let mut next = Vec::new();
            for c in &combos {
                for v in vals {
                    let mut c2 = c.clone();
                    c2.push((p.name.clone(), v.clone()));
                    next.push(c2);
                }
            }
            combos = next;
        }
        let mut out = Vec::new();
        for params in combos {
            for s in self.step(prog, a, &params, cur)? {
                out.push(Succ {
                    params: params.clone(),
                    next: s,
                });
            }
        }
        Ok(out)
    }

    /// Successor states of one action with fixed parameter values.
    pub fn step(
        &self,
        prog: &Program,
        a: &ActionDecl,
        params: &[(String, Value)],
        cur: &[Value],
    ) -> Result<Vec<State>, Diag> {
        let branches = self.exec(prog, &a.body, cur, params.to_vec())?;
        Ok(branches.iter().map(|b| self.apply(cur, b)).collect())
    }

    fn exec(
        &self,
        prog: &Program,
        body: &[Stmt],
        cur: &[Value],
        locals: Vec<(String, Value)>,
    ) -> Result<Vec<Branch>, Diag> {
        let mut branches = vec![Branch {
            next: vec![None; self.vars.len()],
            locals,
        }];
        for st in body {
            let mut out = Vec::new();
            for b in branches {
                self.exec_stmt(prog, st, cur, b, &mut out)?;
            }
            branches = out;
            if branches.is_empty() {
                break;
            }
        }
        Ok(branches)
    }

    fn exec_stmt(
        &self,
        prog: &Program,
        st: &Stmt,
        cur: &[Value],
        b: Branch,
        out: &mut Vec<Branch>,
    ) -> Result<(), Diag> {
        let eval = |b: &Branch, e: &Expr| -> Result<Value, Diag> {
            let mut env = Env::machine(prog, self, Some(cur));
            env.next = Some(&b.next);
            env.locals = b.locals.clone();
            env.eval(e)
        };
        match &st.kind {
            StmtKind::When(e) => match eval(&b, e)? {
                Value::Bool(true) => out.push(b),
                Value::Bool(false) => {}
                Value::Unknown(r) => return err(e.span, format!("guard is unknown: {r}")),
                v => {
                    return err(
                        e.span,
                        format!("a guard must be a Bool, found {} {v}", v.type_name()),
                    )
                }
            },
            StmtKind::Assign(x, e) => {
                let Some(&i) = self.var_index.get(x) else {
                    let what = if self.consts.contains_key(x) {
                        "a const, which can't be assigned"
                    } else {
                        "not a state variable"
                    };
                    return err(st.span, format!("`{x}` is {what}"));
                };
                if b.next[i].is_some() {
                    return err(st.span, format!("`{x}'` is assigned twice"));
                }
                let v = eval(&b, e)?;
                let mut b = b;
                b.next[i] = Some(v);
                out.push(b);
            }
            StmtKind::Let(x, e) => {
                let v = eval(&b, e)?;
                let mut b = b;
                b.locals.push((x.clone(), v));
                out.push(b);
            }
            StmtKind::Pick(x, e) => {
                let s = eval(&b, e)?;
                for v in set_elems(&s, e.span, "`pick`")? {
                    let mut b2 = b.clone();
                    b2.locals.push((x.clone(), v));
                    out.push(b2);
                }
            }
            StmtKind::If(c, then, els) => {
                let body = match eval(&b, c)? {
                    Value::Bool(true) => then,
                    Value::Bool(false) => els,
                    v => {
                        return err(
                            c.span,
                            format!("an `if` condition must be a Bool, found {v}"),
                        )
                    }
                };
                let mut bs = vec![b];
                for s2 in body {
                    let mut o2 = Vec::new();
                    for bb in bs {
                        self.exec_stmt(prog, s2, cur, bb, &mut o2)?;
                    }
                    bs = o2;
                }
                out.extend(bs);
            }
            StmtKind::Either(alts) => {
                for alt in alts {
                    let mut bs = vec![b.clone()];
                    for s2 in alt {
                        let mut o2 = Vec::new();
                        for bb in bs {
                            self.exec_stmt(prog, s2, cur, bb, &mut o2)?;
                        }
                        bs = o2;
                    }
                    out.extend(bs);
                }
            }
        }
        Ok(())
    }

    /// Checks every variable's value against its declared type; returns the first misfit.
    pub fn type_error(&self, prog: &Program, s: &[Value]) -> Option<String> {
        for (v, x) in self.vars.iter().zip(s) {
            if !admits(prog, self.pack, &v.ty, x, v.optional) {
                return Some(format!("`{}` = {x} is not of type {}", v.name, v.ty));
            }
        }
        None
    }

    /// `x = 1, y = "a"` for a state, or only the variables that differ from `prev`.
    pub fn show(&self, s: &[Value], prev: Option<&[Value]>) -> String {
        let mut parts = Vec::new();
        for (i, v) in self.vars.iter().enumerate() {
            if prev.is_none_or(|p| p[i] != s[i]) {
                parts.push(format!("{} = {}", v.name, s[i]));
            }
        }
        if parts.is_empty() {
            "(no change)".into()
        } else {
            parts.join(", ")
        }
    }
}

/// Does `v` fit the declared type? Unknown type names (entities, ids) admit anything.
pub fn admits(prog: &Program, pack: usize, ty: &TypeExpr, v: &Value, optional: bool) -> bool {
    if optional && matches!(v, Value::None) {
        return true;
    }
    if let Some(d) = type_dim(ty) {
        let int_only = ty.name() == Some("Int");
        return matches!(v, Value::Num(n, vd) if *vd == d && (!int_only || n.as_int().is_some()));
    }
    let TypeExpr::Named(n, args, _) = ty else {
        return true;
    };
    match (n.as_str(), v) {
        ("Bool", Value::Bool(_)) | ("String", Value::Str(_)) => true,
        ("Bool" | "String", _) => false,
        ("List", Value::List(xs)) => args
            .first()
            .is_none_or(|t| xs.iter().all(|x| admits(prog, pack, t, x, false))),
        ("Set", Value::Set(xs)) => args
            .first()
            .is_none_or(|t| xs.iter().all(|x| admits(prog, pack, t, x, false))),
        ("Map", Value::Map(m)) => {
            args.len() != 2
                || m.iter().all(|(k, x)| {
                    admits(prog, pack, &args[0], k, false) && admits(prog, pack, &args[1], x, false)
                })
        }
        ("List" | "Set" | "Map", _) => false,
        _ => match prog.lookup_decl(pack, n).map(|r| prog.item(r)) {
            Some(Item::Enum(e)) => {
                matches!(v, Value::Str(s) if e.members.iter().any(|m| *m.0 == **s))
            }
            _ => true,
        },
    }
}
