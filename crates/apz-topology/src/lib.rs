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
//! - [`fault`], [`resilience`], [`netem`], [`steps`] and [`scenarios`]: the fault proxy in front of a dependency, the
//!   resilience scenarios that set its faults and kill or skew the guest's services (PLAN 22.8), the traffic shaping and
//!   the guest's actions where the node can, and the run of a topology's scenarios in the lab VM.
//! - [`load`], [`bench`] and [`compare`]: traffic profiles and their generator, benchmark output parsed, and the
//!   comparison of a candidate's metrics with a baseline's (PLAN 22.8); [`slowserve`] is the stand-in service the load
//!   tests aim at.

pub mod assert;
pub mod bench;
pub mod collector;
pub mod compare;
pub mod compose;
pub mod fault;
pub mod k3s;
pub mod load;
pub mod manifests;
pub mod netem;
pub mod observe;
pub mod pii;
pub mod plan;
pub mod report;
pub mod resilience;
pub mod resolver;
pub mod run;
pub mod scenarios;
pub mod security;
pub mod slowserve;
pub mod spec;
pub mod steps;
