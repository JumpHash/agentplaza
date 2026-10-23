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

pub mod compose;
pub mod k3s;
pub mod manifests;
pub mod plan;
pub mod report;
pub mod resolver;
pub mod run;
pub mod spec;
