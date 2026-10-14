//! A terminal emulator's screen, pure Rust and free of any UI or I/O: bytes go in through
//! [`Emulator::feed`], and the [`Screen`] says what is on the grid, where the cursor is, which
//! modes are set, and what the program is owed in reply.
//!
//! Covered: printable text with wide characters, C0 controls (CR, LF, BS, HT, BEL), cursor
//! movement (CUU/CUD/CUF/CUB/CNL/CPL/CHA/CUP/HVP/VPA), erase (ED/EL/ECH), insert and delete of
//! lines and characters (IL/DL/ICH/DCH), scroll regions (DECSTBM, SU/SD), SGR (bold, italic,
//! underline, inverse, 16/256/truecolour fg and bg, both the `;` and `:` forms), the alternate
//! screen (47, 1047, 1049), cursor visibility (25), application cursor keys (1), origin mode (6),
//! autowrap (7), bracketed paste (2004), cursor save and restore (DECSC/DECRC, `CSI s`/`CSI u`),
//! tab stops, cursor reports (`CSI 5 n`, `CSI 6 n`) and device attributes (`CSI c`), window titles
//! (OSC 0 and 2), and a 10 000 line scrollback. Not covered: mouse reporting, reflow on resize,
//! combining marks, and character sets.

mod screen;
mod width;

#[cfg(test)]
mod tests;

pub use screen::{Cell, Color, MAX_SIZE, Row, SCROLLBACK, Screen, Style};
pub use width::char_width;

/// The parser and the screen together: feed it the bytes of a program's output.
pub struct Emulator {
    parser: vte::Parser,
    screen: Screen,
}

impl Emulator {
    /// A blank emulator of `cols` by `rows` (each clamped to 1 to [`MAX_SIZE`]).
    pub fn new(cols: usize, rows: usize) -> Emulator {
        Emulator {
            parser: vte::Parser::new(),
            screen: Screen::new(cols, rows),
        }
    }

    /// Parses `bytes` and applies them to the screen. A sequence split across calls is kept until
    /// its end arrives.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    /// The screen as the bytes so far have left it.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// The screen, mutably: for the view, the replies and the bell, which the UI takes out.
    pub fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }

    /// Resizes the screen (see [`Screen::resize`]).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.screen.resize(cols, rows);
    }

    /// Moves the view through the scrollback (see [`Screen::scroll_view`]).
    pub fn scroll_view(&mut self, delta: isize) {
        self.screen.scroll_view(delta);
    }
}
