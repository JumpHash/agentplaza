//! The generated files match what the crate generates now, and every built-in passes contrast as written.

use std::path::PathBuf;

use apz_theme::{Mode, builtin, generate as gen_, resolve, skin};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn check(path: &str, want: &str) {
    let path = root().join(path);
    if std::env::var_os("APZ_REGEN").is_some() {
        std::fs::write(&path, want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{} is stale: run APZ_REGEN=1 cargo test -p apz-theme --test gen",
        path.display()
    );
}

#[test]
fn generated_json_is_current() {
    check("apps/shared/themes.gen.json", &gen_::gen_json());
}

#[test]
fn generated_css_is_current() {
    check("apps/web/src/styles/themes.gen.css", &gen_::gen_css());
}

#[test]
fn every_builtin_parses() {
    assert_eq!(builtin::builtins().len(), builtin::SOURCES.len());
    for (id, _) in builtin::SOURCES {
        assert_eq!(builtin::builtin(id).unwrap().id, *id);
    }
}

/// Built-ins reach 4.5:1 for text and 3:1 for marks as written: the contrast pass moves nothing. Every palette is
/// checked with every skin, since any combination can be chosen.
#[test]
fn builtins_pass_contrast_with_every_skin() {
    let mut fails = vec![];
    for file in builtin::builtins() {
        for s in skin::skins() {
            for mode in [Mode::Light, Mode::Dark] {
                let r = resolve(&file, &s, mode);
                for a in r.report.adjusted.iter().chain(&r.report.failing) {
                    fails.push(format!(
                        "{} on {} {}: {} {} is {:.2} on {} (needs {}), fix {}",
                        file.id,
                        s.id,
                        mode.as_str(),
                        a.token,
                        a.from.to_hex(),
                        a.ratio,
                        a.against,
                        a.min,
                        a.to.to_hex()
                    ));
                }
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// `APZ_FIX=1` writes the contrast pass's fixes back into the built-in sources, so they pass as written.
#[test]
fn fix_sources_on_request() {
    if std::env::var_os("APZ_FIX").is_none() {
        return;
    }
    for (id, text) in builtin::SOURCES {
        let mut file = apz_theme::ThemeFile::parse(text).unwrap();
        let s = skin::skin(&file.skin).unwrap();
        for mode in [Mode::Light, Mode::Dark] {
            let r = resolve(&file, &s, mode);
            let pal = match mode {
                Mode::Light => file.palette.light.as_mut().unwrap(),
                Mode::Dark => file.palette.dark.as_mut().unwrap(),
            };
            for a in &r.report.adjusted {
                pal.insert(a.token.clone(), a.to.to_hex());
            }
        }
        let mut out = serde_json::to_string_pretty(&file).unwrap();
        out.push('\n');
        std::fs::write(root().join(format!("apps/shared/themes/{id}.apztheme.json")), out).unwrap();
    }
}
