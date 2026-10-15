//! The Metrics overlays, drawn over the screen with a scrim: the panel menu, the sibling switcher, the find palette
//! (and the compare picker), the keyboard help, the "what changed" drawer, "About this metric", and the toast.

use iced::Element;

use super::{Msg, Overlay, State};

/// The overlay layer (none when no overlay is open), to stack over the screen.
pub fn layer<'a>(state: &'a State, overlay: &'a Overlay) -> Element<'a, Msg> {
    let _ = (state, overlay);
    iced::widget::Space::new().into()
}

/// What the find palette lists for a query: (group, label, the message it sends).
pub fn palette_items(state: &State, query: &str, compare: bool) -> Vec<(String, String, Msg)> {
    let _ = (state, query, compare);
    Vec::new()
}

/// The toast, when one is showing.
pub fn toast<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let _ = state;
    None
}
