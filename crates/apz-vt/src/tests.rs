//! The emulator fed real byte sequences: what `ls --color`, `clear`, vim and a scrolling pager send.

use crate::{Color, Emulator, SCROLLBACK};

fn emu(cols: usize, rows: usize, bytes: &str) -> Emulator {
    let mut e = Emulator::new(cols, rows);
    e.feed(bytes.as_bytes());
    e
}

fn line(e: &Emulator, y: usize) -> String {
    let screen = e.screen();
    let row = screen.view_row(y).expect("a row on the view");
    row.cells
        .iter()
        .filter(|c| c.width != 0)
        .map(|c| c.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn ls_color_output_puts_each_name_in_its_colour_and_resets() {
    // `ls --color` style: directories blue and bold, scripts green, each followed by a reset.
    let e = emu(
        40,
        5,
        "\x1b[0m\x1b[01;34mdir\x1b[0m  \x1b[01;32mrun.sh\x1b[0m\r\n",
    );
    let s = e.screen();
    assert_eq!(line(&e, 0), "dir  run.sh");
    assert_eq!(s.cursor(), (1, 0));
    let d = s.line(0).unwrap().cells[0].style;
    assert_eq!(d.fg, Color::Indexed(4));
    assert!(d.bold);
    let r = s.line(0).unwrap().cells[5].style;
    assert_eq!(r.fg, Color::Indexed(2));
    assert!(r.bold, "01 is bold too, as ls sends it");
    let gap = s.line(0).unwrap().cells[3].style;
    assert_eq!(gap.fg, Color::Default, "the reset clears the colour");
    assert!(!gap.bold);
}

#[test]
fn clear_homes_the_cursor_and_erases_the_screen_and_the_scrollback() {
    let mut e = emu(10, 3, "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
    assert!(
        e.screen().history_len() > 0,
        "lines scrolled off into the history"
    );
    e.feed(b"\x1b[H\x1b[2J\x1b[3J");
    let s = e.screen();
    assert_eq!(s.cursor(), (0, 0));
    assert_eq!(s.history_len(), 0, "ED 3 drops the scrollback");
    for y in 0..3 {
        assert_eq!(line(&e, y), "");
    }
}

#[test]
fn alternate_screen_hides_the_main_one_and_restores_it_with_the_cursor() {
    let mut e = emu(20, 4, "main text");
    let main_cursor = e.screen().cursor();
    e.feed(b"\x1b[?1049h");
    assert!(e.screen().alt_screen());
    assert_eq!(line(&e, 0), "", "the alternate screen starts blank");
    e.feed(b"\x1b[1;1Hvim buffer\x1b[2;1H~");
    assert_eq!(line(&e, 0), "vim buffer");
    assert_eq!(line(&e, 1), "~");
    assert_eq!(
        e.screen().history_len(),
        0,
        "the alternate screen keeps no history"
    );
    e.feed(b"\x1b[?1049l");
    let s = e.screen();
    assert!(!s.alt_screen());
    assert_eq!(line(&e, 0), "main text");
    assert_eq!(s.cursor(), main_cursor);
}

#[test]
fn scroll_region_scrolls_only_its_rows() {
    let mut e = emu(10, 6, "\x1b[1;1HAAAA\x1b[6;1HBBBB");
    e.feed(b"\x1b[2;4r"); // rows 2 to 4, one-based: the region is rows 1 to 3.
    e.feed(b"\x1b[2;1Hr1\x1b[3;1Hr2\x1b[4;1Hr3");
    e.feed(b"\x1b[4;1H\n");
    assert_eq!(line(&e, 0), "AAAA", "above the region is untouched");
    assert_eq!(line(&e, 1), "r2");
    assert_eq!(line(&e, 2), "r3");
    assert_eq!(line(&e, 3), "", "the region's new bottom row is blank");
    assert_eq!(line(&e, 5), "BBBB", "below the region is untouched");
    assert_eq!(
        e.screen().history_len(),
        0,
        "a region below the top does not feed the scrollback"
    );
}

#[test]
fn reverse_index_at_the_top_margin_scrolls_the_region_down() {
    let mut e = emu(10, 3, "top\r\nmid\r\nbot");
    e.feed(b"\x1b[1;1H\x1bM");
    assert_eq!(line(&e, 0), "");
    assert_eq!(line(&e, 1), "top");
    assert_eq!(line(&e, 2), "mid");
}

#[test]
fn wide_characters_take_two_cells_and_wrap_whole() {
    let e = emu(6, 3, "漢字a");
    let s = e.screen();
    assert_eq!(s.line(0).unwrap().cells[0].width, 2);
    assert_eq!(s.line(0).unwrap().cells[1].width, 0);
    assert_eq!(s.line(0).unwrap().cells[2].ch, '字');
    assert_eq!(line(&e, 0), "漢字a");
    assert_eq!(s.cursor(), (0, 5));

    // Three columns: the second wide character does not fit after the first and moves down whole.
    let e = emu(3, 3, "漢字");
    let s = e.screen();
    assert_eq!(line(&e, 0), "漢");
    assert_eq!(line(&e, 1), "字");
    assert!(s.line(0).unwrap().wrapped);
}

#[test]
fn text_wrapped_at_the_edge_copies_without_a_newline() {
    let mut e = emu(5, 3, "abcdefgh");
    assert!(e.screen().line(0).unwrap().wrapped);
    assert_eq!(line(&e, 0), "abcde");
    assert_eq!(line(&e, 1), "fgh");
    let abs0 = e.screen().abs_of_index(0);
    let abs1 = e.screen().abs_of_index(1);
    assert_eq!(e.screen().text_between((abs0, 0), (abs1, 2)), "abcdefgh");
    e.feed(b"\r\nxy");
    let s = e.screen();
    assert_eq!(
        s.text_between((abs0, 0), (s.abs_of_index(2), 1)),
        "abcdefgh\nxy"
    );
}

#[test]
fn scrollback_keeps_history_that_scrolls_back_and_copies() {
    let mut text = String::new();
    for i in 0..30 {
        text.push_str(&format!("line {i}\r\n"));
    }
    let mut e = emu(10, 5, &text);
    assert_eq!(e.screen().history_len(), 26);
    assert_eq!(e.screen().line_text(0), "line 0");
    assert_eq!(e.screen().line_text(25), "line 25");
    e.screen_mut().scroll_view(10);
    assert_eq!(
        line(&e, 0),
        "line 16",
        "ten lines back from the bottom of the history"
    );
    assert!(
        e.screen().cursor_on_view().is_none(),
        "the cursor is out of view when scrolled back"
    );
    e.screen_mut().reset_view();
    assert!(e.screen().cursor_on_view().is_some());

    let s = e.screen();
    let (a, b) = (s.abs_of_index(3), s.abs_of_index(5));
    assert_eq!(s.text_between((a, 0), (b, 5)), "line 3\nline 4\nline 5");
}

#[test]
fn scrollback_is_capped_and_counts_what_it_dropped() {
    let mut e = Emulator::new(10, 3);
    let text = "x\r\n".repeat(10_050);
    e.feed(text.as_bytes());
    let s = e.screen();
    assert_eq!(s.history_len(), SCROLLBACK);
    // The first two line feeds only move the cursor down the three rows; the rest each push a row.
    assert_eq!(s.dropped(), 10_048 - SCROLLBACK as u64);
    assert_eq!(s.index_of_abs(s.dropped()), Some(0));
    assert_eq!(
        s.index_of_abs(0),
        None,
        "an id that fell off the scrollback is gone"
    );
}

#[test]
fn resize_cuts_or_pads_the_columns_and_keeps_the_cursor_on_screen() {
    let mut e = emu(10, 3, "abcdef");
    e.resize(4, 3);
    assert_eq!(line(&e, 0), "abcd");
    assert_eq!(
        e.screen().cursor().1,
        3,
        "the cursor is clamped to the new width"
    );
    e.resize(8, 3);
    assert_eq!(line(&e, 0), "abcd", "text cut away is not brought back");
}

#[test]
fn shrinking_the_rows_pushes_the_top_lines_into_the_history() {
    let mut e = emu(5, 4, "1\r\n2\r\n3\r\n4");
    assert_eq!(e.screen().cursor(), (3, 1));
    e.resize(5, 2);
    let s = e.screen();
    assert_eq!(s.history_len(), 2);
    assert_eq!(line(&e, 0), "3");
    assert_eq!(line(&e, 1), "4");
    assert_eq!(e.screen().cursor(), (1, 1));
}

#[test]
fn extended_colours_read_in_both_the_semicolon_and_colon_forms() {
    let e = emu(
        12,
        2,
        "\x1b[38;5;196mA\x1b[48;2;1;2;3mB\x1b[38:2::10:20:30mC\x1b[38;2;9;8;7mD\x1b[0mE",
    );
    let cells = &e.screen().line(0).unwrap().cells;
    assert_eq!(cells[0].style.fg, Color::Indexed(196));
    assert_eq!(cells[1].style.bg, Color::Rgb(1, 2, 3));
    assert_eq!(cells[2].style.fg, Color::Rgb(10, 20, 30));
    assert_eq!(cells[3].style.fg, Color::Rgb(9, 8, 7));
    assert_eq!(cells[4].style.fg, Color::Default);
}

#[test]
fn sixteen_colours_attributes_and_their_resets() {
    let e = emu(
        12,
        2,
        "\x1b[91;42mA\x1b[7;3;4mB\x1b[22;23;24;27mC\x1b[4:3mD",
    );
    let cells = &e.screen().line(0).unwrap().cells;
    assert_eq!(cells[0].style.fg, Color::Indexed(9), "bright red");
    assert_eq!(cells[0].style.bg, Color::Indexed(2), "green background");
    assert!(cells[1].style.inverse && cells[1].style.italic && cells[1].style.underline);
    assert!(!cells[2].style.inverse && !cells[2].style.italic && !cells[2].style.underline);
    assert!(
        cells[3].style.underline,
        "4:3 is a curly underline, which still underlines"
    );
}

#[test]
fn application_cursor_keys_and_bracketed_paste_are_reported() {
    let mut e = emu(10, 2, "");
    assert!(!e.screen().app_cursor());
    e.feed(b"\x1b[?1h");
    assert!(e.screen().app_cursor());
    e.feed(b"\x1b[?1l");
    assert!(!e.screen().app_cursor());
    e.feed(b"\x1b[?2004h");
    assert!(e.screen().bracketed_paste());
    e.feed(b"\x1b[?2004l");
    assert!(!e.screen().bracketed_paste());
}

#[test]
fn the_cursor_can_be_hidden_and_shown() {
    let mut e = emu(10, 2, "");
    e.feed(b"\x1b[?25l");
    assert!(!e.screen().cursor_visible());
    assert!(e.screen().cursor_on_view().is_none());
    e.feed(b"\x1b[?25h");
    assert_eq!(e.screen().cursor_on_view(), Some((0, 0)));
}

#[test]
fn erase_in_line_and_erase_with_a_background_colour() {
    let e = emu(10, 2, "abcdef\x1b[1;4H\x1b[K");
    assert_eq!(line(&e, 0), "abc");
    let e = emu(4, 2, "\x1b[41m\x1b[K");
    assert_eq!(
        e.screen().line(0).unwrap().cells[2].style.bg,
        Color::Indexed(1),
        "erased cells keep the background"
    );
}

#[test]
fn insert_and_delete_characters_and_lines() {
    let e = emu(10, 2, "abcd\x1b[1;2H\x1b[2@");
    assert_eq!(line(&e, 0), "a  bcd", "two blanks inserted at column 2");
    let e = emu(10, 2, "abcdef\x1b[1;2H\x1b[2P");
    assert_eq!(line(&e, 0), "adef", "two characters deleted at column 2");

    let mut e = emu(4, 4, "A\r\nB\r\nC");
    e.feed(b"\x1b[2;1H\x1b[1L");
    assert_eq!(
        (line(&e, 0), line(&e, 1), line(&e, 2)),
        ("A".into(), "".into(), "B".into())
    );
    e.feed(b"\x1b[2;1H\x1b[1M");
    assert_eq!(
        (line(&e, 0), line(&e, 1), line(&e, 2), line(&e, 3)),
        ("A".into(), "B".into(), "C".into(), "".into())
    );
}

#[test]
fn cursor_reports_and_device_attributes_are_owed_back() {
    let mut e = emu(10, 5, "\x1b[3;5H\x1b[6n\x1b[5n\x1b[c");
    assert_eq!(
        e.screen_mut().take_replies(),
        b"\x1b[3;5R\x1b[0n\x1b[?62;22c".to_vec()
    );
    assert!(e.screen_mut().take_replies().is_empty());
}

#[test]
fn window_titles_bells_and_tabs() {
    let mut e = emu(20, 2, "\x1b]0;hello\x07\x1b]2;world\x1b\\");
    assert_eq!(e.screen().title(), "world");
    e.feed(b"\x07");
    assert!(e.screen_mut().take_bell());
    assert!(!e.screen_mut().take_bell());
    e.feed(b"\x1b[H\ta\tb");
    assert_eq!(e.screen().cursor(), (0, 17));
    assert_eq!(e.screen().line(0).unwrap().cells[8].ch, 'a');
    assert_eq!(e.screen().line(0).unwrap().cells[16].ch, 'b');
}

#[test]
fn saved_cursor_is_restored_by_both_forms() {
    let mut e = emu(10, 4, "\x1b[2;3H\x1b7\x1b[4;4H\x1b8");
    assert_eq!(e.screen().cursor(), (1, 2));
    e.feed(b"\x1b[4;4H\x1b[s\x1b[1;1H\x1b[u");
    assert_eq!(e.screen().cursor(), (3, 3));
}

#[test]
fn origin_mode_places_the_cursor_inside_the_region() {
    let e = emu(10, 6, "\x1b[2;4r\x1b[?6h\x1b[1;1Hx");
    assert_eq!(line(&e, 1), "x");
    assert_eq!(e.screen().cursor(), (1, 1));
}

#[test]
fn sequences_split_across_reads_are_applied_once_complete() {
    let mut e = Emulator::new(10, 2);
    e.feed(b"\x1b[3");
    e.feed(b"1mX");
    let cell = e.screen().line(0).unwrap().cells[0];
    assert_eq!(cell.ch, 'X');
    assert_eq!(cell.style.fg, Color::Indexed(1));
}
