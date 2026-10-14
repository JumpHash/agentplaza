//! `agentplaza stations take` and `agentplaza stations input`: the owner takes a workstation's session from the terminal
//! (`WS /v1/stations/{id}/control?session=`, subprotocol `apz.station.control.v1`, docs/design/workstations.md, section 7;
//! the hub's side is `crates/apid/src/stations/control.rs`). While the socket holds the session the owner's input goes
//! to the station; closing the socket gives it back.
//!
//! The commands, one a line on stdin (`take`) or as the words after the station (`input`):
//! - `click X Y [left|right|middle] [double]`
//! - `move X Y`
//! - `type TEXT...` (the rest of the line, as typed)
//! - `key CHORD` (`ctrl+s`, `ctrl+shift+s`)
//! - `scroll DX DY [X Y]`: a positive DY scrolls the content down. Without X Y the pointer's last position is used.
//! - `drag X1 Y1 X2 Y2`
//! - `give-back`
//!
//! Coordinates are in the pixels of the station's latest screenshot, as the hub's input takes them.
//!
//! The same [`Desk`] and [`drive`] serve the terminal's `take` and the TUI's takeover panel, so the two cannot drift.

use std::io::BufRead;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use apz_station::{Button, InputAction, Takeover};
use serde_json::{Value, json};

use super::screen;
use crate::ctx::{self, Ctx, seg};

/// The subprotocol a takeover socket asks for.
pub const PROTOCOL: &str = "apz.station.control.v1";
/// The commands, as a help line names them.
pub const COMMANDS: &str = "click, move, type, key, scroll, drag, give-back";
/// How long the hub has to answer a take, a give-back or an input.
const ANSWER: Duration = Duration::from_secs(20);
/// How long one wait for a hub frame lasts; the socket is polled again.
const POLL: Duration = Duration::from_millis(100);

/// A frame the hub sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hub {
    Taken {
        session: u64,
        policy: String,
    },
    GivenBack,
    /// The frame was refused (the socket stays open).
    Error(String),
    Pong,
    /// The session ended; the socket closes after it.
    Ended(String),
    Other,
}

/// A text frame as the hub's frames read.
pub fn parse_hub(text: &str) -> Hub {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Hub::Other;
    };
    match value.get("t").and_then(Value::as_str) {
        Some("taken") => Hub::Taken {
            session: value["session"].as_u64().unwrap_or(0),
            policy: ctx::text(&value, "policy"),
        },
        Some("given_back") => Hub::GivenBack,
        Some("error") => Hub::Error(ctx::text(&value, "error")),
        Some("pong") => Hub::Pong,
        Some("ended") => Hub::Ended(ctx::text(&value, "reason")),
        _ => Hub::Other,
    }
}

pub fn take_frame() -> Value {
    json!({"t": "take"})
}

pub fn give_back_frame() -> Value {
    json!({"t": "give_back"})
}

pub fn ping_frame() -> Value {
    json!({"t": "ping"})
}

pub fn input_frame(action: &InputAction) -> Value {
    json!({"t": "input", "action": serde_json::to_value(action).unwrap_or(Value::Null)})
}

/// The socket's path: the station and the session (a number, or `shared`).
pub fn view_path(id: &str, session: &str) -> String {
    format!("/v1/stations/{}/control?session={}", seg(id), seg(session))
}

/// What a takeover's policy does to the agent, in words.
pub fn policy_text(word: &str) -> String {
    match serde_json::from_value::<Takeover>(Value::String(word.to_string())) {
        Ok(Takeover::AgentWaits) => "the agent waits until you give the session back".into(),
        Ok(Takeover::ReleaseLease) => {
            "the agent's lease is released and its task goes back to the queue".into()
        }
        Ok(Takeover::KeepWorkingPointerPaused) => {
            "the agent keeps working; only its pointer pauses".into()
        }
        Err(_) => format!("policy {word}"),
    }
}

/// One line the owner typed, read: an input for the station, or the give-back.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Act(InputAction),
    GiveBack,
}

/// A line of the commands, read against the pointer's last position (for a scroll without X Y). Blank lines are the
/// caller's to skip. The verb is the first word; `type` takes the rest of the line exactly as typed.
pub fn parse_line(line: &str, pointer: Option<(i32, i32)>) -> Result<Step, String> {
    let line = line.trim_end_matches(['\r', '\n']);
    let start = line.trim_start();
    let (verb, rest) = start.split_once(char::is_whitespace).unwrap_or((start, ""));
    let words: Vec<&str> = rest.split_whitespace().collect();
    match verb {
        "" => Err(format!("no command given: the commands are {COMMANDS}")),
        "click" => click(&words),
        "move" => match words.as_slice() {
            [x, y] => Ok(Step::Act(InputAction::Move {
                x: int(x, "X")?,
                y: int(y, "Y")?,
            })),
            _ => Err("move takes X Y".into()),
        },
        "type" => {
            if rest.trim().is_empty() {
                Err("type takes the text to type: type TEXT".into())
            } else {
                Ok(Step::Act(InputAction::Type {
                    text: rest.to_string(),
                }))
            }
        }
        "key" => match words.as_slice() {
            [chord] if chord.split('+').all(|part| !part.is_empty()) => {
                Ok(Step::Act(InputAction::Key {
                    combo: chord.to_string(),
                }))
            }
            _ => Err("key takes one chord, such as ctrl+s".into()),
        },
        "scroll" => scroll(&words, pointer),
        "drag" => match words.as_slice() {
            [x1, y1, x2, y2] => Ok(Step::Act(InputAction::Drag {
                from: (int(x1, "X1")?, int(y1, "Y1")?),
                to: (int(x2, "X2")?, int(y2, "Y2")?),
            })),
            _ => Err("drag takes X1 Y1 X2 Y2".into()),
        },
        "give-back" | "give_back" => {
            if words.is_empty() {
                Ok(Step::GiveBack)
            } else {
                Err("give-back takes no words".into())
            }
        }
        other => Err(format!(
            "no command named {other:?}; the commands are {COMMANDS}"
        )),
    }
}

/// `click X Y`, then any of left, right, middle (one) and double (once), in any order.
fn click(words: &[&str]) -> Result<Step, String> {
    let [x, y, flags @ ..] = words else {
        return Err("click takes X Y, then left, right, middle or double".into());
    };
    let (x, y) = (int(x, "X")?, int(y, "Y")?);
    let mut button = None;
    let mut double = false;
    for flag in flags {
        match *flag {
            "left" | "right" | "middle" => {
                if button.replace(button_of(flag)).is_some() {
                    return Err("click takes one button".into());
                }
            }
            "double" => {
                if std::mem::replace(&mut double, true) {
                    return Err("click takes double once".into());
                }
            }
            other => {
                return Err(format!(
                    "click takes left, right, middle or double after X Y, not {other:?}"
                ));
            }
        }
    }
    Ok(Step::Act(InputAction::Click {
        x,
        y,
        button: button.unwrap_or(Button::Left),
        count: if double { 2 } else { 1 },
    }))
}

/// `scroll DX DY`, or `scroll DX DY X Y`. Without the position the pointer's last one is used.
fn scroll(words: &[&str], pointer: Option<(i32, i32)>) -> Result<Step, String> {
    let [dx, dy, at @ ..] = words else {
        return Err("scroll takes DX DY, then X Y (a positive DY scrolls down)".into());
    };
    let (dx, dy) = (int(dx, "DX")?, int(dy, "DY")?);
    let (x, y) = match at {
        [] => pointer
            .ok_or("scroll needs a position: give X Y after DY, or move the pointer first")?,
        [x, y] => (int(x, "X")?, int(y, "Y")?),
        _ => return Err("scroll takes DX DY, then X Y".into()),
    };
    Ok(Step::Act(InputAction::Scroll { x, y, dx, dy }))
}

fn int(word: &str, what: &str) -> Result<i32, String> {
    word.parse::<i32>()
        .map_err(|_| format!("{what} must be a whole number, not {word:?}"))
}

fn button_of(word: &str) -> Button {
    match word {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    }
}

fn button_word(button: Button) -> &'static str {
    match button {
        Button::Left => "left",
        Button::Right => "right",
        Button::Middle => "middle",
    }
}

/// The pointer's position after an action: where it moved to, if the action moves it.
pub fn resting_point(action: &InputAction) -> Option<(i32, i32)> {
    match action {
        InputAction::Move { x, y }
        | InputAction::Click { x, y, .. }
        | InputAction::Scroll { x, y, .. } => Some((*x, *y)),
        InputAction::Drag { to, .. } => Some(*to),
        InputAction::Down { .. }
        | InputAction::Up { .. }
        | InputAction::Type { .. }
        | InputAction::Key { .. } => None,
    }
}

/// An action as the owner reads it back. The text typed is counted, not printed.
pub fn describe(action: &InputAction) -> String {
    match action {
        InputAction::Move { x, y } => format!("move {x} {y}"),
        InputAction::Click {
            x,
            y,
            button,
            count,
        } => {
            let verb = if *count == 2 { "double-click" } else { "click" };
            format!("{verb} {x} {y} {}", button_word(*button))
        }
        InputAction::Down { button } => format!("button down {}", button_word(*button)),
        InputAction::Up { button } => format!("button up {}", button_word(*button)),
        InputAction::Scroll { x, y, dx, dy } => format!("scroll {dx} {dy} at {x} {y}"),
        InputAction::Type { text } => format!("type {} characters", text.chars().count()),
        InputAction::Key { combo } => format!("key {combo}"),
        InputAction::Drag { from, to } => format!("drag {} {} {} {}", from.0, from.1, to.0, to.1),
    }
}

/// The owner's hold on one session: its socket, the session number and the policy in force.
pub struct Desk {
    socket: Socket,
    pub session: u64,
    pub policy: String,
    pointer: Option<(i32, i32)>,
    /// Why the session is over (the hub ended it, or the link did): nothing more can be sent.
    ended: Option<String>,
}

impl Desk {
    /// Opens the socket and takes the session under the station's policy. A refusal is the error.
    pub fn take(target: &Target, id: &str, session: &str) -> Result<Desk, String> {
        let path = view_path(id, session);
        let socket = ws::connect(
            target,
            &path,
            Some(PROTOCOL),
            Duration::from_secs(10),
            Limits::default(),
        )
        .map_err(|e| format!("{} {path}: {e}", target.base))?;
        let mut desk = Desk {
            socket,
            session: 0,
            policy: String::new(),
            pointer: None,
            ended: None,
        };
        desk.send(&take_frame())?;
        let deadline = Instant::now() + ANSWER;
        while Instant::now() < deadline {
            match desk.receive(POLL)? {
                Some(Hub::Taken { session, policy }) => {
                    desk.session = session;
                    desk.policy = policy;
                    return Ok(desk);
                }
                Some(Hub::Error(reason)) => return Err(reason),
                Some(Hub::Ended(reason)) => {
                    return Err(format!("the session ended: {}", reason_text(&reason)));
                }
                _ => {}
            }
        }
        Err("the hub did not answer the take in time".into())
    }

    /// The pointer's last position on the station, if an action has put it anywhere.
    pub fn pointer(&self) -> Option<(i32, i32)> {
        self.pointer
    }

    /// Why the session is over, once it is.
    pub fn ended(&self) -> Option<&str> {
        self.ended.as_deref()
    }

    /// The next frame the hub sends, within `wait`. The session's end and a broken link are recorded here.
    pub fn receive(&mut self, wait: Duration) -> Result<Option<Hub>, String> {
        let message = match self.socket.recv(wait) {
            Ok(message) => message,
            Err(e) => {
                let reason = format!("the takeover link ended: {e}");
                self.ended = Some(reason.clone());
                return Err(reason);
            }
        };
        let hub = match message {
            None => return Ok(None),
            Some(Message::Text(text)) => parse_hub(&text),
            Some(Message::Binary(_)) => Hub::Other,
        };
        if let Hub::Ended(reason) = &hub {
            self.ended = Some(format!("the session ended: {}", reason_text(reason)));
        }
        Ok(Some(hub))
    }

    /// One action on the station: sent, then answered by a ping's pong, so a refusal of this action is heard here.
    pub fn act(&mut self, action: &InputAction) -> Result<(), String> {
        self.round_trip(input_frame(action))?;
        if let Some(point) = resting_point(action) {
            self.pointer = Some(point);
        }
        Ok(())
    }

    /// Gives the session back. Nothing to do when it already ended.
    pub fn give_back(&mut self) -> Result<(), String> {
        if self.ended.is_some() {
            return Ok(());
        }
        self.send(&give_back_frame())?;
        let deadline = Instant::now() + ANSWER;
        while Instant::now() < deadline {
            match self.receive(POLL)? {
                Some(Hub::GivenBack) => return Ok(()),
                Some(Hub::Error(reason)) => return Err(reason),
                Some(Hub::Ended(_)) => return Ok(()),
                _ => {}
            }
        }
        Err("the hub did not answer the give-back in time".into())
    }

    /// Closes the socket. The hub gives back what the socket holds.
    pub fn close(mut self) {
        self.socket.close(1000, "done");
    }

    fn send(&mut self, frame: &Value) -> Result<(), String> {
        self.socket
            .send(&Message::Text(frame.to_string()))
            .map_err(|e| format!("the takeover link ended: {e}"))
    }

    /// Sends a frame and a ping, then reads until the pong: an error the hub sent for the frame is returned.
    fn round_trip(&mut self, frame: Value) -> Result<(), String> {
        if let Some(reason) = &self.ended {
            return Err(reason.clone());
        }
        self.send(&frame)?;
        self.send(&ping_frame())?;
        let deadline = Instant::now() + ANSWER;
        let mut refused = None;
        while Instant::now() < deadline {
            match self.receive(POLL)? {
                Some(Hub::Pong) => return refused.map_or(Ok(()), Err),
                Some(Hub::Error(reason)) => refused = Some(reason),
                Some(Hub::GivenBack) => return Err("the session was given back".into()),
                Some(Hub::Ended(_)) => return Err(self.ended.clone().unwrap_or_default()),
                _ => {}
            }
        }
        Err("the hub did not answer in time".into())
    }
}

fn reason_text(reason: &str) -> &str {
    if reason.is_empty() {
        "no reason given"
    } else {
        reason
    }
}

/// An order for the driver: a line of the commands, or the end of the owner's input.
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    Line(String),
    GiveBack,
}

/// What the driver tells its owner: a line to show, a refused command, or that the takeover is over.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Line(String),
    Refused(String),
    Ended,
}

enum Outcome {
    Continue,
    GiveBack,
    Refused,
}

/// Runs one command on the desk and reports what happened.
fn command(desk: &mut Desk, line: &str, say: &dyn Fn(Report)) -> Outcome {
    if line.trim().is_empty() {
        return Outcome::Continue;
    }
    let step = match parse_line(line, desk.pointer()) {
        Ok(step) => step,
        Err(reason) => {
            say(Report::Refused(reason));
            return Outcome::Refused;
        }
    };
    match step {
        Step::GiveBack => Outcome::GiveBack,
        Step::Act(action) => match desk.act(&action) {
            Ok(()) => {
                say(Report::Line(format!("done: {}", describe(&action))));
                Outcome::Continue
            }
            Err(reason) => {
                say(Report::Refused(reason));
                Outcome::Refused
            }
        },
    }
}

/// Does the orders on the desk until the owner gives the session back, the orders end, or the session ends; then gives
/// it back if it still holds it. Returns how many commands were refused. Everything that happens is reported.
pub fn drive(desk: &mut Desk, orders: &Receiver<Order>, say: &dyn Fn(Report)) -> usize {
    let mut refused = 0;
    loop {
        let order = match orders.try_recv() {
            Ok(order) => Some(order),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Order::GiveBack),
        };
        match order {
            Some(Order::GiveBack) => break,
            Some(Order::Line(line)) => match command(desk, &line, say) {
                Outcome::Continue => {}
                Outcome::GiveBack => break,
                Outcome::Refused => refused += 1,
            },
            None => {}
        }
        if desk.ended().is_some() {
            break;
        }
        match desk.receive(POLL) {
            Ok(Some(Hub::Error(reason))) => say(Report::Line(format!("the hub: {reason}"))),
            Ok(Some(Hub::Ended(_))) => {
                say(Report::Line(desk.ended().unwrap_or_default().to_string()));
                break;
            }
            Ok(_) => {}
            Err(reason) => {
                say(Report::Line(reason));
                break;
            }
        }
    }
    if desk.ended().is_none() {
        match desk.give_back() {
            Ok(()) => say(Report::Line("given back".into())),
            Err(reason) => {
                refused += 1;
                say(Report::Refused(format!("could not give back: {reason}")));
            }
        }
    }
    refused
}

/// The takeover for the TUI's panel: takes the session, drives it from the orders, and says `Ended` last.
pub fn run_panel(
    target: &Target,
    id: &str,
    session: &str,
    orders: &Receiver<Order>,
    say: &dyn Fn(Report),
) {
    match Desk::take(target, id, session) {
        Ok(mut desk) => {
            say(Report::Line(format!(
                "took session {}: {}",
                desk.session,
                policy_text(&desk.policy)
            )));
            drive(&mut desk, orders, say);
        }
        Err(reason) => say(Report::Refused(format!("could not take: {reason}"))),
    }
    say(Report::Ended);
}

/// The owner's lines on stdin, as orders: the end of input is a give-back.
fn stdin_orders() -> Receiver<Order> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = String::new();
            let order = match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => Order::GiveBack,
                Ok(_) => Order::Line(line),
            };
            let last = order == Order::GiveBack;
            if tx.send(order).is_err() || last {
                break;
            }
        }
    });
    rx
}

fn print_report(report: Report) {
    match report {
        Report::Line(text) => println!("{text}"),
        Report::Refused(text) => eprintln!("refused: {text}"),
        Report::Ended => {}
    }
}

/// `stations take`: takes the session, prints the policy, and does the commands on stdin until give-back or the end of
/// input. Exits 1 when any command was refused.
pub fn take(ctx: &Ctx, id: &str, session: &str) -> Result<(), String> {
    screen::check_session(session)?;
    let mut desk = Desk::take(&ctx.client.target, id, session)?;
    println!(
        "took session {} on {id}: {}",
        desk.session,
        policy_text(&desk.policy)
    );
    println!(
        "commands, one a line: {COMMANDS}; give-back or the end of input gives the session back"
    );
    let orders = stdin_orders();
    let refused = drive(&mut desk, &orders, &print_report);
    desk.close();
    if refused > 0 {
        return Err(format!("{refused} command(s) refused"));
    }
    Ok(())
}

/// `stations input`: takes the session, does the one action in `line`, and gives it back.
pub fn input(ctx: &Ctx, id: &str, session: &str, line: &str) -> Result<(), String> {
    screen::check_session(session)?;
    // Refused before the take when the line is wrong, so no session is taken for nothing. The pointer is not known yet.
    let action = match parse_line(line, None)? {
        Step::Act(action) => action,
        Step::GiveBack => {
            return Err(
                "give-back ends a take; input does one action and gives the session back".into(),
            );
        }
    };
    let mut desk = Desk::take(&ctx.client.target, id, session)?;
    let taken = desk.session;
    let acted = desk.act(&action);
    let given = desk.give_back();
    desk.close();
    acted?;
    given?;
    let done = json!({"station": id, "session": taken, "action": serde_json::to_value(&action).unwrap_or(Value::Null)});
    ctx.out(&done, |_| println!("done on {id}: {}", describe(&action)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(line: &str) -> InputAction {
        match parse_line(line, None) {
            Ok(Step::Act(action)) => action,
            other => panic!("{line:?} should be an input: {other:?}"),
        }
    }

    fn refused(line: &str) -> String {
        parse_line(line, None).expect_err(line)
    }

    #[test]
    fn click_takes_a_position_then_a_button_and_double_in_any_order() {
        assert_eq!(
            act("click 10 20"),
            InputAction::Click {
                x: 10,
                y: 20,
                button: Button::Left,
                count: 1
            }
        );
        assert_eq!(
            act("click 10 20 right"),
            InputAction::Click {
                x: 10,
                y: 20,
                button: Button::Right,
                count: 1
            }
        );
        assert_eq!(
            act("click 10 20 double middle"),
            InputAction::Click {
                x: 10,
                y: 20,
                button: Button::Middle,
                count: 2
            }
        );
        assert_eq!(
            act("click 10 20 double"),
            InputAction::Click {
                x: 10,
                y: 20,
                button: Button::Left,
                count: 2
            }
        );
        assert!(refused("click 10").contains("click takes X Y"));
        assert!(refused("click 10 20 left right").contains("one button"));
        assert!(refused("click 10 20 double double").contains("double once"));
        assert!(refused("click 10 20 triple").contains("not \"triple\""));
        assert!(refused("click ten 20").contains("X must be a whole number"));
    }

    #[test]
    fn move_and_drag_take_their_coordinates() {
        assert_eq!(act("move 5 -4"), InputAction::Move { x: 5, y: -4 });
        assert!(refused("move 5").contains("move takes X Y"));
        assert_eq!(
            act("drag 5 6 50 60"),
            InputAction::Drag {
                from: (5, 6),
                to: (50, 60)
            }
        );
        assert!(refused("drag 5 6 50").contains("drag takes X1 Y1 X2 Y2"));
    }

    #[test]
    fn type_keeps_the_text_as_typed_to_the_end_of_the_line() {
        assert_eq!(
            act("type hello world"),
            InputAction::Type {
                text: "hello world".into()
            }
        );
        assert_eq!(
            act("type  two  spaces "),
            InputAction::Type {
                text: " two  spaces ".into()
            }
        );
        assert_eq!(
            act("type héllo\r\n"),
            InputAction::Type {
                text: "héllo".into()
            }
        );
        assert!(refused("type").contains("takes the text"));
    }

    #[test]
    fn key_takes_one_chord() {
        assert_eq!(
            act("key ctrl+s"),
            InputAction::Key {
                combo: "ctrl+s".into()
            }
        );
        assert_eq!(
            act("key ctrl+shift+s"),
            InputAction::Key {
                combo: "ctrl+shift+s".into()
            }
        );
        assert!(refused("key").contains("one chord"));
        assert!(refused("key ctrl+ s").contains("one chord"));
        assert!(refused("key ctrl++s").contains("one chord"));
    }

    #[test]
    fn scroll_keeps_the_sign_of_dy_positive_scrolls_down() {
        assert_eq!(
            act("scroll 0 3 100 200"),
            InputAction::Scroll {
                x: 100,
                y: 200,
                dx: 0,
                dy: 3
            }
        );
        assert_eq!(
            act("scroll 0 -3 100 200"),
            InputAction::Scroll {
                x: 100,
                y: 200,
                dx: 0,
                dy: -3
            }
        );
        assert_eq!(
            act("scroll -2 1 7 8"),
            InputAction::Scroll {
                x: 7,
                y: 8,
                dx: -2,
                dy: 1
            }
        );
    }

    #[test]
    fn scroll_without_a_position_uses_the_pointer_or_is_refused() {
        let step = parse_line("scroll 0 3", Some((40, 50))).unwrap();
        assert_eq!(
            step,
            Step::Act(InputAction::Scroll {
                x: 40,
                y: 50,
                dx: 0,
                dy: 3
            })
        );
        let err = parse_line("scroll 0 3", None).unwrap_err();
        assert!(err.contains("needs a position"), "{err}");
        assert!(refused("scroll 0").contains("takes DX DY"));
        assert!(refused("scroll 0 1 2").contains("takes DX DY, then X Y"));
    }

    #[test]
    fn give_back_ends_and_the_verb_is_checked() {
        assert_eq!(parse_line("give-back", None), Ok(Step::GiveBack));
        assert_eq!(parse_line("give_back\n", None), Ok(Step::GiveBack));
        assert!(refused("give-back now").contains("no words"));
        assert!(refused("   ").contains("no command given"));
        assert!(refused("fly 1 2").contains("no command named \"fly\""));
    }

    #[test]
    fn the_pointer_follows_the_actions_that_move_it() {
        assert_eq!(resting_point(&act("move 1 2")), Some((1, 2)));
        assert_eq!(resting_point(&act("click 3 4")), Some((3, 4)));
        assert_eq!(resting_point(&act("drag 1 1 9 9")), Some((9, 9)));
        assert_eq!(resting_point(&act("key ctrl+s")), None);
        assert_eq!(resting_point(&act("type x")), None);
    }

    #[test]
    fn every_action_maps_to_the_hub_json_shape() {
        assert_eq!(
            input_frame(&act("click 10 20 right double")),
            json!({"t": "input", "action": {"action": "click", "x": 10, "y": 20, "button": "right", "count": 2}})
        );
        assert_eq!(
            input_frame(&act("key ctrl+s")),
            json!({"t": "input", "action": {"action": "key", "combo": "ctrl+s"}})
        );
        assert_eq!(
            input_frame(&act("drag 5 6 50 60")),
            json!({"t": "input", "action": {"action": "drag", "from": [5, 6], "to": [50, 60]}})
        );
        assert_eq!(
            input_frame(&act("scroll 0 -3 1 1")),
            json!({"t": "input", "action": {"action": "scroll", "x": 1, "y": 1, "dx": 0, "dy": -3}})
        );
    }

    #[test]
    fn the_hub_frames_read_as_hub_notes() {
        assert_eq!(
            parse_hub(r#"{"t":"taken","session":9,"policy":"agent_waits"}"#),
            Hub::Taken {
                session: 9,
                policy: "agent_waits".into()
            }
        );
        assert_eq!(parse_hub(r#"{"t":"given_back"}"#), Hub::GivenBack);
        assert_eq!(
            parse_hub(r#"{"t":"error","error":"frozen by STOP ALL"}"#),
            Hub::Error("frozen by STOP ALL".into())
        );
        assert_eq!(parse_hub(r#"{"t":"pong"}"#), Hub::Pong);
        assert_eq!(
            parse_hub(r#"{"t":"ended","reason":"owner_takeover"}"#),
            Hub::Ended("owner_takeover".into())
        );
        assert_eq!(parse_hub("garbage"), Hub::Other);
    }

    #[test]
    fn the_client_frames_and_the_path_have_the_documented_shape() {
        assert_eq!(take_frame(), json!({"t": "take"}));
        assert_eq!(give_back_frame(), json!({"t": "give_back"}));
        assert_eq!(ping_frame(), json!({"t": "ping"}));
        assert_eq!(
            view_path("w1", "shared"),
            "/v1/stations/w1/control?session=shared"
        );
        assert_eq!(view_path("w1", "7"), "/v1/stations/w1/control?session=7");
    }

    #[test]
    fn the_policy_reads_in_words() {
        assert!(policy_text("agent_waits").contains("waits until you give the session back"));
        assert!(policy_text("release_lease").contains("goes back to the queue"));
        assert!(policy_text("keep_working_pointer_paused").contains("only its pointer pauses"));
        assert_eq!(policy_text("odd"), "policy odd");
    }

    #[test]
    fn an_action_is_described_without_its_typed_text() {
        assert_eq!(describe(&act("click 1 2 double")), "double-click 1 2 left");
        assert_eq!(describe(&act("type secret")), "type 6 characters");
        assert_eq!(describe(&act("scroll 0 2 5 6")), "scroll 0 2 at 5 6");
    }
}
