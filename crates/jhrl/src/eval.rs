//! Expression evaluator (docs/jhrl.md sections 5, 7 and 8).
//!
//! - Logic is strong Kleene: `unknown` comes from missing values (it names the path) and from
//!   inputs v0 doesn't have (graph collections, measures, `now`). Inside machines every value is
//!   concrete; a guard or invariant that comes out unknown is an error there.
//! - Quantities carry dimensions. `+`, `-` and comparisons need equal dimensions, `*` and `/`
//!   combine them, and a user `fn` checks its arguments and result against its declared types.
//! - Not in v0: intervals and tolerances (`±`, `range`, `tol`), graph navigation (`out`, `in`),
//!   measures, relations, non-exact table axes. They evaluate to an error or to `unknown`.

use crate::ast::*;
use crate::machine::MachineCtx;
use crate::num::{Dim, Num, DIM_RANGE};
use crate::program::{DeclRef, NameKind, Program};
use crate::resolve::{self, Bound, Candidate, Explanation, Q};
use crate::span::{Diag, Span};
use crate::value::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

pub type R = Result<Value, Diag>;

const MAX_DEPTH: u32 = 64;

/// Quantifier and comprehension iterations plus function calls one evaluation may perform.
const MAX_STEPS: u64 = 10_000_000;

/// The dimension a numeric type stands for (`Credits/Duration` → credits·time⁻¹).
pub fn type_dim(t: &TypeExpr) -> Option<Dim> {
    match t {
        TypeExpr::Named(n, args, _) if args.is_empty() => Dim::of_type_name(n),
        TypeExpr::Div(a, b) => type_dim(a)?.div(type_dim(b)?),
        _ => None,
    }
}

pub struct Env<'a> {
    pub prog: &'a Program,
    pub pack: usize,
    pub locals: Vec<(String, Value)>,
    pub m: Option<&'a MachineCtx>,
    pub cur: Option<&'a [Value]>,
    pub next: Option<&'a [Option<Value>]>,
    pub depth: u32,
    /// Work done so far, counted against `MAX_STEPS`. A sub-environment starts from its parent's
    /// count and hands it back (see `eval_in`), so nested calls share one budget.
    pub steps: u64,
}

fn err<T>(span: Span, msg: impl Into<String>) -> Result<T, Diag> {
    Err(Diag::error(span, msg))
}

fn as_bool(v: &Value, span: Span, what: &str) -> Result<Option<bool>, Diag> {
    match v {
        Value::Bool(b) => Ok(Some(*b)),
        Value::Unknown(_) => Ok(None),
        _ => err(
            span,
            format!("{what} must be a Bool, found {} {v}", v.type_name()),
        ),
    }
}

fn first_unknown(vs: &[&Value]) -> Option<Value> {
    vs.iter()
        .find(|v| matches!(v, Value::Unknown(_)))
        .map(|v| (*v).clone())
}

fn num_op(span: Span, r: Result<Num, String>) -> Result<Num, Diag> {
    r.map_err(|e| Diag::error(span, e))
}

/// Elements of a collection for a generator; `(k, v)` tuples for a map when destructuring.
fn elements(v: &Value, tuple: bool, span: Span) -> Result<Vec<Value>, Diag> {
    Ok(match v {
        Value::List(xs) | Value::Tuple(xs) => xs.to_vec(),
        Value::Set(xs) => xs.iter().cloned().collect(),
        Value::Map(m) if tuple => m
            .iter()
            .map(|(k, v)| Value::tuple(vec![k.clone(), v.clone()]))
            .collect(),
        Value::Map(m) => m.keys().cloned().collect(),
        _ => return err(span, format!("can't range over {} {v}", v.type_name())),
    })
}

/// Whether `s` matches the pattern `p` (`*` any run, `?` any one character). Iterative, with one backtrack point:
/// the subject is often agent text, so neither its length nor the pattern's stars may cost stack or exponential time.
fn glob(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    // The last `*` seen: the pattern position after it, and how much of the subject it has taken.
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi + 1, si));
                pi += 1;
            }
            Some('?') => {
                pi += 1;
                si += 1;
            }
            Some(c) if *c == s[si] => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some((after, taken)) => {
                    star = Some((after, taken + 1));
                    pi = after;
                    si = taken + 1;
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

impl<'a> Env<'a> {
    pub fn new(prog: &'a Program, pack: usize) -> Env<'a> {
        Env {
            prog,
            pack,
            locals: vec![],
            m: None,
            cur: None,
            next: None,
            depth: 0,
            steps: 0,
        }
    }

    pub fn machine(prog: &'a Program, m: &'a MachineCtx, cur: Option<&'a [Value]>) -> Env<'a> {
        Env {
            prog,
            pack: m.pack,
            locals: vec![],
            m: Some(m),
            cur,
            next: None,
            depth: 0,
            steps: 0,
        }
    }

    fn snippet(&self, span: Span) -> String {
        self.prog.sources.snippet(span).to_string()
    }

    /// Counts one unit of work against `MAX_STEPS`.
    fn tick(&mut self, span: Span) -> Result<(), Diag> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return err(span, "evaluation exceeded its step budget");
        }
        Ok(())
    }

    pub fn eval(&mut self, e: &Expr) -> R {
        let span = e.span;
        match &e.kind {
            ExprKind::Num(n, d) => Ok(Value::Num(*n, *d)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::NoneLit => Ok(Value::None),
            ExprKind::Name(n) => self.name(n, span),
            ExprKind::Primed(n) => self.primed(n, span),
            ExprKind::Unary(UnOp::Neg, x) => {
                let v = self.eval(x)?;
                match v {
                    Value::Num(n, d) => Ok(Value::Num(num_op(span, n.neg())?, d)),
                    Value::Unknown(_) => Ok(v),
                    _ => err(span, format!("can't negate {} {v}", v.type_name())),
                }
            }
            ExprKind::Unary(UnOp::Not, x) => {
                let v = self.eval(x)?;
                Ok(match as_bool(&v, x.span, "the operand of `not`")? {
                    Some(b) => Value::Bool(!b),
                    None => v,
                })
            }
            ExprKind::Unary(UnOp::Always | UnOp::Eventually, _)
            | ExprKind::Binary(BinOp::LeadsTo | BinOp::Until, _, _) => err(
                span,
                "temporal operators belong in a `property` (liveness checking is not in v0)",
            ),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, span),
            ExprKind::If(c, a, b) => {
                let cv = self.eval(c)?;
                match as_bool(&cv, c.span, "an `if` condition")? {
                    Some(true) => self.eval(a),
                    Some(false) => self.eval(b),
                    None => Ok(cv),
                }
            }
            ExprKind::Call(f, args) => self.call(f, args, span),
            ExprKind::Field(x, f) => {
                let v = self.eval(x)?;
                self.field(v, f, span)
            }
            ExprKind::Index(x, i) => {
                let v = self.eval(x)?;
                let k = self.eval(i)?;
                self.index(v, k, span)
            }
            ExprKind::List(xs) => Ok(Value::list(
                xs.iter().map(|x| self.eval(x)).collect::<Result<_, _>>()?,
            )),
            ExprKind::Tuple(xs) => Ok(Value::tuple(
                xs.iter().map(|x| self.eval(x)).collect::<Result<_, _>>()?,
            )),
            ExprKind::Set(xs) => Ok(Value::set(
                xs.iter()
                    .map(|x| self.eval(x))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            ExprKind::Map(kv) => {
                let mut m = BTreeMap::new();
                for (k, v) in kv {
                    let k = self.eval(k)?;
                    let v = self.eval(v)?;
                    m.insert(k, v);
                }
                Ok(Value::map(m))
            }
            ExprKind::Record(fs) => {
                let mut m = BTreeMap::new();
                for (k, v) in fs {
                    let v = self.eval(v)?;
                    m.insert(k.clone(), v);
                }
                Ok(Value::record(m))
            }
            ExprKind::Comp {
                kind,
                gens,
                filter,
                key,
                val,
            } => self.comprehension(*kind, gens, filter.as_deref(), key, val.as_deref(), span),
            ExprKind::Quant(q, gens, body) => self.quantifier(*q, gens, body),
            ExprKind::With(x, path, v) => {
                let base = self.eval(x)?;
                let mut keys = Vec::new();
                for seg in path {
                    keys.push(match seg {
                        PathSeg::Index(k) => Some(self.eval(k)?),
                        PathSeg::Field(_) => None,
                    });
                }
                let nv = self.eval(v)?;
                self.update(base, path, &keys, nv, span)
            }
            ExprKind::Is(x, t) => {
                let v = self.eval(x)?;
                Ok(match (&v, t.as_str()) {
                    (Value::Unknown(_), _) => v.clone(),
                    (Value::Bool(_), "Bool") => Value::Bool(true),
                    (Value::Str(_), "String") => Value::Bool(true),
                    (Value::Num(n, d), "Int") => Value::Bool(d.is_none() && n.as_int().is_some()),
                    (Value::Num(_, d), t) if Dim::of_type_name(t).is_some() => {
                        Value::Bool(Dim::of_type_name(t) == Some(*d))
                    }
                    (_, "Bool" | "String" | "Int" | "Number") => Value::Bool(false),
                    _ => Value::Unknown(format!(
                        "{} is {t} (entity types need a graph snapshot)",
                        self.snippet(x.span)
                    )),
                })
            }
        }
    }

    /// A check body (`for … where … let … require …`): true when every instance's `require`
    /// holds, false when one certainly fails, unknown otherwise. Other clauses are metadata.
    pub fn eval_clauses(&mut self, clauses: &[Clause]) -> Result<Value, Diag> {
        let Some((c, rest)) = clauses.split_first() else {
            return Ok(Value::Bool(true));
        };
        match c {
            Clause::For(gens) => {
                let (mut any_false, mut unk) = (false, None);
                let u = self.each(gens, &mut |env| {
                    match env.eval_clauses(rest)? {
                        Value::Bool(false) => any_false = true,
                        Value::Bool(true) => {}
                        v => unk = Some(v),
                    }
                    Ok(true)
                })?;
                Ok(if any_false {
                    Value::Bool(false)
                } else if let Some(r) = u {
                    Value::Unknown(r)
                } else {
                    unk.unwrap_or(Value::Bool(true))
                })
            }
            Clause::Where(e) => {
                if self.eval_bool(e, "a `where` filter")? == Some(true) {
                    self.eval_clauses(rest)
                } else {
                    Ok(Value::Bool(true))
                }
            }
            Clause::Let(n, e) => {
                let v = self.eval(e)?;
                self.locals.push((n.clone(), v));
                let r = self.eval_clauses(rest);
                self.locals.pop();
                r
            }
            Clause::Require(e, _) => {
                let v = self.eval(e)?;
                as_bool(&v, e.span, "`require`")?;
                match v {
                    Value::Bool(true) => self.eval_clauses(rest),
                    _ => Ok(v),
                }
            }
            _ => self.eval_clauses(rest),
        }
    }

    /// `base with [k].f = v`: replaces the value at the path, copying only what changes.
    fn update(
        &self,
        base: Value,
        path: &[PathSeg],
        keys: &[Option<Value>],
        nv: Value,
        span: Span,
    ) -> R {
        let Some((seg, rest)) = path.split_first() else {
            return Ok(nv);
        };
        let inner = |this: &Self, old: Option<Value>| -> R {
            if rest.is_empty() {
                return Ok(nv.clone());
            }
            match old {
                Some(o) => this.update(o, rest, &keys[1..], nv.clone(), span),
                None => err(span, "`with` path goes through a missing key"),
            }
        };
        match (base, seg) {
            (Value::Map(mut m), PathSeg::Index(_)) => {
                let k = keys[0].clone().unwrap();
                let v = inner(self, m.get(&k).cloned())?;
                Arc::make_mut(&mut m).insert(k, v);
                Ok(Value::Map(m))
            }
            (Value::List(mut l), PathSeg::Index(_)) => {
                let i = self.list_index(keys[0].as_ref().unwrap(), l.len(), span)?;
                let v = inner(self, Some(l[i].clone()))?;
                Arc::make_mut(&mut l)[i] = v;
                Ok(Value::List(l))
            }
            (Value::Record(mut r), PathSeg::Field(f)) => {
                let v = inner(self, r.get(f).cloned())?;
                Arc::make_mut(&mut r).insert(f.clone(), v);
                Ok(Value::Record(r))
            }
            (u @ Value::Unknown(_), _) => Ok(u),
            (b, _) => err(span, format!("can't update {} {b} this way", b.type_name())),
        }
    }

    pub fn eval_bool(&mut self, e: &Expr, what: &str) -> Result<Option<bool>, Diag> {
        let v = self.eval(e)?;
        as_bool(&v, e.span, what)
    }

    fn name(&mut self, n: &str, span: Span) -> R {
        if let Some((_, v)) = self.locals.iter().rev().find(|(k, _)| k == n) {
            return Ok(v.clone());
        }
        if let Some(m) = self.m {
            if let Some(&i) = m.var_index.get(n) {
                return match self.cur {
                    Some(s) if i < s.len() => Ok(s[i].clone()),
                    _ => err(span, format!("state variable `{n}` has no value here")),
                };
            }
            if let Some(v) = m.consts.get(n) {
                return Ok(v.clone());
            }
            if let Some((params, body)) = m.defs.get(n) {
                if !params.is_empty() {
                    return err(span, format!("`{n}` takes {} argument(s)", params.len()));
                }
                self.tick(span)?;
                return self.def(m, body, vec![], span);
            }
            if let Some(v) = m.model_values.get(n) {
                return Ok(v.clone());
            }
            if let Some(v) = m.sets.get(n) {
                return Ok(v.clone());
            }
        }
        match self.prog.lookup(self.pack, n) {
            Err(e) => return err(span, e),
            Ok(Some((NameKind::Collection, _))) => {
                return Ok(Value::Unknown(format!(
                    "collection `{n}` (needs a graph snapshot)"
                )));
            }
            Ok(Some((NameKind::Decl, r))) => return self.decl_value(r, n, span),
            Ok(None) => {}
        }
        match n {
            "Bool" => Ok(Value::set([Value::Bool(false), Value::Bool(true)])),
            "now" => Ok(Value::Unknown("now (an input; not set)".into())),
            _ if Dim::of_type_name(n).is_some() => err(
                span,
                format!("type `{n}` has no finite domain here; a model gives one with `domain {n} = lo .. hi`"),
            ),
            _ => err(span, format!("unknown name `{n}`")),
        }
    }

    fn decl_value(&mut self, r: DeclRef, n: &str, span: Span) -> R {
        let prog = self.prog;
        let pack = prog.pack_of(r.file);
        match prog.item(r) {
            Item::Let(_, e, _) => self.eval_in(pack, e),
            Item::Param(p, _) => match &p.default {
                Some(e) => self.eval_in(pack, e),
                None => Ok(Value::Unknown(format!("param `{n}` (not set)"))),
            },
            Item::Enum(d) => Ok(Value::set(d.members.iter().map(|m| Value::str(&m.0)))),
            Item::Fn(_) | Item::Table(_) | Item::Kind(_) | Item::Measure(_) => {
                err(span, format!("`{n}` is a function; call it with arguments"))
            }
            Item::Rel(_) => Ok(Value::Unknown(format!(
                "relation `{n}` (relations need a graph snapshot)"
            ))),
            _ => err(span, format!("`{n}` is not a value")),
        }
    }

    /// Evaluates a machine-level definition in the current state (its own locals only).
    fn def(
        &mut self,
        m: &'a MachineCtx,
        body: &Expr,
        locals: Vec<(String, Value)>,
        span: Span,
    ) -> R {
        if self.depth > MAX_DEPTH {
            return err(
                span,
                "definition depth exceeded (definitions may not recurse)",
            );
        }
        let mut env = Env {
            prog: self.prog,
            pack: m.pack,
            locals,
            m: Some(m),
            cur: self.cur,
            next: self.next,
            depth: self.depth + 1,
            steps: self.steps,
        };
        let r = env.eval(body);
        self.steps = env.steps;
        r
    }

    /// A fresh environment for evaluating a declaration from another pack.
    fn sub(&self, pack: usize) -> Env<'a> {
        Env {
            prog: self.prog,
            pack,
            locals: vec![],
            m: None,
            cur: None,
            next: None,
            depth: self.depth + 1,
            steps: self.steps,
        }
    }

    /// Evaluates `e` in a fresh environment for `pack`, then carries the step count back.
    fn eval_in(&mut self, pack: usize, e: &Expr) -> R {
        let mut env = self.sub(pack);
        let r = env.eval(e);
        self.steps = env.steps;
        r
    }

    fn primed(&mut self, n: &str, span: Span) -> R {
        let Some(m) = self.m else {
            return err(span, format!("`{n}'` outside a machine"));
        };
        let Some(&i) = m.var_index.get(n) else {
            return err(span, format!("`{n}` is not a state variable"));
        };
        match self.next {
            Some(nx) => match &nx[i] {
                Some(v) => Ok(v.clone()),
                None => err(span, format!("`{n}'` is read before it is assigned")),
            },
            None => err(
                span,
                format!("`{n}'` can only be used in actions and transition invariants"),
            ),
        }
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, span: Span) -> R {
        match op {
            BinOp::And | BinOp::Or | BinOp::Implies => {
                let av = self.eval(a)?;
                let ab = as_bool(&av, a.span, &format!("the left side of `{}`", op.text()))?;
                // short circuit
                match (op, ab) {
                    (BinOp::And, Some(false)) => return Ok(Value::Bool(false)),
                    (BinOp::Or, Some(true)) => return Ok(Value::Bool(true)),
                    (BinOp::Implies, Some(false)) => return Ok(Value::Bool(true)),
                    _ => {}
                }
                let bv = self.eval(b)?;
                let bb = as_bool(&bv, b.span, &format!("the right side of `{}`", op.text()))?;
                let unk = || first_unknown(&[&av, &bv]).unwrap();
                Ok(match (op, ab, bb) {
                    (BinOp::And, Some(true), Some(x)) => Value::Bool(x),
                    (BinOp::And, _, Some(false)) => Value::Bool(false),
                    (BinOp::Or, Some(false), Some(x)) => Value::Bool(x),
                    (BinOp::Or, _, Some(true)) => Value::Bool(true),
                    (BinOp::Implies, Some(true), Some(x)) => Value::Bool(x),
                    (BinOp::Implies, _, Some(true)) => Value::Bool(true),
                    _ => unk(),
                })
            }
            BinOp::Coalesce => {
                let av = self.eval(a)?;
                match av {
                    Value::Unknown(_) | Value::None => self.eval(b),
                    _ => Ok(av),
                }
            }
            _ => {
                let av = self.eval(a)?;
                let bv = self.eval(b)?;
                binop(op, av, bv, span)
            }
        }
    }

    fn list_index(&self, k: &Value, len: usize, span: Span) -> Result<usize, Diag> {
        match k {
            Value::Num(n, d) if d.is_none() => match n.as_int() {
                // A value that doesn't fit a usize is out of range, not a wrapped index.
                Some(i) => match usize::try_from(i) {
                    Ok(idx) if idx < len => Ok(idx),
                    _ => err(
                        span,
                        format!(
                            "index {i} is out of range for a list of length {len} (indexes start at 0)"
                        ),
                    ),
                },
                None => err(span, format!("index {n} is not an integer")),
            },
            _ => err(span, format!("a list index must be an Int, found {k}")),
        }
    }

    fn index(&mut self, v: Value, k: Value, span: Span) -> R {
        if let Some(u) = first_unknown(&[&v, &k]) {
            return Ok(u);
        }
        match v {
            Value::List(xs) | Value::Tuple(xs) => {
                let i = self.list_index(&k, xs.len(), span)?;
                Ok(xs[i].clone())
            }
            Value::Map(m) => Ok(m
                .get(&k)
                .cloned()
                .unwrap_or_else(|| Value::Unknown(format!("{}[{k}]", self.snippet(span))))),
            _ => err(span, format!("can't index {} {v}", v.type_name())),
        }
    }

    fn field(&mut self, v: Value, f: &str, span: Span) -> R {
        match v {
            Value::Record(m) => Ok(m
                .get(f)
                .cloned()
                .unwrap_or_else(|| Value::Unknown(self.snippet(span)))),
            Value::Unknown(_) => Ok(v),
            Value::None => Ok(Value::Unknown(format!("{} (on none)", self.snippet(span)))),
            _ => err(span, format!("{} {v} has no field `{f}`", v.type_name())),
        }
    }

    /// Runs `body` once per binding of the generators, until it returns `false`. Returns the
    /// reason if a source was unknown.
    fn each(
        &mut self,
        gens: &[Gen],
        body: &mut dyn FnMut(&mut Env<'a>) -> Result<bool, Diag>,
    ) -> Result<Option<String>, Diag> {
        let mut stop = false;
        self.each_inner(gens, body, &mut stop)
    }

    fn each_inner(
        &mut self,
        gens: &[Gen],
        body: &mut dyn FnMut(&mut Env<'a>) -> Result<bool, Diag>,
        stop: &mut bool,
    ) -> Result<Option<String>, Diag> {
        let Some((g, rest)) = gens.split_first() else {
            if !body(self)? {
                *stop = true;
            }
            return Ok(None);
        };
        let src = self.eval(&g.src)?;
        if let Value::Unknown(r) = src {
            return Ok(Some(r));
        }
        let mut unknown = None;
        for x in elements(&src, g.tuple, g.src.span)? {
            self.tick(g.src.span)?;
            let n0 = self.locals.len();
            if g.tuple {
                let parts = match &x {
                    Value::Tuple(p) | Value::List(p) if p.len() == g.names.len() => p.to_vec(),
                    _ => {
                        return err(
                            g.span,
                            format!("can't destructure {x} into {} names", g.names.len()),
                        )
                    }
                };
                for (n, p) in g.names.iter().zip(parts) {
                    self.locals.push((n.clone(), p));
                }
            } else {
                self.locals.push((g.names[0].clone(), x));
            }
            let r = self.each_inner(rest, body, stop);
            self.locals.truncate(n0);
            if let Some(u) = r? {
                unknown = Some(u);
            }
            if *stop {
                break;
            }
        }
        Ok(unknown)
    }

    fn comprehension(
        &mut self,
        kind: CompKind,
        gens: &[Gen],
        filter: Option<&Expr>,
        key: &Expr,
        val: Option<&Expr>,
        _span: Span,
    ) -> R {
        let mut list = Vec::new();
        let mut map = BTreeMap::new();
        let unknown = self.each(gens, &mut |env| {
            if let Some(f) = filter {
                if env.eval_bool(f, "a `where` filter")? != Some(true) {
                    return Ok(true); // kept only when certainly true
                }
            }
            let k = env.eval(key)?;
            match val {
                Some(v) => {
                    let v = env.eval(v)?;
                    map.insert(k, v);
                }
                None => list.push(k),
            }
            Ok(true)
        })?;
        if let Some(r) = unknown {
            return Ok(Value::Unknown(r));
        }
        Ok(match kind {
            CompKind::List => Value::list(list),
            CompKind::Set => Value::set(list),
            CompKind::Map => Value::map(map),
        })
    }

    fn quantifier(&mut self, q: Quant, gens: &[Gen], body: &Expr) -> R {
        let (mut t, mut f, mut u) = (0usize, 0usize, None::<Value>);
        let unknown = self.each(gens, &mut |env| {
            let v = env.eval(body)?;
            match as_bool(&v, body.span, "a quantifier body")? {
                Some(true) => t += 1,
                Some(false) => f += 1,
                None => u = Some(v),
            }
            // Stop as soon as the answer is certain.
            Ok(!matches!(
                (q, t, f),
                (Quant::All, _, 1..) | (Quant::Any, 1.., _) | (Quant::One, 2.., _)
            ))
        })?;
        if let Some(r) = unknown {
            return Ok(Value::Unknown(r));
        }
        Ok(match q {
            Quant::All if f > 0 => Value::Bool(false),
            Quant::Any if t > 0 => Value::Bool(true),
            Quant::One if t > 1 => Value::Bool(false),
            _ if u.is_some() => u.unwrap(),
            Quant::All => Value::Bool(true),
            Quant::Any => Value::Bool(false),
            Quant::One => Value::Bool(t == 1),
        })
    }

    // ------------------------------------------------------------------ calls
    fn call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> R {
        self.tick(span)?;
        let name = match &callee.kind {
            ExprKind::Name(n) => n.clone(),
            ExprKind::Field(obj, method) => {
                let o = self.eval(obj)?;
                return match (&o, method.as_str()) {
                    (Value::Unknown(_), _) => Ok(o),
                    (_, "out" | "in") => Ok(Value::Unknown(format!(
                        "{} (graph navigation needs a snapshot)",
                        self.snippet(span)
                    ))),
                    _ => err(span, format!("{} has no method `{method}`", o.type_name())),
                };
            }
            _ => return err(callee.span, "only named functions can be called"),
        };
        if self.locals.iter().any(|(k, _)| *k == name) {
            return err(
                callee.span,
                format!("`{name}` is a local value, not a function"),
            );
        }
        if let Some(m) = self.m {
            if let Some((params, body)) = m.defs.get(&name) {
                if params.len() != args.len() {
                    return err(
                        span,
                        format!(
                            "`{name}` takes {} argument(s), got {}",
                            params.len(),
                            args.len()
                        ),
                    );
                }
                let mut locals = Vec::with_capacity(params.len());
                for (p, a) in params.iter().zip(args) {
                    locals.push((p.clone(), self.eval(&a.value)?));
                }
                return self.def(m, body, locals, span);
            }
        }
        if self.depth > MAX_DEPTH {
            return err(span, "call depth exceeded (functions may not recurse)");
        }
        match self.prog.lookup(self.pack, &name) {
            Err(e) => return err(callee.span, e),
            Ok(Some((NameKind::Decl, r))) => {
                let pack = self.prog.pack_of(r.file);
                return match self.prog.item(r) {
                    Item::Fn(d) => self.call_fn(d, pack, args, span),
                    Item::Table(d) => self.call_table(d, pack, args, span),
                    Item::Kind(d) => {
                        let vals = self.positional(args, &name)?;
                        let ex = resolve_kind(self.prog, d, pack, vals, span)?;
                        Ok(kind_value(&ex, d))
                    }
                    Item::Measure(_) => {
                        for a in args {
                            self.eval(&a.value)?;
                        }
                        Ok(Value::Unknown(format!(
                            "measure `{name}` (computed by the engine from a graph snapshot)"
                        )))
                    }
                    _ => err(callee.span, format!("`{name}` is not callable")),
                };
            }
            Ok(Some((NameKind::Collection, _))) => {
                return err(
                    callee.span,
                    format!("`{name}` is a collection, not a function"),
                )
            }
            Ok(None) => {}
        }
        let vals = self.positional(args, &name)?;
        builtin(&name, vals, span, self)
    }

    fn positional(&mut self, args: &[Arg], name: &str) -> Result<Vec<Value>, Diag> {
        let mut v = Vec::new();
        for a in args {
            if a.name.is_some() {
                return err(a.value.span, format!("`{name}` takes no named arguments"));
            }
            v.push(self.eval(&a.value)?);
        }
        Ok(v)
    }

    /// Binds call arguments (positional, then named, then defaults) to parameters.
    fn bind_params(
        &mut self,
        params: &[ParamDef],
        pack: usize,
        args: &[Arg],
        fname: &str,
        span: Span,
    ) -> Result<Vec<(String, Value)>, Diag> {
        let mut slots: Vec<Option<Value>> = vec![None; params.len()];
        let mut pos = 0;
        for a in args {
            let i = match &a.name {
                None => {
                    pos += 1;
                    pos - 1
                }
                Some(n) => match params.iter().position(|p| p.name == *n) {
                    Some(i) => i,
                    None => return err(a.value.span, format!("`{fname}` has no parameter `{n}`")),
                },
            };
            if i >= params.len() {
                return err(
                    span,
                    format!(
                        "`{fname}` takes {} argument(s), got {}",
                        params.len(),
                        args.len()
                    ),
                );
            }
            slots[i] = Some(self.eval(&a.value)?);
        }
        let mut out = Vec::new();
        for (p, s) in params.iter().zip(slots) {
            let v = match s {
                Some(v) => v,
                None => match &p.default {
                    Some(d) => self.eval_in(pack, d)?,
                    None => {
                        return err(span, format!("`{fname}` is missing argument `{}`", p.name))
                    }
                },
            };
            if let (Some(d), Value::Num(_, vd)) = (type_dim(&p.ty), &v) {
                if d != *vd {
                    return err(
                        span,
                        format!(
                            "argument `{}` of `{fname}` is {}, got {v} ({})",
                            p.name,
                            d.describe(),
                            vd.describe()
                        ),
                    );
                }
            }
            out.push((p.name.clone(), v));
        }
        Ok(out)
    }

    fn call_fn(&mut self, d: &FnDecl, pack: usize, args: &[Arg], span: Span) -> R {
        let locals = self.bind_params(&d.params, pack, args, &d.name, span)?;
        let mut env = self.sub(pack);
        env.locals = locals;
        let v = env.eval(&d.body);
        self.steps = env.steps;
        let v = v?;
        if let (Some(rd), Value::Num(_, vd)) = (type_dim(&d.ret), &v) {
            if rd != *vd {
                return err(
                    d.body.span,
                    format!(
                        "`{}` returns {}, but its body gives {v} ({})",
                        d.name,
                        rd.describe(),
                        vd.describe()
                    ),
                );
            }
        }
        Ok(v)
    }

    fn call_table(&mut self, d: &TableDecl, pack: usize, args: &[Arg], span: Span) -> R {
        let vals: Vec<Value> = self
            .bind_params(&d.params, pack, args, &d.name, span)?
            .into_iter()
            .map(|x| x.1)
            .collect();
        if let Some(u) = first_unknown(&vals.iter().collect::<Vec<_>>()) {
            return Ok(u);
        }
        for (keys, out) in &d.rows {
            let mut all = keys.len() == vals.len();
            for (k, v) in keys.iter().zip(&vals) {
                if self.eval_in(pack, k)? != *v {
                    all = false;
                    break;
                }
            }
            if all {
                return self.eval_in(pack, out);
            }
        }
        let shown: Vec<String> = vals.iter().map(|v| v.to_string()).collect();
        match d.out_of_range.as_deref() {
            None | Some("error") => err(
                span,
                format!("table `{}` has no row for ({})", d.name, shown.join(", ")),
            ),
            Some(k) => err(
                span,
                format!(
                    "table `{}`: out_of_range {k} and non-exact axes are not in v0",
                    d.name
                ),
            ),
        }
    }
}

/// The value a resolved kind gives an expression: a point, or ±∞ when the limit is open.
pub fn kind_value(ex: &Explanation, d: &KindDecl) -> Value {
    let dim = type_dim(&d.ret).unwrap_or(Dim::NONE);
    match ex.value {
        Some(b) => match b.point() {
            Some(q) => Value::Num(q.0, q.1),
            None => Value::Unknown(format!(
                "{} is the window {b} (intervals are not in v0)",
                d.name
            )),
        },
        None => {
            let inf = if d.direction == Direction::Min {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            Value::Num(Num::Real(inf), dim)
        }
    }
}

/// Resolves a kind for the given objects across every loaded constraint rule (section 10).
pub fn resolve_kind(
    prog: &Program,
    d: &KindDecl,
    kind_pack: usize,
    args: Vec<Value>,
    span: Span,
) -> Result<Explanation, Diag> {
    if args.len() != d.params.len() {
        return err(
            span,
            format!(
                "kind `{}` takes {} argument(s), got {}",
                d.name,
                d.params.len(),
                args.len()
            ),
        );
    }
    let dim = type_dim(&d.ret).unwrap_or(Dim::NONE);
    let to_bound = |v: Value, at: Span, what: &str| -> Result<Bound, Diag> {
        match v {
            Value::Num(n, vd) => Ok(Bound::Point(Q(n, vd))),
            other => err(
                at,
                format!(
                    "{what} of kind `{}` must be a number, found {other}",
                    d.name
                ),
            ),
        }
    };
    let default = match &d.default {
        Some(e) => Some(to_bound(
            Env::new(prog, kind_pack).eval(e)?,
            e.span,
            "the default",
        )?),
        None => None,
    };
    let me = prog.lookup_decl(kind_pack, &d.name);
    let mut cands = Vec::new();
    for (pack, _, it) in prog.items() {
        let Item::Constraint(c) = it else { continue };
        if c.kind != d.name || prog.lookup_decl(pack, &c.kind) != me {
            continue;
        }
        if c.args.len() != args.len() {
            return err(
                c.span,
                format!(
                    "rule for `{}` binds {} name(s); the kind has {}",
                    d.name,
                    c.args.len(),
                    args.len()
                ),
            );
        }
        let mut env = Env::new(prog, pack);
        env.locals = c.args.iter().cloned().zip(args.iter().cloned()).collect();
        let mut applies = true;
        for w in &c.wheres {
            if env.eval_bool(w, "a rule's `where`")? != Some(true) {
                applies = false;
                break;
            }
        }
        if !applies {
            continue;
        }
        let Some(ve) = &c.value else {
            return err(c.span, "a constraint rule needs a `value`");
        };
        let value = to_bound(env.eval(ve)?, ve.span, "the value")?;
        cands.push(Candidate {
            rule: c
                .title
                .clone()
                .unwrap_or_else(|| format!("{} rule at {}", c.kind, prog.locate(c.span))),
            pack: prog.packs[pack].name.clone(),
            tier: prog.packs[pack].tier,
            priority: c.priority,
            floor: c.floor,
            value,
        });
    }
    resolve::resolve(&d.name, d.direction, dim, default, cands)
        .map_err(|e| Diag::error(span, e.to_string()))
}

fn same_dim(op: BinOp, a: Dim, b: Dim, span: Span) -> Result<(), Diag> {
    if a != b {
        return err(
            span,
            format!(
                "dimension mismatch: {} {} {}",
                a.describe(),
                op.text(),
                b.describe()
            ),
        );
    }
    Ok(())
}

pub fn binop(op: BinOp, a: Value, b: Value, span: Span) -> R {
    use BinOp::*;
    if let Some(u) = first_unknown(&[&a, &b]) {
        return Ok(u);
    }
    match op {
        Eq | Ne => {
            if let (Value::Num(x, da), Value::Num(y, db)) = (&a, &b) {
                same_dim(op, *da, *db, span)?;
                // A NaN is neither equal nor unequal to anything: comparing it is an error.
                x.try_cmp(*y).map_err(|e| Diag::error(span, e))?;
            }
            let eq = a == b;
            Ok(Value::Bool(if op == Eq { eq } else { !eq }))
        }
        Lt | Le | Gt | Ge => {
            let ord = match (&a, &b) {
                (Value::Num(x, da), Value::Num(y, db)) => {
                    same_dim(op, *da, *db, span)?;
                    x.try_cmp(*y).map_err(|e| Diag::error(span, e))?
                }
                (Value::Str(x), Value::Str(y)) => x.cmp(y),
                _ => {
                    return err(
                        span,
                        format!(
                            "can't compare {} {a} {} {} {b}",
                            a.type_name(),
                            op.text(),
                            b.type_name()
                        ),
                    )
                }
            };
            use std::cmp::Ordering::*;
            Ok(Value::Bool(match op {
                Lt => ord == Less,
                Le => ord != Greater,
                Gt => ord == Greater,
                _ => ord != Less,
            }))
        }
        In => Ok(Value::Bool(match &b {
            Value::List(xs) | Value::Tuple(xs) => xs.contains(&a),
            Value::Set(xs) => xs.contains(&a),
            Value::Map(m) => m.contains_key(&a),
            Value::Str(s) => match &a {
                Value::Str(x) => s.contains(&**x),
                _ => return err(span, "`in` a string needs a string"),
            },
            _ => {
                return err(
                    span,
                    format!(
                        "`in` needs a collection on the right, found {} {b}",
                        b.type_name()
                    ),
                )
            }
        })),
        Tol => err(span, "tolerances (`±`) are not in v0"),
        Range => match (&a, &b) {
            (Value::Num(x, da), Value::Num(y, db)) => {
                same_dim(op, *da, *db, span)?;
                let (Some(lo), Some(hi)) = (x.as_int(), y.as_int()) else {
                    return err(
                        span,
                        format!("a range needs whole base units, found {a} .. {b}"),
                    );
                };
                if hi.checked_sub(lo).is_none_or(|w| w > 1_000_000) {
                    return err(span, "a range of more than a million values");
                }
                Ok(Value::set((lo..=hi).map(|i| Value::Num(Num::int(i), *da))))
            }
            _ => err(span, format!("`..` needs two numbers, found {a} and {b}")),
        },
        Add | Sub => match (a, b) {
            (Value::Num(x, da), Value::Num(y, db)) => {
                same_dim(op, da, db, span)?;
                Ok(Value::Num(
                    num_op(span, if op == Add { x.add(y) } else { x.sub(y) })?,
                    da,
                ))
            }
            (Value::Set(mut x), Value::Set(y)) => {
                let xm = Arc::make_mut(&mut x);
                if op == Add {
                    xm.extend(y.iter().cloned());
                } else {
                    xm.retain(|e| !y.contains(e));
                }
                Ok(Value::Set(x))
            }
            (Value::List(mut x), Value::List(y)) if op == Add => {
                Arc::make_mut(&mut x).extend(y.iter().cloned());
                Ok(Value::List(x))
            }
            (Value::Str(x), Value::Str(y)) if op == Add => Ok(Value::str(&format!("{x}{y}"))),
            (a, b) => err(
                span,
                format!(
                    "can't {} {} {a} and {} {b}",
                    if op == Add { "add" } else { "subtract" },
                    a.type_name(),
                    b.type_name()
                ),
            ),
        },
        Union | Minus | Inter => match (a, b) {
            (Value::Set(x), Value::Set(y)) => Ok(Value::Set(match op {
                Union => Arc::new(x.union(&y).cloned().collect()),
                Minus => Arc::new(x.difference(&y).cloned().collect()),
                _ => Arc::new(x.intersection(&y).cloned().collect()),
            })),
            (a, b) => err(
                span,
                format!("`{}` needs two sets, found {a} and {b}", op.text()),
            ),
        },
        Mul | Div => match (a, b) {
            (Value::Num(x, da), Value::Num(y, db)) => {
                let (n, d) = if op == Mul {
                    (x.mul(y), da.mul(db))
                } else {
                    (x.div(y), da.div(db))
                };
                match d {
                    Some(d) => Ok(Value::Num(num_op(span, n)?, d)),
                    None => err(span, DIM_RANGE),
                }
            }
            (a, b) => err(
                span,
                format!(
                    "can't {} {a} and {b}",
                    if op == Mul { "multiply" } else { "divide" }
                ),
            ),
        },
        Pow => match (a, b) {
            (Value::Num(x, da), Value::Num(y, db)) => {
                if !db.is_none() {
                    return err(
                        span,
                        format!("an exponent must be dimensionless, found {}", db.describe()),
                    );
                }
                match y.as_int() {
                    Some(k) => match da.pow(k) {
                        Some(d) => Ok(Value::Num(num_op(span, x.pow(y))?, d)),
                        None => err(span, DIM_RANGE),
                    },
                    None if da.is_none() => Ok(Value::Num(num_op(span, x.pow(y))?, Dim::NONE)),
                    None => err(
                        span,
                        format!("a {} can only be raised to an integer power", da.describe()),
                    ),
                }
            }
            (a, b) => err(span, format!("can't raise {a} to {b}")),
        },
        _ => err(span, format!("operator `{}` not handled", op.text())),
    }
}

fn want_num(v: &Value, f: &str, span: Span) -> Result<(Num, Dim), Diag> {
    match v {
        Value::Num(n, d) => Ok((*n, *d)),
        _ => err(
            span,
            format!("`{f}` needs a number, found {} {v}", v.type_name()),
        ),
    }
}

fn arity(f: &str, vals: &[Value], n: usize, span: Span) -> Result<(), Diag> {
    if vals.len() != n {
        return err(
            span,
            format!("`{f}` takes {n} argument(s), got {}", vals.len()),
        );
    }
    Ok(())
}

pub const BUILTINS: &[&str] = &[
    "len",
    "count",
    "sum",
    "min",
    "max",
    "abs",
    "ln",
    "floor",
    "ceil",
    "round_to",
    "clamp",
    "certainly",
    "possibly",
    "matches",
    "starts_with",
    "ends_with",
    "lower",
    "first",
    "last",
    "keys",
    "values",
    "any",
    "all",
    "to_credits",
    "to_money",
    "range",
    "tol",
    "sort_by",
    "take",
];

fn builtin(f: &str, vals: Vec<Value>, span: Span, _env: &mut Env) -> R {
    // certainly/possibly look at unknowns; everything else propagates them.
    match f {
        "certainly" | "possibly" => {
            arity(f, &vals, 1, span)?;
            return Ok(match &vals[0] {
                Value::Bool(b) => Value::Bool(*b),
                Value::Unknown(_) => Value::Bool(f == "possibly"),
                v => return err(span, format!("`{f}` needs a Bool, found {v}")),
            });
        }
        _ => {}
    }
    if let Some(u) = first_unknown(&vals.iter().collect::<Vec<_>>()) {
        if BUILTINS.contains(&f) {
            return Ok(u);
        }
    }
    let coll = |v: &Value| -> Result<Vec<Value>, Diag> { elements(v, false, span) };
    match f {
        "len" | "count" => {
            arity(f, &vals, 1, span)?;
            let n = match &vals[0] {
                Value::Str(s) if f == "len" => s.chars().count(),
                Value::Map(m) => m.len(),
                v => coll(v)?.len(),
            };
            Ok(Value::int(n as i128))
        }
        "sum" => {
            arity(f, &vals, 1, span)?;
            let mut acc: Option<(Num, Dim)> = None;
            for x in coll(&vals[0])? {
                if let Value::Unknown(_) = x {
                    return Ok(x);
                }
                let (n, d) = want_num(&x, f, span)?;
                acc = Some(match acc {
                    None => (n, d),
                    Some((a, ad)) => {
                        same_dim(BinOp::Add, ad, d, span)?;
                        (num_op(span, a.add(n))?, ad)
                    }
                });
            }
            let (n, d) = acc.unwrap_or((Num::int(0), Dim::NONE));
            Ok(Value::Num(n, d))
        }
        "min" | "max" => {
            let items = if vals.len() == 1 {
                coll(&vals[0])?
            } else {
                vals
            };
            if items.is_empty() {
                return err(span, format!("`{f}` of nothing"));
            }
            let mut best = items[0].clone();
            for x in &items[1..] {
                let less = binop(BinOp::Lt, x.clone(), best.clone(), span)?;
                let take = matches!(less, Value::Bool(true)) == (f == "min");
                let eq = x == &best;
                if take && !eq {
                    best = x.clone();
                }
            }
            if let Value::Num(..) = best {
                Ok(best)
            } else if let Value::Str(_) = best {
                Ok(best)
            } else {
                err(span, format!("`{f}` needs numbers"))
            }
        }
        "abs" | "floor" | "ceil" => {
            arity(f, &vals, 1, span)?;
            let (n, d) = want_num(&vals[0], f, span)?;
            let r = match f {
                "abs" => n.abs(),
                "floor" => n.floor(),
                _ => n.ceil(),
            };
            Ok(Value::Num(num_op(span, r)?, d))
        }
        "ln" => {
            arity(f, &vals, 1, span)?;
            let (n, d) = want_num(&vals[0], f, span)?;
            if !d.is_none() {
                return err(
                    span,
                    format!("`ln` needs a dimensionless number, found {}", d.describe()),
                );
            }
            if n.to_f64() <= 0.0 {
                return err(span, format!("ln({n}) is undefined"));
            }
            Ok(Value::Num(
                num_op(span, Num::finite(n.to_f64().ln()))?,
                Dim::NONE,
            ))
        }
        "round_to" => {
            arity(f, &vals, 2, span)?;
            let (x, d) = want_num(&vals[0], f, span)?;
            let (s, ds) = want_num(&vals[1], f, span)?;
            same_dim(BinOp::Div, d, ds, span)?;
            if s.try_cmp(Num::int(0)).map_err(|e| Diag::error(span, e))?
                != std::cmp::Ordering::Greater
            {
                return err(span, "round_to needs a positive step");
            }
            let k = num_op(span, x.div(s).and_then(|q| q.round()))?;
            // k is an integer; k * step stays exact when the step is a rational.
            Ok(Value::Num(num_op(span, k.mul(s))?, d))
        }
        "clamp" => {
            arity(f, &vals, 3, span)?;
            let lo_ok = binop(BinOp::Lt, vals[0].clone(), vals[1].clone(), span)?;
            if lo_ok == Value::Bool(true) {
                return Ok(vals[1].clone());
            }
            let hi_ok = binop(BinOp::Gt, vals[0].clone(), vals[2].clone(), span)?;
            Ok(if hi_ok == Value::Bool(true) {
                vals[2].clone()
            } else {
                vals[0].clone()
            })
        }
        "matches" | "starts_with" | "ends_with" => {
            arity(f, &vals, 2, span)?;
            let (Value::Str(s), Value::Str(p)) = (&vals[0], &vals[1]) else {
                return err(span, format!("`{f}` needs two strings"));
            };
            Ok(Value::Bool(match f {
                "matches" => glob(
                    &p.chars().collect::<Vec<_>>(),
                    &s.chars().collect::<Vec<_>>(),
                ),
                "starts_with" => s.starts_with(&**p),
                _ => s.ends_with(&**p),
            }))
        }
        "lower" => {
            arity(f, &vals, 1, span)?;
            match &vals[0] {
                Value::Str(s) => Ok(Value::str(&s.to_lowercase())),
                v => err(span, format!("`lower` needs a string, found {v}")),
            }
        }
        "first" | "last" => {
            arity(f, &vals, 1, span)?;
            let xs = coll(&vals[0])?;
            let x = if f == "first" { xs.first() } else { xs.last() };
            Ok(x.cloned()
                .unwrap_or_else(|| Value::Unknown(format!("{f} of an empty collection"))))
        }
        "keys" | "values" => {
            arity(f, &vals, 1, span)?;
            match &vals[0] {
                Value::Map(m) if f == "keys" => Ok(Value::set(m.keys().cloned())),
                Value::Map(m) => Ok(Value::list(m.values().cloned().collect())),
                v => err(span, format!("`{f}` needs a map, found {v}")),
            }
        }
        "any" | "all" => {
            arity(f, &vals, 1, span)?;
            let mut unk = None;
            for x in coll(&vals[0])? {
                match as_bool(&x, span, f)? {
                    Some(true) if f == "any" => return Ok(Value::Bool(true)),
                    Some(false) if f == "all" => return Ok(Value::Bool(false)),
                    None => unk = Some(x),
                    _ => {}
                }
            }
            Ok(unk.unwrap_or(Value::Bool(f == "all")))
        }
        "to_credits" | "to_money" => {
            arity(f, &vals, 2, span)?;
            let (x, d) = want_num(&vals[0], f, span)?;
            let (peg, dp) = want_num(&vals[1], f, span)?;
            same_dim(BinOp::Div, dp, Dim::MONEY, span)?;
            if f == "to_credits" {
                same_dim(BinOp::Div, d, Dim::MONEY, span)?;
                Ok(Value::Num(num_op(span, x.div(peg))?, Dim::CREDITS))
            } else {
                same_dim(BinOp::Div, d, Dim::CREDITS, span)?;
                Ok(Value::Num(num_op(span, x.mul(peg))?, Dim::MONEY))
            }
        }
        "take" => {
            arity(f, &vals, 2, span)?;
            let n = match &vals[1] {
                Value::Num(n, d) if d.is_none() && n.as_int().is_some_and(|i| i >= 0) => {
                    match n.as_int().map(usize::try_from) {
                        Some(Ok(k)) => k,
                        _ => return err(span, format!("`take` count {n} is out of range")),
                    }
                }
                v => return err(span, format!("`take(xs, n)` needs a count, found {v}")),
            };
            match &vals[0] {
                Value::List(xs) => Ok(Value::list(xs.iter().take(n).cloned().collect())),
                v => err(span, format!("`take` needs a list, found {v}")),
            }
        }
        "range" | "tol" | "sort_by" => err(
            span,
            format!("`{f}` is not in v0 (intervals and sorting are deferred)"),
        ),
        _ => err(span, format!("unknown function `{f}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr_at;

    #[test]
    fn glob_matches_like_before_and_a_long_text_needs_no_stack() {
        let g = |p: &str, s: &str| {
            glob(
                &p.chars().collect::<Vec<_>>(),
                &s.chars().collect::<Vec<_>>(),
            )
        };
        for (p, s, want) in [
            ("", "", true),
            ("", "a", false),
            ("*", "", true),
            ("a*", "a", true),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("*.rs", "src/main.rs", true),
            ("*.rs", "src/main.rsx", false),
            ("a*b*c", "axxbyyc", true),
            ("a*b*c", "axxbyy", false),
            ("**x", "abx", true),
        ] {
            assert_eq!(g(p, s), want, "{p:?} on {s:?}");
        }
        // A megabyte of agent text: the recursive matcher overflowed a thread's stack here.
        let long = format!("{}b", "a".repeat(1 << 20));
        assert!(g("*a*b", &long));
        assert!(!g("*c", &long));
        assert!(g("a?a*", &long));
    }

    fn prog(src: &str) -> Program {
        let (p, d) = Program::load(vec![("t.jhrl".into(), src.into())]);
        assert!(d.is_empty(), "{d:?}");
        p
    }

    fn ev(p: &Program, s: &str) -> R {
        // Put the expression's text in the source map so `unknown` can name its path.
        let mut p = p.clone();
        let f = p.sources.add("expr", s);
        let e = parse_expr_at(f, s, 0).unwrap();
        Env::new(&p, 0).eval(&e)
    }

    fn ok(p: &Program, s: &str) -> Value {
        ev(p, s).unwrap_or_else(|d| panic!("{s}: {}", d.msg))
    }

    #[test]
    fn expressions_evaluate_to_expected_values() {
        let p = prog("");
        // arithmetic_and_dimensions: arithmetic, dimension checks, and the numeric builtins.
        {
            assert_eq!(ok(&p, "1000 cr/d * 1 d == 1000 cr"), Value::Bool(true));
            assert_eq!(ok(&p, "16 KiB == 16384 B"), Value::Bool(true));
            assert_eq!(ok(&p, "30 /min * 2 min"), Value::int(60));
            assert_eq!(
                ok(&p, "2 ^ -2"),
                Value::Num(Num::rat(1, 4).unwrap(), Dim::NONE)
            );
            assert_eq!(
                ok(&p, "(3 s) ^ 2"),
                Value::Num(Num::int(9), Dim::TIME.pow(2).unwrap())
            );
            assert!(ev(&p, "1 cr + 1 s")
                .unwrap_err()
                .msg
                .contains("dimension mismatch: Credits + Duration"));
            assert!(ev(&p, "1 cr <= 1000")
                .unwrap_err()
                .msg
                .contains("dimension mismatch"));
            assert!(ev(&p, "ln(2 cr)").is_err());
            assert!(ev(&p, "1 / 0").is_err());
            assert_eq!(
                ok(&p, "round_to(2 / 3, 0.01)"),
                Value::Num(Num::rat(67, 100).unwrap(), Dim::NONE)
            );
            assert_eq!(ok(&p, "floor(7 / 2)"), Value::int(3));
            assert_eq!(
                ok(&p, "max(1 cr, 3 cr, 2 cr)"),
                Value::Num(Num::int(3), Dim::CREDITS)
            );
            assert_eq!(ok(&p, "min([4, 2, 9])"), Value::int(2));
            assert_eq!(ok(&p, "abs(-3 s)"), Value::Num(Num::int(3), Dim::TIME));
            assert_eq!(
                ok(&p, "sum([1 cr, 2 cr])"),
                Value::Num(Num::int(3), Dim::CREDITS)
            );
            assert_eq!(ok(&p, "clamp(12, 0, 10)"), Value::int(10));
        }
        // collections_comprehensions_quantifiers: collections, comprehensions, and quantifiers.
        {
            assert_eq!(ok(&p, "len([1, 2, 3])"), Value::int(3));
            assert_eq!(ok(&p, "count({1, 1, 2})"), Value::int(2));
            assert_eq!(
                ok(&p, "[for x in [1, 2, 3] where x > 1 yield x * 10]").to_string(),
                "[20, 30]"
            );
            assert_eq!(
                ok(&p, "{for x in [1, 2] yield x: x * x}").to_string(),
                "{1: 1, 2: 4}"
            );
            assert_eq!(ok(&p, "all x in {1, 2}: x > 0"), Value::Bool(true));
            assert_eq!(ok(&p, "any x in {1, 2}: x > 1"), Value::Bool(true));
            assert_eq!(ok(&p, "one x in {1, 2}: x > 0"), Value::Bool(false));
            assert_eq!(ok(&p, "2 in {1, 2}"), Value::Bool(true));
            assert_eq!(ok(&p, "({1, 2} + {3}) - {1}").to_string(), "{2, 3}");
            assert_eq!(ok(&p, "{1, 2} inter {2, 3}").to_string(), "{2}");
            assert_eq!(
                ok(&p, "({\"a\": 1} with [\"b\"] = 2)[\"b\"]"),
                Value::int(2)
            );
            assert_eq!(
                ok(&p, "({used: 1 cr} with .used = 2 cr).used"),
                Value::Num(Num::int(2), Dim::CREDITS)
            );
            assert_eq!(
                ok(&p, "[\"orchestrator\", \"owner\"][1]"),
                Value::str("owner")
            );
            assert!(ev(&p, "[1][1]").unwrap_err().msg.contains("out of range"));
            assert_eq!(
                ok(&p, "[for (a, b) in [(1, 2), (3, 4)] yield a + b]").to_string(),
                "[3, 7]"
            );
            assert_eq!(ok(&p, "if 1 < 2 then \"y\" else \"n\""), Value::str("y"));
            assert_eq!(
                ok(&p, "matches(\"agent-7\", \"agent-*\")"),
                Value::Bool(true)
            );
        }
        // kleene_logic_and_missing_values: three-valued logic and missing record fields.
        {
            // A missing record field is unknown and names its path.
            match ok(&p, "{a: 1}.b") {
                Value::Unknown(r) => assert_eq!(r, "{a: 1}.b"),
                v => panic!("{v}"),
            }
            assert_eq!(ok(&p, "{a: 1}.b > 0 and false"), Value::Bool(false));
            assert_eq!(ok(&p, "{a: 1}.b > 0 or true"), Value::Bool(true));
            assert!(matches!(ok(&p, "{a: 1}.b > 0 and true"), Value::Unknown(_)));
            assert!(matches!(ok(&p, "not ({a: 1}.b > 0)"), Value::Unknown(_)));
            assert_eq!(ok(&p, "certainly({a: 1}.b > 0)"), Value::Bool(false));
            assert_eq!(ok(&p, "possibly({a: 1}.b > 0)"), Value::Bool(true));
            assert_eq!(ok(&p, "{a: 1}.b ?? 7"), Value::int(7));
            assert_eq!(ok(&p, "false => {a: 1}.b > 0"), Value::Bool(true));
            assert_eq!(
                ok(&p, "[for x in [1, 2] where {a: 1}.b > x yield x]").to_string(),
                "[]"
            );
            assert!(matches!(ok(&p, "now - 1 s"), Value::Unknown(_)));
        }
    }

    #[test]
    fn arithmetic_indexes_nan_and_the_step_budget_have_limits() {
        let p = prog("");
        let big = Value::int(i128::MAX);
        // i128::MAX + 1 is an error, not a wrap.
        assert!(binop(BinOp::Add, big.clone(), Value::int(1), Span::default()).is_err());
        // A huge value is not a list index.
        assert!(Env::new(&p, 0)
            .list_index(&big, 3, Span::default())
            .is_err());
        // A NaN has no order, so comparing it is an error.
        let nan = Value::Num(Num::Real(f64::NAN), Dim::NONE);
        assert!(binop(BinOp::Lt, nan.clone(), Value::int(1), Span::default()).is_err());
        assert!(binop(BinOp::Eq, nan, Value::int(1), Span::default()).is_err());
        // Two nested quantifiers over 0..100000 stop at the step budget instead of running on.
        let msg = ev(&p, "all x in 0..100000: all y in 0..100000: true")
            .unwrap_err()
            .msg;
        assert!(msg.contains("step budget"), "{msg}");
        // A unit raised past the range of its exponent is an error, not a wrapped dimension.
        let secs = Value::Num(Num::int(2), Dim::TIME);
        let msg = binop(BinOp::Pow, secs, Value::int(200), Span::default())
            .unwrap_err()
            .msg;
        assert!(msg.contains("out of range"), "{msg}");
    }

    #[test]
    fn functions_tables_and_their_dimension_checks() {
        let p = prog(
            "fn w(x: Credits) -> Number = 1 + ln(1 + x / 100 cr)\n\
             fn bad(x: Credits) -> Credits = x / 1 s\n\
             fn d(x: Number, dt: Duration, h: Duration) -> Number = x * 2 ^ (-(dt / h))\n\
             table t(k: String) -> Number {\n axes k: exact\n out_of_range error\n \"a\" => 1\n \"b\" => -0.5\n}",
        );
        assert_eq!(
            ok(&p, "round_to(w(400 cr), 0.01) == 2.61"),
            Value::Bool(true)
        );
        assert!(ev(&p, "w(4 s)")
            .unwrap_err()
            .msg
            .contains("argument `x` of `w` is Credits"));
        assert!(ev(&p, "bad(1 cr)")
            .unwrap_err()
            .msg
            .contains("returns Credits"));
        assert_eq!(
            ok(&p, "d(1, 180 d, 90 d)"),
            Value::Num(Num::rat(1, 4).unwrap(), Dim::NONE)
        );
        assert_eq!(
            ok(&p, "t(\"b\")"),
            Value::Num(Num::rat(-1, 2).unwrap(), Dim::NONE)
        );
        assert!(ev(&p, "t(\"zzz\")").unwrap_err().msg.contains("no row"));
    }

    #[test]
    fn kinds_resolve_through_rules_and_tiers() {
        let (mut p, d) = Program::load(vec![
            ("k.jhrl".into(), "pack k\nkind contacts_max(a: Agent) -> Int max default 150 \"\"".into()),
            (
                "s.jhrl".into(),
                "pack sys\nimport k\nconstraint \"Fleet contact limit\" contacts_max(a) { value 150 }".into(),
            ),
            (
                "t.jhrl".into(),
                "pack project
import k\nconstraint \"Research projects keep fewer contacts\" contacts_max(a) {\n where a.project.template == \"research\"\n value 60\n priority 10\n}\nconstraint \"Loose\" contacts_max(a) { where a.project.template == \"open\"\n value 500 }"
                    .into(),
            ),
        ]);
        assert!(
            d.iter().all(|x| !x.msg.contains("unknown pack `k`")),
            "{d:?}"
        );
        assert!(p.set_tier("project", crate::resolve::Tier::Plaza));
        let kp = p.pack_by_name("k").unwrap();
        let tp = p.pack_by_name("project").unwrap();
        let research = ok_in(&p, tp, "{project: {template: \"research\"}}");
        let open = ok_in(&p, tp, "{project: {template: \"open\"}}");
        let Item::Kind(kd) = p.item(p.lookup_decl(kp, "contacts_max").unwrap()).clone() else {
            panic!()
        };
        let ex = resolve_kind(&p, &kd, kp, vec![research], Span::default()).unwrap();
        assert_eq!(kind_value(&ex, &kd), Value::int(60));
        assert_eq!(
            ex.decided_by,
            vec!["Research projects keep fewer contacts".to_string()]
        );
        let ex = resolve_kind(&p, &kd, kp, vec![open], Span::default()).unwrap();
        assert_eq!(kind_value(&ex, &kd), Value::int(150));
        assert_eq!(ex.no_effect, vec!["Loose".to_string()]);
        // A missing project makes the where unknown, so the project rule doesn't apply.
        let nobody = ok_in(&p, tp, "{name: \"x\"}");
        let ex = resolve_kind(&p, &kd, kp, vec![nobody], Span::default()).unwrap();
        assert_eq!(kind_value(&ex, &kd), Value::int(150));
        let _ = &mut p;
    }

    fn ok_in(p: &Program, pack: usize, s: &str) -> Value {
        let e = parse_expr_at(0, s, 0).unwrap();
        Env::new(p, pack).eval(&e).unwrap()
    }
}
