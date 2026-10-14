//! The hub<->workstation link protocol.
//!
//! A workstation daemon (`apz-workstation`) keeps one WebSocket to the hub, opened as:
//!
//! ```text
//! WS /v1/stations/link
//! Sec-WebSocket-Protocol: apz.station.v1
//! Authorization: Bearer <station token>
//! ```
//!
//! The link is pinned TLS, dialled out by the daemon, so the machine opens no ports. The station's
//! identity comes from its token, never from a frame: the hub binds the socket to the station the
//! token names, and `Hello.station` is only a label for logs.
//!
//! Every WebSocket text message is one JSON object, tagged by `frame`:
//!
//! ```text
//! daemon -> hub   {"frame":"hello","station":"ws-7","version":"0.0.8","facts":{...}}     first, once
//! hub -> daemon   {"frame":"request","id":41,"op":{"op":"facts"}}                        any time after hello
//! daemon -> hub   {"frame":"response","id":41,"ok":true,"result":{...}}                 one per request
//! daemon -> hub   {"frame":"event","event":{"event":"frame",...}}                       any time
//! hub -> daemon   {"frame":"notify","notice":"drain"}                                    any time
//! ```
//!
//! Rules both sides keep:
//!
//! - The daemon sends `hello` before anything else, and sends it again after every reconnect.
//! - `id` is chosen by the hub and is unique per link. The daemon answers every request exactly once,
//!   with the same `id`: `ok: true` and `result` (the shape is documented on each [`Op`] variant), or
//!   `ok: false` and `error` (a human-readable reason; the hub shows it and audits it).
//! - Requests may be served concurrently. The daemon runs a worker pool for files, terminals, runs, settings
//!   and facts, and one desktop lane for the screen operations (sessions, screenshots, input, UI trees, apps,
//!   streams), which serves them in arrival order because a desktop takes one caller at a time. So responses
//!   can arrive out of order: each answer is matched to its request by `id` alone, never by its position on the
//!   link. A request that cannot start (a closed session, a frozen station) is answered with `ok: false`, never
//!   left unanswered.
//! - Screenshot and stream coordinates are the pixels of the image last sent for the session (see
//!   [`InputAction`]); the daemon maps them to screen points.
//! - `Event` frames are unsolicited: screen frames for live views, facts changes, sync changes, the
//!   owner's input (a takeover), a session that ended, and log lines.
//! - `Notify` tells the daemon something the hub decided: `revoked` (end the link and stop),
//!   `drain` (take no new requests, finish what runs), `settings_changed` (the hub sends the new
//!   values with an `Op::Settings` request).
//! - `Op::Freeze { on: true }` is STOP ALL: the daemon stops input at once, ends pixel sessions, and
//!   stops shells and runs. It answers requests with `ok: false` until `Freeze { on: false }`.
//!
//! The examples in the tests of each module are the wire spec: every variant there round-trips to
//! exactly that JSON.

mod event;
mod facts;
mod input;
mod link;
mod op;
mod settings;
mod sync;

pub use event::{Event, Level};
pub use facts::{AccountMode, Display, Facts, Form, Gpu, Os};
pub use input::{Button, InputAction, Selector, UiAction};
pub use link::{HubFrame, Notice, StationFrame};
pub use op::{AppsOp, Bridge, FilesOp, ImageFormat, Keep, OWNER_VIEW, Op, RunOp, SessionKind, SyncOp, TermOp};
pub use settings::{StationSettings, Takeover};
pub use sync::{CHUNK as SYNC_CHUNK, FILE_CAP as SYNC_FILE_CAP, FolderState, PART_PREFIX as SYNC_PART_PREFIX, SyncEntry, SyncManifest, SyncPull, SyncPushed, SyncStatus, ignored_name, valid_path};

/// The WebSocket subprotocol the link is opened with.
pub const SUBPROTOCOL: &str = "apz.station.v1";

/// The hub path the link is opened on.
pub const LINK_PATH: &str = "/v1/stations/link";

/// A screen session the daemon opened for one agent (see [`Op::SessionOpen`]).
pub type SessionId = u64;

/// Asserts that `value` encodes to exactly `json` and that `json` decodes to `value`.
#[cfg(test)]
pub(crate) fn assert_wire<T>(value: &T, json: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let expected: serde_json::Value = serde_json::from_str(json).expect("the example is JSON");
    assert_eq!(
        serde_json::to_value(value).expect("encodes"),
        expected,
        "encoding"
    );
    let decoded: T = serde_json::from_value(expected).expect("the example decodes");
    assert_eq!(&decoded, value, "decoding");
}
