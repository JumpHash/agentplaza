//! Test topologies (PLAN 22.6): a topology file, the guest plan that one lab VM runs for it, the name service its
//! services resolve through, and the process runner that starts the plan where no container runtime is.
//!
//! - [`spec`]: `.plaza/topologies/<name>.toml`, typed and checked.
//! - [`plan`]: the containers of one topology, their addresses, programs and environment, and the `ip` commands that
//!   build the VM's bridge (pure).
//! - [`resolver`]: a service name's addresses: the DNS answers the runner serves, and the client that asks.
//! - [`run`]: starts a plan's containers as processes, with their names resolving through the runner's resolver.

pub mod plan;
pub mod resolver;
pub mod run;
pub mod spec;
