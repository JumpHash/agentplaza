//! The keepalive session (v2): `hello{role,keepalive}` → `welcome{keepalive}`,
//! then a heartbeat every [`HEARTBEAT_SECS`] and a `tool_result` for every hub
//! `tool_call`, until the hub's `stop`, answered with `bye`. Anything else ends
//! the connection with an error and the caller reconnects with
//! [`backoff_delay`]. The loop never acts on its own account.
//!
//! Every wait is bounded by the next beat (the socket's read timeout), so a
//! busy session still beats on time; the interval is a parameter so tests run
//! on milliseconds.

use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use apz_proto::line::Lines;
use apz_proto::wire::{AgentProfile, CAP_ACP, GuestMessage, HostMessage, WELCOME_ECHO};

use crate::files::Workspace;
use crate::lane::Duplex;
use crate::session::{
    Identity, SessionError, await_host, escape_for_transcript, note, send, wrong_echo,
};

pub const HEARTBEAT_SECS: u64 = 5;
/// The first reconnect delay.
pub const BACKOFF_MIN_MS: u64 = 250;
/// The reconnect delay's cap.
pub const BACKOFF_MAX_MS: u64 = 30_000;

pub fn heartbeat_interval() -> Duration {
    Duration::from_secs(HEARTBEAT_SECS)
}

/// Why one keepalive connection ended.
#[derive(Debug)]
pub struct KeepaliveError {
    /// Whether the hub had accepted the agent first; a reconnect after an
    /// adoption restarts the backoff.
    pub adopted: bool,
    pub error: SessionError,
}

impl fmt::Display for KeepaliveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for KeepaliveError {}

impl From<SessionError> for KeepaliveError {
    fn from(error: SessionError) -> Self {
        Self {
            adopted: false,
            error,
        }
    }
}

/// Serves one keepalive connection until the hub's `stop`; returns how many
/// tool calls were served.
pub fn session<S: Duplex, W: Write + Send>(
    lines: &mut Lines<S>,
    log: &mut W,
    identity: &Identity,
    workspace: &Workspace,
    role: Option<&str>,
    heartbeat: Duration,
) -> Result<u64, KeepaliveError> {
    let profile = handshake(lines, log, identity, role)?;
    crate::agent::serve(lines, log, workspace, profile, heartbeat).map_err(|error| KeepaliveError {
        adopted: true,
        error,
    })
}

/// `hello`, then a `welcome` that confirms keepalive (a v1 hub's does not, and
/// is refused rather than guessed at).
fn handshake<S: Read + Write, W: Write>(
    lines: &mut Lines<S>,
    log: &mut W,
    identity: &Identity,
    role: Option<&str>,
) -> Result<Option<AgentProfile>, SessionError> {
    let hello = GuestMessage::Hello {
        vm: identity.vm,
        guest_cid: identity.guest_cid,
        role: role.map(str::to_string),
        keepalive: Some(true),
        // This harness runs ACP engines, so the hub may hold their permission requests for the owner.
        caps: vec![CAP_ACP.to_string()],
    };
    send(lines, log, &hello)?;
    let accepted = await_host(lines, log, "welcome", |message| match message {
        HostMessage::Welcome { echo, .. } if echo != WELCOME_ECHO => Some(Err(wrong_echo(&echo))),
        HostMessage::Welcome {
            keepalive: Some(true),
            role: hub_role,
            profile,
            ..
        } => Some(Ok((hub_role, profile))),
        HostMessage::Welcome { keepalive, .. } => Some(Err(SessionError::Protocol(format!(
            "the hub did not confirm a keepalive session (welcome.keepalive is {keepalive:?})"
        )))),
        _ => None,
    })?;
    let (accepted, profile) = accepted;
    match accepted.or_else(|| role.map(str::to_string)) {
        Some(role) => note(
            log,
            &format!(
                "agent-harness: role {} accepted",
                escape_for_transcript(&role)
            ),
        ),
        None => note(log, "agent-harness: keepalive session accepted (no role)"),
    }
    if let Some(profile) = &profile {
        note(
            log,
            &format!(
                "agent-harness: model-backed as {}",
                escape_for_transcript(&profile.model)
            ),
        );
    }
    Ok(profile)
}

/// The delay before reconnect attempt `failures` (1-based): 250 ms doubling to
/// a 30 s cap, pulled down by up to an eighth by `jitter` so a fleet that lost
/// the same hub does not reconnect in lockstep.
pub fn backoff_delay(failures: u32, jitter: u64) -> Duration {
    let nominal = BACKOFF_MIN_MS
        .saturating_mul(1u64 << failures.saturating_sub(1).min(31))
        .min(BACKOFF_MAX_MS);
    let spread = nominal / 8 + 1;
    Duration::from_millis(nominal - jitter % spread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::{next, pair};
    use std::os::unix::net::UnixStream;
    use std::thread;

    const IDENTITY: Identity = Identity {
        guest_cid: 104,
        vm: 4,
    };
    const HEARTBEAT: Duration = Duration::from_millis(20);
    const WELCOME: &str =
        r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#;

    /// The guest's next line of `kind`, skipping beats (which may land anywhere).
    fn next_kind(host: &mut Lines<UnixStream>, kind: &str) -> String {
        loop {
            let line = next(host);
            let parsed: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            if parsed["type"] == kind {
                return line;
            }
        }
    }

    fn run_session(
        workspace: &Workspace,
        role: Option<&str>,
        hub: impl FnOnce(Lines<UnixStream>) + Send + 'static,
    ) -> (Result<u64, KeepaliveError>, String) {
        let (mut guest, host) = pair();
        let hub = thread::spawn(move || hub(host));
        let mut log = Vec::new();
        let result = session(&mut guest, &mut log, &IDENTITY, workspace, role, HEARTBEAT);
        drop(guest);
        hub.join().expect("the scripted hub finished");
        (result, String::from_utf8(log).expect("UTF-8"))
    }

    #[test]
    fn the_session_beats_serves_and_ends_on_stop() {
        let (result, log) = run_session(&Workspace::default(), Some("builder"), |mut host| {
            assert_eq!(
                next(&mut host),
                r#"{"type":"hello","vm":4,"guest_cid":104,"role":"builder","keepalive":true,"caps":["acp"]}"#
            );
            host.send(WELCOME).unwrap();
            for seq in 1..=3 {
                assert_eq!(
                    next_kind(&mut host, "heartbeat"),
                    format!(r#"{{"type":"heartbeat","seq":{seq},"busy":false}}"#)
                );
            }
            host.send(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"hi there"}}"#)
                .unwrap();
            assert_eq!(
                next_kind(&mut host, "tool_result"),
                r#"{"type":"tool_result","call":1,"ok":true,"output":"hi there"}"#
            );
            host.send("not json").unwrap();
            host.send(r#"{"type":"tool_call","call":2,"name":"exec","args":{}}"#)
                .unwrap();
            assert_eq!(
                next_kind(&mut host, "tool_result"),
                r#"{"type":"tool_result","call":2,"ok":false,"output":"no such tool: exec"}"#
            );
            host.send(r#"{"type":"stop"}"#).unwrap();
            assert_eq!(next_kind(&mut host, "bye"), r#"{"type":"bye","calls":2}"#);
        });
        assert_eq!(result.expect("the hub stops the agent"), 2);
        assert!(
            log.contains("agent-harness: role builder accepted"),
            "{log}"
        );
        assert!(
            log.contains("agent-harness: tool exec call 2 refused: no such tool: exec"),
            "{log}"
        );
    }

    #[test]
    fn a_v1_welcome_is_refused_and_a_drop_after_adoption_says_adopted() {
        let (result, _) = run_session(&Workspace::default(), None, |mut host| {
            next(&mut host);
            host.send(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .unwrap();
        });
        let err = result.expect_err("a v1 welcome is not a keepalive session");
        assert!(
            !err.adopted
                && err
                    .to_string()
                    .contains("did not confirm a keepalive session"),
            "{err}"
        );

        let (result, _) = run_session(&Workspace::default(), None, |mut host| {
            next(&mut host);
            host.send(WELCOME).unwrap();
        });
        let err = result.expect_err("the hub went away");
        assert!(
            err.adopted && matches!(err.error, SessionError::HostClosed),
            "{err:?}"
        );
    }

    #[test]
    fn the_file_tools_are_served_over_the_wire_and_refuse_escapes() {
        let dir = std::env::temp_dir().join(format!("apz-harness-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        let (result, _) = run_session(&Workspace::at(&dir), Some("builder"), |mut host| {
            next(&mut host);
            host.send(WELCOME).unwrap();
            let calls = [
                (
                    r#"{"name":"write_file","args":{"path":"notes.txt","content":"one\ntwo\n"}}"#,
                    true,
                ),
                (
                    r#"{"name":"edit_file","args":{"path":"notes.txt","find":"two","replace":"TWO"}}"#,
                    true,
                ),
                (r#"{"name":"read_file","args":{"path":"notes.txt"}}"#, true),
                (r#"{"name":"read_file","args":{"path":"../escape"}}"#, false),
                (
                    r#"{"name":"conversation","args":{"from_role":"builder","from_cid":100,"text":"hi"}}"#,
                    true,
                ),
            ];
            for (index, (call, ok)) in calls.into_iter().enumerate() {
                let mut call: serde_json::Value = serde_json::from_str(call).unwrap();
                call["type"] = "tool_call".into();
                call["call"] = (index + 1).into();
                host.send(&call.to_string()).unwrap();
                let answer: serde_json::Value =
                    serde_json::from_str(&next_kind(&mut host, "tool_result")).unwrap();
                assert_eq!(
                    (answer["call"].clone(), answer["ok"].clone()),
                    ((index + 1).into(), ok.into()),
                    "{answer}"
                );
                if index == 2 {
                    let output: serde_json::Value =
                        serde_json::from_str(answer["output"].as_str().unwrap()).unwrap();
                    assert_eq!(output["content"], "     1\tone\n     2\tTWO\n");
                }
            }
            host.send(r#"{"type":"stop","reason":"done"}"#).unwrap();
            next_kind(&mut host, "bye");
        });
        assert_eq!(result.expect("a clean stop"), 5);
        let messages = std::fs::read_to_string(dir.join("messages.log")).expect("the agent's log");
        assert!(
            messages.trim_end().ends_with("from builder (cid 100): hi"),
            "{messages}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_backoff_is_the_contracts_schedule() {
        let ms = |failures, jitter| backoff_delay(failures, jitter).as_millis() as u64;
        assert_eq!(
            [ms(1, 0), ms(2, 0), ms(3, 0), ms(8, 0), ms(u32::MAX, 0)],
            [250, 500, 1_000, 30_000, 30_000]
        );
        for failures in [1u32, 3, 40] {
            for jitter in [0u64, 7, u64::MAX] {
                let (nominal, delay) = (ms(failures, 0), ms(failures, jitter));
                assert!(
                    delay <= nominal && delay >= nominal - nominal / 8 - 1,
                    "{failures}/{jitter}: {delay}"
                );
            }
        }
        assert_ne!(backoff_delay(1, 5), backoff_delay(1, 9));
    }
}
