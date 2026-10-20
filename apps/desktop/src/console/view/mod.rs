//! The shared panel renderers (`apz_view` bodies) that Metrics and artifacts both draw with: the artifact bodies
//! (image, document, code; graph, dataset and HTML still fall back to [`body`]) live here; the Metrics bodies stay in
//! `screens/metrics` and are called from both.

pub mod code;
pub mod dataset;
pub mod document;
pub mod graph;
pub mod image;
pub mod webview;

use apz_view::Body;
use iced::Element;
use iced::widget::text;
use std::collections::HashMap;

/// Bytes of blobs the screen has loaded, by blob id; the screen that owns the artifact fills it.
pub type Blobs = HashMap<String, iced::widget::image::Handle>;

/// Parsed Markdown by panel id; the screen fills it with [`document::parse`] when it loads a document, so the
/// parsed items outlive the elements that borrow them.
pub type Documents = HashMap<String, Vec<iced::widget::markdown::Item>>;

/// What a press in an artifact body asks the screen to do.
#[derive(Debug, Clone)]
pub enum ViewAction {
    /// Open the image of this blob id in the viewer.
    OpenImage(String),
    /// Copy this code text to the clipboard.
    CopyCode(String),
    /// Run the HTML panel with this id in its sandboxed frame.
    RunHtml(String),
    /// Show every line of the code panel with this id.
    LoadMore(String),
    /// Open this link from a document; the screen decides whether to hand it to the browser.
    OpenLink(String),
}

/// An artifact body that the Metrics renderer does not draw itself.
pub fn body<'a, M: 'a>(b: &'a Body) -> Element<'a, M> {
    let label = match b {
        Body::Image(i) => i.alt.as_str(),
        Body::Document { .. } => "Document",
        Body::Code(c) => c.language.as_str(),
        Body::Graph(_) => "Graph",
        Body::Dataset(_) => "Dataset",
        Body::Html(h) => h.entry.as_str(),
        _ => "",
    };
    text(label)
        .size(crate::theme::TYPE_META)
        .color(crate::theme::palette().ink_3)
        .into()
}

/// The body of one artifact panel: an image, a document, or code gets its own renderer; the other bodies fall back to
/// [`body`]. A code panel is drawn collapsed here; the screen draws an expanded one with [`code::code_view`].
pub fn artifact_body<'a, M: Clone + 'a>(
    panel_id: &'a str,
    b: &'a Body,
    blobs: &'a Blobs,
    docs: &'a Documents,
    on: &'a dyn Fn(ViewAction) -> M,
) -> Element<'a, M> {
    match b {
        Body::Image(i) => image::view(i, blobs, on),
        Body::Document { markdown } => document::view(panel_id, markdown, docs, on),
        Body::Code(c) => code::code_view(panel_id, c, false, on),
        _ => body(b),
    }
}
