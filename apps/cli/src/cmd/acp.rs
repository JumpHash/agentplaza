//! `agentplaza acp`: the stdio agent editors start (Zed, JetBrains IDEs, Neovim, any Agent Client
//! Protocol client). Each line the editor writes is one JSON-RPC message, sent as one text frame to
//! the hub's `GET /v1/acp`; each text frame the hub sends comes back as one line on stdout. The
//! bridge speaks no protocol of its own: the hub answers every method. It logs only to stderr,
//! because stdout is the protocol.
//!
//! `agentplaza acp sessions` lists the editor sessions the hub holds (`GET /v1/acp/sessions`).

use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use apz_http::ws::{self, Limits, Message, Socket};
use serde_json::Value;

use crate::cmd::mcp::bridge::error_line;
use crate::ctx::{self, Client, Ctx};

/// The WebSocket subprotocol of `GET /v1/acp` (the hub's `apid::acp::PROTOCOL`).
const PROTOCOL: &str = "apz.acp.v1";
/// The largest line the editor may send, and the largest message the bridge takes from the hub.
const MAX_LINE: usize = 8 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the socket loop waits for a frame before it looks at the editor's queue again.
const TICK: Duration = Duration::from_millis(50);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Appended to why the hub cannot be reached, in the error the editor's first request gets.
const HINT: &str = "(turn on Editors in the hub's settings, or run `agentplaza hub login`)";

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The agent the editor's sessions are made on: its name or CID (default: the orchestrator).
    #[arg(long)]
    pub agent: Option<String>,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// The editor sessions the hub holds: id, agent, title, client, updated and closed.
    Sessions,
}

/// Runs `agentplaza acp` (the stdio agent) or `agentplaza acp sessions`. The hub comes from the
/// global flags or the saved hub, as for every command.
pub fn run(
    args: Args,
    hub: Option<String>,
    token: Option<String>,
    pin: Option<String>,
    json: bool,
) -> Result<(), String> {
    match args.command {
        Some(Cmd::Sessions) => {
            let api = ctx::resolve(hub, token, pin, json)?;
            sessions(&api)
        }
        None => stdio(args.agent.as_deref(), hub, token, pin),
    }
}

fn sessions(api: &Ctx) -> Result<(), String> {
    let value = api.client.get("/v1/acp/sessions")?;
    api.out(&value, |value| {
        let rows: Vec<Vec<String>> = listed(value)
            .iter()
            .map(|session| {
                let closed = session
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                vec![
                    ctx::text(session, "id"),
                    ctx::text(session, "cid"),
                    ctx::text(session, "title"),
                    ctx::text(session, "client"),
                    ctx::ago(session.get("updated_ms").unwrap_or(&Value::Null)),
                    if closed { "yes" } else { "" }.to_string(),
                ]
            })
            .collect();
        ctx::table(
            &["id", "agent", "title", "client", "updated", "closed"],
            &rows,
        );
    });
    Ok(())
}

/// The session list, bare or under `sessions`.
fn listed(value: &Value) -> &[Value] {
    value
        .as_array()
        .or_else(|| value.get("sessions").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// One socket's run ended.
enum Ended {
    /// The editor closed its input; the socket was closed with 1000.
    EditorDone,
    /// The socket ended, and why.
    Lost(String),
}

fn stdio(
    agent: Option<&str>,
    hub: Option<String>,
    token: Option<String>,
    pin: Option<String>,
) -> Result<(), String> {
    let path = socket_path(agent);
    let client = match ctx::resolve(hub, token, pin, false) {
        Ok(api) => api.client,
        Err(reason) => return refuse(&reason, io::stdin().lock(), io::stdout().lock()),
    };
    let mut socket = match connect(&client, &path) {
        Ok(socket) => socket,
        Err(reason) => return refuse(&reason, io::stdin().lock(), io::stdout().lock()),
    };

    // The reader thread owns stdin. It ends when the editor closes its input, which drops the
    // sender: the socket loop sees the channel disconnect once the queue is empty.
    let (tx, rx) = mpsc::channel::<String>();
    let finished = Arc::new(AtomicBool::new(false));
    let reader_finished = Arc::clone(&finished);
    thread::spawn(move || {
        let result = forward_lines(
            io::stdin().lock(),
            MAX_LINE,
            |line| tx.send(line.to_string()).is_ok(),
            report_dropped,
        );
        if let Err(err) = result {
            eprintln!("agentplaza acp: reading the editor's input failed: {err}");
        }
        reader_finished.store(true, Ordering::SeqCst);
    });

    let mut out = io::stdout().lock();
    loop {
        match pump(&mut socket, &rx, &mut out)? {
            Ended::EditorDone => return Ok(()),
            Ended::Lost(reason) => {
                if finished.load(Ordering::SeqCst) {
                    return Ok(());
                }
                // One reconnect per loss: the editor's next request reaches the new socket. A
                // failed attempt is the second failure, and ends the bridge.
                eprintln!(
                    "agentplaza acp: the hub connection ended ({reason}); reconnecting in 1 s"
                );
                thread::sleep(RECONNECT_DELAY);
                socket =
                    connect(&client, &path).map_err(|e| format!("reconnecting failed: {e}"))?;
            }
        }
    }
}

/// Sends queued editor lines and writes each hub message as a line, until the editor is done or
/// the socket ends. Stdout is written and flushed per message.
fn pump(socket: &mut Socket, rx: &Receiver<String>, out: &mut impl Write) -> Result<Ended, String> {
    loop {
        loop {
            match rx.try_recv() {
                Ok(line) => {
                    if let Err(err) = socket.send(&Message::Text(line)) {
                        return Ok(Ended::Lost(err.to_string()));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    socket.close(1000, "the editor closed its input");
                    return Ok(Ended::EditorDone);
                }
            }
        }
        match socket.recv(TICK) {
            Ok(Some(Message::Text(text))) => {
                write_line(out, &text).map_err(|e| format!("writing to the editor failed: {e}"))?;
            }
            Ok(Some(Message::Binary(_))) | Ok(None) => {}
            Err(err) => return Ok(Ended::Lost(err.to_string())),
        }
    }
}

/// The hub cannot be reached: the editor's first request is answered with an error that says why,
/// so the editor shows the reason. Always returns the reason as the error, so the bridge exits 1.
pub fn refuse(reason: &str, input: impl BufRead, mut out: impl Write) -> Result<(), String> {
    let mut answer = None;
    forward_lines(
        input,
        MAX_LINE,
        |line| {
            answer = unreachable_answer(line, reason);
            answer.is_none()
        },
        report_dropped,
    )
    .map_err(|e| format!("reading the editor's input failed: {e}"))?;
    if let Some(answer) = answer {
        write_line(&mut out, &answer).map_err(|e| format!("writing to the editor failed: {e}"))?;
    }
    Err(reason.to_string())
}

/// The answer to an editor request the bridge cannot forward: a JSON-RPC error with the request's
/// id. None for a notification or a line that is not a JSON request, which owe no answer.
pub fn unreachable_answer(line: &str, reason: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    let id = message.get("id")?;
    if message.get("method").is_none() {
        return None;
    }
    Some(error_line(id, -32000, &format!("{reason} {HINT}")))
}

/// Feeds each line of `input` to `send` in order, without its line break (a `\r\n` too) and
/// without blank lines, until the input ends or `send` returns false. A line over `max` bytes is
/// dropped whole and reported to `dropped` with its length; its bytes are read and discarded, so
/// memory stays near `max` whatever the editor writes. A line that is not UTF-8 is dropped too.
pub fn forward_lines<R: BufRead>(
    mut input: R,
    max: usize,
    mut send: impl FnMut(&str) -> bool,
    mut dropped: impl FnMut(usize),
) -> io::Result<()> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        line.clear();
        // One byte past the cap tells a line that is too long from one that just fits.
        let read = input
            .by_ref()
            .take(max as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let complete = line.last() == Some(&b'\n');
        if complete {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        } else if line.len() > max {
            let rest = discard_rest_of_line(&mut input)?;
            dropped(line.len() + rest);
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            dropped(line.len());
            continue;
        };
        if !send(text) {
            return Ok(());
        }
    }
}

/// Reads and drops bytes up to the next line break (not counted) or the end; returns how many
/// bytes were dropped before the break.
fn discard_rest_of_line<R: BufRead>(input: &mut R) -> io::Result<usize> {
    let mut dropped = 0;
    loop {
        let buf = input.fill_buf()?;
        if buf.is_empty() {
            return Ok(dropped);
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(at) => {
                input.consume(at + 1);
                return Ok(dropped + at);
            }
            None => {
                let len = buf.len();
                input.consume(len);
                dropped += len;
            }
        }
    }
}

/// Writes one message as one line and flushes, so the editor reads it at once.
pub fn write_line(out: &mut impl Write, text: &str) -> io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn report_dropped(size: usize) {
    eprintln!(
        "agentplaza acp: dropped an editor line of {size} bytes (the limit is {MAX_LINE} bytes)"
    );
}

/// `/v1/acp`, with `?agent=` when the editor named an agent.
fn socket_path(agent: Option<&str>) -> String {
    match agent {
        Some(agent) => format!("/v1/acp?agent={}", encode_query(agent)),
        None => "/v1/acp".to_string(),
    }
}

/// Percent-encodes a query value: unreserved characters stay, every other byte becomes `%XX`.
pub fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn connect(client: &Client, path: &str) -> Result<Socket, String> {
    let limits = Limits {
        max_frame: MAX_LINE,
        max_message: MAX_LINE,
        ..Limits::default()
    };
    ws::connect(
        &client.target,
        path,
        Some(PROTOCOL),
        CONNECT_TIMEOUT,
        limits,
    )
    .map_err(|e| format!("{}: {e}", client.base()))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// The lines `forward_lines` sends and the lengths it drops, for `input` read with `max`.
    fn split(input: &[u8], max: usize) -> (Vec<String>, Vec<usize>) {
        let mut sent = Vec::new();
        let mut dropped = Vec::new();
        forward_lines(
            input,
            max,
            |line| {
                sent.push(line.to_string());
                true
            },
            |size| dropped.push(size),
        )
        .unwrap();
        (sent, dropped)
    }

    #[test]
    fn lines_go_in_order_without_their_breaks_and_blank_lines_are_skipped() {
        let (sent, dropped) = split(b"{\"a\":1}\r\n\n  \n{\"b\":2}\nlast", MAX_LINE);
        assert_eq!(sent, ["{\"a\":1}", "{\"b\":2}", "last"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_line_over_the_cap_is_dropped_whole_and_the_next_line_still_goes() {
        let (sent, dropped) = split(b"0123456789ABCDEF\nok\n", 8);
        assert_eq!(sent, ["ok"]);
        assert_eq!(dropped, [16]);
    }

    #[test]
    fn a_line_of_exactly_the_cap_goes_through() {
        let (sent, dropped) = split(b"abcd\nef\n", 4);
        assert_eq!(sent, ["abcd", "ef"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_line_that_is_not_utf8_is_dropped() {
        let (sent, dropped) = split(&[0xff, b'\n', b'o', b'k', b'\n'], MAX_LINE);
        assert_eq!(sent, ["ok"]);
        assert_eq!(dropped, [1]);
    }

    #[test]
    fn forwarding_stops_when_send_says_no() {
        let mut sent = Vec::new();
        forward_lines(
            &b"a\nb\n"[..],
            MAX_LINE,
            |line| {
                sent.push(line.to_string());
                false
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(sent, ["a"]);
    }

    #[test]
    fn write_line_is_one_line_with_a_break() {
        let mut out = Vec::new();
        write_line(&mut out, "{\"id\":1}").unwrap();
        assert_eq!(out, b"{\"id\":1}\n");
    }

    #[test]
    fn an_initialize_request_gets_the_error_with_its_id() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#;
        let answer = unreachable_answer(line, "no saved hub").expect("a request is answered");
        let value: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["code"], -32000);
        assert_eq!(
            value["error"]["message"],
            "no saved hub (turn on Editors in the hub's settings, or run `agentplaza hub login`)"
        );
    }

    #[test]
    fn notifications_and_broken_lines_owe_no_answer() {
        let note = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#;
        assert_eq!(unreachable_answer(note, "x"), None);
        assert_eq!(unreachable_answer("{not json", "x"), None);
    }

    #[test]
    fn refuse_answers_the_first_request_and_fails_with_the_reason() {
        let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"initialize\"}\n";
        let mut out = Vec::new();
        let err = refuse("no saved hub", &input[..], &mut out).unwrap_err();
        assert_eq!(err, "no saved hub");
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.lines().count(),
            1,
            "only the request is answered: {text}"
        );
        let value: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["id"], 7);
    }

    #[test]
    fn the_agent_name_is_percent_encoded_in_the_path() {
        assert_eq!(encode_query("builder-1"), "builder-1");
        assert_eq!(encode_query("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(socket_path(Some("builder 1")), "/v1/acp?agent=builder%201");
        assert_eq!(socket_path(None), "/v1/acp");
    }

    #[test]
    fn acp_takes_an_agent_name() {
        let cli = crate::Cli::try_parse_from(["agentplaza", "acp", "--agent", "builder-1"])
            .expect("parses");
        match cli.command {
            crate::Command::Acp(args) => {
                assert_eq!(args.agent.as_deref(), Some("builder-1"));
                assert!(args.command.is_none());
            }
            _ => panic!("not the acp command"),
        }
    }

    #[test]
    fn acp_sessions_is_a_subcommand() {
        let cli = crate::Cli::try_parse_from(["agentplaza", "acp", "sessions"]).expect("parses");
        match cli.command {
            crate::Command::Acp(args) => assert!(matches!(args.command, Some(Cmd::Sessions))),
            _ => panic!("not the acp command"),
        }
    }
}
