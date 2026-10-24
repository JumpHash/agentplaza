//! Test topologies (PLAN 22.6): a topology file, the guest plan that one lab VM runs for it, the name service its
//! services resolve through, and the process runner that starts the plan where no container runtime is.
//!
//! - [`spec`]: `.plaza/topologies/<name>.toml`, typed and checked.
//! - [`plan`]: the containers of one topology, their addresses, programs and environment, and the `ip` commands that
//!   build the VM's bridge (pure).
//! - [`resolver`]: a service name's addresses: the DNS answers the runner serves, and the client that asks.
//! - [`run`]: starts a plan's containers as processes, with their names resolving through the runner's resolver.
//! - [`compose`] and [`manifests`]: a Compose file and rendered Kubernetes manifests converted into a topology, with a
//!   [`report`] of what did not carry over.
//! - [`k3s`]: the guest config of a k3s topology (a Kubernetes cluster in the lab VM).
//! - [`observe`] and [`assert`]: what a run observed (OTLP, log lines) and the topology's test steps over it (PLAN 22.7);
//!   [`collector`] is the lab VM's collector that ships it, and [`pii`] the personal data patterns the steps scan for.
//! - [`security`]: the security stages (PLAN 22.8): the dependency audits, the image scan, the secrets scan of a release's
//!   tree, a built-in DAST scan of the topology's routes, and the fuzzers the topology declares.
//! - [`node_loss`]: a multi-VM topology's `stop_node` scenarios, which take a machine's node out of service through the hub's node
//!   stop (PLAN #59) and judge the degradation the spec declares.
//! - [`fault`], [`resilience`], [`netem`], [`steps`] and [`scenarios`]: the fault proxy in front of a dependency, the
//!   resilience scenarios that set its faults and kill or skew the guest's services (PLAN 22.8), the traffic shaping and
//!   the guest's actions where the node can, and the run of a topology's scenarios in the lab VM.
//! - [`scrub`] and [`source`]: the scrubber (PLAN 22.5): rules per table, column, document field and log pattern, a
//!   read-only SQLite or Postgres source, and the copy of production data the lab receives after its personal data scan.
//! - [`data`], [`cache`] and [`messaging`], with [`stage`] for what they share: the data stage's migrations, backup and
//!   restore, concurrent writers and invariants; the cache's hit rate, invalidation and stampede; and the messaging
//!   stage's delivery, ordering, retries, dead letters and duplicates (PLAN 22.8). Each runs where the guest is and
//!   records numbers, which the hub judges.
//! - [`compat`]: the compatibility and install-and-upgrade matrices of a release (PLAN 22.9): the declared cells, their
//!   placement on pools by capability, and the shell and API runs that judge the CLI and library cells.
//! - [`load`], [`bench`] and [`compare`]: traffic profiles and their generator, benchmark output parsed, and the
//!   comparison of a candidate's metrics with a baseline's (PLAN 22.8); [`slowserve`] is the stand-in service the load
//!   tests aim at.

pub mod assert;
pub mod bench;
pub mod cache;
pub mod collector;
pub mod compare;
pub mod compat;
pub mod compose;
pub mod data;
pub mod fault;
pub mod k3s;
pub mod load;
pub mod manifests;
pub mod messaging;
pub mod netem;
pub mod node_loss;
pub mod observe;
pub mod pii;
pub mod plan;
pub mod report;
pub mod resilience;
pub mod resolver;
pub mod run;
pub mod scenarios;
pub mod scrub;
pub mod security;
pub mod segment;
pub mod slowserve;
pub mod source;
pub mod spec;
pub mod stage;
pub mod steps;

#[cfg(test)]
mod lab_tools_tests {
    //! The lab image's tool list (`scripts/guest/lab-tools.txt`): every `bin` line names a binary of this crate, and the
    //! tools the lab needs are all listed.

    const LIST: &str = include_str!("../../../scripts/guest/lab-tools.txt");
    const MANIFEST: &str = include_str!("../Cargo.toml");

    fn entries() -> Vec<(String, String)> {
        LIST.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| {
                let (kind, name) = l.split_once(' ')?;
                Some((kind.to_string(), name.trim().to_string()))
            })
            .collect()
    }

    #[test]
    fn every_listed_binary_is_a_binary_of_the_crate_and_the_lab_tools_are_listed() {
        let entries = entries();
        for (kind, name) in &entries {
            if kind == "bin" {
                assert!(
                    MANIFEST.contains(&format!("name = \"{name}\"")),
                    "{name} is listed for the lab image but is not a binary of apz-topology"
                );
            }
        }
        for tool in ["apz-topo-run", "apz-fault", "apz-stage", "apz-security"] {
            assert!(
                entries.contains(&("bin".to_string(), tool.to_string())),
                "{tool} is not in the lab image's list"
            );
        }
        assert!(
            entries.contains(&("data".to_string(), "scrub".to_string())),
            "the scrubber's copy is not listed"
        );
    }
}
