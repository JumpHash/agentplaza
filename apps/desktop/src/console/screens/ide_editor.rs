//! The IDE's editor: the tab strip, the active tab's `text_editor` (highlighted), the conflict banner
//! over it, and the quick-open list. Highlight colours are the palette's own: each syntax colour maps
//! to the nearest palette role, so the editor adds no colour of its own.

use std::collections::BTreeMap;

use iced::advanced::text::highlighter::Format;
use iced::highlighter::{Highlight, Theme as SyntaxTheme};
use iced::widget::{Space, button, column, container, row, text_editor, text_input};
use iced::{Alignment, Border, Color, Element, Font, Length, Theme};

use super::ide::{Msg, Tab};
use crate::console::ui::{self, Kind};
use crate::theme;

/// Quick open lists at most this many paths.
pub const QUICK_LIMIT: usize = 40;

/// The file's name, for a tab.
pub fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The syntax token for a path: its extension, or its name when it has none (`Makefile`). A leading
/// dot is not part of the name (`.gitignore` is read as `gitignore`).
pub fn token_for(path: &str) -> String {
    let name = name_of(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => name.trim_start_matches('.').to_ascii_lowercase(),
    }
}

/// The syntax theme for the app's register: a dark one on the night page, a light one on the light page.
pub fn syntax_theme() -> SyntaxTheme {
    if theme::is_light() {
        SyntaxTheme::InspiredGitHub
    } else {
        SyntaxTheme::Base16Eighties
    }
}

/// The palette role nearest to `c` (by RGB distance). Ties keep the earlier role.
pub fn nearest(c: Color, roles: &[Color]) -> Color {
    let dist = |r: &Color| (c.r - r.r).powi(2) + (c.g - r.g).powi(2) + (c.b - r.b).powi(2);
    roles
        .iter()
        .copied()
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))
        .unwrap_or(c)
}

/// A highlight as a format: its colour mapped onto the palette, its font as the highlighter gives it.
fn format(h: &Highlight, _theme: &Theme) -> Format<Font> {
    let p = theme::palette();
    let roles = [
        p.ink,
        p.ink_2,
        p.ink_3,
        p.signal,
        p.signal_ink,
        p.info,
        p.ok,
        p.warn,
        p.crit,
    ];
    Format {
        color: h.color().map(|c| nearest(c, &roles)),
        font: h.font(),
    }
}

/// The editor's own colours: the sheet, a ruled border that turns ink-3 when focused, the palette's text.
fn editor_style(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let p = theme::palette();
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: p.sheet.into(),
        border: Border {
            color: if focused { p.ink_3 } else { p.rule },
            width: 1.0,
            radius: 0.0.into(),
        },
        placeholder: p.ink_3,
        value: p.ink,
        selection: Color { a: 0.3, ..p.signal },
    }
}

/// The tab strip: one tab per open file (its name, a dirty dot, a conflict mark), each with a close.
pub fn tab_strip<'a>(
    open: &'a [String],
    active: Option<&'a str>,
    tabs: &'a BTreeMap<String, Tab>,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut strip = row![].spacing(2).align_y(Alignment::Center);
    for path in open {
        let on = active == Some(path.as_str());
        let tab = tabs.get(path);
        let mut label = row![ui::mono(name_of(path).to_string(), 11.5, p.ink)]
            .spacing(6)
            .align_y(Alignment::Center);
        if tab.is_some_and(|t| t.buffer.dirty) {
            label = label.push(ui::mono("•", 11.5, p.signal));
        }
        if tab.is_some_and(|t| t.conflict.is_some()) {
            label = label.push(ui::mono("!", 11.5, p.crit));
        }
        let pick = button(label)
            .on_press(Msg::Activate(path.clone()))
            .padding([5, 8])
            .style(theme::register_row(on));
        let close = button(ui::icon("x", 11.0, p.ink_3))
            .on_press(Msg::Close(path.clone()))
            .padding([5, 4])
            .style(theme::register_row(false));
        strip = strip.push(row![pick, close].align_y(Alignment::Center));
    }
    ui::scroll_x(strip).width(Length::Fill).into()
}

/// The active tab: a conflict banner when the hub's file moved on, then the editor. With no write
/// access the editor is read-only and says so.
pub fn editor<'a>(path: &'a str, tab: &'a Tab, writable: bool) -> Element<'a, Msg> {
    let mut col = column![].spacing(10).height(Length::Fill);
    if let Some(conflict) = &tab.conflict {
        let hub = match &conflict.server_hash {
            Some(_) => "changed on the hub since you opened it",
            None => "was removed on the hub",
        };
        let banner = row![
            ui::badge(&format!("conflict: {hub}"), ui::Tone::Danger),
            Space::new().width(Length::Fill),
            ui::btn_sm(
                "Reload",
                None,
                Kind::Secondary,
                Some(Msg::Reload(path.to_string()))
            ),
            ui::btn_sm(
                "Keep mine",
                None,
                Kind::Primary,
                Some(Msg::KeepMine(path.to_string()))
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        col = col.push(
            container(banner)
                .padding([8, 10])
                .width(Length::Fill)
                .style(theme::notice(theme::Tone::Crit)),
        );
    }
    let mut input = text_editor(&tab.content)
        .font(theme::MONO)
        .size(13.0)
        .height(Length::Fill)
        .padding(10)
        .style(editor_style);
    if writable {
        let owned = path.to_string();
        input = input.on_action(move |action| Msg::Edit(owned.clone(), action));
    }
    let settings = iced::highlighter::Settings {
        theme: syntax_theme(),
        token: token_for(path),
    };
    let input = input.highlight_with::<iced::highlighter::Highlighter>(settings, format);
    col = col.push(input);
    if !writable {
        col = col.push(row![
            ui::badge("read only", ui::Tone::Muted),
            Space::new().width(Length::Fill)
        ]);
    }
    col.into()
}

/// The paths a quick-open query matches, in the order given, at most `QUICK_LIMIT`. An empty query
/// matches everything. Matching is a case-insensitive substring of the whole path.
pub fn matches(paths: &[String], query: &str) -> Vec<String> {
    let q = query.trim().to_ascii_lowercase();
    paths
        .iter()
        .filter(|path| q.is_empty() || path.to_ascii_lowercase().contains(&q))
        .take(QUICK_LIMIT)
        .cloned()
        .collect()
}

/// Quick open: a query box over the matching paths. Enter opens the first one.
pub fn quick_open<'a>(query: &'a str, hits: Vec<String>, id: iced::widget::Id) -> Element<'a, Msg> {
    let p = theme::palette();
    let input = text_input("Open a file by name", query)
        .id(id)
        .on_input(Msg::QuickQuery)
        .on_submit(Msg::QuickFirst)
        .style(theme::input)
        .padding(8);
    let mut list = column![].spacing(0);
    if hits.is_empty() {
        list = list.push(ui::faint(
            "No loaded file matches. Open folders in the tree to load more.",
        ));
    }
    for path in hits {
        list = list.push(
            button(ui::mono(path.clone(), 12.0, p.ink))
                .on_press(Msg::OpenPath(path))
                .padding([5, 8])
                .width(Length::Fill)
                .style(theme::register_row(false)),
        );
    }
    column![input, ui::scroll(list).height(Length::Fill)]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_the_extension_or_the_name() {
        assert_eq!(token_for("src/main.RS"), "rs");
        assert_eq!(token_for("Cargo.toml"), "toml");
        assert_eq!(token_for("Makefile"), "makefile");
        assert_eq!(
            token_for(".gitignore"),
            "gitignore",
            "a dotfile has no extension to read"
        );
    }

    #[test]
    fn a_colour_maps_to_the_nearest_palette_role() {
        let roles = [Color::from_rgb8(0, 0, 0), Color::from_rgb8(255, 255, 255)];
        assert_eq!(nearest(Color::from_rgb8(20, 20, 20), &roles), roles[0]);
        assert_eq!(nearest(Color::from_rgb8(240, 250, 230), &roles), roles[1]);
        assert_eq!(
            nearest(Color::from_rgb8(9, 9, 9), &[]),
            Color::from_rgb8(9, 9, 9),
            "no roles: the colour stays"
        );
    }

    #[test]
    fn quick_open_matches_a_substring_of_the_path_in_order() {
        let paths: Vec<String> = ["README.md", "src/ide.rs", "src/IDE_tree.rs", "docs/ide.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            matches(&paths, "IDE"),
            ["src/ide.rs", "src/IDE_tree.rs", "docs/ide.md"]
        );
        assert_eq!(matches(&paths, "  readme "), ["README.md"]);
        assert_eq!(matches(&paths, "").len(), 4);
        assert!(matches(&paths, "zzz").is_empty());
    }

    #[test]
    fn quick_open_stops_at_its_limit() {
        let paths: Vec<String> = (0..100).map(|i| format!("f{i}.txt")).collect();
        assert_eq!(matches(&paths, "").len(), QUICK_LIMIT);
    }
}
