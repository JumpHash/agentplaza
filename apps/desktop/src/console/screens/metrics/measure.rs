//! Text widths in the app's own fonts, for laying tables out as a browser does (columns as wide as their content).
//! Widths are cached by text, font and size: a view is rebuilt on every pointer move, the table texts rarely change.

use std::cell::RefCell;
use std::collections::HashMap;

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{self, LineHeight, Paragraph as _, Shaping, Wrapping};
use iced::alignment::Vertical;
use iced::{Font, Pixels, Size};

/// Past this many cached widths the cache starts over (live tables bring new texts every few seconds).
const CACHE_CAP: usize = 20_000;

thread_local! {
    static WIDTHS: RefCell<HashMap<(String, Font, u32), f32>> = RefCell::new(HashMap::new());
}

/// The width of `content` on one line in `font` at `size`.
pub fn width(content: &str, font: Font, size: f32) -> f32 {
    if content.is_empty() {
        return 0.0;
    }
    let key = (content.to_string(), font, size.to_bits());
    if let Some(w) = WIDTHS.with(|c| c.borrow().get(&key).copied()) {
        return w;
    }
    let paragraph = Paragraph::with_text(text::Text {
        content,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: LineHeight::default(),
        font,
        align_x: text::Alignment::Default,
        align_y: Vertical::Top,
        shaping: Shaping::default(),
        wrapping: Wrapping::None,
    });
    // Rounded up, so a column measured at this width never wraps its text.
    let w = paragraph.min_bounds().width.ceil();
    WIDTHS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(key, w);
    });
    w
}

/// The width of the longest word of `content`: the narrowest a wrapping text can get. Text breaks at spaces only
/// (a name like "sonnet-5.5" stays whole; the web keeps it whole with non-breaking hyphens).
pub fn longest_word(content: &str, font: Font, size: f32) -> f32 {
    content
        .split_whitespace()
        .map(|word| width(word, font, size))
        .fold(0.0, f32::max)
}

/// `content` cut to `max` pixels with an ellipsis (the web's `text-overflow: ellipsis`), or whole when it fits.
pub fn ellipsize(content: &str, font: Font, size: f32, max: f32) -> String {
    if width(content, font, size) <= max {
        return content.to_string();
    }
    let chars: Vec<char> = content.chars().collect();
    let cut = |n: usize| -> String {
        let mut s: String = chars[..n].iter().collect::<String>().trim_end().to_string();
        s.push('…');
        s
    };
    // The longest prefix that fits with its ellipsis.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&cut(mid), font, size) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// A column's widths: `max` holds its widest content on one line, `min` the narrowest it can wrap to (the same as
/// `max` unless its cells are free text).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnWidth {
    pub min: f32,
    pub max: f32,
}

/// The columns' widths in `avail` pixels with `gap` between columns, as a browser's automatic table layout at 100 %
/// width sets them: when every column fits on one line, the spare width goes to each in proportion to its width;
/// when not, wrapping columns give up width in proportion to what they can give; when even the narrowest layout is
/// too wide, every column takes its narrowest and the table scrolls.
pub fn layout(columns: &[ColumnWidth], avail: f32, gap: f32) -> Vec<f32> {
    let gaps = gap * columns.len().saturating_sub(1) as f32;
    let room = (avail - gaps).max(0.0);
    let sum_max: f32 = columns.iter().map(|c| c.max).sum();
    let sum_min: f32 = columns.iter().map(|c| c.min).sum();
    if sum_max <= room {
        let spare = room - sum_max;
        return columns
            .iter()
            .map(|c| {
                if sum_max > 0.0 {
                    c.max + spare * c.max / sum_max
                } else {
                    room / columns.len() as f32
                }
            })
            .collect();
    }
    if sum_min <= room && sum_max > sum_min {
        let spare = room - sum_min;
        return columns
            .iter()
            .map(|c| c.min + spare * (c.max - c.min) / (sum_max - sum_min))
            .collect();
    }
    columns.iter().map(|c| c.min).collect()
}

#[cfg(test)]
mod tests {
    use super::{ColumnWidth, layout};

    fn col(min: f32, max: f32) -> ColumnWidth {
        ColumnWidth { min, max }
    }

    #[test]
    fn spare_width_goes_to_each_column_by_its_width() {
        let w = layout(&[col(50.0, 100.0), col(50.0, 50.0)], 310.0, 10.0);
        assert_eq!(w, vec![200.0, 100.0]);
    }

    #[test]
    fn wrapping_columns_give_up_width_first() {
        // 100 + 50 do not fit in 120; only the first column can wrap, down to 50.
        let w = layout(&[col(50.0, 100.0), col(50.0, 50.0)], 120.0, 0.0);
        assert_eq!(w, vec![70.0, 50.0]);
    }

    #[test]
    fn a_table_too_wide_keeps_its_narrowest_and_scrolls() {
        let w = layout(&[col(80.0, 100.0), col(50.0, 50.0)], 100.0, 0.0);
        assert_eq!(w, vec![80.0, 50.0]);
    }
}
