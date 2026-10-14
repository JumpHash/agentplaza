//! The screen: a grid of cells that the parser's events change. It is the whole terminal state a
//! view needs: the active rows, the scrollback above them, the cursor and its modes, the scroll
//! region, the current style, and the bytes the program is owed in reply (cursor reports).
//!
//! Lines have stable absolute ids: the scrollback's first line is `dropped()` (lines that fell off
//! its far end so far), and each line after it counts up. Content keeps its id as it scrolls, so a
//! selection anchored to ids stays on its text while output arrives. The alternate screen (modes
//! 47, 1047, 1049) keeps no history: its rows do not scroll into the scrollback.

use std::collections::VecDeque;
use std::mem;

use vte::{Params, Perform};

use crate::width::char_width;

/// Lines kept above the screen, on the main screen only.
pub const SCROLLBACK: usize = 10_000;
/// The largest size in columns or rows (the hub caps a terminal at this too).
pub const MAX_SIZE: usize = 1_000;

/// A colour as the program set it. `Default` is the theme's own foreground or background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// One of the 256 palette entries (0 to 15 are the ANSI colours).
    Indexed(u8),
    /// A 24-bit colour.
    Rgb(u8, u8, u8),
}

/// The graphic rendition in effect: colours and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// One cell. `width` is 1 for a normal character, 2 for the first cell of a wide one, and 0 for the
/// second cell of a wide one (its `ch` is a space and is never drawn on its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub width: u8,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', width: 1, style: Style::default() }
    }
}

/// One row. `wrapped` is set when the text continued onto the next row by wrapping at the edge,
/// so a copy joins the two without a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl Row {
    fn blank(cols: usize, style: Style) -> Row {
        Row { cells: vec![Cell { ch: ' ', width: 1, style }; cols], wrapped: false }
    }
}

/// The cursor and style saved by DECSC (`ESC 7`, `CSI s`) or by mode 1049.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Saved {
    row: usize,
    col: usize,
    style: Style,
}

/// The terminal's screen state, driven by [`Perform`].
pub struct Screen {
    cols: usize,
    rows: usize,
    /// The active rows (the alternate screen's while it is shown).
    grid: Vec<Row>,
    /// The main screen's rows while the alternate screen is shown.
    saved_main: Option<Vec<Row>>,
    scrollback: VecDeque<Row>,
    dropped: u64,
    row: usize,
    col: usize,
    /// The cursor sits past the last column: the next printed character wraps first.
    wrap_next: bool,
    style: Style,
    /// The scroll region, inclusive, as row indices of the active grid.
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    saved: Option<Saved>,
    alt_saved: Option<Saved>,
    cursor_visible: bool,
    app_cursor: bool,
    autowrap: bool,
    origin: bool,
    bracketed_paste: bool,
    /// Lines scrolled back from the bottom (0 shows the live screen).
    view: usize,
    replies: Vec<u8>,
    title: String,
    bell: bool,
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c > 0 && c % 8 == 0).collect()
}

fn arg(nums: &[u16], i: usize, default: usize) -> usize {
    match nums.get(i) {
        Some(&v) if v > 0 => usize::from(v),
        _ => default,
    }
}

/// Reads the colour after `38` or `48` (`i` is that group's index). Both the colon form
/// (`38:5:n`, `38:2::r:g:b`) and the semicolon form (`38;5;n`, `38;2;r;g;b`) are read; the
/// semicolon form takes the groups after it, so `i` moves past them.
fn extended(groups: &[Vec<u16>], i: &mut usize) -> Option<Color> {
    let group = &groups[*i];
    if group.len() > 1 {
        return match *group.get(1)? {
            5 => group.get(2).map(|&n| Color::Indexed(clamp8(n))),
            2 => {
                let rgb = if group.len() >= 6 { group.get(3..6)? } else { group.get(2..5)? };
                Some(Color::Rgb(clamp8(rgb[0]), clamp8(rgb[1]), clamp8(rgb[2])))
            }
            _ => None,
        };
    }
    let first = |offset: usize| groups.get(*i + offset).and_then(|g| g.first().copied());
    match first(1)? {
        5 => {
            let n = first(2)?;
            *i += 2;
            Some(Color::Indexed(clamp8(n)))
        }
        2 => {
            let (r, g, b) = (first(2)?, first(3)?, first(4)?);
            *i += 4;
            Some(Color::Rgb(clamp8(r), clamp8(g), clamp8(b)))
        }
        _ => None,
    }
}

fn clamp8(v: u16) -> u8 {
    v.min(255) as u8
}

/// Makes a row exactly `cols` cells: cut at the right edge (a wide character split by the cut
/// becomes a blank), or padded with blanks.
fn fit_row(row: &mut Row, cols: usize) {
    if row.cells.len() > cols {
        row.cells.truncate(cols);
        if let Some(last) = row.cells.last_mut().filter(|last| last.width == 2) {
            *last = Cell::default();
        }
    } else {
        row.cells.resize(cols, Cell::default());
    }
}

impl Screen {
    /// A blank screen of `cols` by `rows` (each clamped to 1 to [`MAX_SIZE`]).
    pub fn new(cols: usize, rows: usize) -> Screen {
        let cols = cols.clamp(1, MAX_SIZE);
        let rows = rows.clamp(1, MAX_SIZE);
        Screen {
            cols,
            rows,
            grid: vec![Row::blank(cols, Style::default()); rows],
            saved_main: None,
            scrollback: VecDeque::new(),
            dropped: 0,
            row: 0,
            col: 0,
            wrap_next: false,
            style: Style::default(),
            top: 0,
            bottom: rows - 1,
            tabs: default_tabs(cols),
            saved: None,
            alt_saved: None,
            cursor_visible: true,
            app_cursor: false,
            autowrap: true,
            origin: false,
            bracketed_paste: false,
            view: 0,
            replies: Vec::new(),
            title: String::new(),
            bell: false,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The cursor as (row, column) on the active grid, whether or not it is shown.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Whether the cursor is shown (mode 25).
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// The cursor's place on the view as (row, column), or nothing when it is hidden or scrolled
    /// out of view.
    pub fn cursor_on_view(&self) -> Option<(usize, usize)> {
        let y = self.row + self.view;
        (self.cursor_visible && y < self.rows).then_some((y, self.col))
    }

    /// Application cursor keys (mode 1): arrows are sent as `ESC O x` instead of `CSI x`.
    pub fn app_cursor(&self) -> bool {
        self.app_cursor
    }

    /// Bracketed paste (mode 2004): pasted text is wrapped in `ESC [200~` and `ESC [201~`.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// Whether the alternate screen is shown.
    pub fn alt_screen(&self) -> bool {
        self.saved_main.is_some()
    }

    /// The window title the program set (OSC 0 or 2), or empty.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Lines scrolled back from the bottom.
    pub fn view_offset(&self) -> usize {
        self.view
    }

    /// Scrolls the view: positive moves back into history, negative forward to the live screen.
    /// Clamped to the history; the alternate screen has none.
    pub fn scroll_view(&mut self, delta: isize) {
        let max = self.history_len() as isize;
        self.view = (self.view as isize + delta).clamp(0, max) as usize;
    }

    /// Returns to the live screen.
    pub fn reset_view(&mut self) {
        self.view = 0;
    }

    /// Lines above the active rows: the scrollback on the main screen, none on the alternate one.
    pub fn history_len(&self) -> usize {
        if self.alt_screen() { 0 } else { self.scrollback.len() }
    }

    /// Lines that fell off the far end of the scrollback so far. A line's absolute id is this
    /// plus its index.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Every line there is: the history, then the active rows.
    pub fn line_count(&self) -> usize {
        self.history_len() + self.rows
    }

    /// Line `index` (0 is the oldest line of the history).
    pub fn line(&self, index: usize) -> Option<&Row> {
        let history = self.history_len();
        if index < history {
            self.scrollback.get(index)
        } else {
            self.grid.get(index - history)
        }
    }

    /// The row on the view at `y` (0 is the top), as the view is scrolled now.
    pub fn view_row(&self, y: usize) -> Option<&Row> {
        let top = self.history_len() - self.view;
        self.line(top + y)
    }

    /// The absolute id of line `index`.
    pub fn abs_of_index(&self, index: usize) -> u64 {
        self.dropped + index as u64
    }

    /// The absolute id of the line on the view at `y`.
    pub fn abs_of_view_row(&self, y: usize) -> u64 {
        self.abs_of_index(self.history_len() - self.view + y)
    }

    /// The index of the line with absolute id `abs`, if it is still kept.
    pub fn index_of_abs(&self, abs: u64) -> Option<usize> {
        let index = abs.checked_sub(self.dropped)? as usize;
        (index < self.line_count()).then_some(index)
    }

    /// The text of line `index` with its trailing spaces removed.
    pub fn line_text(&self, index: usize) -> String {
        let Some(row) = self.line(index) else {
            return String::new();
        };
        row_text(row, 0, self.cols).trim_end_matches(' ').to_string()
    }

    /// The text from one cell to another, both inclusive, as absolute line ids and columns. Rows
    /// the text wraps across join without a newline; other rows end with one. Ids no longer kept
    /// read as the oldest line.
    pub fn text_between(&self, start: (u64, usize), end: (u64, usize)) -> String {
        let (a, b) = if start <= end { (start, end) } else { (end, start) };
        let last = self.line_count().saturating_sub(1);
        let first_index = self.index_of_abs(a.0).unwrap_or(0).min(last);
        let last_index = self.index_of_abs(b.0).unwrap_or(last).min(last);
        let mut out = String::new();
        for index in first_index..=last_index {
            let Some(row) = self.line(index) else {
                continue;
            };
            let from = if index == first_index { a.1 } else { 0 };
            let to = if index == last_index { b.1 + 1 } else { self.cols };
            let text = row_text(row, from, to.min(self.cols));
            if index == last_index {
                out.push_str(text.trim_end_matches(' '));
            } else if row.wrapped {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end_matches(' '));
                out.push('\n');
            }
        }
        out
    }

    /// The bytes the program is owed in reply (cursor reports, device attributes), taken out.
    pub fn take_replies(&mut self) -> Vec<u8> {
        mem::take(&mut self.replies)
    }

    /// Whether a bell rang since the last call.
    pub fn take_bell(&mut self) -> bool {
        mem::take(&mut self.bell)
    }

    /// Resizes to `cols` by `rows` (each clamped to 1 to [`MAX_SIZE`]). Text is cut or padded, not
    /// reflowed. When the screen shrinks, rows above the cursor go into the scrollback first, so the
    /// cursor stays on screen.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.clamp(1, MAX_SIZE);
        let rows = rows.clamp(1, MAX_SIZE);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let shift = if rows < self.rows { (self.rows - rows).min(self.row) } else { 0 };
        let mut grid = mem::take(&mut self.grid);
        let on_main = self.saved_main.is_none();
        for _ in 0..shift {
            let row = grid.remove(0);
            if on_main {
                self.push_history(row);
            }
        }
        grid.truncate(rows);
        grid.resize_with(rows, || Row::blank(cols, Style::default()));
        for row in &mut grid {
            fit_row(row, cols);
        }
        self.grid = grid;
        if let Some(main) = self.saved_main.as_mut() {
            main.truncate(rows);
            main.resize_with(rows, || Row::blank(cols, Style::default()));
            for row in main.iter_mut() {
                fit_row(row, cols);
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.row = (self.row - shift).min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.wrap_next = false;
        self.top = 0;
        self.bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.view = self.view.min(self.history_len());
    }

    /// A blank cell that keeps the current background, as erasing does.
    fn blank_cell(&self) -> Cell {
        Cell { ch: ' ', width: 1, style: Style { bg: self.style.bg, ..Style::default() } }
    }

    fn blank_row(&self) -> Row {
        Row { cells: vec![self.blank_cell(); self.cols], wrapped: false }
    }

    fn push_history(&mut self, row: Row) {
        self.scrollback.push_back(row);
        if self.scrollback.len() > SCROLLBACK {
            self.scrollback.pop_front();
            self.dropped += 1;
        }
        if self.view > 0 {
            // Keep the same lines in view while output arrives.
            self.view = (self.view + 1).min(self.scrollback.len());
        }
    }

    /// Scrolls the scroll region up by `n`: the top rows leave (into the history when the region
    /// starts at the top of the main screen) and blank rows come in at the bottom.
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let to_history = self.top == 0 && !self.alt_screen();
        for _ in 0..n {
            let row = self.grid.remove(self.top);
            if to_history {
                self.push_history(row);
            }
            let blank = self.blank_row();
            self.grid.insert(self.bottom, blank);
        }
    }

    /// Scrolls the scroll region down by `n`: blank rows come in at the top, the bottom rows leave.
    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            self.grid.remove(self.bottom);
            let blank = self.blank_row();
            self.grid.insert(self.top, blank);
        }
    }

    /// Moves the cursor down a line, scrolling the region when it is at the bottom margin.
    fn index(&mut self) {
        if self.row == self.bottom {
            self.scroll_up(1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    /// Moves the cursor up a line, scrolling the region down when it is at the top margin.
    fn reverse_index(&mut self) {
        if self.row == self.top {
            self.scroll_down(1);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }

    /// The lowest row a relative move may reach, and the highest it may go up to: the scroll
    /// region when the cursor is inside it, else the screen.
    fn move_limits(&self) -> (usize, usize) {
        let up = if self.row >= self.top { self.top } else { 0 };
        let down = if self.row <= self.bottom { self.bottom } else { self.rows - 1 };
        (up, down)
    }

    /// Places the cursor at (`row`, `col`), both relative to the origin when origin mode is on.
    fn place(&mut self, row: usize, col: usize) {
        let (base, limit) = if self.origin { (self.top, self.bottom) } else { (0, self.rows - 1) };
        self.row = (base + row).min(limit);
        self.col = col.min(self.cols - 1);
        self.wrap_next = false;
    }

    /// Wraps the cursor to the start of the next row, marking the row it leaves as wrapped.
    fn wrap(&mut self) {
        self.grid[self.row].wrapped = true;
        self.col = 0;
        self.wrap_next = false;
        self.index();
    }

    /// Blanks the neighbour of a wide character that is about to be overwritten, so no half of a
    /// wide character is left behind.
    fn fix_wide(&mut self, row: usize, col: usize) {
        let cells = &mut self.grid[row].cells;
        if col >= cells.len() {
            return;
        }
        match cells[col].width {
            0 if col > 0 => cells[col - 1] = Cell::default(),
            2 if col + 1 < cells.len() => cells[col + 1] = Cell::default(),
            _ => {}
        }
    }

    /// Clears `from..to` of the cursor's row to the erase colour, keeping wide-character pairs whole.
    fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        let blank = self.blank_cell();
        let to = to.min(self.cols);
        for col in from..to {
            self.fix_wide(row, col);
            self.grid[row].cells[col] = blank;
        }
    }

    fn save_cursor(&mut self) -> Saved {
        Saved { row: self.row, col: self.col, style: self.style }
    }

    fn restore_cursor(&mut self, saved: Option<Saved>) {
        let saved = saved.unwrap_or(Saved { row: 0, col: 0, style: Style::default() });
        self.row = saved.row.min(self.rows - 1);
        self.col = saved.col.min(self.cols - 1);
        self.style = saved.style;
        self.wrap_next = false;
    }

    /// Switches to the alternate screen (blank) or back to the main one, saving or restoring the
    /// cursor for mode 1049.
    fn set_alt_screen(&mut self, on: bool, with_cursor: bool) {
        if on == self.alt_screen() {
            return;
        }
        if on {
            if with_cursor {
                self.alt_saved = Some(self.save_cursor());
            }
            let blank = vec![Row::blank(self.cols, Style::default()); self.rows];
            self.saved_main = Some(mem::replace(&mut self.grid, blank));
        } else if let Some(main) = self.saved_main.take() {
            self.grid = main;
            if with_cursor {
                let saved = self.alt_saved.take();
                self.restore_cursor(saved);
            }
        }
        self.top = 0;
        self.bottom = self.rows - 1;
        self.wrap_next = false;
        self.view = 0;
    }

    fn set_private_mode(&mut self, mode: usize, on: bool) {
        match mode {
            1 => self.app_cursor = on,
            6 => {
                self.origin = on;
                self.place(0, 0);
            }
            7 => {
                self.autowrap = on;
                if !on {
                    self.wrap_next = false;
                }
            }
            25 => self.cursor_visible = on,
            47 | 1047 => self.set_alt_screen(on, false),
            1049 => self.set_alt_screen(on, true),
            2004 => self.bracketed_paste = on,
            _ => {}
        }
    }

    /// Clears the whole screen and the cursor's modes, as `ESC c` does. The scrollback is kept.
    fn reset(&mut self) {
        let history = mem::take(&mut self.scrollback);
        let dropped = self.dropped;
        let title = mem::take(&mut self.title);
        *self = Screen::new(self.cols, self.rows);
        self.scrollback = history;
        self.dropped = dropped;
        self.title = title;
    }

    fn sgr(&mut self, params: &Params) {
        let groups: Vec<Vec<u16>> = params.iter().map(<[u16]>::to_vec).collect();
        if groups.is_empty() {
            self.style = Style::default();
            return;
        }
        let mut i = 0;
        while i < groups.len() {
            let group = &groups[i];
            let code = group.first().copied().unwrap_or(0);
            match code {
                0 => self.style = Style::default(),
                1 => self.style.bold = true,
                3 => self.style.italic = true,
                4 => self.style.underline = group.get(1).is_none_or(|&style| style != 0),
                7 => self.style.inverse = true,
                21 => self.style.underline = true,
                22 => self.style.bold = false,
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.fg = Color::Indexed((code - 30) as u8),
                38 => {
                    if let Some(color) = extended(&groups, &mut i) {
                        self.style.fg = color;
                    }
                }
                39 => self.style.fg = Color::Default,
                40..=47 => self.style.bg = Color::Indexed((code - 40) as u8),
                48 => {
                    if let Some(color) = extended(&groups, &mut i) {
                        self.style.bg = color;
                    }
                }
                49 => self.style.bg = Color::Default,
                90..=97 => self.style.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => self.style.bg = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
    }
}

/// The text of `row` from column `from` up to `to` (exclusive), skipping the second cell of wide
/// characters.
fn row_text(row: &Row, from: usize, to: usize) -> String {
    let end = to.min(row.cells.len());
    row.cells
        .get(from..end)
        .unwrap_or(&[])
        .iter()
        .filter(|cell| cell.width != 0)
        .map(|cell| cell.ch)
        .collect()
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        let width = char_width(c);
        if width == 0 {
            // A combining mark or a format character: the screen keeps base characters only.
            return;
        }
        if self.wrap_next {
            self.wrap();
        }
        if width == 2 {
            if self.cols < 2 {
                return;
            }
            if self.col + 2 > self.cols {
                if !self.autowrap {
                    return;
                }
                self.wrap();
            }
        }
        let (row, col) = (self.row, self.col);
        self.fix_wide(row, col);
        if width == 2 {
            self.fix_wide(row, col + 1);
        }
        let style = self.style;
        let cells = &mut self.grid[row].cells;
        cells[col] = Cell { ch: c, width: width as u8, style };
        if width == 2 {
            cells[col + 1] = Cell { ch: ' ', width: 0, style };
        }
        let next = col + width;
        if next >= self.cols {
            self.col = self.cols - 1;
            self.wrap_next = self.autowrap;
        } else {
            self.col = next;
            self.wrap_next = false;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                self.col = self.col.saturating_sub(1);
                self.wrap_next = false;
            }
            0x09 => {
                let mut col = self.col + 1;
                while col < self.cols - 1 && !self.tabs[col] {
                    col += 1;
                }
                self.col = col.min(self.cols - 1);
                self.wrap_next = false;
            }
            0x0A..=0x0C => {
                self.index();
                self.wrap_next = false;
            }
            0x0D => {
                self.col = 0;
                self.wrap_next = false;
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        if action == 'm' && intermediates.is_empty() {
            self.sgr(params);
            return;
        }
        let nums: Vec<u16> = params.iter().map(|group| group.first().copied().unwrap_or(0)).collect();
        let private = intermediates.first() == Some(&b'?');
        if private && intermediates.len() == 1 {
            match action {
                'h' | 'l' => {
                    for &mode in &nums {
                        self.set_private_mode(usize::from(mode), action == 'h');
                    }
                }
                _ => {}
            }
            return;
        }
        if !intermediates.is_empty() {
            // Other intermediates (DECSCUSR's space, the `>` of DA2) change nothing kept here.
            return;
        }
        match action {
            '@' => {
                let n = arg(&nums, 0, 1).min(self.cols - self.col);
                let row = &mut self.grid[self.row].cells;
                for _ in 0..n {
                    row.pop();
                    row.insert(self.col, Cell { ch: ' ', width: 1, style: Style { bg: self.style.bg, ..Style::default() } });
                }
            }
            'A' | 'F' => {
                let (up, _) = self.move_limits();
                let n = arg(&nums, 0, 1);
                self.row = self.row.saturating_sub(n).max(up);
                self.wrap_next = false;
                if action == 'F' {
                    self.col = 0;
                }
            }
            'B' | 'E' => {
                let (_, down) = self.move_limits();
                let n = arg(&nums, 0, 1);
                self.row = (self.row + n).min(down);
                self.wrap_next = false;
                if action == 'E' {
                    self.col = 0;
                }
            }
            'C' => {
                self.col = (self.col + arg(&nums, 0, 1)).min(self.cols - 1);
                self.wrap_next = false;
            }
            'D' => {
                self.col = self.col.saturating_sub(arg(&nums, 0, 1));
                self.wrap_next = false;
            }
            'G' => {
                self.col = (arg(&nums, 0, 1) - 1).min(self.cols - 1);
                self.wrap_next = false;
            }
            'H' | 'f' => {
                let row = arg(&nums, 0, 1) - 1;
                let col = arg(&nums, 1, 1) - 1;
                self.place(row, col);
            }
            'd' => {
                let row = arg(&nums, 0, 1) - 1;
                let col = self.col;
                self.place(row, col);
            }
            'J' => match nums.first().copied().unwrap_or(0) {
                0 => {
                    let (row, col) = (self.row, self.col);
                    self.erase_cells(row, col, self.cols);
                    for y in self.row + 1..self.rows {
                        self.erase_cells(y, 0, self.cols);
                    }
                }
                1 => {
                    for y in 0..self.row {
                        self.erase_cells(y, 0, self.cols);
                    }
                    let (row, col) = (self.row, self.col);
                    self.erase_cells(row, 0, col + 1);
                }
                2 => {
                    for y in 0..self.rows {
                        self.erase_cells(y, 0, self.cols);
                    }
                }
                3 => {
                    self.dropped += self.scrollback.len() as u64;
                    self.scrollback.clear();
                    self.view = 0;
                }
                _ => {}
            },
            'K' => {
                let row = self.row;
                match nums.first().copied().unwrap_or(0) {
                    0 => self.erase_cells(row, self.col, self.cols),
                    1 => self.erase_cells(row, 0, self.col + 1),
                    2 => {
                        self.erase_cells(row, 0, self.cols);
                        self.grid[row].wrapped = false;
                    }
                    _ => {}
                }
            }
            'X' => {
                let row = self.row;
                let from = self.col;
                self.erase_cells(row, from, from + arg(&nums, 0, 1));
            }
            'L' | 'M' => {
                if self.row < self.top || self.row > self.bottom {
                    return;
                }
                let n = arg(&nums, 0, 1).min(self.bottom - self.row + 1);
                for _ in 0..n {
                    if action == 'L' {
                        self.grid.remove(self.bottom);
                        let blank = self.blank_row();
                        self.grid.insert(self.row, blank);
                    } else {
                        self.grid.remove(self.row);
                        let blank = self.blank_row();
                        self.grid.insert(self.bottom, blank);
                    }
                }
                self.col = 0;
                self.wrap_next = false;
            }
            'P' => {
                let row = self.row;
                let n = arg(&nums, 0, 1).min(self.cols - self.col);
                let blank = self.blank_cell();
                let cells = &mut self.grid[row].cells;
                for _ in 0..n {
                    cells.remove(self.col);
                    cells.push(blank);
                }
            }
            'S' => self.scroll_up(arg(&nums, 0, 1)),
            'T' => self.scroll_down(arg(&nums, 0, 1)),
            'g' => match nums.first().copied().unwrap_or(0) {
                0 => self.tabs[self.col] = false,
                3 => self.tabs.iter_mut().for_each(|tab| *tab = false),
                _ => {}
            },
            'r' => {
                let top = arg(&nums, 0, 1) - 1;
                let bottom = arg(&nums, 1, self.rows) - 1;
                if top < bottom && bottom < self.rows {
                    self.top = top;
                    self.bottom = bottom;
                } else {
                    self.top = 0;
                    self.bottom = self.rows - 1;
                }
                self.place(0, 0);
            }
            'n' => match nums.first().copied() {
                Some(5) => self.replies.extend_from_slice(b"\x1b[0n"),
                Some(6) => {
                    let report = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                    self.replies.extend_from_slice(report.as_bytes());
                }
                _ => {}
            },
            'c' => {
                if nums.first().copied().unwrap_or(0) == 0 {
                    self.replies.extend_from_slice(b"\x1b[?62;22c");
                }
            }
            // The parser reports a bare `CSI s` as one default parameter, so "no parameters" is all zeros.
            's' if nums.iter().all(|&n| n == 0) => self.saved = Some(self.save_cursor()),
            'u' if nums.iter().all(|&n| n == 0) => {
                let saved = self.saved;
                self.restore_cursor(saved);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            // Character set designations (`ESC ( B` and the like) are not kept.
            return;
        }
        match byte {
            b'7' => self.saved = Some(self.save_cursor()),
            b'8' => {
                let saved = self.saved;
                self.restore_cursor(saved);
            }
            b'D' => {
                self.index();
                self.wrap_next = false;
            }
            b'E' => {
                self.col = 0;
                self.index();
                self.wrap_next = false;
            }
            b'M' => {
                self.reverse_index();
                self.wrap_next = false;
            }
            b'H' => self.tabs[self.col] = true,
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if let [code, text, ..] = params
            && matches!(*code, [b'0'] | [b'2'])
        {
            self.title = String::from_utf8_lossy(text).into_owned();
        }
    }
}
