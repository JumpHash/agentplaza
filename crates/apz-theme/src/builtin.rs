//! The built-in themes: one per palette, each with its own skin and background. The sources are
//! `apps/shared/themes/*.apztheme.json`, read here at compile time and by the web build.

use std::collections::BTreeMap;

use crate::color::Rgba;
use crate::{Mode, ThemeFile, Tokens};

/// The theme a new hub starts in.
pub const DEFAULT_THEME: &str = "loom-clay";

/// The sources, in the Appearance screen's order.
pub const SOURCES: &[(&str, &str)] = &[
    (
        "loom-clay",
        include_str!("../../../apps/shared/themes/loom-clay.apztheme.json"),
    ),
    (
        "loom-moss",
        include_str!("../../../apps/shared/themes/loom-moss.apztheme.json"),
    ),
    (
        "loom-indigo",
        include_str!("../../../apps/shared/themes/loom-indigo.apztheme.json"),
    ),
    (
        "graphite-cobalt",
        include_str!("../../../apps/shared/themes/graphite-cobalt.apztheme.json"),
    ),
    (
        "graphite-violet",
        include_str!("../../../apps/shared/themes/graphite-violet.apztheme.json"),
    ),
    (
        "graphite-teal",
        include_str!("../../../apps/shared/themes/graphite-teal.apztheme.json"),
    ),
];

/// Every built-in theme. A source that does not parse is a build error caught by the tests.
pub fn builtins() -> Vec<ThemeFile> {
    SOURCES
        .iter()
        .filter_map(|(_, text)| ThemeFile::parse(text).ok())
        .collect()
}

/// The built-in theme with this id.
pub fn builtin(id: &str) -> Option<ThemeFile> {
    SOURCES
        .iter()
        .find(|(name, _)| *name == id)
        .and_then(|(_, text)| ThemeFile::parse(text).ok())
}

/// Whether `id` names a built-in theme.
pub fn is_builtin(id: &str) -> bool {
    SOURCES.iter().any(|(name, _)| *name == id)
}

/// A built-in theme's colours as written (no derivation), for seeding a palette that lacks them.
pub fn seed(id: &str, mode: Mode) -> Tokens {
    let mut out = Tokens(BTreeMap::new());
    let file = builtin(id).or_else(|| builtin(DEFAULT_THEME));
    if let Some(file) = file {
        let pal = file.palette.get(mode).or(file.palette.light.as_ref());
        for (k, v) in pal.into_iter().flatten() {
            if let Some(c) = Rgba::parse(v) {
                out.set(k, c);
            }
        }
    }
    out
}
