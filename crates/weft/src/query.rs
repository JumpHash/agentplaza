//! Queries over changes, after Jujutsu's revsets: `lane(t51)`, `landed()`, `touches(crates/apid/**)`,
//! `by(agent:107) & since(7d)`, `queued() | conflicts()`, `landed() ~ imported()`.
//!
//! | Function | Changes |
//! |---|---|
//! | `all()` | every change |
//! | `lane(t51)`, `task(t51)`, `change(w-12)` | the change of a task, or by name |
//! | `open()`, `sealed()`, `queued()`, `conflicts()`, `landed()`, `abandoned()` | by state |
//! | `touches(glob)`, `touches(path::symbol)` | changing those paths (or that definition) |
//! | `by(actor)` | written in part by an actor (any draft or round) |
//! | `author(actor)` | whose lane belongs to an actor |
//! | `since(7d)`, `since(12h)`, `since(v0.8.0)`, `since(w-40)` | opened after a time, a release or a change |
//! | `imported()`, `reverts()`, `edits()` | imported from git, reverts, the owner's direct edits |
//! | `reviewed()`, `needs(owner)` | with a review record; waiting on the owner's ruling |
//! | `diverges(t53)` | sharing a file with another open change |
//! | `kind(fix)` | with an intent of that kind |
//! | `component(name)` | changing a component of the project map |
//!
//! `a & b` both, `a | b` either, `a ~ b` a but not b, `~a` not a, parentheses group.

use std::collections::BTreeSet;

use crate::derived::glob;
use crate::record::Actor;
use crate::state::{ChangeState, State};

#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    Fn(String, String),
    And(Box<Query>, Box<Query>),
    Or(Box<Query>, Box<Query>),
    Minus(Box<Query>, Box<Query>),
    Not(Box<Query>),
}

pub fn parse(text: &str) -> Result<Query, String> {
    let tokens = tokenize(text)?;
    let mut p = Parser { tokens, at: 0 };
    let q = p.expr()?;
    if p.at != p.tokens.len() {
        return Err(format!("unexpected {:?}", p.tokens[p.at]));
    }
    Ok(q)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Call(String, String),
    And,
    Or,
    Tilde,
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' => i += 1,
            '&' => {
                out.push(Tok::And);
                i += 1
            }
            '|' => {
                out.push(Tok::Or);
                i += 1
            }
            '~' => {
                out.push(Tok::Tilde);
                i += 1
            }
            '(' => {
                out.push(Tok::Open);
                i += 1
            }
            ')' => {
                out.push(Tok::Close);
                i += 1
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                if chars.get(i) != Some(&'(') {
                    return Err(format!(
                        "`{name}` needs its arguments in parentheses, like `{name}()`"
                    ));
                }
                i += 1;
                let arg_start = i;
                let mut depth = 0;
                while i < chars.len() && !(chars[i] == ')' && depth == 0) {
                    if chars[i] == '(' {
                        depth += 1;
                    } else if chars[i] == ')' {
                        depth -= 1;
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(format!("`{name}(` is not closed"));
                }
                let arg: String = chars[arg_start..i].iter().collect();
                out.push(Tok::Call(name, arg.trim().to_string()));
                i += 1;
            }
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at)
    }

    fn expr(&mut self) -> Result<Query, String> {
        let mut left = self.term()?;
        while self.peek() == Some(&Tok::Or) {
            self.at += 1;
            left = Query::Or(Box::new(left), Box::new(self.term()?));
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Query, String> {
        let mut left = self.factor()?;
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.at += 1;
                    left = Query::And(Box::new(left), Box::new(self.factor()?));
                }
                Some(Tok::Tilde) => {
                    self.at += 1;
                    left = Query::Minus(Box::new(left), Box::new(self.factor()?));
                }
                _ => return Ok(left),
            }
        }
    }

    fn factor(&mut self) -> Result<Query, String> {
        match self.tokens.get(self.at).cloned() {
            Some(Tok::Tilde) => {
                self.at += 1;
                Ok(Query::Not(Box::new(self.factor()?)))
            }
            Some(Tok::Open) => {
                self.at += 1;
                let q = self.expr()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err("a `(` is not closed".into());
                }
                self.at += 1;
                Ok(q)
            }
            Some(Tok::Call(name, arg)) => {
                self.at += 1;
                Ok(Query::Fn(name, arg))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the query ends too soon".into()),
        }
    }
}

/// What a query needs beyond the state.
pub struct Context<'a> {
    pub state: &'a State,
    /// Only changes in this repo (all repos when `None`).
    pub repo: Option<&'a str>,
    pub now_ms: u64,
    /// The paths a change's latest revision touches.
    pub paths: &'a dyn Fn(&State, u64) -> Vec<String>,
    /// The definitions (`path::qualified`) a change's latest revision changes.
    pub symbols: &'a dyn Fn(u64) -> Vec<String>,
    /// A project map component's globs.
    pub component: &'a dyn Fn(&str) -> Vec<String>,
}

/// The arguments of every call to `name` in a query.
pub fn calls<'a>(q: &'a Query, name: &str) -> Vec<&'a str> {
    match q {
        Query::Fn(n, a) if n == name => vec![a.as_str()],
        Query::Fn(..) => vec![],
        Query::And(a, b) | Query::Or(a, b) | Query::Minus(a, b) => {
            let mut v = calls(a, name);
            v.extend(calls(b, name));
            v
        }
        Query::Not(a) => calls(a, name),
    }
}

pub fn eval(q: &Query, cx: &Context) -> Result<BTreeSet<u64>, String> {
    let all = || -> BTreeSet<u64> {
        cx.state
            .changes
            .values()
            .filter(|c| cx.repo.is_none_or(|r| c.repo == r))
            .map(|c| c.id)
            .collect()
    };
    Ok(match q {
        Query::And(a, b) => eval(a, cx)?.intersection(&eval(b, cx)?).copied().collect(),
        Query::Or(a, b) => eval(a, cx)?.union(&eval(b, cx)?).copied().collect(),
        Query::Minus(a, b) => eval(a, cx)?.difference(&eval(b, cx)?).copied().collect(),
        Query::Not(a) => all().difference(&eval(a, cx)?).copied().collect(),
        Query::Fn(name, arg) => {
            let st = cx.state;
            let pick = |f: &dyn Fn(&crate::state::Change) -> bool| -> BTreeSet<u64> {
                all()
                    .into_iter()
                    .filter(|id| st.changes.get(id).is_some_and(f))
                    .collect()
            };
            match name.as_str() {
                "all" => all(),
                "lane" | "task" | "change" => {
                    pick(&|c| c.task.as_deref() == Some(arg) || c.name == *arg)
                }
                "open" => pick(&|c| {
                    matches!(
                        c.state,
                        ChangeState::Open
                            | ChangeState::Sealed
                            | ChangeState::Queued
                            | ChangeState::Conflict
                    )
                }),
                "sealed" => pick(&|c| c.state == ChangeState::Sealed),
                "queued" => pick(&|c| {
                    c.state == ChangeState::Queued
                        && (arg.is_empty()
                            || st.repos.get(&c.repo).and_then(|r| r.project.as_deref())
                                == Some(arg)
                            || c.repo == *arg)
                }),
                "conflicts" => pick(&|c| c.state == ChangeState::Conflict),
                "landed" => pick(&|c| c.state == ChangeState::Landed),
                "abandoned" => pick(&|c| c.state == ChangeState::Abandoned),
                "imported" => pick(&|c| c.import.is_some()),
                "reverts" => pick(&|c| c.kind == "revert"),
                "edits" => pick(&|c| c.kind == "owner-edit"),
                "kind" => pick(&|c| c.intents.values().flatten().any(|i| i.kind == *arg)),
                "author" => {
                    let a = Actor::parse(arg)?;
                    pick(&|c| c.author == a)
                }
                "by" => {
                    let a = Actor::parse(arg)?;
                    pick(&|c| {
                        c.author == a
                            || c.drafts
                                .iter()
                                .chain(&c.rounds)
                                .any(|r| st.revisions.get(r).is_some_and(|r| r.actor == a))
                    })
                }
                "reviewed" => pick(&|c| {
                    c.records.iter().any(|s| {
                        st.records
                            .get(s)
                            .is_some_and(|r| r.attest.predicate_type == "review")
                    })
                }),
                "needs" => {
                    if arg != "owner" {
                        return Err("needs() takes `owner`".into());
                    }
                    pick(&|c| {
                        let types: Vec<(&str, &serde_json::Value)> = c
                            .records
                            .iter()
                            .filter_map(|s| st.records.get(s))
                            .map(|r| (r.attest.predicate_type.as_str(), &r.attest.predicate))
                            .collect();
                        let asked = types
                            .iter()
                            .rposition(|(t, p)| *t == "gate" && p["outcome"] == "owner");
                        let ruled = types.iter().rposition(|(t, _)| *t == "ruling");
                        asked.is_some() && ruled.is_none_or(|r| r < asked.unwrap())
                    })
                }
                "since" => {
                    let from = since(st, arg, cx.now_ms)?;
                    pick(&|c| c.opened_seq > from.0 || c.opened_ms >= from.1)
                }
                "touches" => {
                    let (path, sym) = match arg.split_once("::") {
                        Some((p, s)) => (p, Some(s)),
                        None => (arg.as_str(), None),
                    };
                    all()
                        .into_iter()
                        .filter(|id| match sym {
                            None => (cx.paths)(st, *id).iter().any(|p| {
                                glob(path, p)
                                    || p.starts_with(&format!("{}/", path.trim_end_matches('/')))
                            }),
                            Some(s) => (cx.symbols)(*id).iter().any(|item| {
                                item.split_once("::").is_some_and(|(p, q)| {
                                    glob(path, p) && (q == s || q.ends_with(&format!(" {s}")))
                                })
                            }),
                        })
                        .collect()
                }
                "component" => {
                    let globs = (cx.component)(arg);
                    if globs.is_empty() {
                        return Err(format!("no component {arg} in the map"));
                    }
                    all()
                        .into_iter()
                        .filter(|id| {
                            (cx.paths)(st, *id)
                                .iter()
                                .any(|p| globs.iter().any(|g| glob(g, p)))
                        })
                        .collect()
                }
                "diverges" => {
                    let other = st
                        .changes
                        .values()
                        .find(|c| c.task.as_deref() == Some(arg) || c.name == *arg)
                        .ok_or_else(|| format!("no change for {arg}"))?;
                    let theirs: BTreeSet<String> = (cx.paths)(st, other.id).into_iter().collect();
                    let me = other.id;
                    all()
                        .into_iter()
                        .filter(|id| {
                            *id != me
                                && st.changes[id].state != ChangeState::Landed
                                && st.changes[id].state != ChangeState::Abandoned
                        })
                        .filter(|id| (cx.paths)(st, *id).iter().any(|p| theirs.contains(p)))
                        .collect()
                }
                other => return Err(format!("no query function {other}()")),
            }
        }
    })
}

/// `(seq, ms)`: changes opened after this log position or time.
fn since(st: &State, arg: &str, now_ms: u64) -> Result<(u64, u64), String> {
    let unit = |s: &str, mult: u64| {
        s.parse::<u64>()
            .map(|n| (u64::MAX, now_ms.saturating_sub(n * mult)))
            .map_err(|_| format!("since({arg})"))
    };
    if let Some(n) = arg.strip_suffix('d') {
        return unit(n, 86_400_000);
    }
    if let Some(n) = arg.strip_suffix('h') {
        return unit(n, 3_600_000);
    }
    if let Some(n) = arg.strip_suffix('m') {
        return unit(n, 60_000);
    }
    if arg.starts_with("w-") {
        let c = st.change(arg)?;
        return Ok((c.opened_seq, u64::MAX));
    }
    for repo in st.repos.values() {
        if let Some(r) = repo.releases.get(arg) {
            return Ok((
                r.signed.as_ref().map(|s| s.1).unwrap_or(r.proposed_seq),
                u64::MAX,
            ));
        }
    }
    Err(format!(
        "since() takes 7d, 12h, 30m, a release or a change, not {arg}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combinations() {
        let q = parse("by(agent:107) & since(7d) | ~landed() ~ imported()").unwrap();
        match q {
            Query::Or(a, b) => {
                assert!(matches!(*a, Query::And(..)));
                assert!(matches!(*b, Query::Minus(..)));
            }
            other => panic!("{other:?}"),
        }
        assert!(parse("landed").is_err());
        assert!(parse("(landed()").is_err());
        assert_eq!(
            parse("touches(crates/apid/src/nodes.rs::memory_mib)").unwrap(),
            Query::Fn(
                "touches".into(),
                "crates/apid/src/nodes.rs::memory_mib".into()
            )
        );
    }
}
