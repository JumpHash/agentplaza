//! VS Code extension packages (`.vsix`): the themes an extension contributes, read from the archive in memory.
//!
//! A `.vsix` is a zip whose `extension/package.json` lists `contributes.themes` (label, uiTheme, and a path relative
//! to `extension/`). [`import`] reads one theme and follows its `include` chain inside the archive: keys of the child
//! win, `colors` and `semanticTokenColors` merge key by key, and `tokenColors` lists put the parent's rules first.
//! The merged JSON then goes to [`vscode::from_json`] with no further lookups. Nothing is written to disk.

use std::io::{Cursor, Read};

use serde_json::{Map, Value};

use crate::vscode;

/// The largest archive accepted, in bytes.
const MAX_ARCHIVE: usize = 64 * 1024 * 1024;
/// The largest entry read from an archive, in bytes. The declared size is checked, then the bytes actually read.
const MAX_ENTRY: u64 = 8 * 1024 * 1024;
/// The most entries an archive may hold.
const MAX_ENTRIES: usize = 4000;
/// How deep an `include` chain may go before it is refused, which also ends any cycle.
const MAX_INCLUDE_DEPTH: usize = 16;
/// The folder every contributed path is relative to.
const ROOT: &str = "extension";

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// A theme a package contributes: its label, its `uiTheme` ("vs", "vs-dark", "hc-black", ...) and its path as written
/// in the manifest, relative to `extension/`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct VsixTheme {
    pub label: String,
    pub ui_theme: String,
    pub path: String,
}

/// The themes `extension/package.json` contributes (contributes.themes[]: label, uiTheme, path relative to extension/).
/// A theme without a label is listed under its file name; one without a path is an error.
pub fn themes(bytes: &[u8]) -> Result<Vec<VsixTheme>, String> {
    let mut archive = open(bytes)?;
    let manifest = package_json(&mut archive)?;
    contributed(&manifest)
}

/// Imports one theme from the package: `pick` is a label; None takes the first. Includes resolve inside the archive,
/// relative to the including file's directory (normalise `./` and `../`; refuse paths that leave `extension/`).
pub fn import(bytes: &[u8], pick: Option<&str>, skin: &str) -> Result<vscode::Import, String> {
    let mut archive = open(bytes)?;
    let manifest = package_json(&mut archive)?;
    let list = contributed(&manifest)?;
    let theme = match pick {
        None => list.first().ok_or("the package contributes no themes")?,
        Some(label) => list.iter().find(|t| t.label == label).ok_or_else(|| {
            let names: Vec<&str> = list.iter().map(|t| t.label.as_str()).collect();
            format!(
                "no theme labelled {label:?} in the package (it has: {})",
                names.join(", ")
            )
        })?,
    };
    let path = resolve(ROOT, &theme.path)?;
    let json = theme_json(&mut archive, &path, 0)?;
    // The package's uiTheme is the mode hint: it decides the mode only when the merged JSON has no "type" or
    // "uiTheme" of its own, and it picks the default paper.
    let has_name = json.contains_key("name");
    let text =
        serde_json::to_string(&Value::Object(json)).map_err(|e| format!("{}: {e}", theme.label))?;
    let no_includes = |_: &str| -> Option<String> { None };
    let ui_mode = vscode::mode_of(&theme.ui_theme);
    let mut imp = vscode::from_json_hinted(&text, skin, &no_includes, Some(ui_mode))?;
    if !has_name {
        imp.name = theme.label.clone();
        imp.file.name = theme.label.clone();
    }
    if imp.file.source.is_none() {
        imp.file.source = package_source(&manifest);
    }
    Ok(imp)
}

fn open(bytes: &[u8]) -> Result<Archive<'_>, String> {
    if bytes.len() > MAX_ARCHIVE {
        return Err(format!("the package is larger than {MAX_ARCHIVE} bytes"));
    }
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("not a .vsix (zip) package: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("the package has more than {MAX_ENTRIES} entries"));
    }
    Ok(archive)
}

/// Reads one entry as UTF-8 text, without its byte order mark.
fn read_entry(archive: &mut Archive<'_>, path: &str) -> Result<String, String> {
    let file = archive
        .by_name(path)
        .map_err(|_| format!("the package has no {path}"))?;
    if file.size() > MAX_ENTRY {
        return Err(format!("{path} is larger than {MAX_ENTRY} bytes"));
    }
    let mut data = Vec::new();
    file.take(MAX_ENTRY + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("{path}: {e}"))?;
    if data.len() as u64 > MAX_ENTRY {
        return Err(format!("{path} is larger than {MAX_ENTRY} bytes"));
    }
    let text = String::from_utf8(data).map_err(|_| format!("{path} is not UTF-8"))?;
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

fn package_json(archive: &mut Archive<'_>) -> Result<Value, String> {
    let text = read_entry(archive, "extension/package.json")?;
    serde_json::from_str(&text).map_err(|e| format!("extension/package.json: {e}"))
}

/// The themes listed under `contributes.themes` in a manifest.
fn contributed(manifest: &Value) -> Result<Vec<VsixTheme>, String> {
    let Some(list) = manifest.pointer("/contributes/themes") else {
        return Ok(Vec::new());
    };
    let list = list.as_array().ok_or("contributes.themes is not a list")?;
    list.iter()
        .enumerate()
        .map(|(i, item)| -> Result<VsixTheme, String> {
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("contributes.themes[{i}] has no path"))?;
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .map_or_else(|| file_stem(path), str::to_string);
            let ui_theme = item
                .get("uiTheme")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(VsixTheme {
                label,
                ui_theme,
                path: path.to_string(),
            })
        })
        .collect()
}

/// The file name without its folder or extension.
fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_string()
}

/// The `vscode: <name>` source text for a package: its display name, or its name.
fn package_source(manifest: &Value) -> Option<String> {
    let name = manifest
        .get("displayName")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("name").and_then(Value::as_str))?;
    Some(format!("vscode: {name}"))
}

/// Joins a relative path onto a folder inside the archive and normalises it. A path that climbs out of `extension/`
/// is refused, as is an absolute path.
fn resolve(dir: &str, rel: &str) -> Result<String, String> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') {
        return Err(format!("{rel}: absolute paths are refused"));
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            ".." => return Err(format!("{rel}: leaves {ROOT}/")),
            _ => parts.push(part),
        }
    }
    if parts.len() < 2 {
        return Err(format!("{rel}: leaves {ROOT}/"));
    }
    Ok(parts.join("/"))
}

/// The folder part of an archive path ("" for a file at the top).
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The theme JSON at `path` with its `include` chain merged in, parent first. The result has no "include" key.
fn theme_json(
    archive: &mut Archive<'_>,
    path: &str,
    depth: usize,
) -> Result<Map<String, Value>, String> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(format!(
            "{path}: the include chain is deeper than {MAX_INCLUDE_DEPTH}"
        ));
    }
    let text = read_entry(archive, path)?;
    let Value::Object(mut child) = vscode::parse_jsonc(&text)? else {
        return Err(format!("{path}: not a theme object"));
    };
    let Some(include) = child.remove("include") else {
        return Ok(child);
    };
    let include = include
        .as_str()
        .ok_or_else(|| format!("{path}: include is not a path"))?;
    let parent_path = resolve(dir_of(path), include)?;
    let parent = theme_json(archive, &parent_path, depth + 1)?;
    Ok(merge(parent, child))
}

/// Child keys override parent keys. `tokenColors` lists are concatenated, parent first; other objects merge key by key.
fn merge(mut base: Map<String, Value>, over: Map<String, Value>) -> Map<String, Value> {
    for (key, value) in over {
        let merged = match (base.remove(&key), value) {
            (Some(Value::Array(mut parent)), Value::Array(child)) if key == "tokenColors" => {
                parent.extend(child);
                Value::Array(parent)
            }
            (Some(Value::Object(mut parent)), Value::Object(child)) => {
                parent.extend(child);
                Value::Object(parent)
            }
            (_, child) => child,
        };
        base.insert(key, merged);
    }
    base
}
