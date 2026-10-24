//! `jhrl check <files…>` parses and checks every file, model-checks every model, and runs every
//! mutant and witness. `jhrl test <files…>` runs the scenarios. One `ok`, `FAIL` or `skip` line
//! per item; the exit code is 1 on any failure.
//!
//! Options: `-v` prints traces for caught mutants and reached witnesses; `--tier pack=tier`
//! sets a pack's trust tier (default: system).

use crate::ast::Item;
use crate::check::check_program;
use crate::mc::{self, Options, Report};
use crate::program::{DeclRef, Program};
use crate::resolve::Tier;
use crate::scenario::{self, Outcome};
use crate::span::Diag;
use std::collections::HashMap;
use std::io::Write;

const USAGE: &str = "usage: jhrl check [-v] [--tier pack=tier] <files…>\n       jhrl test [-v] [--tier pack=tier] <files…>";

struct Ctx<'w> {
    out: &'w mut dyn Write,
    failures: usize,
    skips: usize,
    verbose: bool,
}

impl Ctx<'_> {
    fn ok(&mut self, s: &str) {
        let _ = writeln!(self.out, "ok    {s}");
    }
    fn fail(&mut self, s: &str) {
        self.failures += 1;
        let _ = writeln!(self.out, "FAIL  {s}");
    }
    fn skip(&mut self, s: &str) {
        self.skips += 1;
        let _ = writeln!(self.out, "skip  {s}");
    }
    fn detail(&mut self, s: &str) {
        for l in s.lines() {
            let _ = writeln!(self.out, "      {l}");
        }
    }
}

pub fn main(args: &[String], out: &mut dyn Write) -> i32 {
    let Some(cmd) = args.first() else {
        let _ = writeln!(out, "{USAGE}");
        return 2;
    };
    let mut files = Vec::new();
    let mut verbose = false;
    let mut tiers = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--tier" => match it.next().and_then(|s| s.split_once('=')) {
                Some((p, t)) => match Tier::parse(t) {
                    Some(t) => tiers.push((p.to_string(), t)),
                    None => {
                        let _ = writeln!(out, "unknown tier `{t}` (constitution, statute, system, plaza, contract, agent)");
                        return 2;
                    }
                },
                None => {
                    let _ = writeln!(out, "--tier takes pack=tier");
                    return 2;
                }
            },
            _ => files.push(a.clone()),
        }
    }
    if !matches!(cmd.as_str(), "check" | "test") || files.is_empty() {
        let _ = writeln!(out, "{USAGE}");
        return 2;
    }
    let mut c = Ctx {
        out,
        failures: 0,
        skips: 0,
        verbose,
    };
    let mut sources = Vec::new();
    for f in &files {
        match std::fs::read_to_string(f) {
            Ok(t) => sources.push((f.clone(), t)),
            Err(e) => c.fail(&format!("read {f}: {e}")),
        }
    }
    let (mut prog, load_diags) = Program::load(sources);
    for (p, t) in tiers {
        if !prog.set_tier(&p, t) {
            c.fail(&format!("--tier: no loaded pack `{p}`"));
        }
    }
    let static_diags = check_program(&prog);
    report_files(&mut c, &prog, &load_diags, &static_diags, cmd == "check");
    if cmd == "check" {
        check_models(&mut c, &prog);
    } else {
        run_scenarios(&mut c, &prog);
        run_authz_cases(&mut c, &prog);
    }
    let summary = if c.failures == 0 {
        "all ok".to_string()
    } else {
        format!("{} FAILURE(S)", c.failures)
    };
    let skips = if c.skips > 0 {
        format!(", {} skipped", c.skips)
    } else {
        String::new()
    };
    let _ = writeln!(
        c.out,
        "jhrl {cmd}: {} file(s), {summary}{skips}",
        files.len()
    );
    if c.failures > 0 {
        1
    } else {
        0
    }
}

fn report_files(c: &mut Ctx, prog: &Program, load: &[Diag], stat: &[Diag], print_ok: bool) {
    for (i, src) in prog.sources.files.iter().enumerate() {
        let mine = |d: &&Diag| d.span.map(|s| s.file as usize) == Some(i);
        let ld: Vec<&Diag> = load.iter().filter(mine).collect();
        let sd: Vec<&Diag> = stat.iter().filter(mine).collect();
        let errs: Vec<&&Diag> = ld.iter().chain(sd.iter()).filter(|d| !d.warning).collect();
        let n = prog.modules[i].items.len();
        if errs.is_empty() {
            if print_ok {
                c.ok(&format!(
                    "{}: {n} declarations parse; names and dimensions check",
                    src.path
                ));
            }
        } else {
            let what = if ld.iter().any(|d| !d.warning) {
                "parse"
            } else {
                "names and dimensions"
            };
            c.fail(&format!("{}: {} error(s) ({what})", src.path, errs.len()));
            for d in errs {
                c.detail(&d.render(&prog.sources));
            }
        }
        for d in ld.iter().chain(sd.iter()).filter(|d| d.warning) {
            c.detail(&d.render(&prog.sources));
        }
    }
    for d in load.iter().chain(stat).filter(|d| d.span.is_none()) {
        c.fail(&d.render(&prog.sources));
    }
}

fn check_models(c: &mut Ctx, prog: &Program) {
    let mut reports: HashMap<DeclRef, Report> = HashMap::new();
    for (pack, r, it) in prog.items() {
        let Item::Model(m) = it else { continue };
        let head = format!("model {} \"{}\"", m.machine, m.name);
        match mc::explore(prog, pack, m, &Options::default()) {
            Err(d) => c.fail(&format!("{head}: {}", d.render(&prog.sources))),
            Ok(rep) => {
                if rep.violations.is_empty() {
                    let mut holds = format!("{} invariant(s)", rep.invariants.len());
                    if !rep.transitions.is_empty() {
                        holds.push_str(&format!(" and {} transition(s)", rep.transitions.len()));
                    }
                    let dl = if rep.deadlock_checked {
                        ", no deadlock"
                    } else {
                        ""
                    };
                    c.ok(&format!(
                        "{head}: {} distinct states, depth {}, {holds} hold{dl} ({} generated)",
                        rep.states.len(),
                        rep.depth,
                        rep.generated
                    ));
                } else {
                    let v = &rep.violations[0];
                    c.fail(&format!(
                        "{head}: {} ({} distinct states so far)",
                        v.what.describe(),
                        rep.states.len()
                    ));
                    c.detail(&rep.render_trace(v, "  "));
                }
                for p in &rep.properties {
                    c.skip(&format!(
                        "{head}: property \"{p}\" (liveness checking is not in v0)"
                    ));
                }
                for n in &rep.notes {
                    c.detail(n);
                }
                reports.insert(r, rep);
            }
        }
    }
    for (pack, _, it) in prog.items() {
        let Item::Mutant(m) = it else { continue };
        let head = format!("mutant {} \"{}\"", m.machine, m.name);
        let o = mc::run_mutant(prog, pack, m);
        if o.caught {
            c.ok(&format!("{head} {}", o.line));
            if c.verbose {
                for d in &o.detail {
                    c.detail(d);
                }
            }
        } else {
            c.fail(&format!("{head} {}", o.line));
        }
    }
    for (pack, _, it) in prog.items() {
        let Item::Witness(w) = it else { continue };
        let head = format!("witness {} \"{}\"", w.machine, w.name);
        let Some(mref) = prog.lookup_decl(pack, &w.machine) else {
            c.fail(&format!("{head}: unknown machine"));
            continue;
        };
        let models: Vec<DeclRef> = mc::models_of(prog, mref)
            .into_iter()
            .filter(|(_, _, md)| w.model.as_ref().is_none_or(|n| *n == md.name))
            .map(|x| x.1)
            .collect();
        if models.is_empty() {
            c.fail(&format!(
                "{head}: no model{}",
                w.model
                    .as_ref()
                    .map(|n| format!(" \"{n}\""))
                    .unwrap_or_default()
            ));
            continue;
        }
        let mut hit = None;
        for mr in models {
            let Some(rep) = reports.get(&mr) else {
                continue;
            };
            match rep.find(prog, pack, &w.expr) {
                Ok(Some(i)) => {
                    hit = Some((rep, i));
                    break;
                }
                Ok(None) => {}
                Err(d) => {
                    c.fail(&format!("{head}: {}", d.render(&prog.sources)));
                    hit = None;
                    break;
                }
            }
        }
        match hit {
            Some((rep, i)) => {
                c.ok(&format!(
                    "{head} reachable in \"{}\" at depth {}",
                    rep.model, rep.states[i].depth
                ));
                if c.verbose {
                    let v = mc::Violation {
                        what: mc::Found::Deadlock,
                        state: i,
                        step: None,
                    };
                    let t = rep.render_trace(&v, "  ");
                    c.detail(&t);
                }
            }
            None => c.fail(&format!(
                "{head} is unreachable (its invariants may pass vacuously)"
            )),
        }
    }
}

fn run_authz_cases(c: &mut Ctx, prog: &Program) {
    for (label, pass, detail) in crate::authz::run_authz(prog) {
        if pass {
            c.ok(&label);
        } else {
            c.fail(&label);
            c.detail(&detail);
        }
    }
}

fn run_scenarios(c: &mut Ctx, prog: &Program) {
    for (pack, _, it) in prog.items() {
        let Item::Scenario(s) = it else { continue };
        let head = format!("scenario {} \"{}\"", s.req, s.name);
        match scenario::run(prog, pack, s) {
            Outcome::Pass(m) => c.ok(&format!("{head} ({m})")),
            Outcome::Fail(m) => {
                c.fail(&head);
                c.detail(&m);
            }
            Outcome::Skip(m) => c.skip(&format!("{head}: {m}")),
        }
    }
}
