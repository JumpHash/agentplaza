//! The code-intelligence tools (PLAN 15.2, 15.3): `symbols`, `definition`, `references`, `diagnostics` and `outline`.
//!
//! A language server answers for its language when one is installed in this VM and answers for the file; the answer
//! names the server. Otherwise, or when the server fails (the answer then says why), Weft's tree-sitter index of the
//! lane's revision answers. A file outside every lane, in a language no index covers, is refused with the reason.
//! Lines are one-based and columns one-based in UTF-16 units, as the protocol counts them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use apz_lsp::{
    Checked, Diagnostic, DocumentSymbol, Language, Location, LspError, Pool, Position, Timeouts,
};
use serde_json::{Value, json};

use crate::files::{self, Workspace};
use crate::tools::ToolError;
use crate::weft_lane;

/// Where the fallback's answers come from: Weft's CLI in the lane's directory, or a canned one in the tests.
pub trait Index {
    /// The symbols and references of one lane-relative path at the lane's revision (`weft symbols`).
    fn symbols(&self, lane: &Path, path: &str) -> Result<Value, String>;
    /// The definitions and references of a name, or with `find`, of the names containing it (`weft names`).
    fn names(&self, lane: &Path, name: &str, find: bool) -> Result<Value, String>;
}

/// Weft's CLI in this VM, run in the lane's directory, through the same hub the lanes use.
pub struct Cli;

impl Index for Cli {
    fn symbols(&self, lane: &Path, path: &str) -> Result<Value, String> {
        run_json(lane, &["symbols", path, "--json"])
    }

    fn names(&self, lane: &Path, name: &str, find: bool) -> Result<Value, String> {
        if find {
            run_json(lane, &["names", "--find", name, "--json"])
        } else {
            run_json(lane, &["names", name, "--json"])
        }
    }
}

fn run_json(lane: &Path, args: &[&str]) -> Result<Value, String> {
    let bin = weft_lane::cli().ok_or("no weft CLI in this VM")?;
    let text = weft_lane::run(&bin, lane, args)?;
    serde_json::from_str(&text).map_err(|e| format!("weft answered something other than JSON: {e}"))
}

/// What a tool call works with.
pub struct Ctx<'a> {
    pub workspace: &'a Workspace,
    pub pool: &'a Pool,
    pub index: &'a dyn Index,
    /// Whether a language's server is installed in this VM.
    pub installed: &'a dyn Fn(Language) -> bool,
}

/// The servers of this process: started on the first query for their language, kept up for its life. A server a base
/// started and warmed (PLAN 3.8) is reached on its socket in the workspace's plaza directory, for the workspace root.
fn shared_pool(workspace: &Workspace) -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        Pool::new(Timeouts::default())
            .with_warm(workspace.plaza_dir().join("lsp"), workspace.weft_root())
    })
}

/// Runs one of the tools for the agent, with the VM's servers and Weft's CLI.
pub fn run(workspace: &Workspace, name: &str, input: &Value) -> Result<String, String> {
    let installed = apz_lsp::is_installed;
    let cx = Ctx {
        workspace,
        pool: shared_pool(workspace),
        index: &Cli,
        installed: &installed,
    };
    call(&cx, name, input).map_err(|e| e.to_string())
}

/// Runs a tool against a context: the entry the tests use with their own pool, index and installed set.
pub fn call(cx: &Ctx, name: &str, input: &Value) -> Result<String, ToolError> {
    let answer = match name {
        "symbols" => symbols(cx, input)?,
        "definition" => location_answer(cx, input, Want::Definition)?,
        "references" => location_answer(cx, input, Want::References)?,
        "diagnostics" => diagnostics(cx, input)?,
        "outline" => outline(cx, input)?,
        other => return Err(ToolError::UnknownTool(other.to_string())),
    };
    Ok(answer.to_string())
}

/// A file the tools were asked about.
struct File {
    abs: PathBuf,
    /// Its path from the workspace root, as the tools name files.
    label: String,
    /// Where its server runs: its lane's directory, else the workspace.
    root: PathBuf,
    lane: Option<Lane>,
    language: Option<Language>,
}

/// A lane's directory and a path within it.
struct Lane {
    dir: PathBuf,
    rel: String,
}

fn file(cx: &Ctx, raw: &str) -> Result<File, ToolError> {
    let (abs, label) = cx.workspace.resolve(raw)?;
    let lane = lane_of(cx.workspace, &label);
    let root = match &lane {
        Some(lane) => lane.dir.clone(),
        None => cx.workspace.weft_root(),
    };
    Ok(File {
        abs,
        language: Language::from_path(&label),
        label,
        root,
        lane,
    })
}

/// The lane a workspace path is in: its first component is the lane's directory under the weft root.
fn lane_of(workspace: &Workspace, label: &str) -> Option<Lane> {
    let (repo, rel) = label.split_once('/')?;
    let dir = workspace.weft_root().join(repo);
    dir.join(".weft").join("lane.json").is_file().then(|| Lane {
        dir,
        rel: rel.to_string(),
    })
}

/// The lane and the lane-relative path of a file, or why there is no index for it.
fn indexed(f: &File) -> Result<(&Lane, String), ToolError> {
    match &f.lane {
        Some(lane) => Ok((lane, lane.rel.clone())),
        None => Err(ToolError::NotFound {
            reason: format!(
                "no code index for {}: it is outside the lanes, and no language server here answers for it",
                f.label
            ),
        }),
    }
}

/// The server's answer for a file, when its language's server is installed here.
enum Served<T> {
    Answer(T, String),
    /// No server here for the file's language, with the reason to say so; none when no language covers the file.
    NotHere(Option<String>),
    /// The server is installed but failed: the answer comes from Weft, and says why.
    Failed(String),
}

fn served<T>(
    cx: &Ctx,
    f: &File,
    ask: impl FnOnce(&Pool, &Path, &Path) -> Result<T, LspError>,
) -> Result<Served<T>, ToolError> {
    let Some(language) = f.language else {
        return Ok(Served::NotHere(None));
    };
    if !(cx.installed)(language) {
        let missing = LspError::NoServer {
            language: language.name().to_string(),
            command: language.server().command.to_string(),
        };
        return Ok(Served::NotHere(Some(missing.to_string())));
    }
    match ask(cx.pool, &f.root, &f.abs) {
        Ok(answer) => Ok(Served::Answer(
            answer,
            format!("language server ({})", language.server().id),
        )),
        Err(e @ LspError::File(_)) => Err(ToolError::InvalidInput {
            reason: e.to_string(),
        }),
        Err(e) => Ok(Served::Failed(e.to_string())),
    }
}

/// The note a fallback answer carries: why no server answered (`no server for <language> here ...`), or why it
/// failed. Absent when there was nothing to say.
fn with_note(mut answer: Value, note: Option<String>) -> Value {
    if let Some(note) = note {
        answer["server"] = json!(note);
    }
    answer
}

fn weft_via(v: &Value) -> String {
    match v["pack"].as_str() {
        Some(pack) => format!("weft (tree-sitter, pack {pack})"),
        None => "weft (tree-sitter)".to_string(),
    }
}

fn unavailable(reason: String) -> ToolError {
    ToolError::Unavailable { reason }
}

fn invalid(reason: impl Into<String>) -> ToolError {
    files::invalid(reason)
}

/// An argument the call must have.
fn required<'v>(input: &'v Value, key: &str) -> Result<&'v Value, ToolError> {
    input
        .get(key)
        .ok_or_else(|| invalid(format!("\"{key}\" is required")))
}

fn string_arg(v: &Value, what: &str) -> Result<String, ToolError> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("{what} must be a string")))
}

/// A one-based line or column.
fn position_arg(input: &Value, key: &str) -> Result<usize, ToolError> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .filter(|n| *n >= 1)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| invalid(format!("\"{key}\" must be a number from 1")))
}

/// Where a path the server names is, for the answer: its place in the workspace, else the path as it is.
fn label_of(workspace: &Workspace, path: &Path) -> String {
    match path.strip_prefix(workspace.weft_root()) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The lane-relative path of a Weft answer's path, named from the workspace root (`<repo>/<path>`).
fn lane_label(lane: &Lane, path: &str) -> String {
    let repo = lane
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{repo}/{path}")
}

fn position_out(p: Position) -> (u32, u32) {
    (p.line + 1, p.character + 1)
}

fn location_out(workspace: &Workspace, l: &Location) -> Value {
    let (line, column) = position_out(l.range.start);
    let (end_line, end_column) = position_out(l.range.end);
    let path = match l.path() {
        Some(path) => label_of(workspace, &path),
        None => l.uri.clone(),
    };
    json!({ "path": path, "line": line, "column": column, "end_line": end_line, "end_column": end_column })
}

/// A server's symbols as a flat list, each with the path of its parents.
fn flatten(items: &[DocumentSymbol], parent: Option<&str>, out: &mut Vec<Value>) {
    for s in items {
        let qualified = match parent {
            Some(p) => format!("{p} › {}", s.name),
            None => s.name.clone(),
        };
        out.push(json!({
            "name": s.name,
            "kind": s.kind,
            "qualified": qualified,
            "line": s.range.start.line + 1,
            "end_line": s.range.end.line + 1,
        }));
        flatten(&s.children, Some(&qualified), out);
    }
}

/// A server's symbols as a tree.
fn outline_of(items: &[DocumentSymbol]) -> Vec<Value> {
    items
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "kind": s.kind,
                "line": s.range.start.line + 1,
                "end_line": s.range.end.line + 1,
                "children": outline_of(&s.children),
            })
        })
        .collect()
}

/// Weft's symbols of a file, flat, with their parents' names: refused when Weft has no pack for the file.
fn weft_symbols(path: &str, v: &Value) -> Result<Vec<Value>, ToolError> {
    if v["pack"].is_null() {
        return Err(ToolError::NotFound {
            reason: format!(
                "no code index for {path}: no language server here, and Weft has no tree-sitter pack for its language"
            ),
        });
    }
    let items = v["symbols"].as_array().cloned().unwrap_or_default();
    let mut qualified: Vec<String> = Vec::with_capacity(items.len());
    let mut out = Vec::with_capacity(items.len());
    for s in &items {
        let name = s["name"].as_str().unwrap_or("").to_string();
        let parent = s["parent"].as_u64().and_then(|p| usize::try_from(p).ok());
        let full = match parent.and_then(|p| qualified.get(p)) {
            Some(p) => format!("{p} › {name}"),
            None => name.clone(),
        };
        out.push(json!({
            "name": name,
            "kind": s["kind"],
            "qualified": full,
            "line": s["start_line"],
            "end_line": s["end_line"],
        }));
        qualified.push(full);
    }
    Ok(out)
}

/// Weft's symbols as a tree, from each symbol's parent.
fn weft_outline(flat: &[Value], parents: &[Option<usize>]) -> Vec<Value> {
    let mut nodes: Vec<Value> = flat
        .iter()
        .map(|s| {
            json!({
                "name": s["name"],
                "kind": s["kind"],
                "line": s["line"],
                "end_line": s["end_line"],
                "children": [],
            })
        })
        .collect();
    let mut roots = Vec::new();
    for i in (0..nodes.len()).rev() {
        let node = std::mem::take(&mut nodes[i]);
        match parents.get(i).copied().flatten() {
            Some(p) if p < i => nodes[p]["children"]
                .as_array_mut()
                .map(|c| c.insert(0, node)),
            _ => {
                roots.insert(0, node);
                None
            }
        };
    }
    roots
}

fn parents_of(v: &Value) -> Vec<Option<usize>> {
    v["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| s["parent"].as_u64().and_then(|p| usize::try_from(p).ok()))
        .collect()
}

fn symbols(cx: &Ctx, input: &Value) -> Result<Value, ToolError> {
    match (input.get("path"), input.get("query")) {
        (Some(path), _) => {
            let f = file(cx, &string_arg(path, "path")?)?;
            let note = match served(cx, &f, |pool, root, abs| pool.document_symbols(root, abs))? {
                Served::Answer(items, via) => {
                    let mut out = Vec::new();
                    flatten(&items, None, &mut out);
                    return Ok(json!({ "path": f.label, "via": via, "symbols": out }));
                }
                Served::Failed(why) => Some(why),
                Served::NotHere(why) => why,
            };
            let (lane, rel) = indexed(&f)?;
            let v = cx.index.symbols(&lane.dir, &rel).map_err(unavailable)?;
            let symbols = weft_symbols(&f.label, &v)?;
            Ok(with_note(
                json!({ "path": f.label, "via": weft_via(&v), "symbols": symbols }),
                note,
            ))
        }
        (None, Some(query)) => {
            let text = string_arg(query, "query")?;
            let lanes = workspace_lanes(cx.workspace)?;
            if lanes.is_empty() {
                return Err(ToolError::NotFound {
                    reason: "no lane is checked out here, so there is no index to search".into(),
                });
            }
            let mut hits = Vec::new();
            for lane in &lanes {
                let v = cx
                    .index
                    .names(&lane.dir, &text, true)
                    .map_err(unavailable)?;
                for d in v["definitions"].as_array().into_iter().flatten() {
                    hits.push(json!({
                        "path": lane_label(lane, d["path"].as_str().unwrap_or("")),
                        "name": d["name"],
                        "kind": d["kind"],
                        "qualified": d["qualified"],
                        "line": d["start_line"],
                        "end_line": d["end_line"],
                    }));
                }
            }
            hits.truncate(200);
            Ok(
                json!({ "query": text, "via": "weft (tree-sitter, the lanes' revisions)", "symbols": hits }),
            )
        }
        (None, None) => Err(invalid("symbols needs a path or a query")),
    }
}

/// Every lane checked out under the weft root.
fn workspace_lanes(workspace: &Workspace) -> Result<Vec<Lane>, ToolError> {
    let root = workspace.weft_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    let mut lanes = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.join(".weft").join("lane.json").is_file() {
            lanes.push(Lane {
                dir,
                rel: String::new(),
            });
        }
    }
    lanes.sort_by(|a, b| a.dir.cmp(&b.dir));
    Ok(lanes)
}

#[derive(Clone, Copy)]
enum Want {
    Definition,
    References,
}

fn location_answer(cx: &Ctx, input: &Value, want: Want) -> Result<Value, ToolError> {
    let raw = string_arg(required(input, "path")?, "path")?;
    let line = position_arg(input, "line")?;
    let column = position_arg(input, "column")?;
    let f = file(cx, &raw)?;
    let at = Position {
        line: u32::try_from(line - 1).map_err(|_| invalid("\"line\" is too large"))?,
        character: u32::try_from(column - 1).map_err(|_| invalid("\"column\" is too large"))?,
    };
    let served_by = served(cx, &f, |pool, root, abs| match want {
        Want::Definition => pool.definition(root, abs, at),
        Want::References => pool.references(root, abs, at),
    })?;
    let note = match served_by {
        Served::Answer(found, via) => {
            let locations: Vec<Value> = found
                .iter()
                .map(|l| location_out(cx.workspace, l))
                .collect();
            return Ok(
                json!({ "path": f.label, "line": line, "column": column, "via": via, "locations": locations }),
            );
        }
        Served::Failed(why) => Some(why),
        Served::NotHere(why) => why,
    };
    // The fallback: the name at the position, looked up in Weft's index of the lane's revision.
    let (lane, _) = indexed(&f)?;
    let text = fs::read_to_string(&f.abs)
        .map_err(|e| files::io_tool_error(e, format!("cannot read {}", f.label)))?;
    let name = ident_at(&text, line, column)
        .ok_or_else(|| invalid(format!("no name at {}:{line}:{column}", f.label)))?;
    let v = cx
        .index
        .names(&lane.dir, &name, false)
        .map_err(unavailable)?;
    let key = match want {
        Want::Definition => "definitions",
        Want::References => "references",
    };
    let locations: Vec<Value> = v[key]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            json!({
                "path": lane_label(lane, d["path"].as_str().unwrap_or("")),
                "line": d["start_line"].as_u64().or(d["line"].as_u64()),
                "end_line": d["end_line"],
                "kind": d["kind"],
            })
        })
        .collect();
    Ok(with_note(
        json!({ "path": f.label, "line": line, "column": column, "name": name, "via": "weft (tree-sitter index)", "locations": locations }),
        note,
    ))
}

/// The identifier at a one-based line and UTF-16 column, when there is one there.
fn ident_at(text: &str, line: usize, column: usize) -> Option<String> {
    let line_text = text.lines().nth(line.checked_sub(1)?)?;
    let target = column.checked_sub(1)?;
    let mut units = 0;
    let mut at = None;
    let chars: Vec<char> = line_text.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if units >= target {
            at = Some(i);
            break;
        }
        units += ch.len_utf16();
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = at?;
    if !is_ident(*chars.get(i)?) {
        // A cursor just after a name names it.
        if i == 0 || !is_ident(chars[i - 1]) {
            return None;
        }
        i -= 1;
    }
    let mut start = i;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < chars.len() && is_ident(chars[end + 1]) {
        end += 1;
    }
    Some(chars[start..=end].iter().collect())
}

/// The changed files of every lane: those whose entries differ from the base's, as `<repo>/<path>`.
fn changed_paths(workspace: &Workspace) -> Result<Vec<String>, ToolError> {
    let mut out = Vec::new();
    for lane in workspace_lanes(workspace)? {
        let text = fs::read_to_string(lane.dir.join(".weft").join("lane.json"))
            .map_err(|e| files::io_tool_error(e, "cannot read the lane"))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| unavailable(format!("the lane's record: {e}")))?;
        for (path, entry) in v["files"].as_object().into_iter().flatten() {
            if v["base_files"].get(path) != Some(entry) {
                out.push(lane_label(&lane, path));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn diagnostics(cx: &Ctx, input: &Value) -> Result<Value, ToolError> {
    let paths: Vec<String> = match input.get("paths") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|p| string_arg(p, "paths"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(invalid("paths must be an array of strings")),
        None => changed_paths(cx.workspace)?,
    };
    let checked: Vec<Value> = paths
        .iter()
        .map(|raw| match check_file(cx, raw) {
            Ok(answer) => answer,
            Err(e) => json!({ "path": raw, "error": e.to_string() }),
        })
        .collect();
    Ok(json!({ "checked": checked }))
}

fn diagnostic_out(d: &Diagnostic) -> Value {
    let severity = match d.severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "information",
        Some(4) => "hint",
        _ => "unknown",
    };
    json!({
        "line": d.range.start.line + 1,
        "column": d.range.start.character + 1,
        "severity": severity,
        "source": d.source,
        "message": d.message,
    })
}

fn check_file(cx: &Ctx, raw: &str) -> Result<Value, ToolError> {
    let f = file(cx, raw)?;
    let note = match served(cx, &f, |pool, root, abs| pool.diagnostics(root, abs))? {
        Served::Answer(Checked { items, settled }, via) => {
            let items: Vec<Value> = items.iter().map(diagnostic_out).collect();
            return Ok(
                json!({ "path": f.label, "via": via, "diagnostics": items, "settled": settled }),
            );
        }
        Served::Failed(why) => Some(why),
        Served::NotHere(why) => why,
    };
    // The fallback knows only whether the file parses: a tree-sitter error is the one diagnostic it can give.
    let (lane, rel) = indexed(&f)?;
    let v = cx.index.symbols(&lane.dir, &rel).map_err(unavailable)?;
    if v["pack"].is_null() {
        return Ok(with_note(
            json!({ "path": f.label, "via": "none", "diagnostics": [], "note": "no language server here and no Weft pack for its language: nothing was checked" }),
            note,
        ));
    }
    let items = if v["error"].as_bool() == Some(true) {
        vec![json!({
            "severity": "error",
            "message": "tree-sitter found a syntax error in this file (no language server here)",
        })]
    } else {
        Vec::new()
    };
    Ok(with_note(
        json!({ "path": f.label, "via": weft_via(&v), "diagnostics": items }),
        note,
    ))
}

fn outline(cx: &Ctx, input: &Value) -> Result<Value, ToolError> {
    let f = file(cx, &string_arg(required(input, "path")?, "path")?)?;
    let note = match served(cx, &f, |pool, root, abs| pool.document_symbols(root, abs))? {
        Served::Answer(items, via) => {
            return Ok(json!({ "path": f.label, "via": via, "outline": outline_of(&items) }));
        }
        Served::Failed(why) => Some(why),
        Served::NotHere(why) => why,
    };
    let (lane, rel) = indexed(&f)?;
    let v = cx.index.symbols(&lane.dir, &rel).map_err(unavailable)?;
    let flat = weft_symbols(&f.label, &v)?;
    let tree = weft_outline(&flat, &parents_of(&v));
    Ok(with_note(
        json!({ "path": f.label, "via": weft_via(&v), "outline": tree }),
        note,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_lsp::fake::{self, Script};
    use apz_lsp::{Location, Range, path_to_uri};
    use std::cell::RefCell;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../apz-lsp/testdata/repo");

    /// A workspace with one lane (`repo`, checked out from the W40 fixture) and one loose file outside any lane.
    fn workspace(name: &str) -> (PathBuf, Workspace) {
        let root =
            std::env::temp_dir().join(format!("apz-code-intel-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        copy_tree(Path::new(FIXTURE), &root.join("repo"));
        fs::create_dir_all(root.join("repo").join(".weft")).unwrap();
        fs::write(
            root.join("repo").join(".weft").join("lane.json"),
            json!({
                "hub": "http://plaza.gateway/weft", "repo": "repo", "change": "c1", "base": "r0", "at": "c1.r1",
                "base_files": {}, "files": {"src/lib.rs": {"hash": "a", "mode": 33188}}, "next_draft": 1
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(root.join("loose")).unwrap();
        fs::write(root.join("loose").join("notes.rs"), "fn x() {}\n").unwrap();
        let ws = Workspace::at(&root);
        (root, ws)
    }

    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap().flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let target = to.join(&name);
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// The fake index: its answers are canned, and it records what it was asked.
    struct FakeIndex {
        symbols: Value,
        names: Value,
        asked: RefCell<Vec<String>>,
    }

    impl Index for FakeIndex {
        fn symbols(&self, _lane: &Path, path: &str) -> Result<Value, String> {
            self.asked.borrow_mut().push(format!("symbols {path}"));
            Ok(self.symbols.clone())
        }

        fn names(&self, _lane: &Path, name: &str, find: bool) -> Result<Value, String> {
            self.asked
                .borrow_mut()
                .push(format!("names {name} find={find}"));
            Ok(self.names.clone())
        }
    }

    fn index_answers() -> FakeIndex {
        FakeIndex {
            symbols: json!({
                "pack": "rust", "error": false,
                "symbols": [
                    {"kind": "function", "name": "greet", "start_line": 1, "end_line": 3, "parent": null},
                    {"kind": "function", "name": "welcome", "start_line": 5, "end_line": 7, "parent": null}
                ],
                "references": []
            }),
            names: json!({
                "definitions": [{"path": "src/lib.rs", "kind": "function", "name": "greet", "qualified": "fn greet", "start_line": 1, "end_line": 3}],
                "references": [{"path": "src/lib.rs", "kind": "call", "name": "greet", "line": 6}]
            }),
            asked: RefCell::new(Vec::new()),
        }
    }

    fn server_with(script: Script, timeouts: Timeouts) -> Pool {
        Pool::with_launch(timeouts, move |_spec, _root| {
            Ok(fake::transport(script.clone()).0)
        })
    }

    fn quick() -> Timeouts {
        Timeouts {
            warm_up: std::time::Duration::ZERO,
            ..Timeouts::default()
        }
    }

    fn run_tool(cx: &Ctx, name: &str, input: Value) -> Result<Value, ToolError> {
        call(cx, name, &input).map(|s| serde_json::from_str(&s).unwrap())
    }

    #[test]
    fn an_installed_server_answers_every_tool_and_starts_once() {
        let (root, ws) = workspace("server");
        let file = root.join("repo").join("src").join("lib.rs");
        let uri = path_to_uri(&file);
        let at = |line, character| Range {
            start: Position { line, character },
            end: Position {
                line,
                character: character + 5,
            },
        };
        let script = Script {
            definition: vec![Location {
                uri: uri.clone(),
                range: at(0, 7),
            }],
            references: vec![
                Location {
                    uri: uri.clone(),
                    range: at(0, 7),
                },
                Location {
                    uri: uri.clone(),
                    range: at(5, 4),
                },
            ],
            symbols: vec![DocumentSymbol {
                name: "Plaza".into(),
                kind: "class",
                detail: None,
                range: at(0, 0),
                children: vec![DocumentSymbol {
                    name: "open".into(),
                    kind: "method",
                    detail: None,
                    range: at(1, 2),
                    children: vec![],
                }],
            }],
            diagnostics: vec![Diagnostic {
                range: at(2, 0),
                severity: Some(2),
                source: Some("fake".into()),
                message: "unused".into(),
            }],
            ..Script::default()
        };
        let pool = server_with(script, quick());
        let index = index_answers();
        let installed = |_: Language| true;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };

        let def = run_tool(
            &cx,
            "definition",
            json!({"path": "repo/src/lib.rs", "line": 6, "column": 5}),
        )
        .unwrap();
        assert_eq!(def["via"], "language server (rust-analyzer)");
        assert_eq!(def["locations"][0]["path"], "repo/src/lib.rs");
        assert_eq!(def["locations"][0]["line"], 1);
        assert_eq!(def["locations"][0]["column"], 8);
        let refs = run_tool(
            &cx,
            "references",
            json!({"path": "repo/src/lib.rs", "line": 6, "column": 5}),
        )
        .unwrap();
        assert_eq!(refs["locations"].as_array().unwrap().len(), 2);
        let symbols = run_tool(&cx, "symbols", json!({"path": "repo/src/lib.rs"})).unwrap();
        assert_eq!(symbols["symbols"][1]["qualified"], "Plaza › open");
        let outline = run_tool(&cx, "outline", json!({"path": "repo/src/lib.rs"})).unwrap();
        assert_eq!(outline["outline"][0]["children"][0]["name"], "open");
        let checked = run_tool(&cx, "diagnostics", json!({"paths": ["repo/src/lib.rs"]})).unwrap();
        assert_eq!(checked["checked"][0]["diagnostics"][0]["message"], "unused");
        assert_eq!(
            checked["checked"][0]["diagnostics"][0]["severity"],
            "warning"
        );
        assert!(
            index.asked.borrow().is_empty(),
            "the index is not asked when the server answers"
        );
        assert_eq!(
            pool.starts(),
            1,
            "one server for the language, started once and kept"
        );
        assert_eq!(pool.running().len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn without_a_server_weft_answers_and_says_so() {
        let (root, ws) = workspace("weft");
        let pool = Pool::new(quick());
        let index = index_answers();
        let installed = |_: Language| false;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let symbols = run_tool(&cx, "symbols", json!({"path": "repo/src/lib.rs"})).unwrap();
        assert_eq!(symbols["via"], "weft (tree-sitter, pack rust)");
        assert!(
            symbols["server"]
                .as_str()
                .unwrap()
                .starts_with("no server for rust here"),
            "{symbols}"
        );
        assert_eq!(symbols["symbols"][0]["qualified"], "greet");
        let outline = run_tool(&cx, "outline", json!({"path": "repo/src/lib.rs"})).unwrap();
        assert_eq!(outline["outline"].as_array().unwrap().len(), 2);
        // The name at the position is looked up in Weft's index.
        let def = run_tool(
            &cx,
            "definition",
            json!({"path": "repo/src/lib.rs", "line": 6, "column": 6}),
        )
        .unwrap();
        assert_eq!(def["name"], "greet");
        assert_eq!(def["locations"][0]["path"], "repo/src/lib.rs");
        assert!(
            index
                .asked
                .borrow()
                .contains(&"names greet find=false".to_string())
        );
        let checked = run_tool(&cx, "diagnostics", json!({"paths": ["repo/src/lib.rs"]})).unwrap();
        assert_eq!(
            checked["checked"][0]["via"],
            "weft (tree-sitter, pack rust)"
        );
        assert_eq!(pool.starts(), 0, "no server was started");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_server_that_fails_is_answered_by_weft_with_the_reason() {
        let (root, ws) = workspace("failing");
        let refusing = Script {
            refuse: Some("the index is not ready".into()),
            ..Script::default()
        };
        let pool = server_with(refusing, quick());
        let index = index_answers();
        let installed = |_: Language| true;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let def = run_tool(
            &cx,
            "definition",
            json!({"path": "repo/src/lib.rs", "line": 6, "column": 6}),
        )
        .unwrap();
        assert_eq!(def["via"], "weft (tree-sitter index)");
        assert!(
            def["server"]
                .as_str()
                .unwrap()
                .contains("the index is not ready")
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn files_outside_the_lanes_and_paths_outside_the_workspace_are_refused() {
        let (root, ws) = workspace("refused");
        let pool = Pool::new(quick());
        let index = index_answers();
        let installed = |_: Language| false;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let err = call(&cx, "symbols", &json!({"path": "loose/notes.rs"})).unwrap_err();
        assert!(
            err.to_string().contains("no code index for loose/notes.rs"),
            "{err}"
        );
        let escaped = call(&cx, "symbols", &json!({"path": "../etc/hosts"})).unwrap_err();
        assert!(!escaped.to_string().is_empty());
        let missing = call(
            &cx,
            "definition",
            &json!({"path": "repo/src/lib.rs", "line": 0, "column": 1}),
        )
        .unwrap_err();
        assert!(
            matches!(missing, ToolError::InvalidInput { .. }),
            "{missing:?}"
        );
        assert!(call(&cx, "symbols", &json!({})).is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn diagnostics_without_paths_check_the_lanes_changed_files() {
        let (root, ws) = workspace("changed");
        let pool = Pool::new(quick());
        let index = index_answers();
        let installed = |_: Language| false;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let all = run_tool(&cx, "diagnostics", json!({})).unwrap();
        let paths: Vec<&str> = all["checked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["repo/src/lib.rs"]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_identifier_at_a_position_is_the_name_the_cursor_is_on() {
        let text = "let a = greet(x);\nfn é(){ name_2 }\n";
        assert_eq!(ident_at(text, 1, 9).as_deref(), Some("greet"));
        assert_eq!(
            ident_at(text, 1, 14).as_deref(),
            Some("greet"),
            "just after the name"
        );
        assert_eq!(ident_at(text, 1, 1).as_deref(), Some("let"));
        assert_eq!(ident_at(text, 2, 9).as_deref(), Some("name_2"));
        assert_eq!(ident_at(text, 1, 7), None, "an operator names nothing");
        assert_eq!(ident_at(text, 9, 1), None);
    }

    #[test]
    fn a_symbol_query_searches_the_lanes_weft_index() {
        let (root, ws) = workspace("query");
        let pool = Pool::new(quick());
        let index = index_answers();
        let installed = |_: Language| false;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let found = run_tool(&cx, "symbols", json!({"query": "gree"})).unwrap();
        assert_eq!(found["symbols"][0]["path"], "repo/src/lib.rs");
        assert!(
            index
                .asked
                .borrow()
                .contains(&"names gree find=true".to_string())
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_definition_is_answered_by_the_warm_server_a_base_left_running() {
        let (root, ws) = workspace("warm");
        // The base's bridge serves the server on its socket in the workspace's plaza directory, for the workspace root.
        let base_root = ws.weft_root();
        let warm_dir = ws.plaza_dir().join("lsp");
        let socket = apz_lsp::bridge::socket_path(&warm_dir, "rust-analyzer");
        let script = Script {
            definition: vec![Location {
                uri: path_to_uri(&base_root.join("loose").join("notes.rs")),
                range: Range {
                    start: Position {
                        line: 0,
                        character: 3,
                    },
                    end: Position {
                        line: 0,
                        character: 6,
                    },
                },
            }],
            ..Script::default()
        };
        let (transport, _seen) = fake::transport(script);
        let (serve_socket, serve_root) = (socket.clone(), base_root.clone());
        std::thread::spawn(move || {
            let _ = apz_lsp::bridge::serve(
                &serve_socket,
                transport,
                &apz_lsp::servers::RUST_ANALYZER,
                &serve_root,
                std::time::Duration::from_secs(5),
            );
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !socket.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the bridge never listened"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // The harness's pool, as the process builds it: a server it is not told about is launched, and must not be.
        let pool = Pool::with_launch(Timeouts::default(), |_, _| {
            Err(LspError::Protocol("launched".into()))
        })
        .with_warm(&warm_dir, &base_root);
        let index = index_answers();
        let installed = |_: Language| true;
        let cx = Ctx {
            workspace: &ws,
            pool: &pool,
            index: &index,
            installed: &installed,
        };
        let def = run_tool(
            &cx,
            "definition",
            json!({"path": "loose/notes.rs", "line": 1, "column": 4}),
        )
        .unwrap();
        assert_eq!(def["via"], "language server (rust-analyzer)");
        assert_eq!(def["locations"][0]["line"], 1);
        assert_eq!(pool.warmed(), 1);
        assert!(index.asked.borrow().is_empty());
        fs::remove_dir_all(&root).ok();
    }
}
