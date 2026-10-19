//! VS Code theme import: a theme `.json` (comments and trailing commas allowed) or a `.vsix`.
//!
//! [`from_json`] reads one theme file, follows its `include` chain and maps the VS Code colour keys onto the
//! AgentPlaza tokens for the theme's mode. Colours with alpha are composited over the resolved paper, so the
//! stored palette is opaque. A token with no VS Code key is left out; [`crate::resolve`] derives it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::color::Rgba;
use crate::{Mode, PaletteText, Palettes, SCHEMA, ThemeFile};

/// How many `include` links a theme may follow before the import is refused.
const MAX_INCLUDE_DEPTH: usize = 8;

/// The largest theme JSON read, in bytes: the root file and each file it includes.
const MAX_JSON: usize = 4 * 1024 * 1024;

/// The VS Code colour keys for each token, in order: the first one present wins. `paper` is set separately.
const KEY_MAP: &[(&str, &[&str])] = &[
    (
        "paper-side",
        &["sideBar.background", "activityBar.background"],
    ),
    (
        "sheet",
        &[
            "editorWidget.background",
            "dropdown.background",
            "input.background",
        ],
    ),
    ("hover", &["list.hoverBackground"]),
    ("press", &["list.activeSelectionBackground"]),
    (
        "rule",
        &[
            "panel.border",
            "sideBar.border",
            "editorGroup.border",
            "widget.border",
        ],
    ),
    ("ink", &["editor.foreground", "foreground"]),
    ("ink-2", &["sideBar.foreground", "descriptionForeground"]),
    (
        "ink-3",
        &["disabledForeground", "editorLineNumber.foreground"],
    ),
    (
        "signal",
        &["focusBorder", "button.background", "textLink.foreground"],
    ),
    ("signal-fill", &["button.background"]),
    ("on-signal", &["button.foreground"]),
    ("signal-ink", &["textLink.foreground"]),
    (
        "ok",
        &["editorGutter.addedBackground", "terminal.ansiGreen"],
    ),
    ("warn", &["editorWarning.foreground", "terminal.ansiYellow"]),
    ("crit", &["editorError.foreground", "terminal.ansiRed"]),
    ("info", &["editorInfo.foreground", "terminal.ansiBlue"]),
    ("add-bg", &["diffEditor.insertedTextBackground"]),
    ("del-bg", &["diffEditor.removedTextBackground"]),
    ("term-bg", &["terminal.background"]),
    ("term-fg", &["terminal.foreground"]),
];

/// The terminal colour keys, in `ansi-0` to `ansi-15` order.
const ANSI_KEYS: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

/// The syntax tokens, each with the token-colour scopes that feed it, in order.
const SYNTAX: &[(&str, &[&str])] = &[
    ("syn-keyword", &["keyword", "storage"]),
    ("syn-string", &["string"]),
    (
        "syn-type",
        &["entity.name.type", "support.type", "storage.type"],
    ),
    ("syn-number", &["constant.numeric", "constant"]),
    ("syn-comment", &["comment"]),
    (
        "syn-function",
        &["entity.name.function", "support.function"],
    ),
    ("syn-variable", &["variable"]),
];

/// The semanticTokenColors keys that override the syntax tokens.
const SEMANTIC: &[(&str, &str)] = &[
    ("syn-keyword", "keyword"),
    ("syn-string", "string"),
    ("syn-type", "type"),
    ("syn-number", "number"),
    ("syn-comment", "comment"),
    ("syn-function", "function"),
    ("syn-variable", "variable"),
];

/// The ANSI indices that feed `chart-2` to `chart-8`.
const CHART_ANSI: [usize; 7] = [4, 2, 5, 3, 1, 6, 12];

/// Parses JSON that may hold `//` and `/* */` comments and trailing commas before `}` or `]`. Text inside
/// strings is left alone, so a `//` in a value survives.
pub fn parse_jsonc(text: &str) -> Result<Value, String> {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for next in chars.by_ref() {
                    if prev == '*' && next == '/' {
                        break;
                    }
                    prev = next;
                }
            }
            '}' | ']' => {
                if let Some(i) = out.iter().rposition(|c| !c.is_whitespace())
                    && out[i] == ','
                {
                    out.remove(i);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    let clean: String = out.into_iter().collect();
    serde_json::from_str(&clean).map_err(|e| format!("not JSON: {e}"))
}

/// An imported theme: the file, its name and the mode its palette is for.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub file: ThemeFile,
    pub name: String,
    pub mode: Mode,
}

/// The mode a VS Code theme `type` or `uiTheme` names. The light names give light; every other name gives dark.
pub fn mode_of(type_or_ui_theme: &str) -> Mode {
    match type_or_ui_theme {
        "light" | "hcLight" | "vs" | "hc-light" => Mode::Light,
        _ => Mode::Dark,
    }
}

/// Imports a VS Code theme `text` as a theme for `skin`, with no mode hint. See [`from_json_hinted`].
pub fn from_json(
    text: &str,
    skin: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Import, String> {
    from_json_hinted(text, skin, lookup, None)
}

/// Imports a VS Code theme `text` as a theme for `skin`. `lookup` returns the text of a file named by an
/// `include`, given the path exactly as the including file wrote it. The theme's own `colors` and `tokenColors`
/// are read after its parents', so the child overrides them. Fails with every problem the resulting file has.
///
/// The mode is the first `type` or `uiTheme` the chain names. When none names one, `mode_hint` decides it (a
/// `.vsix` passes its package's `uiTheme`), and dark is the default. The mode also picks the default paper.
/// Input over 4 MiB is refused, the root file and each include alike.
pub fn from_json_hinted(
    text: &str,
    skin: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    mode_hint: Option<Mode>,
) -> Result<Import, String> {
    if text.len() > MAX_JSON {
        return Err(format!("the theme is larger than {MAX_JSON} bytes"));
    }
    let root = parse_jsonc(text)?;
    if !root.is_object() {
        return Err("a VS Code theme is a JSON object".into());
    }
    let layers = include_chain(root, lookup)?;
    let name = first_str(&layers, &["name"])
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Imported theme".into());
    let name: String = name.chars().take(80).collect();
    let mode = match first_str(&layers, &["type", "uiTheme"]) {
        Some(named) => mode_of(&named),
        None => mode_hint.unwrap_or(Mode::Dark),
    };

    let mut colors: Map<String, Value> = Map::new();
    let mut token_colors: Vec<Value> = Vec::new();
    let mut semantic: Map<String, Value> = Map::new();
    for layer in layers.iter().rev() {
        if let Some(Value::Object(c)) = layer.get("colors") {
            for (k, v) in c {
                colors.insert(k.clone(), v.clone());
            }
        }
        if let Some(Value::Array(t)) = layer.get("tokenColors") {
            token_colors.extend(t.iter().cloned());
        }
        if let Some(Value::Object(s)) = layer.get("semanticTokenColors") {
            for (k, v) in s {
                semantic.insert(k.clone(), v.clone());
            }
        }
    }

    let default = default_paper(mode);
    let paper = pick(&colors, &["editor.background"], default).unwrap_or(default);
    let mut out: BTreeMap<String, Rgba> = BTreeMap::new();
    out.insert("paper".into(), paper);
    for &(token, keys) in KEY_MAP {
        if let Some(c) = pick(&colors, keys, paper) {
            out.insert(token.into(), c);
        }
    }
    out.entry("term-bg".into()).or_insert(paper);
    for (i, key) in ANSI_KEYS.iter().enumerate() {
        if let Some(c) = pick(&colors, &[*key], paper) {
            out.insert(format!("ansi-{i}"), c);
        }
    }
    for &(token, keys) in SYNTAX {
        if let Some(c) = keys
            .iter()
            .find_map(|key| scope_colour(&token_colors, key, paper))
        {
            out.insert(token.into(), c);
        }
    }
    for &(token, key) in SEMANTIC {
        if let Some(c) = semantic.get(key).and_then(|v| colour_value(v, paper)) {
            out.insert(token.into(), c);
        }
    }
    if let Some(signal) = out.get("signal").copied() {
        out.insert("chart-1".into(), signal);
    }
    for (n, ansi) in CHART_ANSI.iter().enumerate() {
        if let Some(c) = out.get(&format!("ansi-{ansi}")).copied() {
            out.insert(format!("chart-{}", n + 2), c);
        }
    }

    let palette: PaletteText = out.iter().map(|(k, v)| (k.clone(), v.to_hex())).collect();
    let mut palettes = Palettes::default();
    match mode {
        Mode::Light => palettes.light = Some(palette),
        Mode::Dark => palettes.dark = Some(palette),
    }
    let file = ThemeFile {
        schema: SCHEMA,
        id: format!("vscode-{}", slug(&name)),
        name: name.clone(),
        about: String::new(),
        skin: skin.to_string(),
        palette: palettes,
        background: None,
        overrides: Palettes::default(),
        source: Some(format!("vscode: {name}")),
    };
    let problems = file.problems();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(Import { file, name, mode })
}

/// The theme and the files it includes, the theme first and each parent after its child. Fails on a cycle, on a
/// missing file and on a chain deeper than [`MAX_INCLUDE_DEPTH`].
fn include_chain(
    root: Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<Value>, String> {
    let mut layers = vec![root];
    let mut seen: Vec<String> = Vec::new();
    while let Some(path) = layers
        .last()
        .and_then(|l| l.get("include"))
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        if seen.contains(&path) {
            return Err(format!("include cycle at {path:?}"));
        }
        if seen.len() >= MAX_INCLUDE_DEPTH {
            return Err(format!(
                "the include chain is deeper than {MAX_INCLUDE_DEPTH} files"
            ));
        }
        let text = lookup(&path).ok_or_else(|| format!("include {path:?} was not found"))?;
        if text.len() > MAX_JSON {
            return Err(format!("include {path:?} is larger than {MAX_JSON} bytes"));
        }
        let value = parse_jsonc(&text).map_err(|e| format!("include {path:?}: {e}"))?;
        seen.push(path);
        layers.push(value);
    }
    Ok(layers)
}

/// The first string under one of `keys`, reading the theme first and then its parents.
fn first_str(layers: &[Value], keys: &[&str]) -> Option<String> {
    layers
        .iter()
        .find_map(|layer| keys.iter().find_map(|key| layer.get(*key)?.as_str()))
        .map(str::to_string)
}

/// The page colour a theme without `editor.background` gets: VS Code's own default for the mode.
fn default_paper(mode: Mode) -> Rgba {
    match mode {
        Mode::Dark => Rgba::rgb8(0x1e, 0x1e, 0x1e),
        Mode::Light => Rgba::WHITE,
    }
}

/// The first of `keys` whose colour parses, composited over `ground`.
fn pick(colors: &Map<String, Value>, keys: &[&str], ground: Rgba) -> Option<Rgba> {
    keys.iter()
        .find_map(|key| colors.get(*key).and_then(|v| colour_value(v, ground)))
}

/// A colour given as a string, or as an object with a `foreground` string, composited over `ground`.
fn colour_value(value: &Value, ground: Rgba) -> Option<Rgba> {
    value
        .as_str()
        .or_else(|| value.get("foreground")?.as_str())
        .and_then(Rgba::parse)
        .map(|c| c.over(ground))
}

/// The foreground of the most specific token rule that names `key` with a usable colour, composited over `ground`.
/// A rule's specificity is the number of dot-separated parts of its matching scope, so `comment.line` beats
/// `comment`. Among equally specific rules the later one wins, and a child theme's rules come last.
fn scope_colour(rules: &[Value], key: &str, ground: Rgba) -> Option<Rgba> {
    let mut best: Option<(usize, Rgba)> = None;
    for rule in rules {
        let Some(depth) = scopes(rule)
            .iter()
            .filter(|scope| scope_is(scope, key))
            .map(|scope| scope.split('.').count())
            .max()
        else {
            continue;
        };
        let colour = rule
            .get("settings")
            .and_then(|settings| colour_value(settings.get("foreground")?, ground));
        let Some(colour) = colour else {
            continue;
        };
        if best.is_none_or(|(d, _)| depth >= d) {
            best = Some((depth, colour));
        }
    }
    best.map(|(_, colour)| colour)
}

/// The scopes a token rule names: a comma-separated string or an array. Each one counts by its last
/// whitespace-separated part, so `source.ts keyword` names `keyword`.
fn scopes(rule: &Value) -> Vec<&str> {
    let raw: Vec<&str> = match rule.get("scope") {
        Some(Value::String(s)) => s.split(',').collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    raw.into_iter()
        .filter_map(|s| s.split_whitespace().last())
        .collect()
}

/// Whether `scope` is `key` or a more specific scope under it, such as `keyword.control` under `keyword`.
fn scope_is(scope: &str, key: &str) -> bool {
    scope == key
        || scope
            .strip_prefix(key)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// A file name for a theme: lower-case a–z and 0–9 with single dashes, at most 57 characters so the id fits
/// the 64 the file format allows after `vscode-`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let cut: String = out.trim_matches('-').chars().take(57).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "theme".into()
    } else {
        cut.into()
    }
}
