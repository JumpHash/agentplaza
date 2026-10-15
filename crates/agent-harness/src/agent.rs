//! The model-driven agent loop (v3), run inside a keepalive session.
//!
//! A welcome carrying a `profile` (or a later `profile` line) makes the session
//! model-backed. `work` items queue FIFO and run one at a time. A work item is a
//! loop of model calls: the guest sends `model_call` with the conversation, the
//! host answers `model_result`, and each tool the model asks for goes through
//! `tool_request` and `tool_decision` (allowed and run here, decided by the host,
//! or denied). The conversation only grows. A chat thread keeps its conversation
//! across work items until it passes [`THREAD_CLOSE_BYTES`].
//!
//! Every wait is one loop ([`Agent::await_message`]) that keeps beating, queues
//! work and records cancels while it waits. The host's lines arrive through the
//! reader lane ([`crate::lane`]), which also answers the owner's operator
//! `tool_call`s on a lane of their own. A local tool runs on its own thread that
//! the same beat runs beside, so a slow tool does not silence the session.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Message, Role, StopReason, blocks_text};
use apz_proto::line::Lines;
use apz_proto::wire::{
    AgentProfile, Control, Decision, GuestMessage, HostMessage, MAX_TOOL_OUTPUT, WorkKind,
    WorkStatus, cap_output,
};
use serde_json::{Value, json};

use crate::acp::hub::HubLink;
use crate::bash;
use crate::files::{self, Workspace};
use crate::lane::{Duplex, Inbound, OPERATOR_EXPIRY, Shared, read_host, serve_operators};
use crate::session::{SessionError, escape_for_transcript, kind_of};
use crate::tools;
use crate::{checkpoints, engines, library, plan, search_tools, shells};

/// A chat thread whose history serializes past this is closed: its next work
/// starts a new conversation, opened with the thread's last summary.
pub const THREAD_CLOSE_BYTES: usize = 400 * 1024;
/// Consecutive identical failing tool calls that stop a work item.
const STUCK_FAILURES: u32 = 3;
/// Token-limit stops a work item continues past (with a "continue" message).
const TOKEN_LIMIT_CONTINUES: u32 = 1;
/// How often a local tool's thread is checked while the beat runs beside it.
const LOCAL_POLL: Duration = Duration::from_millis(20);

/// A local tool's result: its output, or why it failed.
type Outcome = Result<String, String>;

/// A queued work item.
struct Job {
    work: u64,
    kind: WorkKind,
    thread: String,
    prompt: String,
}

/// A chat thread's conversation, kept between its work items.
#[derive(Default)]
struct Thread {
    messages: Vec<Message>,
    /// The summary of the thread's last work item.
    summary: String,
    /// When the thread was last stored, on the agent's clock: the oldest go first.
    at: u64,
}

/// What one turn of the read loop saw.
enum Incoming {
    /// The beat came due before a line; a heartbeat went out.
    Beat,
    /// A line the loop acted on itself (queued work, cancel, profile, control).
    Handled,
    /// The host stopped the agent.
    Stop(String),
    /// A line a wait may be waiting for.
    Message(HostMessage),
}

/// Why a wait ended early: the host stopped the agent, or the session broke.
enum Halt {
    Stop(String),
    Failed(SessionError),
}

impl From<SessionError> for Halt {
    fn from(error: SessionError) -> Self {
        Halt::Failed(error)
    }
}

/// The agent's state for one keepalive connection.
struct Agent<'a, 'l, S, W> {
    shared: &'a Shared<'l, S, W>,
    /// The host's messages the reader lane passes on.
    inbox: Receiver<Inbound>,
    workspace: &'a Workspace,
    heartbeat: Duration,
    next_beat: Instant,
    seq: u64,
    /// The last model call's request id.
    req: u64,
    profile: Option<AgentProfile>,
    queue: VecDeque<Job>,
    threads: HashMap<String, Thread>,
    /// Orders the stored threads: the next value a stored thread takes.
    clock: u64,
    /// The work item in progress, if any.
    current: Option<u64>,
    /// A cancel for the work item in progress, taken at its next step.
    cancelled: Option<String>,
    /// Steering notes (from, text) for the work item in progress, added to the
    /// conversation before its next model call.
    steers: Vec<(String, String)>,
    /// The operator asked for the work item in progress to be compressed.
    compress_requested: bool,
    /// The conversation's size right after the last compression of this work item: an automatic
    /// compression waits until it has grown by a quarter of `compress_at` since.
    compressed_size: u64,
    /// Model calls the operator allowed beyond the profile's turn budget.
    extra_turns: u32,
    /// Set while a local tool runs, once the work item is cancelled or the agent is stopping: an
    /// engine session reads it and stops at once, instead of at the tool's end.
    tool_cancel: Arc<AtomicBool>,
    /// A stop that arrived while a local tool ran. The loop takes it at its next wait, as it
    /// would have taken the line had it arrived then.
    stopping: Option<String>,
}

/// Serves one keepalive connection's agent loop until the host's `stop`, then
/// sends the `bye`; returns how many operator tool calls were answered.
///
/// The reader lane owns the connection's read half for the whole session, and the
/// operator worker answers the owner's calls beside the loop. The `bye` goes out
/// only after both have finished, so every answer precedes it and the count is final.
pub fn serve<S: Duplex, W: Write + Send>(
    lines: &mut Lines<S>,
    log: &mut W,
    workspace: &Workspace,
    profile: Option<AgentProfile>,
    heartbeat: Duration,
) -> Result<u64, SessionError> {
    let writer = Lines::new(lines.get_ref().duplicate()?);
    let shared = Shared::new(writer, log);
    let (inbox_tx, inbox_rx) = mpsc::channel();
    let (operator_tx, operator_rx) = mpsc::channel();
    let shared_ref = &shared;
    let outcome = thread::scope(|scope| {
        scope.spawn(move || read_host(lines, shared_ref, inbox_tx, operator_tx));
        scope.spawn(move || {
            let exec = |name: &str, args: &Value| {
                tools::serve(workspace, name, args).map_err(|err| err.to_string())
            };
            serve_operators(operator_rx, shared_ref, exec, OPERATOR_EXPIRY)
        });
        let mut agent = Agent {
            shared: shared_ref,
            inbox: inbox_rx,
            workspace,
            heartbeat,
            next_beat: Instant::now() + heartbeat,
            seq: 0,
            req: 0,
            profile,
            queue: VecDeque::new(),
            threads: load_threads(workspace),
            clock: 0,
            current: None,
            cancelled: None,
            steers: Vec::new(),
            compress_requested: false,
            compressed_size: 0,
            extra_turns: 0,
            tool_cancel: Arc::new(AtomicBool::new(false)),
            stopping: None,
        };
        agent.clock = agent
            .threads
            .values()
            .map(|thread| thread.at)
            .max()
            .unwrap_or(0);
        let outcome = agent.run();
        shared_ref.stop();
        outcome
    });
    outcome?;
    let calls = shared.calls();
    shared.say(&GuestMessage::Bye { calls })?;
    Ok(calls)
}

impl<S: Write + Send, W: Write + Send> Agent<'_, '_, S, W> {
    /// The agent loop: runs queued work and waits for the host, until the host stops it.
    fn run(&mut self) -> Result<(), SessionError> {
        loop {
            let halt = match self.queue.pop_front() {
                Some(job) => self.run_job(job).err(),
                None => match self.pump() {
                    Ok(Incoming::Stop(reason)) => Some(Halt::Stop(reason)),
                    Ok(_) => None,
                    Err(error) => Some(Halt::Failed(error)),
                },
            };
            match halt {
                None => {}
                Some(Halt::Stop(reason)) => {
                    self.end(&reason);
                    return Ok(());
                }
                Some(Halt::Failed(error)) => return Err(error),
            }
        }
    }

    /// Records the host's stop; the `bye` follows once the lanes have drained.
    fn end(&self, reason: &str) {
        let reason = if reason.is_empty() {
            "no reason given"
        } else {
            reason
        };
        self.shared.note(&format!(
            "agent-harness: the hub stopped the agent ({})",
            escape_for_transcript(reason)
        ));
    }

    fn say(&self, message: &GuestMessage) -> Result<(), SessionError> {
        self.shared.say(message)
    }

    /// Sends a heartbeat, saying whether a work item is in progress.
    fn beat(&mut self) -> Result<(), SessionError> {
        self.seq += 1;
        let seq = self.seq;
        let busy = self.current.is_some();
        self.say(&GuestMessage::Heartbeat {
            seq,
            busy: Some(busy),
        })?;
        self.shared.note(&format!("agent-harness: heartbeat {seq}"));
        self.next_beat = Instant::now() + self.heartbeat;
        Ok(())
    }

    /// Takes the next message from the reader lane, or beats when the beat came
    /// due. The messages the loop owns are acted on here; anything else is returned
    /// for a waiting caller.
    fn pump(&mut self) -> Result<Incoming, SessionError> {
        if let Some(reason) = self.stopping.take() {
            return Ok(Incoming::Stop(reason));
        }
        let wait = self.next_beat.saturating_duration_since(Instant::now());
        let message = match self.inbox.recv_timeout(wait) {
            Ok(Inbound::Message(message)) => message,
            Ok(Inbound::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Err(SessionError::HostClosed);
            }
            Ok(Inbound::Failed(err)) => return Err(SessionError::Io(err)),
            Err(RecvTimeoutError::Timeout) => {
                self.beat()?;
                return Ok(Incoming::Beat);
            }
        };
        self.take(message)
    }

    /// Acts on one host message the loop owns; anything else is returned for a waiting caller.
    fn take(&mut self, message: HostMessage) -> Result<Incoming, SessionError> {
        Ok(match message {
            HostMessage::Stop { reason } => Incoming::Stop(reason),
            HostMessage::Work {
                work,
                kind,
                thread,
                prompt,
            } => {
                self.queue.push_back(Job {
                    work,
                    kind,
                    thread,
                    prompt,
                });
                Incoming::Handled
            }
            HostMessage::Cancel { work, reason } => {
                self.cancel(work, reason)?;
                Incoming::Handled
            }
            HostMessage::Profile { profile } => {
                self.profile = Some(profile);
                Incoming::Handled
            }
            HostMessage::Control { work, control } => {
                if self.current == Some(work) {
                    match control {
                        Control::Compress => self.compress_requested = true,
                        Control::Steer { from, text } => self.steers.push((from, text)),
                        Control::ExtendTurns { turns } => {
                            self.extra_turns = self.extra_turns.saturating_add(turns)
                        }
                    }
                } else {
                    self.shared.note(&format!(
                        "agent-harness: ignoring control for work {work}, which is not running"
                    ));
                }
                Incoming::Handled
            }
            other => Incoming::Message(other),
        })
    }

    /// Waits for the host message `pick` accepts; anything else is logged and
    /// skipped. A stop from the host ends the wait with [`Halt::Stop`].
    fn await_message<T>(
        &mut self,
        awaited: &str,
        mut pick: impl FnMut(HostMessage) -> Result<T, HostMessage>,
    ) -> Result<T, Halt> {
        loop {
            match self.pump()? {
                Incoming::Message(message) => match pick(message) {
                    Ok(picked) => return Ok(picked),
                    Err(other) => {
                        let kind = kind_of(&other);
                        self.shared.note(&format!(
                            "agent-harness: ignoring {kind} while waiting for {awaited}"
                        ));
                    }
                },
                Incoming::Stop(reason) => return Err(Halt::Stop(reason)),
                Incoming::Beat | Incoming::Handled => {}
            }
        }
    }

    /// Runs a local tool on its own thread, beating beside it and taking the
    /// host's messages meanwhile. A panicking tool is an error result, not a dead
    /// session.
    fn run_local(&mut self, job: impl FnOnce() -> Outcome + Send) -> Result<Outcome, SessionError> {
        self.tool_cancel.store(false, Ordering::SeqCst);
        thread::scope(|scope| -> Result<Outcome, SessionError> {
            let handle = scope.spawn(job);
            while !handle.is_finished() {
                if Instant::now() >= self.next_beat {
                    self.beat()?;
                }
                self.hear_during_tool()?;
                thread::sleep(LOCAL_POLL);
            }
            Ok(handle
                .join()
                .unwrap_or_else(|_| Err("internal: the tool panicked".to_string())))
        })
    }

    /// Takes the host's messages that arrived while a local tool runs, without
    /// waiting for them. A cancel of the work item reaches the tool through
    /// `tool_cancel`; a stop is kept for the loop's next wait, so the tool's result
    /// is still reported first.
    fn hear_during_tool(&mut self) -> Result<(), SessionError> {
        loop {
            let message = match self.inbox.try_recv() {
                Ok(Inbound::Message(message)) => message,
                Err(TryRecvError::Empty) => return Ok(()),
                Ok(Inbound::Closed) | Err(TryRecvError::Disconnected) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    return Err(SessionError::HostClosed);
                }
                Ok(Inbound::Failed(err)) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    return Err(SessionError::Io(err));
                }
            };
            match self.take(message)? {
                Incoming::Stop(reason) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    self.stopping = Some(reason);
                }
                Incoming::Message(other) => {
                    let kind = kind_of(&other);
                    self.shared
                        .note(&format!("agent-harness: ignoring {kind} while a tool runs"));
                }
                Incoming::Beat | Incoming::Handled => {}
            }
        }
    }

    /// `cancel`: the work item in progress stops after its current step; a
    /// queued one is finished as stopped without running.
    fn cancel(&mut self, work: u64, reason: String) -> Result<(), SessionError> {
        if self.current == Some(work) {
            self.cancelled = Some(reason);
            self.tool_cancel.store(true, Ordering::SeqCst);
        } else if let Some(at) = self.queue.iter().position(|job| job.work == work) {
            self.queue.remove(at);
            self.finish(work, WorkStatus::Stopped, &reason)?;
        } else {
            self.shared.note(&format!(
                "agent-harness: ignoring cancel for work {work}, which is not queued"
            ));
        }
        Ok(())
    }

    fn finish(&mut self, work: u64, status: WorkStatus, summary: &str) -> Result<(), SessionError> {
        self.shared.note(&format!(
            "agent-harness: work {work} {status:?}: {}",
            escape_for_transcript(summary)
        ));
        self.say(&GuestMessage::WorkDone {
            work,
            status,
            summary: summary.to_string(),
        })
    }

    /// Runs one work item to its end. A chat item extends its thread's
    /// conversation; any other item starts fresh.
    fn run_job(&mut self, job: Job) -> Result<(), Halt> {
        let Some(profile) = self.profile.clone() else {
            self.finish(job.work, WorkStatus::Failed, "this agent has no model")?;
            return Ok(());
        };
        let chat = job.kind == WorkKind::Chat;
        let mut prompt = job.prompt;
        let mut messages = Vec::new();
        if chat {
            let thread = self.threads.remove(&job.thread).unwrap_or_default();
            if serialized_len(&thread.messages) > THREAD_CLOSE_BYTES {
                prompt = format!(
                    "Earlier in this conversation: {}\n\n{prompt}",
                    thread.summary
                );
            } else {
                messages = thread.messages;
            }
        }
        // A conversation that ended on tool results (a stopped item) gets the
        // prompt in that same user turn, so roles still alternate.
        match messages.last_mut() {
            Some(last) if last.role == Role::User => {
                last.content.push(Block::Text { text: prompt })
            }
            _ => messages.push(Message::user_text(prompt)),
        }

        let _ = crate::checkpoints::snapshot(self.workspace, &format!("work-{}", job.work));
        self.current = Some(job.work);
        self.cancelled = None;
        self.steers.clear();
        self.compress_requested = false;
        self.compressed_size = 0;
        self.extra_turns = 0;
        let result = self.drive(job.work, &profile, &mut messages);
        self.current = None;
        let (status, summary) = result?;
        if chat {
            self.clock += 1;
            let at = self.clock;
            self.threads.insert(
                job.thread,
                Thread {
                    messages,
                    summary: summary.clone(),
                    at,
                },
            );
            if let Err(err) = save_threads(self.workspace, &self.threads) {
                self.shared
                    .note(&format!("agent-harness: chat threads not saved: {err}"));
            }
        }
        self.finish(job.work, status, &summary)?;
        Ok(())
    }

    /// The model loop of one work item: call the model, run what it asks for,
    /// repeat until a stop condition. `messages` is extended in place.
    fn drive(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &mut Vec<Message>,
    ) -> Result<(WorkStatus, String), Halt> {
        let mut turns = 0u32;
        let mut token_limit_hits = 0u32;
        // The last tool call's name and input, and how many times in a row it failed.
        let mut failing: Option<(String, u32)> = None;
        loop {
            if let Some(reason) = self.cancelled.take() {
                return Ok((WorkStatus::Stopped, reason));
            }
            for (from, text) in std::mem::take(&mut self.steers) {
                let note_text = format!("[note from {from}]: {text}");
                match messages.last_mut() {
                    Some(last) if last.role == Role::User => {
                        last.content.push(Block::Text { text: note_text })
                    }
                    _ => messages.push(Message::user_text(note_text)),
                }
            }
            let size = serialized_len(messages.as_slice()) as u64;
            let auto = profile.compress_at > 0
                && size > profile.compress_at
                && size > self.compressed_size.saturating_add(profile.compress_at / 4);
            if self.compress_requested || auto {
                let reason = if self.compress_requested {
                    "asked"
                } else {
                    "auto"
                };
                self.compress_requested = false;
                self.compress(work, profile, messages, reason)?;
            }
            if turns >= profile.max_turns.saturating_add(self.extra_turns) {
                return Ok((WorkStatus::Stopped, "turn budget reached".to_string()));
            }
            turns += 1;
            let response = match self.model_call(work, profile, messages)? {
                Ok(response) => response,
                Err(reason) => return Ok((WorkStatus::Failed, reason)),
            };
            let ChatResponse { content, stop, .. } = response;
            messages.push(Message::assistant(content.clone()));

            let uses: Vec<(String, String, Value, Option<Value>)> = content
                .iter()
                .filter_map(|block| match block {
                    Block::ToolUse {
                        id,
                        name,
                        input,
                        meta,
                    } => Some((id.clone(), name.clone(), input.clone(), meta.clone())),
                    _ => None,
                })
                .collect();
            let ran_tools = !uses.is_empty();
            let mut results = Vec::new();
            let mut stuck = None;
            for (id, name, input, meta) in uses {
                let (text, is_error) = self.tool_step(work, &id, &name, &input, meta.as_ref())?;
                let key = format!("{name} {input}");
                failing = match (is_error, failing.take()) {
                    (false, _) => None,
                    (true, Some((last, count))) if last == key => Some((last, count + 1)),
                    (true, _) => Some((key, 1)),
                };
                if matches!(failing, Some((_, count)) if count >= STUCK_FAILURES) && stuck.is_none()
                {
                    stuck = Some(format!(
                        "stuck: {name} failed {STUCK_FAILURES} times in a row with the same input"
                    ));
                }
                results.push(Block::ToolResult {
                    tool_use_id: id,
                    content: text,
                    is_error,
                });
            }
            if ran_tools {
                messages.push(Message {
                    role: Role::User,
                    content: results,
                });
            }
            if let Some(reason) = stuck {
                return Ok((WorkStatus::Stopped, reason));
            }

            match stop {
                StopReason::EndTurn => return Ok((WorkStatus::Done, blocks_text(&content))),
                StopReason::Refusal => {
                    return Ok((WorkStatus::Failed, "the model declined".to_string()));
                }
                StopReason::MaxTokens => {
                    token_limit_hits += 1;
                    if token_limit_hits > TOKEN_LIMIT_CONTINUES {
                        return Ok((
                            WorkStatus::Stopped,
                            "the model hit its token limit".to_string(),
                        ));
                    }
                    if !ran_tools {
                        messages.push(Message::user_text("continue"));
                    }
                }
                other if !ran_tools => {
                    return Ok((
                        WorkStatus::Failed,
                        format!("the model stopped without an answer ({other:?})"),
                    ));
                }
                _ => {}
            }
        }
    }

    /// Folds the conversation's older turns into a summary the model writes: the
    /// first message (the task) stays, the `keep_recent` latest turns stay verbatim,
    /// and the summary is added to the first message. A failed summary is logged
    /// and the conversation goes on uncompressed.
    fn compress(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &mut Vec<Message>,
        reason: &str,
    ) -> Result<(), Halt> {
        let budget = if profile.compress_at > 0 {
            (profile.compress_at / 2) as usize
        } else {
            usize::MAX
        };
        let Some(cut) = compress_cut(messages.as_slice(), profile.keep_recent, budget) else {
            return Ok(());
        };
        if cut <= 1 {
            return Ok(());
        }
        let before = serialized_len(messages.as_slice()) as u64;
        // The previous summary is folded into the new one, so the conversation carries exactly one.
        let previous: Vec<String> = messages[0]
            .content
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } if text.starts_with(SUMMARY_HEAD) => Some(text.clone()),
                _ => None,
            })
            .collect();
        messages[0].content.retain(
            |block| !matches!(block, Block::Text { text } if text.starts_with(SUMMARY_HEAD)),
        );
        let mut transcript = previous.join("\n");
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        for message in &messages[1..cut] {
            let role = if message.role == Role::User {
                "user"
            } else {
                "assistant"
            };
            for block in &message.content {
                let line = match block {
                    Block::Text { text } => text.clone(),
                    Block::ToolUse { name, input, .. } => format!("→ {name} {input}"),
                    Block::ToolResult { content, .. } => format!("← {}", cap_output(content, 2000)),
                    _ => continue,
                };
                transcript.push_str(&format!("{role}: {line}\n"));
            }
        }
        let request = format!(
            "Summarize this work so far for yourself, so you can continue it without the transcript: the goal, what you did, what you found, files you created or changed (paths), decisions, what failed, and the next steps. Be specific and complete; at most 600 words.\n\n{transcript}"
        );
        let mut summary_profile = profile.clone();
        summary_profile.tools.clear();
        let summary_msgs = vec![Message::user_text(request)];
        let response = match self.model_call(work, &summary_profile, &summary_msgs)? {
            Ok(response) => response,
            Err(reason) => {
                let reason = escape_for_transcript(&reason);
                self.shared.note(&format!(
                    "agent-harness: work {work} not compressed: {reason}"
                ));
                return Ok(());
            }
        };
        let summary = blocks_text(&response.content);
        messages[0].content.push(Block::Text {
            text: format!("{SUMMARY_HEAD}\n{summary}"),
        });
        messages.drain(1..cut);
        clip_old_results(messages);
        let after = serialized_len(messages.as_slice()) as u64;
        self.compressed_size = after;
        self.shared.note(&format!(
            "agent-harness: work {work} compressed ({reason}): {before} -> {after} bytes"
        ));
        self.say(&GuestMessage::Compressed {
            work,
            before,
            after,
            folded: (cut - 1) as u32,
            reason: reason.to_string(),
        })?;
        Ok(())
    }

    /// One model call on the conversation so far. The inner `Err` is the host's
    /// reason the call failed.
    fn model_call(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &[Message],
    ) -> Result<Result<ChatResponse, String>, Halt> {
        self.req += 1;
        let req = self.req;
        let request = ChatRequest {
            model: String::new(),
            system: (!profile.system.is_empty()).then(|| profile.system.clone()),
            messages: messages.to_vec(),
            tools: profile.tools.clone(),
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        self.say(&GuestMessage::ModelCall { req, work, request })?;
        let (ok, response, error) =
            self.await_message("model_result", |message| match message {
                HostMessage::ModelResult {
                    req: got,
                    ok,
                    response,
                    error,
                } if got == req => Ok((ok, response, error)),
                other => Err(other),
            })?;
        Ok(match (ok, response) {
            (true, Some(response)) => Ok(response),
            (true, None) => Err("the host sent no answer".to_string()),
            (false, _) => Err(error.unwrap_or_else(|| "the model call failed".to_string())),
        })
    }

    /// One tool the model asked for. Returns the result's text and whether it is
    /// an error. Malformed input is refused without asking the host; otherwise the
    /// host decides, and an allowed tool runs here.
    fn tool_step(
        &mut self,
        work: u64,
        id: &str,
        name: &str,
        input: &Value,
        meta: Option<&Value>,
    ) -> Result<(String, bool), Halt> {
        if let Some(reason) = invalid_input(input, meta) {
            return Ok((format!("INVALID_INPUT: {reason}"), true));
        }
        self.say(&GuestMessage::ToolRequest {
            work,
            call: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        })?;
        let (decision, output, is_error, program) =
            self.await_message("tool_decision", |message| match message {
                HostMessage::ToolDecision {
                    call,
                    decision,
                    output,
                    is_error,
                    program,
                    ..
                } if call == id => Ok((decision, output, is_error, program)),
                other => Err(other),
            })?;
        match decision {
            Decision::Allow => {
                let workspace = self.workspace;
                let headless = self
                    .profile
                    .as_ref()
                    .is_some_and(|profile| profile.engine_transport == "headless");
                // A library tool comes with its program; built-in tools run from this harness.
                let outcome = match program {
                    Some(program) => {
                        self.run_local(move || library::run(workspace, &program, input))?
                    }
                    // An engine runs as an ACP session whose every action the hub checks, unless the
                    // profile keeps the one-shot run of earlier releases.
                    None if name == "code_engine" && !headless => {
                        let shared = self.shared;
                        let cancel = Arc::clone(&self.tool_cancel);
                        let call = id.to_string();
                        // The engine's MCP server offers the profile's plaza tools, through the hub.
                        let tools = self
                            .profile
                            .as_ref()
                            .map_or_else(Vec::new, |profile| profile.tools.clone());
                        let protocol = self.profile.as_ref().map_or(1, |profile| {
                            crate::acp::run::protocol_offer(&profile.engine_transport)
                        });
                        self.run_local(move || {
                            let link = HubLink::new(shared, work, call, &cancel, tools);
                            crate::acp::run::run(workspace, input, &link, protocol)
                        })?
                    }
                    None => self.run_local(move || run_guest_tool(workspace, name, input))?,
                };
                let (ok, output) = match outcome {
                    Ok(output) => (true, output),
                    Err(err) => (false, err),
                };
                // A packaged tool travels to the host whole (the library files it from the report);
                // the model only needs to hear what was packaged.
                if name == "tool_package" && ok {
                    self.say(&GuestMessage::ToolReport {
                        work,
                        call: id.to_string(),
                        ok,
                        output: output.clone(),
                    })?;
                    return Ok((library::summary(&output), false));
                }
                let output = cap_output(&output, MAX_TOOL_OUTPUT);
                self.say(&GuestMessage::ToolReport {
                    work,
                    call: id.to_string(),
                    ok,
                    output: output.clone(),
                })?;
                Ok((output, !ok))
            }
            Decision::Done => Ok((cap_output(&output, MAX_TOOL_OUTPUT), is_error)),
            // Held for the owner (only an ACP engine's requests are held, and those wait on their own
            // nested ids). A model's call that comes back held is an error it can retry later.
            Decision::Pending => Ok((
                "the owner has not decided this tool call yet; try again later".to_string(),
                true,
            )),
            Decision::Deny if output.is_empty() => {
                Ok(("the host denied this tool call".to_string(), true))
            }
            Decision::Deny => Ok((output, true)),
        }
    }
}

/// Why a tool call's input cannot run, found without asking the host: the
/// gateway could not parse the model's arguments, or they are not an object.
fn invalid_input(input: &Value, meta: Option<&Value>) -> Option<&'static str> {
    if meta.is_some_and(|meta| {
        meta.get("kind").and_then(Value::as_str) == Some("gateway")
            && meta.get("error").and_then(Value::as_str) == Some("INVALID_JSON")
    }) {
        return Some("the arguments were not valid JSON");
    }
    if !input.is_object() {
        return Some("the arguments must be a JSON object");
    }
    None
}

/// The names [`run_guest_tool`] serves itself, besides the file tools of [`tools::serve`].
const GUEST_TOOLS: [&str; 17] = [
    "bash",
    "install_packages",
    "multi_edit",
    "glob",
    "grep",
    "bash_start",
    "bash_output",
    "bash_kill",
    "todo_write",
    "code_engine",
    "tool_package",
    "deliver_file",
    "share_file",
    "checkpoint_diff",
    "checkpoint_restore",
    "checkpoint_list",
    "edit_file",
];

/// True for a tool the harness runs itself, so the VM's own tools: the ones an engine's plaza MCP
/// server must not offer, since the engine has its own through ACP.
pub(crate) fn is_guest_tool(name: &str) -> bool {
    GUEST_TOOLS.contains(&name) || tools::serves(name)
}

/// Runs a tool the host allowed, under the names the model knows. `edit_file`
/// takes `old` and `new` from the model, which the file tool calls `find` and
/// `replace`.
fn run_guest_tool(workspace: &Workspace, name: &str, input: &Value) -> Outcome {
    match name {
        "bash" => bash::run(workspace, input),
        "install_packages" => bash::install_packages(workspace, input),
        "multi_edit" => files::multi_edit(workspace, input),
        "glob" => search_tools::glob(workspace, input),
        "grep" => search_tools::grep(workspace, input),
        "bash_start" => shells::start(workspace, input),
        "bash_output" => shells::output(workspace, input),
        "bash_kill" => shells::kill(workspace, input),
        "todo_write" => plan::todo_write(workspace, input),
        "code_engine" => engines::run(workspace, input),
        "tool_package" => library::package(workspace, input),
        "deliver_file" => library::deliver(workspace, input),
        "share_file" => library::share(workspace, input),
        "checkpoint_diff" => checkpoints::diff(workspace, input),
        "checkpoint_restore" => checkpoints::restore(workspace, input),
        "checkpoint_list" => checkpoints::list(workspace, input),
        "edit_file" => {
            let args = edit_args(input)?;
            tools::serve(workspace, name, &args).map_err(|err| err.to_string())
        }
        // The owner's IDE tools come from the hub's operator, never from the model's own calls.
        name if name.starts_with("ide_") => Err(format!(
            "denied: {name} is an owner tool; the model is not offered it"
        )),
        _ => tools::serve(workspace, name, input).map_err(|err| err.to_string()),
    }
}

/// The file tool's arguments for the model's `{path, old, new}`.
fn edit_args(input: &Value) -> Result<Value, String> {
    let (Some(old), Some(new)) = (
        input.get("old").and_then(Value::as_str),
        input.get("new").and_then(Value::as_str),
    ) else {
        return Err("invalid input: edit_file needs string \"old\" and \"new\"".to_string());
    };
    let mut args = input.clone();
    args["find"] = json!(old);
    args["replace"] = json!(new);
    Ok(args)
}

/// The size of a conversation as it is sent, in bytes.
fn serialized_len(messages: &[Message]) -> usize {
    serde_json::to_vec(messages).map_or(0, |bytes| bytes.len())
}

/// The head of the one summary block a compressed conversation carries in its first message.
const SUMMARY_HEAD: &str = "Summary of the work so far (earlier turns were compressed):";
/// The most a tool result kept after a compression may hold, except in the latest turn.
const KEPT_RESULT_BYTES: usize = 8 * 1024;

/// Where a compression cuts the conversation: an assistant message from which on the turns
/// stay verbatim. It starts at the `keep_recent`-th most recent assistant message (at least
/// the latest) and moves later while the kept tail is larger than `budget` bytes, so a
/// compression always brings the conversation well under its threshold. None when there is
/// no such message (fewer assistant messages than `keep_recent`).
fn compress_cut(messages: &[Message], keep_recent: u32, budget: usize) -> Option<usize> {
    let keep = keep_recent.max(1) as usize;
    let assistants: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Assistant)
        .map(|(index, _)| index)
        .collect();
    let last = *assistants.last()?;
    let mut position = assistants.len().checked_sub(keep)?;
    while assistants[position] < last && serialized_len(&messages[assistants[position]..]) > budget
    {
        position += 1;
    }
    Some(assistants[position])
}

/// Clips every tool result before the latest turn to KEPT_RESULT_BYTES: what was read long
/// ago is in the summary; the latest results stay whole.
fn clip_old_results(messages: &mut [Message]) {
    let latest = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
        .unwrap_or(0);
    for message in &mut messages[..latest] {
        for block in &mut message.content {
            if let Block::ToolResult { content, .. } = block {
                if content.len() > KEPT_RESULT_BYTES {
                    *content = cap_output(content, KEPT_RESULT_BYTES);
                }
            }
        }
    }
}

/// The file, under the workspace's state directory, that holds the operator's
/// chat threads across restarts.
const THREADS_FILE: &str = "threads.json";
/// The most a saved thread file may hold; the oldest threads are dropped first.
const THREADS_MAX_BYTES: usize = 1024 * 1024;

fn thread_json(thread: &Thread) -> Value {
    json!({
        "messages": serde_json::to_value(&thread.messages).unwrap_or(Value::Null),
        "summary": thread.summary,
        "at": thread.at,
    })
}

fn thread_from_json(value: &Value) -> Option<Thread> {
    Some(Thread {
        messages: serde_json::from_value(value.get("messages")?.clone()).ok()?,
        summary: value.get("summary")?.as_str()?.to_string(),
        at: value.get("at")?.as_u64()?,
    })
}

/// The operator's chat threads a previous session saved; none when the file is
/// absent or unreadable.
fn load_threads(workspace: &Workspace) -> HashMap<String, Thread> {
    let Ok(bytes) = std::fs::read(workspace.plaza_dir().join(THREADS_FILE)) else {
        return HashMap::new();
    };
    let Ok(Value::Object(saved)) = serde_json::from_slice::<Value>(&bytes) else {
        return HashMap::new();
    };
    saved
        .iter()
        .filter(|(id, _)| id.starts_with("op-"))
        .filter_map(|(id, value)| Some((id.clone(), thread_from_json(value)?)))
        .collect()
}

/// Saves the operator's chat threads (the `op-*` ones) by writing a temp file
/// and renaming it over the old one. Past [`THREADS_MAX_BYTES`] the oldest
/// threads are dropped first. Skips silently when the workspace is not mounted.
fn save_threads(workspace: &Workspace, threads: &HashMap<String, Thread>) -> std::io::Result<()> {
    if !workspace.dir().is_dir() {
        return Ok(());
    }
    // Newest first, so the oldest is last and goes first.
    let mut kept: Vec<(&String, &Thread)> = threads
        .iter()
        .filter(|(id, _)| id.starts_with("op-"))
        .collect();
    kept.sort_by(|a, b| b.1.at.cmp(&a.1.at));
    let render = |kept: &[(&String, &Thread)]| {
        let object: serde_json::Map<String, Value> = kept
            .iter()
            .map(|(id, thread)| ((*id).clone(), thread_json(thread)))
            .collect();
        serde_json::to_vec(&object)
    };
    let mut bytes = render(&kept)?;
    while bytes.len() > THREADS_MAX_BYTES && !kept.is_empty() {
        kept.pop();
        bytes = render(&kept)?;
    }

    let dir = workspace.plaza_dir();
    std::fs::create_dir_all(&dir)?;
    let temp = dir.join(format!("{THREADS_FILE}.tmp"));
    std::fs::write(&temp, &bytes)?;
    std::fs::rename(&temp, dir.join(THREADS_FILE))
}

#[cfg(test)]
mod thread_store_tests {
    use super::*;

    #[test]
    fn saved_threads_load_back_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("apz-harness-threads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);

        let mut threads = HashMap::new();
        threads.insert(
            "op-1".to_string(),
            Thread {
                messages: vec![Message::user_text("hello".to_string())],
                summary: "greeted".into(),
                at: 1,
            },
        );
        // A non-operator thread is not kept across restarts.
        threads.insert(
            "job-9".to_string(),
            Thread {
                messages: Vec::new(),
                summary: String::new(),
                at: 2,
            },
        );
        save_threads(&workspace, &threads).expect("threads save");

        let loaded = load_threads(&workspace);
        assert_eq!(loaded.len(), 1);
        let thread = &loaded["op-1"];
        assert_eq!(thread.messages, threads["op-1"].messages);
        assert_eq!(thread.summary, "greeted");
        assert_eq!(thread.at, 1);
        assert!(!dir.join(format!("{THREADS_FILE}.tmp")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keepalive;
    use crate::session::Identity;
    use crate::session::testing::{next, pair};
    use apz_proto::line::Lines;
    use std::os::unix::net::UnixStream;

    const IDENTITY: Identity = Identity {
        guest_cid: 104,
        vm: 4,
    };
    const HEARTBEAT: Duration = Duration::from_millis(20);

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn the_owners_ide_tools_are_refused_when_the_model_asks_for_them() {
        let workspace = scratch("ide-guard");
        let err = run_guest_tool(
            &workspace,
            "ide_write",
            &json!({"path": "a.txt", "text": "x", "expected_hash": null}),
        )
        .expect_err("the model may not write through the owner's tools");
        assert!(err.starts_with("denied: ide_write"), "{err}");
        assert!(!workspace.dir().join("a.txt").exists());
    }

    fn welcome(max_turns: u32) -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system": "be brief",
                "tools": [{"name": "bash", "description": "run", "input_schema": {"type": "object"}}],
                "max_turns": max_turns,
            },
        })
        .to_string()
    }

    fn work(id: u64, kind: &str, thread: &str, prompt: &str) -> String {
        json!({"type": "work", "work": id, "kind": kind, "thread": thread, "prompt": prompt})
            .to_string()
    }

    fn answer(req: u64, content: Value, stop: &str) -> String {
        json!({
            "type": "model_result", "req": req, "ok": true,
            "response": {"model": "anthropic/x", "content": content, "stop": stop,
                         "usage": {"input_tokens": 1, "output_tokens": 1}},
        })
        .to_string()
    }

    fn text(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    fn decision(call: &str, decision: &str, output: &str, is_error: bool) -> String {
        json!({"type": "tool_decision", "call": call, "decision": decision, "output": output, "is_error": is_error})
            .to_string()
    }

    /// The guest's next line that is not a heartbeat, parsed.
    fn next_msg(host: &mut Lines<UnixStream>) -> Value {
        loop {
            let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
            if value["type"] != "heartbeat" {
                return value;
            }
        }
    }

    fn expect_type(host: &mut Lines<UnixStream>, kind: &str) -> Value {
        let value = next_msg(host);
        assert_eq!(value["type"], kind, "{value}");
        value
    }

    /// Ends the session the way the hub does, and checks the `bye`.
    fn stop(host: &mut Lines<UnixStream>) {
        host.send(r#"{"type":"stop","reason":"test over"}"#)
            .unwrap();
        expect_type(host, "bye");
    }

    /// Runs a keepalive session whose hub is `script`, and returns the calls served.
    fn run_hub(
        workspace: &Workspace,
        script: impl FnOnce(&mut Lines<UnixStream>) + Send + 'static,
    ) -> u64 {
        let (mut guest, mut host) = pair();
        let hub = thread::spawn(move || {
            next(&mut host);
            script(&mut host);
        });
        let mut log = Vec::new();
        let result = keepalive::session(
            &mut guest,
            &mut log,
            &IDENTITY,
            workspace,
            Some("builder"),
            HEARTBEAT,
        );
        drop(guest);
        hub.join().expect("the scripted hub finished");
        result.expect("the hub stops the agent")
    }

    #[test]
    fn an_allowed_tool_runs_here_and_its_output_reaches_the_next_model_call() {
        let workspace = scratch("allow");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "say hi")).unwrap();
            let first = expect_type(host, "model_call");
            assert_eq!(
                (first["req"].clone(), first["request"]["system"].clone()),
                (json!(1), json!("be brief"))
            );
            assert_eq!(
                first["request"]["messages"][0]["content"][0]["text"],
                "say hi"
            );
            host.send(&answer(
                1,
                json!([tool_use("t1", "bash", json!({"command": "echo hi"}))]),
                "tool_use",
            ))
            .unwrap();
            let request = expect_type(host, "tool_request");
            assert_eq!(
                (request["call"].clone(), request["name"].clone()),
                (json!("t1"), json!("bash"))
            );
            host.send(&decision("t1", "allow", "", false)).unwrap();
            let report = expect_type(host, "tool_report");
            assert_eq!(
                (report["ok"].clone(), report["output"].clone()),
                (json!(true), json!("hi\nexit 0"))
            );
            let second = expect_type(host, "model_call");
            assert_eq!(
                second["request"]["messages"][2]["content"][0]["content"],
                "hi\nexit 0"
            );
            host.send(&answer(2, json!([text("all done")]), "end_turn"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(
                (
                    done["work"].clone(),
                    done["status"].clone(),
                    done["summary"].clone()
                ),
                (json!(1), json!("done"), json!("all done"))
            );
            stop(host);
        });
    }

    #[test]
    fn a_host_run_tool_uses_the_decisions_output_and_the_guest_reports_nothing() {
        let workspace = scratch("done");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "find it")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("s1", "search", json!({"pattern": "x"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("s1", "done", "3 hits", false)).unwrap();
            let next = expect_type(host, "model_call");
            assert_eq!(
                next["request"]["messages"][2]["content"][0]["content"],
                "3 hits"
            );
            host.send(&answer(2, json!([text("found")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    fn control(work: u64, body: Value) -> String {
        json!({"type": "control", "work": work, "control": body}).to_string()
    }

    /// A profile that compresses whenever there is something older to fold.
    fn compressing_welcome() -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system": "be brief",
                "tools": [{"name": "search", "description": "find", "input_schema": {"type": "object"}}],
                "max_turns": 10, "compress_at": 1, "keep_recent": 1,
            },
        })
        .to_string()
    }

    #[test]
    fn a_steer_control_reaches_the_next_model_call_as_a_note() {
        let workspace = scratch("steer");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            host.send(&control(
                1,
                json!({"action": "steer", "from": "mayor", "text": "focus on the parser"}),
            ))
            .unwrap();
            host.send(&answer(
                1,
                json!([tool_use("s1", "search", json!({"pattern": "x"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("s1", "done", "3 hits", false)).unwrap();
            let next = expect_type(host, "model_call");
            let results = &next["request"]["messages"][2]["content"];
            assert_eq!(
                results[1]["text"], "[note from mayor]: focus on the parser",
                "{results}"
            );
            host.send(&answer(2, json!([text("found")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_long_conversation_is_compressed_before_the_next_model_call() {
        let workspace = scratch("compress");
        run_hub(&workspace, |host| {
            host.send(&compressing_welcome()).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            for (req, id) in [(1, "s1"), (2, "s2")] {
                expect_type(host, "model_call");
                host.send(&answer(
                    req,
                    json!([tool_use(id, "search", json!({"pattern": id}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_request");
                host.send(&decision(id, "done", "hits", false)).unwrap();
            }
            let summary_call = expect_type(host, "model_call");
            let asked = &summary_call["request"]["messages"][0]["content"][0]["text"];
            assert!(
                asked
                    .as_str()
                    .is_some_and(|text| text.starts_with("Summarize")),
                "{asked}"
            );
            host.send(&answer(3, json!([text("the summary")]), "end_turn"))
                .unwrap();
            let compressed = expect_type(host, "compressed");
            assert_eq!(
                (
                    compressed["work"].clone(),
                    compressed["folded"].clone(),
                    compressed["reason"].clone()
                ),
                (json!(1), json!(2), json!("auto"))
            );
            let next = expect_type(host, "model_call");
            let messages = &next["request"]["messages"];
            assert_eq!(messages.as_array().map(Vec::len), Some(3), "{messages}");
            assert!(
                messages[0]["content"][1]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("the summary"))
            );
            host.send(&answer(4, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn the_compression_cut_keeps_the_latest_assistant_turns() {
        let user = || Message::user_text("u");
        let assistant = || {
            Message::assistant(vec![Block::Text {
                text: "a".to_string(),
            }])
        };
        let messages = vec![user(), assistant(), user(), assistant(), user()];
        assert_eq!(compress_cut(&messages, 1, usize::MAX), Some(3));
        assert_eq!(compress_cut(&messages, 0, usize::MAX), Some(3));
        assert_eq!(compress_cut(&messages, 2, usize::MAX), Some(1));
        assert_eq!(compress_cut(&messages, 3, usize::MAX), None);
        // A tight budget moves the cut later, keeping less, but always the latest turn.
        assert_eq!(compress_cut(&messages, 2, 1), Some(3));
    }

    #[test]
    fn a_denied_tool_is_an_error_result_carrying_the_reason() {
        let workspace = scratch("deny");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "rm it")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("d1", "bash", json!({"command": "rm -rf /"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("d1", "deny", "not allowed here", true))
                .unwrap();
            let next = expect_type(host, "model_call");
            let result = &next["request"]["messages"][2]["content"][0];
            assert_eq!(
                (result["content"].clone(), result["is_error"].clone()),
                (json!("not allowed here"), json!(true))
            );
            host.send(&answer(2, json!([text("ok")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn malformed_tool_input_is_refused_without_asking_the_host() {
        let workspace = scratch("invalid");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            let not_an_object = tool_use("i1", "bash", json!("ls"));
            let gateway_meta = json!({"type": "tool_use", "id": "i2", "name": "bash", "input": {},
                                      "meta": {"kind": "gateway", "error": "INVALID_JSON"}});
            host.send(&answer(1, json!([not_an_object, gateway_meta]), "tool_use"))
                .unwrap();
            let next = expect_type(host, "model_call");
            let results = &next["request"]["messages"][2]["content"];
            assert!(
                results[0]["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("INVALID_INPUT: ")),
                "{results}"
            );
            assert!(
                results[1]["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("INVALID_INPUT: ")),
                "{results}"
            );
            assert_eq!(results[1]["is_error"], true);
            host.send(&answer(2, json!([text("ok")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_refusal_fails_the_work_item() {
        let workspace = scratch("refusal");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "x")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(1, json!([text("no")]), "refusal"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (json!("failed"), json!("the model declined"))
            );
            stop(host);
        });
    }

    #[test]
    fn a_second_token_limit_stops_after_one_continue() {
        let workspace = scratch("limit");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "long")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(1, json!([text("partial")]), "max_tokens"))
                .unwrap();
            let again = expect_type(host, "model_call");
            let messages = &again["request"]["messages"];
            assert_eq!(
                messages[messages.as_array().unwrap().len() - 1]["content"][0]["text"],
                "continue"
            );
            host.send(&answer(2, json!([text("more")]), "max_tokens"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (json!("stopped"), json!("the model hit its token limit"))
            );
            stop(host);
        });
    }

    #[test]
    fn the_turn_budget_stops_the_work_before_another_model_call() {
        let workspace = scratch("budget");
        run_hub(&workspace, |host| {
            host.send(&welcome(2)).unwrap();
            host.send(&work(1, "task", "t", "loop")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("b1", "list", json!({"path": "a"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("b1", "done", "[]", false)).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                2,
                json!([tool_use("b2", "list", json!({"path": "b"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("b2", "done", "[]", false)).unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (json!("stopped"), json!("turn budget reached"))
            );
            stop(host);
        });
    }

    #[test]
    fn the_same_failing_call_three_times_is_stuck() {
        let workspace = scratch("stuck");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "again")).unwrap();
            for req in 1..=3 {
                expect_type(host, "model_call");
                let call = format!("s{req}");
                host.send(&answer(
                    req,
                    json!([tool_use(&call, "bash", json!({"command": "x"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_request");
                host.send(&decision(&call, "deny", "no", true)).unwrap();
            }
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (
                    json!("stopped"),
                    json!("stuck: bash failed 3 times in a row with the same input")
                )
            );
            stop(host);
        });
    }

    #[test]
    fn a_cancel_stops_after_the_current_step_and_runs_nothing_more() {
        let workspace = scratch("cancel");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "stop me")).unwrap();
            expect_type(host, "model_call");
            host.send(r#"{"type":"cancel","work":1,"reason":"operator stop"}"#)
                .unwrap();
            host.send(&answer(
                1,
                json!([tool_use("c1", "bash", json!({"command": "echo hi"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("c1", "allow", "", false)).unwrap();
            expect_type(host, "tool_report");
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (json!("stopped"), json!("operator stop"))
            );
            stop(host);
        });
    }

    #[test]
    fn a_cancel_that_arrives_while_a_tool_runs_stops_the_work_before_its_next_model_call() {
        let workspace = scratch("cancel-in-tool");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "run it")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("c1", "bash", json!({"command": "sleep 1"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("c1", "allow", "", false)).unwrap();
            // Sent while the tool runs: the loop takes it beside the tool, not after it.
            host.send(r#"{"type":"cancel","work":1,"reason":"operator stop"}"#)
                .unwrap();
            expect_type(host, "tool_report");
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["status"].clone(), done["summary"].clone()),
                (json!("stopped"), json!("operator stop"))
            );
            stop(host);
        });
    }

    #[test]
    fn a_chat_thread_keeps_its_conversation_verbatim_and_queued_work_runs_in_order() {
        let workspace = scratch("chat");
        let opaque = json!({"type": "opaque", "kind": "anthropic", "raw": {"type": "thinking", "signature": "sig"}});
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "chat", "room", "hello")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([opaque.clone(), text("hi there")]),
                "end_turn",
            ))
            .unwrap();
            expect_type(host, "work_done");

            host.send(&work(2, "chat", "room", "again")).unwrap();
            let second = expect_type(host, "model_call");
            let messages = &second["request"]["messages"];
            assert_eq!(messages[0]["content"][0]["text"], "hello");
            assert_eq!(
                messages[1]["content"],
                json!([opaque.clone(), text("hi there")])
            );
            assert_eq!(messages[2]["content"][0]["text"], "again");
            // Work 3 arrives while work 2 waits for its model answer, and runs after it.
            host.send(&answer(2, json!([text("again!")]), "end_turn"))
                .unwrap();
            host.send(&work(3, "task", "other", "queued")).unwrap();
            expect_type(host, "work_done");
            let third = expect_type(host, "model_call");
            assert_eq!(
                (
                    third["work"].clone(),
                    third["request"]["messages"][0]["content"][0]["text"].clone()
                ),
                (json!(3), json!("queued"))
            );
            host.send(&answer(3, json!([text("queued done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn heartbeats_go_out_while_a_model_call_is_pending() {
        let workspace = scratch("beats");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "wait")).unwrap();
            expect_type(host, "model_call");
            let mut busy_beats = 0;
            while busy_beats < 3 {
                let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
                if value["type"] == "heartbeat" && value["busy"] == true {
                    busy_beats += 1;
                }
            }
            host.send(&answer(1, json!([text("late")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            let mut idle_beat = false;
            while !idle_beat {
                let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
                idle_beat = value["type"] == "heartbeat" && value["busy"] == false;
            }
            stop(host);
        });
    }

    #[test]
    fn a_chat_thread_past_its_size_limit_opens_a_new_conversation_with_its_summary() {
        let workspace = scratch("close");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(
                1,
                "chat",
                "long",
                &"x".repeat(THREAD_CLOSE_BYTES + 1),
            ))
            .unwrap();
            expect_type(host, "model_call");
            host.send(&answer(1, json!([text("summary one")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            host.send(&work(2, "chat", "long", "second")).unwrap();
            let next = expect_type(host, "model_call");
            let messages = next["request"]["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 1, "{messages:?}");
            assert_eq!(
                messages[0]["content"][0]["text"],
                "Earlier in this conversation: summary one\n\nsecond"
            );
            host.send(&answer(2, json!([text("ok")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn an_operator_call_is_answered_while_the_agent_runs_a_local_tool() {
        let workspace = scratch("lane");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "wait")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("slow", "bash", json!({"command": "sleep 1"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_request");
            host.send(&decision("slow", "allow", "", false)).unwrap();
            // The local tool is sleeping now; the owner's call must not wait for it.
            host.send(r#"{"type":"tool_call","call":77,"name":"echo","args":{"text":"owner"}}"#)
                .unwrap();
            let first = next_msg(host);
            assert_eq!(
                (
                    first["type"].clone(),
                    first["call"].clone(),
                    first["output"].clone()
                ),
                (json!("tool_result"), json!(77), json!("owner")),
                "{first}"
            );
            expect_type(host, "tool_report");
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn operator_answers_and_heartbeats_never_interleave_on_the_wire() {
        let workspace = scratch("interleave");
        let padding = "x".repeat(400);
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            for call in 1..=200u64 {
                let text = format!("{call}-{padding}");
                let line = json!({"type": "tool_call", "call": call, "name": "echo", "args": {"text": text}});
                host.send(&line.to_string()).unwrap();
            }
            // Every answer arrives whole and in order: a torn line would fail to parse in `next_msg`.
            for call in 1..=200u64 {
                let answer = next_msg(host);
                assert_eq!(
                    (
                        answer["type"].clone(),
                        answer["call"].clone(),
                        answer["ok"].clone()
                    ),
                    (json!("tool_result"), json!(call), json!(true)),
                    "{answer}"
                );
            }
            stop(host);
        });
    }

    #[test]
    fn the_model_edit_arguments_map_onto_the_file_tools_names() {
        let mapped = edit_args(&json!({"path": "a.txt", "old": "x", "new": "y"})).unwrap();
        assert_eq!(
            (mapped["find"].clone(), mapped["replace"].clone()),
            (json!("x"), json!("y"))
        );
        assert!(
            edit_args(&json!({"path": "a.txt", "old": "x"}))
                .unwrap_err()
                .starts_with("invalid input:")
        );
    }
}
