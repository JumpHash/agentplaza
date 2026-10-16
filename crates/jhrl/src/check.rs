//! Static checks (AP-074.W2, the v0 part): every name resolves, types named in declarations
//! exist, and dimensions agree wherever they can be known without data. `transfer.amount <=
//! 1000` is reported here, as is a `fn` whose body's dimension differs from its declared result,
//! a string compared with an enum that has no such member, and a recursive `fn`.
//!
//! The checker is deliberately permissive where v0 has no information (graph data, measure
//! results): such expressions get the type `Any`, which agrees with everything.

use crate::ast::*;
use crate::eval::BUILTINS;
use crate::num::Dim;
use crate::program::{DeclRef, NameKind, Program};
use crate::span::{Diag, Span};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Any,
    Bool,
    Num(Dim),
    Str,
    Enum(DeclRef, String),
    Instant,
    None,
    /// An entity or edge type.
    Node(DeclRef, String),
    List(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Record(Vec<(String, Ty)>),
    Tuple(Vec<Ty>),
    /// Prelude types of the graph measures.
    Graph,
    Cycle,
    Id,
    ModelValue,
}

impl Ty {
    fn show(&self) -> String {
        match self {
            Ty::Any => "any".into(),
            Ty::Bool => "Bool".into(),
            Ty::Num(d) => d.describe(),
            Ty::Str => "String".into(),
            Ty::Enum(_, n) | Ty::Node(_, n) => n.clone(),
            Ty::Instant => "Instant".into(),
            Ty::None => "none".into(),
            Ty::List(t) => format!("List<{}>", t.show()),
            Ty::Set(t) => format!("Set<{}>", t.show()),
            Ty::Map(k, v) => format!("Map<{}, {}>", k.show(), v.show()),
            Ty::Record(_) => "record".into(),
            Ty::Tuple(_) => "tuple".into(),
            Ty::Graph => "Graph".into(),
            Ty::Cycle => "Cycle".into(),
            Ty::Id => "Id".into(),
            Ty::ModelValue => "model value".into(),
        }
    }
    fn elem(&self) -> Ty {
        match self {
            Ty::List(t) | Ty::Set(t) => (**t).clone(),
            Ty::Map(k, _) => (**k).clone(),
            _ => Ty::Any,
        }
    }
    fn is_textual(&self) -> bool {
        matches!(self, Ty::Str | Ty::Enum(..))
    }
}

/// Machine-level names while checking a machine and its models.
#[derive(Default, Clone)]
struct MScope {
    consts: HashMap<String, Ty>,
    vars: HashMap<String, Ty>,
    actions: HashSet<String>,
    /// Model `values T` names and their members.
    model_types: HashSet<String>,
    model_values: HashSet<String>,
    /// Machine-level definitions and their parameter counts.
    defs: HashMap<String, usize>,
    primes: bool,
}

pub struct Checker<'a> {
    prog: &'a Program,
    pack: usize,
    scopes: Vec<(String, Ty)>,
    m: Option<MScope>,
    pub diags: Vec<Diag>,
}

/// Checks every item of every loaded file. Returns errors (and warnings) in source order.
pub fn check_program(prog: &Program) -> Vec<Diag> {
    let mut c = Checker {
        prog,
        pack: 0,
        scopes: vec![],
        m: None,
        diags: vec![],
    };
    for (pack, r, it) in prog.items() {
        c.pack = pack;
        c.scopes.clear();
        c.m = None;
        c.item(r, it);
    }
    c.recursion();
    let mut d = c.diags;
    d.extend(crate::authz::check_authz(prog));
    d
}

impl<'a> Checker<'a> {
    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diag::error(span, msg));
    }

    // ------------------------------------------------------------------ types
    fn ty_of(&mut self, t: &TypeExpr) -> Ty {
        match t {
            TypeExpr::Div(a, b) => match (self.ty_of(a), self.ty_of(b)) {
                (Ty::Num(x), Ty::Num(y)) => Ty::Num(x.div(y)),
                (Ty::Any, _) | (_, Ty::Any) => Ty::Any,
                _ => {
                    self.err(t.span(), format!("`{t}`: only numeric types divide"));
                    Ty::Any
                }
            },
            TypeExpr::Record(fs) => {
                Ty::Record(fs.iter().map(|(n, t)| (n.clone(), self.ty_of(t))).collect())
            }
            TypeExpr::Named(n, args, span) => {
                let arg =
                    |c: &mut Checker, i: usize| args.get(i).map(|a| c.ty_of(a)).unwrap_or(Ty::Any);
                if let Some(d) = Dim::of_type_name(n) {
                    return Ty::Num(d);
                }
                match n.as_str() {
                    "Bool" => Ty::Bool,
                    "String" => Ty::Str,
                    "Instant" => Ty::Instant,
                    "Graph" => Ty::Graph,
                    "Cycle" => Ty::Cycle,
                    "Id" => {
                        for a in args {
                            self.ty_of(a);
                        }
                        Ty::Id
                    }
                    "List" => Ty::List(Box::new(arg(self, 0))),
                    "Set" => Ty::Set(Box::new(arg(self, 0))),
                    "Map" => Ty::Map(Box::new(arg(self, 0)), Box::new(arg(self, 1))),
                    _ => {
                        if self.m.as_ref().is_some_and(|m| m.model_types.contains(n)) {
                            return Ty::ModelValue;
                        }
                        match self.prog.lookup(self.pack, n) {
                            Ok(Some((NameKind::Decl, r))) => match self.prog.item(r) {
                                Item::Enum(e) => Ty::Enum(r, e.name.clone()),
                                Item::Entity(e) => Ty::Node(r, e.name.clone()),
                                Item::Edge(e) => Ty::Node(r, e.name.clone()),
                                _ => {
                                    self.err(*span, format!("`{n}` is not a type"));
                                    Ty::Any
                                }
                            },
                            Err(e) => {
                                self.err(*span, e);
                                Ty::Any
                            }
                            _ => {
                                self.err(*span, format!("unknown type `{n}`"));
                                Ty::Any
                            }
                        }
                    }
                }
            }
        }
    }

    /// Reports when a value of type `got` (from expression `e`) can't be used where `want` is.
    fn expect(&mut self, want: &Ty, got: &Ty, e: &Expr, what: &str) {
        if let Some(msg) = self.mismatch(want, got, e) {
            self.err(e.span, format!("{what}: {msg}"));
        }
    }

    fn mismatch(&self, want: &Ty, got: &Ty, e: &Expr) -> Option<String> {
        match (want, got) {
            (Ty::Any, _) | (_, Ty::Any) | (_, Ty::None) => None,
            (Ty::Num(a), Ty::Num(b)) if a != b => {
                Some(format!("expected {}, found {}", a.describe(), b.describe()))
            }
            (Ty::Num(_), Ty::Num(_)) => None,
            (Ty::Enum(r, n), _) => self.enum_literal(*r, n, e).or_else(|| {
                (!got.is_textual()).then(|| format!("expected {n}, found {}", got.show()))
            }),
            (Ty::Str, g) if g.is_textual() => None,
            (Ty::Bool, Ty::Bool) | (Ty::Instant, Ty::Instant) => None,
            (Ty::List(a), Ty::List(b)) | (Ty::Set(a), Ty::Set(b)) => {
                // element-wise, for literals check each element
                match &e.kind {
                    ExprKind::List(xs) | ExprKind::Set(xs) => {
                        xs.iter().find_map(|x| self.mismatch(a, &self.quick(x), x))
                    }
                    _ => self.mismatch(a, b, e),
                }
            }
            (Ty::Map(..), Ty::Map(..))
            | (Ty::Record(_), Ty::Record(_))
            | (Ty::Tuple(_), Ty::Tuple(_)) => None,
            (Ty::Node(_, a), Ty::Node(_, b)) if a == b => None,
            (Ty::Id | Ty::ModelValue | Ty::Graph | Ty::Cycle, _) => None,
            (Ty::Node(..), Ty::Record(_)) => None,
            (w, g) if w == g => None,
            (w, g) => Some(format!("expected {}, found {}", w.show(), g.show())),
        }
    }

    /// A cheap type for literals inside a collection literal (no diagnostics).
    fn quick(&self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Num(_, d) => Ty::Num(*d),
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::List(_) => Ty::List(Box::new(Ty::Any)),
            ExprKind::Set(_) => Ty::Set(Box::new(Ty::Any)),
            _ => Ty::Any,
        }
    }

    /// A string literal (or each branch of an `if` of them) compared with an enum must be a member.
    fn enum_literal(&self, r: DeclRef, n: &str, e: &Expr) -> Option<String> {
        let Item::Enum(d) = self.prog.item(r) else {
            return None;
        };
        match &e.kind {
            ExprKind::Str(s) if !d.members.iter().any(|m| *m.0 == **s) => Some(format!(
                "\"{s}\" is not a value of enum {n} ({})",
                d.members
                    .iter()
                    .map(|m| format!("\"{}\"", m.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            ExprKind::If(_, a, b) => self
                .enum_literal(r, n, a)
                .or_else(|| self.enum_literal(r, n, b)),
            ExprKind::Set(xs) | ExprKind::List(xs) => {
                xs.iter().find_map(|x| self.enum_literal(r, n, x))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ names
    fn bind(&mut self, n: &str, t: Ty) {
        self.scopes.push((n.to_string(), t));
    }

    fn name(&mut self, n: &str, span: Span) -> Ty {
        if let Some((_, t)) = self.scopes.iter().rev().find(|(k, _)| k == n) {
            return t.clone();
        }
        if let Some(m) = &self.m {
            if let Some(t) = m.vars.get(n).or_else(|| m.consts.get(n)) {
                return t.clone();
            }
            if m.model_values.contains(n) {
                return Ty::ModelValue;
            }
            if let Some(&k) = m.defs.get(n) {
                if k > 0 {
                    self.err(span, format!("`{n}` takes {k} argument(s)"));
                }
                return Ty::Any;
            }
            if m.model_types.contains(n) {
                return Ty::Set(Box::new(Ty::ModelValue));
            }
            if let Some(d) = Dim::of_type_name(n) {
                return Ty::Set(Box::new(Ty::Num(d))); // a model domain
            }
        }
        match self.prog.lookup(self.pack, n) {
            Err(e) => {
                self.err(span, e);
                return Ty::Any;
            }
            Ok(Some((NameKind::Collection, r))) => {
                let name = match self.prog.item(r) {
                    Item::Entity(e) => e.name.clone(),
                    Item::Edge(e) => e.name.clone(),
                    _ => String::new(),
                };
                return Ty::List(Box::new(Ty::Node(r, name)));
            }
            Ok(Some((NameKind::Decl, r))) => {
                return match self.prog.item(r) {
                    Item::Enum(e) => Ty::Set(Box::new(Ty::Enum(r, e.name.clone()))),
                    Item::Param(p, _) => {
                        let saved = self.pack;
                        self.pack = self.prog.pack_of(r.file);
                        let t = self.ty_of(&p.ty);
                        self.pack = saved;
                        t
                    }
                    Item::Let(..) | Item::Rel(_) => Ty::Any,
                    Item::Fn(_) | Item::Table(_) | Item::Kind(_) | Item::Measure(_) => {
                        self.err(span, format!("`{n}` is a function; call it with arguments"));
                        Ty::Any
                    }
                    _ => {
                        self.err(span, format!("`{n}` is not a value"));
                        Ty::Any
                    }
                };
            }
            Ok(None) => {}
        }
        match n {
            "Bool" => Ty::Set(Box::new(Ty::Bool)),
            "now" => Ty::Instant,
            _ => {
                self.err(span, format!("unknown name `{n}`"));
                Ty::Any
            }
        }
    }

    // ------------------------------------------------------------------ expressions
    fn ty(&mut self, e: &Expr) -> Ty {
        let span = e.span;
        match &e.kind {
            ExprKind::Num(_, d) => Ty::Num(*d),
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::NoneLit => Ty::None,
            ExprKind::Name(n) => self.name(n, span),
            ExprKind::Primed(n) => {
                match self.m.as_ref().map(|m| (m.vars.get(n).cloned(), m.primes)) {
                    Some((Some(t), primes)) => {
                        if !primes {
                            self.err(
                                span,
                                format!(
                                    "`{n}'` is only allowed in actions and transition invariants"
                                ),
                            );
                        }
                        t
                    }
                    _ => {
                        self.err(
                            span,
                            format!("`{n}` is not a state variable, so it can't be primed"),
                        );
                        Ty::Any
                    }
                }
            }
            ExprKind::Unary(UnOp::Neg, x) => match self.ty(x) {
                t @ (Ty::Num(_) | Ty::Any) => t,
                t => {
                    self.err(span, format!("can't negate a {}", t.show()));
                    Ty::Any
                }
            },
            ExprKind::Unary(UnOp::Not, x) => {
                let t = self.ty(x);
                self.want_bool(&t, x);
                Ty::Bool
            }
            ExprKind::Unary(_, x) => {
                let t = self.ty(x);
                self.want_bool(&t, x);
                Ty::Bool
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, span),
            ExprKind::If(c, a, b) => {
                let ct = self.ty(c);
                self.want_bool(&ct, c);
                let (ta, tb) = (self.ty(a), self.ty(b));
                if let (Ty::Num(x), Ty::Num(y)) = (&ta, &tb) {
                    if x != y {
                        self.err(
                            span,
                            format!(
                                "`if` branches differ in dimension: {} and {}",
                                x.describe(),
                                y.describe()
                            ),
                        );
                    }
                }
                if matches!(ta, Ty::Any | Ty::None) {
                    tb
                } else {
                    ta
                }
            }
            ExprKind::Call(f, args) => self.call(f, args, span),
            ExprKind::Field(x, f) => {
                let t = self.ty(x);
                self.field(&t, f, span)
            }
            ExprKind::Index(x, i) => {
                let t = self.ty(x);
                let it = self.ty(i);
                match t {
                    Ty::List(el) => {
                        if !matches!(it, Ty::Num(d) if d.is_none()) && it != Ty::Any {
                            self.err(
                                i.span,
                                format!("a list index is an Int, found {}", it.show()),
                            );
                        }
                        *el
                    }
                    Ty::Map(_, v) => *v,
                    Ty::Any | Ty::Tuple(_) => Ty::Any,
                    other => {
                        self.err(span, format!("can't index a {}", other.show()));
                        Ty::Any
                    }
                }
            }
            ExprKind::List(xs) => Ty::List(Box::new(self.elems(xs))),
            ExprKind::Set(xs) => Ty::Set(Box::new(self.elems(xs))),
            ExprKind::Tuple(xs) => Ty::Tuple(xs.iter().map(|x| self.ty(x)).collect()),
            ExprKind::Map(kv) => {
                let mut kt = Ty::Any;
                let mut vt = Ty::Any;
                for (k, v) in kv {
                    kt = self.ty(k);
                    vt = self.ty(v);
                }
                Ty::Map(Box::new(kt), Box::new(vt))
            }
            ExprKind::Record(fs) => {
                Ty::Record(fs.iter().map(|(n, x)| (n.clone(), self.ty(x))).collect())
            }
            ExprKind::Comp {
                kind,
                gens,
                filter,
                key,
                val,
            } => {
                let n0 = self.scopes.len();
                self.gens(gens);
                if let Some(f) = filter {
                    let t = self.ty(f);
                    self.want_bool(&t, f);
                }
                let kt = self.ty(key);
                let vt = val.as_ref().map(|v| self.ty(v));
                self.scopes.truncate(n0);
                match kind {
                    CompKind::List => Ty::List(Box::new(kt)),
                    CompKind::Set => Ty::Set(Box::new(kt)),
                    CompKind::Map => Ty::Map(Box::new(kt), Box::new(vt.unwrap_or(Ty::Any))),
                }
            }
            ExprKind::Quant(_, gens, body) => {
                let n0 = self.scopes.len();
                self.gens(gens);
                let t = self.ty(body);
                self.want_bool(&t, body);
                self.scopes.truncate(n0);
                Ty::Bool
            }
            ExprKind::With(x, path, v) => {
                let t = self.ty(x);
                for seg in path {
                    if let PathSeg::Index(k) = seg {
                        self.ty(k);
                    }
                }
                self.ty(v);
                t
            }
            ExprKind::Is(x, tname) => {
                self.ty(x);
                let builtin = ["Bool", "String", "Int", "Number"].contains(&tname.as_str())
                    || Dim::of_type_name(tname).is_some();
                if !builtin && self.prog.lookup_decl(self.pack, tname).is_none() {
                    self.err(span, format!("unknown type `{tname}`"));
                }
                Ty::Bool
            }
        }
    }

    fn elems(&mut self, xs: &[Expr]) -> Ty {
        let mut t = Ty::Any;
        for x in xs {
            let u = self.ty(x);
            if t == Ty::Any {
                t = u;
            }
        }
        t
    }

    fn want_bool(&mut self, t: &Ty, e: &Expr) {
        if !matches!(t, Ty::Bool | Ty::Any) {
            self.err(e.span, format!("expected a Bool, found {}", t.show()));
        }
    }

    fn gens(&mut self, gens: &[Gen]) {
        for g in gens {
            let st = self.ty(&g.src);
            let el = match &st {
                Ty::List(_) | Ty::Set(_) => st.elem(),
                Ty::Map(k, v) if g.tuple => Ty::Tuple(vec![(**k).clone(), (**v).clone()]),
                Ty::Map(k, _) => (**k).clone(),
                Ty::Any => Ty::Any,
                other => {
                    self.err(g.src.span, format!("can't range over a {}", other.show()));
                    Ty::Any
                }
            };
            if g.tuple {
                let parts = match &el {
                    Ty::Tuple(ts) if ts.len() == g.names.len() => ts.clone(),
                    _ => vec![Ty::Any; g.names.len()],
                };
                for (n, t) in g.names.iter().zip(parts) {
                    self.bind(n, t);
                }
            } else {
                self.bind(&g.names[0], el);
            }
        }
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, span: Span) -> Ty {
        use BinOp::*;
        let ta = self.ty(a);
        let tb = self.ty(b);
        let dims = |c: &mut Checker, x: Dim, y: Dim| {
            if x != y {
                c.err(
                    span,
                    format!(
                        "dimension mismatch: {} {} {}",
                        x.describe(),
                        op.text(),
                        y.describe()
                    ),
                );
            }
        };
        match op {
            And | Or | Implies | LeadsTo | Until => {
                self.want_bool(&ta, a);
                self.want_bool(&tb, b);
                Ty::Bool
            }
            Range => match (&ta, &tb) {
                (Ty::Num(x), Ty::Num(y)) => {
                    dims(self, *x, *y);
                    Ty::Set(Box::new(ta.clone()))
                }
                _ => Ty::Set(Box::new(Ty::Any)),
            },
            Coalesce => {
                if ta == Ty::Any {
                    tb
                } else {
                    ta
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                match (&ta, &tb) {
                    (Ty::Num(x), Ty::Num(y)) => dims(self, *x, *y),
                    (Ty::Enum(..), _) => self.expect(&ta, &tb, b, "comparison"),
                    (_, Ty::Enum(..)) => self.expect(&tb, &ta, a, "comparison"),
                    (Ty::Instant, Ty::Instant) | (Ty::Str, Ty::Str) => {}
                    (Ty::Any, _) | (_, Ty::Any) | (Ty::None, _) | (_, Ty::None) => {}
                    (x, y) if op.is_comparison() && !matches!(op, Eq | Ne) => {
                        self.err(span, format!("can't order {} and {}", x.show(), y.show()));
                    }
                    (x, y) => {
                        if let Some(m) = self.mismatch(x, y, b) {
                            self.err(span, format!("comparison: {m}"));
                        }
                    }
                }
                Ty::Bool
            }
            In => {
                match &tb {
                    Ty::List(el) | Ty::Set(el) | Ty::Map(el, _) => {
                        let el = (**el).clone();
                        match (&ta, &el) {
                            (Ty::Enum(..), _) => self.expect(&ta, &el, b, "`in`"),
                            (_, Ty::Enum(..)) => self.expect(&el, &ta, a, "`in`"),
                            (Ty::Num(x), Ty::Num(y)) => dims(self, *x, *y),
                            _ => {}
                        }
                    }
                    Ty::Str | Ty::Any => {}
                    other => self.err(
                        b.span,
                        format!("`in` needs a collection, found {}", other.show()),
                    ),
                }
                Ty::Bool
            }
            Tol => ta,
            Add | Sub => match (&ta, &tb) {
                (Ty::Num(x), Ty::Num(y)) => {
                    dims(self, *x, *y);
                    ta
                }
                (Ty::Instant, Ty::Instant) if op == Sub => Ty::Num(Dim::TIME),
                (Ty::Instant, Ty::Num(d)) | (Ty::Num(d), Ty::Instant) => {
                    if *d != Dim::TIME {
                        self.err(
                            span,
                            format!("an Instant moves by a Duration, not {}", d.describe()),
                        );
                    }
                    if op == Sub && tb == Ty::Instant {
                        self.err(span, "Duration − Instant is meaningless");
                    }
                    Ty::Instant
                }
                (Ty::Instant, Ty::Instant) => {
                    self.err(
                        span,
                        "Instant + Instant is meaningless (an Instant − Instant is a Duration)",
                    );
                    Ty::Any
                }
                (Ty::Any, _) => tb,
                (_, Ty::Any) => ta,
                (Ty::Set(_), Ty::Set(_)) | (Ty::List(_), Ty::List(_)) | (Ty::Str, Ty::Str) => ta,
                (x, y) => {
                    self.err(
                        span,
                        format!(
                            "can't {} {} and {}",
                            if op == Add { "add" } else { "subtract" },
                            x.show(),
                            y.show()
                        ),
                    );
                    Ty::Any
                }
            },
            Union | Minus | Inter => ta,
            Mul | Div => match (&ta, &tb) {
                (Ty::Num(x), Ty::Num(y)) => Ty::Num(if op == Mul { x.mul(*y) } else { x.div(*y) }),
                (Ty::Any, _) | (_, Ty::Any) => Ty::Any,
                (x, y) => {
                    self.err(
                        span,
                        format!(
                            "can't {} {} and {}",
                            if op == Mul { "multiply" } else { "divide" },
                            x.show(),
                            y.show()
                        ),
                    );
                    Ty::Any
                }
            },
            Pow => match (&ta, &tb) {
                (Ty::Num(x), Ty::Num(y)) => {
                    if !y.is_none() {
                        self.err(
                            b.span,
                            format!("an exponent must be dimensionless, found {}", y.describe()),
                        );
                    }
                    let k = match &b.kind {
                        ExprKind::Num(n, _) => n.as_int(),
                        ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
                            ExprKind::Num(n, _) => n.as_int().map(|i| -i),
                            _ => None,
                        },
                        _ => None,
                    };
                    match k {
                        Some(k) => Ty::Num(x.pow(k)),
                        None if x.is_none() => Ty::Num(Dim::NONE),
                        None => {
                            self.err(
                                span,
                                format!(
                                    "a {} can only be raised to a literal integer power",
                                    x.describe()
                                ),
                            );
                            Ty::Any
                        }
                    }
                }
                _ => Ty::Any,
            },
        }
    }

    fn field(&mut self, t: &Ty, f: &str, span: Span) -> Ty {
        match t {
            Ty::Any | Ty::Id | Ty::ModelValue => Ty::Any,
            Ty::Record(fs) => match fs.iter().find(|x| x.0 == f) {
                Some(x) => x.1.clone(),
                None => {
                    self.err(span, format!("record has no field `{f}`"));
                    Ty::Any
                }
            },
            Ty::Cycle => match f {
                "agents" => Ty::List(Box::new(Ty::Any)),
                "transfers" | "edges" => Ty::List(Box::new(Ty::Any)),
                "returned" => Ty::Num(Dim::NONE),
                _ => {
                    self.err(
                        span,
                        format!("Cycle has no field `{f}` (agents, transfers, returned)"),
                    );
                    Ty::Any
                }
            },
            Ty::Node(r, name) => match self.node_field(*r, f) {
                Some(t) => t,
                None => {
                    self.err(span, format!("{name} has no field `{f}`"));
                    Ty::Any
                }
            },
            other => {
                self.err(span, format!("a {} has no field `{f}`", other.show()));
                Ty::Any
            }
        }
    }

    fn node_field(&mut self, r: DeclRef, f: &str) -> Option<Ty> {
        let saved = self.pack;
        self.pack = self.prog.pack_of(r.file);
        let out = match self.prog.item(r) {
            Item::Entity(e) => {
                if let Some(fd) = e.fields.iter().find(|x| x.name == f) {
                    Some(self.ty_of(&fd.ty.clone()))
                } else if f == "id" {
                    Some(Ty::Id)
                } else {
                    match &e.parent {
                        Some(p) => match self.prog.lookup_decl(self.pack, p) {
                            Some(pr) => self.node_field(pr, f),
                            None => None,
                        },
                        None => None,
                    }
                }
            }
            Item::Edge(e) => {
                if let Some(fd) = e.fields.iter().find(|x| x.name == f) {
                    Some(self.ty_of(&fd.ty.clone()))
                } else {
                    match f {
                        "from" => Some(self.ty_of(&e.from.clone())),
                        "to" => Some(self.ty_of(&e.to.clone())),
                        "id" => Some(Ty::Id),
                        "valid_from" | "valid_to" if e.temporal => Some(Ty::Instant),
                        _ => None,
                    }
                }
            }
            _ => None,
        };
        self.pack = saved;
        out
    }

    fn args_against(
        &mut self,
        fname: &str,
        params: &[ParamDef],
        decl_pack: usize,
        args: &[Arg],
        span: Span,
    ) {
        let saved = self.pack;
        self.pack = decl_pack;
        let ptys: Vec<Ty> = params.iter().map(|p| self.ty_of(&p.ty.clone())).collect();
        self.pack = saved;
        let mut filled = vec![false; params.len()];
        let mut pos = 0;
        for a in args {
            let t = self.ty(&a.value);
            let i = match &a.name {
                None => {
                    pos += 1;
                    pos - 1
                }
                Some(n) => match params.iter().position(|p| p.name == *n) {
                    Some(i) => i,
                    None => {
                        self.err(a.value.span, format!("`{fname}` has no parameter `{n}`"));
                        continue;
                    }
                },
            };
            if i >= params.len() {
                self.err(
                    span,
                    format!(
                        "`{fname}` takes {} argument(s), got {}",
                        params.len(),
                        args.len()
                    ),
                );
                return;
            }
            filled[i] = true;
            let what = format!("argument `{}` of `{fname}`", params[i].name);
            self.expect(&ptys[i], &t, &a.value, &what);
        }
        for (p, f) in params.iter().zip(filled) {
            if !f && p.default.is_none() {
                self.err(span, format!("`{fname}` is missing argument `{}`", p.name));
            }
        }
    }

    fn call(&mut self, f: &Expr, args: &[Arg], span: Span) -> Ty {
        if let ExprKind::Name(n) = &f.kind {
            if let Some(&k) = self.m.as_ref().and_then(|m| m.defs.get(n)) {
                if k != args.len() {
                    self.err(
                        span,
                        format!("`{n}` takes {k} argument(s), got {}", args.len()),
                    );
                }
                for a in args {
                    self.ty(&a.value);
                }
                return Ty::Any;
            }
        }
        let name = match &f.kind {
            ExprKind::Name(n) => n.clone(),
            ExprKind::Field(obj, method) => {
                let ot = self.ty(obj);
                if method == "out" || method == "in" {
                    if !matches!(ot, Ty::Node(..) | Ty::Any) {
                        self.err(
                            f.span,
                            format!("`.{method}(EDGE)` needs a node, found {}", ot.show()),
                        );
                    }
                    if let [Arg {
                        value:
                            Expr {
                                kind: ExprKind::Name(edge),
                                span: es,
                            },
                        ..
                    }] = args
                    {
                        return match self
                            .prog
                            .lookup_decl(self.pack, edge)
                            .map(|r| (r, self.prog.item(r)))
                        {
                            Some((r, Item::Edge(e))) => {
                                Ty::List(Box::new(Ty::Node(r, e.name.clone())))
                            }
                            _ => {
                                self.err(*es, format!("`{edge}` is not an edge type"));
                                Ty::Any
                            }
                        };
                    }
                    self.err(
                        span,
                        format!("`.{method}` takes one edge type, like `.{method}(CONTACT)`"),
                    );
                    return Ty::Any;
                }
                self.err(f.span, format!("unknown method `{method}`"));
                return Ty::Any;
            }
            _ => {
                self.err(f.span, "only named functions can be called");
                return Ty::Any;
            }
        };
        match self.prog.lookup(self.pack, &name) {
            Err(e) => {
                self.err(f.span, e);
                return Ty::Any;
            }
            Ok(Some((NameKind::Decl, r))) => {
                let dp = self.prog.pack_of(r.file);
                let item = self.prog.item(r).clone();
                let ret = |c: &mut Checker, t: &TypeExpr| {
                    let saved = c.pack;
                    c.pack = dp;
                    let x = c.ty_of(t);
                    c.pack = saved;
                    x
                };
                return match &item {
                    Item::Fn(d) => {
                        self.args_against(&name, &d.params, dp, args, span);
                        ret(self, &d.ret)
                    }
                    Item::Table(d) => {
                        self.args_against(&name, &d.params, dp, args, span);
                        ret(self, &d.ret)
                    }
                    Item::Kind(d) => {
                        self.args_against(&name, &d.params, dp, args, span);
                        ret(self, &d.ret)
                    }
                    Item::Measure(d) => {
                        self.args_against(&name, &d.params, dp, args, span);
                        ret(self, &d.ret)
                    }
                    _ => {
                        self.err(f.span, format!("`{name}` is not callable"));
                        Ty::Any
                    }
                };
            }
            Ok(Some((NameKind::Collection, _))) => {
                self.err(f.span, format!("`{name}` is a collection, not a function"));
                return Ty::Any;
            }
            Ok(None) => {}
        }
        if !BUILTINS.contains(&name.as_str()) {
            self.err(f.span, format!("unknown function `{name}`"));
            for a in args {
                self.ty(&a.value);
            }
            return Ty::Any;
        }
        let ts: Vec<Ty> = args.iter().map(|a| self.ty(&a.value)).collect();
        let first = ts.first().cloned().unwrap_or(Ty::Any);
        let num = |t: &Ty| if let Ty::Num(d) = t { Some(*d) } else { None };
        match name.as_str() {
            "len" | "count" => Ty::Num(Dim::NONE),
            "sum" | "first" | "last" => first.elem(),
            "min" | "max" => {
                if ts.len() == 1 {
                    first.elem()
                } else {
                    for (t, a) in ts.iter().zip(args).skip(1) {
                        if let (Some(x), Some(y)) = (num(&first), num(t)) {
                            if x != y {
                                self.err(
                                    a.value.span,
                                    format!("`{name}` of {} and {}", x.describe(), y.describe()),
                                );
                            }
                        }
                    }
                    first
                }
            }
            "abs" | "floor" | "ceil" | "clamp" | "range" | "tol" | "sort_by" => first,
            "round_to" => {
                if let (Some(x), Some(Some(y))) = (num(&first), ts.get(1).map(num)) {
                    if x != y {
                        self.err(
                            span,
                            format!(
                                "round_to: the step is {}, the value {}",
                                y.describe(),
                                x.describe()
                            ),
                        );
                    }
                }
                first
            }
            "ln" => {
                if let Some(d) = num(&first) {
                    if !d.is_none() {
                        self.err(
                            args[0].value.span,
                            format!("`ln` needs a dimensionless number, found {}", d.describe()),
                        );
                    }
                }
                Ty::Num(Dim::NONE)
            }
            "certainly" | "possibly" | "matches" | "starts_with" | "ends_with" | "any" | "all" => {
                Ty::Bool
            }
            "lower" => Ty::Str,
            "keys" => Ty::Set(Box::new(first.elem())),
            "values" => match first {
                Ty::Map(_, v) => Ty::List(v),
                _ => Ty::Any,
            },
            "to_credits" => Ty::Num(Dim::CREDITS),
            "to_money" => Ty::Num(Dim::MONEY),
            _ => Ty::Any,
        }
    }

    // ------------------------------------------------------------------ items
    fn params(&mut self, ps: &[ParamDef]) {
        for p in ps {
            let t = self.ty_of(&p.ty);
            if let Some(d) = &p.default {
                let dt = self.ty(d);
                self.expect(&t, &dt, d, &format!("default of `{}`", p.name));
            }
            self.bind(&p.name, t);
        }
    }

    fn interp(&mut self, s: &Interp) {
        for p in &s.parts {
            if let InterpPart::Expr(e) = p {
                self.ty(e);
            }
        }
    }

    fn clauses(&mut self, cs: &[Clause]) {
        for c in cs {
            match c {
                Clause::For(g) => self.gens(g),
                Clause::Where(e) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                }
                Clause::Let(n, e) => {
                    let t = self.ty(e);
                    self.bind(n, t);
                }
                Clause::Require(e, m) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                    if let Some(m) = m {
                        self.ty(m);
                    }
                }
                Clause::Message(s) => self.interp(s),
                Clause::Flag(s, on) => {
                    self.interp(s);
                    let t = self.ty(on);
                    if !matches!(t, Ty::List(_) | Ty::Set(_) | Ty::Any) {
                        self.err(
                            on.span,
                            format!("`flag … on` takes a list of agents, found {}", t.show()),
                        );
                    }
                }
                Clause::Fix(s, set) => {
                    self.interp(s);
                    if let Some((p, v)) = set {
                        self.ty(p);
                        self.ty(v);
                    }
                }
                Clause::OnChange(ps) => {
                    for p in ps {
                        self.ty(p);
                    }
                }
                Clause::Severity(_)
                | Clause::Violation(_)
                | Clause::Cite(_)
                | Clause::Implements(_) => {}
            }
        }
    }

    fn item(&mut self, r: DeclRef, it: &Item) {
        match it {
            Item::Import(..) => {}
            Item::Param(p, _) => self.params(std::slice::from_ref(p)),
            Item::Let(_, e, _) => {
                self.ty(e);
            }
            Item::Fn(d) => {
                self.params(&d.params);
                let rt = self.ty_of(&d.ret);
                let bt = self.ty(&d.body);
                self.expect(
                    &rt,
                    &bt,
                    &d.body,
                    &format!("`{}` returns {}", d.name, d.ret),
                );
            }
            Item::Table(d) => {
                let pts: Vec<Ty> = d.params.iter().map(|p| self.ty_of(&p.ty)).collect();
                let rt = self.ty_of(&d.ret);
                for (a, _, s) in &d.axes {
                    if !d.params.iter().any(|p| p.name == *a) {
                        self.err(
                            *s,
                            format!("axis `{a}` is not a parameter of table {}", d.name),
                        );
                    }
                }
                let mut seen: Vec<String> = Vec::new();
                for (keys, v) in &d.rows {
                    if keys.len() != d.params.len() {
                        self.err(
                            keys[0].span,
                            format!(
                                "a row of {} has {} key(s), the table has {} axis(es)",
                                d.name,
                                keys.len(),
                                d.params.len()
                            ),
                        );
                    }
                    for (k, pt) in keys.iter().zip(&pts) {
                        let kt = self.ty(k);
                        self.expect(pt, &kt, k, "table key");
                    }
                    let vt = self.ty(v);
                    self.expect(&rt, &vt, v, &format!("row value of {}", d.name));
                    let text: String = keys
                        .iter()
                        .map(|k| self.prog.sources.snippet(k.span).to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    if seen.contains(&text) {
                        self.err(
                            keys[0].span,
                            format!("duplicate row ({text}) in table {}", d.name),
                        );
                    }
                    seen.push(text);
                }
            }
            Item::Rel(d) => {
                self.params(&d.params);
                let tys: Vec<Ty> = d.params.iter().map(|p| self.ty_of(&p.ty)).collect();
                for b in &d.bodies {
                    let n0 = self.scopes.len();
                    self.bind(&d.name, Ty::Set(Box::new(Ty::Tuple(tys.clone()))));
                    self.gens(&b.gens);
                    if let Some(f) = &b.filter {
                        let t = self.ty(f);
                        self.want_bool(&t, f);
                    }
                    self.ty(&b.yields);
                    self.scopes.truncate(n0);
                }
            }
            Item::Enum(d) => {
                let mut seen = HashSet::new();
                for (m, _, s) in &d.members {
                    if !seen.insert(m) {
                        self.err(*s, format!("enum {} lists \"{m}\" twice", d.name));
                    }
                }
            }
            Item::Entity(e) => {
                if let Some(p) = &e.parent {
                    match self
                        .prog
                        .lookup_decl(self.pack, p)
                        .map(|r| self.prog.item(r))
                    {
                        Some(Item::Entity(_)) => {}
                        _ => self.err(
                            e.span,
                            format!("parent `{p}` of {} is not an entity", e.name),
                        ),
                    }
                }
                self.fields(&e.fields);
            }
            Item::Edge(e) => {
                for t in [&e.from, &e.to] {
                    if !matches!(self.ty_of(t), Ty::Node(..) | Ty::Any) {
                        self.err(
                            t.span(),
                            format!("edge {} must connect entity types, found {t}", e.name),
                        );
                    }
                }
                self.fields(&e.fields);
            }
            Item::Measure(d) => {
                self.params(&d.params);
                self.ty_of(&d.ret);
            }
            Item::Kind(d) => {
                self.params(&d.params);
                let rt = self.ty_of(&d.ret);
                if !matches!(rt, Ty::Num(_) | Ty::Any) {
                    self.err(
                        d.ret.span(),
                        format!("kind {} must be numeric in v0, found {}", d.name, d.ret),
                    );
                }
                if let Some(e) = &d.default {
                    let t = self.ty(e);
                    self.expect(&rt, &t, e, &format!("default of kind {}", d.name));
                    if d.ret.name() == Some("Int") {
                        if let ExprKind::Num(n, _) = &e.kind {
                            if n.as_int().is_none() {
                                self.err(
                                    e.span,
                                    format!("kind {} is an Int; its default {n} is not", d.name),
                                );
                            }
                        }
                    }
                }
            }
            Item::Constraint(c) => self.constraint(c),
            Item::Check(c) => self.clauses(&c.clauses),
            Item::Machine(m) => self.machine(r, m),
            Item::Model(m) => self.model(m),
            Item::Mutant(m) => self.mutant(m),
            Item::Witness(w) => self.witness(w),
            Item::Scenario(s) => self.scenario(s),
            _ => {}
        }
    }

    fn fields(&mut self, fs: &[FieldDecl]) {
        let mut seen = HashSet::new();
        for f in fs {
            if !seen.insert(&f.name) {
                self.err(f.span, format!("field `{}` is declared twice", f.name));
            }
            let t = self.ty_of(&f.ty);
            if let Some(d) = &f.default {
                let dt = self.ty(d);
                self.expect(&t, &dt, d, &format!("default of field `{}`", f.name));
            }
        }
    }

    fn constraint(&mut self, c: &ConstraintDecl) {
        let Some(r) = self.prog.lookup_decl(self.pack, &c.kind) else {
            self.err(
                c.span,
                format!("unknown kind `{}` (kinds live in specs/kinds.jhrl)", c.kind),
            );
            return;
        };
        let Item::Kind(k) = self.prog.item(r).clone() else {
            self.err(c.span, format!("`{}` is not a kind", c.kind));
            return;
        };
        if k.params.len() != c.args.len() {
            self.err(
                c.span,
                format!(
                    "kind {} takes {} object(s); the rule names {}",
                    k.name,
                    k.params.len(),
                    c.args.len()
                ),
            );
        }
        let saved = self.pack;
        self.pack = self.prog.pack_of(r.file);
        let ptys: Vec<Ty> = k.params.iter().map(|p| self.ty_of(&p.ty)).collect();
        let rt = self.ty_of(&k.ret);
        self.pack = saved;
        for (a, t) in c.args.iter().zip(ptys) {
            self.bind(a, t);
        }
        for w in &c.wheres {
            let t = self.ty(w);
            self.want_bool(&t, w);
        }
        match &c.value {
            Some(v) => {
                let t = self.ty(v);
                self.expect(&rt, &t, v, &format!("value of a {} rule", k.name));
            }
            None => self.err(c.span, "a constraint rule needs a `value`"),
        }
    }

    fn mscope(&mut self, mref: DeclRef, m: &MachineDecl) -> MScope {
        let mut s = MScope::default();
        // model value types from every model of this machine
        for (pack, _, it) in self.prog.items() {
            if let Item::Model(md) = it {
                if self.prog.lookup_decl(pack, &md.machine) == Some(mref) {
                    for o in &md.opts {
                        if let MOpt::Values(t, e) = o {
                            s.model_types.insert(t.clone());
                            if let ExprKind::Set(xs) = &e.kind {
                                for x in xs {
                                    if let ExprKind::Name(n) = &x.kind {
                                        s.model_values.insert(n.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        self.m = Some(s.clone());
        for it in &m.items {
            match it {
                MItem::Const { name, ty, .. } => {
                    let t = self.ty_of(ty);
                    s.consts.insert(name.clone(), t);
                }
                MItem::Var { name, ty, .. } => {
                    let t = self.ty_of(ty);
                    s.vars.insert(name.clone(), t);
                }
                MItem::Action(a) => {
                    s.actions.insert(a.name.clone());
                }
                MItem::Def { name, params, .. } => {
                    s.defs.insert(name.clone(), params.len());
                }
                _ => {}
            }
        }
        s
    }

    fn machine(&mut self, r: DeclRef, m: &MachineDecl) {
        if let Some((parts, _)) = &m.compose {
            for p in parts {
                if !matches!(
                    self.prog
                        .lookup_decl(self.pack, p)
                        .map(|r| self.prog.item(r)),
                    Some(Item::Machine(_))
                ) {
                    self.err(m.span, format!("`{p}` is not a machine"));
                }
            }
            return;
        }
        let scope = self.mscope(r, m);
        self.m = Some(scope);
        for it in &m.items {
            match it {
                MItem::Const {
                    name, ty, default, ..
                } => {
                    if let Some(d) = default {
                        let t = self.ty_of(ty);
                        let dt = self.ty(d);
                        self.expect(&t, &dt, d, &format!("const {name}"));
                    }
                }
                MItem::Var { name, ty, init, .. } => {
                    let t = self.ty_of(ty);
                    match init {
                        VarInit::Eq(e) => {
                            let et = self.ty(e);
                            self.expect(&t, &et, e, &format!("initial value of `{name}`"));
                        }
                        VarInit::In(e) => {
                            let et = self.ty(e);
                            match et {
                                Ty::Set(el) | Ty::List(el) => {
                                    self.expect(&t, &el, e, &format!("initial set of `{name}`"))
                                }
                                Ty::Any => {}
                                other => self.err(
                                    e.span,
                                    format!(
                                        "`var {name}: … in S` needs a set, found {}",
                                        other.show()
                                    ),
                                ),
                            }
                        }
                        VarInit::None => {}
                    }
                }
                MItem::Init(body, _) => self.block(body, true),
                MItem::Action(a) => {
                    let n0 = self.scopes.len();
                    for p in &a.params {
                        let t = match &p.dom {
                            ADom::Type(t) => self.ty_of(t),
                            ADom::In(e) => match self.ty(e) {
                                Ty::Set(el) | Ty::List(el) => *el,
                                Ty::Any => Ty::Any,
                                other => {
                                    self.err(
                                        e.span,
                                        format!(
                                            "parameter `{}` ranges over a set, found {}",
                                            p.name,
                                            other.show()
                                        ),
                                    );
                                    Ty::Any
                                }
                            },
                        };
                        self.bind(&p.name, t);
                    }
                    self.block(&a.body, false);
                    self.scopes.truncate(n0);
                }
                MItem::Invariant { body, .. } => match body {
                    InvBody::Expr(e) => {
                        let t = self.ty(e);
                        self.want_bool(&t, e);
                    }
                    InvBody::Clauses(c) => {
                        let n0 = self.scopes.len();
                        self.clauses(c);
                        self.scopes.truncate(n0);
                    }
                },
                MItem::Transition { expr, .. } => {
                    self.m.as_mut().unwrap().primes = true;
                    let t = self.ty(expr);
                    self.want_bool(&t, expr);
                    self.m.as_mut().unwrap().primes = false;
                }
                MItem::Property { expr, .. } => {
                    let t = self.ty(expr);
                    self.want_bool(&t, expr);
                }
                MItem::Fairness { actions, span, .. } => {
                    for a in actions {
                        if !self.m.as_ref().unwrap().actions.contains(a) {
                            self.err(
                                *span,
                                format!(
                                    "fairness names `{a}`, which is not an action of {}",
                                    m.name
                                ),
                            );
                        }
                    }
                }
                MItem::Terminal(e) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                }
                MItem::Def { params, body, .. } => {
                    let n0 = self.scopes.len();
                    for p in params {
                        self.bind(p, Ty::Any);
                    }
                    self.ty(body);
                    self.scopes.truncate(n0);
                }
            }
        }
        self.m = None;
    }

    fn block(&mut self, body: &[Stmt], init: bool) {
        let n0 = self.scopes.len();
        self.stmts(body, init);
        self.scopes.truncate(n0);
    }

    /// Checks alternatives; the names every alternative binds stay bound after them (an action's
    /// `let` and `pick` inside `either` or `if` carry on to the following statements).
    fn alternatives(&mut self, alts: &[&[Stmt]], init: bool) {
        let n0 = self.scopes.len();
        let mut common: Option<Vec<(String, Ty)>> = None;
        for a in alts {
            self.stmts(a, init);
            let added: Vec<(String, Ty)> = self.scopes[n0..].to_vec();
            self.scopes.truncate(n0);
            common = Some(match common {
                None => added,
                Some(c) => c
                    .into_iter()
                    .filter(|(n, _)| added.iter().any(|(m, _)| m == n))
                    .collect(),
            });
        }
        self.scopes.extend(common.unwrap_or_default());
    }

    fn stmts(&mut self, body: &[Stmt], init: bool) {
        for st in body {
            match &st.kind {
                StmtKind::When(e) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                }
                StmtKind::Assign(x, e) => {
                    self.m.as_mut().unwrap().primes = !init;
                    let et = self.ty(e);
                    self.m.as_mut().unwrap().primes = false;
                    let vt = self.m.as_ref().unwrap().vars.get(x).cloned();
                    match vt {
                        Some(vt) => self.expect(&vt, &et, e, &format!("`{x}'`")),
                        None => self.err(st.span, format!("`{x}` is not a state variable")),
                    }
                }
                StmtKind::Let(x, e) => {
                    let t = self.ty(e);
                    self.bind(x, t);
                }
                StmtKind::Pick(x, e) => {
                    let t = self.ty(e);
                    self.bind(x, t.elem());
                }
                StmtKind::Either(alts) => {
                    let alts: Vec<&[Stmt]> = alts.iter().map(|a| a.as_slice()).collect();
                    self.alternatives(&alts, init);
                }
                StmtKind::If(c, then, els) => {
                    let t = self.ty(c);
                    self.want_bool(&t, c);
                    self.alternatives(&[then, els], init);
                }
            }
        }
    }

    fn machine_for(&mut self, name: &str, span: Span) -> Option<(DeclRef, MachineDecl)> {
        match self.prog.lookup(self.pack, name) {
            Ok(Some((NameKind::Decl, r))) => match self.prog.item(r) {
                Item::Machine(m) => Some((r, m.clone())),
                _ => {
                    self.err(span, format!("`{name}` is not a machine"));
                    None
                }
            },
            Err(e) => {
                self.err(span, e);
                None
            }
            _ => {
                self.err(span, format!("unknown machine `{name}`"));
                None
            }
        }
    }

    /// Enters a machine's scope (its vars and consts) to check expressions about it.
    fn enter(&mut self, r: DeclRef, m: &MachineDecl) {
        let saved = self.pack;
        self.pack = self.prog.pack_of(r.file);
        let s = self.mscope(r, m);
        self.pack = saved;
        self.m = Some(s);
    }

    fn model(&mut self, md: &ModelDecl) {
        let Some((r, m)) = self.machine_for(&md.machine, md.span) else {
            return;
        };
        self.enter(r, &m);
        for o in &md.opts {
            match o {
                MOpt::Const(n, e) => {
                    let ct = self.m.as_ref().unwrap().consts.get(n).cloned();
                    let et = self.ty(e);
                    match ct {
                        Some(ct) => self.expect(&ct, &et, e, &format!("const {n}")),
                        None => self.err(e.span, format!("machine {} has no const `{n}`", m.name)),
                    }
                }
                MOpt::Domain(_, a, b) => {
                    let (ta, tb) = (self.ty(a), self.ty(b));
                    if let (Ty::Num(x), Ty::Num(y)) = (&ta, &tb) {
                        if x != y {
                            self.err(a.span.to(b.span), "domain ends differ in dimension");
                        }
                    }
                }
                MOpt::Constraint(e) | MOpt::Terminal(e) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                }
                MOpt::Check(ws) => {
                    for w in ws {
                        if !["invariants", "properties", "deadlock"].contains(&w.as_str()) {
                            self.err(
                                md.span,
                                format!("`check {w}`: expected invariants, properties or deadlock"),
                            );
                        }
                    }
                }
                MOpt::Values(..) | MOpt::Symmetry(_) | MOpt::DeadlockAllowed => {}
            }
        }
        self.m = None;
    }

    fn mutant(&mut self, mu: &MutantDecl) {
        let Some((_, m)) = self.machine_for(&mu.machine, mu.span) else {
            return;
        };
        for op in &mu.ops {
            let (a, span) = match op {
                MutOp::Replace { action, span, .. }
                | MutOp::DropWhen { action, span, .. }
                | MutOp::RemoveAction { action, span } => (action, span),
            };
            if !m.actions().any(|x| x.name == *a) {
                self.err(*span, format!("machine {} has no action `{a}`", m.name));
            }
        }
        if let Expect::Violation(n) = &mu.expect {
            let known = m.items.iter().any(|i| matches!(i, MItem::Invariant { name, .. } | MItem::Transition { name, .. } if name == n));
            if !known {
                self.err(
                    mu.span,
                    format!("machine {} has no invariant or transition \"{n}\"", m.name),
                );
            }
        }
        if let Expect::Property(n) = &mu.expect {
            if !m
                .items
                .iter()
                .any(|i| matches!(i, MItem::Property { name, .. } if name == n))
            {
                self.err(
                    mu.span,
                    format!("machine {} has no property \"{n}\"", m.name),
                );
            }
        }
    }

    fn witness(&mut self, w: &WitnessDecl) {
        let Some((r, m)) = self.machine_for(&w.machine, w.span) else {
            return;
        };
        if let Some(mn) = &w.model {
            let found = crate::mc::models_of(self.prog, r)
                .iter()
                .any(|(_, _, md)| md.name == *mn);
            if !found {
                self.err(w.span, format!("machine {} has no model \"{mn}\"", m.name));
            }
        }
        self.enter(r, &m);
        let t = self.ty(&w.expr);
        self.want_bool(&t, &w.expr);
        self.m = None;
    }

    fn scenario(&mut self, s: &ScenarioDecl) {
        let machine = match &s.machine {
            Some(n) => match self.machine_for(n, s.span) {
                Some(x) => Some(x),
                None => return,
            },
            None => None,
        };
        if let Some((r, m)) = &machine {
            self.enter(*r, m);
        }
        for st in &s.steps {
            match &st.kind {
                SStepKind::Given(asg) => {
                    for (n, e) in asg {
                        let et = self.ty(e);
                        let want = self
                            .m
                            .as_ref()
                            .and_then(|m| m.vars.get(n).or_else(|| m.consts.get(n)).cloned());
                        match (&machine, want) {
                            (Some(_), Some(t)) => self.expect(&t, &et, e, &format!("given `{n}`")),
                            (Some((_, m)), None) => self.err(
                                e.span,
                                format!("machine {} has no const or var `{n}`", m.name),
                            ),
                            (None, _) => {}
                        }
                    }
                }
                SStepKind::When(e) => {
                    if let Some((_, m)) = &machine {
                        let (an, args) = match &e.kind {
                            ExprKind::Name(n) => (n.clone(), vec![]),
                            ExprKind::Call(f, a) => match &f.kind {
                                ExprKind::Name(n) => (n.clone(), a.clone()),
                                _ => (String::new(), vec![]),
                            },
                            _ => (String::new(), vec![]),
                        };
                        match m.actions().find(|a| a.name == an) {
                            Some(a) => {
                                if a.params.len() != args.len() {
                                    self.err(
                                        e.span,
                                        format!(
                                            "{an} takes {} argument(s), got {}",
                                            a.params.len(),
                                            args.len()
                                        ),
                                    );
                                }
                                for (p, arg) in a.params.iter().zip(&args) {
                                    let at = self.ty(&arg.value);
                                    let pt = match &p.dom {
                                        ADom::Type(t) => self.ty_of(t),
                                        ADom::In(d) => self.ty(d).elem(),
                                    };
                                    self.expect(
                                        &pt,
                                        &at,
                                        &arg.value,
                                        &format!("argument `{}` of {an}", p.name),
                                    );
                                }
                            }
                            None => {
                                self.err(e.span, format!("machine {} has no action `{an}`", m.name))
                            }
                        }
                    }
                }
                SStepKind::Then(e) => {
                    let t = self.ty(e);
                    self.want_bool(&t, e);
                }
                SStepKind::Fixture(_) | SStepKind::Rejected(_) => {}
            }
        }
        self.m = None;
    }

    // ------------------------------------------------------------------ recursion
    fn recursion(&mut self) {
        let mut graph: HashMap<DeclRef, Vec<DeclRef>> = HashMap::new();
        let mut spans = HashMap::new();
        for (pack, r, it) in self.prog.items() {
            if let Item::Fn(d) = it {
                let mut calls = Vec::new();
                collect_calls(&d.body, &mut calls);
                let targets = calls
                    .iter()
                    .filter_map(|n| self.prog.lookup_decl(pack, n))
                    .filter(|t| matches!(self.prog.item(*t), Item::Fn(_)))
                    .collect();
                graph.insert(r, targets);
                spans.insert(r, (d.span, d.name.clone()));
            }
        }
        // A fn is recursive when it can reach itself.
        let mut reported = HashSet::new();
        let mut keys: Vec<&DeclRef> = graph.keys().collect();
        keys.sort_by_key(|r| (r.file, r.item));
        for &start in keys {
            let mut stack = graph[&start].clone();
            let mut seen = HashSet::new();
            while let Some(n) = stack.pop() {
                if n == start {
                    if reported.insert(start) {
                        let (s, name) = spans[&start].clone();
                        self.err(
                            s,
                            format!(
                                "fn {name} is recursive; functions may not recurse (section 9.2)"
                            ),
                        );
                    }
                    break;
                }
                if seen.insert(n) {
                    stack.extend(graph.get(&n).cloned().unwrap_or_default());
                }
            }
        }
    }
}

fn collect_calls(e: &Expr, out: &mut Vec<String>) {
    let mut go = |x: &Expr| collect_calls(x, out);
    match &e.kind {
        ExprKind::Call(f, args) => {
            if let ExprKind::Name(n) = &f.kind {
                out.push(n.clone());
            } else {
                collect_calls(f, out);
            }
            for a in args {
                collect_calls(&a.value, out);
            }
        }
        ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::Is(x, _) => go(x),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            collect_calls(a, out);
            collect_calls(b, out);
        }
        ExprKind::If(a, b, c) => {
            collect_calls(a, out);
            collect_calls(b, out);
            collect_calls(c, out);
        }
        ExprKind::List(xs) | ExprKind::Set(xs) | ExprKind::Tuple(xs) => xs.iter().for_each(go),
        ExprKind::Map(kv) => {
            for (k, v) in kv {
                collect_calls(k, out);
                collect_calls(v, out);
            }
        }
        ExprKind::Record(fs) => fs.iter().for_each(|(_, x)| collect_calls(x, out)),
        ExprKind::Comp {
            gens,
            filter,
            key,
            val,
            ..
        } => {
            for g in gens {
                collect_calls(&g.src, out);
            }
            if let Some(f) = filter {
                collect_calls(f, out);
            }
            collect_calls(key, out);
            if let Some(v) = val {
                collect_calls(v, out);
            }
        }
        ExprKind::Quant(_, gens, b) => {
            for g in gens {
                collect_calls(&g.src, out);
            }
            collect_calls(b, out);
        }
        ExprKind::With(a, path, b) => {
            collect_calls(a, out);
            for seg in path {
                if let PathSeg::Index(k) = seg {
                    collect_calls(k, out);
                }
            }
            collect_calls(b, out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(src: &str) -> Vec<String> {
        let (p, d) = Program::load(vec![("t.jhrl".into(), src.into())]);
        assert!(d.is_empty(), "{d:?}");
        check_program(&p).into_iter().map(|d| d.msg).collect()
    }

    #[test]
    fn static_checks_report_each_error_class() {
        // dimension_errors_in_fn_bodies_are_static: dimension errors in fn bodies are static.
        {
            let d = diags("fn f(x: Credits) -> Credits = x * 2 s");
            assert_eq!(d.len(), 1, "{d:?}");
            assert!(d[0].contains("expected Credits, found [cr·s]"), "{d:?}");
            let d = diags("fn g(x: Credits) -> Bool = x <= 1000");
            assert!(
                d[0].contains("dimension mismatch: Credits <= Number"),
                "{d:?}"
            );
            assert!(diags("fn h(x: Credits) -> Bool = x <= 1000 cr").is_empty());
            let d = diags("fn k(x: Duration) -> Number = ln(x)");
            assert!(d[0].contains("`ln` needs a dimensionless number"), "{d:?}");
            assert!(diags("fn r(t: Number, s: Credits) -> Credits = floor(s * t)").is_empty());
        }
        // names_and_calls_are_checked: unknown names and calls, argument types, recursion, defaults.
        {
            let d = diags("fn f(x: Number) -> Number = y + nope(x)");
            assert!(d.iter().any(|m| m.contains("unknown name `y`")), "{d:?}");
            assert!(
                d.iter().any(|m| m.contains("unknown function `nope`")),
                "{d:?}"
            );
            let d = diags("fn f(x: Credits) -> Credits = x\nfn g() -> Credits = f(3 s)");
            assert!(
                d[0].contains("argument `x` of `f`: expected Credits, found Duration"),
                "{d:?}"
            );
            let d = diags("fn f(x: Number) -> Number = g(x)\nfn g(x: Number) -> Number = f(x)");
            assert_eq!(
                d.iter().filter(|m| m.contains("recursive")).count(),
                2,
                "{d:?}"
            );
            let d = diags("kind k(a: Nobody) -> Int max default 1");
            assert!(d[0].contains("unknown type `Nobody`"), "{d:?}");
            let d = diags("kind k(a: Int) -> Credits max default 5 s");
            assert!(d[0].contains("expected Credits, found Duration"), "{d:?}");
        }
        // enum_literals_and_entity_fields: enum literals and entity fields are checked.
        {
            let src = "enum S { \"a\"\n \"b\" }\nentity A { field s: S\n field n: Credits }\n\
                       check \"c\" { for a in as\n require a.s == \"zz\" and a.n >= 0 and a.m == 1 }";
            let d = diags(src);
            assert!(
                d.iter()
                    .any(|m| m.contains("\"zz\" is not a value of enum S")),
                "{d:?}"
            );
            assert!(
                d.iter()
                    .any(|m| m.contains("dimension mismatch: Credits >= Number")),
                "{d:?}"
            );
            assert!(d.iter().any(|m| m.contains("A has no field `m`")), "{d:?}");
        }
        // machine_names_primes_and_assignments: machine names, primes, and assignments.
        {
            let src =
                "enum S { \"p\"\n \"q\" }\nmachine M {\n var s: S = \"p\"\n var n: Credits = 0 cr\n\
                       action A { when s == \"p\"\n s' = \"r\"\n n' = n + 1 }\n\
                       invariant \"i\" n' >= 0 cr\n fairness weak B\n}\n";
            let d = diags(src);
            assert!(
                d.iter()
                    .any(|m| m.contains("\"r\" is not a value of enum S")),
                "{d:?}"
            );
            assert!(
                d.iter()
                    .any(|m| m.contains("dimension mismatch: Credits + Number")),
                "{d:?}"
            );
            assert!(
                d.iter()
                    .any(|m| m.contains("only allowed in actions and transition")),
                "{d:?}"
            );
            assert!(
                d.iter().any(|m| m.contains("`B`, which is not an action")),
                "{d:?}"
            );
        }
    }
}
