//! Workstations: machines agents operate through the hub (docs/design/workstations.md). This is the hub's core:
//! the enrollment (join codes, the registry, drain, revoke), the link socket each station's daemon holds open,
//! leases with the pixel-session queue, the station audit, and STOP ALL's freeze.
//!
//! - `registry`: the stations and the join codes (the `stations` component of plaza_state), the link table.
//! - `link`: one station's live link: requests and their answers by id, event fan-out, the loop that serves it.
//! - `leases`: an agent's claim on a station, the pixel capacity of a machine, and the queue.
//! - `ops`: the actions with the rest of the hub: the audit, the hello and events, grants, STOP ALL, the ticker.
//! - `recordings`: a lease's recording (its actions, its frames in Garage), the sampler and the retention sweep.
//! - `api`: the routes, listed in its module docs.
//! - `screen`: the live view (`WS .../screen`) and the thumbnails; the viewers of a session share one stream.
//! - `control`: the owner's takeover socket (`WS .../control`): take, input, give back.
//! - `takeover`: the takeover policies (agent waits, release the lease, keep working with the pointer paused).
//! - `act`: the choke point every workstation action passes (the checks, the laws, the audit), and `hooks` after it.
//! - `tools`: the workstation tools agents and the owner's MCP client are offered, parsed into operations for `act`.
//!
//! The containment rule holds: a station is never the hub's own machine (refused at join and at every hello), and
//! agents reach a station only through the hub.

mod act;
mod api;
mod control;
mod hooks;
mod leases;
mod link;
mod ops;
pub mod recordings;
mod registry;
mod screen;
mod takeover;
pub mod tools;

pub use api::routes;
pub use ops::{freeze_all, spawn};
pub use registry::Store;
// The agents' pixel path (stations/act.rs) asks these before a screenshot or an input.
#[allow(unused_imports)]
pub use takeover::{pixel_paused, pointer_paused};
