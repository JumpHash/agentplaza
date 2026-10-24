//! The fork-side agent of a Linux desktop user test (PLAN 19.3).
//!
//! A lane's test fork runs this agent, and the hub's desktop target drives the desktop through it. The agent answers
//! the station tools the target calls (`station_lease`, `station_ui_tree`, `station_click`, ...) on a [`Desktop`], so the
//! target is the same whether its desktop is a station's or this fork's. On Linux the binary serves an X session (Xvfb
//! started by the X11 backend, XTest input, XShm capture) with the AT-SPI tree of the fork's session bus.
//!
//! The wire: one TCP connection at a time on the fork's port; a request per line, `{"tool": .., "input": ..}`, and a reply
//! per line, `{"ok": true, "value": ..}` or `{"ok": false, "error": ..}`. `hello` answers the desktop's OS.

pub mod agent;
pub mod script;
pub mod serve;

pub use agent::{Agent, OS, SharedDesk, share};
pub use serve::serve;
