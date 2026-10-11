//! Executable AP scenarios (docs/jhrl.md section 9.12, AP-074.R7).
//!
//! A scenario `of` a machine starts from the machine's initial state(s) with `given` setting
//! constants and variables, applies each `when Action(args)` (failing if it isn't enabled, unless
//! the next step is `then rejected`), and requires each `then` to hold in every current state.
//! A scenario without `of` evaluates its `then` expressions over the visible functions, tables
//! and kinds. Fixture scenarios (`given fixture`, engine operations in `when`) need the graph
//! snapshot loader and are reported as skipped in v0.

use crate::ast::*;
use crate::eval::Env;
use crate::machine::{ConstSource, MachineCtx, State};
use crate::program::Program;
use crate::span::Diag;
use crate::value::Value;

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

pub fn run(prog: &Program, pack: usize, sc: &ScenarioDecl) -> Outcome {
    if let Some(s) = sc.steps.iter().find(|s| matches!(s.kind, SStepKind::Fixture(_))) {
        return Outcome::Skip(format!("needs a fixture graph (not in v0), at {}", prog.locate(s.span)));
    }
    let r = match &sc.machine {
        Some(m) => run_machine(prog, pack, sc, m),
        None => run_pure(prog, pack, sc),
    };
    match r {
        Ok(o) => o,
        Err(d) => Outcome::Fail(d.render(&prog.sources)),
    }
}

fn run_pure(prog: &Program, pack: usize, sc: &ScenarioDecl) -> Result<Outcome, Diag> {
    let mut n = 0;
    for st in &sc.steps {
        match &st.kind {
            SStepKind::Then(e) => {
                let v = Env::new(prog, pack).eval(e)?;
                if v != Value::Bool(true) {
                    return Ok(Outcome::Fail(format!("{}: `then` is {v}: {}", prog.locate(e.span), prog.sources.snippet(e.span))));
                }
                n += 1;
            }
            SStepKind::When(_) | SStepKind::Given(_) | SStepKind::Rejected(_) => {
                return Ok(Outcome::Skip(format!(
                    "engine operations without a machine need a fixture graph (not in v0), at {}",
                    prog.locate(st.span)
                )));
            }
            SStepKind::Fixture(_) => unreachable!(),
        }
    }
    Ok(Outcome::Pass(format!("{n} assertion(s)")))
}

fn run_machine(prog: &Program, pack: usize, sc: &ScenarioDecl, mname: &str) -> Result<Outcome, Diag> {
    let (mpack, decl) = prog.machine(pack, mname).map_err(|e| Diag::error(sc.span, e))?;
    // `given` steps before the first `when` set constants and initial values.
    let mut const_src = Vec::new();
    let mut first_given: Vec<&(String, Expr)> = Vec::new();
    for st in &sc.steps {
        match &st.kind {
            SStepKind::Given(asg) => first_given.extend(asg.iter()),
            SStepKind::When(_) => break,
            _ => {}
        }
    }
    let is_const = |n: &str| decl.items.iter().any(|i| matches!(i, MItem::Const { name, .. } if name == n));
    for (n, e) in &first_given {
        if is_const(n) {
            const_src.push(ConstSource { name: n.clone(), expr: e, pack });
        }
    }
    let ctx = MachineCtx::new(prog, mpack, decl, None, &const_src)?;
    let mut states: Vec<State> = ctx.initial_states(prog)?;
    let mut started = false;
    let mut pending_reject: Option<String> = None;
    let mut steps = 0;
    for st in &sc.steps {
        if let Some(a) = &pending_reject {
            if !matches!(st.kind, SStepKind::Rejected(_)) {
                return Ok(Outcome::Fail(format!("{}: {a} is not enabled in {}", prog.locate(st.span), show_all(&ctx, &states))));
            }
        }
        match &st.kind {
            SStepKind::Given(asg) => {
                for (n, e) in asg {
                    if is_const(n) {
                        if started {
                            return Ok(Outcome::Fail(format!("{}: const {n} can only be given before the first `when`", prog.locate(st.span))));
                        }
                        continue;
                    }
                    let Some(&i) = ctx.var_index.get(n) else {
                        return Err(Diag::error(e.span, format!("{mname} has no const or var `{n}`")));
                    };
                    for s in states.iter_mut() {
                        let mut env = Env::machine(prog, &ctx, Some(s));
                        env.pack = pack;
                        let v = env.eval(e)?;
                        s[i] = v;
                    }
                }
            }
            SStepKind::When(e) => {
                started = true;
                steps += 1;
                let (aname, args) = match &e.kind {
                    ExprKind::Name(n) => (n.clone(), vec![]),
                    ExprKind::Call(f, args) => match &f.kind {
                        ExprKind::Name(n) => (n.clone(), args.clone()),
                        _ => return Err(Diag::error(e.span, "`when` names an action: `when Draw(150 cr)`")),
                    },
                    _ => return Err(Diag::error(e.span, "`when` names an action: `when Draw(150 cr)`")),
                };
                let Some(a) = ctx.action(&aname) else {
                    return Err(Diag::error(e.span, format!("{mname} has no action `{aname}`")));
                };
                if args.len() != a.params.len() {
                    return Err(Diag::error(e.span, format!("{aname} takes {} argument(s), got {}", a.params.len(), args.len())));
                }
                let mut next_states = Vec::new();
                for s in &states {
                    let mut params = Vec::new();
                    for (p, arg) in a.params.iter().zip(&args) {
                        if arg.name.as_ref().is_some_and(|n| *n != p.name) {
                            return Err(Diag::error(arg.value.span, format!("{aname} has parameter `{}` here", p.name)));
                        }
                        let mut env = Env::machine(prog, &ctx, Some(s));
                        env.pack = pack;
                        let v = env.eval(&arg.value)?;
                        // `a in S` parameters must be in S when S can be evaluated here.
                        if let ADom::In(dom) = &p.dom {
                            if let Ok(Value::Set(set)) = Env::machine(prog, &ctx, Some(s)).eval(dom) {
                                if !set.contains(&v) {
                                    return Ok(Outcome::Fail(format!("{}: {v} is not in the domain of `{}`", prog.locate(arg.value.span), p.name)));
                                }
                            }
                        }
                        params.push((p.name.clone(), v));
                    }
                    next_states.extend(ctx.step(prog, a, &params, s)?);
                }
                if next_states.is_empty() {
                    pending_reject = Some(prog.sources.snippet(e.span).to_string());
                } else {
                    next_states.dedup();
                    states = next_states;
                }
            }
            SStepKind::Then(e) => {
                for s in &states {
                    let mut env = Env::machine(prog, &ctx, Some(s));
                    env.pack = pack;
                    let v = env.eval(e)?;
                    if v != Value::Bool(true) {
                        return Ok(Outcome::Fail(format!(
                            "{}: `then {}` is {v} in {}",
                            prog.locate(e.span),
                            prog.sources.snippet(e.span),
                            ctx.show(s, None)
                        )));
                    }
                }
            }
            SStepKind::Rejected(_) => {
                if pending_reject.take().is_none() {
                    return Ok(Outcome::Fail(format!("{}: expected the last `when` to be rejected, but it was enabled", prog.locate(st.span))));
                }
            }
            SStepKind::Fixture(_) => unreachable!(),
        }
    }
    if let Some(a) = pending_reject {
        return Ok(Outcome::Fail(format!("{a} is not enabled in {}", show_all(&ctx, &states))));
    }
    let n = states.len();
    Ok(Outcome::Pass(format!("{mname}, {steps} step(s){}", if n > 1 { format!(", {n} final states") } else { String::new() })))
}

fn show_all(ctx: &MachineCtx, states: &[State]) -> String {
    states.iter().map(|s| format!("[{}]", ctx.show(s, None))).collect::<Vec<_>>().join(" or ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_all(src: &str) -> Vec<Outcome> {
        let (p, d) = Program::load(vec![("t.jhrl".into(), src.into())]);
        assert!(d.is_empty(), "{d:?}");
        p.items()
            .filter_map(|(pk, _, it)| if let Item::Scenario(s) = it { Some(run(&p, pk, s)) } else { None })
            .collect()
    }

    const M: &str = "machine L {\n const Cap: Credits = 5 cr\n var used: Credits = 0 cr\n\
        action Draw(n in {1 cr, 2 cr, 150 cr}) { when used + n <= Cap\n used' = used + n }\n}\n";

    #[test]
    fn machine_scenarios() {
        let out = run_all(&format!(
            "{M}scenario AP-000.R1 \"ok\" of L {{\n given Cap = 300 cr\n when Draw(150 cr)\n then used == 150 cr\n}}\n\
             scenario AP-000.R1 \"rejected\" of L {{\n when Draw(150 cr)\n then rejected\n}}\n\
             scenario AP-000.R1 \"not enabled\" of L {{\n when Draw(150 cr)\n then used == 150 cr\n}}\n\
             scenario AP-000.R1 \"wrong then\" of L {{\n when Draw(1 cr)\n then used == 2 cr\n}}\n\
             scenario AP-000.R1 \"given var\" of L {{\n given used = 4 cr\n when Draw(1 cr)\n then used == Cap\n}}\n\
             scenario AP-000.R1 \"outside domain\" of L {{\n when Draw(3 cr)\n then used == 3 cr\n}}\n"
        ));
        assert!(matches!(out[0], Outcome::Pass(_)), "{:?}", out[0]);
        assert!(matches!(out[1], Outcome::Pass(_)), "{:?}", out[1]);
        assert!(matches!(&out[2], Outcome::Fail(m) if m.contains("not enabled")), "{:?}", out[2]);
        assert!(matches!(&out[3], Outcome::Fail(m) if m.contains("is false")), "{:?}", out[3]);
        assert!(matches!(out[4], Outcome::Pass(_)), "{:?}", out[4]);
        assert!(matches!(&out[5], Outcome::Fail(m) if m.contains("not in the domain")), "{:?}", out[5]);
    }

    #[test]
    fn pure_and_fixture_scenarios() {
        let out = run_all(
            "fn half(x: Number) -> Number = x / 2\n\
             scenario AP-000.R1 \"pure\" {\n then half(3) == 1.5 and half(0) == 0\n}\n\
             scenario AP-000.R1 \"pure fails\" {\n then half(3) == 1\n}\n\
             scenario AP-000.R1 \"fixture\" {\n given fixture \"x.json\"\n then true\n}\n",
        );
        assert!(matches!(out[0], Outcome::Pass(_)));
        assert!(matches!(out[1], Outcome::Fail(_)));
        assert!(matches!(out[2], Outcome::Skip(_)));
    }
}
