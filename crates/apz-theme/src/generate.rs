//! The generated files both consoles read. `apps/shared/themes.gen.json` holds every built-in theme resolved in
//! both modes, the skins, the background presets and the token list; `apps/web/src/styles/themes.gen.css` is the
//! same as CSS custom properties keyed by `data-skin`, `data-palette` and `data-mode` on the root element.
//! `tests/gen.rs` regenerates both and fails when the checked-in bytes differ (`APZ_REGEN=1` rewrites them).

use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::appearance::Density;
use crate::skin::{ActionFrom, Shadow, Skin, skins};
use crate::{Mode, Resolved, Tokens, background, builtin, derive, resolve, skin, tokens};

/// The faces both skins use, with their fallbacks.
pub const FONT_STACK: &str =
    "\"Geist\", ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", sans-serif";
pub const MONO_STACK: &str =
    "\"Geist Mono\", ui-monospace, \"SF Mono\", Menlo, Consolas, monospace";

/// Every built-in theme resolved with its own skin, in both modes.
pub fn resolved_builtins() -> Vec<(Resolved, Resolved)> {
    builtin::builtins()
        .iter()
        .map(|f| {
            let s = skin::skin(&f.skin).expect("built-in skin");
            (resolve(f, &s, Mode::Light), resolve(f, &s, Mode::Dark))
        })
        .collect()
}

/// `themes.gen.json`.
pub fn gen_json() -> String {
    let themes: Vec<Value> = resolved_builtins()
        .into_iter()
        .map(|(l, d)| {
            json!({
                "id": l.id,
                "name": l.name,
                "about": builtin::builtin(&l.id).map(|f| f.about).unwrap_or_default(),
                "skin": l.skin.id,
                "background": l.background.id,
                "light": l.tokens,
                "dark": d.tokens,
            })
        })
        .collect();
    let doc = json!({
        "schema": crate::SCHEMA,
        "default": builtin::DEFAULT_THEME,
        "tokens": tokens::TOKENS,
        "skin_tokens": derive::SKIN_TOKENS,
        "skins": skins(),
        "backgrounds": background::presets(),
        "themes": themes,
    });
    let mut text = serde_json::to_string_pretty(&doc).expect("json");
    text.push('\n');
    text
}

fn px(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}px")
}

fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    let s = s.strip_prefix("0.").map(|r| format!(".{r}")).unwrap_or(s);
    if s.is_empty() { "0".into() } else { s }
}

/// A shadow list as CSS (`none` when empty).
pub fn shadow_css(layers: &[Shadow]) -> String {
    if layers.is_empty() {
        return "none".into();
    }
    layers
        .iter()
        .map(|s| format!("0 {} {} rgba(0,0,0,{})", px(s.y), px(s.blur), num(s.alpha)))
        .collect::<Vec<_>>()
        .join(",")
}

/// The CSS for one skin: shape, type and what a primary action takes.
pub fn skin_css(s: &Skin) -> String {
    let mut out = String::new();
    let label_font = if s.label.mono {
        "var(--font-mono)"
    } else {
        "var(--font)"
    };
    let (action, on_action) = match s.action {
        ActionFrom::Ink => ("var(--ink)", "var(--sheet)"),
        ActionFrom::Signal => ("var(--signal-fill)", "var(--on-signal)"),
    };
    let _ = writeln!(
        out,
        ":root[data-skin=\"{id}\"]{{--text-size:{text};--radius-sm:{rs};--radius:{r};--radius-lg:{rl};--row:{row};\
--label-font:{label_font};--label-case:{case};--label-size:{ls};--label-weight:{lw};--label-tracking:{lt}em;\
--action:{action};--on-action:{on_action}}}",
        id = s.id,
        text = px(s.text),
        rs = px((s.radius - 2.0).max(0.0)),
        r = px(s.radius),
        rl = px(s.radius_lg),
        row = px(s.row),
        case = if s.label.upper { "uppercase" } else { "none" },
        ls = px(s.label.size),
        lw = s.label.weight,
        lt = num(s.label.tracking),
    );
    for (mode, sh) in [(Mode::Light, &s.light), (Mode::Dark, &s.dark)] {
        let _ = writeln!(
            out,
            ":root[data-skin=\"{}\"][data-mode=\"{}\"]{{--shadow-card:{};--shadow-pop:{}}}",
            s.id,
            mode.as_str(),
            shadow_css(&sh.card),
            shadow_css(&sh.pop)
        );
    }
    out
}

/// The CSS custom properties for a resolved palette, without the skin's own tokens.
pub fn palette_decls(tokens: &Tokens) -> String {
    let mut out = String::new();
    for t in tokens::TOKENS {
        if derive::SKIN_TOKENS.contains(&t.name) {
            continue;
        }
        let _ = write!(out, "--{}:{};", t.name, tokens.get(t.name).to_hex());
    }
    out.pop();
    out
}

/// The CSS for a resolved theme under a selector, used for saved and imported themes at run time.
pub fn theme_css(selector: &str, r: &Resolved) -> String {
    format!(
        "{selector}{{color-scheme:{};{}}}\n",
        r.mode.as_str(),
        palette_decls(&r.tokens)
    )
}

/// `themes.gen.css`.
pub fn gen_css() -> String {
    let mut out = String::from(
        "/* Generated by crates/apz-theme (src/generate.rs) from apps/shared/themes/*.apztheme.json. Do not edit:\n   \
APZ_REGEN=1 cargo test -p apz-theme --test gen rewrites it. */\n",
    );
    let _ = writeln!(
        out,
        ":root{{--font:{FONT_STACK};--font-mono:{MONO_STACK};--density:1}}"
    );
    for d in [Density::Compact, Density::Comfortable] {
        let name = serde_json::to_value(d)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            ":root[data-density=\"{name}\"]{{--density:{}}}",
            num(d.factor())
        );
    }
    for s in skins() {
        out.push_str(&skin_css(&s));
    }
    for (l, d) in resolved_builtins() {
        for r in [&l, &d] {
            let sel = format!(
                ":root[data-palette=\"{}\"][data-mode=\"{}\"]",
                r.id,
                r.mode.as_str()
            );
            out.push_str(&theme_css(&sel, r));
        }
    }
    out
}
