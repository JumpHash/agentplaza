//! The terminal UI's look, shared by the chrome and the screens: the spinner, the pulse and the
//! shimmer on the animation clock, the colour blend, key hints that fit a width, the panels, chips,
//! status dots and toasts. Everything animated takes `app.clock`, so a recording is deterministic.

use std::f32::consts::TAU;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use super::app::Palette;

/// The braille frames of the spinner, twelve a second.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The shimmer's sweep: one pass across the text every this many seconds.
pub const SHIMMER_PERIOD: f32 = 1.6;

/// How long a toast stays up, in seconds.
pub const TOAST_SECS: f32 = 4.0;

/// The toast's slide-in, in seconds.
const SLIDE_SECS: f32 = 0.25;

/// The spinner frame at `clock` (seconds).
pub fn spinner(clock: f32) -> &'static str {
    let frame = (clock.max(0.0) * 12.0) as usize;
    SPINNER[frame % SPINNER.len()]
}

/// A smooth wave between 0 and 1 with the given period in seconds; 0.5 at the start.
pub fn pulse(clock: f32, period: f32) -> f32 {
    if period <= 0.0 {
        return 0.5;
    }
    0.5 + 0.5 * (TAU * clock / period).sin()
}

/// The blend of two colours: `t` of the way from `a` to `b`. Colours that are not RGB cannot blend,
/// so they switch at the halfway point instead.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let t = t.clamp(0.0, 1.0);
            let lerp =
                |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
            Color::Rgb(lerp(r1, r2), lerp(g1, g2), lerp(b1, b2))
        }
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// The text with a highlight band about four characters wide sweeping across it every
/// `SHIMMER_PERIOD` seconds, one span per character. Without RGB colours the band is bold instead.
#[allow(dead_code)] // the chat's "thinking" line takes it
pub fn shimmer(text: &str, clock: f32, base: Color, hi: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as f32;
    let phase = (clock.max(0.0) % SHIMMER_PERIOD) / SHIMMER_PERIOD;
    // The band's centre runs from just before the text to just past it.
    let centre = -2.0 + phase * (n + 4.0);
    let rgb = matches!(base, Color::Rgb(..));
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let distance = (i as f32 + 0.5 - centre).abs();
            let weight = (1.0 - distance / 2.0).clamp(0.0, 1.0);
            let mut style = Style::new().fg(mix(base, hi, weight));
            if !rgb && weight >= 0.5 {
                style = style.add_modifier(Modifier::BOLD);
            }
            Span::styled(c.to_string(), style)
        })
        .collect()
}

/// One key hint: the key in ink and bold, then its label in muted after a space.
pub fn key_hint(key: &str, label: &str, p: &Palette) -> [Span<'static>; 2] {
    [
        Span::styled(
            key.to_string(),
            Style::new().fg(p.ink()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), Style::new().fg(p.muted)),
    ]
}

/// As many key hints as fit in `width` columns, separated by two spaces. Hints are whole: the first
/// one that does not fit ends the line.
pub fn hints(items: &[(&str, &str)], width: u16, p: &Palette) -> Line<'static> {
    let budget = usize::from(width);
    let mut spans = Vec::new();
    let mut used = 0;
    for (i, (key, label)) in items.iter().enumerate() {
        let gap = if i == 0 { 0 } else { 2 };
        let cost = gap + key.chars().count() + 1 + label.chars().count();
        if used + cost > budget {
            break;
        }
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.extend(key_hint(key, label, p));
        used += cost;
    }
    Line::from(spans)
}

/// A bordered panel with a rounded frame and its title; the accent marks the focused one.
pub fn panel(title: &str, p: &Palette, focused: bool) -> Block<'static> {
    let (edge, ink) = if focused {
        (p.accent, p.accent)
    } else {
        (p.rule_strong(), p.ink2())
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(edge))
        .title(Span::styled(
            format!(" {} ", title.trim()),
            Style::new().fg(ink).add_modifier(Modifier::BOLD),
        ))
}

/// A short label on a coloured background, bold. With no background colour (`NO_COLOR`) it is
/// reversed instead, so it still stands out.
pub fn chip(text: &str, fg: Color, bg: Color) -> Span<'static> {
    let style = Style::new().fg(fg).add_modifier(Modifier::BOLD);
    let text = format!(" {text} ");
    if bg == Color::Reset {
        Span::styled(text, style.add_modifier(Modifier::REVERSED))
    } else {
        Span::styled(text, style.bg(bg))
    }
}

/// A status dot: a spinner while working, a filled dot for idle (ok) and failed (crit), a square
/// for held (warn), a hollow dot for anything else.
#[allow(dead_code)] // the body screens' rows take it
pub fn dot(status: &str, clock: f32, p: &Palette) -> Span<'static> {
    let (glyph, color) = match status {
        "working" => (spinner(clock), p.accent),
        "idle" => ("●", p.ok),
        "held" | "paused" => ("◼", p.warn()),
        "failed" | "error" => ("●", p.crit()),
        _ => ("○", p.muted),
    };
    Span::styled(glyph, Style::new().fg(color))
}

/// A toast at the top right of `area`: a rounded box with the notice, sliding in from the right
/// edge during its first quarter second. `age` is seconds since it appeared.
pub fn toast(frame: &mut Frame, area: Rect, text: &str, age: f32, p: &Palette) {
    let width = (text.chars().count() as u16 + 6).min(60).min(area.width);
    if width < 8 || area.height < 3 {
        return;
    }
    let rest = area.right().saturating_sub(width + 1);
    let t = (age / SLIDE_SECS).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    let offset = ((1.0 - eased) * 8.0).round() as u16;
    let x = rest.saturating_add(offset).min(area.right());
    let visible = (area.right() - x).min(width);
    if visible < 4 {
        return;
    }
    let rect = Rect::new(x, area.y, visible, 3);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(p.ok))
        .style(Style::new().bg(p.sheet()));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let line = Line::from(vec![
        Span::styled("✓ ", Style::new().fg(p.ok).add_modifier(Modifier::BOLD)),
        Span::styled(text.to_string(), Style::new().fg(p.ink())),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_spinner_turns_twelve_frames_a_second() {
        assert_eq!(spinner(0.0), "⠋");
        assert_eq!(spinner(0.1), "⠙");
        assert_eq!(spinner(0.84), "⠋", "ten frames, then it starts over");
        assert_eq!(spinner(-1.0), "⠋", "a negative clock is the start");
    }

    #[test]
    fn the_pulse_is_a_smooth_wave_from_zero_to_one() {
        assert!((pulse(0.0, 2.4) - 0.5).abs() < 1e-6);
        assert!(
            (pulse(0.6, 2.4) - 1.0).abs() < 1e-6,
            "a quarter period is the top"
        );
        assert!(
            (pulse(1.8, 2.4) - 0.0).abs() < 1e-6,
            "three quarters is the bottom"
        );
        for step in 0..100 {
            let v = pulse(step as f32 * 0.07, 2.4);
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn colours_blend_when_they_are_rgb_and_switch_otherwise() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(mix(black, white, 0.5), Color::Rgb(128, 128, 128));
        assert_eq!(mix(black, white, 2.0), white, "t is clamped");
        assert_eq!(mix(Color::Red, Color::Blue, 0.2), Color::Red);
        assert_eq!(mix(Color::Red, Color::Blue, 0.8), Color::Blue);
    }

    #[test]
    fn hints_keep_whole_items_and_drop_the_rest() {
        let p = Palette::detect();
        let items = [("q", "quit"), ("r", "refresh"), ("Enter", "details")];
        let full = hints(&items, 80, &p);
        assert_eq!(text(&full.spans), "q quit  r refresh  Enter details");
        // "q quit  r refresh" is 17 columns: a width of 17 keeps it, 16 drops the second item.
        assert_eq!(text(&hints(&items, 17, &p).spans), "q quit  r refresh");
        assert_eq!(text(&hints(&items, 16, &p).spans), "q quit");
        assert_eq!(
            text(&hints(&items, 5, &p).spans),
            "",
            "nothing fits in five columns"
        );
        assert!(hints(&[], 80, &p).spans.is_empty());
    }

    #[test]
    fn the_shimmer_keeps_the_text_and_sweeps_a_band() {
        let base = Color::Rgb(0x95, 0x8f, 0x83);
        let hi = Color::Rgb(0xec, 0xe8, 0xdf);
        assert_eq!(text(&shimmer("thinking…", 0.0, base, hi)), "thinking…");
        let lit = |spans: &[Span]| {
            spans
                .iter()
                .enumerate()
                .filter(|(_, s)| s.style.fg != Some(base))
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        };
        // At 0.3 s the band's centre is on the first characters; at 1 s it is in the middle.
        let early = lit(&shimmer("thinking…", 0.3, base, hi));
        let later = lit(&shimmer("thinking…", 1.0, base, hi));
        assert!(
            !early.is_empty(),
            "the band is on the text early in the sweep"
        );
        assert!(early.len() <= 4, "the band is about four characters wide");
        assert_ne!(early, later, "the band moves");
    }

    #[test]
    fn the_status_dots_show_each_state() {
        let p = Palette::detect();
        assert_eq!(dot("working", 0.0, &p).content, "⠋");
        assert_eq!(dot("idle", 0.0, &p).content, "●");
        assert_eq!(dot("held", 0.0, &p).content, "◼");
        assert_eq!(dot("failed", 0.0, &p).content, "●");
        assert_eq!(dot("gone", 0.0, &p).content, "○");
    }

    #[test]
    fn chips_are_reversed_when_there_is_no_background() {
        let plain = chip("MODELS", Color::Reset, Color::Reset);
        assert_eq!(plain.content, " MODELS ");
        assert!(plain.style.add_modifier.contains(Modifier::REVERSED));
        let filled = chip("HELD", Color::Black, Color::Red);
        assert!(!filled.style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(filled.style.bg, Some(Color::Red));
    }
}
