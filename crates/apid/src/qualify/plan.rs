//! The stages of a qualification (PLAN 22.10) and the depth a candidate is qualified to: the planner's choice, per channel,
//! with the project's override and the reasons the stages it leaves out are out (pure).
//!
//! `nightly` runs stages 1 to 5 and a short 6; `beta` adds 6 to 9; `stable` runs all of them, the soak (10) only when the
//! project asks for one. A risky change raises a nightly to resilience and security. Without a declared topology the lab
//! has nothing to run, so the stages that need it are out of the depth with that reason, and the verdict (11) is always in.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The eleven stages, in order (22.10). The stage's number is its place in the list, from 1.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    apz_store::Column,
)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// 1. Verify the artifacts: signatures, provenance, SBOMs.
    Verify,
    /// 2. Stand up the topologies (22.6): one lab run each.
    Topologies,
    /// 3. Functional, API and contract tests.
    Functional,
    /// 4. Data, caching and messaging.
    Data,
    /// 5. Observability assertions, throughout the run.
    Observe,
    /// 6. Performance and traffic, against the baselines.
    Performance,
    /// 7. Resilience.
    Resilience,
    /// 8. Security.
    Security,
    /// 9. UI and UX, the compatibility matrix, install and upgrade.
    Ux,
    /// 10. Soak (stable releases, where the project asks for it).
    Soak,
    /// 11. The verdict and the report.
    Verdict,
}

impl Stage {
    /// Every stage, in order.
    pub const ALL: [Stage; 11] = [
        Stage::Verify,
        Stage::Topologies,
        Stage::Functional,
        Stage::Data,
        Stage::Observe,
        Stage::Performance,
        Stage::Resilience,
        Stage::Security,
        Stage::Ux,
        Stage::Soak,
        Stage::Verdict,
    ];

    /// The stage's place, 1 to 11.
    pub fn number(self) -> u8 {
        self as u8 + 1
    }

    /// The stage's key: its step's id in the recipe (`qualify.toml`) and its name in the report.
    pub fn key(self) -> &'static str {
        match self {
            Stage::Verify => "verify",
            Stage::Topologies => "topologies",
            Stage::Functional => "functional",
            Stage::Data => "data",
            Stage::Observe => "observe",
            Stage::Performance => "performance",
            Stage::Resilience => "resilience",
            Stage::Security => "security",
            Stage::Ux => "ux",
            Stage::Soak => "soak",
            Stage::Verdict => "verdict",
        }
    }

    /// The stage a key names.
    pub fn of_key(key: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|stage| stage.key() == key)
    }

    /// The stage's title in the report.
    pub fn title(self) -> &'static str {
        match self {
            Stage::Verify => "Verify the artifacts",
            Stage::Topologies => "Stand up the topologies",
            Stage::Functional => "Functional, API and contract tests",
            Stage::Data => "Data, caching and messaging",
            Stage::Observe => "Observability assertions",
            Stage::Performance => "Performance and traffic",
            Stage::Resilience => "Resilience",
            Stage::Security => "Security",
            Stage::Ux => "UI and UX, compatibility, install and upgrade",
            Stage::Soak => "Soak",
            Stage::Verdict => "The verdict and the report",
        }
    }

    /// The lab verdict kinds a stage judges (the stage's result path: the checks its lab runs recorded). Empty for the
    /// stages that are not judged by a lab run's checks: verify, topologies (its runs' outcomes), functional (its tests),
    /// security (its own scans), and the verdict.
    pub fn kinds(self) -> &'static [&'static str] {
        match self {
            Stage::Data => &["data", "cache", "messaging"],
            Stage::Observe => &["observability"],
            Stage::Performance => &["performance"],
            Stage::Resilience => &["resilience"],
            Stage::Security => &["security"],
            Stage::Ux => &["visual", "accessibility", "layout"],
            Stage::Soak => &["soak"],
            Stage::Verify | Stage::Topologies | Stage::Functional | Stage::Verdict => &[],
        }
    }

    /// Whether the stage needs the topology's lab runs to have ended (the stages after 2 that the lab judges).
    pub fn needs_lab(self) -> bool {
        matches!(
            self,
            Stage::Functional
                | Stage::Data
                | Stage::Observe
                | Stage::Performance
                | Stage::Resilience
                | Stage::Ux
                | Stage::Soak
        )
    }
}

/// What the planner knows about the candidate and its project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requirements {
    /// The project declares at least one topology.
    pub has_topologies: bool,
    /// The release holds a change that is risky (a high-risk path or a large change).
    pub risky: bool,
    /// The project asks for a soak (its `qualify_soak` setting).
    pub soak_asked: bool,
}

/// A stage a depth leaves out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Excluded {
    pub stage: Stage,
    pub why: String,
}

/// The depth one candidate is qualified to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Depth {
    pub channel: String,
    /// The stages that run, in order; always verify and the verdict.
    pub stages: Vec<Stage>,
    /// The stages that run in a short form (6 on nightly: a smoke profile).
    pub short: Vec<Stage>,
    /// The stages that do not run, each with the reason.
    pub excluded: Vec<Excluded>,
    /// The planner's choice in a line: the channel's depth, the override, the raise, and the exclusions.
    pub reason: String,
}

/// The stages of a channel's default depth (1 to 6 nightly, 1 to 9 beta, 1 to 10 for the rest; 10 only when asked).
fn base(channel: &str) -> Vec<Stage> {
    let last = match channel {
        "nightly" => 6,
        "beta" => 9,
        _ => 10,
    };
    Stage::ALL[..last].to_vec()
}

/// Parses a project's depth override: stage keys or numbers, separated by commas (`verify, functional, 6`).
pub fn parse_stages(text: &str) -> Result<Vec<Stage>, String> {
    let mut out = BTreeSet::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let stage = match part.parse::<u8>() {
            Ok(n) => Stage::ALL
                .into_iter()
                .find(|s| s.number() == n)
                .ok_or_else(|| format!("{part} is not a stage: 1 to 11"))?,
            Err(_) => Stage::of_key(part).ok_or_else(|| format!("{part} is not a stage"))?,
        };
        if stage == Stage::Verdict {
            return Err("the verdict (11) is always in a qualification".into());
        }
        out.insert(stage);
    }
    if !out.contains(&Stage::Verify) {
        return Err("a depth includes verify (1): the artifacts are always verified".into());
    }
    Ok(out.into_iter().collect())
}

/// The depth of a candidate on `channel`, with the project's `override_` for the channel when it sets one. Pure: the
/// planner's rules are in the module's docs.
pub fn plan(channel: &str, override_: Option<&[Stage]>, req: &Requirements) -> Depth {
    let mut reasons: Vec<String> = Vec::new();
    let mut want: BTreeSet<Stage> = match override_ {
        Some(list) => {
            reasons.push(format!("{channel}: the project's depth"));
            list.iter().copied().collect()
        }
        None => {
            reasons.push(format!("{channel}: the channel's depth"));
            base(channel).into_iter().collect()
        }
    };
    if override_.is_none() && channel == "nightly" && req.risky {
        want.insert(Stage::Resilience);
        want.insert(Stage::Security);
        reasons.push("a risky change raises nightly to resilience and security".into());
    }
    let mut excluded = Vec::new();
    if want.contains(&Stage::Soak) && !req.soak_asked {
        want.remove(&Stage::Soak);
        excluded.push(Excluded {
            stage: Stage::Soak,
            why: "the project does not ask for a soak".into(),
        });
    }
    if !req.has_topologies {
        for stage in Stage::ALL {
            if (stage.needs_lab() || stage == Stage::Topologies) && want.remove(&stage) {
                excluded.push(Excluded {
                    stage,
                    why: "no topology is declared: the lab has nothing to run".into(),
                });
            }
        }
    }
    want.insert(Stage::Verify);
    want.insert(Stage::Verdict);
    for stage in Stage::ALL {
        if !want.contains(&stage) && !excluded.iter().any(|e| e.stage == stage) {
            excluded.push(Excluded {
                stage,
                why: format!("not in the {channel} depth"),
            });
        }
    }
    let short = if channel == "nightly" && want.contains(&Stage::Performance) {
        vec![Stage::Performance]
    } else {
        Vec::new()
    };
    excluded.sort_by_key(|e| e.stage);
    let stages: Vec<Stage> = want.into_iter().collect();
    Depth {
        channel: channel.to_string(),
        stages,
        short,
        excluded,
        reason: reasons.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Requirements {
        Requirements {
            has_topologies: true,
            risky: false,
            soak_asked: true,
        }
    }

    #[test]
    fn nightly_runs_one_to_five_and_a_short_six() {
        let depth = plan("nightly", None, &all());
        let numbers: Vec<u8> = depth.stages.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, vec![1, 2, 3, 4, 5, 6, 11]);
        assert_eq!(depth.short, vec![Stage::Performance]);
        assert!(
            depth
                .excluded
                .iter()
                .any(|e| e.stage == Stage::Ux && e.why.contains("not in the nightly depth"))
        );
    }

    #[test]
    fn beta_adds_six_to_nine_and_stable_runs_every_stage_the_project_asks_for() {
        let beta = plan("beta", None, &all());
        assert!(beta.stages.contains(&Stage::Ux) && !beta.stages.contains(&Stage::Soak));
        let stable = plan("stable", None, &all());
        assert_eq!(stable.stages.len(), 11, "{:?}", stable.stages);
        let unasked = plan(
            "stable",
            None,
            &Requirements {
                soak_asked: false,
                ..all()
            },
        );
        assert!(!unasked.stages.contains(&Stage::Soak));
        assert!(
            unasked
                .excluded
                .iter()
                .any(|e| e.stage == Stage::Soak && e.why.contains("does not ask"))
        );
    }

    #[test]
    fn a_project_depth_overrides_the_channel_and_the_planner_records_why() {
        let custom = parse_stages("verify, functional, 9").unwrap();
        let depth = plan("nightly", Some(&custom), &all());
        assert_eq!(
            depth.stages,
            vec![Stage::Verify, Stage::Functional, Stage::Ux, Stage::Verdict],
            "9 is UX"
        );
        assert!(depth.reason.contains("the project's depth"));
        assert!(parse_stages("functional").unwrap_err().contains("verify"));
        assert!(parse_stages("12").unwrap_err().contains("not a stage"));
    }

    #[test]
    fn a_risky_nightly_is_raised_and_a_project_without_topologies_runs_no_lab_stage() {
        let risky = plan(
            "nightly",
            None,
            &Requirements {
                risky: true,
                ..all()
            },
        );
        assert!(
            risky.stages.contains(&Stage::Resilience) && risky.stages.contains(&Stage::Security)
        );
        assert!(risky.reason.contains("risky"));
        let bare = plan(
            "stable",
            None,
            &Requirements {
                has_topologies: false,
                ..all()
            },
        );
        assert_eq!(
            bare.stages,
            vec![Stage::Verify, Stage::Security, Stage::Verdict]
        );
        assert!(
            bare.excluded
                .iter()
                .any(|e| e.stage == Stage::Topologies && e.why.contains("no topology"))
        );
    }

    #[test]
    fn every_stage_has_a_key_a_number_and_a_title() {
        for (i, stage) in Stage::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(stage.number()), i + 1);
            assert_eq!(Stage::of_key(stage.key()), Some(stage));
            assert!(!stage.title().is_empty());
        }
    }
}
