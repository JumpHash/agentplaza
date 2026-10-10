//! AgentPlaza's desktop register: an iced + wgpu (WebGPU) dashboard for the
//! node daemon's fleet API.
//!
//! The crate is split so the interesting half is testable without a display:
//!
//! * [`api`] — the daemon's JSON API over a tiny blocking HTTP/1.1 client,
//!   loopback only, no client dependency.
//! * [`form`] — the spawn form and the validation that mirrors the daemon's.
//! * [`model`] — the panes, the reducer and the poll arithmetic: pure state,
//!   no iced types.
//! * [`theme`] — the register's palette, stamps and rulings.
//! * [`view`] — iced widgets built from the model.
//! * [`cli`] — `--url` / `APZ_DESKTOP_URL` and the call timeout.

pub mod api;
pub mod app;
pub mod cli;
pub mod form;
pub mod model;
pub mod theme;
pub mod view;

/// How often the poll timer fires; the model decides whether a tick becomes a
/// call, so this is the finest granularity the schedule has.
pub const TICK: std::time::Duration =
    std::time::Duration::from_millis(model::POLL_INTERVAL_MS);

/// The current wall-clock time in milliseconds since the Unix epoch.
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
