//! Events the daemon sends without being asked.

use serde::{Deserialize, Serialize};

use crate::SessionId;
use crate::facts::Facts;
use crate::op::ImageFormat;

/// How serious a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// An unsolicited event from the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A screen frame of a live session. `data_b64` is the whole image encoded as `format`; `seq`
    /// counts the session's frames from 1, so the hub can drop a late one.
    Frame {
        session: SessionId,
        seq: u64,
        w: u32,
        h: u32,
        format: ImageFormat,
        data_b64: String,
    },
    /// The machine's facts changed (an app installed, a display plugged in).
    Facts(Facts),
    /// Work-folder files changed on the machine; `paths` are relative to the work folder.
    SyncChanged { paths: Vec<String> },
    /// A screen session ended, and why (`closed`, `owner_takeover`, `stop_all`, `lease_expired`, ...).
    SessionEnded { session: SessionId, reason: String },
    /// The owner touched the keyboard or pointer of a session the agent is using: a takeover.
    OwnerInput { session: SessionId },
    /// A log line for the hub's station log.
    Log { level: Level, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    #[test]
    fn a_frame_event_carries_the_encoded_image() {
        assert_wire(
            &Event::Frame {
                session: 9,
                seq: 128,
                w: 1280,
                h: 720,
                format: ImageFormat::Jpeg,
                data_b64: "/9j/4AAQSkZJRg==".into(),
            },
            r#"{"event":"frame","session":9,"seq":128,"w":1280,"h":720,"format":"jpeg","data_b64":"/9j/4AAQSkZJRg=="}"#,
        );
    }

    #[test]
    fn sync_session_and_owner_events_round_trip() {
        assert_wire(
            &Event::SyncChanged {
                paths: vec!["reports/q3.csv".into(), "render/out.png".into()],
            },
            r#"{"event":"sync_changed","paths":["reports/q3.csv","render/out.png"]}"#,
        );
        assert_wire(
            &Event::SessionEnded {
                session: 9,
                reason: "owner_takeover".into(),
            },
            r#"{"event":"session_ended","session":9,"reason":"owner_takeover"}"#,
        );
        assert_wire(
            &Event::OwnerInput { session: 9 },
            r#"{"event":"owner_input","session":9}"#,
        );
    }

    #[test]
    fn a_facts_event_carries_the_whole_facts_object() {
        use crate::facts::{AccountMode, Facts, Form, Os};
        assert_wire(
            &Event::Facts(Facts {
                os: Os::Linux,
                os_version: "6.8".into(),
                arch: "x86_64".into(),
                hostname: "desk-1".into(),
                form: Form::Desktop,
                cpus: 16,
                ram_mb: 65536,
                gpus: vec![],
                displays: vec![],
                account: AccountMode::Owner {
                    name: "irfan".into(),
                },
                apps: 0,
                virtual_displays: false,
                ui_tree: false,
                machine_id: None,
            }),
            r#"{"event":"facts","os":"linux","os_version":"6.8","arch":"x86_64","hostname":"desk-1","form":"desktop","cpus":16,"ram_mb":65536,"gpus":[],"displays":[],"account":{"mode":"owner","name":"irfan"},"apps":0,"virtual_displays":false,"ui_tree":false}"#,
        );
    }

    #[test]
    fn a_log_line_has_a_level() {
        for (level, json) in [
            (Level::Info, "info"),
            (Level::Warn, "warn"),
            (Level::Error, "error"),
        ] {
            assert_wire(
                &Event::Log {
                    level,
                    message: "blender exited 0".into(),
                },
                &format!(r#"{{"event":"log","level":"{json}","message":"blender exited 0"}}"#),
            );
        }
    }
}
