//! Syntax tree for the grammar of docs/jhrl.md section 17.

use crate::num::{Dim, Num};
use crate::span::Span;

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Coalesce,
    Implies,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    Tol,
    Add,
    Sub,
    Union,
    Minus,
    Mul,
    Div,
    Inter,
    Pow,
    LeadsTo,
    Until,
    /// `a .. b`: the integers (in base units) from a to b.
    Range,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        use BinOp::*;
        match self {
            Coalesce => "??",
            Implies => "=>",
            Or => "or",
            And => "and",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            In => "in",
            Tol => "±",
            Add => "+",
            Sub => "-",
            Union => "union",
            Minus => "minus",
            Mul => "*",
            Div => "/",
            Inter => "inter",
            Pow => "^",
            LeadsTo => "leads_to",
            Until => "until",
            Range => "..",
        }
    }
    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Always,
    Eventually,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompKind {
    List,
    Set,
    Map,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quant {
    All,
    Any,
    One,
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

/// A generator `x in S` or `(a, b) in S`.
#[derive(Clone, Debug)]
pub struct Gen {
    pub names: Vec<String>,
    pub tuple: bool,
    pub src: Expr,
    pub span: Span,
}

/// One step of a `with` path: `[key]` or `.field`.
#[derive(Clone, Debug)]
pub enum PathSeg {
    Index(Expr),
    Field(String),
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Num(Num, Dim),
    Str(std::sync::Arc<str>),
    Bool(bool),
    NoneLit,
    Name(String),
    Primed(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
    Set(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Record(Vec<(String, Expr)>),
    Tuple(Vec<Expr>),
    Comp { kind: CompKind, gens: Vec<Gen>, filter: Option<Box<Expr>>, key: Box<Expr>, val: Option<Box<Expr>> },
    Quant(Quant, Vec<Gen>, Box<Expr>),
    /// `base with [k].f = v`: a copy of base with the value at the path replaced.
    With(Box<Expr>, Vec<PathSeg>, Box<Expr>),
    Is(Box<Expr>, String),
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    Named(String, Vec<TypeExpr>, Span),
    Div(Box<TypeExpr>, Box<TypeExpr>),
    Record(Vec<(String, TypeExpr)>),
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(_, _, s) => *s,
            TypeExpr::Div(a, b) => a.span().to(b.span()),
            TypeExpr::Record(f) => f.first().map(|x| x.1.span()).unwrap_or_default(),
        }
    }
    pub fn name(&self) -> Option<&str> {
        match self {
            TypeExpr::Named(n, _, _) => Some(n),
            _ => None,
        }
    }
}

impl std::fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeExpr::Named(n, args, _) if args.is_empty() => write!(f, "{n}"),
            TypeExpr::Named(n, args, _) => {
                write!(f, "{n}<")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ">")
            }
            TypeExpr::Div(a, b) => write!(f, "{a}/{b}"),
            TypeExpr::Record(fs) => {
                write!(f, "{{")?;
                for (i, (n, t)) in fs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{n}: {t}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

/// A string with `{expr}` interpolations (message, fix, flag).
#[derive(Clone, Debug)]
pub struct Interp {
    pub raw: String,
    pub parts: Vec<InterpPart>,
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Module {
    pub file: u32,
    pub pack: Option<PackDecl>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub struct PackDecl {
    pub name: String,
    pub version: Option<String>,
    pub locked: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Import(String, Span),
    Param(ParamDef, Option<String>),
    Let(String, Expr, Span),
    Fn(FnDecl),
    Table(TableDecl),
    Rel(RelDecl),
    Enum(EnumDecl),
    Entity(EntityDecl),
    Edge(EdgeDecl),
    Measure(MeasureDecl),
    Kind(KindDecl),
    Constraint(ConstraintDecl),
    Check(CheckDecl),
    Machine(MachineDecl),
    Model(ModelDecl),
    Mutant(MutantDecl),
    Witness(WitnessDecl),
    Scenario(ScenarioDecl),
}

impl Item {
    /// The name the item declares in its pack's namespace, if any.
    pub fn name(&self) -> Option<&str> {
        Some(match self {
            Item::Param(p, _) => &p.name,
            Item::Let(n, _, _) => n,
            Item::Fn(d) => &d.name,
            Item::Table(d) => &d.name,
            Item::Rel(d) => &d.name,
            Item::Enum(d) => &d.name,
            Item::Entity(d) => &d.name,
            Item::Edge(d) => &d.name,
            Item::Measure(d) => &d.name,
            Item::Kind(d) => &d.name,
            Item::Machine(d) => &d.name,
            _ => return None,
        })
    }
    pub fn span(&self) -> Span {
        match self {
            Item::Import(_, s) | Item::Let(_, _, s) => *s,
            Item::Param(p, _) => p.span,
            Item::Fn(d) => d.span,
            Item::Table(d) => d.span,
            Item::Rel(d) => d.span,
            Item::Enum(d) => d.span,
            Item::Entity(d) => d.span,
            Item::Edge(d) => d.span,
            Item::Measure(d) => d.span,
            Item::Kind(d) => d.span,
            Item::Constraint(d) => d.span,
            Item::Check(d) => d.span,
            Item::Machine(d) => d.span,
            Item::Model(d) => d.span,
            Item::Mutant(d) => d.span,
            Item::Witness(d) => d.span,
            Item::Scenario(d) => d.span,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub ret: TypeExpr,
    pub doc: Option<String>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TableDecl {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub ret: TypeExpr,
    pub axes: Vec<(String, String, Span)>,
    pub out_of_range: Option<String>,
    pub cites: Vec<String>,
    pub implements: Vec<String>,
    pub rows: Vec<(Vec<Expr>, Expr)>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct RelBody {
    pub gens: Vec<Gen>,
    pub filter: Option<Expr>,
    pub yields: Expr,
}

#[derive(Clone, Debug)]
pub struct RelDecl {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub bodies: Vec<RelBody>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub doc: Option<String>,
    pub members: Vec<(String, Option<String>, Span)>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub optional: bool,
    pub computed: bool,
    pub default: Option<Expr>,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EntityDecl {
    pub name: String,
    pub parent: Option<String>,
    pub doc: Option<String>,
    pub fields: Vec<FieldDecl>,
    /// ("store" | "view", table)
    pub store: Option<(String, String)>,
    pub collection: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EdgeDecl {
    pub name: String,
    pub from: TypeExpr,
    pub to: TypeExpr,
    pub doc: Option<String>,
    pub fields: Vec<FieldDecl>,
    pub store: Option<(String, String)>,
    pub temporal: bool,
    pub collection: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct MeasureDecl {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub ret: TypeExpr,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Min,
    Max,
    Exact,
    Prefer,
}

#[derive(Clone, Debug)]
pub struct KindDecl {
    pub name: String,
    pub params: Vec<ParamDef>,
    pub ret: TypeExpr,
    pub direction: Direction,
    pub default: Option<Expr>,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ConstraintDecl {
    pub title: Option<String>,
    pub kind: String,
    pub args: Vec<String>,
    pub wheres: Vec<Expr>,
    pub value: Option<Expr>,
    pub priority: i64,
    pub floor: bool,
    pub cites: Vec<String>,
    pub implements: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Clause {
    For(Vec<Gen>),
    Where(Expr),
    Let(String, Expr),
    Require(Expr, Option<Expr>),
    Severity(String),
    Message(Interp),
    Flag(Interp, Expr),
    Violation(String),
    Cite(String),
    Implements(Vec<String>),
    Fix(Interp, Option<(Expr, Expr)>),
    OnChange(Vec<Expr>),
}

#[derive(Clone, Debug)]
pub struct CheckDecl {
    pub title: String,
    pub clauses: Vec<Clause>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum VarInit {
    None,
    Eq(Expr),
    In(Expr),
}

#[derive(Clone, Debug)]
pub enum InvBody {
    Expr(Expr),
    Clauses(Vec<Clause>),
}

#[derive(Clone, Debug)]
pub enum MItem {
    Const { name: String, ty: TypeExpr, default: Option<Expr>, span: Span },
    Var { name: String, ty: TypeExpr, optional: bool, init: VarInit, span: Span },
    Init(Vec<Stmt>, Span),
    /// `let name(params) = e`: a definition that reads the current state (TLA's `Op == …`).
    Def { name: String, params: Vec<String>, body: Expr, span: Span },
    Action(ActionDecl),
    Invariant { name: String, body: InvBody, implements: Vec<String>, span: Span },
    Transition { name: String, expr: Expr, implements: Vec<String>, span: Span },
    Property { name: String, expr: Expr, implements: Vec<String>, span: Span },
    Fairness { strong: bool, actions: Vec<String>, span: Span },
    Terminal(Expr),
}

#[derive(Clone, Debug)]
pub enum ADom {
    Type(TypeExpr),
    In(Expr),
}

#[derive(Clone, Debug)]
pub struct AParam {
    pub name: String,
    pub dom: ADom,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ActionDecl {
    pub name: String,
    pub params: Vec<AParam>,
    pub doc: Option<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    When(Expr),
    Assign(String, Expr),
    Let(String, Expr),
    Pick(String, Expr),
    Either(Vec<Vec<Stmt>>),
    /// `if c { … } else { … }`; a missing else does nothing.
    If(Expr, Vec<Stmt>, Vec<Stmt>),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct MachineDecl {
    pub name: String,
    pub doc: Option<String>,
    pub items: Vec<MItem>,
    /// `machine M = A || B sync on X`: (components, sync actions). Parsed, not yet checked.
    pub compose: Option<(Vec<String>, Vec<String>)>,
    pub span: Span,
}

impl MachineDecl {
    pub fn actions(&self) -> impl Iterator<Item = &ActionDecl> {
        self.items.iter().filter_map(|i| if let MItem::Action(a) = i { Some(a) } else { None })
    }
}

#[derive(Clone, Debug)]
pub enum MOpt {
    Const(String, Expr),
    Values(String, Expr),
    Domain(String, Expr, Expr),
    Symmetry(String),
    Constraint(Expr),
    Terminal(Expr),
    Check(Vec<String>),
    DeadlockAllowed,
}

#[derive(Clone, Debug)]
pub struct ModelDecl {
    pub name: String,
    pub machine: String,
    pub opts: Vec<MOpt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum MutOp {
    Replace { text: String, action: String, with: String, span: Span },
    DropWhen { n: usize, action: String, span: Span },
    RemoveAction { action: String, span: Span },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    Violation(String),
    Deadlock,
    Property(String),
}

#[derive(Clone, Debug)]
pub struct MutantDecl {
    pub name: String,
    pub machine: String,
    pub ops: Vec<MutOp>,
    pub expect: Expect,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct WitnessDecl {
    pub name: String,
    pub machine: String,
    pub model: Option<String>,
    pub expr: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum SStepKind {
    Fixture(String),
    Given(Vec<(String, Expr)>),
    When(Expr),
    Then(Expr),
    Rejected(Option<String>),
}

#[derive(Clone, Debug)]
pub struct SStep {
    pub kind: SStepKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ScenarioDecl {
    pub req: String,
    pub name: String,
    pub machine: Option<String>,
    pub steps: Vec<SStep>,
    pub span: Span,
}
