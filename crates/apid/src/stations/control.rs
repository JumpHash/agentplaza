//! The owner's takeover socket: `WS /v1/stations/{id}/control?session=<id|shared>` (docs/design/workstations.md
//! section 7). The owner takes a session on it, types and clicks into the station while this socket holds the session,
//! and gives it back. The station's takeover policy decides what the agent does meanwhile (takeover.rs). Owner-only
//! (the gate in `hub_auth`), subprotocol `apz.station.control.v1`.
//!
//! Client to hub:
//! - `{"t":"take"}`: takes the session under the station's policy.
//! - `{"t":"input","action":<InputAction>}`: one keyboard or pointer action, only while this socket holds the session.
//!   A successful input is not answered; a refused one is an `error`.
//! - `{"t":"give_back"}`.
//! - `{"t":"ping"}`.
//!
//! Hub to client:
//! - `{"t":"taken","session":N,"policy":"agent_waits"}`.
//! - `{"t":"given_back"}`.
//! - `{"t":"error","error":"..."}`: the frame was refused; the socket stays open.
//! - `{"t":"pong"}`.
//! - `{"t":"ended","reason":"..."}` when the session ends; the socket then closes (1000).
//!
//! Closing the socket gives back what it holds. Each input is written to the station audit, at most once per 30 s per
//! session, with its kind and never its text, as the owner-input audit is.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use apz_http::ws::{self, Message, Origins};
use apz_http::{Request, Response, Router};
use apz_station::{Event, InputAction, Op, SessionId, Takeover};
use serde_json::{Value, json};

use super::ops::{self, RPC_TIMEOUT, TAKEOVER_AUDIT_MS};
use super::screen::{LIMITS, Pick, SOCKETS, Wire, parse_pick, send_text};
use super::takeover;
use crate::state::App;

/// The subprotocol a control socket asks for.
pub const PROTOCOL: &str = "apz.station.control.v1";
/// How long a control socket waits for a client message between batches of events.
const POLL: Duration = Duration::from_millis(50);

/// A frame from the client.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientFrame {
    Take,
    Input(InputAction),
    GiveBack,
    Ping,
}

/// Reads one client frame. A frame that is not one of these is an error for the client to hear.
pub fn parse_client(text: &str) -> Result<ClientFrame, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|err| format!("the frame is not JSON: {err}"))?;
    let kind = value
        .get("t")
        .and_then(Value::as_str)
        .ok_or("the frame names no \"t\"")?;
    match kind {
        "take" => Ok(ClientFrame::Take),
        "input" => {
            let action = value
                .get("action")
                .cloned()
                .ok_or("the input frame names no \"action\"")?;
            serde_json::from_value(action)
                .map(ClientFrame::Input)
                .map_err(|err| format!("the action is not an input: {err}"))
        }
        "give_back" => Ok(ClientFrame::GiveBack),
        "ping" => Ok(ClientFrame::Ping),
        other => Err(format!("no frame named {other:?}")),
    }
}

/// What a control socket does: the takeover of its session and the owner's input to it. The live hub is [`Live`];
/// the tests use a fake.
pub trait Desk {
    /// Takes the session; the answer is the policy in force.
    fn take(&self) -> Result<Takeover, String>;
    fn give_back(&self) -> Result<(), String>;
    /// Whether this socket holds the session now.
    fn holds(&self) -> bool;
    /// One owner action on the station.
    fn input(&self, action: &InputAction) -> Result<(), String>;
}

/// The live hub's desk: one station and session, for one control socket.
pub struct Live<'a> {
    pub app: &'a App,
    pub station: &'a str,
    pub session: SessionId,
    pub holder: u64,
}

impl Desk for Live<'_> {
    fn take(&self) -> Result<Takeover, String> {
        takeover::take(self.app, self.station, self.session, self.holder)
    }

    fn give_back(&self) -> Result<(), String> {
        takeover::give_back(self.app, self.station, self.session, self.holder)
    }

    fn holds(&self) -> bool {
        takeover::holds(self.app, self.station, self.session, self.holder)
    }

    fn input(&self, action: &InputAction) -> Result<(), String> {
        let app = self.app;
        if app
            .stations
            .takeover_due(self.session, apz_plaza::now_ms(), TAKEOVER_AUDIT_MS)
        {
            let kind = serde_json::to_value(action)
                .ok()
                .and_then(|v| v["action"].as_str().map(str::to_string))
                .unwrap_or_default();
            ops::audit(
                app,
                Some(self.station),
                None,
                "owner_input",
                "owner",
                json!({"session": self.session, "source": "control", "action": kind}),
            )?;
        }
        let link = app
            .stations
            .live(self.station)
            .ok_or("the workstation is not linked")?;
        link.request(
            Op::Input {
                session: self.session,
                action: action.clone(),
            },
            RPC_TIMEOUT,
        )
        .map(|_| ())
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/stations/{id}/control", |app, req, p| {
        control_route(app, req, p.get("id"))
    })
}

fn control_route(app: &App, req: &Request, id: &str) -> Response {
    let pick = match parse_pick(req.query_param("session")) {
        Ok(pick) => pick,
        Err(reason) => return Response::error(400, &reason),
    };
    let Some(station) = app.stations.station(id) else {
        return Response::error(404, "no such workstation");
    };
    if station.revoked {
        return Response::error(409, "this workstation was revoked");
    }
    let Some(link) = app.stations.live(id) else {
        return Response::error(409, "this workstation is offline");
    };
    let session = match pick {
        Pick::Session(session) => session,
        Pick::Shared => match app.stations.screen.shared_session(id) {
            Some(session) => session,
            None => {
                return Response::error(
                    409,
                    "no shared view is open on this workstation: open the live view first",
                );
            }
        },
    };
    let Some(slot) = SOCKETS.take(req.peer) else {
        return Response::error(
            429,
            "too many open live views and takeover sockets; close one first",
        );
    };
    // Subscribed before the socket starts, so the session's end is not missed.
    let events = link.subscribe();
    let holder = app.stations.taken.next_holder();
    let weak = app.handle();
    let station_id = id.to_string();
    ws::accept(
        req,
        Origins::SameHost,
        Some(PROTOCOL),
        LIMITS,
        move |mut socket| {
            let _slot = slot;
            let Some(app) = weak.upgrade() else {
                socket.close(1001, "the hub is shutting down");
                return;
            };
            let desk = Live {
                app: app.as_ref(),
                station: &station_id,
                session,
                holder,
            };
            serve(&desk, session, &events, &mut socket);
        },
    )
}

/// Serves one control socket until it ends. Closing it gives back what it holds.
fn serve(desk: &dyn Desk, session: SessionId, events: &Receiver<Event>, socket: &mut dyn Wire) {
    pump(desk, session, events, socket);
    if desk.holds() {
        let _ = desk.give_back();
    }
}

fn pump(desk: &dyn Desk, session: SessionId, events: &Receiver<Event>, socket: &mut dyn Wire) {
    loop {
        loop {
            match events.try_recv() {
                Ok(Event::SessionEnded {
                    session: id,
                    reason,
                }) if id == session => {
                    send_text(socket, &json!({"t": "ended", "reason": reason}));
                    socket.close(1000, "the session ended");
                    return;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    socket.close(1011, "the workstation's link ended");
                    return;
                }
            }
        }
        match socket.recv(POLL) {
            Ok(None) => {}
            Ok(Some(Message::Text(text))) => {
                if !client(desk, session, &text, socket) {
                    return;
                }
            }
            Ok(Some(Message::Binary(_))) => {
                socket.close(1003, "text frames only");
                return;
            }
            Err(_) => return,
        }
    }
}

/// One client frame, acted on. False when the reply could not be sent.
fn client(desk: &dyn Desk, session: SessionId, text: &str, socket: &mut dyn Wire) -> bool {
    let reply = match parse_client(text) {
        Err(reason) => refusal(&reason),
        Ok(ClientFrame::Take) => match desk.take() {
            Ok(policy) => json!({"t": "taken", "session": session, "policy": policy}),
            Err(reason) => refusal(&reason),
        },
        Ok(ClientFrame::Input(action)) => {
            if !desk.holds() {
                refusal("take the session first: the agent has the screen until you take it")
            } else {
                match desk.input(&action) {
                    Ok(()) => return true,
                    Err(reason) => refusal(&reason),
                }
            }
        }
        Ok(ClientFrame::GiveBack) => match desk.give_back() {
            Ok(()) => json!({"t": "given_back"}),
            Err(reason) => refusal(&reason),
        },
        Ok(ClientFrame::Ping) => json!({"t": "pong"}),
    };
    send_text(socket, &reply)
}

fn refusal(reason: &str) -> Value {
    json!({"t": "error", "error": reason})
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::mpsc;

    use apz_station::Button;

    use crate::stations::screen::script::Script;

    use super::*;

    /// The desk, recorded: whether the socket holds the session, and what it was asked.
    #[derive(Default)]
    struct FakeDesk {
        holding: Cell<bool>,
        log: RefCell<Vec<String>>,
        refuse_input: Option<String>,
    }

    impl Desk for FakeDesk {
        fn take(&self) -> Result<Takeover, String> {
            self.log.borrow_mut().push("take".into());
            self.holding.set(true);
            Ok(Takeover::AgentWaits)
        }

        fn give_back(&self) -> Result<(), String> {
            self.log.borrow_mut().push("give_back".into());
            if !self.holding.get() {
                return Err("this view does not hold the session".into());
            }
            self.holding.set(false);
            Ok(())
        }

        fn holds(&self) -> bool {
            self.holding.get()
        }

        fn input(&self, action: &InputAction) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("input {}", serde_json::to_value(action).unwrap()));
            match &self.refuse_input {
                Some(reason) => Err(reason.clone()),
                None => Ok(()),
            }
        }
    }

    fn text(value: Value) -> Message {
        Message::Text(value.to_string())
    }

    #[test]
    fn the_client_frames_parse_and_a_bad_one_is_named() {
        assert_eq!(parse_client(r#"{"t":"take"}"#), Ok(ClientFrame::Take));
        assert_eq!(
            parse_client(r#"{"t":"give_back"}"#),
            Ok(ClientFrame::GiveBack)
        );
        assert_eq!(parse_client(r#"{"t":"ping"}"#), Ok(ClientFrame::Ping));
        assert_eq!(
            parse_client(
                r#"{"t":"input","action":{"action":"click","x":10,"y":20,"button":"left","count":2}}"#
            ),
            Ok(ClientFrame::Input(InputAction::Click {
                x: 10,
                y: 20,
                button: Button::Left,
                count: 2
            }))
        );
        assert!(parse_client("not json").unwrap_err().contains("not JSON"));
        assert!(parse_client(r#"{"x":1}"#).unwrap_err().contains("no \"t\""));
        assert!(
            parse_client(r#"{"t":"input"}"#)
                .unwrap_err()
                .contains("no \"action\"")
        );
        assert!(
            parse_client(r#"{"t":"input","action":{"action":"smash"}}"#)
                .unwrap_err()
                .contains("not an input")
        );
        assert!(
            parse_client(r#"{"t":"fly"}"#)
                .unwrap_err()
                .contains("no frame named")
        );
    }

    #[test]
    fn input_is_refused_until_the_session_is_taken_and_the_socket_gives_it_back_when_it_closes() {
        let desk = FakeDesk::default();
        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket.inbound.push_back(text(
            json!({"t": "input", "action": {"action": "type", "text": "hi"}}),
        ));
        socket.inbound.push_back(text(json!({"t": "take"})));
        socket.inbound.push_back(text(
            json!({"t": "input", "action": {"action": "type", "text": "hi"}}),
        ));
        socket.inbound.push_back(text(json!({"t": "ping"})));
        serve(&desk, 9, &rx, &mut socket);

        let texts = socket.texts();
        assert_eq!(
            texts[0],
            json!({"t": "error", "error": "take the session first: the agent has the screen until you take it"})
        );
        assert_eq!(
            texts[1],
            json!({"t": "taken", "session": 9, "policy": "agent_waits"})
        );
        assert_eq!(
            texts[2],
            json!({"t": "pong"}),
            "a successful input is not answered"
        );
        assert_eq!(texts.len(), 3);
        assert_eq!(
            desk.log
                .borrow()
                .iter()
                .filter(|l| l.starts_with("input"))
                .count(),
            1,
            "only the held input reached the station"
        );
        assert_eq!(
            desk.log.borrow().last().map(String::as_str),
            Some("give_back"),
            "closing the socket gives back what it holds"
        );
        assert!(!desk.holds());
    }

    #[test]
    fn a_refused_input_is_an_error_and_the_socket_stays_open() {
        let desk = FakeDesk {
            refuse_input: Some("frozen by STOP ALL".into()),
            ..FakeDesk::default()
        };
        desk.holding.set(true);
        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket.inbound.push_back(text(
            json!({"t": "input", "action": {"action": "key", "combo": "ctrl+s"}}),
        ));
        serve(&desk, 9, &rx, &mut socket);
        assert_eq!(
            socket.texts(),
            vec![json!({"t": "error", "error": "frozen by STOP ALL"})]
        );
        assert_eq!(socket.closed, None, "the socket was not closed for it");
    }

    #[test]
    fn a_give_back_is_answered_and_a_second_one_is_an_error() {
        let desk = FakeDesk::default();
        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket.inbound.push_back(text(json!({"t": "take"})));
        socket.inbound.push_back(text(json!({"t": "give_back"})));
        socket.inbound.push_back(text(json!({"t": "give_back"})));
        serve(&desk, 9, &rx, &mut socket);
        let texts = socket.texts();
        assert_eq!(texts[1], json!({"t": "given_back"}));
        assert_eq!(texts[2]["t"], "error");
    }

    #[test]
    fn a_session_that_ends_says_so_and_the_socket_closes() {
        let desk = FakeDesk::default();
        let (tx, rx) = mpsc::channel::<Event>();
        tx.send(Event::SessionEnded {
            session: 9,
            reason: "owner_takeover".into(),
        })
        .unwrap();
        let mut socket = Script::default();
        serve(&desk, 9, &rx, &mut socket);
        assert!(
            socket
                .texts()
                .contains(&json!({"t": "ended", "reason": "owner_takeover"}))
        );
        assert_eq!(socket.closed, Some((1000, "the session ended".to_string())));
    }

    #[test]
    fn the_control_socket_asks_for_its_subprotocol_and_refuses_other_origins() {
        use crate::stations::screen::script::upgrade_request;
        let path = "/v1/stations/w1/control";
        let upgrade = |origin, protocol| {
            ws::accept(
                &upgrade_request(origin, protocol, path),
                Origins::SameHost,
                Some(PROTOCOL),
                LIMITS,
                |_| {},
            )
            .status
        };
        assert_eq!(upgrade(Some("http://127.0.0.1:7700"), PROTOCOL), 101);
        assert_eq!(upgrade(Some("http://evil.example"), PROTOCOL), 403);
        assert_eq!(
            upgrade(None, "apz.station.screen.v1"),
            400,
            "the view's subprotocol is not the control's"
        );
    }

    #[test]
    fn a_binary_frame_closes_the_socket_and_gives_back() {
        let desk = FakeDesk::default();
        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket.inbound.push_back(text(json!({"t": "take"})));
        socket.inbound.push_back(Message::Binary(vec![0]));
        serve(&desk, 9, &rx, &mut socket);
        assert_eq!(socket.closed, Some((1003, "text frames only".to_string())));
        assert_eq!(
            desk.log.borrow().last().map(String::as_str),
            Some("give_back")
        );
    }
}
