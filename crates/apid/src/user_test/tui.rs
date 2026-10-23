//! The terminal target (PLAN 19.3): a program in a pseudo-terminal.
//!
//! - The tree is the terminal's text grid (`apz-vt`): each non-blank row is a text element, and the terminal is one
//!   textbox ([`TERMINAL`]) whose value is the cursor's row.
//! - The input is the keystrokes: a `type` writes its text to the terminal, a `press` writes the key's bytes.
//! - The picture is the grid rendered to a PNG. Glyphs are drawn as blocks: no font is vendored, so a checker reads the
//!   tree where there is one.
//!
//! The pseudo-terminal is opened here (`openpty`), the program is started as a session leader on its slave (so the
//! terminal is its controlling one), and a reader thread feeds the program's output to the emulator. The program is
//! killed when the target drops.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_vt::{Color, Emulator, Row, Screen};

use super::target::{Action, Element, Screenshot, Target, Tree};

/// The id and the accessible name of the terminal's input: what a driver types into.
pub const TERMINAL: &str = "terminal";
/// The output is settled once it has been quiet this long (and at least this long has passed since the input).
const QUIET: Duration = Duration::from_millis(120);
/// The longest an action waits for the program's output.
const MAX_SETTLE: Duration = Duration::from_millis(1500);
/// A cell of the picture, in pixels.
const CELL_W: usize = 8;
const CELL_H: usize = 16;
const FG: [u8; 3] = [220, 220, 220];
const BG: [u8; 3] = [18, 18, 18];
/// The eight basic colours, for the indexed palette's first eight entries.
const BASIC: [[u8; 3]; 8] = [
    [0, 0, 0],
    [205, 49, 49],
    [13, 188, 121],
    [229, 229, 16],
    [36, 114, 200],
    [188, 63, 188],
    [17, 168, 205],
    [229, 229, 229],
];

/// A program running in a pseudo-terminal, with its screen.
pub struct Tui {
    emu: Arc<Mutex<Emulator>>,
    /// When the program last wrote.
    touched: Arc<Mutex<Instant>>,
    /// The master side: input goes in here, output comes out of the reader thread's duplicate.
    master: Arc<File>,
    child: Child,
    started: Instant,
}

impl Tui {
    /// Starts `command` (its first item the program, the rest its arguments) in `cwd`, on a terminal of `cols` by `rows`.
    pub fn open(command: &[String], cwd: &Path, cols: u16, rows: u16) -> Result<Tui, String> {
        let (program, args) = command
            .split_first()
            .ok_or("a terminal target names a program to run")?;
        let (master, slave) = open_pty(cols, rows)?;
        let slave_in = slave
            .try_clone()
            .map_err(|e| format!("the terminal's slave: {e}"))?;
        let slave_out = slave
            .try_clone()
            .map_err(|e| format!("the terminal's slave: {e}"))?;
        let mut process = Command::new(program);
        process
            .args(args)
            .current_dir(cwd)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave_in))
            .stdout(Stdio::from(slave_out))
            .stderr(Stdio::from(slave));
        // SAFETY: between fork and exec this makes the child a session leader and takes its stdin (the terminal's
        // slave) as its controlling terminal. Both calls are async-signal-safe and touch no memory of the parent.
        unsafe {
            process.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = process
            .spawn()
            .map_err(|e| format!("{program} did not start: {e}"))?;
        drop(process);

        let master = Arc::new(File::from(master));
        let emu = Arc::new(Mutex::new(Emulator::new(cols as usize, rows as usize)));
        let touched = Arc::new(Mutex::new(Instant::now()));
        let mut reader = master
            .try_clone()
            .map_err(|e| format!("the terminal's master: {e}"))?;
        let (emu_r, touched_r, writer) =
            (Arc::clone(&emu), Arc::clone(&touched), Arc::clone(&master));
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // The program's queries (cursor reports, device attributes) are answered on the same channel.
                        let replies = {
                            let mut emu = lock(&emu_r);
                            emu.feed(&buf[..n]);
                            emu.screen_mut().take_replies()
                        };
                        *lock(&touched_r) = Instant::now();
                        if !replies.is_empty() && (&*writer).write_all(&replies).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    // The program has exited: its slave is closed, and the read fails.
                    Err(_) => break,
                }
            }
        });
        Ok(Tui {
            emu,
            touched,
            master,
            child,
            started: Instant::now(),
        })
    }

    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        (&*self.master)
            .write_all(bytes)
            .map_err(|e| format!("the terminal's input: {e}"))
    }

    /// Waits for the program to answer the input: until its output has been quiet, or the longest wait has passed.
    fn settle(&self) {
        let start = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(20));
            let quiet = lock(&self.touched).elapsed() >= QUIET;
            if (quiet && start.elapsed() >= QUIET) || start.elapsed() >= MAX_SETTLE {
                break;
            }
        }
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Target for Tui {
    fn kind(&self) -> &str {
        "tui"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let emu = lock(&self.emu);
        let screen = emu.screen();
        let rows: Vec<String> = (0..screen.rows())
            .map(|y| row_text(screen.view_row(y)))
            .collect();
        let title = if screen.title().trim().is_empty() {
            rows.iter()
                .find(|r| !r.trim().is_empty())
                .map(|r| r.trim().to_string())
                .unwrap_or_default()
        } else {
            screen.title().trim().to_string()
        };
        let (cursor_row, _) = screen.cursor();
        let mut elements: Vec<Element> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.trim().is_empty())
            .map(|(y, r)| Element {
                id: format!("row-{y}"),
                role: "text".into(),
                label: r.trim_end().to_string(),
                value: String::new(),
            })
            .collect();
        elements.push(Element {
            id: TERMINAL.into(),
            role: "textbox".into(),
            label: "Terminal".into(),
            value: rows
                .get(cursor_row)
                .map(|r| r.trim_end().to_string())
                .unwrap_or_default(),
        });
        Ok(Tree { title, elements })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let emu = lock(&self.emu);
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes: render(emu.screen())?,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        match action {
            // Focusing the terminal is the only pointer action: it has one input, which is always focused.
            Action::Click { element } if element == TERMINAL => return Ok(()),
            Action::Click { element } => {
                return Err(format!(
                    "no element {element} on the terminal: a terminal takes keys and text"
                ));
            }
            Action::Type { element, text } if element == TERMINAL => {
                self.write(text.as_bytes())?;
            }
            Action::Type { element, .. } => {
                return Err(format!("{element} is not on the terminal"));
            }
            Action::Press { key } => self.write(&key_bytes(key)?)?,
        }
        self.settle();
        Ok(())
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.started.elapsed().as_millis() as u64)
    }
}

/// The bytes a key sends to a terminal: the named keys, a `Ctrl+` letter, or a single character.
pub fn key_bytes(key: &str) -> Result<Vec<u8>, String> {
    let named: &[u8] = match key {
        "Enter" => b"\r",
        "Tab" => b"\t",
        "Escape" | "Esc" => b"\x1b",
        "Backspace" => b"\x7f",
        "ArrowUp" => b"\x1b[A",
        "ArrowDown" => b"\x1b[B",
        "ArrowRight" => b"\x1b[C",
        "ArrowLeft" => b"\x1b[D",
        other => {
            if let Some(letter) = other.strip_prefix("Ctrl+")
                && let [c] = letter.as_bytes()
                && c.is_ascii_alphabetic()
            {
                return Ok(vec![c.to_ascii_lowercase() - b'a' + 1]);
            }
            let mut chars = other.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c.to_string().into_bytes()),
                _ => Err(format!("the terminal does not know the key {other}")),
            };
        }
    };
    Ok(named.to_vec())
}

/// The text of one row of the grid, its wide characters' second cells left out.
fn row_text(row: Option<&Row>) -> String {
    row.map(|r| {
        r.cells
            .iter()
            .filter(|c| c.width > 0)
            .map(|c| c.ch)
            .collect::<String>()
    })
    .unwrap_or_default()
}

/// The grid as a PNG: a cell's background, and a block in its foreground where it holds a character.
pub fn render(screen: &Screen) -> Result<Vec<u8>, String> {
    let (cols, rows) = (screen.cols(), screen.rows());
    let (width, height) = (cols * CELL_W, rows * CELL_H);
    let mut pixels = vec![0u8; width * height * 3];
    for y in 0..rows {
        let row = screen.view_row(y);
        for x in 0..cols {
            let cell = row
                .and_then(|r| r.cells.get(x))
                .copied()
                .unwrap_or_default();
            let mut fg = colour(cell.style.fg, FG);
            let mut bg = colour(cell.style.bg, BG);
            if cell.style.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            let inked = cell.ch != ' ' && cell.ch != '\0';
            for py in 0..CELL_H {
                for px in 0..CELL_W {
                    let ink =
                        inked && (2..CELL_W - 2).contains(&px) && (4..CELL_H - 4).contains(&py);
                    let at = ((y * CELL_H + py) * width + x * CELL_W + px) * 3;
                    pixels[at..at + 3].copy_from_slice(if ink { &fg } else { &bg });
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("the terminal's picture header: {e}"))?;
    writer
        .write_image_data(&pixels)
        .map_err(|e| format!("the terminal's picture data: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("the terminal's picture end: {e}"))?;
    Ok(out)
}

fn colour(color: Color, default: [u8; 3]) -> [u8; 3] {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) if (i as usize) < BASIC.len() => BASIC[i as usize],
        Color::Indexed(i) => {
            let grey = i.wrapping_mul(7);
            [grey, grey, grey]
        }
    }
}

/// A pseudo-terminal of `cols` by `rows`: its master and its slave.
fn open_pty(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd), String> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes one new descriptor through each out-pointer (live locals) and reads the size (a live
    // local); the name and the attributes are null, which it accepts.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if rc != 0 {
        return Err(format!(
            "no pseudo-terminal: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: openpty succeeded, so both descriptors are open and owned by nothing else.
    Ok(unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    /// Polls the tree until it shows `text`, or gives up after five seconds.
    fn until_shows(tui: &mut Tui, text: &str) -> Tree {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let tree = tui.tree().unwrap();
            if tree.shows(text) || Instant::now() >= deadline {
                return tree;
            }
            thread::sleep(Duration::from_millis(30));
        }
    }

    #[test]
    fn keys_are_the_bytes_a_terminal_reads() {
        assert_eq!(key_bytes("Enter").unwrap(), b"\r");
        assert_eq!(key_bytes("Ctrl+c").unwrap(), vec![3]);
        assert_eq!(key_bytes("x").unwrap(), b"x");
        assert!(key_bytes("Hyper").is_err());
    }

    #[test]
    fn a_picture_of_the_grid_is_a_png_of_its_size() {
        let emu = Emulator::new(10, 3);
        let png = render(emu.screen()).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        // The IHDR chunk's width and height follow its tag at byte 16.
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((width as usize, height as usize), (10 * CELL_W, 3 * CELL_H));
    }

    #[test]
    fn a_program_in_a_pty_shows_its_output_and_takes_its_keys() {
        let cwd = std::env::current_dir().unwrap();
        let mut tui = Tui::open(
            &sh("echo hello from the pty; read line; echo got $line"),
            &cwd,
            80,
            24,
        )
        .expect("the program starts in a pty");
        assert_eq!(tui.kind(), "tui");
        let shown = until_shows(&mut tui, "hello from the pty");
        assert_eq!(shown.title, "hello from the pty");
        assert!(shown.elements.iter().any(|e| e.id == TERMINAL));
        tui.act(&Action::Type {
            element: TERMINAL.into(),
            text: "abc".into(),
        })
        .unwrap();
        tui.act(&Action::Press {
            key: "Enter".into(),
        })
        .unwrap();
        assert!(until_shows(&mut tui, "got abc").shows("got abc"));
        let shot = tui.screenshot().unwrap();
        assert!(shot.bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(tui.clock_ms().is_some());
        assert!(
            tui.act(&Action::Click {
                element: "row-0".into()
            })
            .is_err()
        );
    }
}
