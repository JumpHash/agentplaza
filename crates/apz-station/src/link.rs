//! The frames on the link: what the hub sends and what the daemon sends.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::Event;
use crate::facts::Facts;
use crate::op::Op;

/// A frame the hub sends to the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum HubFrame {
    /// An operation. The daemon answers with one [`StationFrame::Response`] carrying the same `id`.
    Request { id: u64, op: Op },
    /// Something the hub decided.
    Notify { notice: Notice },
}

/// A frame the daemon sends to the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum StationFrame {
    /// Sent first on every link. `version` is the daemon's version string; `facts` is the machine.
    Hello {
        station: String,
        version: String,
        facts: Facts,
    },
    /// The answer to the request `id`. When `ok` is false, `error` says why. When `ok` is true, `result` is
    /// set for the operations that return one and absent for the rest.
    Response {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// An unsolicited event.
    Event { event: Event },
}

/// What the hub tells a daemon. Unit variants, so they encode as plain strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    /// The station's token was revoked: the daemon ends the link and stops.
    Revoked,
    /// The station is draining: take no new requests, finish what runs, then the hub closes the link.
    Drain,
    /// The hub's settings for this station changed; the hub sends them with an `Op::Settings` request.
    SettingsChanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;
    use crate::facts::{AccountMode, Display, Form, Gpu, Os};
    use crate::input::{Button, InputAction};
    use crate::op::SessionKind;

    fn facts() -> Facts {
        Facts {
            os: Os::Macos,
            os_version: "15.2".into(),
            arch: "arm64".into(),
            hostname: "studio-mini".into(),
            form: Form::Mini,
            cpus: 10,
            ram_mb: 32768,
            gpus: vec![Gpu {
                vendor: "apple".into(),
                name: "M4 Pro".into(),
                vram_mb: 0,
            }],
            displays: vec![Display {
                id: 1,
                w: 3024,
                h: 1964,
                scale: 2.0,
            }],
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
            apps: 142,
            virtual_displays: true,
            ui_tree: true,
            machine_id: None,
        }
    }

    const FACTS_JSON: &str = r#"{"os":"macos","os_version":"15.2","arch":"arm64","hostname":"studio-mini","form":"mini","cpus":10,"ram_mb":32768,"gpus":[{"vendor":"apple","name":"M4 Pro","vram_mb":0}],"displays":[{"id":1,"w":3024,"h":1964,"scale":2.0}],"account":{"mode":"agent_user","name":"agentws"},"apps":142,"virtual_displays":true,"ui_tree":true}"#;

    #[test]
    fn hello_carries_the_machine_facts() {
        assert_wire(
            &StationFrame::Hello {
                station: "ws-7".into(),
                version: "0.0.8".into(),
                facts: facts(),
            },
            &format!(
                r#"{{"frame":"hello","station":"ws-7","version":"0.0.8","facts":{FACTS_JSON}}}"#
            ),
        );
    }

    #[test]
    fn a_request_carries_its_operation() {
        assert_wire(
            &HubFrame::Request {
                id: 41,
                op: Op::Facts,
            },
            r#"{"frame":"request","id":41,"op":{"op":"facts"}}"#,
        );
        assert_wire(
            &HubFrame::Request {
                id: 42,
                op: Op::SessionOpen {
                    agent: 3,
                    kind: SessionKind::Pixel,
                },
            },
            r#"{"frame":"request","id":42,"op":{"op":"session_open","agent":3,"kind":"pixel"}}"#,
        );
        assert_wire(
            &HubFrame::Request {
                id: 43,
                op: Op::Input {
                    session: 9,
                    action: InputAction::Click {
                        x: 640,
                        y: 360,
                        button: Button::Left,
                        count: 2,
                    },
                },
            },
            r#"{"frame":"request","id":43,"op":{"op":"input","session":9,"action":{"action":"click","x":640,"y":360,"button":"left","count":2}}}"#,
        );
    }

    #[test]
    fn a_response_is_ok_with_a_result_or_not_ok_with_an_error() {
        assert_wire(
            &StationFrame::Response {
                id: 41,
                ok: true,
                result: Some(serde_json::json!({"ok": true})),
                error: None,
            },
            r#"{"frame":"response","id":41,"ok":true,"result":{"ok":true}}"#,
        );
        assert_wire(
            &StationFrame::Response {
                id: 42,
                ok: false,
                result: None,
                error: Some("no session 9".into()),
            },
            r#"{"frame":"response","id":42,"ok":false,"error":"no session 9"}"#,
        );
    }

    #[test]
    fn an_event_frame_wraps_the_event() {
        assert_wire(
            &StationFrame::Event {
                event: Event::OwnerInput { session: 9 },
            },
            r#"{"frame":"event","event":{"event":"owner_input","session":9}}"#,
        );
    }

    #[test]
    fn every_notice_encodes_as_a_plain_string() {
        for (notice, json) in [
            (Notice::Revoked, "revoked"),
            (Notice::Drain, "drain"),
            (Notice::SettingsChanged, "settings_changed"),
        ] {
            assert_wire(
                &HubFrame::Notify { notice },
                &format!(r#"{{"frame":"notify","notice":"{json}"}}"#),
            );
        }
    }
}
