//! Explicit-state model checking (docs/jhrl.md sections 9.10 and 11, AP-074.R2).
//!
//! Breadth-first search from every initial state over every action instance. States are
//! deduplicated by their canonical byte encoding, so the first time a state is reached is by a
//! shortest path and every counterexample trace is a shortest one. Invariants are checked in every
//! reachable state, transition invariants on every non-stuttering step, and a state with no
//! enabled action is a deadlock unless it is terminal or the model allows deadlock. States are
//! kept packed (one record plus the canonical bytes each, see `States`) and rebuilt when expanded.
//!
//! Not in v0: temporal properties and fairness (parsed, reported as skipped), symmetry
//! reduction (the full space is explored, which gives the same verdicts), parallel workers and
//! disk spill.

use crate::ast::*;
use crate::eval::Env;
use crate::machine::{MachineCtx, State};
use crate::program::{DeclRef, Program};
use crate::span::{Diag, Span};
use crate::value::Value;

#[derive(Clone, Debug)]
pub struct Step {
    pub action: String,
    pub params: Vec<(String, Value)>,
}

impl Step {
    pub fn label(&self) -> String {
        if self.params.is_empty() {
            self.action.clone()
        } else {
            let ps: Vec<String> = self
                .params
                .iter()
                .map(|(k, v)| format!("{k} = {v}"))
                .collect();
            format!("{}({})", self.action, ps.join(", "))
        }
    }
}

/// One state, decoded from the packed store by `States::get`: its values are rebuilt from the
/// canonical bytes, and the parent, step and depth come from the state's record.
#[derive(Clone, Debug)]
pub struct StateRec {
    pub vals: State,
    pub parent: Option<usize>,
    pub via: Option<Step>,
    /// BFS level, counting the initial states as 1.
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Invariant(String),
    Transition(String),
    Deadlock,
    Type(String),
    Error(String),
}

impl Found {
    pub fn describe(&self) -> String {
        match self {
            Found::Invariant(n) => format!("invariant \"{n}\" violated"),
            Found::Transition(n) => format!("transition \"{n}\" violated"),
            Found::Deadlock => "deadlock".into(),
            Found::Type(m) => format!("type error: {m}"),
            Found::Error(m) => format!("evaluation error: {m}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Violation {
    pub what: Found,
    /// The state where it shows (for a transition, the state before the step).
    pub state: usize,
    /// For transition violations and type errors on a new state: the step and the state after.
    pub step: Option<(Step, State)>,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Stop at the first violation (normal checking), or finish the breadth-first level where
    /// the first violation shows and record every violation at that depth (mutants). The second
    /// terminates even when a mutant breaks the model's bounds, and doesn't depend on the order
    /// in which actions are tried.
    pub stop_at_first: bool,
    pub force_deadlock_check: bool,
    pub max_states: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            stop_at_first: true,
            force_deadlock_check: false,
            max_states: 5_000_000,
        }
    }
}

pub struct Report {
    pub machine: String,
    pub model: String,
    pub ctx: MachineCtx,
    pub states: States,
    pub generated: usize,
    pub depth: u32,
    pub violations: Vec<Violation>,
    pub invariants: Vec<String>,
    pub transitions: Vec<String>,
    pub properties: Vec<String>,
    pub deadlock_checked: bool,
    pub notes: Vec<String>,
}

const NONE: u32 = u32::MAX;

/// One reached state's fixed-size record (24 bytes, no padding).
#[repr(C)]
#[derive(Clone, Copy)]
struct Rec {
    /// Where the state starts in the arena: its canonical encoding, then its step's parameters.
    start: u64,
    /// The length of the encoding; the parameters run from there to the next state's start.
    len: u32,
    /// The state it was reached from, `NONE` for an initial state.
    parent: u32,
    /// The action that reached it, an index into `States::actions`; `NONE` for an initial state.
    action: u32,
    /// BFS level, counting the initial states as 1.
    depth: u32,
}

const _: () = assert!(std::mem::size_of::<Rec>() == 24);

/// The reached states, packed. Each has a 24-byte record; its canonical encoding and its step's
/// parameters share one byte arena. An open-addressed table of 8-byte slots finds a state again:
/// a slot holds the upper half of the state's hash, then its index + 1 (0 is empty). A state's
/// values are rebuilt from its bytes when they are needed, so a state costs about 35 bytes plus
/// its encoding, where a tree of `Value`s costs kilobytes.
pub struct States {
    recs: Vec<Rec>,
    arena: Vec<u8>,
    slots: Vec<u64>,
    /// The machine's action names, which records refer to by index.
    actions: Vec<String>,
}

impl States {
    fn new(actions: Vec<String>) -> States {
        States {
            recs: Vec::new(),
            arena: Vec::new(),
            slots: vec![0; 1024],
            actions,
        }
    }

    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    pub fn depth(&self, i: usize) -> u32 {
        self.recs[i].depth
    }

    pub fn parent(&self, i: usize) -> Option<usize> {
        let p = self.recs[i].parent;
        (p != NONE).then_some(p as usize)
    }

    /// State `i`'s canonical encoding.
    fn bytes(&self, i: usize) -> &[u8] {
        let r = self.recs[i];
        &self.arena[r.start as usize..r.start as usize + r.len as usize]
    }

    /// State `i`'s values, rebuilt from its encoding.
    pub fn vals(&self, i: usize) -> State {
        let mut b = self.bytes(i);
        let mut out = State::new();
        while !b.is_empty() {
            out.push(Value::decode(&mut b));
        }
        out
    }

    /// The step that reached state `i`; none for an initial state.
    pub fn via(&self, i: usize) -> Option<Step> {
        let r = self.recs[i];
        if r.action == NONE {
            return None;
        }
        let end = self
            .recs
            .get(i + 1)
            .map_or(self.arena.len(), |n| n.start as usize);
        let mut b = &self.arena[r.start as usize + r.len as usize..end];
        let mut params = Vec::new();
        while !b.is_empty() {
            let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
            let name = std::str::from_utf8(&b[4..4 + n])
                .expect("parameter names are UTF-8")
                .to_owned();
            b = &b[4 + n..];
            params.push((name, Value::decode(&mut b)));
        }
        Some(Step {
            action: self.actions[r.action as usize].clone(),
            params,
        })
    }

    /// State `i`, decoded.
    pub fn get(&self, i: usize) -> StateRec {
        StateRec {
            vals: self.vals(i),
            parent: self.parent(i),
            via: self.via(i),
            depth: self.depth(i),
        }
    }

    /// Adds the state encoded as `enc` unless it is already known, and returns its index when it
    /// is new. `via` is the index of the action that reached it and that step's parameters.
    fn add(
        &mut self,
        enc: &[u8],
        parent: Option<usize>,
        via: Option<(usize, &[(String, Value)])>,
        depth: u32,
    ) -> Option<usize> {
        let tag = hash(enc) >> 32;
        let mask = self.slots.len() - 1;
        let mut p = (tag >> (32 - self.slots.len().trailing_zeros())) as usize;
        loop {
            let s = self.slots[p];
            if s == 0 {
                break;
            }
            if s >> 32 == tag && self.bytes((s as u32 - 1) as usize) == enc {
                return None;
            }
            p = (p + 1) & mask;
        }
        let idx = self.recs.len();
        assert!(idx < NONE as usize, "more states than a u32 index holds");
        self.slots[p] = (tag << 32) | (idx as u64 + 1);
        let start = self.arena.len() as u64;
        self.arena.extend_from_slice(enc);
        let action = match via {
            Some((a, params)) => {
                for (k, v) in params {
                    self.arena.extend((k.len() as u32).to_le_bytes());
                    self.arena.extend(k.as_bytes());
                    v.encode(&mut self.arena);
                }
                a as u32
            }
            None => NONE,
        };
        self.recs.push(Rec {
            start,
            len: enc.len() as u32,
            parent: parent.map_or(NONE, |p| p as u32),
            action,
            depth,
        });
        if self.recs.len() * 4 > self.slots.len() * 3 {
            self.grow();
        }
        Some(idx)
    }

    /// Doubles the table. A slot's position comes from its tag, so nothing is hashed again.
    fn grow(&mut self) {
        let n = self.slots.len() * 2;
        let old = std::mem::replace(&mut self.slots, vec![0; n]);
        let mask = self.slots.len() - 1;
        let shift = 32 - self.slots.len().trailing_zeros();
        for s in old.into_iter().filter(|&s| s != 0) {
            let mut p = ((s >> 32) >> shift) as usize;
            while self.slots[p] != 0 {
                p = (p + 1) & mask;
            }
            self.slots[p] = s;
        }
    }
}

/// FxHash over 8-byte words with murmur3's finish, so the upper bits (the table position) are well
/// mixed.
fn hash(b: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h = b.len() as u64;
    let (words, rest) = b.as_chunks::<8>();
    for w in words {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(*w)).wrapping_mul(K);
    }
    if !rest.is_empty() {
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(last)).wrapping_mul(K);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// Encodes the values of a state into `out`, replacing what it held.
fn encode_into(s: &[Value], out: &mut Vec<u8>) {
    out.clear();
    for v in s {
        v.encode(out);
    }
}

/// The models of a machine, wherever they are declared.
pub fn models_of(prog: &Program, machine: DeclRef) -> Vec<(usize, DeclRef, &ModelDecl)> {
    let mut v = Vec::new();
    for (pack, r, it) in prog.items() {
        if let Item::Model(m) = it {
            if prog.lookup_decl(pack, &m.machine) == Some(machine) {
                v.push((pack, r, m));
            }
        }
    }
    v
}

struct Search<'a> {
    prog: &'a Program,
    ctx: &'a MachineCtx,
    invs: Vec<(&'a str, &'a InvBody)>,
    trans: Vec<(&'a str, &'a Expr)>,
    opts: &'a Options,
    rep: Report,
    /// Depth of the shallowest violation so far (level mode).
    cutoff: Option<u32>,
    stop: bool,
}

impl<'a> Search<'a> {
    fn render(&self, d: &Diag) -> String {
        d.render(&self.prog.sources)
    }

    fn record(&mut self, v: Violation, depth: u32) {
        let fatal = matches!(v.what, Found::Error(_));
        self.cutoff = Some(self.cutoff.map_or(depth, |c| c.min(depth)));
        let states = &self.rep.states;
        let vd = |x: &Violation| states.depth(x.state) + x.step.is_some() as u32;
        match self.rep.violations.iter().position(|x| x.what == v.what) {
            Some(i) if vd(&self.rep.violations[i]) > depth => self.rep.violations[i] = v,
            Some(_) => {}
            None => self.rep.violations.push(v),
        }
        if fatal || self.opts.stop_at_first {
            self.stop = true;
        }
    }

    fn check_state(&self, s: &[Value]) -> Result<Vec<String>, Diag> {
        let mut bad = Vec::new();
        for (name, body) in &self.invs {
            let mut env = Env::machine(self.prog, self.ctx, Some(s));
            let v = match body {
                InvBody::Expr(e) => env.eval(e)?,
                InvBody::Clauses(c) => env.eval_clauses(c)?,
            };
            match v {
                Value::Bool(true) => {}
                Value::Bool(false) => bad.push(name.to_string()),
                other => {
                    return Err(Diag::bare(format!(
                        "invariant \"{name}\" evaluates to {other}, not a Bool"
                    )))
                }
            }
        }
        Ok(bad)
    }

    /// Type and invariant checks of a newly reached state `vals`. `via` is the action's name and
    /// parameters that reached it; none for an initial state.
    fn check_new(&mut self, idx: usize, vals: &[Value], via: Option<(&str, &[(String, Value)])>) {
        let depth = self.rep.states.depth(idx);
        if let Some(m) = self.ctx.type_error(self.prog, vals) {
            let m = match via {
                Some((action, params)) => {
                    let st = Step {
                        action: action.to_owned(),
                        params: params.to_vec(),
                    };
                    format!("{m} after {}", st.label())
                }
                None => m,
            };
            self.record(
                Violation {
                    what: Found::Type(m),
                    state: idx,
                    step: None,
                },
                depth,
            );
            if self.stop {
                return;
            }
        }
        match self.check_state(vals) {
            Ok(bad) => {
                for b in bad {
                    self.record(
                        Violation {
                            what: Found::Invariant(b),
                            state: idx,
                            step: None,
                        },
                        depth,
                    );
                    if self.stop {
                        return;
                    }
                }
            }
            Err(d) => {
                let msg = self.render(&d);
                self.record(
                    Violation {
                        what: Found::Error(msg),
                        state: idx,
                        step: None,
                    },
                    depth,
                );
            }
        }
    }
}

pub fn explore(
    prog: &Program,
    model_pack: usize,
    model: &ModelDecl,
    opts: &Options,
) -> Result<Report, Diag> {
    let (mpack, mdecl) = prog
        .machine(model_pack, &model.machine)
        .map_err(|e| Diag::error(model.span, e))?;
    let ctx = MachineCtx::new(prog, mpack, mdecl, Some((model, model_pack)), &[])?;
    let mut invs: Vec<(&str, &InvBody)> = Vec::new();
    let mut trans: Vec<(&str, &Expr)> = Vec::new();
    let mut props = Vec::new();
    let mut terminals: Vec<(&Expr, usize)> = Vec::new();
    for it in &mdecl.items {
        match it {
            MItem::Invariant { name, body, .. } => invs.push((name, body)),
            MItem::Transition { name, expr, .. } => trans.push((name, expr)),
            MItem::Property { name, .. } => props.push(name.clone()),
            MItem::Terminal(e) => terminals.push((e, mpack)),
            _ => {}
        }
    }
    let mut constraints: Vec<&Expr> = Vec::new();
    let mut deadlock_allowed = false;
    let mut notes = Vec::new();
    for o in &model.opts {
        match o {
            MOpt::Terminal(e) => terminals.push((e, model_pack)),
            MOpt::Constraint(e) => constraints.push(e),
            MOpt::DeadlockAllowed => deadlock_allowed = true,
            MOpt::Symmetry(s) if s != "none" => notes.push(format!(
                "symmetry {s} not applied (v0 explores the full space)"
            )),
            _ => {}
        }
    }
    let check_deadlock = !deadlock_allowed || opts.force_deadlock_check;
    let actions: Vec<&ActionDecl> = mdecl.actions().collect();
    let rep = Report {
        machine: mdecl.name.clone(),
        model: model.name.clone(),
        ctx: ctx.clone(),
        states: States::new(actions.iter().map(|a| a.name.clone()).collect()),
        generated: 0,
        depth: 0,
        violations: vec![],
        invariants: invs.iter().map(|x| x.0.to_string()).collect(),
        transitions: trans.iter().map(|x| x.0.to_string()).collect(),
        properties: props,
        deadlock_checked: check_deadlock,
        notes,
    };
    let mut s = Search {
        prog,
        ctx: &ctx,
        invs,
        trans,
        opts,
        rep,
        cutoff: None,
        stop: false,
    };
    let mut enc = Vec::with_capacity(256);

    let inits = match ctx.initial_states(prog) {
        Ok(v) => v,
        Err(d) => {
            let msg = s.render(&d);
            s.rep.violations.push(Violation {
                what: Found::Error(msg),
                state: 0,
                step: None,
            });
            return Ok(s.rep);
        }
    };
    for st in inits {
        encode_into(&st, &mut enc);
        let Some(idx) = s.rep.states.add(&enc, None, None, 1) else {
            continue;
        };
        s.rep.generated += 1;
        s.rep.depth = 1;
        s.check_new(idx, &st, None);
        if s.stop {
            return Ok(s.rep);
        }
    }

    // Every new state is appended to the store, so walking the states in index order visits them
    // in the order of a breadth-first queue.
    let mut next_idx = 0;
    while next_idx < s.rep.states.len() {
        let idx = next_idx;
        next_idx += 1;
        let cur = s.rep.states.vals(idx);
        let depth = s.rep.states.depth(idx);
        if s.cutoff.is_some_and(|c| depth > c) {
            continue;
        }
        // At the cutoff depth only deadlock is still of interest: count, don't add states.
        let count_only = s.cutoff == Some(depth);
        // State constraint: the state is kept but not expanded.
        let mut inside = true;
        for c in &constraints {
            let mut env = Env::machine(prog, &ctx, Some(&cur));
            env.pack = model_pack;
            match env.eval_bool(c, "a model constraint") {
                Ok(Some(true)) => {}
                Ok(_) => inside = false,
                Err(d) => {
                    let msg = s.render(&d);
                    s.record(
                        Violation {
                            what: Found::Error(msg),
                            state: idx,
                            step: None,
                        },
                        depth,
                    );
                    return Ok(s.rep);
                }
            }
        }
        if !inside {
            continue;
        }
        let mut enabled = 0usize;
        for (ai, a) in actions.iter().enumerate() {
            let succs = match ctx.successors(prog, a, &cur) {
                Ok(v) => v,
                Err(d) => {
                    let msg = format!(
                        "{} (in action {} from {})",
                        s.render(&d),
                        a.name,
                        ctx.show(&cur, None)
                    );
                    s.record(
                        Violation {
                            what: Found::Error(msg),
                            state: idx,
                            step: None,
                        },
                        depth,
                    );
                    return Ok(s.rep);
                }
            };
            enabled += succs.len();
            if count_only {
                continue;
            }
            for sc in succs {
                s.rep.generated += 1;
                let next = sc.next;
                if next != cur {
                    let nx: Vec<Option<Value>> = next.iter().cloned().map(Some).collect();
                    for ti in 0..s.trans.len() {
                        let (name, e) = s.trans[ti];
                        let mut env = Env::machine(prog, &ctx, Some(&cur));
                        env.next = Some(&nx);
                        match env.eval(e) {
                            Ok(Value::Bool(true)) => {}
                            Ok(Value::Bool(false)) => {
                                let v = Violation {
                                    what: Found::Transition(name.to_string()),
                                    state: idx,
                                    step: Some((
                                        Step {
                                            action: a.name.clone(),
                                            params: sc.params.clone(),
                                        },
                                        next.clone(),
                                    )),
                                };
                                s.record(v, depth + 1);
                            }
                            Ok(other) => {
                                let msg = format!(
                                    "transition \"{name}\" evaluates to {other}, not a Bool"
                                );
                                s.record(
                                    Violation {
                                        what: Found::Error(msg),
                                        state: idx,
                                        step: None,
                                    },
                                    depth + 1,
                                );
                            }
                            Err(d) => {
                                let msg = s.render(&d);
                                s.record(
                                    Violation {
                                        what: Found::Error(msg),
                                        state: idx,
                                        step: None,
                                    },
                                    depth + 1,
                                );
                            }
                        }
                        if s.stop {
                            return Ok(s.rep);
                        }
                    }
                }
                encode_into(&next, &mut enc);
                let Some(nidx) =
                    s.rep
                        .states
                        .add(&enc, Some(idx), Some((ai, sc.params.as_slice())), depth + 1)
                else {
                    continue;
                };
                s.rep.depth = s.rep.depth.max(depth + 1);
                if s.rep.states.len() > opts.max_states {
                    let msg = format!(
                        "more than {} states; tighten the model's bounds",
                        opts.max_states
                    );
                    s.record(
                        Violation {
                            what: Found::Error(msg),
                            state: nidx,
                            step: None,
                        },
                        depth + 1,
                    );
                    return Ok(s.rep);
                }
                s.check_new(nidx, &next, Some((a.name.as_str(), sc.params.as_slice())));
                if s.stop {
                    return Ok(s.rep);
                }
            }
        }
        if enabled == 0 && check_deadlock {
            let mut terminal = false;
            for (t, p) in &terminals {
                let mut env = Env::machine(prog, &ctx, Some(&cur));
                env.pack = *p;
                if let Ok(Some(true)) = env.eval_bool(t, "terminal") {
                    terminal = true;
                }
            }
            if !terminal {
                s.record(
                    Violation {
                        what: Found::Deadlock,
                        state: idx,
                        step: None,
                    },
                    depth,
                );
                if s.stop {
                    return Ok(s.rep);
                }
            }
        }
    }
    // In level mode keep only the shallowest violations.
    if let Some(c) = s.cutoff {
        let states = &s.rep.states;
        s.rep.violations.retain(|v| {
            states.depth(v.state) + v.step.is_some() as u32 == c
                || matches!(v.what, Found::Error(_))
        });
    }
    Ok(s.rep)
}

impl Report {
    pub fn path_to(&self, idx: usize) -> Vec<usize> {
        let mut v = vec![idx];
        let mut i = idx;
        while let Some(p) = self.states.parent(i) {
            v.push(p);
            i = p;
        }
        v.reverse();
        v
    }

    /// The shortest trace to a violation, one line per step: action, parameters, changed vars.
    pub fn render_trace(&self, v: &Violation, indent: &str) -> String {
        let path = self.path_to(v.state);
        let vals: Vec<State> = path.iter().map(|&i| self.states.vals(i)).collect();
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut prev: Option<&[Value]> = None;
        for (k, &i) in path.iter().enumerate() {
            let label = self
                .states
                .via(i)
                .map(|x| x.label())
                .unwrap_or_else(|| "init".into());
            rows.push((label, self.ctx.show(&vals[k], prev)));
            prev = Some(vals[k].as_slice());
        }
        if let Some((step, next)) = &v.step {
            rows.push((step.label(), self.ctx.show(next, prev)));
        }
        let w = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
        let mut out = String::new();
        for (k, (a, b)) in rows.iter().enumerate() {
            out.push_str(&format!("{indent}{:>2}. {a:<w$}  {b}\n", k + 1));
        }
        out
    }

    /// The first state (in BFS order, so at least depth) where `e` holds.
    pub fn find(&self, prog: &Program, pack: usize, e: &Expr) -> Result<Option<usize>, Diag> {
        for i in 0..self.states.len() {
            let vals = self.states.vals(i);
            let mut env = Env::machine(prog, &self.ctx, Some(&vals));
            env.pack = pack;
            if env.eval_bool(e, "a witness")? == Some(true) {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------- mutants

/// Applies a mutant's changes to a copy of the program. Textual replacements are applied to
/// the action's source text first (each must match exactly once) and re-parsed; `drop when`
/// and `remove action` then edit the syntax tree.
pub fn mutate(prog: &Program, pack: usize, m: &MutantDecl) -> Result<Program, Diag> {
    let Some(mref) = prog.lookup_decl(pack, &m.machine) else {
        return Err(Diag::error(
            m.span,
            format!("unknown machine `{}`", m.machine),
        ));
    };
    let find_action = |p: &Program, name: &str, span: Span| -> Result<Span, Diag> {
        let Item::Machine(md) = p.item(mref) else {
            return Err(Diag::error(
                span,
                "the mutated file no longer has the machine at the same place",
            ));
        };
        md.actions()
            .find(|a| a.name == name)
            .map(|a| a.span)
            .ok_or_else(|| Diag::error(span, format!("machine {} has no action `{name}`", md.name)))
    };
    let mut p = prog.clone();
    for op in &m.ops {
        if let MutOp::Replace {
            text,
            action,
            with,
            span,
        } = op
        {
            let aspan = find_action(&p, action, *span)?;
            let src = &p.sources.files[mref.file as usize].text;
            let body = &src[aspan.lo as usize..aspan.hi as usize];
            let n = body.matches(text.as_str()).count();
            if n != 1 {
                return Err(Diag::error(*span, format!("\"{text}\" occurs {n} times in action {action}; it must occur exactly once")));
            }
            let at = aspan.lo as usize + body.find(text.as_str()).unwrap();
            let new_text = format!("{}{}{}", &src[..at], with, &src[at + text.len()..]);
            p = p.with_source(mref.file, new_text).map_err(|d| {
                Diag::error(
                    *span,
                    format!(
                        "the mutated machine does not parse: {}",
                        d.render(&prog.sources)
                    ),
                )
            })?;
        }
    }
    for op in &m.ops {
        let Item::Machine(md) = &mut p.modules[mref.file as usize].items[mref.item] else {
            return Err(Diag::error(m.span, "machine moved after mutation"));
        };
        match op {
            MutOp::DropWhen { n, action, span } => {
                let Some(a) = md.items.iter_mut().find_map(|i| match i {
                    MItem::Action(a) if a.name == *action => Some(a),
                    _ => None,
                }) else {
                    return Err(Diag::error(*span, format!("no action `{action}`")));
                };
                let pos: Vec<usize> = a
                    .body
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s.kind, StmtKind::When(_)))
                    .map(|(i, _)| i)
                    .collect();
                if *n == 0 || *n > pos.len() {
                    return Err(Diag::error(
                        *span,
                        format!("action {action} has {} top-level guard(s)", pos.len()),
                    ));
                }
                a.body.remove(pos[*n - 1]);
            }
            MutOp::RemoveAction { action, span } => {
                let before = md.items.len();
                md.items
                    .retain(|i| !matches!(i, MItem::Action(a) if a.name == *action));
                if md.items.len() == before {
                    return Err(Diag::error(*span, format!("no action `{action}`")));
                }
            }
            MutOp::Replace { .. } => {}
        }
    }
    Ok(p)
}

pub struct MutantOutcome {
    pub caught: bool,
    pub line: String,
    pub detail: Vec<String>,
}

pub fn run_mutant(prog: &Program, pack: usize, m: &MutantDecl) -> MutantOutcome {
    let fail = |msg: String| MutantOutcome {
        caught: false,
        line: msg,
        detail: vec![],
    };
    let Some(mref) = prog.lookup_decl(pack, &m.machine) else {
        return fail(format!("unknown machine `{}`", m.machine));
    };
    let models = models_of(prog, mref);
    if models.is_empty() {
        return fail(format!(
            "machine {} has no model to check the mutant in",
            m.machine
        ));
    }
    if let Expect::Property(p) = &m.expect {
        return fail(format!(
            "expects property \"{p}\", but property checking is not in v0"
        ));
    }
    let mp = match mutate(prog, pack, m) {
        Ok(p) => p,
        Err(d) => return fail(d.render(&prog.sources)),
    };
    let opts = Options {
        stop_at_first: false,
        force_deadlock_check: m.expect == Expect::Deadlock,
        ..Options::default()
    };
    let mut seen = Vec::new();
    for (mpack, mr, _) in models {
        let Item::Model(md) = mp.item(mr) else {
            return fail("model moved after mutation".into());
        };
        let rep = match explore(&mp, mpack, md, &opts) {
            Ok(r) => r,
            Err(d) => return fail(d.render(&mp.sources)),
        };
        for v in &rep.violations {
            let hit = match (&m.expect, &v.what) {
                (Expect::Violation(n), Found::Invariant(x) | Found::Transition(x)) => n == x,
                (Expect::Deadlock, Found::Deadlock) => true,
                _ => false,
            };
            if hit {
                let others: Vec<String> = rep
                    .violations
                    .iter()
                    .filter(|o| o.what != v.what)
                    .map(|o| o.what.describe())
                    .collect();
                let mut line = format!(
                    "caught in \"{}\": {} at depth {}",
                    md.name,
                    v.what.describe(),
                    rep.states.depth(v.state) + v.step.is_some() as u32
                );
                if !others.is_empty() {
                    line.push_str(&format!(" (also: {})", others.join("; ")));
                }
                return MutantOutcome {
                    caught: true,
                    line,
                    detail: vec![rep.render_trace(v, "  ")],
                };
            }
        }
        seen.extend(
            rep.violations
                .iter()
                .map(|v| format!("\"{}\": {}", md.name, v.what.describe())),
        );
    }
    let what = match &m.expect {
        Expect::Violation(n) => format!("violation \"{n}\""),
        Expect::Deadlock => "deadlock".into(),
        Expect::Property(p) => format!("property \"{p}\""),
    };
    let found = if seen.is_empty() {
        "no violation at all".to_string()
    } else {
        seen.join("; ")
    };
    fail(format!("was NOT caught: expected {what}, found {found}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(src: &str) -> Program {
        let (p, d) = Program::load(vec![("t.jhrl".into(), src.into())]);
        assert!(
            d.is_empty(),
            "{:?}",
            d.iter().map(|x| x.render(&p.sources)).collect::<Vec<_>>()
        );
        p
    }

    fn model<'a>(p: &'a Program, name: &str) -> (usize, &'a ModelDecl) {
        p.items()
            .find_map(|(pk, _, it)| match it {
                Item::Model(m) if m.name == name => Some((pk, m)),
                _ => None,
            })
            .unwrap()
    }

    const COUNTER: &str = "machine C {\n const N: Int\n var x: Int in {0, 1}\n action Inc { when x < N\n x' = x + 1 }\n invariant \"bounded\" x <= N\n transition \"up\" x' >= x\n}\nmodel \"m\" of C { const N = 3 }\n";

    #[test]
    fn explore_reports_states_violations_and_errors() {
        // counter_states_depth_and_deadlock: state count, depth, and the deadlock.
        {
            let p = load(COUNTER);
            let (pk, m) = model(&p, "m");
            let r = explore(&p, pk, m, &Options::default()).unwrap();
            assert_eq!(r.states.len(), 4); // 0, 1, 2, 3
            assert_eq!(r.depth, 3); // x starts in {0, 1}: 1 → 2 → 3
            assert_eq!(r.violations.len(), 1);
            assert_eq!(r.violations[0].what, Found::Deadlock);
            assert_eq!(r.states.vals(r.violations[0].state)[0], Value::int(3));
        }
        // shortest_counterexample: the trace to a violation is the shortest one.
        {
            let src = COUNTER
                .replace("x <= N", "x < 3")
                .replace("const N = 3", "const N = 5\n deadlock allowed");
            let p = load(&src);
            let (pk, m) = model(&p, "m");
            let r = explore(&p, pk, m, &Options::default()).unwrap();
            let v = &r.violations[0];
            assert_eq!(v.what, Found::Invariant("bounded".into()));
            // From x = 1 two steps reach 3: init, Inc, Inc (not the 4-state path from 0).
            assert_eq!(r.path_to(v.state).len(), 3);
            let t = r.render_trace(v, "");
            assert!(t.contains("1. init") && t.contains("3. Inc"), "{t}");
        }
        // either_pick_and_init_blocks: either, pick, init blocks, and action parameters.
        {
            let p = load(
                "machine M {\n var x: Int = 0\n var y: Int = 0\n init { pick v in {1, 2}\n y' = v }\n\
                 action A { either { when x == 0\n x' = 1 } or { when x == 1\n x' = 2 } }\n\
                 action B(k in {10, 20}) { when x == 2\n pick j in {0, 1}\n y' = k + j\n x' = 3 }\n}\n\
                 model \"m\" of M { deadlock allowed }\n",
            );
            let (pk, m) = model(&p, "m");
            let r = explore(&p, pk, m, &Options::default()).unwrap();
            assert!(r.violations.is_empty(), "{:?}", r.violations);
            // y ∈ {1, 2} × x ∈ {0, 1, 2} = 6, then x = 3 with y ∈ {10, 11, 20, 21} = 4
            assert_eq!(r.states.len(), 10);
        }
        // assigning_a_variable_twice_is_an_error: a second assignment in one action is an error.
        {
            let p = load("machine M {\n var x: Int = 0\n action A { x' = 1; x' = 2 }\n}\nmodel \"m\" of M { deadlock allowed }\n");
            let (pk, m) = model(&p, "m");
            let r = explore(&p, pk, m, &Options::default()).unwrap();
            assert!(
                matches!(&r.violations[0].what, Found::Error(e) if e.contains("assigned twice"))
            );
        }
        // type_errors_are_reported: a value outside the enum type is a type error.
        {
            let p = load(
                "enum S { \"a\"\n \"b\" }\nmachine M {\n var s: S = \"a\"\n action A { when s == \"a\"\n s' = \"c\" }\n}\nmodel \"m\" of M { deadlock allowed }\n",
            );
            let (pk, m) = model(&p, "m");
            let r = explore(&p, pk, m, &Options::default()).unwrap();
            assert!(
                matches!(&r.violations[0].what, Found::Type(e) if e.contains("\"c\" is not of type S")),
                "{:?}",
                r.violations
            );
        }
    }

    #[test]
    fn mutants_must_match_once_and_be_caught() {
        let src = format!(
            "{COUNTER}mutant \"no guard\" of C {{\n replace \"x < N\" in Inc with \"true\"\n expect violation \"bounded\"\n}}\n\
             mutant \"twice\" of C {{\n replace \"x\" in Inc with \"y\"\n expect violation \"bounded\"\n}}\n\
             mutant \"drop\" of C {{\n drop when 1 in Inc\n expect violation \"bounded\"\n}}\n\
             mutant \"down\" of C {{\n replace \"x + 1\" in Inc with \"x - 1\"\n expect violation \"up\"\n}}\n\
             mutant \"remove\" of C {{\n remove action Inc\n expect deadlock\n}}\n\
             mutant \"harmless\" of C {{\n replace \"x + 1\" in Inc with \"x + 0\"\n expect violation \"bounded\"\n}}\n"
        );
        let p = load(&src);
        let muts: Vec<(usize, MutantDecl)> = p
            .items()
            .filter_map(|(pk, _, it)| {
                if let Item::Mutant(m) = it {
                    Some((pk, m.clone()))
                } else {
                    None
                }
            })
            .collect();
        let out: Vec<(String, bool, String)> = muts
            .iter()
            .map(|(pk, m)| {
                let o = run_mutant(&p, *pk, m);
                (m.name.clone(), o.caught, o.line)
            })
            .collect();
        assert!(out[0].1, "{:?}", out[0]);
        assert!(!out[1].1 && out[1].2.contains("occurs"), "{:?}", out[1]);
        assert!(out[2].1, "{:?}", out[2]);
        assert!(out[3].1, "{:?}", out[3]);
        assert!(out[4].1, "{:?}", out[4]);
        assert!(!out[5].1 && out[5].2.contains("NOT caught"), "{:?}", out[5]);
    }

    #[test]
    fn the_packed_store_finds_states_again_and_rebuilds_them() {
        let mut st = States::new(vec!["go".into()]);
        let state_of = |i: usize| vec![Value::int(i as i128), Value::Bool(i.is_multiple_of(2))];
        let mut enc = Vec::new();
        for i in 0..5000usize {
            encode_into(&state_of(i), &mut enc);
            let params = [("n".to_string(), Value::int(i as i128))];
            let (parent, via) = if i == 0 {
                (None, None)
            } else {
                (Some(i - 1), Some((0, &params[..])))
            };
            assert_eq!(st.add(&enc, parent, via, 1 + i as u32), Some(i));
        }
        // Adding every state again finds each one, across several doublings of the table.
        for i in 0..5000usize {
            encode_into(&state_of(i), &mut enc);
            assert_eq!(st.add(&enc, None, None, 1), None);
        }
        assert_eq!(st.len(), 5000);
        assert_eq!(st.vals(1234), state_of(1234));
        assert_eq!(st.parent(1234), Some(1233));
        assert_eq!(st.depth(1234), 1235);
        assert!(st.via(0).is_none());
        let via = st.via(7).expect("state 7 has a step");
        assert_eq!(via.action, "go");
        assert_eq!(via.params, vec![("n".to_string(), Value::int(7))]);
    }
}
