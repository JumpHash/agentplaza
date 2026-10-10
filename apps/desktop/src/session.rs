//! The frozen vsock session, read back out of a VM's serial capture.
//!
//! The harness logs every protocol line it sends (`[vsock->]`) and receives
//! (`[vsock<-]`) to the serial console — the only place a session is
//! observable from outside the guest. This module is the one pure derivation
//! of "what the session did": the run report's summary line is built from it
//! alone, so the report can never disagree with the transcript it carries.
//!
//! The capture is untrusted guest output and is not protocol: the kernel's
//! boot, the guest init's own lines and the harness's transcript are all
//! interleaved on it. Only a line that starts with one of the two prefixes is
//! read, and a payload that is not the JSON object the frozen shapes require
//! (see `crates/agent-harness/src/proto.rs`) is skipped, never believed.
//!
//! The reading restarts at every `hello`: a guest that reboots (the harness
//! exits by rebooting) begins a new session, and a keepalive guest that
//! reconnects begins one too, so the summary describes the latest session the
//! capture shows. A capture whose latest session neither completed nor
//! announced a keepalive session is [`SessionRead::None`] — the report says it
//! saw no session rather than claiming one from a fragment.

use serde_json::{Map, Value};

/// What the harness writes before a line the guest sends.
const SENT: &str = "[vsock->] ";
/// What the harness writes before a line the guest receives.
const RECEIVED: &str = "[vsock<-] ";
/// The stamp a tool call gets once its result landed.
const ANSWERED: char = '✓';
/// The stamp a tool call gets when its result says the tool failed.
const FAILED: char = '✗';

/// Where a VM's session stands, derived from its serial capture alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRead {
    /// The frozen v1 protocol ran to its `bye`: the steps in observed order,
    /// and the tool calls the session made.
    Complete {
        /// The session's steps: `hello`, `welcome`, each tool by name with its
        /// answer stamped, `bye`.
        steps: Vec<String>,
        /// The tool calls the `bye` reported (or, when it carried no count,
        /// the ones the capture showed).
        calls: u64,
    },
    /// A keepalive (v2) session: the guest beat, the hub asked it to serve
    /// tools, and a `bye` landed only if the hub stopped it.
    Keepalive {
        /// How many heartbeats the guest sent.
        heartbeats: u64,
        /// How many tool calls the guest served (the `bye`'s own count once it
        /// landed).
        calls: u64,
        /// Whether the guest sent its `bye` — the hub stopped it.
        stopped: bool,
    },
    /// The capture's latest session neither completed nor announced a
    /// keepalive one: nothing to summarise.
    None,
}

impl SessionRead {
    /// The one-line summary the run report carries:
    /// `hello→welcome→echo✓→uptime✓→bye, 2 tool calls`, or
    /// `keepalive, 3 heartbeats, 2 tool calls, no bye seen`, or `none seen`.
    pub fn summary(&self) -> String {
        match self {
            SessionRead::Complete { steps, calls } => {
                format!("{}, {}", steps.join("→"), tool_calls(*calls))
            }
            SessionRead::Keepalive {
                heartbeats,
                calls,
                stopped,
            } => format!(
                "keepalive, {}, {}, {}",
                heartbeats_label(*heartbeats),
                tool_calls(*calls),
                if *stopped { "bye seen" } else { "no bye seen" }
            ),
            SessionRead::None => "none seen".to_string(),
        }
    }
}

/// A tool-call count as the summary writes it.
fn tool_calls(calls: u64) -> String {
    format!("{calls} tool call{}", if calls == 1 { "" } else { "s" })
}

/// A heartbeat count as the summary writes it.
fn heartbeats_label(heartbeats: u64) -> String {
    format!(
        "{heartbeats} heartbeat{}",
        if heartbeats == 1 { "" } else { "s" }
    )
}

/// The latest session a serial capture shows.
pub fn read(capture: &str) -> SessionRead {
    let mut reading: Option<Reading> = None;
    for line in capture.lines() {
        let line = line.trim_end();
        let (sent, payload) = match (line.strip_prefix(SENT), line.strip_prefix(RECEIVED)) {
            (Some(payload), _) => (true, payload),
            (_, Some(payload)) => (false, payload),
            _ => continue,
        };
        let Some(message) = object(payload) else {
            continue;
        };
        let Some(kind) = message.get("type").and_then(Value::as_str) else {
            continue;
        };
        match (sent, kind) {
            // A new session begins here, and the earlier one is forgotten: a
            // reboot or a reconnect starts the reading over.
            (true, "hello") => {
                reading = Some(Reading {
                    steps: vec!["hello".to_string()],
                    keepalive: keepalive_field(&message),
                    ..Reading::default()
                });
            }
            // v1: the guest asks the orchestrator to run a tool.
            (true, "tool_call") => {
                if let Some(reading) = reading.as_mut() {
                    if !reading.bye {
                        reading.asked += 1;
                        if let Some(name) = message.get("name").and_then(Value::as_str) {
                            let step = reading.steps.len();
                            reading.steps.push(name.to_string());
                            if let Some(call) = message.get("call").and_then(Value::as_u64) {
                                reading.pending.push((call, step));
                            }
                        }
                    }
                }
            }
            // v2: the guest serves the hub's tool calls.
            (true, "tool_result") => {
                if let Some(reading) = reading.as_mut() {
                    reading.served += 1;
                }
            }
            (true, "heartbeat") => {
                if let Some(reading) = reading.as_mut() {
                    reading.keepalive = true;
                    reading.heartbeats += 1;
                }
            }
            (true, "bye") => {
                if let Some(reading) = reading.as_mut() {
                    if !reading.steps.is_empty() {
                        reading.steps.push("bye".to_string());
                    }
                    reading.bye = true;
                    reading.calls = message.get("calls").and_then(Value::as_u64);
                }
            }
            // v1: the answer to the guest's own tool call.
            (false, "tool_result") => {
                if let Some(reading) = reading.as_mut() {
                    if !reading.bye {
                        if let Some(index) = message
                            .get("call")
                            .and_then(Value::as_u64)
                            .and_then(|call| reading.pending.iter().position(|(id, _)| *id == call))
                        {
                            let (_, step) = reading.pending.remove(index);
                            let stamp =
                                if message.get("ok") == Some(&Value::Bool(false)) {
                                    FAILED
                                } else {
                                    ANSWERED
                                };
                            if let Some(step) = reading.steps.get_mut(step) {
                                step.push(stamp);
                            }
                        }
                    }
                }
            }
            (false, "welcome") => {
                if let Some(reading) = reading.as_mut() {
                    if keepalive_field(&message) {
                        reading.keepalive = true;
                    }
                    if !reading.steps.iter().any(|step| step == "welcome") {
                        reading.steps.push("welcome".to_string());
                    }
                }
            }
            // A hub's tool call (v2) or stop: the guest's own `bye` is what
            // ends the session, and the hub's lines are not steps.
            (false, _) => {}
            (true, _) => {}
        }
    }

    let Some(reading) = reading else {
        return SessionRead::None;
    };
    if reading.keepalive {
        return SessionRead::Keepalive {
            heartbeats: reading.heartbeats,
            calls: reading.calls.unwrap_or(reading.served),
            stopped: reading.bye,
        };
    }
    if reading.bye {
        return SessionRead::Complete {
            steps: reading.steps,
            calls: reading.calls.unwrap_or(reading.asked),
        };
    }
    SessionRead::None
}

/// The session in progress, as the capture has shown it so far.
#[derive(Debug, Default)]
struct Reading {
    /// The steps, in observed order.
    steps: Vec<String>,
    /// The guest's own tool calls, by wire call number: the step their result
    /// will stamp.
    pending: Vec<(u64, usize)>,
    /// How many tool calls the guest made (v1).
    asked: u64,
    /// How many tool calls the guest served (v2).
    served: u64,
    /// How many heartbeats the guest sent.
    heartbeats: u64,
    /// Whether this is a keepalive session.
    keepalive: bool,
    /// Whether the guest sent its `bye`.
    bye: bool,
    /// The `bye`'s tool-call count, when it carried one.
    calls: Option<u64>,
}

/// A payload as the plain JSON object the protocol lines carry, or `None` when
/// it is not one: the capture is guest output and is never believed to be well
/// formed.
fn object(payload: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(payload) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Whether a `hello` or `welcome` asks for (or confirms) a keepalive session.
fn keepalive_field(message: &Map<String, Value>) -> bool {
    message.get("keepalive") == Some(&Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real v1 session as the console carries it: the guest init's own lines
    /// and the harness's transcript interleaved, with CRLF line endings.
    const V1: &str = "plaza guest: plaza.harness=1: loading the vsock modules\r\n\
                      [    0.417190] virtio_vsock: vsock ready\r\n\
                      agent-harness: vm 0 -> host CID 2 port 5000\r\n\
                      [vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\r\n\
                      [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\r\n\
                      agent-harness: handshake complete (host echo \"plaza-orchestrator\")\r\n\
                      [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{\"text\":\"plaza-guest-40-hello\"}}\r\n\
                      [vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"plaza-guest-40-hello\"}\r\n\
                      agent-harness: echo call 1 verified: \"plaza-guest-40-hello\"\r\n\
                      [vsock->] {\"type\":\"tool_call\",\"call\":2,\"name\":\"uptime\",\"args\":{}}\r\n\
                      [vsock<-] {\"type\":\"tool_result\",\"call\":2,\"ok\":true,\"output\":\"12.34 45.67\"}\r\n\
                      agent-harness: uptime call 2: 12.34 45.67\r\n\
                      [vsock->] {\"type\":\"bye\",\"calls\":2}\r\n\
                      agent-harness: session complete after 2 tool calls\r\n\
                      plaza guest: agent harness session complete, rebooting\r\n";

    #[test]
    fn a_frozen_session_between_boot_lines_reads_as_its_steps() {
        assert_eq!(
            read(V1),
            SessionRead::Complete {
                steps: vec![
                    "hello".to_string(),
                    "welcome".to_string(),
                    "echo✓".to_string(),
                    "uptime✓".to_string(),
                    "bye".to_string(),
                ],
                calls: 2,
            }
        );
        assert_eq!(
            read(V1).summary(),
            "hello→welcome→echo✓→uptime✓→bye, 2 tool calls"
        );
    }

    #[test]
    fn a_keepalive_session_reads_as_beats_and_served_calls_with_no_bye() {
        let capture = "agent-harness: vm 0 -> host CID 2 port 5000\n\
                       [vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":100,\"role\":\"builder\",\"keepalive\":true}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"role\":\"builder\",\"keepalive\":true}\n\
                       agent-harness: role builder accepted\n\
                       [vsock->] {\"type\":\"heartbeat\",\"seq\":1}\n\
                       agent-harness: heartbeat 1\n\
                       [vsock->] {\"type\":\"heartbeat\",\"seq\":2}\n\
                       [vsock<-] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{\"text\":\"x\"}}\n\
                       [vsock->] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"x\"}\n\
                       [vsock<-] {\"type\":\"tool_call\",\"call\":2,\"name\":\"uptime\",\"args\":{}}\n\
                       [vsock->] {\"type\":\"tool_result\",\"call\":2,\"ok\":true,\"output\":\"1.0 2.0\"}\n\
                       [vsock->] {\"type\":\"heartbeat\",\"seq\":3}\n";
        assert_eq!(
            read(capture),
            SessionRead::Keepalive {
                heartbeats: 3,
                calls: 2,
                stopped: false,
            }
        );
        assert_eq!(
            read(capture).summary(),
            "keepalive, 3 heartbeats, 2 tool calls, no bye seen"
        );

        // A heartbeat is itself the evidence: a hub whose welcome did not
        // carry the field has still accepted a keepalive session, and the
        // beat says so.
        let beats_only = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":100}\n\
                          [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                          [vsock->] {\"type\":\"heartbeat\",\"seq\":1}\n";
        assert_eq!(
            read(beats_only),
            SessionRead::Keepalive {
                heartbeats: 1,
                calls: 0,
                stopped: false,
            }
        );

        // So is the hello's own ask, before anything else has happened.
        let asked = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":100,\"keepalive\":true}\n";
        assert_eq!(
            read(asked).summary(),
            "keepalive, 0 heartbeats, 0 tool calls, no bye seen"
        );
    }

    #[test]
    fn a_keepalive_session_the_hub_stopped_says_the_bye_was_seen() {
        let capture = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":100,\"keepalive\":true}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"keepalive\":true}\n\
                       [vsock->] {\"type\":\"heartbeat\",\"seq\":1}\n\
                       [vsock<-] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{}}\n\
                       [vsock->] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"x\"}\n\
                       [vsock<-] {\"type\":\"stop\",\"reason\":\"the fleet is going down\"}\n\
                       [vsock->] {\"type\":\"bye\",\"calls\":1}\n";
        assert_eq!(
            read(capture).summary(),
            "keepalive, 1 heartbeat, 1 tool call, bye seen"
        );
    }

    #[test]
    fn a_partial_session_and_a_capture_with_none_read_as_none_seen() {
        // The bye never landed, and no keepalive was announced: no session is
        // claimed from a fragment.
        let partial = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{}}\n";
        assert_eq!(read(partial), SessionRead::None);
        assert_eq!(read(partial).summary(), "none seen");

        // A console with no protocol at all — the boot of a VM whose harness
        // never connected.
        let bare = "login: plaza\r\n$ uname -a\r\nLinux plaza 7.2.8 x86_64\r\n";
        assert_eq!(read(bare), SessionRead::None);
        assert_eq!(read("").summary(), "none seen");
    }

    #[test]
    fn a_payload_that_is_not_the_frozen_shape_is_skipped_never_believed() {
        let capture = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\"\n\
                       [vsock->] not json at all\n\
                       [vsock->] [\"an array is not a message\"]\n\
                       [vsock->] {\"type\":42}\n\
                       [vsock->] {}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{}}\n\
                       [vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"x\"}\n\
                       [vsock<-] {\"type\":\"tool_result\",\"call\":9,\"ok\":true,\"output\":\"nobody asked\"}\n\
                       [vsock->] {\"type\":\"bye\",\"calls\":1}\n";
        // The malformed lines added no step and no call; the real echo call is
        // stamped by its own result, and the result for a call nobody made is
        // ignored rather than attached to the wrong step.
        assert_eq!(
            read(capture).summary(),
            "hello→welcome→echo✓→bye, 1 tool call"
        );
    }

    #[test]
    fn a_failed_tool_is_stamped_and_the_sessions_own_count_wins() {
        let capture = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"uptime\",\"args\":{}}\n\
                       [vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":false,\"output\":\"no clock\"}\n\
                       [vsock->] {\"type\":\"bye\"}\n";
        // The bye carried no count: the calls the capture showed stand in.
        assert_eq!(
            read(capture).summary(),
            "hello→welcome→uptime✗→bye, 1 tool call"
        );
    }

    #[test]
    fn a_results_call_number_decides_which_step_it_stamps() {
        // The two calls are outstanding together and their answers come back
        // in the other order: each result belongs to the call it names, so the
        // failures and the marks do not swap.
        let capture = "[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{}}\n\
                       [vsock->] {\"type\":\"tool_call\",\"call\":2,\"name\":\"uptime\",\"args\":{}}\n\
                       [vsock<-] {\"type\":\"tool_result\",\"call\":2,\"ok\":false,\"output\":\"no clock\"}\n\
                       [vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"x\"}\n\
                       [vsock->] {\"type\":\"bye\",\"calls\":2}\n";
        assert_eq!(
            read(capture).summary(),
            "hello→welcome→echo✓→uptime✗→bye, 2 tool calls"
        );
    }

    #[test]
    fn the_reading_restarts_at_each_hello_so_the_latest_session_is_described() {
        // A guest that rebooted: the first session completed, the second is
        // the one the summary describes.
        let capture = format!(
            "{V1}[vsock->] {{\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}}\n\
             [vsock->] {{\"type\":\"bye\",\"calls\":0}}\n"
        );
        assert_eq!(
            read(&capture).summary(),
            "hello→bye, 0 tool calls"
        );
    }

    #[test]
    fn a_session_that_follows_a_boot_line_is_read_past_the_console() {
        // The console interleaves the guest init's lines between protocol
        // frames; none of them is protocol, and none of them resets the
        // session.
        let capture = "[vsock->] {\"type\":\"hello\",\"vm\":1,\"guest_cid\":41}\n\
                       plaza guest: the harness is connected\n\
                       [    1.203] EXT4-fs (vda): mounted filesystem\n\
                       [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\n\
                       agent-harness: handshake complete\n\
                       [vsock->] {\"type\":\"bye\",\"calls\":0}\n";
        assert_eq!(read(capture).summary(), "hello→welcome→bye, 0 tool calls");
    }
}
