//! The image artifact body: the picture the screen has loaded, or a quiet box while its bytes are on the way. A press
//! opens the picture in the viewer.

use super::{Blobs, ViewAction};
use crate::theme::{TYPE_META, palette, skin};
use apz_view::ImageBody;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{column, container, image, mouse_area, text};
use iced::{Background, Border, ContentFit, Element, Length, mouse};

/// One image panel: the picture (contained in the panel's width), its caption (or else its alt text) under it, and a press on
/// the picture that emits `OpenImage` with the blob's id.
pub fn view<'a, M: Clone + 'a>(
    i: &'a ImageBody,
    blobs: &'a Blobs,
    on: &'a dyn Fn(ViewAction) -> M,
) -> Element<'a, M> {
    let p = *palette();
    let radius = skin().radius;
    let picture: Element<'a, M> = match blobs.get(&i.blob.id) {
        Some(handle) => image(handle.clone())
            .content_fit(ContentFit::Contain)
            .width(Length::Fill)
            .into(),
        None => container(text("Loading picture").size(TYPE_META).color(p.ink_3))
            .width(Length::Fill)
            .height(Length::Fixed(180.0))
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(move |_| container::Style {
                background: Some(Background::Color(p.sheet_2)),
                border: Border {
                    color: p.rule_faint,
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into(),
    };
    let caption = text(if i.caption.is_empty() {
        i.alt.as_str()
    } else {
        i.caption.as_str()
    })
    .size(TYPE_META)
    .color(p.ink_3);
    mouse_area(column![picture, caption].spacing(6))
        .interaction(mouse::Interaction::Pointer)
        .on_press(on(ViewAction::OpenImage(i.blob.id.clone())))
        .into()
}
