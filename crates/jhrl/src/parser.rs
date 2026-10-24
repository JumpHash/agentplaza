//! Recursive-descent parser with precedence climbing (docs/jhrl.md sections 8 and 17).
//!
//! Layout rules the grammar leaves implicit, made explicit here:
//! - Statements and clauses end where the next token can't continue the expression. A call
//!   `(`, an index `[`, and a binary `+` or `-` must be on the same line as what they continue,
//!   so a new line starting with `(`, `[` or `-` begins a new statement or table row.
//! - In `{ … }`, `ident:` starts a record, `expr:` (any other key) a map, otherwise a set.
//! - In an `enum`, a member's description is a second string on the same line.

use crate::ast::*;
use crate::lexer::{lex, Tok, Token, RESERVED};
use crate::span::{Diag, Span};

pub type PResult<T> = Result<T, Diag>;

pub fn parse_module(file: u32, text: &str) -> PResult<Module> {
    let toks = lex(file, text)?;
    let mut p = Parser {
        toks,
        pos: 0,
        file,
        depth: 0,
    };
    p.module()
}

/// Parses a standalone expression (used for interpolations and mutant replacement checks).
pub fn parse_expr_at(file: u32, text: &str, base: usize) -> PResult<Expr> {
    let mut toks = lex(file, text)?;
    for t in toks.iter_mut() {
        t.span.lo += base as u32;
        t.span.hi += base as u32;
    }
    let mut p = Parser {
        toks,
        pos: 0,
        file,
        depth: 0,
    };
    let e = p.expr()?;
    if !matches!(p.peek(), Tok::Eof) {
        return Err(p.err_here("unexpected text after the expression"));
    }
    Ok(e)
}

/// The deepest nesting of recursive expression parses the parser accepts (each level costs about 6 KiB of
/// stack in a release build and 40 KiB in a debug one, so 32 fits a default 2 MiB thread). Deeper input is refused
/// with a diagnostic instead of overflowing the stack.
const MAX_NESTING: u32 = 32;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    file: u32,
    /// Open recursive expression parses around the current position.
    depth: u32,
}

impl Parser {
    /// Runs one recursive expression parse a level deeper. The level is given back on every exit,
    /// error or not, so the caller sees the depth it entered with.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        if self.depth >= MAX_NESTING {
            return Err(self.err_here(format!(
                "expression nested deeper than {MAX_NESTING} levels"
            )));
        }
        self.depth += 1;
        let out = parse(self);
        self.depth -= 1;
        out
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Prime => "`'`".into(),
        Tok::Str(s) => format!("string \"{s}\""),
        Tok::Num { text, .. } => format!("number `{text}`"),
        Tok::ReqId(s) => format!("requirement id `{s}`"),
        Tok::Sym(s) => format!("`{s}`"),
        Tok::Eof => "end of file".into(),
    }
}

impl Parser {
    // ------------------------------------------------------------------ token helpers
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }
    fn tok(&self) -> &Token {
        &self.toks[self.pos]
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn err_here(&self, msg: impl Into<String>) -> Diag {
        Diag::error(self.span(), msg)
    }
    fn expected(&self, what: &str) -> Diag {
        self.err_here(format!("expected {what}, found {}", describe(self.peek())))
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }
    fn is_sym_at(&self, n: usize, s: &str) -> bool {
        matches!(self.peek_at(n), Tok::Sym(x) if *x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &str) -> PResult<Span> {
        if self.is_sym(s) {
            Ok(self.bump().span)
        } else {
            Err(self.expected(&format!("`{s}`")))
        }
    }
    fn is_kw(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == w)
    }
    fn is_kw_at(&self, n: usize, w: &str) -> bool {
        matches!(self.peek_at(n), Tok::Ident(x) if x == w)
    }
    fn eat_kw(&mut self, w: &str) -> bool {
        if self.is_kw(w) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_kw(&mut self, w: &str) -> PResult<Span> {
        if self.is_kw(w) {
            Ok(self.bump().span)
        } else {
            Err(self.expected(&format!("`{w}`")))
        }
    }
    fn ident(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) if !RESERVED.contains(&s.as_str()) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.expected("a name")),
        }
    }
    fn string(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.expected("a string")),
        }
    }
    fn opt_string(&mut self) -> Option<String> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Some(s)
            }
            _ => None,
        }
    }
    fn reqid(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::ReqId(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.expected("a requirement id like AP-073.R4")),
        }
    }
    fn reqids(&mut self) -> PResult<Vec<String>> {
        let mut v = vec![self.reqid()?];
        while self.eat_sym(",") {
            v.push(self.reqid()?);
        }
        Ok(v)
    }
    fn dotted(&mut self) -> PResult<String> {
        let mut s = self.ident()?;
        while self.is_sym(".") {
            self.bump();
            s.push('.');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }
    fn on_new_line(&self) -> bool {
        self.tok().nl_before
    }

    // ------------------------------------------------------------------ module and items
    fn module(&mut self) -> PResult<Module> {
        let mut pack = None;
        if self.is_kw("pack") {
            let lo = self.bump().span;
            let name = self.dotted()?;
            let version = if self.eat_kw("version") {
                Some(self.string()?)
            } else {
                None
            };
            let locked = self.eat_kw("locked");
            pack = Some(PackDecl {
                name,
                version,
                locked,
                span: lo.to(self.prev_span()),
            });
        }
        let mut items = Vec::new();
        while !matches!(self.peek(), Tok::Eof) {
            items.push(self.item()?);
        }
        Ok(Module {
            file: self.file,
            pack,
            items,
        })
    }

    fn item(&mut self) -> PResult<Item> {
        let lo = self.span();
        let kw = match self.peek() {
            Tok::Ident(s) => s.clone(),
            _ => return Err(self.expected("a declaration (fn, kind, machine, check …)")),
        };
        let item = match kw.as_str() {
            "import" => {
                self.bump();
                Item::Import(self.dotted()?, lo.to(self.prev_span()))
            }
            "pack" => return Err(self.err_here("`pack` must be the first line of a file")),
            "param" => {
                self.bump();
                let name = self.ident()?;
                self.expect_sym(":")?;
                let ty = self.ty()?;
                let default = if self.eat_sym("=") {
                    Some(self.expr()?)
                } else {
                    None
                };
                let doc = self.opt_string();
                Item::Param(
                    ParamDef {
                        name,
                        ty,
                        default,
                        span: lo.to(self.prev_span()),
                    },
                    doc,
                )
            }
            "let" => {
                self.bump();
                let name = self.ident()?;
                self.expect_sym("=")?;
                let e = self.expr()?;
                Item::Let(name, e, lo.to(self.prev_span()))
            }
            "fn" => {
                self.bump();
                let name = self.ident()?;
                let params = self.params()?;
                self.expect_sym("->")?;
                let ret = self.ty()?;
                let doc = self.opt_string();
                self.expect_sym("=")?;
                let body = self.expr()?;
                Item::Fn(FnDecl {
                    name,
                    params,
                    ret,
                    doc,
                    body,
                    span: lo.to(self.prev_span()),
                })
            }
            "table" => self.table()?,
            "rel" => self.rel()?,
            "enum" => self.enum_decl()?,
            "entity" => self.entity()?,
            "edge" => self.edge()?,
            "measure" => {
                self.bump();
                let name = self.ident()?;
                let params = self.params()?;
                self.expect_sym("->")?;
                let ret = self.ty()?;
                let doc = self.opt_string();
                Item::Measure(MeasureDecl {
                    name,
                    params,
                    ret,
                    doc,
                    span: lo.to(self.prev_span()),
                })
            }
            "kind" => {
                self.bump();
                let name = self.ident()?;
                let params = self.params()?;
                self.expect_sym("->")?;
                let ret = self.ty()?;
                let direction = match self.peek() {
                    Tok::Ident(s) if s == "min" => Direction::Min,
                    Tok::Ident(s) if s == "max" => Direction::Max,
                    Tok::Ident(s) if s == "exact" => Direction::Exact,
                    Tok::Ident(s) if s == "prefer" => Direction::Prefer,
                    _ => return Err(self.expected("a direction: min, max, exact or prefer")),
                };
                self.bump();
                let default = if self.eat_kw("default") {
                    Some(self.expr()?)
                } else {
                    None
                };
                let doc = self.opt_string();
                Item::Kind(KindDecl {
                    name,
                    params,
                    ret,
                    direction,
                    default,
                    doc,
                    span: lo.to(self.prev_span()),
                })
            }
            "constraint" => self.constraint()?,
            "check" => {
                self.bump();
                let title = self.string()?;
                self.expect_sym("{")?;
                let clauses = self.clauses()?;
                self.expect_sym("}")?;
                Item::Check(CheckDecl {
                    title,
                    clauses,
                    span: lo.to(self.prev_span()),
                })
            }
            "machine" => self.machine()?,
            "model" => self.model()?,
            "mutant" => self.mutant()?,
            "witness" => {
                self.bump();
                let name = self.string()?;
                self.expect_kw("of")?;
                let machine = self.ident()?;
                let model = if self.eat_kw("in") {
                    Some(self.string()?)
                } else {
                    None
                };
                self.expect_sym("{")?;
                self.expect_kw("reachable")?;
                let expr = self.expr()?;
                self.expect_sym("}")?;
                Item::Witness(WitnessDecl {
                    name,
                    machine,
                    model,
                    expr,
                    span: lo.to(self.prev_span()),
                })
            }
            "scenario" => self.scenario()?,
            "operation" => self.operation()?,
            "law" => self.law()?,
            "fixture" => self.fixture()?,
            "authz" => self.authz()?,
            _ => return Err(self.err_here(format!("unknown declaration `{kw}`"))),
        };
        Ok(item)
    }

    fn params(&mut self) -> PResult<Vec<ParamDef>> {
        self.expect_sym("(")?;
        let mut v = Vec::new();
        if !self.is_sym(")") {
            loop {
                let lo = self.span();
                let name = self.ident()?;
                self.expect_sym(":")?;
                let ty = self.ty()?;
                let default = if self.eat_sym("=") {
                    Some(self.expr()?)
                } else {
                    None
                };
                v.push(ParamDef {
                    name,
                    ty,
                    default,
                    span: lo.to(self.prev_span()),
                });
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }

    fn ty(&mut self) -> PResult<TypeExpr> {
        let mut t = self.ty_atom()?;
        while self.is_sym("/") {
            self.bump();
            let r = self.ty_atom()?;
            t = TypeExpr::Div(Box::new(t), Box::new(r));
        }
        Ok(t)
    }

    fn ty_atom(&mut self) -> PResult<TypeExpr> {
        if self.eat_sym("{") {
            let mut fs = Vec::new();
            loop {
                let n = self.ident()?;
                self.expect_sym(":")?;
                fs.push((n, self.ty()?));
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("}")?;
            return Ok(TypeExpr::Record(fs));
        }
        let lo = self.span();
        let name = self.ident()?;
        let mut args = Vec::new();
        if self.eat_sym("<") {
            loop {
                args.push(self.ty()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(">")?;
        }
        Ok(TypeExpr::Named(name, args, lo.to(self.prev_span())))
    }

    fn table(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        let params = self.params()?;
        self.expect_sym("->")?;
        let ret = self.ty()?;
        self.expect_sym("{")?;
        let mut t = TableDecl {
            name,
            params,
            ret,
            axes: vec![],
            out_of_range: None,
            cites: vec![],
            implements: vec![],
            rows: vec![],
            span: lo,
        };
        while !self.is_sym("}") {
            if self.is_kw("axes") && self.is_sym_at(2, ":") {
                self.bump();
                loop {
                    let s = self.span();
                    let a = self.ident()?;
                    self.expect_sym(":")?;
                    let k = self.ident()?;
                    if !["linear", "log", "floor", "ceil", "exact"].contains(&k.as_str()) {
                        return Err(Diag::error(
                            self.prev_span(),
                            "an axis is linear, log, floor, ceil or exact",
                        ));
                    }
                    t.axes.push((a, k, s));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            } else if self.eat_kw("out_of_range") {
                let k = self.ident()?;
                if !["error", "clamp", "extrapolate"].contains(&k.as_str()) {
                    return Err(Diag::error(
                        self.prev_span(),
                        "out_of_range is error, clamp or extrapolate",
                    ));
                }
                t.out_of_range = Some(k);
            } else if self.eat_kw("cite") {
                t.cites.push(self.string()?);
            } else if self.eat_kw("implements") {
                t.implements.extend(self.reqids()?);
            } else {
                let mut keys = vec![self.expr_bp_or()?];
                while self.eat_sym(",") {
                    keys.push(self.expr_bp_or()?);
                }
                self.expect_sym("=>")?;
                let v = self.expr()?;
                self.eat_sym(";");
                t.rows.push((keys, v));
            }
        }
        self.expect_sym("}")?;
        t.span = lo.to(self.prev_span());
        Ok(Item::Table(t))
    }

    fn rel(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        let params = self.params()?;
        self.expect_sym("{")?;
        let mut bodies = Vec::new();
        loop {
            self.expect_kw("for")?;
            let gens = self.gens()?;
            let filter = if self.eat_kw("where") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_kw("yield")?;
            let yields = self.expr()?;
            bodies.push(RelBody {
                gens,
                filter,
                yields,
            });
            if !self.eat_sym("|") {
                break;
            }
        }
        self.expect_sym("}")?;
        Ok(Item::Rel(RelDecl {
            name,
            params,
            bodies,
            span: lo.to(self.prev_span()),
        }))
    }

    fn enum_decl(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        let doc = self.opt_string();
        self.expect_sym("{")?;
        let mut members = Vec::new();
        while !self.is_sym("}") {
            let s = self.span();
            let m = self.string()?;
            let d = if matches!(self.peek(), Tok::Str(_)) && !self.on_new_line() {
                self.opt_string()
            } else {
                None
            };
            members.push((m, d, s));
            self.eat_sym(",");
        }
        self.expect_sym("}")?;
        Ok(Item::Enum(EnumDecl {
            name,
            doc,
            members,
            span: lo.to(self.prev_span()),
        }))
    }

    fn field(&mut self) -> PResult<FieldDecl> {
        let lo = self.expect_kw("field")?;
        let name = self.ident()?;
        self.expect_sym(":")?;
        let ty = self.ty()?;
        let (mut optional, mut computed) = (false, false);
        loop {
            if self.eat_kw("optional") {
                optional = true;
            } else if self.eat_kw("computed") {
                computed = true;
            } else {
                break;
            }
        }
        let default = if self.eat_sym("=") {
            Some(self.expr()?)
        } else {
            None
        };
        let doc = self.opt_string();
        Ok(FieldDecl {
            name,
            ty,
            optional,
            computed,
            default,
            doc,
            span: lo.to(self.prev_span()),
        })
    }

    fn entity(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        let parent = if self.eat_sym(":") {
            Some(self.ident()?)
        } else {
            None
        };
        let doc = self.opt_string();
        self.expect_sym("{")?;
        let mut e = EntityDecl {
            name,
            parent,
            doc,
            fields: vec![],
            store: None,
            collection: None,
            span: lo,
        };
        while !self.is_sym("}") {
            if self.is_kw("field") {
                e.fields.push(self.field()?);
            } else if self.is_kw("store") || self.is_kw("view") {
                let k = self.ident()?;
                e.store = Some((k, self.string()?));
            } else if self.eat_kw("collection") {
                e.collection = Some(self.string()?);
            } else {
                return Err(self.expected("field, store, view or collection"));
            }
        }
        self.expect_sym("}")?;
        e.span = lo.to(self.prev_span());
        Ok(Item::Entity(e))
    }

    fn edge(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        self.expect_sym("(")?;
        self.expect_kw("from")?;
        self.expect_sym(":")?;
        let from = self.ty()?;
        self.expect_sym(",")?;
        self.expect_kw("to")?;
        self.expect_sym(":")?;
        let to = self.ty()?;
        self.expect_sym(")")?;
        let doc = self.opt_string();
        self.expect_sym("{")?;
        let mut e = EdgeDecl {
            name,
            from,
            to,
            doc,
            fields: vec![],
            store: None,
            temporal: false,
            collection: None,
            span: lo,
        };
        while !self.is_sym("}") {
            if self.is_kw("field") {
                e.fields.push(self.field()?);
            } else if self.is_kw("store") || self.is_kw("view") {
                let k = self.ident()?;
                e.store = Some((k, self.string()?));
            } else if self.eat_kw("temporal") {
                e.temporal = true;
            } else if self.eat_kw("collection") {
                e.collection = Some(self.string()?);
            } else {
                return Err(self.expected("field, store, view, temporal or collection"));
            }
        }
        self.expect_sym("}")?;
        e.span = lo.to(self.prev_span());
        Ok(Item::Edge(e))
    }

    fn constraint(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let title = self.opt_string();
        let kind = self.ident()?;
        self.expect_sym("(")?;
        let mut args = Vec::new();
        if !self.is_sym(")") {
            loop {
                args.push(self.ident()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")")?;
        self.expect_sym("{")?;
        let mut c = ConstraintDecl {
            title,
            kind,
            args,
            wheres: vec![],
            value: None,
            priority: 0,
            floor: false,
            cites: vec![],
            implements: vec![],
            span: lo,
        };
        while !self.is_sym("}") {
            if self.eat_kw("where") {
                c.wheres.push(self.expr()?);
            } else if self.eat_kw("value") {
                if c.value.is_some() {
                    return Err(Diag::error(
                        self.prev_span(),
                        "a constraint rule has one value",
                    ));
                }
                c.value = Some(self.expr()?);
            } else if self.eat_kw("priority") {
                let neg = self.eat_sym("-");
                match self.peek().clone() {
                    Tok::Num { value, dim, .. } if dim.is_none() && value.as_int().is_some() => {
                        self.bump();
                        let v = value.as_int().unwrap() as i64;
                        c.priority = if neg { -v } else { v };
                    }
                    _ => return Err(self.expected("an integer priority")),
                }
            } else if self.eat_kw("floor") {
                c.floor = true;
            } else if self.eat_kw("cite") {
                c.cites.push(self.string()?);
            } else if self.eat_kw("implements") {
                c.implements.extend(self.reqids()?);
            } else if self.is_kw("for") {
                return Err(self.err_here("constraint rules contain no `for` (section 9.7)"));
            } else {
                return Err(self.expected("where, value, priority, floor, cite or implements"));
            }
        }
        self.expect_sym("}")?;
        c.span = lo.to(self.prev_span());
        Ok(Item::Constraint(c))
    }

    fn interp(&mut self) -> PResult<Interp> {
        let span = self.span();
        let raw = self.string()?;
        let mut parts = Vec::new();
        let mut text = String::new();
        let bytes: Vec<char> = raw.chars().collect();
        let mut i = 0;
        let mut byte_off = 0usize;
        while i < bytes.len() {
            let c = bytes[i];
            if c == '{' && bytes.get(i + 1) == Some(&'{') {
                text.push('{');
                i += 2;
                byte_off += 2;
            } else if c == '}' && bytes.get(i + 1) == Some(&'}') {
                text.push('}');
                i += 2;
                byte_off += 2;
            } else if c == '{' {
                let mut depth = 1;
                let mut j = i + 1;
                let mut in_str = false;
                while j < bytes.len() {
                    match bytes[j] {
                        '"' => in_str = !in_str,
                        '{' if !in_str => depth += 1,
                        '}' if !in_str => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err(Diag::error(
                        span,
                        "unclosed `{` in an interpolated string (use `{{` for a brace)",
                    ));
                }
                if !text.is_empty() {
                    parts.push(InterpPart::Text(std::mem::take(&mut text)));
                }
                let src: String = bytes[i + 1..j].iter().collect();
                let base = span.lo as usize + 1 + byte_off + 1;
                parts.push(InterpPart::Expr(parse_expr_at(self.file, &src, base)?));
                byte_off += src.len() + 2;
                i = j + 1;
            } else {
                text.push(c);
                byte_off += c.len_utf8();
                i += 1;
            }
        }
        if !text.is_empty() {
            parts.push(InterpPart::Text(text));
        }
        Ok(Interp { raw, parts })
    }

    fn clauses(&mut self) -> PResult<Vec<Clause>> {
        let mut v = Vec::new();
        while !self.is_sym("}") {
            let c = if self.eat_kw("for") {
                Clause::For(self.gens()?)
            } else if self.eat_kw("where") {
                Clause::Where(self.expr()?)
            } else if self.eat_kw("let") {
                let n = self.ident()?;
                self.expect_sym("=")?;
                Clause::Let(n, self.expr()?)
            } else if self.eat_kw("require") {
                let e = self.expr()?;
                let m = if self.eat_kw("margin") {
                    Some(self.expr()?)
                } else {
                    None
                };
                Clause::Require(e, m)
            } else if self.eat_kw("severity") {
                let s = self.ident()?;
                if !["error", "warning", "info"].contains(&s.as_str()) {
                    return Err(Diag::error(
                        self.prev_span(),
                        "severity is error, warning or info",
                    ));
                }
                Clause::Severity(s)
            } else if self.eat_kw("message") {
                Clause::Message(self.interp()?)
            } else if self.eat_kw("flag") {
                let s = self.interp()?;
                self.expect_kw("on")?;
                Clause::Flag(s, self.expr()?)
            } else if self.eat_kw("violation") {
                Clause::Violation(self.string()?)
            } else if self.eat_kw("cite") {
                Clause::Cite(self.string()?)
            } else if self.eat_kw("implements") {
                Clause::Implements(self.reqids()?)
            } else if self.eat_kw("fix") {
                let s = self.interp()?;
                let set = if self.eat_kw("set") {
                    let path = self.postfix()?;
                    self.expect_sym("=")?;
                    Some((path, self.expr()?))
                } else {
                    None
                };
                Clause::Fix(s, set)
            } else if self.is_kw("on") && self.is_kw_at(1, "change") {
                self.bump();
                self.bump();
                self.expect_kw("of")?;
                let mut paths = vec![self.postfix()?];
                while self.eat_sym(",") {
                    paths.push(self.postfix()?);
                }
                Clause::OnChange(paths)
            } else {
                return Err(
                    self.expected("a check clause (for, where, let, require, severity, message …)")
                );
            };
            v.push(c);
        }
        Ok(v)
    }

    fn machine(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.ident()?;
        if self.eat_sym("=") {
            let mut parts = vec![self.ident()?];
            while self.eat_sym("||") {
                parts.push(self.ident()?);
            }
            let mut sync = Vec::new();
            if self.eat_kw("sync") {
                self.expect_kw("on")?;
                loop {
                    sync.push(self.ident()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            }
            return Ok(Item::Machine(MachineDecl {
                name,
                doc: None,
                items: vec![],
                compose: Some((parts, sync)),
                span: lo.to(self.prev_span()),
            }));
        }
        let doc = self.opt_string();
        self.expect_sym("{")?;
        let mut items = Vec::new();
        while !self.is_sym("}") {
            items.push(self.mitem()?);
        }
        self.expect_sym("}")?;
        Ok(Item::Machine(MachineDecl {
            name,
            doc,
            items,
            compose: None,
            span: lo.to(self.prev_span()),
        }))
    }

    fn mitem(&mut self) -> PResult<MItem> {
        let lo = self.span();
        if self.eat_kw("const") {
            let name = self.ident()?;
            self.expect_sym(":")?;
            let ty = self.ty()?;
            let default = if self.eat_sym("=") {
                Some(self.expr()?)
            } else {
                None
            };
            return Ok(MItem::Const {
                name,
                ty,
                default,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("var") {
            let name = self.ident()?;
            self.expect_sym(":")?;
            let ty = self.ty()?;
            let optional = self.eat_kw("optional");
            let init = if self.eat_sym("=") {
                VarInit::Eq(self.expr()?)
            } else if self.eat_kw("in") {
                VarInit::In(self.expr()?)
            } else {
                VarInit::None
            };
            return Ok(MItem::Var {
                name,
                ty,
                optional,
                init,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("init") {
            let b = self.block(true)?;
            return Ok(MItem::Init(b, lo.to(self.prev_span())));
        }
        if self.eat_kw("let") {
            let name = self.ident()?;
            let mut params = Vec::new();
            if self.eat_sym("(") {
                loop {
                    params.push(self.ident()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
            }
            self.expect_sym("=")?;
            let body = self.expr()?;
            return Ok(MItem::Def {
                name,
                params,
                body,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("action") {
            let name = self.ident()?;
            let mut params = Vec::new();
            if self.eat_sym("(") {
                loop {
                    let ps = self.span();
                    let n = self.ident()?;
                    let dom = if self.eat_sym(":") {
                        ADom::Type(self.ty()?)
                    } else if self.eat_kw("in") {
                        ADom::In(self.expr()?)
                    } else {
                        return Err(self.expected("`: Type` or `in Set`"));
                    };
                    params.push(AParam {
                        name: n,
                        dom,
                        span: ps.to(self.prev_span()),
                    });
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
            }
            let doc = self.opt_string();
            let body = self.block(false)?;
            return Ok(MItem::Action(ActionDecl {
                name,
                params,
                doc,
                body,
                span: lo.to(self.prev_span()),
            }));
        }
        if self.eat_kw("invariant") {
            let name = self.string()?;
            let body = if self.is_sym("{")
                && matches!(self.peek_at(1), Tok::Ident(w) if ["for", "require", "where", "let"].contains(&w.as_str()))
            {
                self.bump();
                let c = self.clauses()?;
                self.expect_sym("}")?;
                InvBody::Clauses(c)
            } else {
                InvBody::Expr(self.expr()?)
            };
            let implements = if self.eat_kw("implements") {
                self.reqids()?
            } else {
                vec![]
            };
            return Ok(MItem::Invariant {
                name,
                body,
                implements,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("transition") {
            let name = self.string()?;
            let expr = self.expr()?;
            let implements = if self.eat_kw("implements") {
                self.reqids()?
            } else {
                vec![]
            };
            return Ok(MItem::Transition {
                name,
                expr,
                implements,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("property") {
            let name = self.string()?;
            let expr = self.expr()?;
            let implements = if self.eat_kw("implements") {
                self.reqids()?
            } else {
                vec![]
            };
            return Ok(MItem::Property {
                name,
                expr,
                implements,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("fairness") {
            let strong = if self.eat_kw("strong") {
                true
            } else {
                self.expect_kw("weak")?;
                false
            };
            let mut actions = vec![self.ident()?];
            while self.eat_sym(",") {
                actions.push(self.ident()?);
            }
            return Ok(MItem::Fairness {
                strong,
                actions,
                span: lo.to(self.prev_span()),
            });
        }
        if self.eat_kw("terminal") {
            return Ok(MItem::Terminal(self.expr()?));
        }
        Err(self.expected(
            "const, var, init, action, invariant, transition, property, fairness or terminal",
        ))
    }

    fn block(&mut self, init: bool) -> PResult<Vec<Stmt>> {
        self.expect_sym("{")?;
        let mut v = Vec::new();
        while !self.is_sym("}") {
            v.push(self.stmt(init)?);
            while self.eat_sym(";") {}
        }
        self.expect_sym("}")?;
        Ok(v)
    }

    fn stmt(&mut self, init: bool) -> PResult<Stmt> {
        let lo = self.span();
        let kind = if self.eat_kw("when") {
            StmtKind::When(self.expr()?)
        } else if self.eat_kw("let") {
            let n = self.ident()?;
            self.expect_sym("=")?;
            StmtKind::Let(n, self.expr()?)
        } else if self.eat_kw("pick") {
            let n = self.ident()?;
            self.expect_kw("in")?;
            StmtKind::Pick(n, self.expr()?)
        } else if self.eat_kw("if") {
            let c = self.expr()?;
            let then = self.block(init)?;
            let els = if self.eat_kw("else") {
                if self.is_kw("if") {
                    vec![self.stmt(init)?]
                } else {
                    self.block(init)?
                }
            } else {
                vec![]
            };
            StmtKind::If(c, then, els)
        } else if self.eat_kw("either") {
            let mut alts = vec![self.block(init)?];
            while self.is_kw("or") && self.is_sym_at(1, "{") {
                self.bump();
                alts.push(self.block(init)?);
            }
            StmtKind::Either(alts)
        } else {
            let n = self.ident()?;
            let primed = matches!(self.peek(), Tok::Prime);
            if primed {
                self.bump();
            } else if !init {
                return Err(Diag::error(
                    lo,
                    format!("expected a statement; to assign the next value write `{n}' = …`"),
                ));
            }
            self.expect_sym("=")?;
            StmtKind::Assign(n, self.expr()?)
        };
        Ok(Stmt {
            kind,
            span: lo.to(self.prev_span()),
        })
    }

    fn model(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.string()?;
        self.expect_kw("of")?;
        let machine = self.ident()?;
        self.expect_sym("{")?;
        let mut opts = Vec::new();
        while !self.is_sym("}") {
            let o = if self.eat_kw("const") {
                let n = self.ident()?;
                self.expect_sym("=")?;
                MOpt::Const(n, self.expr()?)
            } else if self.eat_kw("values") {
                let n = self.ident()?;
                self.expect_sym("=")?;
                MOpt::Values(n, self.expr()?)
            } else if self.eat_kw("domain") {
                let n = self.ident()?;
                self.expect_sym("=")?;
                let e = self.expr()?;
                match e.kind {
                    ExprKind::Binary(BinOp::Range, a, b) => MOpt::Domain(n, *a, *b),
                    _ => return Err(Diag::error(e.span, "a domain is `lo .. hi`")),
                }
            } else if self.eat_kw("symmetry") {
                MOpt::Symmetry(self.ident()?)
            } else if self.eat_kw("constraint") {
                MOpt::Constraint(self.expr()?)
            } else if self.eat_kw("terminal") {
                MOpt::Terminal(self.expr()?)
            } else if self.eat_kw("check") {
                let mut v = vec![self.ident()?];
                while self.eat_sym(",") {
                    v.push(self.ident()?);
                }
                MOpt::Check(v)
            } else if self.eat_kw("deadlock") {
                self.expect_kw("allowed")?;
                MOpt::DeadlockAllowed
            } else {
                return Err(self.expected(
                    "const, values, domain, symmetry, constraint, terminal, check or deadlock",
                ));
            };
            opts.push(o);
        }
        self.expect_sym("}")?;
        Ok(Item::Model(ModelDecl {
            name,
            machine,
            opts,
            span: lo.to(self.prev_span()),
        }))
    }

    fn mutant(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.string()?;
        self.expect_kw("of")?;
        let machine = self.ident()?;
        self.expect_sym("{")?;
        let mut ops = Vec::new();
        let expect;
        loop {
            let s = self.span();
            if self.eat_kw("replace") {
                let text = self.string()?;
                self.expect_kw("in")?;
                let action = self.ident()?;
                self.expect_kw("with")?;
                let with = self.string()?;
                ops.push(MutOp::Replace {
                    text,
                    action,
                    with,
                    span: s.to(self.prev_span()),
                });
            } else if self.eat_kw("drop") {
                self.expect_kw("when")?;
                let n = match self.peek().clone() {
                    Tok::Num { value, .. } if value.as_int().is_some_and(|n| n >= 1) => {
                        self.bump();
                        value.as_int().unwrap() as usize
                    }
                    _ => return Err(self.expected("the guard's number, counting from 1")),
                };
                self.expect_kw("in")?;
                let action = self.ident()?;
                ops.push(MutOp::DropWhen {
                    n,
                    action,
                    span: s.to(self.prev_span()),
                });
            } else if self.eat_kw("remove") {
                self.expect_kw("action")?;
                let action = self.ident()?;
                ops.push(MutOp::RemoveAction {
                    action,
                    span: s.to(self.prev_span()),
                });
            } else if self.eat_kw("expect") {
                expect = if self.eat_kw("violation") {
                    Expect::Violation(self.string()?)
                } else if self.eat_kw("deadlock") {
                    Expect::Deadlock
                } else if self.eat_kw("property") {
                    Expect::Property(self.string()?)
                } else {
                    return Err(self.expected("violation \"name\", deadlock or property \"name\""));
                };
                break;
            } else {
                return Err(self.expected("replace, drop, remove or expect"));
            }
        }
        self.expect_sym("}")?;
        if ops.is_empty() {
            return Err(Diag::error(lo, "a mutant needs at least one change"));
        }
        Ok(Item::Mutant(MutantDecl {
            name,
            machine,
            ops,
            expect,
            span: lo.to(self.prev_span()),
        }))
    }

    fn scenario(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let req = self.reqid()?;
        let name = self.string()?;
        let machine = if self.eat_kw("of") {
            Some(self.ident()?)
        } else {
            None
        };
        self.expect_sym("{")?;
        let mut steps = Vec::new();
        while !self.is_sym("}") {
            let s = self.span();
            let kind = if self.eat_kw("given") {
                if self.eat_kw("fixture") {
                    SStepKind::Fixture(self.string()?)
                } else {
                    let mut v = Vec::new();
                    loop {
                        let n = self.ident()?;
                        self.expect_sym("=")?;
                        v.push((n, self.expr()?));
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    SStepKind::Given(v)
                }
            } else if self.eat_kw("when") {
                SStepKind::When(self.expr()?)
            } else if self.eat_kw("then") {
                if self.eat_kw("rejected") {
                    SStepKind::Rejected(if self.eat_kw("with") {
                        Some(self.string()?)
                    } else {
                        None
                    })
                } else {
                    SStepKind::Then(self.expr()?)
                }
            } else {
                return Err(self.expected("given, when or then"));
            };
            steps.push(SStep {
                kind,
                span: s.to(self.prev_span()),
            });
        }
        self.expect_sym("}")?;
        Ok(Item::Scenario(ScenarioDecl {
            req,
            name,
            machine,
            steps,
            span: lo.to(self.prev_span()),
        }))
    }

    // ------------------------------------------------------------------ generators
    // ------------------------------------------------------------------ authorization (spec 9.13-9.14)
    fn operation(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.dotted()?;
        self.expect_sym("(")?;
        self.expect_kw("principal")?;
        self.expect_sym(":")?;
        let mut principal = vec![self.ident()?];
        while self.eat_sym("|") {
            principal.push(self.ident()?);
        }
        self.expect_sym(",")?;
        self.expect_kw("resource")?;
        self.expect_sym(":")?;
        let resource = self.ident()?;
        self.expect_sym(",")?;
        self.expect_kw("context")?;
        self.expect_sym(":")?;
        self.expect_sym("{")?;
        let mut context = Vec::new();
        while !self.is_sym("}") {
            let f = self.ident()?;
            self.expect_sym(":")?;
            let ty = self.ty()?;
            let optional = self.eat_kw("optional");
            context.push((f, ty, optional));
            self.eat_sym(",");
        }
        self.expect_sym("}")?;
        self.expect_sym(")")?;
        Ok(Item::Operation(Operation {
            name,
            principal,
            resource,
            context,
            span: lo.to(self.prev_span()),
        }))
    }

    fn law(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let id = self.string()?;
        let title = self.string()?;
        self.expect_sym("{")?;
        let mut l = Law {
            id,
            title,
            tier: None,
            enforcement: None,
            severity: None,
            statement: None,
            rationale: None,
            effect: Effect::Permit,
            ops: Vec::new(),
            principal: None,
            resource: None,
            when: Vec::new(),
            unless: Vec::new(),
            project: None,
            expires: None,
            implements: Vec::new(),
            span: lo,
        };
        let mut effect = None;
        while !self.is_sym("}") {
            let kw = match self.peek().clone() {
                Tok::Ident(s) => s,
                _ => return Err(self.expected("a law clause")),
            };
            match kw.as_str() {
                "tier" => {
                    self.bump();
                    l.tier = Some(self.ident()?);
                }
                "enforcement" => {
                    self.bump();
                    l.enforcement = Some(self.ident()?);
                }
                "severity" => {
                    self.bump();
                    l.severity = Some(self.ident()?);
                }
                "statement" => {
                    self.bump();
                    l.statement = Some(self.string()?);
                }
                "rationale" => {
                    self.bump();
                    l.rationale = Some(self.string()?);
                }
                "permit" | "forbid" => {
                    self.bump();
                    effect = Some(if kw == "permit" {
                        Effect::Permit
                    } else {
                        Effect::Forbid
                    });
                    if self.eat_kw("any") {
                        l.ops.push("any".to_string());
                    } else {
                        l.ops.push(self.dotted()?);
                        while self.eat_sym(",") {
                            l.ops.push(self.dotted()?);
                        }
                    }
                }
                "principal" | "resource" => {
                    self.bump();
                    let sc = if self.eat_kw("is") {
                        Scope::Is(self.ty_atom()?)
                    } else if self.eat_sym("==") {
                        Scope::Eq(self.expr()?)
                    } else if self.eat_kw("in") {
                        Scope::In(self.expr()?)
                    } else {
                        return Err(self.expected("`is`, `==` or `in`"));
                    };
                    if kw == "principal" {
                        l.principal = Some(sc);
                    } else {
                        l.resource = Some(sc);
                    }
                }
                "when" => {
                    self.bump();
                    l.when.push(self.expr()?);
                }
                "unless" => {
                    self.bump();
                    l.unless.push(self.expr()?);
                }
                "project" => {
                    self.bump();
                    l.project = Some(self.string()?);
                }
                "expires" => {
                    self.bump();
                    l.expires = Some(self.expr()?);
                }
                "implements" => {
                    self.bump();
                    l.implements = self.reqids()?;
                }
                _ => return Err(self.err_here(format!("unknown law clause `{kw}`"))),
            }
        }
        self.expect_sym("}")?;
        let span = lo.to(self.prev_span());
        match effect {
            Some(e) if !l.ops.is_empty() => {
                l.effect = e;
            }
            _ => {
                return Err(Diag::error(
                    span,
                    "a law needs `permit` or `forbid` with its operations",
                ))
            }
        }
        l.span = span;
        Ok(Item::Law(l))
    }

    /// `Type("id")`, with a `{ field: expr, … }` block only where `fields_ok` (fixtures).
    fn entity_lit(&mut self, fields_ok: bool) -> PResult<EntityLit> {
        let lo = self.span();
        let ty = self.ident()?;
        self.expect_sym("(")?;
        let id = self.string()?;
        self.expect_sym(")")?;
        let mut fields = Vec::new();
        if fields_ok && self.eat_sym("{") {
            while !self.is_sym("}") {
                let f = self.ident()?;
                self.expect_sym(":")?;
                fields.push((f, self.expr()?));
                self.eat_sym(",");
            }
            self.expect_sym("}")?;
        }
        Ok(EntityLit {
            ty,
            id,
            fields,
            span: lo.to(self.prev_span()),
        })
    }

    fn fixture(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.string()?;
        let title = self.string()?;
        self.expect_sym("{")?;
        let mut entities = Vec::new();
        while !self.is_sym("}") {
            entities.push(self.entity_lit(true)?);
        }
        self.expect_sym("}")?;
        Ok(Item::Fixture(Fixture {
            name,
            title,
            entities,
            span: lo.to(self.prev_span()),
        }))
    }

    fn authz(&mut self) -> PResult<Item> {
        let lo = self.bump().span;
        let name = self.string()?;
        self.expect_kw("using")?;
        let fixture = self.string()?;
        self.expect_sym("{")?;
        let mut cases = Vec::new();
        while !self.is_sym("}") {
            let clo = self.span();
            let allow = if self.eat_kw("allow") {
                true
            } else if self.eat_kw("deny") {
                false
            } else {
                return Err(self.expected("`allow` or `deny`"));
            };
            let label = self.string()?;
            let subject = self.entity_lit(false)?;
            let op = self.dotted()?;
            let object = self.entity_lit(false)?;
            let record = self.expr()?;
            let law = if self.eat_kw("by") {
                Some(self.string()?)
            } else {
                None
            };
            cases.push(AuthzCase {
                allow,
                label,
                subject,
                op,
                object,
                record,
                law,
                span: clo.to(self.prev_span()),
            });
        }
        self.expect_sym("}")?;
        Ok(Item::Authz(Authz {
            name,
            fixture,
            cases,
            span: lo.to(self.prev_span()),
        }))
    }

    fn gens(&mut self) -> PResult<Vec<Gen>> {
        let mut v = vec![self.gen()?];
        while self.is_sym(",") && self.gen_follows(1) {
            self.bump();
            v.push(self.gen()?);
        }
        Ok(v)
    }

    /// Whether a generator starts `n` tokens ahead: `x in` or `(a, b) in`.
    fn gen_follows(&self, n: usize) -> bool {
        match self.peek_at(n) {
            Tok::Ident(_) => self.is_kw_at(n + 1, "in"),
            Tok::Sym("(") => {
                let mut k = n + 1;
                while matches!(self.peek_at(k), Tok::Ident(_)) && self.is_sym_at(k + 1, ",") {
                    k += 2;
                }
                matches!(self.peek_at(k), Tok::Ident(_))
                    && self.is_sym_at(k + 1, ")")
                    && self.is_kw_at(k + 2, "in")
            }
            _ => false,
        }
    }

    fn gen(&mut self) -> PResult<Gen> {
        let lo = self.span();
        let (names, tuple) = if self.eat_sym("(") {
            let mut v = vec![self.ident()?];
            while self.eat_sym(",") {
                v.push(self.ident()?);
            }
            self.expect_sym(")")?;
            (v, true)
        } else {
            (vec![self.ident()?], false)
        };
        self.expect_kw("in")?;
        let src = self.expr_tol()?;
        Ok(Gen {
            names,
            tuple,
            src,
            span: lo.to(self.prev_span()),
        })
    }

    // ------------------------------------------------------------------ expressions
    pub fn expr(&mut self) -> PResult<Expr> {
        let mut e = self.temporal()?;
        while self.eat_kw("with") {
            let mut target = Vec::new();
            loop {
                if self.eat_sym("[") {
                    let k = self.expr()?;
                    self.expect_sym("]")?;
                    target.push(PathSeg::Index(k));
                } else if self.eat_sym(".") {
                    target.push(PathSeg::Field(self.ident()?));
                } else if target.is_empty() {
                    return Err(self.expected("`[key]` or `.field` after `with`"));
                } else {
                    break;
                }
            }
            self.expect_sym("=")?;
            let v = self.temporal()?;
            let span = e.span.to(v.span);
            e = Expr {
                kind: ExprKind::With(Box::new(e), target, Box::new(v)),
                span,
            };
        }
        Ok(e)
    }

    fn mk(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }

    fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
        let span = a.span.to(b.span);
        Expr {
            kind: ExprKind::Binary(op, Box::new(a), Box::new(b)),
            span,
        }
    }

    fn temporal(&mut self) -> PResult<Expr> {
        let lo = self.span();
        if self.eat_kw("always") {
            let e = self.temporal()?;
            return Ok(Self::mk(
                ExprKind::Unary(UnOp::Always, Box::new(e.clone())),
                lo.to(e.span),
            ));
        }
        if self.eat_kw("eventually") {
            let e = self.temporal()?;
            return Ok(Self::mk(
                ExprKind::Unary(UnOp::Eventually, Box::new(e.clone())),
                lo.to(e.span),
            ));
        }
        let a = self.coalesce()?;
        if self.eat_kw("leads_to") {
            let b = self.temporal()?;
            return Ok(Self::bin(BinOp::LeadsTo, a, b));
        }
        if self.eat_kw("until") {
            let b = self.temporal()?;
            return Ok(Self::bin(BinOp::Until, a, b));
        }
        Ok(a)
    }

    fn coalesce(&mut self) -> PResult<Expr> {
        let mut a = self.implies()?;
        while self.eat_sym("??") {
            let b = self.implies()?;
            a = Self::bin(BinOp::Coalesce, a, b);
        }
        Ok(a)
    }

    fn implies(&mut self) -> PResult<Expr> {
        let a = self.expr_bp_or()?;
        if self.eat_sym("=>") {
            let b = self.implies()?;
            return Ok(Self::bin(BinOp::Implies, a, b));
        }
        Ok(a)
    }

    /// `or` level; `and` and `or` may not be mixed without parentheses.
    fn expr_bp_or(&mut self) -> PResult<Expr> {
        let (mut a, first_and) = self.and()?;
        let mut first = true;
        while self.is_kw("or") {
            let at = self.bump().span;
            let (b, b_and) = self.and()?;
            if (first && first_and) || b_and {
                return Err(Diag::error(
                    at,
                    "`and` and `or` can't be mixed without parentheses",
                ));
            }
            first = false;
            a = Self::bin(BinOp::Or, a, b);
        }
        Ok(a)
    }

    fn and(&mut self) -> PResult<(Expr, bool)> {
        let mut a = self.not()?;
        let mut had = false;
        while self.eat_kw("and") {
            let b = self.not()?;
            a = Self::bin(BinOp::And, a, b);
            had = true;
        }
        Ok((a, had))
    }

    fn not(&mut self) -> PResult<Expr> {
        let lo = self.span();
        if self.eat_kw("not") {
            let e = self.not()?;
            let span = lo.to(e.span);
            return Ok(Self::mk(ExprKind::Unary(UnOp::Not, Box::new(e)), span));
        }
        self.cmp()
    }

    fn cmp_op(&self) -> Option<BinOp> {
        Some(match self.peek() {
            Tok::Sym("==") => BinOp::Eq,
            Tok::Sym("!=") => BinOp::Ne,
            Tok::Sym("<") => BinOp::Lt,
            Tok::Sym("<=") => BinOp::Le,
            Tok::Sym(">") => BinOp::Gt,
            Tok::Sym(">=") => BinOp::Ge,
            Tok::Ident(s) if s == "in" => BinOp::In,
            _ => return None,
        })
    }

    fn cmp(&mut self) -> PResult<Expr> {
        let a = self.expr_tol()?;
        if self.is_kw("is") {
            self.bump();
            let lo = self.span();
            let t = self.ident()?;
            let span = a.span.to(lo);
            return Ok(Self::mk(ExprKind::Is(Box::new(a), t), span));
        }
        if let Some(op) = self.cmp_op() {
            self.bump();
            let b = self.expr_tol()?;
            if self.cmp_op().is_some() {
                return Err(self.err_here("comparisons don't chain; use `and`"));
            }
            return Ok(Self::bin(op, a, b));
        }
        Ok(a)
    }

    fn expr_tol(&mut self) -> PResult<Expr> {
        let mut a = self.add()?;
        while self.eat_sym("±") {
            let b = self.add()?;
            a = Self::bin(BinOp::Tol, a, b);
        }
        if self.eat_sym("..") {
            let b = self.add()?;
            a = Self::bin(BinOp::Range, a, b);
        }
        Ok(a)
    }

    fn add(&mut self) -> PResult<Expr> {
        let mut a = self.mul()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("+") if !self.on_new_line() => BinOp::Add,
                Tok::Sym("-") if !self.on_new_line() => BinOp::Sub,
                Tok::Ident(s) if s == "union" => BinOp::Union,
                Tok::Ident(s) if s == "minus" => BinOp::Minus,
                _ => break,
            };
            self.bump();
            let b = self.mul()?;
            a = Self::bin(op, a, b);
        }
        Ok(a)
    }

    fn mul(&mut self) -> PResult<Expr> {
        let mut a = self.pow()?;
        loop {
            let op = match self.peek() {
                Tok::Sym("*") => BinOp::Mul,
                Tok::Sym("/") => BinOp::Div,
                Tok::Ident(s) if s == "inter" => BinOp::Inter,
                _ => break,
            };
            self.bump();
            let b = self.pow()?;
            a = Self::bin(op, a, b);
        }
        Ok(a)
    }

    fn pow(&mut self) -> PResult<Expr> {
        let a = self.unary()?;
        if self.eat_sym("^") {
            let b = self.nested(Self::pow)?;
            return Ok(Self::bin(BinOp::Pow, a, b));
        }
        Ok(a)
    }

    fn unary(&mut self) -> PResult<Expr> {
        self.nested(Self::unary_body)
    }

    fn unary_body(&mut self) -> PResult<Expr> {
        let lo = self.span();
        if self.eat_sym("-") {
            let e = self.unary()?;
            let span = lo.to(e.span);
            return Ok(Self::mk(ExprKind::Unary(UnOp::Neg, Box::new(e)), span));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.is_sym(".") {
                self.bump();
                let name = match self.peek().clone() {
                    Tok::Ident(s) => {
                        self.bump();
                        s
                    }
                    _ => return Err(self.expected("a field name")),
                };
                let span = e.span.to(self.prev_span());
                e = Self::mk(ExprKind::Field(Box::new(e), name), span);
            } else if self.is_sym("(") && !self.on_new_line() {
                self.bump();
                let mut args = Vec::new();
                if !self.is_sym(")") {
                    loop {
                        let name = if matches!(self.peek(), Tok::Ident(_)) && self.is_sym_at(1, ":")
                        {
                            let n = self.ident()?;
                            self.bump();
                            Some(n)
                        } else {
                            None
                        };
                        let value = self.expr()?;
                        args.push(Arg { name, value });
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                }
                self.expect_sym(")")?;
                let span = e.span.to(self.prev_span());
                e = Self::mk(ExprKind::Call(Box::new(e), args), span);
            } else if self.is_sym("[") && !self.on_new_line() {
                self.bump();
                let i = self.expr()?;
                self.expect_sym("]")?;
                let span = e.span.to(self.prev_span());
                e = Self::mk(ExprKind::Index(Box::new(e), Box::new(i)), span);
            } else if matches!(self.peek(), Tok::Prime) {
                let ExprKind::Name(n) = &e.kind else {
                    return Err(self.err_here("only a state variable can be primed"));
                };
                let n = n.clone();
                self.bump();
                e = Self::mk(ExprKind::Primed(n), e.span.to(self.prev_span()));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let lo = self.span();
        match self.peek().clone() {
            Tok::Num { value, dim, .. } => {
                self.bump();
                Ok(Self::mk(ExprKind::Num(value, dim), lo))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Self::mk(ExprKind::Str(s.into()), lo))
            }
            Tok::Ident(w) => match w.as_str() {
                "true" | "false" => {
                    self.bump();
                    Ok(Self::mk(ExprKind::Bool(w == "true"), lo))
                }
                "none" => {
                    self.bump();
                    Ok(Self::mk(ExprKind::NoneLit, lo))
                }
                "if" => {
                    self.bump();
                    let c = self.expr()?;
                    self.expect_kw("then")?;
                    let a = self.expr()?;
                    self.expect_kw("else")?;
                    let b = self.expr()?;
                    let span = lo.to(b.span);
                    Ok(Self::mk(
                        ExprKind::If(Box::new(c), Box::new(a), Box::new(b)),
                        span,
                    ))
                }
                "all" | "any" | "one" if self.gen_follows(1) => {
                    self.bump();
                    let q = match w.as_str() {
                        "all" => Quant::All,
                        "any" => Quant::Any,
                        _ => Quant::One,
                    };
                    let gens = self.gens()?;
                    self.expect_sym(":")?;
                    let body = self.expr()?;
                    let span = lo.to(body.span);
                    Ok(Self::mk(ExprKind::Quant(q, gens, Box::new(body)), span))
                }
                _ if RESERVED.contains(&w.as_str()) => Err(self.expected("an expression")),
                _ => {
                    self.bump();
                    Ok(Self::mk(ExprKind::Name(w), lo))
                }
            },
            Tok::Sym("(") => {
                self.bump();
                if self.eat_sym(")") {
                    return Ok(Self::mk(ExprKind::Tuple(vec![]), lo.to(self.prev_span())));
                }
                let first = self.expr()?;
                if self.eat_sym(")") {
                    // Parentheses only group; keep the inner node but widen the span.
                    return Ok(Expr {
                        kind: first.kind,
                        span: lo.to(self.prev_span()),
                    });
                }
                let mut v = vec![first];
                while self.eat_sym(",") {
                    v.push(self.expr()?);
                }
                self.expect_sym(")")?;
                Ok(Self::mk(ExprKind::Tuple(v), lo.to(self.prev_span())))
            }
            Tok::Sym("[") => {
                self.bump();
                if self.is_kw("for") {
                    return self.comprehension(lo, "]");
                }
                let mut v = Vec::new();
                if !self.is_sym("]") {
                    loop {
                        v.push(self.expr()?);
                        if !self.eat_sym(",") || self.is_sym("]") {
                            break;
                        }
                    }
                }
                self.expect_sym("]")?;
                Ok(Self::mk(ExprKind::List(v), lo.to(self.prev_span())))
            }
            Tok::Sym("{") => {
                self.bump();
                if self.is_kw("for") {
                    return self.comprehension(lo, "}");
                }
                if self.eat_sym("}") {
                    return Ok(Self::mk(ExprKind::Set(vec![]), lo.to(self.prev_span())));
                }
                // record: `{ident: …}`
                if matches!(self.peek(), Tok::Ident(_)) && self.is_sym_at(1, ":") {
                    let mut fs = Vec::new();
                    loop {
                        let n = self.ident()?;
                        self.expect_sym(":")?;
                        fs.push((n, self.expr()?));
                        if !self.eat_sym(",") || self.is_sym("}") {
                            break;
                        }
                    }
                    self.expect_sym("}")?;
                    return Ok(Self::mk(ExprKind::Record(fs), lo.to(self.prev_span())));
                }
                let first = self.expr()?;
                if self.eat_sym(":") {
                    let mut kv = vec![(first, self.expr()?)];
                    while self.eat_sym(",") && !self.is_sym("}") {
                        let k = self.expr()?;
                        self.expect_sym(":")?;
                        kv.push((k, self.expr()?));
                    }
                    self.expect_sym("}")?;
                    return Ok(Self::mk(ExprKind::Map(kv), lo.to(self.prev_span())));
                }
                let mut v = vec![first];
                while self.eat_sym(",") && !self.is_sym("}") {
                    v.push(self.expr()?);
                }
                self.expect_sym("}")?;
                Ok(Self::mk(ExprKind::Set(v), lo.to(self.prev_span())))
            }
            _ => Err(self.expected("an expression")),
        }
    }

    fn comprehension(&mut self, lo: Span, close: &str) -> PResult<Expr> {
        self.expect_kw("for")?;
        let gens = self.gens()?;
        let filter = if self.eat_kw("where") {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_kw("yield")?;
        let key = self.expr()?;
        let mut kind = if close == "]" {
            CompKind::List
        } else {
            CompKind::Set
        };
        let val = if close == "}" && self.eat_sym(":") {
            kind = CompKind::Map;
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_sym(close)?;
        Ok(Self::mk(
            ExprKind::Comp {
                kind,
                gens,
                filter,
                key: Box::new(key),
                val,
            },
            lo.to(self.prev_span()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> Expr {
        parse_expr_at(0, s, 0).unwrap_or_else(|d| panic!("{s}: {}", d.msg))
    }

    fn shape(x: &Expr) -> String {
        match &x.kind {
            ExprKind::Binary(op, a, b) => format!("({} {} {})", shape(a), op.text(), shape(b)),
            ExprKind::Unary(op, a) => format!("({op:?} {})", shape(a)),
            ExprKind::Name(n) => n.clone(),
            ExprKind::Primed(n) => format!("{n}'"),
            ExprKind::Num(n, _) => n.to_string(),
            ExprKind::Field(a, f) => format!("{}.{f}", shape(a)),
            ExprKind::Call(f, args) => format!("{}({})", shape(f), args.len()),
            ExprKind::Index(a, i) => format!("{}[{}]", shape(a), shape(i)),
            _ => "…".into(),
        }
    }

    #[test]
    fn nesting_past_the_budget_is_refused_with_a_diagnostic() {
        // 100 open parentheses is deeper than MAX_NESTING (32): refused, not a stack overflow.
        let deep = format!("{}true{}", "(".repeat(100), ")".repeat(100));
        match parse_expr_at(0, &deep, 0) {
            Err(d) => assert!(d.msg.contains("nested deeper than 32 levels"), "{}", d.msg),
            Ok(_) => panic!("100 nested parentheses parsed"),
        }
    }

    #[test]
    fn nesting_within_the_budget_parses() {
        let inside = format!("{}true{}", "(".repeat(30), ")".repeat(30));
        assert!(
            parse_expr_at(0, &inside, 0).is_ok(),
            "30 nested parentheses are within the budget"
        );
    }

    #[test]
    fn expression_parsing_follows_the_grammar() {
        // precedence_follows_the_table: each operator binds as the table says.
        for (src, want) in [
            ("a or b => c", "((a or b) => c)"),
            ("a => b => c", "(a => (b => c))"),
            ("not a == b and c", "((Not (a == b)) and c)"),
            ("a + b * c ^ d ^ e", "(a + (b * (c ^ (d ^ e))))"),
            ("-a.b", "(Neg a.b)"),
            ("-x ^ 2", "((Neg x) ^ 2)"),
            ("x ?? y or z", "(x ?? (y or z))"),
            ("not open'", "(Not open')"),
            ("chain[pos] == a", "(chain[pos] == a)"),
            ("always (p leads_to q)", "(Always (p leads_to q))"),
        ] {
            assert_eq!(shape(&e(src)), want, "{src}");
        }
        // and_or_need_parentheses_and_comparisons_do_not_chain: and/or need parentheses, comparisons do not chain.
        {
            assert!(parse_expr_at(0, "a and b or c", 0).is_err());
            assert!(parse_expr_at(0, "a or b and c", 0).is_err());
            assert!(parse_expr_at(0, "(a and b) or c", 0).is_ok());
            assert!(parse_expr_at(0, "a < b < c", 0).is_err());
        }
        // literals_and_comprehensions: literals, comprehensions, quantifiers, calls, with and is.
        {
            assert!(matches!(e("{a: 1, b: 2}").kind, ExprKind::Record(_)));
            assert!(matches!(e("{\"a\": 1}").kind, ExprKind::Map(_)));
            assert!(matches!(e("{a, b}").kind, ExprKind::Set(_)));
            assert!(matches!(e("{}").kind, ExprKind::Set(_)));
            assert!(matches!(e("(a, b)").kind, ExprKind::Tuple(_)));
            assert!(matches!(
                e("[for x in xs where x > 0 yield x]").kind,
                ExprKind::Comp {
                    kind: CompKind::List,
                    ..
                }
            ));
            assert!(matches!(
                e("{for x in xs yield x: 1}").kind,
                ExprKind::Comp {
                    kind: CompKind::Map,
                    ..
                }
            ));
            assert!(
                matches!(e("all x in S, y in T: x == y").kind, ExprKind::Quant(Quant::All, ref g, _) if g.len() == 2)
            );
            assert!(matches!(e("any(xs)").kind, ExprKind::Call(..)));
            assert!(matches!(
                e("m with [k] = v with .f = 2").kind,
                ExprKind::With(..)
            ));
            assert!(
                matches!(e("f(x, restart: 15 %)").kind, ExprKind::Call(_, ref a) if a[1].name.as_deref() == Some("restart"))
            );
            assert!(matches!(e("x is Agent").kind, ExprKind::Is(..)));
            assert!(matches!(e("if a then 1 else 2").kind, ExprKind::If(..)));
        }
    }

    #[test]
    fn a_new_line_ends_a_statement() {
        let m = parse_module(
            0,
            "machine M {\n var x: Int = 0\n var y: Int = 0\n action A {\n  when x < 1\n  x' = x + 1\n  y' = -1\n }\n}",
        )
        .unwrap();
        let Item::Machine(mm) = &m.items[0] else {
            panic!()
        };
        assert_eq!(mm.actions().next().unwrap().body.len(), 3);
    }
}
