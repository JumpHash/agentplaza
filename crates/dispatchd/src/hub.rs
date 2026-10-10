//! The hub's admin surface, reached over the loopback: one `POST /tell` per
//! tool call, and the agent's own `tool_result` as the answer.
//!
//! This is the only way the dispatcher makes an agent *do* anything, and it is
//! one call deep: the dispatcher asks for a tool, the hub forwards it into the
//! agent's session and waits (bounded, ≤ 5 s), and the answer that comes back
//! is the agent's. Nothing here invents a tool, retries a call, or turns a
//! non-answer into an answer: a hub that is not there is an error, a CID the
//! hub does not serve is [`Tell::UnknownCid`], an agent with no session is
//! [`Tell::NotConnected`], and an agent that does not answer is
//! [`Tell::TimedOut`] — each reported as itself, so the pass can decide what
//! to retry.
//!
//! The dispatcher's own wait is the hub's maximum (5 s) and the socket's
//! deadline is longer than that, so a call is never cut off by this client
//! before the hub has answered: the verdict is the hub's, not a race between
//! two timeouts.

use serde::Deserialize;

use crate::dispatch::Operator;
use crate::http::{self, Target};
use crate::views::Tell;

/// The hub's admin surface.
#[derive(Debug, Clone)]
pub struct Hub {
    target: Target,
}

impl Hub {
    /// A client for the hub's admin surface at `url`
    /// (`http://127.0.0.1:19080`).
    pub fn new(url: &str) -> Result<Self, String> {
        Ok(Self {
            target: Target::parse(url)?,
        })
    }

    /// Where this client reaches the hub.
    pub fn target(&self) -> Target {
        self.target
    }
}

impl Operator for Hub {
    fn tell(
        &self,
        cid: u64,
        name: &str,
        args: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<Tell, String> {
        let body = serde_json::json!({
            "cid": cid,
            "name": name,
            "args": args,
            "timeout_ms": timeout_ms,
        })
        .to_string();
        let answer = http::post(&self.target, "/tell", body.as_bytes(), http::TELL_TIMEOUT)?;
        match answer.status() {
            200 => {
                let view: TellView = serde_json::from_slice(answer.body())
                    .map_err(|err| format!("the hub's answer could not be read: {err}"))?;
                Ok(Tell::Answered {
                    ok: view.ok,
                    output: view.output,
                })
            }
            // The four statuses the surface's contract defines, each as
            // itself: a CID the hub does not serve, a served CID with no live
            // session, and an agent that did not answer in time. The waited
            // duration is this client's own request, not a guess at what the
            // hub managed.
            404 => Ok(Tell::UnknownCid),
            409 => Ok(Tell::NotConnected),
            504 => Ok(Tell::TimedOut {
                waited_ms: timeout_ms,
            }),
            other => Err(format!("the hub answered {other}: {}", answer.refusal())),
        }
    }
}

/// The admin surface's own answer shape: `{"ok": bool, "output": "..."}`.
#[derive(Debug, Deserialize)]
struct TellView {
    ok: bool,
    output: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::mpsc::{Receiver, channel};
    use std::thread;

    /// A stub admin surface: it answers each connection with the next canned
    /// reply and reports the request body it was sent.
    fn stub(
        replies: Vec<(u16, &'static str)>,
    ) -> (Target, Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let (requests, received) = channel();
        let served = thread::spawn(move || {
            for (status, body) in replies {
                let (mut stream, _peer) = listener.accept().expect("the client connects");
                let request = crate::testing::read_http_request(&mut stream);
                requests
                    .send(
                        request
                            .split_once("\r\n\r\n")
                            .map(|(_, body)| body.to_string())
                            .unwrap_or_default(),
                    )
                    .expect("the test is listening");
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("the answer is written");
            }
        });
        (
            Target::parse(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL"),
            received,
            served,
        )
    }

    /// The wire contract: the dispatcher sends the CID, the tool, the
    /// arguments and its own wait, and reads the agent's answer back.
    #[test]
    fn a_tell_sends_the_tool_and_reads_the_agents_answer() {
        let (target, requests, served) = stub(vec![
            (
                200,
                r#"{"ok":true,"output":"{\"path\":\"build-notes.txt\",\"bytes\":7}"}"#,
            ),
            (200, r#"{"ok":false,"output":"denied: nope"}"#),
        ]);
        let hub = Hub { target };

        let answer = hub
            .tell(
                300,
                "write_file",
                &serde_json::json!({ "path": "build-notes.txt" }),
                5_000,
            )
            .expect("the hub answers");
        assert_eq!(
            answer,
            Tell::Answered {
                ok: true,
                output: r#"{"path":"build-notes.txt","bytes":7}"#.to_string(),
            }
        );
        let sent: serde_json::Value =
            serde_json::from_str(&requests.recv().expect("a request")).expect("the body is JSON");
        assert_eq!(sent["cid"], serde_json::json!(300));
        assert_eq!(sent["name"], serde_json::json!("write_file"));
        assert_eq!(sent["args"]["path"], serde_json::json!("build-notes.txt"));
        assert_eq!(sent["timeout_ms"], serde_json::json!(5_000));

        // A refusal is an *answer*: the agent said no, and the dispatcher
        // hears it.
        let answer = hub
            .tell(300, "write_file", &serde_json::json!({}), 5_000)
            .expect("the hub answers");
        assert_eq!(
            answer,
            Tell::Answered {
                ok: false,
                output: "denied: nope".to_string(),
            }
        );
        let _ = requests.recv();
        served.join().expect("the stub surface finishes");
    }

    /// The surface's four statuses, each as itself: an unknown CID, a not
    /// connected agent, an honest timeout, and anything else as an error.
    #[test]
    fn the_surfaces_statuses_are_each_their_own_verdict() {
        let (target, _requests, served) = stub(vec![
            (404, r#"{"error":"no agent with guest CID 99"}"#),
            (
                409,
                r#"{"error":"agent with guest CID 300 is not connected"}"#,
            ),
            (504, r#"{"error":"agent did not answer within 5000 ms"}"#),
            (500, r#"{"error":"boom"}"#),
        ]);
        let hub = Hub { target };
        let args = serde_json::json!({});

        assert_eq!(
            hub.tell(99, "echo", &args, 5_000).expect("an answer"),
            Tell::UnknownCid
        );
        assert_eq!(
            hub.tell(300, "echo", &args, 5_000).expect("an answer"),
            Tell::NotConnected
        );
        assert_eq!(
            hub.tell(300, "echo", &args, 1_234).expect("an answer"),
            Tell::TimedOut { waited_ms: 1_234 }
        );
        let err = hub
            .tell(300, "echo", &args, 5_000)
            .expect_err("the hub broke");
        assert_eq!(err, "the hub answered 500: boom");
        served.join().expect("the stub surface finishes");
    }

    /// An answer that is not the surface's shape is an error naming what came
    /// back, never a made-up success.
    #[test]
    fn an_answer_that_is_not_the_surfaces_shape_is_an_error() {
        let (target, _requests, served) = stub(vec![(200, r#"{"ok":"yes"}"#)]);
        let hub = Hub { target };
        let err = hub
            .tell(300, "echo", &serde_json::json!({}), 5_000)
            .expect_err("the shape is wrong");
        assert!(err.contains("could not be read"), "{err}");
        served.join().expect("the stub surface finishes");
    }

    /// A hub that is not there is an honest connection error.
    #[test]
    fn a_hub_that_is_not_there_is_an_honest_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port is free for a moment");
        let addr = listener.local_addr().expect("a bound address");
        drop(listener);
        let hub = Hub {
            target: Target::parse(&format!("http://127.0.0.1:{}", addr.port())).expect("a URL"),
        };
        let err = hub
            .tell(300, "echo", &serde_json::json!({}), 5_000)
            .expect_err("nothing is listening");
        assert!(err.contains("cannot connect"), "{err}");
    }
}
