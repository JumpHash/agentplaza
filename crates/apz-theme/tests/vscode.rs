//! VS Code theme import: the sample themes, comments, include chains, alpha and the resolved result.

use std::collections::HashMap;

use apz_theme::vscode::{self, Import};
use apz_theme::{Mode, Resolved, resolve, skin, tokens};
use serde_json::json;

const DARK_PLUS: &str = r##"{
  // Dark+ sample
  "name": "Dark+ (default dark)",
  "type": "dark",
  "colors": {
    "editor.background": "#1e1e1e",
    "editor.foreground": "#d4d4d4",
    "editorLineNumber.foreground": "#858585",
    "editorWidget.background": "#252526",
    "editorGroup.border": "#444444",
    "sideBar.background": "#252526",
    "sideBar.foreground": "#cccccc",
    "activityBar.background": "#333333",
    "list.hoverBackground": "#2a2d2e",
    "list.activeSelectionBackground": "#04395e",
    "focusBorder": "#007fd4",
    "button.background": "#0e639c",
    "button.foreground": "#ffffff",
    "descriptionForeground": "#9d9d9d",
    "editorError.foreground": "#f14c4c",
    "editorWarning.foreground": "#cca700",
    "editorInfo.foreground": "#3794ff",
    "terminal.ansiGreen": "#23d18b",
    "terminal.ansiRed": "#f14c4c",
    "terminal.ansiBlue": "#3b8eea",
    "terminal.ansiYellow": "#e5e510",
  },
  "tokenColors": [
    { "scope": ["keyword", "storage.type"], "settings": { "foreground": "#569cd6" } },
    { "scope": "string", "settings": { "foreground": "#ce9178" } },
    { "scope": "comment, punctuation.definition.comment", "settings": { "foreground": "#6a9955" } },
    { "scope": "entity.name.function", "settings": { "foreground": "#dcdcaa" } },
    { "scope": "constant.numeric", "settings": { "foreground": "#b5cea8" } },
    { "scope": "entity.name.type", "settings": { "foreground": "#4ec9b0" } },
    { "scope": "variable", "settings": { "foreground": "#9cdcfe" } },
  ],
}"##;

const LIGHT_PLUS: &str = r##"{
  "name": "Light+ (default light)",
  "type": "light",
  "colors": {
    "editor.background": "#ffffff",
    "editor.foreground": "#000000",
    "editorLineNumber.foreground": "#237893",
    "editorWidget.background": "#f3f3f3",
    "sideBar.background": "#f3f3f3",
    "sideBar.foreground": "#616161",
    "activityBar.background": "#2c2c2c",
    "list.hoverBackground": "#e8e8e8",
    "list.activeSelectionBackground": "#e4e6f1",
    "panel.border": "#80808059",
    "focusBorder": "#0090f1",
    "button.background": "#005fb8",
    "button.foreground": "#ffffff",
    "textLink.foreground": "#006ab1",
    "editorError.foreground": "#e51400",
    "editorWarning.foreground": "#bf8803",
    "terminal.ansiGreen": "#14ce14",
    "terminal.ansiRed": "#cd3131",
    "terminal.ansiBlue": "#0451a5",
  },
  "tokenColors": [
    { "scope": "keyword", "settings": { "foreground": "#0000ff" } },
    { "scope": "string", "settings": { "foreground": "#a31515" } },
    { "scope": "comment", "settings": { "foreground": "#008000" } },
    { "scope": "entity.name.function", "settings": { "foreground": "#795e26" } },
    { "scope": "constant.numeric", "settings": { "foreground": "#098658" } },
  ],
}"##;

const HIGH_CONTRAST: &str = r##"{
  "name": "High Contrast",
  "type": "hc",
  "colors": {
    "editor.background": "#000000",
    "editor.foreground": "#ffffff",
    "editorLineNumber.foreground": "#bbbbbb",
    "sideBar.background": "#000000",
    "sideBar.foreground": "#ffffff",
    "focusBorder": "#f38518",
    "button.background": "#000000",
    "button.foreground": "#ffffff",
    "panel.border": "#6fc3df",
    "editorError.foreground": "#f48771",
    "editorWarning.foreground": "#ff8800",
    "editorInfo.foreground": "#75bfff",
  },
  "tokenColors": [
    { "scope": "keyword", "settings": { "foreground": "#569cd6" } },
    { "scope": "comment", "settings": { "foreground": "#7ca668" } },
  ],
}"##;

const SOLARIZED_DARK: &str = r##"{
  "name": "Solarized Dark",
  "type": "dark",
  "colors": {
    "editor.background": "#002b36",
    "editor.foreground": "#839496",
    "editorLineNumber.foreground": "#586e75",
    "sideBar.background": "#073642",
    "sideBar.foreground": "#93a1a1",
    "activityBar.background": "#002b36",
    "list.hoverBackground": "#073642",
    "focusBorder": "#268bd2",
    "button.background": "#268bd2",
    "button.foreground": "#fdf6e3",
    "descriptionForeground": "#93a1a1",
    "editorError.foreground": "#dc322f",
    "editorWarning.foreground": "#b58900",
    "terminal.ansiGreen": "#859900",
    "terminal.ansiBlue": "#268bd2",
    "terminal.ansiCyan": "#2aa198",
  },
  "tokenColors": [
    { "scope": "keyword", "settings": { "foreground": "#859900" } },
    { "scope": "string", "settings": { "foreground": "#2aa198" } },
    { "scope": "comment", "settings": { "foreground": "#586e75" } },
    { "scope": "entity.name.function", "settings": { "foreground": "#268bd2" } },
  ],
}"##;

const ONE_DARK_PRO: &str = r##"{
  "name": "One Dark Pro",
  "type": "dark",
  "colors": {
    "editor.background": "#282c34",
    "editor.foreground": "#abb2bf",
    "editorLineNumber.foreground": "#4b5263",
    "sideBar.background": "#21252b",
    "sideBar.foreground": "#abb2bf",
    "activityBar.background": "#282c34",
    "list.activeSelectionBackground": "#2c313a",
    "focusBorder": "#528bff",
    "button.background": "#4d78cc",
    "textLink.foreground": "#61afef",
    "editorError.foreground": "#e06c75",
    "editorWarning.foreground": "#e5c07b",
    "editorInfo.foreground": "#61afef",
    "diffEditor.insertedTextBackground": "#98c37926",
    "diffEditor.removedTextBackground": "#e06c7526",
    "terminal.ansiGreen": "#98c379",
    "terminal.ansiRed": "#e06c75",
    "terminal.ansiMagenta": "#c678dd",
  },
  "tokenColors": [
    { "scope": ["keyword", "storage"], "settings": { "foreground": "#c678dd" } },
    { "scope": "string", "settings": { "foreground": "#98c379" } },
    { "scope": "constant.numeric", "settings": { "foreground": "#d19a66" } },
    { "scope": "entity.name.type", "settings": { "foreground": "#e5c07b" } },
    { "scope": "entity.name.function", "settings": { "foreground": "#61afef" } },
    { "scope": "variable", "settings": { "foreground": "#e06c75" } },
  ],
}"##;

/// No file is ever found: for themes without an `include`.
fn no_files(_: &str) -> Option<String> {
    None
}

/// The colour a token has in the imported palette for the theme's mode, or an empty string.
fn colour(imp: &Import, token: &str) -> String {
    imp.file
        .palette
        .get(imp.mode)
        .and_then(|p| p.get(token))
        .cloned()
        .unwrap_or_default()
}

fn import(text: &str) -> Import {
    vscode::from_json(text, "loom", &no_files).unwrap()
}

/// Resolves the import for Loom in its own mode.
fn resolved(imp: &Import) -> Resolved {
    let skin = skin::skin("loom").unwrap();
    resolve(&imp.file, &skin, imp.mode)
}

/// Every token is present and nothing still fails its contrast check.
fn assert_resolves(imp: &Import) -> Resolved {
    let r = resolved(imp);
    assert_eq!(
        r.tokens.0.len(),
        tokens::TOKENS.len(),
        "every token is present"
    );
    assert!(
        r.report.failing.is_empty(),
        "failing checks: {:?}",
        r.report.failing
    );
    r
}

/// As [`assert_resolves`], and the file also took more than ten colours from the theme.
fn assert_resolves_cleanly(imp: &Import) {
    let r = assert_resolves(imp);
    assert!(r.report.taken > 10, "taken: {}", r.report.taken);
}

#[test]
fn dark_plus_maps_its_keys() {
    let imp = import(DARK_PLUS);
    assert_eq!(imp.mode, Mode::Dark);
    assert_eq!(imp.name, "Dark+ (default dark)");
    assert_eq!(imp.file.id, "vscode-dark-default-dark");
    assert_eq!(
        imp.file.source.as_deref(),
        Some("vscode: Dark+ (default dark)")
    );
    assert_eq!(imp.file.skin, "loom");
    assert!(
        imp.file.palette.light.is_none(),
        "only the theme's mode is filled"
    );
    assert_eq!(colour(&imp, "paper"), "#1e1e1e");
    assert_eq!(colour(&imp, "ink"), "#d4d4d4");
    assert_eq!(colour(&imp, "signal"), "#007fd4");
    assert_eq!(colour(&imp, "syn-keyword"), "#569cd6");
    assert_eq!(colour(&imp, "syn-string"), "#ce9178");
    assert_eq!(colour(&imp, "syn-comment"), "#6a9955");
    assert_eq!(colour(&imp, "syn-function"), "#dcdcaa");
    assert_eq!(colour(&imp, "syn-variable"), "#9cdcfe");
    assert_eq!(colour(&imp, "ansi-1"), "#f14c4c");
    assert_eq!(colour(&imp, "ansi-4"), "#3b8eea");
    assert_eq!(colour(&imp, "chart-1"), "#007fd4");
    assert_eq!(colour(&imp, "chart-2"), "#3b8eea", "chart-2 is ansi-4");
    assert_eq!(
        colour(&imp, "term-bg"),
        "#1e1e1e",
        "terminal background falls back to paper"
    );
    assert_resolves_cleanly(&imp);
}

#[test]
fn light_plus_is_light() {
    let imp = import(LIGHT_PLUS);
    assert_eq!(imp.mode, Mode::Light);
    assert!(imp.file.palette.dark.is_none());
    assert_eq!(colour(&imp, "paper"), "#ffffff");
    assert_eq!(colour(&imp, "syn-keyword"), "#0000ff");
    assert_eq!(resolved(&imp).mode, Mode::Light);
    assert_resolves_cleanly(&imp);
}

#[test]
fn high_contrast_type_is_dark() {
    let imp = import(HIGH_CONTRAST);
    assert_eq!(imp.mode, Mode::Dark, "the hc type is a dark theme");
    assert_eq!(colour(&imp, "paper"), "#000000");
    assert_eq!(colour(&imp, "ink"), "#ffffff");
    assert_resolves_cleanly(&imp);
}

#[test]
fn solarized_dark_maps_its_keys() {
    let imp = import(SOLARIZED_DARK);
    assert_eq!(imp.file.id, "vscode-solarized-dark");
    assert_eq!(colour(&imp, "paper"), "#002b36");
    assert_eq!(colour(&imp, "paper-side"), "#073642");
    assert_eq!(colour(&imp, "ink"), "#839496");
    assert_eq!(colour(&imp, "syn-comment"), "#586e75");
    assert_eq!(colour(&imp, "syn-keyword"), "#859900");
    assert_eq!(colour(&imp, "ansi-6"), "#2aa198");
    assert_resolves_cleanly(&imp);
}

#[test]
fn one_dark_pro_maps_its_keys() {
    let imp = import(ONE_DARK_PRO);
    assert_eq!(imp.file.id, "vscode-one-dark-pro");
    assert_eq!(colour(&imp, "paper"), "#282c34");
    assert_eq!(colour(&imp, "signal-ink"), "#61afef");
    assert_eq!(colour(&imp, "syn-keyword"), "#c678dd");
    assert_eq!(colour(&imp, "syn-type"), "#e5c07b");
    assert_eq!(colour(&imp, "syn-variable"), "#e06c75");
    // The translucent diff ground is composited, so the stored colour is opaque.
    assert_eq!(colour(&imp, "add-bg").len(), 7);
    assert_resolves_cleanly(&imp);
}

#[test]
fn comments_and_trailing_commas_are_allowed() {
    let text = r##"
    /* a theme with comments
       and trailing commas */
    {
      "name": "Half // Commented", // a line comment
      "type": "dark",
      "colors": {
        "editor.background": "#101010", /* inline */
        "editor.foreground": "#eeeeee",
      },
      "tokenColors": [
        { "scope": "comment", "settings": { "foreground": "#777777", }, },
      ],
    }
    "##;
    let imp = import(text);
    assert_eq!(
        imp.name, "Half // Commented",
        "a // inside a string is text"
    );
    assert_eq!(colour(&imp, "paper"), "#101010");
    assert_eq!(colour(&imp, "syn-comment"), "#777777");
    assert_eq!(imp.file.id, "vscode-half-commented");
}

#[test]
fn parse_jsonc_strips_comments_and_commas() {
    let v = vscode::parse_jsonc("// head\n[1, /* two */ 2,\n 3, ] ").unwrap();
    assert_eq!(v, json!([1, 2, 3]));
    let s = vscode::parse_jsonc(r#"{"a": "/* not a comment */", }"#).unwrap();
    assert_eq!(s, json!({"a": "/* not a comment */"}));
    assert!(vscode::parse_jsonc("{").is_err());
}

#[test]
fn includes_are_followed_and_the_child_overrides() {
    let base = r##"{
      "colors": {
        "editor.background": "#000000",
        "editor.foreground": "#aaaaaa",
        "focusBorder": "#00ff00",
        "sideBar.background": "#111111"
      },
      "tokenColors": [ { "scope": "keyword", "settings": { "foreground": "#ff0000" } } ],
      "semanticTokenColors": { "function": { "foreground": "#123456" } }
    }"##;
    let child = r##"{
      "include": "./base.json",
      "name": "Child",
      "type": "dark",
      "colors": { "editor.foreground": "#ffffff" },
      "tokenColors": [ { "scope": "string", "settings": { "foreground": "#00ff00" } } ]
    }"##;
    let files: HashMap<&str, &str> = HashMap::from([("./base.json", base)]);
    let lookup = |path: &str| files.get(path).map(|s| s.to_string());
    let imp = vscode::from_json(child, "loom", &lookup).unwrap();
    assert_eq!(imp.name, "Child");
    assert_eq!(colour(&imp, "paper"), "#000000", "from the parent");
    assert_eq!(colour(&imp, "paper-side"), "#111111", "from the parent");
    assert_eq!(
        colour(&imp, "ink"),
        "#ffffff",
        "the child overrides the parent"
    );
    assert_eq!(
        colour(&imp, "syn-keyword"),
        "#ff0000",
        "a parent's token rule still applies"
    );
    assert_eq!(colour(&imp, "syn-string"), "#00ff00");
    assert_eq!(
        colour(&imp, "syn-function"),
        "#123456",
        "a semantic colour overrides"
    );
    assert_resolves(&imp);
}

#[test]
fn include_cycles_and_missing_files_are_refused() {
    let a = r#"{ "include": "./b.json", "colors": {} }"#;
    let b = r#"{ "include": "./a.json", "colors": {} }"#;
    let files: HashMap<&str, &str> = HashMap::from([("./a.json", a), ("./b.json", b)]);
    let lookup = |path: &str| files.get(path).map(|s| s.to_string());
    let err = vscode::from_json(a, "loom", &lookup).unwrap_err();
    assert!(err.contains("cycle"), "{err}");
    let missing = r#"{ "include": "./nope.json", "colors": {} }"#;
    let err = vscode::from_json(missing, "loom", &lookup).unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[test]
fn alpha_is_composited_over_the_paper() {
    let text = r##"{
      "type": "dark",
      "colors": {
        "editor.background": "#000000",
        "editor.foreground": "#ffffff80",
        "sideBar.background": "#ffffff80"
      }
    }"##;
    let imp = import(text);
    assert_eq!(colour(&imp, "ink"), "#808080");
    assert_eq!(colour(&imp, "paper-side"), "#808080");
    assert_eq!(colour(&imp, "paper"), "#000000");
}

#[test]
fn mode_of_reads_type_names() {
    for name in ["dark", "hc", "hcDark", "vs-dark", "hc-black"] {
        assert_eq!(vscode::mode_of(name), Mode::Dark, "{name}");
    }
    for name in ["light", "hcLight", "vs", "hc-light"] {
        assert_eq!(vscode::mode_of(name), Mode::Light, "{name}");
    }
}

#[test]
fn ui_theme_stands_in_for_type() {
    let imp = import(
        r##"{ "name": "Old", "uiTheme": "vs", "colors": { "editor.background": "#fafafa" } }"##,
    );
    assert_eq!(imp.mode, Mode::Light);
    assert_eq!(colour(&imp, "paper"), "#fafafa");
}

#[test]
fn bad_input_is_refused() {
    assert!(vscode::from_json("[1]", "loom", &no_files).is_err());
    assert!(vscode::from_json("{", "loom", &no_files).is_err());
    let err = vscode::from_json(LIGHT_PLUS, "nope", &no_files).unwrap_err();
    assert!(err.contains("unknown skin"), "{err}");
}

#[test]
fn the_most_specific_scope_wins() {
    let text = r##"{
      "type": "dark",
      "colors": { "editor.background": "#101010" },
      "tokenColors": [
        { "scope": "comment", "settings": { "foreground": "#111111" } },
        { "scope": "comment.line", "settings": { "foreground": "#222222" } }
      ]
    }"##;
    let imp = import(text);
    assert_eq!(
        colour(&imp, "syn-comment"),
        "#222222",
        "comment.line is more specific than comment"
    );
}

#[test]
fn a_child_comment_rule_beats_its_parents() {
    let base = r##"{
      "colors": { "editor.background": "#101010" },
      "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#111111" } } ]
    }"##;
    let child = r##"{
      "include": "./base.json",
      "type": "dark",
      "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#333333" } } ]
    }"##;
    let files: HashMap<&str, &str> = HashMap::from([("./base.json", base)]);
    let lookup = |path: &str| files.get(path).map(|s| s.to_string());
    let imp = vscode::from_json(child, "loom", &lookup).unwrap();
    assert_eq!(
        colour(&imp, "syn-comment"),
        "#333333",
        "the child's rule comes after its parent's"
    );
}

#[test]
fn a_hint_gives_the_mode_and_default_paper_when_there_is_no_type() {
    let text = r##"{ "name": "Bare", "colors": { "editor.foreground": "#101010" } }"##;
    let light = vscode::from_json_hinted(text, "loom", &no_files, Some(Mode::Light)).unwrap();
    assert_eq!(light.mode, Mode::Light);
    assert_eq!(colour(&light, "paper"), "#ffffff");
    let dark = vscode::from_json(text, "loom", &no_files).unwrap();
    assert_eq!(dark.mode, Mode::Dark);
    assert_eq!(colour(&dark, "paper"), "#1e1e1e");
}

#[test]
fn oversized_input_is_refused() {
    let padding = "a".repeat(4 * 1024 * 1024);
    let big = format!(r#"{{ "name": "Big", "note": "{padding}" }}"#);
    let err = vscode::from_json(&big, "loom", &no_files).unwrap_err();
    assert!(err.contains("larger"), "{err}");
}

#[test]
fn a_translucent_paper_is_composited_over_white_or_black_by_its_own_luminance() {
    let skin = skin::skin("loom").unwrap();
    // 50% grey at 50% alpha reads as light, so it sits on white: 0.5 * 0.5 + 1.0 * 0.5 is #bfbfbf.
    let given = std::collections::BTreeMap::from([(
        "paper".to_string(),
        apz_theme::Rgba::parse("#80808080").unwrap(),
    )]);
    let tokens = apz_theme::derive::derive(&given, &skin, Mode::Dark);
    assert_eq!(tokens.get("paper").to_hex(), "#bfbfbf");
}
