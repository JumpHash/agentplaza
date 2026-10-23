//! Constraint resolution across trust tiers (docs/jhrl.md section 10, AP-074.R3).
//!
//! 1. The caller collects the rules whose `where` is certainly true (the candidates).
//! 2. Within a tier the highest priority wins; ties across packs meet (with a warning), ties
//!    inside one pack are an error; `floor` rules always join their tier's result by meet.
//! 3. Tiers combine by meet in the kind's direction, so a lower tier can only tighten.
//! 4. An empty meet (exact windows that don't overlap) is an error naming both rules.
//! 5. `prefer` skips the meet: the highest tier and priority wins.
//!
//! The default is not a rule: it is used only when no rule matches in any tier.

use crate::ast::Direction;
use crate::num::{Dim, Num};
use std::cmp::Ordering;
use std::fmt;

/// Trust tiers, highest first. `Ord` follows the listing: `Constitution < Agent` means
/// "constitution comes first".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Constitution,
    Statute,
    System,
    Plaza,
    Contract,
    Agent,
}

impl Tier {
    pub const ALL: [Tier; 6] = [
        Tier::Constitution,
        Tier::Statute,
        Tier::System,
        Tier::Plaza,
        Tier::Contract,
        Tier::Agent,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Tier::Constitution => "constitution",
            Tier::Statute => "statute",
            Tier::System => "system",
            Tier::Plaza => "plaza",
            Tier::Contract => "contract",
            Tier::Agent => "agent",
        }
    }
    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// A quantity in base units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Q(pub Num, pub Dim);

impl fmt::Display for Q {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.0, self.1.unit_suffix())
    }
}

/// A rule's value: a point, or for `exact` kinds a window `[lo, hi]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    Point(Q),
    Window(Q, Q),
}

impl Bound {
    fn lo(self) -> Q {
        match self {
            Bound::Point(q) | Bound::Window(q, _) => q,
        }
    }
    fn hi(self) -> Q {
        match self {
            Bound::Point(q) | Bound::Window(_, q) => q,
        }
    }
    fn dim(self) -> Dim {
        self.lo().1
    }
    /// The single value, when the bound is a point (or a window of width zero).
    pub fn point(self) -> Option<Q> {
        let (a, b) = (self.lo(), self.hi());
        (a.0 == b.0).then_some(a)
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bound::Point(q) => write!(f, "{q}"),
            Bound::Window(a, b) => write!(f, "[{a}, {b}]"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub rule: String,
    pub pack: String,
    pub tier: Tier,
    pub priority: i64,
    pub floor: bool,
    pub value: Bound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TierResult {
    pub tier: Tier,
    pub candidates: Vec<Candidate>,
    /// Rules that produced the tier's value (the priority winners and the floors).
    pub winners: Vec<String>,
    pub value: Bound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Rules,
    Default,
    /// No rule and no default: the limit is open (unbounded in its direction).
    Open,
}

/// Why a resolved value is what it is (section 12, "Explanations").
#[derive(Clone, Debug, PartialEq)]
pub struct Explanation {
    pub kind: String,
    pub direction: Direction,
    pub tiers: Vec<TierResult>,
    pub value: Option<Bound>,
    pub source: Source,
    pub decided_by: Vec<String>,
    /// Rules that matched but did not change the result (lost on priority, or weaker than a
    /// higher tier, e.g. a plaza tier trying to loosen a system limit).
    pub no_effect: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolveError {
    EmptyMeet {
        kind: String,
        a: String,
        b: String,
        detail: String,
    },
    SamePackTie {
        kind: String,
        a: String,
        b: String,
        pack: String,
    },
    Dimension {
        kind: String,
        rule: String,
        expected: Dim,
        found: Dim,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::EmptyMeet { kind, a, b, detail } => {
                write!(
                    f,
                    "{kind}: empty meet between \"{a}\" and \"{b}\" ({detail})"
                )
            }
            ResolveError::SamePackTie { kind, a, b, pack } => {
                write!(
                    f,
                    "{kind}: rules \"{a}\" and \"{b}\" in pack {pack} tie on priority"
                )
            }
            ResolveError::Dimension {
                kind,
                rule,
                expected,
                found,
            } => write!(
                f,
                "{kind}: rule \"{rule}\" gives a {} where the kind is {}",
                found.describe(),
                expected.describe()
            ),
        }
    }
}

fn cmpq(a: Q, b: Q) -> Ordering {
    a.0.cmp_num(b.0)
}

/// Meet of two bounds in a direction; `Err` carries a description of an empty meet.
fn meet(dir: Direction, a: Bound, b: Bound) -> Result<Bound, String> {
    Ok(match dir {
        Direction::Max => {
            if cmpq(b.lo(), a.lo()) == Ordering::Less {
                b
            } else {
                a
            }
        }
        Direction::Min => {
            if cmpq(b.lo(), a.lo()) == Ordering::Greater {
                b
            } else {
                a
            }
        }
        Direction::Exact => {
            let lo = if cmpq(a.lo(), b.lo()) == Ordering::Less {
                b.lo()
            } else {
                a.lo()
            };
            let hi = if cmpq(a.hi(), b.hi()) == Ordering::Greater {
                b.hi()
            } else {
                a.hi()
            };
            if cmpq(lo, hi) == Ordering::Greater {
                return Err(format!("{a} and {b} don't overlap"));
            }
            if lo.0 == hi.0 {
                Bound::Point(lo)
            } else {
                Bound::Window(lo, hi)
            }
        }
        Direction::Prefer => a,
    })
}

/// Is `a` at least as tight as `b` in this direction?
pub fn at_least_as_tight(dir: Direction, a: Bound, b: Bound) -> bool {
    match dir {
        Direction::Max => cmpq(a.lo(), b.lo()) != Ordering::Greater,
        Direction::Min => cmpq(a.lo(), b.lo()) != Ordering::Less,
        Direction::Exact => {
            cmpq(a.lo(), b.lo()) != Ordering::Less && cmpq(a.hi(), b.hi()) != Ordering::Greater
        }
        Direction::Prefer => true,
    }
}

/// The rule that sets the binding side of an exact meet, for error messages.
fn exact_culprit(cands: &[(String, Bound)], lo_side: bool) -> &str {
    let mut best: Option<&(String, Bound)> = None;
    for c in cands {
        best = match best {
            None => Some(c),
            Some(b) => {
                let better = if lo_side {
                    cmpq(c.1.lo(), b.1.lo()) == Ordering::Greater
                } else {
                    cmpq(c.1.hi(), b.1.hi()) == Ordering::Less
                };
                if better {
                    Some(c)
                } else {
                    Some(b)
                }
            }
        }
    }
    best.map(|b| b.0.as_str()).unwrap_or("?")
}

pub fn resolve(
    kind: &str,
    dir: Direction,
    dim: Dim,
    default: Option<Bound>,
    cands: Vec<Candidate>,
) -> Result<Explanation, ResolveError> {
    let mut ex = Explanation {
        kind: kind.to_string(),
        direction: dir,
        tiers: vec![],
        value: None,
        source: Source::Rules,
        decided_by: vec![],
        no_effect: vec![],
        warnings: vec![],
    };
    for c in &cands {
        if c.value.dim() != dim {
            return Err(ResolveError::Dimension {
                kind: kind.into(),
                rule: c.rule.clone(),
                expected: dim,
                found: c.value.dim(),
            });
        }
    }
    if cands.is_empty() {
        ex.value = default;
        ex.source = if default.is_some() {
            Source::Default
        } else {
            Source::Open
        };
        ex.decided_by = vec![if default.is_some() {
            "default".into()
        } else {
            "open (no rule, no default)".into()
        }];
        return Ok(ex);
    }
    // Resolve within each tier.
    for tier in Tier::ALL {
        let mut in_tier: Vec<Candidate> =
            cands.iter().filter(|c| c.tier == tier).cloned().collect();
        if in_tier.is_empty() {
            continue;
        }
        in_tier.sort_by_key(|c| std::cmp::Reverse(c.priority));
        let regular: Vec<&Candidate> = in_tier.iter().filter(|c| !c.floor).collect();
        let floors: Vec<&Candidate> = in_tier.iter().filter(|c| c.floor).collect();
        let mut winners: Vec<&Candidate> = Vec::new();
        if let Some(top) = regular.first() {
            let tied: Vec<&Candidate> = regular
                .iter()
                .copied()
                .filter(|c| c.priority == top.priority)
                .collect();
            for (i, a) in tied.iter().enumerate() {
                for b in &tied[i + 1..] {
                    if a.pack == b.pack {
                        return Err(ResolveError::SamePackTie {
                            kind: kind.into(),
                            a: a.rule.clone(),
                            b: b.rule.clone(),
                            pack: a.pack.clone(),
                        });
                    }
                }
            }
            if tied.len() > 1 {
                ex.warnings.push(format!(
                    "{} tier: rules from different packs tie on priority {} and combine by meet: {}",
                    tier.name(),
                    top.priority,
                    tied.iter().map(|c| format!("\"{}\"", c.rule)).collect::<Vec<_>>().join(", ")
                ));
            }
            for c in regular.iter().filter(|c| c.priority < top.priority) {
                ex.no_effect.push(c.rule.clone());
            }
            winners.extend(tied);
        }
        winners.extend(floors);
        let mut value = winners[0].value;
        let mut seen: Vec<(String, Bound)> = vec![(winners[0].rule.clone(), winners[0].value)];
        for w in &winners[1..] {
            if dir == Direction::Prefer {
                break;
            }
            seen.push((w.rule.clone(), w.value));
            value = meet(dir, value, w.value).map_err(|detail| ResolveError::EmptyMeet {
                kind: kind.into(),
                a: exact_culprit(&seen, true).to_string(),
                b: exact_culprit(&seen, false).to_string(),
                detail,
            })?;
        }
        let names = if dir == Direction::Prefer {
            vec![winners[0].rule.clone()]
        } else {
            winners.iter().map(|w| w.rule.clone()).collect()
        };
        ex.tiers.push(TierResult {
            tier,
            candidates: in_tier.clone(),
            winners: names,
            value,
        });
    }
    // Combine across tiers.
    if dir == Direction::Prefer {
        let t = &ex.tiers[0];
        ex.value = Some(t.value);
        ex.decided_by = vec![t.winners[0].clone()];
        for t2 in &ex.tiers[1..] {
            ex.no_effect.extend(t2.winners.iter().cloned());
        }
        return Ok(ex);
    }
    let mut acc = ex.tiers[0].value;
    let mut seen: Vec<(String, Bound)> = ex.tiers[0]
        .winners
        .iter()
        .map(|w| (w.clone(), ex.tiers[0].value))
        .collect();
    for t in &ex.tiers[1..] {
        let before = acc;
        for w in &t.winners {
            seen.push((w.clone(), t.value));
        }
        acc = meet(dir, acc, t.value).map_err(|detail| ResolveError::EmptyMeet {
            kind: kind.into(),
            a: exact_culprit(&seen, true).to_string(),
            b: exact_culprit(&seen, false).to_string(),
            detail,
        })?;
        if acc == before && t.value != before {
            // Weaker than (or equal-but-different window to) a higher tier: no effect.
            ex.no_effect.extend(t.winners.iter().cloned());
        }
    }
    ex.value = Some(acc);
    // Decided by: the highest tier whose result is the final value; for windows, the rules
    // that set each side.
    match dir {
        Direction::Exact => {
            let lo = exact_culprit(&seen, true).to_string();
            let hi = exact_culprit(&seen, false).to_string();
            ex.decided_by = if lo == hi { vec![lo] } else { vec![lo, hi] };
        }
        _ => {
            if let Some(t) = ex.tiers.iter().find(|t| t.value.lo().0 == acc.lo().0) {
                ex.decided_by = t.winners.clone();
            }
        }
    }
    Ok(ex)
}

impl Explanation {
    /// A table like the worked example in section 10.
    pub fn render(&self) -> String {
        let mut s = format!("{} ({:?})\n", self.kind, self.direction);
        for t in &self.tiers {
            let cands: Vec<String> = t
                .candidates
                .iter()
                .map(|c| {
                    format!(
                        "\"{}\" → {}{}",
                        c.rule,
                        c.value,
                        if c.floor { " (floor)" } else { "" }
                    )
                })
                .collect();
            s.push_str(&format!(
                "  {:<12} {:<50} {}\n",
                t.tier.name(),
                cands.join("; "),
                t.value
            ));
        }
        match &self.value {
            Some(v) => s.push_str(&format!(
                "  effective    {v}, decided by {}\n",
                self.decided_by.join(" and ")
            )),
            None => s.push_str("  effective    open\n"),
        }
        for n in &self.no_effect {
            s.push_str(&format!("  no effect    \"{n}\"\n"));
        }
        for w in &self.warnings {
            s.push_str(&format!("  warning      {w}\n"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(n: i128) -> Bound {
        Bound::Point(Q(Num::int(n), Dim::NONE))
    }
    fn win(a: i128, b: i128) -> Bound {
        Bound::Window(Q(Num::int(a), Dim::NONE), Q(Num::int(b), Dim::NONE))
    }
    fn cand(rule: &str, pack: &str, tier: Tier, priority: i64, value: Bound) -> Candidate {
        Candidate {
            rule: rule.into(),
            pack: pack.into(),
            tier,
            priority,
            floor: false,
            value,
        }
    }

    #[test]
    fn worked_example_town_tightens_contacts_max() {
        let ex = resolve(
            "contacts_max",
            Direction::Max,
            Dim::NONE,
            Some(int(150)),
            vec![
                cand(
                    "Fleet contact limit",
                    "agentplaza.system",
                    Tier::System,
                    0,
                    int(150),
                ),
                cand(
                    "Research projects keep fewer contacts",
                    "project.research",
                    Tier::Plaza,
                    10,
                    int(60),
                ),
            ],
        )
        .unwrap();
        assert_eq!(ex.value, Some(int(60)));
        assert_eq!(
            ex.decided_by,
            vec!["Research projects keep fewer contacts".to_string()]
        );
        assert_eq!(ex.tiers.len(), 2);
        assert_eq!(
            (ex.tiers[0].tier, ex.tiers[0].value),
            (Tier::System, int(150))
        );
        assert_eq!((ex.tiers[1].tier, ex.tiers[1].value), (Tier::Plaza, int(60)));
        assert!(ex
            .render()
            .contains("decided by Research projects keep fewer contacts"));
    }

    #[test]
    fn town_cannot_loosen_and_is_reported_as_no_effect() {
        let ex = resolve(
            "contacts_max",
            Direction::Max,
            Dim::NONE,
            Some(int(150)),
            vec![
                cand("Fleet contact limit", "sys", Tier::System, 0, int(150)),
                cand("Big project", "project", Tier::Plaza, 0, int(500)),
            ],
        )
        .unwrap();
        assert_eq!(ex.value, Some(int(150)));
        assert_eq!(ex.decided_by, vec!["Fleet contact limit".to_string()]);
        assert_eq!(ex.no_effect, vec!["Big project".to_string()]);
    }

    #[test]
    fn resolve_applies_defaults_priority_ties_and_meets() {
        // default_only_without_rules_and_open_without_default
        {
            let ex = resolve("k", Direction::Max, Dim::NONE, Some(int(3)), vec![]).unwrap();
            assert_eq!(
                (ex.value, ex.source.clone()),
                (Some(int(3)), Source::Default)
            );
            let ex = resolve("k", Direction::Max, Dim::NONE, None, vec![]).unwrap();
            assert_eq!((ex.value, ex.source), (None, Source::Open));
        }
        // priority_within_a_tier_and_floors_meet
        {
            let mut f = cand("floor", "p", Tier::System, -5, int(40));
            f.floor = true;
            let ex = resolve(
                "k",
                Direction::Max,
                Dim::NONE,
                None,
                vec![
                    cand("low", "p", Tier::System, 0, int(10)),
                    cand("high", "p", Tier::System, 9, int(80)),
                    f,
                ],
            )
            .unwrap();
            // high (80) wins on priority, the floor (40) meets it: min(80, 40) = 40.
            assert_eq!(ex.value, Some(int(40)));
            assert!(ex.no_effect.contains(&"low".to_string()));
        }
        // ties_in_one_pack_are_errors_across_packs_warnings
        {
            let e = resolve(
                "k",
                Direction::Max,
                Dim::NONE,
                None,
                vec![
                    cand("a", "p", Tier::System, 0, int(1)),
                    cand("b", "p", Tier::System, 0, int(2)),
                ],
            );
            assert!(matches!(e, Err(ResolveError::SamePackTie { .. })));
            let ex = resolve(
                "k",
                Direction::Max,
                Dim::NONE,
                None,
                vec![
                    cand("a", "p", Tier::System, 0, int(1)),
                    cand("b", "q", Tier::System, 0, int(2)),
                ],
            )
            .unwrap();
            assert_eq!(ex.value, Some(int(1)));
            assert_eq!(ex.warnings.len(), 1);
        }
        // exact_windows_intersect_and_empty_meet_names_both_rules
        {
            let ex = resolve(
                "half_life",
                Direction::Exact,
                Dim::NONE,
                None,
                vec![
                    cand("wide", "s", Tier::System, 0, win(10, 20)),
                    cand("narrow", "t", Tier::Plaza, 0, win(12, 30)),
                ],
            )
            .unwrap();
            assert_eq!(ex.value, Some(win(12, 20)));
            let e = resolve(
                "half_life",
                Direction::Exact,
                Dim::NONE,
                None,
                vec![
                    cand("low", "s", Tier::System, 0, win(1, 2)),
                    cand("high", "t", Tier::Plaza, 0, win(5, 6)),
                ],
            )
            .unwrap_err();
            match e {
                ResolveError::EmptyMeet { a, b, .. } => {
                    assert_eq!((a.as_str(), b.as_str()), ("high", "low"));
                }
                other => panic!("{other:?}"),
            }
        }
        // min_direction_takes_the_largest
        {
            let ex = resolve(
                "snapshot_coalesce",
                Direction::Min,
                Dim::NONE,
                Some(int(5)),
                vec![
                    cand("sys", "s", Tier::System, 0, int(5)),
                    cand("project", "t", Tier::Plaza, 0, int(9)),
                ],
            )
            .unwrap();
            assert_eq!(ex.value, Some(int(9)));
        }
        // prefer_takes_the_highest_tier
        {
            let ex = resolve(
                "style",
                Direction::Prefer,
                Dim::NONE,
                None,
                vec![
                    cand("agent", "a", Tier::Agent, 99, int(1)),
                    cand("sys", "s", Tier::System, 0, int(2)),
                ],
            )
            .unwrap();
            assert_eq!(ex.value, Some(int(2)));
            assert_eq!(ex.decided_by, vec!["sys".to_string()]);
        }
        // wrong_dimension_is_an_error
        {
            let e = resolve(
                "k",
                Direction::Max,
                Dim::CREDITS,
                None,
                vec![cand("a", "p", Tier::System, 0, int(1))],
            );
            assert!(matches!(e, Err(ResolveError::Dimension { .. })));
        }
    }

    /// Small deterministic generator (std only).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn random_bound(g: &mut Lcg, dir: Direction) -> Bound {
        if dir == Direction::Exact {
            let a = g.below(50) as i128;
            win(a, a + g.below(60) as i128)
        } else {
            int(g.below(200) as i128)
        }
    }

    #[test]
    fn property_a_lower_tier_rule_never_loosens() {
        let mut g = Lcg(0x005e_ed74);
        let dirs = [Direction::Min, Direction::Max, Direction::Exact];
        let mut checked = 0;
        for round in 0..3000 {
            let dir = dirs[round % 3];
            let n = 1 + g.below(5) as usize;
            let mut rules = Vec::new();
            for i in 0..n {
                let tier = Tier::ALL[g.below(6) as usize];
                // distinct packs per rule, so ties are meets rather than errors
                let mut c = cand(
                    &format!("r{i}"),
                    &format!("p{i}"),
                    tier,
                    g.below(3) as i64,
                    random_bound(&mut g, dir),
                );
                c.floor = g.below(5) == 0;
                rules.push(c);
            }
            let Ok(before) = resolve("k", dir, Dim::NONE, Some(int(100)), rules.clone()) else {
                continue;
            };
            let lowest = rules.iter().map(|r| r.tier).max().unwrap();
            // (1) A rule at a tier strictly below every existing rule never loosens the result.
            if lowest != Tier::Agent {
                let tier = Tier::ALL
                    [lowest as usize + 1 + g.below((Tier::Agent as u64) - lowest as u64) as usize];
                let mut more = rules.clone();
                more.push(cand(
                    "new",
                    "pnew",
                    tier,
                    g.below(3) as i64,
                    random_bound(&mut g, dir),
                ));
                if let Ok(after) = resolve("k", dir, Dim::NONE, Some(int(100)), more) {
                    assert!(
                        at_least_as_tight(dir, after.value.unwrap(), before.value.unwrap()),
                        "round {round}: {:?} loosened {} to {}",
                        dir,
                        before.value.unwrap(),
                        after.value.unwrap()
                    );
                    checked += 1;
                }
            }
            // (2) The result is always at least as tight as every tier's own result.
            for t in &before.tiers {
                assert!(
                    at_least_as_tight(dir, before.value.unwrap(), t.value),
                    "round {round}"
                );
            }
        }
        assert!(checked > 1000, "only {checked} cases checked");
    }
}
