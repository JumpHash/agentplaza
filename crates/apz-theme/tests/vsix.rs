//! Tests for `apz_theme::vsix`. Packages are built in memory with the zip writer, so nothing touches disk.

use std::io::{Cursor, Write};

use apz_theme::{Mode, PaletteText, vsix};
use zip::write::SimpleFileOptions;

fn package(files: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in files {
        zip.start_file(*name, SimpleFileOptions::default())
            .expect("start the entry");
        zip.write_all(body.as_bytes()).expect("write the entry");
    }
    zip.finish().expect("finish the archive").into_inner()
}

/// Whether some colour in the palette contains the hex digits (case-insensitive).
fn has(palette: Option<&PaletteText>, digits: &str) -> bool {
    palette.is_some_and(|m| m.values().any(|v| v.to_ascii_lowercase().contains(digits)))
}

const MANIFEST: &str = r#"{
  "name": "night-pack",
  "displayName": "Night Pack",
  "contributes": {
    "themes": [
      { "label": "Night Owl", "uiTheme": "vs-dark", "path": "./themes/night.json" },
      { "label": "Day", "uiTheme": "vs", "path": "./themes/day.json" }
    ]
  }
}"#;

const NIGHT: &str = r##"// The child: it overrides the background and includes the base.
{
  "include": "./base.json",
  "colors": { "editor.background": "#222222" }
}"##;

const BASE: &str = r##"{
  "colors": { "editor.background": "#111111", "editor.foreground": "#abcdef" },
  "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#777777" } } ]
}"##;

const DAY: &str = r##"{
  "colors": { "editor.background": "#fafafa", "editor.foreground": "#101010" }
}"##;

fn pack() -> Vec<u8> {
    package(&[
        ("extension/package.json", MANIFEST),
        ("extension/themes/night.json", NIGHT),
        ("extension/themes/base.json", BASE),
        ("extension/themes/day.json", DAY),
    ])
}

#[test]
fn lists_the_contributed_themes() {
    let list = vsix::themes(&pack()).expect("the themes");
    let labels: Vec<&str> = list.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, ["Night Owl", "Day"]);
    assert_eq!(list[0].ui_theme, "vs-dark");
    assert_eq!(list[0].path, "./themes/night.json");
}

#[test]
fn the_package_ui_theme_decides_the_mode_when_the_json_has_no_type() {
    let imp = vsix::import(&pack(), Some("Day"), "graphite").expect("the Day theme");
    assert_eq!(imp.mode, Mode::Light);
    assert!(imp.file.palette.light.is_some());
    assert!(imp.file.palette.dark.is_none());
    assert!(has(imp.file.palette.get(Mode::Light), "101010"));
}

#[test]
fn includes_resolve_inside_the_archive_and_the_child_wins() {
    let imp = vsix::import(&pack(), Some("Night Owl"), "graphite").expect("the Night Owl theme");
    assert_eq!(imp.mode, Mode::Dark);
    let dark = imp.file.palette.get(Mode::Dark);
    assert!(has(dark, "abcdef"), "the included foreground is missing");
    assert!(has(dark, "222222"), "the child's background is missing");
    assert!(
        !has(dark, "111111"),
        "the included background won over the child"
    );
}

#[test]
fn a_missing_label_is_an_error() {
    let err = vsix::import(&pack(), Some("Nope"), "graphite")
        .expect_err("an error for the missing label");
    assert!(err.contains("Nope"), "{err}");
}

#[test]
fn a_path_leaving_the_extension_folder_is_refused() {
    let manifest = r#"{ "contributes": { "themes": [
        { "label": "Escape", "uiTheme": "vs", "path": "../outside.json" }
    ] } }"#;
    let bytes = package(&[
        ("extension/package.json", manifest),
        ("outside.json", r#"{ "colors": {} }"#),
    ]);
    let err = vsix::import(&bytes, None, "graphite").expect_err("an error for the escaping path");
    assert!(err.contains("leaves"), "{err}");

    let walk = r#"{ "contributes": { "themes": [
        { "label": "Walk", "uiTheme": "vs", "path": "./themes/walk.json" }
    ] } }"#;
    let bytes = package(&[
        ("extension/package.json", walk),
        (
            "extension/themes/walk.json",
            r#"{ "include": "../../../outside.json" }"#,
        ),
        ("outside.json", r#"{ "colors": {} }"#),
    ]);
    let err =
        vsix::import(&bytes, None, "graphite").expect_err("an error for the escaping include");
    assert!(err.contains("leaves"), "{err}");
}

#[test]
fn a_typeless_light_theme_gets_a_light_paper() {
    let manifest = r#"{ "contributes": { "themes": [
        { "label": "Bare Day", "uiTheme": "vs", "path": "./themes/bare.json" }
    ] } }"#;
    let bytes = package(&[
        ("extension/package.json", manifest),
        (
            "extension/themes/bare.json",
            r##"{ "colors": { "editor.foreground": "#101010" } }"##,
        ),
    ]);
    let imp = vsix::import(&bytes, None, "graphite").expect("the bare theme");
    assert_eq!(imp.mode, Mode::Light);
    assert!(imp.file.palette.dark.is_none());
    let paper = imp
        .file
        .palette
        .get(Mode::Light)
        .and_then(|p| p.get("paper"))
        .map(String::as_str);
    assert_eq!(paper, Some("#ffffff"));
}

#[test]
fn bytes_that_are_not_a_zip_are_an_error() {
    assert!(vsix::themes(b"not a zip").is_err());
    assert!(vsix::import(b"not a zip", None, "graphite").is_err());
}
