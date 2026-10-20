//! The document artifact body: Markdown drawn with iced's Markdown widget in the console's type and palette. The
//! screen parses a document once when it loads it ([`parse`] into [`Documents`](super::Documents)); until then the raw
//! text shows as prose.

use super::{Documents, ViewAction};
use crate::theme::{
    MONO, SANS, TYPE_CODE, TYPE_ITEM, TYPE_PAGE, TYPE_PROSE, TYPE_SECTION, palette,
};
use iced::advanced::text::Highlight;
use iced::widget::markdown;
use iced::widget::text;
use iced::{Background, Border, Element, Padding};

/// Parses a document's Markdown into the items [`view`] draws; the screen keeps them in [`Documents`](super::Documents).
pub fn parse(markdown_text: &str) -> Vec<markdown::Item> {
    markdown::parse(markdown_text).collect()
}

/// The Markdown settings of the console: prose at the reading size, headings on the type roles, code in the mono
/// voice, inline code on the sheet's second tone, and links in the signal ink.
fn settings() -> markdown::Settings {
    let p = *palette();
    let mut s = markdown::Settings::with_text_size(
        TYPE_PROSE,
        markdown::Style {
            font: SANS,
            inline_code_highlight: Highlight {
                background: Background::Color(p.sheet_2),
                border: Border {
                    radius: 3.0.into(),
                    ..Border::default()
                },
            },
            inline_code_padding: Padding {
                top: 1.0,
                right: 4.0,
                bottom: 1.0,
                left: 4.0,
            },
            inline_code_color: p.ink,
            inline_code_font: MONO,
            code_block_font: MONO,
            link_color: p.signal_ink,
        },
    );
    s.h1_size = TYPE_PAGE.into();
    s.h2_size = TYPE_SECTION.into();
    s.h3_size = TYPE_ITEM.into();
    s.h4_size = TYPE_PROSE.into();
    s.h5_size = TYPE_PROSE.into();
    s.h6_size = TYPE_PROSE.into();
    s.code_size = TYPE_CODE.into();
    s.spacing = 12.0.into();
    s
}

/// One document panel: its parsed items when the screen has them, else the raw Markdown as prose. A link press emits
/// `OpenLink` with the link's target.
pub fn view<'a, M: 'a>(
    panel_id: &'a str,
    markdown_text: &'a str,
    docs: &'a Documents,
    on: &'a dyn Fn(ViewAction) -> M,
) -> Element<'a, M> {
    match docs.get(panel_id) {
        Some(items) => {
            markdown::view(items.iter(), settings()).map(move |uri| on(ViewAction::OpenLink(uri)))
        }
        None => text(markdown_text)
            .size(TYPE_PROSE)
            .color(palette().ink)
            .font(SANS)
            .into(),
    }
}
