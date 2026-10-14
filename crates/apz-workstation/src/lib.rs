//! apz-workstation: the daemon that makes a machine an AgentPlaza workstation.
//!
//! `join` enrolls the machine with a hub (`POST /v1/stations/join`) and keeps what the hub gave back in the
//! state directory. `run` dials the hub's station link (`WS /v1/stations/link`, the protocol `apz_station`
//! defines) and serves the hub's requests: files, terminals and runs confined to the work folder (`apz_ops`),
//! screen sessions, screenshots, input, UI trees and streams (`apz_desktop`), and STOP ALL.
//!
//! Layout:
//! - [`daemon`]: the shared state, and the routing of each request to the lane that serves it.
//! - [`desk`]: the desktop lane, one thread that owns the [`apz_desktop::Desktop`].
//! - [`work`]: the worker pool for files, terminals, runs, settings and facts.
//! - [`sync`]: the work folder's two-way sync: a scanner that reports changes, and the hub's manifest, pull, push and
//!   delete requests, written atomically.
//! - [`script`]: the scripting bridges. The `cli` bridge is built; the app bridges are not yet.
//! - [`link`]: the hub connection: connect, hello, serve, reconnect with backoff.
//! - [`join`], [`state`], [`facts`], [`backend`], [`route`], [`access`], [`uplink`], [`log`], [`cli`]: enrolment,
//!   the state directory, the machine's facts, the desktop backend choice, the desktop the lanes serve (owner's
//!   screen and virtual displays), the Linux accessibility tree, the outgoing queue, the log file and the command
//!   line.

pub mod acl;
pub mod account;
pub mod backend;
pub mod cli;
pub mod daemon;
pub mod desk;
pub mod enrol;
pub mod facts;
#[cfg(target_os = "linux")]
pub mod access;
pub mod join;
pub mod link;
pub mod log;
pub mod route;
pub mod plan;
pub mod script;
pub mod service;
pub mod state;
pub mod sync;
pub mod uplink;
pub mod vsock;
pub mod work;
