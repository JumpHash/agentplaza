// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error.
#![forbid(unsafe_code)]

//! AgentPlaza's desktop register: an iced + wgpu (WebGPU) dashboard for the
//! node daemon's fleet API.
//!
//! The crate is split so the interesting half is testable without a display:
//!
//! * [`api`] — the daemon's JSON API over a tiny blocking HTTP/1.1 client,
//!   loopback only, no client dependency.
//! * [`chart`] — the charts' arithmetic: the ceiling, the window, the plate.
//! * [`events`] — the daemon's event stream: the server-sent frames, the
//!   parser, and the connection that follows the stream.
//! * [`exit`] — how a VM left the running state, read off its serial tail.
//! * [`export`] — where a transcript goes, the bytes it holds, and the one
//!   `std::fs` write that puts it there.
//! * [`form`] — the spawn form, its quick-boot presets, and the validation
//!   that mirrors the daemon's.
//! * [`history`] — the session's rolling telemetry history and its CSV.
//! * [`map`] — the fleet map's geometry and its serial-activity feed: pure,
//!   no iced types.
//! * [`mapview`] — the map as an iced canvas, painting what [`map`] computes.
//! * [`model`] — the panes, the reducer and the poll arithmetic: pure state,
//!   no iced types. The roster — who is running right now, from the live
//!   register — is a derivation here, alongside the age and instant arithmetic
//!   a long-running fleet run needs.
//! * [`serial`] — the offset-idempotent merge of the stream's serial deltas,
//!   and the per-VM capture bookkeeping it needs.
//! * [`telemetryview`] — the session's figures as iced canvases, painting what
//!   [`chart`] and [`history`] compute.
//! * [`theme`] — the register's palette, stamps and rulings.
//! * [`view`] — iced widgets built from the model.
//! * [`cli`] — `--url` / `--refresh-ms` / `APZ_DESKTOP_URL` and the call
//!   timeout.

pub mod api;
pub mod app;
pub mod chart;
pub mod cli;
pub mod events;
pub mod exit;
pub mod export;
pub mod form;
pub mod history;
pub mod map;
pub mod mapview;
pub mod model;
pub mod serial;
pub mod telemetryview;
pub mod theme;
pub mod view;

/// The poll timer's default granularity: the model decides whether a tick
/// becomes a call, so this is the finest granularity the schedule has unless
/// `--refresh-ms` says otherwise.
pub const TICK: std::time::Duration =
    std::time::Duration::from_millis(model::DEFAULT_REFRESH_MS);

/// The current wall-clock time in milliseconds since the Unix epoch.
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
