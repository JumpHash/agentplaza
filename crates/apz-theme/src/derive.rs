//! Deriving the tokens a palette leaves out. Every rule is a mix or a lightness step in OKLab/OKLCH from tokens
//! already known, so an imported theme with ten colours still fills all of them in its own hues. The contrast pass
//! runs after this and fixes anything a rule made too faint.

use std::collections::BTreeMap;

use crate::color::{Oklch, Rgba};
use crate::skin::{ActionFrom, Skin};
use crate::{Mode, Tokens};

/// Tokens the skin decides, whatever the palette says.
pub const SKIN_TOKENS: &[&str] = &["action", "on-action"];

/// Tokens the page draws over what lies beneath them, so they stay translucent: the ambient weave and ornament
/// lines and the dialog veil. Every other colour with alpha is composited onto the page.
pub const KEEP_ALPHA: &[&str] = &["amb-line", "amb-lit", "scrim"];

/// A state colour from a hue, at a lightness that reads on the mode's ground.
fn state(hue: f32, light: bool) -> Rgba {
    Rgba::from_oklch(
        Oklch {
            l: if light { 0.50 } else { 0.76 },
            c: 0.13,
            h: hue,
        },
        1.0,
    )
}

fn m(a: &'static str, b: &'static str, k: f32) -> impl Fn(&Tokens) -> Rgba {
    move |t: &Tokens| t.get(a).mix(t.get(b), k)
}

fn same(a: &'static str) -> impl Fn(&Tokens) -> Rgba {
    move |t: &Tokens| t.get(a)
}

/// Fills in every token `given` lacks. `given` must be opaque colours; seeds missing from it (paper, ink, signal)
/// come from the skin's own palette for the mode.
pub fn derive(given: &BTreeMap<String, Rgba>, skin: &Skin, mode: Mode) -> Tokens {
    let mut t = Tokens::default();
    for (k, v) in given {
        t.set(k, *v);
    }
    let seeds = crate::builtin::seed(&skin.palette, mode);
    for name in ["paper", "ink", "signal"] {
        if !t.has(name) {
            t.set(name, seeds.get(name));
        }
    }
    // A translucent paper is composited over white (light) or black (dark), by its own luminance, before anything
    // else reads it, so it does not composite against itself.
    let paper = t.get("paper");
    let paper = if paper.a < 1.0 {
        let ground = if paper.is_light() {
            Rgba::WHITE
        } else {
            Rgba::BLACK
        };
        let opaque = paper.over(ground);
        t.set("paper", opaque);
        opaque
    } else {
        paper
    };
    let light = paper.is_light();
    // Text over a translucent value is the value composited onto the page.
    for (name, v) in t.0.iter_mut() {
        if v.a < 1.0 && !KEEP_ALPHA.contains(&name.as_str()) {
            *v = v.over(paper);
        }
    }
    let ink = t.get("ink");
    let put = |t: &mut Tokens, name: &str, f: &dyn Fn(&Tokens) -> Rgba| {
        if !t.has(name) {
            let c = f(t);
            t.set(name, c);
        }
    };

    put(&mut t, "paper-side", &m("paper", "ink", 0.02));
    put(&mut t, "sheet", &move |t: &Tokens| {
        if light {
            t.get("paper").mix(Rgba::WHITE, 0.6)
        } else {
            t.get("paper").mix(ink, 0.035)
        }
    });
    put(&mut t, "panel", &m("paper", "sheet", 0.5));
    put(&mut t, "raised", &same("sheet"));
    put(&mut t, "sheet-2", &m("paper", "ink", 0.035));
    put(&mut t, "hover", &m("paper", "ink", 0.055));
    put(&mut t, "press", &m("paper", "ink", 0.09));
    put(&mut t, "pattern", &m("paper", "ink", 0.08));
    put(&mut t, "rule", &m("paper", "ink", 0.10));
    put(&mut t, "rule-strong", &m("paper", "ink", 0.18));
    put(&mut t, "rule-faint", &m("paper", "ink", 0.06));
    put(&mut t, "ink-2", &m("ink", "paper", 0.33));
    put(&mut t, "ink-3", &m("ink", "paper", 0.46));
    put(&mut t, "amb-1", &same("signal"));
    put(&mut t, "amb-line", &|t: &Tokens| {
        t.get("ink").with_alpha(0.05)
    });
    put(&mut t, "amb-lit", &|t: &Tokens| {
        t.get("ink").with_alpha(0.11)
    });

    put(&mut t, "signal-fill", &same("signal"));
    put(&mut t, "signal-ink", &same("signal"));
    put(&mut t, "on-signal", &|t: &Tokens| {
        let fill = t.get("signal-fill");
        let dark = fill.with_lightness(0.18);
        if Rgba::WHITE.contrast(fill) >= dark.contrast(fill) {
            Rgba::WHITE
        } else {
            dark
        }
    });
    put(
        &mut t,
        "signal-soft",
        &m("paper", "signal", if light { 0.13 } else { 0.18 }),
    );

    for (name, hue) in [
        ("ok", 152.0),
        ("warn", 70.0),
        ("crit", 25.0),
        ("info", 250.0),
        ("review", 295.0),
    ] {
        put(&mut t, name, &move |_| state(hue, light));
        let soft = format!("{name}-soft");
        put(&mut t, &soft, &move |t: &Tokens| {
            t.get("paper")
                .mix(t.get(name), if light { 0.12 } else { 0.16 })
        });
    }

    put(&mut t, "amb-2", &same("warn"));
    put(&mut t, "amb-3", &same("info"));

    put(&mut t, "add-fg", &same("ok"));
    put(&mut t, "del-fg", &same("crit"));
    put(&mut t, "add-bg", &m("paper", "ok", 0.12));
    put(&mut t, "del-bg", &m("paper", "crit", 0.12));
    put(&mut t, "add-bar", &same("ok"));
    put(&mut t, "del-bar", &same("crit"));

    put(&mut t, "syn-keyword", &same("review"));
    put(&mut t, "syn-string", &same("ok"));
    put(&mut t, "syn-type", &same("signal-ink"));
    put(&mut t, "syn-number", &same("info"));
    put(&mut t, "syn-comment", &same("ink-3"));
    put(&mut t, "syn-function", &|t: &Tokens| {
        let i = t.get("info");
        i.with_hue((i.to_oklch().h + 330.0) % 360.0)
    });
    put(&mut t, "syn-variable", &same("ink"));

    put(&mut t, "term-bg", &same("sheet-2"));
    put(&mut t, "term-fg", &same("ink"));
    put(&mut t, "ansi-0", &move |t: &Tokens| {
        if light {
            t.get("ink")
        } else {
            t.get("paper").mix(t.get("ink"), 0.22)
        }
    });
    put(&mut t, "ansi-1", &same("crit"));
    put(&mut t, "ansi-2", &same("ok"));
    put(&mut t, "ansi-3", &same("warn"));
    put(&mut t, "ansi-4", &same("info"));
    put(&mut t, "ansi-5", &same("review"));
    put(&mut t, "ansi-6", &|t: &Tokens| {
        t.get("info").with_hue(200.0)
    });
    put(&mut t, "ansi-7", &move |t: &Tokens| {
        if light {
            t.get("ink-3")
        } else {
            t.get("ink-2")
        }
    });
    put(&mut t, "ansi-8", &same("ink-3"));
    let step = if light { -0.06 } else { 0.06 };
    for (bright, base) in [(9, 1), (10, 2), (11, 3), (12, 4), (13, 5), (14, 6)] {
        let base = format!("ansi-{base}");
        put(&mut t, &format!("ansi-{bright}"), &move |t: &Tokens| {
            let c = t.get(&base);
            c.with_lightness((c.to_oklch().l + step).clamp(0.0, 1.0))
        });
    }
    put(&mut t, "ansi-15", &move |t: &Tokens| {
        if light { t.get("ink-2") } else { t.get("ink") }
    });

    put(&mut t, "chart-1", &same("signal"));
    put(&mut t, "chart-2", &same("info"));
    put(&mut t, "chart-3", &same("ok"));
    put(&mut t, "chart-4", &same("review"));
    put(&mut t, "chart-5", &same("warn"));
    put(&mut t, "chart-6", &same("crit"));
    put(&mut t, "chart-7", &same("ink-3"));
    put(&mut t, "chart-8", &|t: &Tokens| {
        t.get("info").with_hue(200.0)
    });
    put(&mut t, "chart-other", &m("ink-3", "paper", 0.4));
    put(&mut t, "chart-grid", &same("rule-faint"));
    put(&mut t, "chart-axis", &same("ink-3"));
    for (i, name) in ["signal", "ok", "crit", "warn", "info", "review"]
        .iter()
        .enumerate()
    {
        put(&mut t, &format!("hue-{i}"), &|t: &Tokens| t.get(name));
    }
    put(&mut t, "on-hue", &|_| Rgba::WHITE);
    put(&mut t, "toast-bg", &same("ink"));
    put(&mut t, "toast-fg", &same("paper"));
    put(&mut t, "scrim", &move |_| {
        Rgba::BLACK.with_alpha(if light { 0.32 } else { 0.56 })
    });

    apply_skin(&mut t, skin);
    t
}

/// Sets the tokens the skin decides: what a primary action is drawn in.
pub fn apply_skin(t: &mut Tokens, skin: &Skin) {
    match skin.action {
        ActionFrom::Ink => {
            t.set("action", t.get("ink"));
            t.set("on-action", t.get("sheet"));
        }
        ActionFrom::Signal => {
            t.set("action", t.get("signal-fill"));
            t.set("on-action", t.get("on-signal"));
        }
    }
}
