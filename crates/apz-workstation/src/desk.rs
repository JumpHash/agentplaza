//! The desktop lane: one thread that owns the desktop and serves every screen operation.
//!
//! The [`Desktop`] contract has every call made from one request loop, so this thread is that loop. Between
//! requests it does the periodic work too: it captures the streams that are due (one frame each, at its fps)
//! and asks the desktop whether the owner is using each shared session (a takeover is reported once, then
//! again only after a quiet spell).
//!
//! Coordinates: a screenshot or stream frame is scaled by a factor, and the hub's pointer and drag coordinates
//! are in that image's pixels. The session remembers the factor of the last image it sent and maps each point
//! back to screen points with it (`apz_desktop::map_point`). Without an image yet the factor is 1.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use apz_desktop::{Desktop, Frame, SessionSpec, encode, grid_overlay, map_point, scale};
use apz_station::{AppsOp, Event, ImageFormat, InputAction, Op, SessionId, SessionKind};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::daemon::{DeskMsg, FROZEN, Job, Shared};

/// How long the lane waits for a request before it does its periodic work.
const TICK: Duration = Duration::from_millis(50);
/// How often the owner's activity is checked.
const OWNER_POLL: Duration = Duration::from_millis(250);
/// After an owner's input is reported, a continuing takeover is reported again after this.
const OWNER_REPEAT: Duration = Duration::from_secs(2);
/// The fastest a stream may run.
pub const MAX_FPS: u8 = 10;
/// The longest side of a stream frame.
const STREAM_SIDE: u32 = 1280;
const STREAM_QUALITY: u8 = 70;
const SHOT_QUALITY: u8 = 85;
/// The spacing of the coordinate grid, in screenshot pixels.
pub const GRID_STEP: u32 = 100;
/// The session size when the host reports no display.
const FALLBACK_SIZE: (u32, u32) = (1280, 720);
/// How often the applications are counted again.
const APPS_EVERY: Duration = Duration::from_secs(300);

/// Starts the desktop lane on `desktop`, reading jobs from `rx`.
pub fn spawn(
    desktop: Box<dyn Desktop>,
    shared: Arc<Shared>,
    rx: Receiver<DeskMsg>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut lane = Lane {
            desktop,
            shared,
            sessions: BTreeMap::new(),
            primary: None,
        };
        lane.run(rx);
    })
}

struct Lane {
    desktop: Box<dyn Desktop>,
    shared: Arc<Shared>,
    sessions: BTreeMap<SessionId, Session>,
    /// The size a new session takes: the first display the host reports.
    primary: Option<(u32, u32)>,
}

struct Session {
    kind: SessionKind,
    /// The factor of the last image sent for this session (see the module docs).
    scale: f32,
    stream: Option<Stream>,
    seq: u64,
    owner_busy: bool,
    owner_reported: Option<Instant>,
}

struct Stream {
    interval: Duration,
    next: Instant,
}

impl Session {
    fn new(kind: SessionKind) -> Self {
        Session {
            kind,
            scale: 1.0,
            stream: None,
            seq: 0,
            owner_busy: false,
            owner_reported: None,
        }
    }
}

impl Lane {
    fn run(&mut self, rx: Receiver<DeskMsg>) {
        self.count_apps();
        let mut next_owner_poll = Instant::now();
        let mut next_apps = Instant::now() + APPS_EVERY;
        loop {
            match rx.recv_timeout(TICK) {
                Ok(DeskMsg::Job(job)) => self.job(job),
                Ok(DeskMsg::StopAll) => self.stop_all(),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let now = Instant::now();
            self.streams(now);
            if now >= next_owner_poll {
                next_owner_poll = now + OWNER_POLL;
                self.poll_owners(now);
            }
            if now >= next_apps {
                next_apps = now + APPS_EVERY;
                self.count_apps();
            }
        }
    }

    fn job(&mut self, job: Job) {
        let outcome = self.handle(job.op);
        self.shared.uplink.respond(job.generation, job.id, outcome);
    }

    fn handle(&mut self, op: Op) -> Result<Option<Value>, String> {
        match op {
            Op::SessionOpen { agent, kind } => self.open(agent, kind),
            Op::SessionClose { session } => self.close(session),
            Op::Screenshot {
                session,
                scale: factor,
                grid,
                format,
            } => self.screenshot(session, factor, grid, format),
            Op::Input { session, action } => self.input(session, &action),
            Op::UiTree {
                session,
                app,
                depth,
            } => {
                self.live(session)?;
                let node = self
                    .desktop
                    .ui_tree(session, app.as_deref(), depth)
                    .map_err(text)?;
                Ok(Some(value(&node)?))
            }
            Op::UiAct {
                session,
                selector,
                action,
            } => {
                self.live(session)?;
                self.desktop
                    .ui_act(session, &selector, &action)
                    .map_err(text)?;
                Ok(None)
            }
            Op::Apps(apps) => self.apps(apps),
            Op::Stream { session, fps, on, scale: _ } => self.stream(session, fps, on),
            other => Err(format!("{:?} is not a screen operation", op_kind(&other))),
        }
    }

    fn open(&mut self, agent: u64, kind: SessionKind) -> Result<Option<Value>, String> {
        self.refuse_frozen()?;
        let (width, height) = self.primary_size();
        let id = self
            .desktop
            .open_session(&SessionSpec {
                agent,
                kind,
                width,
                height,
            })
            .map_err(text)?;
        self.sessions.insert(id, Session::new(kind));
        Ok(Some(json!({ "session": id })))
    }

    fn close(&mut self, session: SessionId) -> Result<Option<Value>, String> {
        self.desktop.close_session(session).map_err(text)?;
        self.sessions.remove(&session);
        self.shared.uplink.event(Event::SessionEnded {
            session,
            reason: "closed".to_string(),
        });
        Ok(None)
    }

    fn screenshot(
        &mut self,
        session: SessionId,
        factor: f32,
        grid: bool,
        format: ImageFormat,
    ) -> Result<Option<Value>, String> {
        self.live(session)?;
        if !(factor.is_finite() && factor > 0.0 && factor <= 1.0) {
            return Err(format!("a screenshot's scale is in (0, 1], not {factor}"));
        }
        let frame = self.desktop.capture(session).map_err(text)?;
        let small = scale(&frame, factor).map_err(text)?;
        let shown = if grid {
            grid_overlay(&small, GRID_STEP).map_err(text)?
        } else {
            small
        };
        let bytes = encode(&shown, format, SHOT_QUALITY).map_err(text)?;
        self.set_scale(session, factor);
        Ok(Some(json!({
            "w": shown.w,
            "h": shown.h,
            "format": format_name(format),
            "data_b64": STANDARD.encode(bytes),
        })))
    }

    fn input(&mut self, session: SessionId, action: &InputAction) -> Result<Option<Value>, String> {
        self.live(session)?;
        let factor = self.sessions.get(&session).map_or(1.0, |s| s.scale);
        let mapped = map_input(action, factor);
        self.desktop.input(session, &mapped).map_err(text)?;
        Ok(None)
    }

    fn apps(&mut self, op: AppsOp) -> Result<Option<Value>, String> {
        match op {
            AppsOp::List => {
                let apps = self.desktop.apps().map_err(text)?;
                self.publish_apps(apps.len());
                Ok(Some(value(&apps)?))
            }
            AppsOp::Launch { app, args } => {
                self.refuse_frozen()?;
                let session = self
                    .sessions
                    .keys()
                    .next()
                    .copied()
                    .ok_or("open a screen session first: an app launches in one")?;
                self.desktop.launch(session, &app, &args).map_err(text)?;
                Ok(None)
            }
            AppsOp::Focus { app } => {
                self.refuse_frozen()?;
                self.desktop.focus(&app).map_err(text)?;
                Ok(None)
            }
            AppsOp::Quit { app, force } => {
                self.refuse_frozen()?;
                self.desktop.quit(&app, force).map_err(text)?;
                Ok(None)
            }
        }
    }

    fn stream(&mut self, session: SessionId, fps: u8, on: bool) -> Result<Option<Value>, String> {
        self.live(session)?;
        let entry = self
            .sessions
            .get_mut(&session)
            .ok_or_else(|| format!("no session {session}"))?;
        if on {
            if fps == 0 || fps > MAX_FPS {
                return Err(format!("a stream runs at 1 to {MAX_FPS} frames a second"));
            }
            entry.stream = Some(Stream {
                interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
                next: Instant::now(),
            });
        } else {
            entry.stream = None;
        }
        Ok(None)
    }

    /// STOP ALL: the pixel sessions end (the owner's shared sessions stay), and every stream stops.
    fn stop_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.stream = None;
        }
        let pixel: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.kind == SessionKind::Pixel)
            .map(|(id, _)| *id)
            .collect();
        for id in pixel {
            let _ = self.desktop.close_session(id);
            self.sessions.remove(&id);
            self.shared.uplink.event(Event::SessionEnded {
                session: id,
                reason: "stop_all".to_string(),
            });
        }
    }

    /// Captures each stream whose frame is due.
    fn streams(&mut self, now: Instant) {
        let due: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.stream.as_ref().is_some_and(|st| now >= st.next))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            match self.stream_frame(id) {
                Ok(()) => {
                    if let Some(stream) = self.sessions.get_mut(&id).and_then(|s| s.stream.as_mut())
                    {
                        stream.next = now + stream.interval;
                    }
                }
                Err(err) => {
                    self.shared
                        .log
                        .warn(format!("stream of session {id} stopped: {err}"));
                    if let Some(session) = self.sessions.get_mut(&id) {
                        session.stream = None;
                    }
                }
            }
        }
    }

    fn stream_frame(&mut self, id: SessionId) -> Result<(), String> {
        let frame = self.desktop.capture(id).map_err(text)?;
        let factor = fit_factor(&frame, STREAM_SIDE);
        let small = if factor < 1.0 {
            scale(&frame, factor).map_err(text)?
        } else {
            frame.clone()
        };
        let bytes = encode(&small, ImageFormat::Jpeg, STREAM_QUALITY).map_err(text)?;
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| format!("no session {id}"))?;
        session.seq += 1;
        session.scale = small.w as f32 / frame.w as f32;
        self.shared.uplink.event(Event::Frame {
            session: id,
            seq: session.seq,
            w: small.w,
            h: small.h,
            format: ImageFormat::Jpeg,
            data_b64: STANDARD.encode(bytes),
        });
        Ok(())
    }

    /// Reports the owner's activity on each shared session: once when it starts, then again after a quiet spell.
    fn poll_owners(&mut self, now: Instant) {
        let shared: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.kind == SessionKind::Shared)
            .map(|(id, _)| *id)
            .collect();
        for id in shared {
            let busy = self.desktop.owner_active(id);
            let Some(session) = self.sessions.get_mut(&id) else {
                continue;
            };
            let report = busy
                && match session.owner_reported {
                    None => true,
                    Some(at) => !session.owner_busy || now.duration_since(at) >= OWNER_REPEAT,
                };
            session.owner_busy = busy;
            if report {
                session.owner_reported = Some(now);
                self.shared.uplink.event(Event::OwnerInput { session: id });
            }
        }
    }

    fn count_apps(&mut self) {
        if let Ok(apps) = self.desktop.apps() {
            self.publish_apps(apps.len());
        }
    }

    /// Records how many applications the desktop reports. The fake desktop's list is invented, so it is not
    /// published: a build with no real backend reports no applications.
    fn publish_apps(&self, len: usize) {
        if self.shared.backend != Backend::Fake {
            self.shared.apps.store(count(len), Ordering::SeqCst);
        }
    }

    /// The size a new session takes: the host's first display, else a fallback.
    fn primary_size(&mut self) -> (u32, u32) {
        if let Some(size) = self.primary {
            return size;
        }
        let apps = self.shared.apps.load(Ordering::SeqCst);
        let size = (self.shared.gather)(apps)
            .displays
            .first()
            .map(|d| (d.w, d.h))
            .filter(|(w, h)| *w > 0 && *h > 0)
            .unwrap_or(FALLBACK_SIZE);
        self.primary = Some(size);
        size
    }

    fn set_scale(&mut self, session: SessionId, factor: f32) {
        if let Some(entry) = self.sessions.get_mut(&session) {
            entry.scale = factor;
        }
    }

    /// Refuses under STOP ALL.
    fn refuse_frozen(&self) -> Result<(), String> {
        if self.shared.frozen.load(Ordering::SeqCst) {
            return Err(FROZEN.to_string());
        }
        Ok(())
    }

    /// Refuses under STOP ALL, and refuses a session that is not open.
    fn live(&self, session: SessionId) -> Result<(), String> {
        self.refuse_frozen()?;
        if self.sessions.contains_key(&session) {
            Ok(())
        } else {
            Err(format!("no session {session}"))
        }
    }
}

/// Maps the points of an input action from image pixels to screen points with `factor`. Scroll amounts are
/// line counts, not points, so they pass unchanged.
pub fn map_input(action: &InputAction, factor: f32) -> InputAction {
    match action {
        InputAction::Move { x, y } => {
            let (x, y) = map_point(*x, *y, factor);
            InputAction::Move { x, y }
        }
        InputAction::Click {
            x,
            y,
            button,
            count,
        } => {
            let (x, y) = map_point(*x, *y, factor);
            InputAction::Click {
                x,
                y,
                button: *button,
                count: *count,
            }
        }
        InputAction::Scroll { x, y, dx, dy } => {
            let (x, y) = map_point(*x, *y, factor);
            InputAction::Scroll {
                x,
                y,
                dx: *dx,
                dy: *dy,
            }
        }
        InputAction::Drag { from, to } => InputAction::Drag {
            from: map_point(from.0, from.1, factor),
            to: map_point(to.0, to.1, factor),
        },
        other => other.clone(),
    }
}

/// The factor that brings the longer side of a frame to `side` (1 when it is already no longer).
fn fit_factor(frame: &Frame, side: u32) -> f32 {
    let longest = frame.w.max(frame.h);
    if longest <= side {
        1.0
    } else {
        side as f32 / longest as f32
    }
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
    }
}

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

fn text(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn value<T: Serialize>(item: &T) -> Result<Value, String> {
    serde_json::to_value(item).map_err(text)
}

/// The operation's name, for an error about the wrong lane.
fn op_kind(op: &Op) -> &'static str {
    match op {
        Op::Facts => "facts",
        Op::Files(_) => "files",
        Op::Term(_) => "term",
        Op::Run(_) => "run",
        Op::Script { .. } => "script",
        Op::Settings(_) => "settings",
        Op::Sync(_) => "sync",
        Op::Freeze { .. } => "freeze",
        _ => "operation",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::Button;

    #[test]
    fn a_click_in_a_half_scale_image_lands_at_twice_the_point() {
        assert_eq!(
            map_input(
                &InputAction::Click {
                    x: 200,
                    y: 100,
                    button: Button::Left,
                    count: 2,
                },
                0.5,
            ),
            InputAction::Click {
                x: 400,
                y: 200,
                button: Button::Left,
                count: 2,
            }
        );
    }

    #[test]
    fn drags_map_both_ends_and_scrolls_keep_their_amounts() {
        assert_eq!(
            map_input(
                &InputAction::Drag {
                    from: (10, 20),
                    to: (30, 40),
                },
                0.5,
            ),
            InputAction::Drag {
                from: (20, 40),
                to: (60, 80),
            }
        );
        assert_eq!(
            map_input(
                &InputAction::Scroll {
                    x: 50,
                    y: 50,
                    dx: 0,
                    dy: -3,
                },
                0.5,
            ),
            InputAction::Scroll {
                x: 100,
                y: 100,
                dx: 0,
                dy: -3,
            }
        );
    }

    #[test]
    fn keys_and_text_are_not_mapped() {
        let key = InputAction::Key {
            combo: "ctrl+s".into(),
        };
        assert_eq!(map_input(&key, 0.25), key);
        let typed = InputAction::Type { text: "hi".into() };
        assert_eq!(map_input(&typed, 0.25), typed);
    }

    #[test]
    fn a_frame_fits_the_stream_side() {
        let big = Frame::new(2560, 1440, vec![0; 2560 * 1440 * 4]).unwrap();
        assert_eq!(fit_factor(&big, 1280), 0.5);
        let small = Frame::new(640, 360, vec![0; 640 * 360 * 4]).unwrap();
        assert_eq!(fit_factor(&small, 1280), 1.0);
    }
}
