//! Workstations: machines agents operate through the hub (docs/design/workstations.md). This is the hub's core:
//! the enrollment (join codes, the registry, drain, revoke), the link socket each station's daemon holds open,
//! leases with the pixel-session queue, the station audit, and STOP ALL's freeze.
//!
//! - `registry`: the stations and the join codes (the `stations` component of plaza_state), the link table.
//! - `link`: one station's live link: requests and their answers by id, event fan-out, the loop that serves it.
//! - `leases`: an agent's claim on a station, the pixel capacity of a machine, and the queue.
//! - `ops`: the actions with the rest of the hub: the audit, the hello and events, grants, STOP ALL, the ticker.
//! - `api`: the routes, listed in its module docs.
//!
//! The containment rule holds: a station is never the hub's own machine (refused at join and at every hello), and
//! agents reach a station only through the hub.

mod api;
mod leases;
mod link;
mod ops;
mod registry;

pub use api::routes;
pub use ops::{freeze_all, spawn};
pub use registry::Store;
