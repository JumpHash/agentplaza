//! apz-link: one per node. `join` turns a join token into this node's identity (pinned hub,
//! node key, CID range); `run` long-polls the hub's command queue for this node, runs each
//! command against the local noded / vsockd, heartbeats, and forwards the agents' gateway
//! traffic from the local vsockd to the hub, signed. See `docs/plan-hub-nodes-desktop.md`.

pub mod forward;
pub mod identity;
pub mod join;
pub mod run;
