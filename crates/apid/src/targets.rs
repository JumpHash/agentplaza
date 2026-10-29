//! Where a project's build targets are built (PLAN 20.5): each target goes to a node of a pool that can build it.
//!
//! A target names a platform (`platform` in `.plaza/project.toml`, 3.1). What a pool can build follows from what it
//! is: its architecture and its hypervisor (the names `project_stack` gives them) say its OS too, since a KVM pool is
//! Linux, an HVF pool is macOS and a WHP pool is Windows. macOS and iOS need a Mac (Xcode and signing need macOS);
//! Android needs a Linux pool whose node holds the project's base, which is where the SDK is; web builds anywhere. A
//! target no pool in service can build is `NotRun`, with the pool it needs.
//!
//! [`place`] is pure over the candidates placement gathers (`placement::Candidate`: a node, its pool, its free memory and
//! whether it holds the project's base), so the fleet's pools and the targets can be tested with fake nodes.
//!
//! Route: `GET /v1/projects/{id}/build-plan`, the plan a release's build step (`builtin/release.toml`) would place.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::nodes::Pool;
use crate::placement::{self, Candidate};
use crate::project_stack::{self, Arch, Hypervisor};
use crate::state::App;

/// A platform a project builds a target for, by the name its declaration uses (`platform = "linux-x86_64"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Platform {
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "linux-arm64")]
    LinuxArm64,
    #[serde(rename = "windows-x86_64")]
    Windows,
    #[serde(rename = "macos-arm64")]
    Macos,
    #[serde(rename = "ios-arm64")]
    Ios,
    #[serde(rename = "android-arm64")]
    Android,
    #[serde(rename = "web")]
    Web,
}

impl Platform {
    /// The platform's name in a project declaration.
    pub fn name(self) -> &'static str {
        match self {
            Platform::LinuxX86_64 => "linux-x86_64",
            Platform::LinuxArm64 => "linux-arm64",
            Platform::Windows => "windows-x86_64",
            Platform::Macos => "macos-arm64",
            Platform::Ios => "ios-arm64",
            Platform::Android => "android-arm64",
            Platform::Web => "web",
        }
    }

    /// Whether a candidate in `pool` (with `base`: it holds the project's base) can build this platform (pure).
    fn buildable_on(self, pool: Option<&Pool>, base: bool) -> bool {
        let is = |arch: Arch, hypervisor: Hypervisor| {
            pool.is_some_and(|p| p.arch == arch.name() && p.hypervisor == hypervisor.name())
        };
        match self {
            Platform::LinuxX86_64 => is(Arch::X86_64, Hypervisor::Kvm),
            Platform::LinuxArm64 => is(Arch::Aarch64, Hypervisor::Kvm),
            Platform::Windows => is(Arch::X86_64, Hypervisor::Whp),
            Platform::Macos | Platform::Ios => is(Arch::Aarch64, Hypervisor::Hvf),
            Platform::Android => {
                pool.is_some_and(|p| p.hypervisor == Hypervisor::Kvm.name()) && base
            }
            Platform::Web => true,
        }
    }

    /// The pool a target needs, as the reason it is not run says it.
    fn needs(self) -> &'static str {
        match self {
            Platform::LinuxX86_64 => "a Linux x86_64 pool (KVM)",
            Platform::LinuxArm64 => "a Linux arm64 pool (KVM)",
            Platform::Windows => "a Windows x86_64 pool (WHP)",
            Platform::Macos => "a Mac pool (Apple Silicon, HVF)",
            Platform::Ios => "a Mac pool (Apple Silicon, HVF): Xcode and signing need macOS",
            Platform::Android => {
                "a Linux KVM pool whose node holds the project's base with the Android SDK"
            }
            Platform::Web => "any node in service",
        }
    }
}

/// Where one target is built: on `node` (in `pool`, when it has one), or not run, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Built {
        platform: Platform,
        node: String,
        pool: Option<Pool>,
    },
    NotRun {
        platform: Platform,
        reason: String,
    },
}

/// Places each platform on the candidate that can build it with the most memory free; a tie goes to the name that sorts
/// first (pure). A platform no candidate can build is `NotRun`, with the pool it needs.
pub fn place(platforms: &[Platform], candidates: &[Candidate]) -> Vec<Placement> {
    platforms
        .iter()
        .map(|&platform| {
            let best = candidates
                .iter()
                .filter(|c| platform.buildable_on(c.pool.as_ref(), c.base))
                .max_by(|a, b| {
                    a.free_mib
                        .cmp(&b.free_mib)
                        .then_with(|| b.node.cmp(&a.node))
                });
            match best {
                Some(c) => Placement::Built {
                    platform,
                    node: c.node.clone(),
                    pool: c.pool.clone(),
                },
                None => Placement::NotRun {
                    platform,
                    reason: format!(
                        "no pool in service can build it: it needs {}",
                        platform.needs()
                    ),
                },
            }
        })
        .collect()
}

impl Placement {
    /// The platform the placement is for.
    pub fn platform(&self) -> Platform {
        match self {
            Placement::Built { platform, .. } | Placement::NotRun { platform, .. } => *platform,
        }
    }
}

/// A pool as the plan shows it: its architecture, hypervisor and CPU baseline.
fn pool_json(pool: &Pool) -> Value {
    json!({"arch": pool.arch, "hypervisor": pool.hypervisor, "cpu_baseline": pool.cpu_baseline})
}

/// One placement as the plan answers it.
fn placement_json(placement: &Placement) -> Value {
    match placement {
        Placement::Built {
            platform,
            node,
            pool,
        } => json!({
            "platform": platform.name(), "node": node,
            "pool": pool.as_ref().map(pool_json),
        }),
        Placement::NotRun { platform, reason } => {
            json!({"platform": platform.name(), "not_run": reason})
        }
    }
}

/// The build plan of `project`: its declared targets (`platform`, 3.1), each placed on the pool that builds it. Empty
/// when the project declares no build target. Errors when its declaration does not read.
pub fn build_plan(app: &App, project: &str) -> Result<Vec<Placement>, String> {
    let state = app
        .projects
        .get(project)
        .ok_or_else(|| format!("no project {project}"))?;
    // A project opened while Weft is stopped has no repo yet, so it cannot have
    // a stack declaration. Reading its empty plan does not require a repo write.
    if state.repo.is_none() {
        return Ok(Vec::new());
    }
    let platforms: Vec<Platform> = project_stack::read(app, project)?
        .map(|stack| stack.targets.iter().filter_map(|t| t.platform).collect())
        .unwrap_or_default();
    Ok(place(
        &platforms,
        &placement::build_candidates(app, project),
    ))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/build-plan", plan_route)
        .doc(
            RouteDoc::new(
                "A project's build plan: each declared build target on the pool that builds it, or not run with its reason",
            )
            .errors(&[404, 502]),
        )
}

/// `GET /v1/projects/{id}/build-plan`: the plan of the project's build targets.
fn plan_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    match build_plan(app, project) {
        Ok(plan) => Response::json(
            200,
            &json!({"targets": plan.iter().map(placement_json).collect::<Vec<Value>>()}),
        ),
        Err(why) => Response::error(502, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform, in the order the plan lists them.
    const ALL: [Platform; 7] = [
        Platform::LinuxX86_64,
        Platform::LinuxArm64,
        Platform::Windows,
        Platform::Macos,
        Platform::Ios,
        Platform::Android,
        Platform::Web,
    ];

    use crate::recipes::Report;
    use crate::recipes::runs::Runs;
    use std::collections::BTreeMap;

    fn pool(arch: &str, hypervisor: &str) -> Pool {
        Pool {
            arch: arch.into(),
            hypervisor: hypervisor.into(),
            cpu_baseline: format!("{arch} {hypervisor} baseline"),
        }
    }

    /// A fake candidate: a node in `pool` with `free_mib` free, and whether it holds the project's base.
    fn node(name: &str, pool: Option<Pool>, free_mib: u64, base: bool) -> Candidate {
        Candidate {
            node: name.into(),
            free_mib,
            running_vms: 0,
            warm: false,
            pool,
            base,
            disk_free_gib: None,
            idle_cpus: None,
        }
    }

    /// A fleet with one pool of each kind: x86_64 and arm64 Linux (KVM, the x86 one holding the base), a Windows
    /// machine (WHP) and a Mac (HVF).
    fn fleet() -> Vec<Candidate> {
        vec![
            node("lin-x86", Some(pool("x86_64", "kvm")), 8000, true),
            node("lin-arm", Some(pool("aarch64", "kvm")), 6000, false),
            node("win", Some(pool("x86_64", "whp")), 4000, false),
            node("mac", Some(pool("aarch64", "hvf")), 9000, false),
        ]
    }

    /// Each built platform as its node, and each not run one's reason.
    fn outcome(placements: &[Placement]) -> Vec<(&'static str, String)> {
        placements
            .iter()
            .map(|p| match p {
                Placement::Built { platform, node, .. } => (platform.name(), node.clone()),
                Placement::NotRun { platform, reason } => {
                    (platform.name(), format!("not run: {reason}"))
                }
            })
            .collect()
    }

    #[test]
    fn every_platform_is_built_on_a_pool_that_can_build_it() {
        let placements = place(&ALL, &fleet());
        assert_eq!(
            outcome(&placements),
            [
                ("linux-x86_64", "lin-x86".to_string()),
                ("linux-arm64", "lin-arm".to_string()),
                ("windows-x86_64", "win".to_string()),
                ("macos-arm64", "mac".to_string()),
                ("ios-arm64", "mac".to_string()),
                // Android needs the base's SDK: only the x86_64 Linux node holds the base.
                ("android-arm64", "lin-x86".to_string()),
                // Web builds anywhere: the Mac has the most room.
                ("web", "mac".to_string()),
            ]
        );
        for placement in &placements {
            if let Placement::Built { pool, platform, .. } = placement
                && *platform != Platform::Web
            {
                assert!(pool.is_some(), "{} records its pool", platform.name());
            }
        }
    }

    #[test]
    fn a_platform_with_no_pool_is_not_run_with_its_reason_and_the_rest_still_build() {
        let without_windows: Vec<Candidate> =
            fleet().into_iter().filter(|c| c.node != "win").collect();
        let placements = place(
            &[Platform::LinuxX86_64, Platform::Windows, Platform::Web],
            &without_windows,
        );
        let outcome = outcome(&placements);
        assert_eq!(outcome[0], ("linux-x86_64", "lin-x86".to_string()));
        assert_eq!(outcome[2], ("web", "mac".to_string()));
        assert!(
            outcome[1].1.starts_with("not run:")
                && outcome[1].1.contains("a Windows x86_64 pool (WHP)"),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_mac_target_needs_a_mac_and_no_linux_pool_takes_it() {
        let linux_only = vec![node("lin-x86", Some(pool("x86_64", "kvm")), 8000, true)];
        for platform in [Platform::Macos, Platform::Ios] {
            match &place(&[platform], &linux_only)[0] {
                Placement::NotRun { reason, .. } => {
                    assert!(reason.contains("a Mac pool"), "{reason}")
                }
                other => panic!("expected not run, got {other:?}"),
            }
        }
    }

    #[test]
    fn android_needs_the_project_base_on_a_linux_kvm_node() {
        // The arm64 Linux node has room but no base; the x86_64 one has the base; the Mac is not Linux.
        let no_base = vec![
            node("lin-arm", Some(pool("aarch64", "kvm")), 9000, false),
            node("mac", Some(pool("aarch64", "hvf")), 9000, true),
        ];
        match &place(&[Platform::Android], &no_base)[0] {
            Placement::NotRun { reason, .. } => assert!(reason.contains("Android SDK"), "{reason}"),
            other => panic!("expected not run, got {other:?}"),
        }
    }

    #[test]
    fn a_node_with_no_pool_builds_only_web() {
        let unpooled = vec![node("old", None, 8000, true)];
        let placements = place(&ALL, &unpooled);
        for placement in placements {
            match placement {
                Placement::Built { platform, .. } => assert_eq!(platform, Platform::Web),
                Placement::NotRun { .. } => {}
            }
        }
    }

    #[test]
    fn a_tie_on_memory_goes_to_the_name_that_sorts_first() {
        let tied = vec![
            node("n2", Some(pool("x86_64", "kvm")), 4096, false),
            node("n1", Some(pool("x86_64", "kvm")), 4096, false),
        ];
        assert_eq!(
            outcome(&place(&[Platform::LinuxX86_64], &tied)),
            [("linux-x86_64", "n1".to_string())]
        );
    }

    #[test]
    fn platform_names_are_the_declared_ones() {
        for platform in ALL {
            let parsed: Platform = serde_json::from_value(json!(platform.name())).unwrap();
            assert_eq!(parsed, platform);
        }
        assert!(serde_json::from_value::<Platform>(json!("solaris-sparc")).is_err());
    }

    /// The release recipe's build step runs one item per placed target (fan_out per_target), and the run passes when
    /// every item passes. A target not run is not an item, so the missing pool does not block the release.
    #[test]
    fn a_release_runs_one_build_item_per_placed_target() {
        let runs = Runs::default();
        let placements = place(&ALL, &fleet_without_windows());
        let built = placements
            .iter()
            .filter(|p| matches!(p, Placement::Built { .. }))
            .count();
        assert_eq!(built, 6, "the Windows target is not run");
        let items = BTreeMap::from([("build".to_string(), built as u32)]);
        let run = runs
            .start_engine_items("release", &items, "release", None, Some("p1"), None, 1)
            .expect("the release recipe is a built-in");
        runs.settle(&run, "draft", &Report::verdict(true), 1);
        runs.started(&run, "build", 1);
        for item in 0..built {
            let moves = runs.settle(&run, "build", &Report::verdict(true), 2 + item as u64);
            if item + 1 < built {
                assert!(moves.is_empty(), "item {} is not the last", item + 1);
            }
        }
        // The hub cuts the candidate once every built target passed.
        runs.started(&run, "candidate", 9);
        runs.settle(&run, "candidate", &Report::verdict(true), 9);
        assert_eq!(
            runs.view(&run).unwrap().run.state,
            crate::recipes::runs::RunState::Passed
        );
    }

    fn fleet_without_windows() -> Vec<Candidate> {
        fleet().into_iter().filter(|c| c.node != "win").collect()
    }
}
