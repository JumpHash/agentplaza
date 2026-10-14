//! `agentplaza stations screenshot` and `agentplaza stations watch`: a workstation's screen through the live view
//! (`WS /v1/stations/{id}/screen`, subprotocol `apz.station.screen.v1`, docs/design/workstations.md, section 7). A frame
//! is one binary message: a 4-byte big-endian header length, the JSON header `{"w","h","format","ts","seq"}` (format
//! `png` or `jpeg`), then the image. The hub's text messages are `started`, `ended`, `error` and `pong`.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use apz_http::ws::{self, Limits, Message, Socket};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, seg};

/// The subprotocol a live view asks for.
pub const PROTOCOL: &str = "apz.station.screen.v1";
/// The session a view names when none is given: the station's shared session, the owner's own screen.
pub const DEFAULT_SESSION: &str = "shared";
/// The socket limits of a view: a full-size frame is one message.
const LIMITS: Limits = Limits {
    max_frame: 16 << 20,
    max_message: 16 << 20,
    ping_every: Duration::from_secs(20),
    dead_after: Duration::from_secs(45),
};
/// How long a screenshot waits for its first frame.
const FIRST_FRAME: Duration = Duration::from_secs(20);
/// How long one wait for a message lasts; a quiet socket is polled again.
const WAIT: Duration = Duration::from_millis(500);

/// A frame's header, as the hub sends it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FrameHeader {
    pub w: u32,
    pub h: u32,
    pub format: String,
    pub ts: u64,
    pub seq: u64,
}

impl FrameHeader {
    /// The image's file extension.
    pub fn extension(&self) -> &'static str {
        if self.format == "png" { "png" } else { "jpg" }
    }
}

/// What a text message from the hub says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Started(u64),
    Ended(String),
    Error(String),
    Other,
}

/// One binary message split into its header and its image. A frame with no image, or with a format other than
/// `png` or `jpeg`, is refused.
pub fn parse_frame(bytes: &[u8]) -> Result<(FrameHeader, &[u8]), String> {
    if bytes.len() < 4 {
        return Err("a frame is shorter than its length prefix".into());
    }
    let head_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let image_at = 4usize
        .checked_add(head_len)
        .filter(|at| *at <= bytes.len())
        .ok_or("a frame's header is cut short")?;
    let header: FrameHeader = serde_json::from_slice(&bytes[4..image_at])
        .map_err(|e| format!("a frame's header is not its JSON: {e}"))?;
    if header.format != "png" && header.format != "jpeg" {
        return Err(format!(
            "a frame's format is {:?}, not png or jpeg",
            header.format
        ));
    }
    let image = &bytes[image_at..];
    if image.is_empty() {
        return Err("a frame has no image".into());
    }
    Ok((header, image))
}

/// A text message as the hub's notes read.
pub fn note(text: &str) -> Note {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Note::Other;
    };
    match value.get("t").and_then(Value::as_str) {
        Some("started") => Note::Started(value["session"].as_u64().unwrap_or(0)),
        Some("ended") => Note::Ended(ctx::text(&value, "reason")),
        Some("error") => Note::Error(ctx::text(&value, "error")),
        _ => Note::Other,
    }
}

/// The view's path: the session, and the frame rate and scale when given.
pub fn view_path(id: &str, session: &str, fps: Option<u8>, scale: Option<f32>) -> String {
    let mut path = format!("/v1/stations/{}/screen?session={}", seg(id), seg(session));
    if let Some(fps) = fps {
        path.push_str(&format!("&fps={fps}"));
    }
    if let Some(scale) = scale {
        path.push_str(&format!("&scale={scale}"));
    }
    path
}

/// Refuses a session that is neither `shared` nor a session number, before the socket opens.
pub fn check_session(session: &str) -> Result<(), String> {
    if session == "shared" || session.parse::<u64>().is_ok() {
        Ok(())
    } else {
        Err("the session is a session number or shared".into())
    }
}

/// `fps` from 1 to 15 (the hub's range).
pub fn check_fps(fps: u8) -> Result<(), String> {
    if (1..=15).contains(&fps) {
        Ok(())
    } else {
        Err("fps is a whole number from 1 to 15".into())
    }
}

/// `scale` from 0.1 to 1 (the hub's range).
pub fn check_scale(scale: f32) -> Result<(), String> {
    if scale.is_finite() && (0.1..=1.0).contains(&scale) {
        Ok(())
    } else {
        Err("scale is a number from 0.1 to 1".into())
    }
}

/// Frames per second over the last second or so: the frames counted since the window opened, over its length.
#[derive(Debug, Default)]
pub struct Meter {
    window: Option<Instant>,
    count: u32,
    fps: f64,
}

impl Meter {
    /// Counts one frame at `now` and returns the rate as it stands.
    pub fn tick(&mut self, now: Instant) -> f64 {
        let start = *self.window.get_or_insert(now);
        self.count += 1;
        let span = now.duration_since(start).as_secs_f64();
        if span >= 1.0 {
            self.fps = f64::from(self.count) / span;
            self.window = Some(now);
            self.count = 0;
        }
        self.fps
    }
}

/// The status line of a frame: its sequence number, the rate, the size and the image's size.
pub fn status_line(header: &FrameHeader, bytes: usize, fps: f64) -> String {
    format!(
        "seq {:>6}  {fps:>5.1} fps  {}x{}  {}  {}",
        header.seq,
        header.w,
        header.h,
        header.format,
        crate::cmd::lab::size_text(bytes as f64)
    )
}

fn connect(ctx: &Ctx, path: &str) -> Result<Socket, String> {
    ws::connect(
        &ctx.client.target,
        path,
        Some(PROTOCOL),
        Duration::from_secs(10),
        LIMITS,
    )
    .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))
}

/// `stations screenshot`: the first frame of the session, at `scale`, saved to `out` (overwritten).
pub fn screenshot(
    ctx: &Ctx,
    id: &str,
    session: &str,
    scale: f32,
    out: &Path,
) -> Result<(), String> {
    let mut socket = connect(ctx, &view_path(id, session, Some(1), Some(scale)))?;
    let frame = first_frame(&mut socket);
    socket.close(1000, "screenshot taken");
    let (header, image) = frame?;
    std::fs::write(out, &image).map_err(|e| format!("{}: {e}", out.display()))?;
    let saved = json!({
        "station": id, "session": session, "saved": out.display().to_string(), "bytes": image.len(),
        "w": header.w, "h": header.h, "format": header.format, "seq": header.seq
    });
    ctx.out(&saved, |_| {
        println!(
            "saved {} ({}x{} {}, {})",
            out.display(),
            header.w,
            header.h,
            header.format,
            crate::cmd::lab::size_text(image.len() as f64)
        )
    });
    Ok(())
}

/// The first frame the socket delivers, or why none came.
fn first_frame(socket: &mut Socket) -> Result<(FrameHeader, Vec<u8>), String> {
    let deadline = Instant::now() + FIRST_FRAME;
    while Instant::now() < deadline {
        match socket
            .recv(WAIT)
            .map_err(|e| format!("the screen link ended: {e}"))?
        {
            None => {}
            Some(Message::Binary(bytes)) => {
                let (header, image) = parse_frame(&bytes)?;
                return Ok((header, image.to_vec()));
            }
            Some(Message::Text(text)) => match note(&text) {
                Note::Error(error) => return Err(error),
                Note::Ended(reason) => {
                    return Err(format!("the session ended before a frame: {reason}"));
                }
                Note::Started(_) | Note::Other => {}
            },
        }
    }
    Err("no frame came in time".into())
}

/// `stations watch`: the live status of the session, one line a frame (on a terminal) or one a second (piped), and
/// with `save` every frame written into that folder. `--json` prints each frame's header as one line instead.
pub fn watch(
    ctx: &Ctx,
    id: &str,
    session: &str,
    fps: u8,
    save: Option<&Path>,
) -> Result<(), String> {
    if let Some(dir) = save {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut socket = connect(ctx, &view_path(id, session, Some(fps), None))?;
    let tty = std::io::stdout().is_terminal();
    let mut meter = Meter::default();
    let mut shown: Option<Instant> = None;
    loop {
        let Some(message) = socket
            .recv(WAIT)
            .map_err(|e| format!("the screen link ended: {e}"))?
        else {
            continue;
        };
        match message {
            Message::Binary(bytes) => {
                let (header, image) = parse_frame(&bytes)?;
                let now = Instant::now();
                let rate = meter.tick(now);
                if let Some(dir) = save {
                    let path = dir.join(format!("frame-{:06}.{}", header.seq, header.extension()));
                    std::fs::write(&path, image).map_err(|e| format!("{}: {e}", path.display()))?;
                }
                if ctx.json {
                    println!(
                        "{}",
                        json!({"seq": header.seq, "w": header.w, "h": header.h, "format": header.format, "ts": header.ts, "bytes": image.len()})
                    );
                } else if shown.is_none_or(|at| {
                    now.duration_since(at)
                        >= if tty {
                            Duration::from_millis(200)
                        } else {
                            Duration::from_secs(1)
                        }
                }) {
                    shown = Some(now);
                    let line = status_line(&header, image.len(), rate);
                    if tty {
                        print!("\r\x1b[2K{line}");
                        let _ = std::io::stdout().flush();
                    } else {
                        println!("{line}");
                    }
                }
            }
            Message::Text(text) => match note(&text) {
                Note::Started(session) => {
                    if !ctx.json {
                        println!("watching session {session} on {id}");
                    }
                }
                Note::Ended(reason) => {
                    if tty {
                        println!();
                    }
                    println!(
                        "session ended: {}",
                        if reason.is_empty() {
                            "no reason given"
                        } else {
                            reason.as_str()
                        }
                    );
                    return Ok(());
                }
                Note::Error(error) => return Err(error),
                Note::Other => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One binary frame as the hub sends it: the length, the header, the image.
    fn frame_bytes(header: &str, image: &[u8]) -> Vec<u8> {
        let mut out = (header.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(image);
        out
    }

    #[test]
    fn a_frame_splits_into_its_header_and_its_image() {
        let bytes = frame_bytes(
            r#"{"w":1280,"h":720,"format":"jpeg","ts":1700,"seq":9}"#,
            &[0xff, 0xd8, 0xff],
        );
        let (header, image) = parse_frame(&bytes).unwrap();
        assert_eq!(
            header,
            FrameHeader {
                w: 1280,
                h: 720,
                format: "jpeg".into(),
                ts: 1700,
                seq: 9
            }
        );
        assert_eq!(image, &[0xff, 0xd8, 0xff]);
        assert_eq!(header.extension(), "jpg");
        let png = frame_bytes(r#"{"w":2,"h":2,"format":"png","ts":0,"seq":1}"#, b"\x89PNG");
        assert_eq!(parse_frame(&png).unwrap().0.extension(), "png");
    }

    #[test]
    fn a_broken_frame_is_refused_with_its_reason() {
        assert!(parse_frame(&[0, 0]).unwrap_err().contains("length prefix"));
        let cut = [0, 0, 0, 90, b'{'];
        assert!(parse_frame(&cut).unwrap_err().contains("cut short"));
        let bad_json = frame_bytes("not json", b"x");
        assert!(parse_frame(&bad_json).unwrap_err().contains("not its JSON"));
        let gif = frame_bytes(r#"{"w":1,"h":1,"format":"gif","ts":0,"seq":1}"#, b"x");
        assert!(parse_frame(&gif).unwrap_err().contains("not png or jpeg"));
        let empty = frame_bytes(r#"{"w":1,"h":1,"format":"png","ts":0,"seq":1}"#, b"");
        assert!(parse_frame(&empty).unwrap_err().contains("no image"));
    }

    #[test]
    fn the_hubs_text_messages_read_as_notes() {
        assert_eq!(note(r#"{"t":"started","session":4}"#), Note::Started(4));
        assert_eq!(
            note(r#"{"t":"ended","reason":"owner"}"#),
            Note::Ended("owner".into())
        );
        assert_eq!(
            note(r#"{"t":"error","error":"no such session"}"#),
            Note::Error("no such session".into())
        );
        assert_eq!(note(r#"{"t":"pong"}"#), Note::Other);
        assert_eq!(note("garbage"), Note::Other);
    }

    #[test]
    fn the_view_path_names_the_session_and_what_it_asks() {
        assert_eq!(
            view_path("w1", "shared", Some(5), None),
            "/v1/stations/w1/screen?session=shared&fps=5"
        );
        assert_eq!(
            view_path("w1", "7", Some(1), Some(1.0)),
            "/v1/stations/w1/screen?session=7&fps=1&scale=1"
        );
        assert_eq!(DEFAULT_SESSION, "shared");
        assert!(check_session("shared").is_ok() && check_session("12").is_ok());
        assert!(check_session("nope").is_err());
        assert!(check_fps(15).is_ok() && check_fps(0).is_err() && check_fps(16).is_err());
        assert!(
            check_scale(0.1).is_ok() && check_scale(1.5).is_err() && check_scale(f32::NAN).is_err()
        );
    }

    #[test]
    fn the_meter_reports_frames_over_the_last_second() {
        let start = Instant::now();
        let mut meter = Meter::default();
        assert_eq!(meter.tick(start), 0.0, "one frame has no span yet");
        for step in 1..=4 {
            meter.tick(start + Duration::from_millis(step * 100));
        }
        // Six frames have arrived when the window's first second is up.
        let rate = meter.tick(start + Duration::from_millis(1_000));
        assert!(
            (rate - 6.0).abs() < 0.01,
            "six frames in one second: {rate}"
        );
    }

    #[test]
    fn the_status_line_has_the_sequence_the_rate_and_the_size() {
        let header = FrameHeader {
            w: 1920,
            h: 1080,
            format: "jpeg".into(),
            ts: 0,
            seq: 42,
        };
        let line = status_line(&header, 2048, 4.96);
        assert!(line.contains("seq     42"), "{line}");
        assert!(line.contains("  5.0 fps"), "{line}");
        assert!(line.contains("1920x1080  jpeg  2.0 KiB"), "{line}");
    }
}
