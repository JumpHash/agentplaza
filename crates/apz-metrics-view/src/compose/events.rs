//! The events tab.

use super::Ctx;
use crate::model::Panel;
use crate::source::Source;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let _ = (ctx, src);
    Vec::new()
}
