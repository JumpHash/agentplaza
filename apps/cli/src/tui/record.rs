//! `APZ_TUI_RECORD=<script.json>`: runs the terminal UI off-screen against the hub, plays the
//! script's keys and waits, and writes what it drew as `<out>.svg`, an animated terminal window
//! whose frames loop with CSS keyframes (no script, so GitHub's README renderer plays it), and
//! `<out>-poster.svg`, one frame as a still. Identical frames are merged into one longer frame and
//! identical rows are written once. Box-drawing and block characters are drawn as geometry (see
//! `cell_geometry`), not text, so their joins are exact; the text is set in the embedded APZ Mono
//! font, which `scripts/readme/tui-font.py` subsets into the `/*APZ-FONT*/` placeholder.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::Value;

use super::app::App;
use crate::ctx::Client;

/// Pixels per terminal cell (a 13px monospace font advances 0.6em) and per row.
const CELL_W: f64 = 7.8;
const ROW_H: f64 = 17.0;
/// The window: a title bar, the screen below it at (PAD, TOP), and the bottom margin.
const BAR_H: f64 = 30.0;
const PAD: f64 = 16.0;
const TOP: f64 = BAR_H + 8.0;
const BOTTOM: f64 = 12.0;
/// The baseline of a row's text, from the row's top.
const BASE_Y: f64 = 13.4;

/// The Loom dark colours: the window, its title bar and border, the title text and the ink.
const INK: u32 = 0xece8df;
const GROUND: u32 = 0x121110;
const BAR: u32 = 0x1c1b18;
const EDGE: u32 = 0x2a2824;
const TITLE: u32 = 0x958f83;
/// What a wide glyph's second cell holds: zero-width, so each cell is one character of a run.
const CONT: char = '\u{200b}';
/// The embedded face first; the stack after it is what a renderer without the font uses.
const FONT: &str = r#""APZ Mono","JetBrains Mono","SF Mono",Menlo,Consolas,monospace"#;

/// The named colours on the dark ground, in ANSI order (the eight normal, then the eight bright).
/// Red, Green and the greys follow the terminal UI's own palette.
const NAMED: [u32; 16] = [
    0x2b2e35, 0xe5534b, 0x2f9e44, 0xd9a21b, 0x4f8cf0, 0xb98cdb, 0x3bb6c8, 0xa0a4ab, 0x868e96,
    0xff7b72, 0x56c26b, 0xf0c040, 0x7aa7ff, 0xd2a8ff, 0x6fd3e0, 0xe8e6e1,
];

/// The script: the window size and frame rate, the title, the seconds of polling before the first
/// frame, the steps, which frame is the poster (negative counts from the end), and the output path
/// without extension.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub title: String,
    pub warmup: f32,
    pub steps: Vec<Step>,
    pub poster: i64,
    pub out: String,
}

/// One step: a key press (repeated, `every` seconds apart), typed text (`every` seconds per
/// character), or a wait.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Wait(f32),
    Key {
        press: KeyEvent,
        repeat: u32,
        every: f32,
    },
    Type {
        text: String,
        every: f32,
    },
}

/// The colour of one cell as the recording draws it: concrete RGB (the ground and the ink stand
/// in for the terminal's defaults) and the modifiers that change the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: u32,
    /// None where the ground shows through (no background rectangle).
    pub bg: Option<u32>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// A run of cells with one style: `cells` columns from `col`, holding `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub col: u16,
    pub cells: u16,
    pub text: String,
    pub style: Style,
}

pub type Row = Vec<Run>;

/// One merged frame: the rows drawn, and how many captured frames it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub rows: Vec<Row>,
    pub ticks: u32,
}

/// Reads a script (`default_out` is used when it names no `out`).
pub fn parse(text: &str, default_out: &str) -> Result<Script, String> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| format!("the script is not JSON: {e}"))?;
    let steps = match doc.get("steps") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(n, item)| parse_step(item, n + 1))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("steps must be a list".into()),
    };
    let fps = field_f64(&doc, "fps", 12.0)?.clamp(1.0, 30.0);
    Ok(Script {
        width: field_f64(&doc, "width", 120.0)?.clamp(20.0, 300.0) as u16,
        height: field_f64(&doc, "height", 34.0)?.clamp(5.0, 120.0) as u16,
        fps: fps as u32,
        title: doc
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("agentplaza tui")
            .to_string(),
        warmup: field_f64(&doc, "warmup", 3.0)?.max(0.0) as f32,
        steps,
        poster: doc.get("poster").and_then(Value::as_i64).unwrap_or(-1),
        out: doc
            .get("out")
            .and_then(Value::as_str)
            .map_or_else(|| default_out.to_string(), str::to_string),
    })
}

fn field_f64(doc: &Value, key: &str, default: f64) -> Result<f64, String> {
    match doc.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .ok_or_else(|| format!("\"{key}\" must be a number")),
    }
}

fn parse_step(v: &Value, n: usize) -> Result<Step, String> {
    let fail = |why: &str| format!("step {n}: {why}");
    if let Some(secs) = v.get("wait") {
        let secs = secs.as_f64().ok_or_else(|| fail("wait takes seconds"))?;
        return Ok(Step::Wait(secs.max(0.0) as f32));
    }
    if let Some(text) = v.get("type").and_then(Value::as_str) {
        return Ok(Step::Type {
            text: text.to_string(),
            every: field_f64(v, "every", 0.08).map_err(|e| fail(&e))?.max(0.0) as f32,
        });
    }
    let press = if let Some(name) = v.get("key").and_then(Value::as_str) {
        let code = key_code(name).ok_or_else(|| fail(&format!("unknown key {name:?}")))?;
        KeyEvent::new(code, KeyModifiers::NONE)
    } else if let Some(name) = v.get("ctrl").and_then(Value::as_str) {
        let mut chars = name.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), KeyModifiers::CONTROL)
            }
            _ => return Err(fail("ctrl takes one character")),
        }
    } else {
        return Err(fail("expected key, ctrl, type or wait"));
    };
    let repeat = v
        .get("repeat")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 500) as u32;
    Ok(Step::Key {
        press,
        repeat,
        every: field_f64(v, "every", 0.25).map_err(|e| fail(&e))?.max(0.0) as f32,
    })
}

/// The crossterm key for a script's key name (see the module docs for the names).
fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "Space" => KeyCode::Char(' '),
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
    })
}

fn pack(r: u32, g: u32, b: u32) -> u32 {
    r << 16 | g << 8 | b
}

/// The xterm-256 palette: the named colours, the 6×6×6 cube and the grey ramp.
pub fn indexed(i: u8) -> u32 {
    match i {
        0..=15 => NAMED[usize::from(i)],
        16..=231 => {
            const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
            let n = usize::from(i - 16);
            pack(LEVELS[n / 36], LEVELS[(n / 6) % 6], LEVELS[n % 6])
        }
        232..=255 => {
            let v = 8 + 10 * u32::from(i - 232);
            pack(v, v, v)
        }
    }
}

/// A ratatui colour as RGB; `default` stands for Reset.
pub fn color_rgb(color: Color, default: u32) -> u32 {
    match color {
        Color::Reset => default,
        Color::Rgb(r, g, b) => pack(u32::from(r), u32::from(g), u32::from(b)),
        Color::Indexed(i) => indexed(i),
        Color::Black => NAMED[0],
        Color::Red => NAMED[1],
        Color::Green => NAMED[2],
        Color::Yellow => NAMED[3],
        Color::Blue => NAMED[4],
        Color::Magenta => NAMED[5],
        Color::Cyan => NAMED[6],
        Color::Gray => NAMED[7],
        Color::DarkGray => NAMED[8],
        Color::LightRed => NAMED[9],
        Color::LightGreen => NAMED[10],
        Color::LightYellow => NAMED[11],
        Color::LightBlue => NAMED[12],
        Color::LightMagenta => NAMED[13],
        Color::LightCyan => NAMED[14],
        Color::White => NAMED[15],
    }
}

/// The style a cell is drawn with: REVERSED swaps the colours, HIDDEN paints the text in the ground.
pub fn style_of(fg: Color, bg: Color, m: Modifier) -> Style {
    let (mut fg, mut bg) = (color_rgb(fg, INK), color_rgb(bg, GROUND));
    if m.contains(Modifier::REVERSED) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if m.contains(Modifier::HIDDEN) {
        fg = bg;
    }
    Style {
        fg,
        bg: (bg != GROUND).then_some(bg),
        bold: m.contains(Modifier::BOLD),
        dim: m.contains(Modifier::DIM),
        italic: m.contains(Modifier::ITALIC),
        underline: m.contains(Modifier::UNDERLINED),
        strike: m.contains(Modifier::CROSSED_OUT),
    }
}

/// The rows of a drawn buffer as runs of equal style. Blank runs without a background are left out.
pub fn capture(buf: &Buffer) -> Vec<Row> {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let mut runs: Row = Vec::new();
            for x in 0..area.width {
                let cell = &buf[(x, y)];
                let style = style_of(cell.fg, cell.bg, cell.modifier);
                let sym = match cell.symbol() {
                    "" => "\u{200b}",
                    sym => sym,
                };
                let sym = sym.to_string();
                if let Some(run) = runs.last_mut()
                    && run.style == style
                {
                    run.cells += 1;
                    run.text.push_str(&sym);
                } else {
                    runs.push(Run {
                        col: x,
                        cells: 1,
                        text: sym.clone(),
                        style,
                    });
                }
            }
            runs.retain(|r| r.style.bg.is_some() || !r.text.trim().is_empty());
            runs
        })
        .collect()
}

/// Adds a captured frame: an identical frame to the last one only lengthens it.
pub fn push_frame(shots: &mut Vec<Shot>, rows: Vec<Row>) {
    if let Some(last) = shots.last_mut()
        && last.rows == rows
    {
        last.ticks += 1;
        return;
    }
    shots.push(Shot { rows, ticks: 1 });
}

/// The frame a poster names (a negative index counts from the end), if there is one.
pub fn poster_index(len: usize, poster: i64) -> Option<usize> {
    let i = if poster < 0 {
        len as i64 + poster
    } else {
        poster
    };
    usize::try_from(i).ok().filter(|i| *i < len)
}

/// A number for SVG attributes: two decimals at most, no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The CSS classes and the shared row definitions a rendering builds up.
#[derive(Default)]
struct Sheet {
    text: HashMap<Style, usize>,
    bg: HashMap<u32, usize>,
    css: String,
    rows: HashMap<String, usize>,
    pens: HashMap<(u32, bool, Pen), usize>,
    defs: String,
}

impl Sheet {
    /// The class of a text style (its background is not part of it).
    fn text_class(&mut self, style: Style) -> usize {
        let key = Style { bg: None, ..style };
        if let Some(&n) = self.text.get(&key) {
            return n;
        }
        let n = self.text.len();
        let mut decl = vec![format!("fill:{}", hex(key.fg))];
        if key.bold {
            decl.push("font-weight:600".into());
        }
        if key.italic {
            decl.push("font-style:italic".into());
        }
        if key.dim {
            decl.push("fill-opacity:.6".into());
        }
        let mut lines = Vec::new();
        if key.underline {
            lines.push("underline");
        }
        if key.strike {
            lines.push("line-through");
        }
        if !lines.is_empty() {
            decl.push(format!("text-decoration:{}", lines.join(" ")));
        }
        self.css.push_str(&format!(".k{n}{{{}}}", decl.join(";")));
        self.text.insert(key, n);
        n
    }

    fn bg_class(&mut self, bg: u32) -> usize {
        if let Some(&n) = self.bg.get(&bg) {
            return n;
        }
        let n = self.bg.len();
        self.css.push_str(&format!(".g{n}{{fill:{}}}", hex(bg)));
        self.bg.insert(bg, n);
        n
    }

    /// The class of a line or fill: its colour, pen, and opacity (dim text and shades are fainter).
    fn line_class(&mut self, fg: u32, dim: bool, pen: Pen) -> usize {
        if let Some(&n) = self.pens.get(&(fg, dim, pen)) {
            return n;
        }
        let n = self.pens.len();
        let mut opacity = if dim { 0.6 } else { 1.0 };
        let mut decl = match pen {
            Pen::Light => format!(
                "fill:none;stroke:{};stroke-width:1.1;stroke-linecap:square",
                hex(fg)
            ),
            Pen::Heavy => format!(
                "fill:none;stroke:{};stroke-width:2;stroke-linecap:square",
                hex(fg)
            ),
            Pen::Fill(pct) => {
                opacity *= f64::from(pct) / 100.0;
                format!("fill:{}", hex(fg))
            }
        };
        if opacity < 1.0 {
            decl.push_str(&format!(";opacity:{}", num(opacity)));
        }
        self.css.push_str(&format!(".l{n}{{{decl}}}"));
        self.pens.insert((fg, dim, pen), n);
        n
    }

    /// The id of a row's markup, defining it on first use.
    fn row(&mut self, markup: String) -> usize {
        if let Some(&n) = self.rows.get(&markup) {
            return n;
        }
        let n = self.rows.len();
        self.defs.push_str(&format!("<g id=\"r{n}\">{markup}</g>"));
        self.rows.insert(markup, n);
        n
    }
}

/// A pen a box or block character is drawn with: light or heavy lines (strokes), or a fill at a
/// percentage of the cell (the shades and the block elements).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Pen {
    Light,
    Heavy,
    Fill(u8),
}

/// The arms of a box-drawing character as [left, right, up, down]: 0 for none, 1 light, 2 heavy.
/// The rounded corners, the double lines and the block elements are not here (see `cell_geometry`).
fn arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' => [1, 1, 0, 0],
        '━' => [2, 2, 0, 0],
        '│' => [0, 0, 1, 1],
        '┃' => [0, 0, 2, 2],
        '┌' => [0, 1, 0, 1],
        '┐' => [1, 0, 0, 1],
        '└' => [0, 1, 1, 0],
        '┘' => [1, 0, 1, 0],
        '┏' => [0, 2, 0, 2],
        '┓' => [2, 0, 0, 2],
        '┗' => [0, 2, 2, 0],
        '┛' => [2, 0, 2, 0],
        '├' => [0, 1, 1, 1],
        '┤' => [1, 0, 1, 1],
        '┬' => [1, 1, 0, 1],
        '┴' => [1, 1, 1, 0],
        '┼' => [1, 1, 1, 1],
        '┣' => [0, 2, 2, 2],
        '┫' => [2, 0, 2, 2],
        '┳' => [2, 2, 0, 2],
        '┻' => [2, 2, 2, 0],
        '╋' => [2, 2, 2, 2],
        '╴' => [1, 0, 0, 0],
        '╵' => [0, 0, 1, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 0, 1],
        '╸' => [2, 0, 0, 0],
        '╹' => [0, 0, 2, 0],
        '╺' => [0, 2, 0, 0],
        '╻' => [0, 0, 0, 2],
        _ => return None,
    })
}

/// Appends a horizontal line from x0 to x1 at height y to path data.
fn hline(d: &mut String, y: f64, x0: f64, x1: f64) {
    d.push_str(&format!("M{} {}H{}", num(x0), num(y), num(x1)));
}

/// Appends a vertical line at x from y0 to y1 to path data.
fn vline(d: &mut String, x: f64, y0: f64, y1: f64) {
    d.push_str(&format!("M{} {}V{}", num(x), num(y0), num(y1)));
}

/// Appends a filled rectangle (x0, y0) to (x1, y1) to path data.
fn rect(d: &mut String, x0: f64, y0: f64, x1: f64, y1: f64) {
    d.push_str(&format!(
        "M{} {}H{}V{}H{}Z",
        num(x0),
        num(y0),
        num(x1),
        num(y1),
        num(x0)
    ));
}

/// The pieces a box-drawing or block character is drawn with, in the cell whose left edge is `x`
/// (row pixels; the cell is CELL_W by ROW_H). Lines run from the cell's centre to its edges with
/// square caps, so neighbouring cells join exactly; a rounded corner is a quarter arc through the
/// middles of its two arms. None for every other character, which stays text.
fn cell_geometry(c: char, x: f64) -> Option<Vec<(Pen, String)>> {
    let (w, h) = (CELL_W, ROW_H);
    let (cx, cy) = (x + w / 2.0, h / 2.0);
    if let Some([left, right, up, down]) = arms(c) {
        let mut pieces = Vec::new();
        for (pen, weight) in [(Pen::Light, 1), (Pen::Heavy, 2)] {
            let mut d = String::new();
            match (left == weight, right == weight) {
                (true, true) => hline(&mut d, cy, x, x + w),
                (true, false) => hline(&mut d, cy, x, cx),
                (false, true) => hline(&mut d, cy, cx, x + w),
                (false, false) => {}
            }
            match (up == weight, down == weight) {
                (true, true) => vline(&mut d, cx, 0.0, h),
                (true, false) => vline(&mut d, cx, 0.0, cy),
                (false, true) => vline(&mut d, cx, cy, h),
                (false, false) => {}
            }
            if !d.is_empty() {
                pieces.push((pen, d));
            }
        }
        return Some(pieces);
    }
    let arc = |from: (f64, f64), to: (f64, f64), sweep: u8| {
        format!(
            "M{} {}A{} {} 0 0 {sweep} {} {}",
            num(from.0),
            num(from.1),
            num(w / 2.0),
            num(h / 2.0),
            num(to.0),
            num(to.1)
        )
    };
    let line = match c {
        '╭' => arc((cx, h), (x + w, cy), 1),
        '╮' => arc((cx, h), (x, cy), 0),
        '╰' => arc((cx, 0.0), (x + w, cy), 0),
        '╯' => arc((cx, 0.0), (x, cy), 1),
        '═' => {
            let mut d = String::new();
            for dy in [-1.5, 1.5] {
                hline(&mut d, cy + dy, x, x + w);
            }
            d
        }
        '║' => {
            let mut d = String::new();
            for dx in [-1.5, 1.5] {
                vline(&mut d, cx + dx, 0.0, h);
            }
            d
        }
        _ => return block_geometry(c, x),
    };
    Some(vec![(Pen::Light, line)])
}

/// The block elements and the shades as filled rectangles of the cell at `x`.
fn block_geometry(c: char, x: f64) -> Option<Vec<(Pen, String)>> {
    type Part = (f64, f64, f64, f64);
    const FULL: Part = (0.0, 0.0, 1.0, 1.0);
    const UL: Part = (0.0, 0.0, 0.5, 0.5);
    const UR: Part = (0.5, 0.0, 1.0, 0.5);
    const LL: Part = (0.0, 0.5, 0.5, 1.0);
    const LR: Part = (0.5, 0.5, 1.0, 1.0);
    let (pct, parts): (u8, Vec<Part>) = match c {
        '▀' => (100, vec![(0.0, 0.0, 1.0, 0.5)]),
        '▔' => (100, vec![(0.0, 0.0, 1.0, 0.125)]),
        '▐' => (100, vec![(0.5, 0.0, 1.0, 1.0)]),
        '▕' => (100, vec![(0.875, 0.0, 1.0, 1.0)]),
        '░' => (25, vec![FULL]),
        '▒' => (50, vec![FULL]),
        '▓' => (75, vec![FULL]),
        '▖' => (100, vec![LL]),
        '▗' => (100, vec![LR]),
        '▘' => (100, vec![UL]),
        '▙' => (100, vec![UL, LL, LR]),
        '▚' => (100, vec![UL, LR]),
        '▛' => (100, vec![UL, UR, LL]),
        '▜' => (100, vec![UL, UR, LR]),
        '▝' => (100, vec![UR]),
        '▞' => (100, vec![UR, LL]),
        '▟' => (100, vec![UR, LL, LR]),
        // The lower eighths (▁ to ▇), the full block, then the left eighths (▉ to ▏).
        '\u{2581}'..='\u{2588}' => {
            let n = f64::from(c as u32 - 0x2580) / 8.0;
            (100, vec![(0.0, 1.0 - n, 1.0, 1.0)])
        }
        '\u{2589}'..='\u{258F}' => {
            let n = f64::from(0x2590 - c as u32) / 8.0;
            (100, vec![(0.0, 0.0, n, 1.0)])
        }
        _ => return None,
    };
    let (w, h) = (CELL_W, ROW_H);
    let mut d = String::new();
    for (x0, y0, x1, y1) in parts {
        rect(&mut d, x + x0 * w, y0 * h, x + x1 * w, y1 * h);
    }
    Some(vec![(Pen::Fill(pct), d)])
}

/// Adds a text chunk: a tspan at its column. Trailing spaces are dropped (nothing is drawn there);
/// inner spaces stay, so a run of text is one tspan (the font is monospace and the text is `pre`).
fn push_chunk(words: &mut String, sheet: &mut Sheet, style: Style, col: u16, text: &str) {
    let text = text.trim_end_matches(' ');
    if text.is_empty() {
        return;
    }
    let class = sheet.text_class(style);
    words.push_str(&format!(
        "<tspan x=\"{}\" class=\"k{class}\">{}</tspan>",
        num(f64::from(col) * CELL_W),
        esc(text),
    ));
}

/// One row as SVG: one background path per colour (a rectangle per filled stretch), the box and
/// block characters as geometry (one path per colour and pen), then one text element with a tspan
/// per chunk of text, placed at its column. Empty when the row shows nothing.
fn row_markup(row: &Row, sheet: &mut Sheet) -> String {
    let mut backs: BTreeMap<u32, Vec<(u16, u16)>> = BTreeMap::new();
    let mut pieces: BTreeMap<(u32, bool, Pen), String> = BTreeMap::new();
    let mut words = String::new();
    for run in row {
        if let Some(bg) = run.style.bg {
            let spans = backs.entry(bg).or_default();
            match spans.last_mut() {
                Some(last) if last.0 + last.1 == run.col => last.1 += run.cells,
                _ => spans.push((run.col, run.cells)),
            }
        }
        let mut chunk = String::new();
        let mut chunk_col = run.col;
        for (col, ch) in (run.col..).zip(run.text.chars()) {
            let ch = match cell_geometry(ch, f64::from(col) * CELL_W) {
                Some(drawn) => {
                    for (pen, d) in drawn {
                        pieces
                            .entry((run.style.fg, run.style.dim, pen))
                            .or_default()
                            .push_str(&d);
                    }
                    ' '
                }
                None => ch,
            };
            if ch == CONT {
                // The second cell of a wide character: the chunk ends with the wide character.
                push_chunk(&mut words, sheet, run.style, chunk_col, &chunk);
                chunk.clear();
                continue;
            }
            if chunk.is_empty() {
                chunk_col = col;
            }
            chunk.push(ch);
        }
        push_chunk(&mut words, sheet, run.style, chunk_col, &chunk);
    }
    let mut shown = String::new();
    for (bg, spans) in &backs {
        let class = sheet.bg_class(*bg);
        let mut d = String::new();
        for &(col, cells) in spans {
            let (x, w) = (f64::from(col) * CELL_W, f64::from(cells) * CELL_W);
            d.push_str(&format!(
                "M{} 0h{}v{}h{}z",
                num(x),
                num(w),
                num(ROW_H),
                num(-w)
            ));
        }
        shown.push_str(&format!("<path class=\"g{class}\" d=\"{d}\"/>"));
    }
    for ((fg, dim, pen), d) in &pieces {
        let class = sheet.line_class(*fg, *dim, *pen);
        shown.push_str(&format!("<path class=\"l{class}\" d=\"{d}\"/>"));
    }
    if words.is_empty() {
        shown
    } else {
        format!(
            "{shown}<text class=\"t\" y=\"{}\">{words}</text>",
            num(BASE_Y)
        )
    }
}

/// The window's size in pixels for a `cols` × `rows` screen.
fn window_size(cols: u16, rows: u16) -> (f64, f64) {
    (
        2.0 * PAD + f64::from(cols) * CELL_W,
        TOP + f64::from(rows) * ROW_H + BOTTOM,
    )
}

/// The SVG of a recording. Each row position's distinct rows are defined once (`<g id="r…">`) and
/// placed with a `<use>`. A row that stays the same over the loop is a plain `<use>`; one that
/// changes shows for its share of the loop (`opacity` keyframes with `step-end` timing). One frame
/// is a still.
pub fn svg(title: &str, cols: u16, rows: u16, shots: &[Shot], fps: u32) -> String {
    let (width, height) = window_size(cols, rows);
    let (w, h) = (num(width), num(height));
    let mut sheet = Sheet::default();
    let animate = shots.len() > 1;
    let total: u32 = shots.iter().map(|s| s.ticks).sum();
    // The variants of each row position in order of first appearance, with the ticks each shows.
    let mut variants: Vec<(usize, usize, Vec<(u32, u32)>)> = Vec::new();
    let mut index: HashMap<(usize, usize), usize> = HashMap::new();
    let mut at = 0;
    for shot in shots {
        for (y, row) in shot.rows.iter().enumerate() {
            let markup = row_markup(row, &mut sheet);
            if markup.is_empty() {
                continue;
            }
            let id = sheet.row(markup);
            let k = *index.entry((y, id)).or_insert_with(|| {
                variants.push((y, id, Vec::new()));
                variants.len() - 1
            });
            let spans = &mut variants[k].2;
            match spans.last_mut() {
                Some(last) if last.1 == at => last.1 = at + shot.ticks,
                _ => spans.push((at, at + shot.ticks)),
            }
        }
        at += shot.ticks;
    }
    let mut keyframes = String::new();
    let mut body = String::new();
    for (k, (y, id, spans)) in variants.iter().enumerate() {
        let y = num(*y as f64 * ROW_H);
        if !animate || (spans.len() == 1 && spans[0] == (0, total)) {
            body.push_str(&format!("<use href=\"#r{id}\" y=\"{y}\"/>"));
            continue;
        }
        let mut stops: Vec<(u32, u8)> = Vec::new();
        for &(a, b) in spans {
            if a > 0 && stops.is_empty() {
                stops.push((0, 0));
            }
            stops.push((a, 1));
            stops.push((b, 0));
        }
        let steps: String = stops
            .iter()
            .map(|&(t, on)| {
                format!(
                    "{}%{{opacity:{on}}}",
                    num(f64::from(t) / f64::from(total) * 100.0)
                )
            })
            .collect();
        keyframes.push_str(&format!(
            "@keyframes w{k}{{{steps}}}.w{k}{{animation:w{k} {}s step-end infinite}}",
            num(f64::from(total) / f64::from(fps)),
        ));
        body.push_str(&format!("<use class=\"w{k}\" href=\"#r{id}\" y=\"{y}\"/>"));
    }
    let css = format!(
        "/*APZ-FONT*/.t{{font-family:{FONT};font-size:13px;white-space:pre;fill:{}}}.ti{{font-family:{FONT};font-size:12px;fill:{}}}{}{keyframes}",
        hex(INK),
        hex(TITLE),
        sheet.css,
    );
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><title>{}</title><style>{css}</style><defs>{}</defs>",
        esc(title),
        sheet.defs,
    );
    out.push_str(&format!(
        "<rect x=\".5\" y=\".5\" width=\"{}\" height=\"{}\" rx=\"10\" fill=\"{}\" stroke=\"{}\"/>",
        num(width - 1.0),
        num(height - 1.0),
        hex(GROUND),
        hex(EDGE),
    ));
    out.push_str(&format!(
        "<path d=\"M0 {b}V10A10 10 0 0 1 10 0H{r}A10 10 0 0 1 {w} 10V{b}Z\" fill=\"{}\"/>",
        hex(BAR),
        b = num(BAR_H),
        r = num(width - 10.0),
    ));
    out.push_str(&format!(
        "<path d=\"M10 1.5H{}\" fill=\"none\" stroke=\"#fff\" stroke-opacity=\".06\"/>",
        num(width - 10.0),
    ));
    for (cx, color) in [(18, "#ff5f57"), (36, "#febc2e"), (54, "#28c840")] {
        out.push_str(&format!(
            "<circle cx=\"{cx}\" cy=\"15\" r=\"6\" fill=\"{color}\"/>"
        ));
    }
    out.push_str(&format!(
        "<text class=\"ti\" x=\"{}\" y=\"19\" text-anchor=\"middle\">{}</text>",
        num(width / 2.0),
        esc(title),
    ));
    out.push_str(&format!(
        "<g transform=\"translate({} {})\">{body}</g></svg>",
        num(PAD),
        num(TOP),
    ));
    out
}

struct Recorder {
    app: App,
    term: Terminal<TestBackend>,
    fps: u32,
    dt: f32,
    shots: Vec<Shot>,
}

impl Recorder {
    /// One frame: the app takes what the hub brought, moves its animations on by one frame and
    /// draws; `keep` captures the drawn frame. Paced in real time, so the hub's polls keep up.
    fn frame(&mut self, keep: bool) -> Result<(), String> {
        let started = Instant::now();
        self.app.drain();
        self.app.tick(self.dt);
        self.term
            .draw(|frame| self.app.draw(frame))
            .map_err(|e| e.to_string())?;
        if keep {
            push_frame(&mut self.shots, capture(self.term.backend().buffer()));
        }
        thread::sleep(Duration::from_secs_f32(self.dt).saturating_sub(started.elapsed()));
        Ok(())
    }

    fn hold(&mut self, secs: f32, keep: bool) -> Result<(), String> {
        for _ in 0..(secs * self.fps as f32).round() as u32 {
            self.frame(keep)?;
        }
        Ok(())
    }

    fn press(&mut self, key: KeyEvent) -> Result<(), String> {
        self.app.on_key(key);
        self.frame(true)
    }

    fn play(&mut self, steps: &[Step]) -> Result<(), String> {
        for step in steps {
            match step {
                Step::Wait(secs) => self.hold(*secs, true)?,
                Step::Key {
                    press,
                    repeat,
                    every,
                } => {
                    for n in 1..=*repeat {
                        self.press(*press)?;
                        if n < *repeat {
                            self.hold(*every, true)?;
                        }
                    }
                }
                Step::Type { text, every } => {
                    for ch in text.chars() {
                        self.press(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))?;
                        self.hold(*every, true)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// `APZ_TUI_RECORD=<script.json>`: plays the script and writes `<out>.svg` and `<out>-poster.svg`.
pub fn run(client: Client, path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let default_out = path.with_extension("").display().to_string();
    let script = parse(&text, &default_out)?;
    let term =
        Terminal::new(TestBackend::new(script.width, script.height)).map_err(|e| e.to_string())?;
    let mut rec = Recorder {
        app: App::new(client),
        term,
        fps: script.fps,
        dt: 1.0 / script.fps as f32,
        shots: Vec::new(),
    };
    rec.hold(script.warmup, false)?;
    rec.play(&script.steps)?;
    if rec.shots.is_empty() {
        rec.frame(true)?;
    }
    let poster = poster_index(rec.shots.len(), script.poster)
        .ok_or_else(|| "the poster names no frame of the recording".to_string())?;
    let animated = svg(
        &script.title,
        script.width,
        script.height,
        &rec.shots,
        script.fps,
    );
    let still = svg(
        &script.title,
        script.width,
        script.height,
        &rec.shots[poster..=poster],
        script.fps,
    );
    if let Some(dir) = Path::new(&script.out)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let ticks: u32 = rec.shots.iter().map(|s| s.ticks).sum();
    for (suffix, body) in [(".svg", &animated), ("-poster.svg", &still)] {
        let file = format!("{}{suffix}", script.out);
        std::fs::write(&file, body).map_err(|e| format!("{file}: {e}"))?;
        eprintln!("wrote {file}: {} bytes", body.len());
    }
    eprintln!(
        "{} frames merged from {} captures, {:.1} s loop",
        rec.shots.len(),
        ticks,
        f64::from(ticks) / f64::from(script.fps)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style as Ink;

    /// A tiny well-formedness check: every tag opened is closed in order, and nothing is left open.
    fn well_formed(xml: &str) -> Result<(), String> {
        let mut open: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find('<') {
            let end = rest[start..].find('>').ok_or("a tag is not closed")? + start;
            let tag = &rest[start + 1..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                let last = open.pop().ok_or(format!("</{name}> with nothing open"))?;
                if last != name.trim() {
                    return Err(format!("<{last}> closed by </{name}>"));
                }
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap_or("");
                open.push(name.to_string());
            }
        }
        match open.is_empty() {
            true => Ok(()),
            false => Err(format!("left open: {open:?}")),
        }
    }

    fn plain() -> Style {
        style_of(Color::Reset, Color::Reset, Modifier::empty())
    }

    #[test]
    fn colours_map_to_rgb() {
        assert_eq!(color_rgb(Color::Rgb(1, 2, 3), INK), 0x010203);
        assert_eq!(color_rgb(Color::Reset, INK), INK);
        assert_eq!(color_rgb(Color::Red, INK), NAMED[1]);
        assert_eq!(color_rgb(Color::LightCyan, INK), NAMED[14]);
        assert_eq!(indexed(196), 0xff0000, "the cube's red corner");
        assert_eq!(indexed(16), 0x000000);
        assert_eq!(indexed(231), 0xffffff);
        assert_eq!(indexed(232), 0x080808, "the grey ramp starts at 8");
        assert_eq!(indexed(255), 0xeeeeee);
        assert_eq!(indexed(9), NAMED[9]);
    }

    #[test]
    fn reverse_and_hidden_follow_the_defaults() {
        let reversed = style_of(Color::Reset, Color::Reset, Modifier::REVERSED);
        assert_eq!(reversed.fg, GROUND);
        assert_eq!(reversed.bg, Some(INK));
        let hidden = style_of(Color::Red, Color::Reset, Modifier::HIDDEN);
        assert_eq!(hidden.fg, GROUND, "hidden text is painted in the ground");
        assert_eq!(hidden.bg, None);
        assert_eq!(plain().bg, None, "the ground needs no rectangle");
    }

    #[test]
    fn equal_cells_merge_into_one_run() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 2));
        buf.set_string(0, 0, "hello", Ink::default().fg(Color::Red));
        buf.set_string(5, 0, " ", Ink::default());
        buf.set_string(6, 0, "x", Ink::default().bg(Color::Blue));
        let rows = capture(&buf);
        assert_eq!(rows.len(), 2);
        let row = &rows[0];
        assert_eq!(row.len(), 2, "red text, then the blue cell; blank runs go");
        assert_eq!(
            (row[0].col, row[0].cells, row[0].text.as_str()),
            (0, 5, "hello")
        );
        assert_eq!(row[0].style.fg, NAMED[1]);
        assert_eq!((row[1].col, row[1].cells), (6, 1));
        assert_eq!(row[1].style.bg, Some(NAMED[4]));
        assert!(rows[1].is_empty(), "a blank row has no runs");
    }

    #[test]
    fn words_are_placed_by_their_column() {
        let row = vec![Run {
            col: 3,
            cells: 3,
            text: "a b".into(),
            style: plain(),
        }];
        let markup = row_markup(&row, &mut Sheet::default());
        assert!(
            markup.contains("x=\"23.4\" class=\"k0\">a b</tspan>"),
            "{markup}"
        );
    }

    #[test]
    fn identical_frames_merge() {
        let row = |text: &str| {
            vec![Run {
                col: 0,
                cells: 1,
                text: text.into(),
                style: plain(),
            }]
        };
        let mut shots = Vec::new();
        push_frame(&mut shots, vec![row("a")]);
        push_frame(&mut shots, vec![row("a")]);
        push_frame(&mut shots, vec![row("a")]);
        push_frame(&mut shots, vec![row("b")]);
        assert_eq!(shots.len(), 2);
        assert_eq!(shots[0].ticks, 3);
        assert_eq!(shots[1].ticks, 1);
    }

    #[test]
    fn the_poster_counts_from_the_end() {
        assert_eq!(poster_index(5, -1), Some(4));
        assert_eq!(poster_index(5, 0), Some(0));
        assert_eq!(poster_index(5, 5), None);
        assert_eq!(poster_index(5, -6), None);
        assert_eq!(poster_index(0, -1), None);
    }

    #[test]
    fn a_script_reads_its_keys_and_defaults() {
        let text = r#"{
            "width": 120, "height": 34, "fps": 12, "title": "agentplaza tui", "warmup": 3.0,
            "steps": [
                {"key": "m"}, {"wait": 1.5}, {"type": "hi"}, {"key": "Enter"},
                {"key": "Down", "repeat": 3, "every": 0.25}, {"ctrl": "K"}, {"key": "Esc"}
            ],
            "poster": -1, "out": "demo/tour"
        }"#;
        let script = parse(text, "x").unwrap();
        assert_eq!((script.width, script.height, script.fps), (120, 34, 12));
        assert_eq!(script.out, "demo/tour");
        assert_eq!(script.steps.len(), 7);
        assert_eq!(
            script.steps[0],
            Step::Key {
                press: KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
                repeat: 1,
                every: 0.25,
            }
        );
        assert_eq!(script.steps[1], Step::Wait(1.5));
        assert_eq!(
            script.steps[2],
            Step::Type {
                text: "hi".into(),
                every: 0.08,
            }
        );
        assert!(matches!(
            script.steps[4],
            Step::Key { repeat: 3, every, .. } if (every - 0.25).abs() < 1e-6
        ));
        assert_eq!(
            script.steps[5],
            Step::Key {
                press: KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
                repeat: 1,
                every: 0.25,
            }
        );
        let bare = parse("{}", "from/script").unwrap();
        assert_eq!((bare.width, bare.height, bare.fps), (120, 34, 12));
        assert_eq!(bare.out, "from/script");
        assert!(parse(r#"{"steps": [{"key": "Hyper"}]}"#, "x").is_err());
        assert!(parse(r#"{"steps": [{"wait": "soon"}]}"#, "x").is_err());
        assert!(parse(r#"{"steps": [{"ctrl": "ab"}]}"#, "x").is_err());
        assert!(parse("[1]", "x").is_ok(), "a list is read as no fields");
    }

    #[test]
    fn the_svg_is_well_formed_and_shares_rows() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        buf.set_string(0, 0, "<a & b>", Ink::default().fg(Color::Green));
        buf.set_string(
            0,
            1,
            "\"quoted\"",
            Ink::default().add_modifier(Modifier::BOLD),
        );
        let first = capture(&buf);
        buf.set_string(0, 2, "tail", Ink::default().bg(Color::Indexed(196)));
        let second = capture(&buf);
        let shots = vec![
            Shot {
                rows: first.clone(),
                ticks: 2,
            },
            Shot {
                rows: second,
                ticks: 1,
            },
        ];
        let out = svg("a <b> & \"c\"", 10, 3, &shots, 12);
        well_formed(&out).expect("well formed");
        assert!(
            out.contains("@keyframes w2{0%{opacity:0}66.67%{opacity:1}100%{opacity:0}}"),
            "the third row shows for the last two thirds of the loop: {out}"
        );
        assert!(
            out.contains(".w2{animation:w2 0.25s step-end infinite}"),
            "{out}"
        );
        assert!(
            out.contains(">&lt;a &amp; b&gt;</tspan>"),
            "a run of one style is one span"
        );
        assert_eq!(
            out.matches("<use href=").count(),
            2,
            "the two rows that stay the same are plain uses"
        );
        assert_eq!(
            out.matches("<use class=\"w2\"").count(),
            1,
            "the row that changes is a timed use"
        );
        assert_eq!(
            out.matches("<g id=\"r").count(),
            3,
            "each distinct row is defined once"
        );
        let still = svg("t", 10, 3, &shots[..1], 12);
        well_formed(&still).expect("well formed");
        assert!(!still.contains("@keyframes"), "one frame is a still");
    }

    #[test]
    fn box_characters_become_geometry() {
        let row = vec![Run {
            col: 0,
            cells: 3,
            text: "╭─╮".into(),
            style: plain(),
        }];
        let markup = row_markup(&row, &mut Sheet::default());
        assert!(!markup.contains("<text"), "no text is left: {markup}");
        assert!(
            !markup.contains('╭') && !markup.contains('─') && !markup.contains('╮'),
            "{markup}"
        );
        assert!(markup.contains("<path"), "{markup}");
        assert!(
            markup.contains("A3.9 8.5 0 0 1 7.8 8.5"),
            "╭ is a quarter arc from the bottom middle to the right middle: {markup}"
        );
        let corner = vec![Run {
            col: 2,
            cells: 1,
            text: "╮".into(),
            style: plain(),
        }];
        let markup = row_markup(&corner, &mut Sheet::default());
        assert!(
            markup.contains("A3.9 8.5 0 0 0 15.6 8.5"),
            "╮ bends the other way: {markup}"
        );
    }

    #[test]
    fn box_and_block_characters_keep_their_columns() {
        let row = vec![Run {
            col: 0,
            cells: 4,
            text: "a│ b".into(),
            style: plain(),
        }];
        let markup = row_markup(&row, &mut Sheet::default());
        assert!(
            markup.contains("x=\"0\" class=\"k0\">a  b</tspan>"),
            "the box character is a space, so b stays in column 3: {markup}"
        );
        assert!(
            markup.contains("M11.7 0V17"),
            "│ in column 1 runs the full cell height: {markup}"
        );
    }

    #[test]
    fn blocks_are_filled_cells() {
        let row = vec![Run {
            col: 0,
            cells: 2,
            text: "█▌".into(),
            style: plain(),
        }];
        let markup = row_markup(&row, &mut Sheet::default());
        assert!(
            markup.contains("d=\"M0 0H7.8V17H0ZM7.8 0H11.7V17H7.8Z\""),
            "a full block, then the left half: {markup}"
        );
        assert!(!markup.contains("<text"), "{markup}");
    }

    #[test]
    fn the_font_placeholder_leads_the_style() {
        let shots = vec![Shot {
            rows: Vec::new(),
            ticks: 1,
        }];
        let out = svg("t", 10, 3, &shots, 12);
        assert!(out.contains("<style>/*APZ-FONT*/.t{"), "{out}");
        assert_eq!(out.matches("/*APZ-FONT*/").count(), 1, "written once");
        assert!(out.contains("font-family:\"APZ Mono\""), "{out}");
    }
}
