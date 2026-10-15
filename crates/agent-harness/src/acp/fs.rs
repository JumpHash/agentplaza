//! The engine's `fs/read_text_file` and `fs/write_text_file`, confined to the workspace.
//!
//! ACP names files by absolute path. The engine runs with its cwd inside the workspace
//! (`/workspace/<path>`), so a path it may touch starts with the workspace root. Anything else, a
//! `..` component, a symlink that resolves outside, or the harness's own `.plaza/` is refused: the
//! same rules the file tools follow (`crate::files`). Both methods are checked by the hub before
//! they run (`engine.read`, `engine.edit`); this module only does the confined I/O.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::layout;
use crate::files::{Workspace, create_dirs, refuse_symlink_target, string_arg, write_atomic};

/// The most `fs/read_text_file` returns, in bytes, after `line` and `limit` are applied.
pub const MAX_READ_BYTES: usize = 4 * 1024 * 1024;
/// The most `fs/write_text_file` writes, in bytes.
pub const MAX_WRITE_BYTES: usize = 8 * 1024 * 1024;

/// The workspace-relative label of the absolute `path` an engine names (what the hub's check and the
/// transcript show), or why it is refused.
pub fn label(workspace: &Workspace, path: &str) -> Result<String, String> {
    resolve_absolute(workspace, path).map(|(_, label)| label)
}

/// `fs/read_text_file {sessionId, path, line?, limit?}` → `{"content": …}`. `line` is 1-based and
/// `limit` counts lines; a file that is not UTF-8 or is larger than [`MAX_READ_BYTES`] is refused.
pub fn read_text_file(workspace: &Workspace, params: &Value) -> Result<Value, String> {
    let path = string_arg(params, "path").map_err(|err| err.to_string())?;
    let line = count_arg(params, "line")?;
    let limit = count_arg(params, "limit")?;
    let (file, label) = resolve_absolute(workspace, &path)?;

    let metadata = fs::metadata(&file).map_err(|err| io_error(err, &label))?;
    if metadata.is_dir() {
        return Err(format!("{label} is a directory; this reads one file"));
    }
    // Checked before a byte is read: a file over the cap must not be pulled into memory first.
    if metadata.len() > MAX_READ_BYTES as u64 {
        return Err(format!(
            "{label} is {} bytes, over the {MAX_READ_BYTES}-byte read cap",
            metadata.len()
        ));
    }
    // The file may have grown since its metadata was read, so the read itself is bounded too.
    let mut bytes = Vec::new();
    fs::File::open(&file)
        .and_then(|handle| {
            handle
                .take(MAX_READ_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|err| io_error(err, &label))?;
    if bytes.len() > MAX_READ_BYTES {
        return Err(format!(
            "{label} grew past the {MAX_READ_BYTES}-byte read cap while it was read"
        ));
    }
    let text = String::from_utf8(bytes).map_err(|err| {
        format!(
            "{label} is not UTF-8 text (an invalid sequence at byte {})",
            err.utf8_error().valid_up_to()
        )
    })?;

    if line.is_none() && limit.is_none() {
        return Ok(json!({ "content": text }));
    }
    // Lines keep their own endings, so joining the window gives back the file's bytes for it.
    let skip = line.unwrap_or(1) - 1;
    let take = limit.map_or(usize::MAX, |count| count as usize);
    let content: String = text
        .split_inclusive('\n')
        .skip(skip as usize)
        .take(take)
        .collect();
    Ok(json!({ "content": content }))
}

/// `fs/write_text_file {sessionId, path, content}` → `null`. Creates missing parent folders and
/// writes atomically; refuses content larger than [`MAX_WRITE_BYTES`].
pub fn write_text_file(workspace: &Workspace, params: &Value) -> Result<Value, String> {
    let path = string_arg(params, "path").map_err(|err| err.to_string())?;
    let content = string_arg(params, "content").map_err(|err| err.to_string())?;
    if content.len() > MAX_WRITE_BYTES {
        return Err(format!(
            "the content is {} bytes, over the {MAX_WRITE_BYTES}-byte write cap",
            content.len()
        ));
    }
    let (file, label) = resolve_absolute(workspace, &path)?;
    refuse_symlink_target(&file, &label).map_err(|err| err.to_string())?;
    if file.is_dir() {
        return Err(format!("{label} is a directory; this writes one file"));
    }
    // `resolve` proved the deepest existing ancestor is inside the workspace, so the folders
    // created here are too. The harness writes as root, so what it creates goes to the engine
    // (see `layout`): the engine keeps editing what it asked to write.
    let mut new_top = None;
    if let Some(parent) = file.parent() {
        new_top = layout::first_missing(parent);
        create_dirs(parent)
            .map_err(|err| format!("cannot create the folders of {label}: {err}"))?;
    }
    write_atomic(&file, content.as_bytes())
        .map_err(|err| format!("cannot write {label}: {err}"))?;
    if let Some(top) = new_top {
        layout::give_to_engine(&top)?;
    }
    layout::give_to_engine(&file)?;
    Ok(Value::Null)
}

/// The absolute `path` an engine named, resolved to the file inside the workspace and its label.
///
/// The workspace is matched by its configured root first and by its canonical root second: the
/// engine may name either, and both mean the same folder. The rest goes through the file tools'
/// own resolver, which refuses `..`, the `.plaza` state directory and symlinks that escape.
fn resolve_absolute(workspace: &Workspace, path: &str) -> Result<(PathBuf, String), String> {
    let absolute = Path::new(path);
    if !absolute.is_absolute() {
        return Err(format!("not an absolute path: {path}"));
    }
    let rest = workspace_relative(workspace, absolute)?;
    let rest = rest
        .to_str()
        .ok_or_else(|| format!("not valid UTF-8: {path}"))?;
    workspace.resolve(rest).map_err(|err| err.to_string())
}

/// `path` with the workspace's root removed, or why it is not inside the workspace.
///
/// `strip_prefix` compares whole components, so `/ws2/x` is not inside `/ws`.
fn workspace_relative<'a>(workspace: &Workspace, path: &'a Path) -> Result<&'a Path, String> {
    if let Ok(rest) = path.strip_prefix(workspace.dir()) {
        return Ok(rest);
    }
    // An unmounted workspace has no canonical root either, and then nothing can be inside it.
    workspace
        .canonical_root()
        .ok()
        .and_then(|root| path.strip_prefix(root).ok())
        .ok_or_else(|| format!("outside the workspace: {}", path.display()))
}

/// An optional count argument: `line` (1-based) and `limit` both start at one, so zero is refused.
fn count_arg(params: &Value, key: &str) -> Result<Option<u64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(count) if count >= 1 => Ok(Some(count)),
            _ => Err(format!("{key:?} must be a whole number of at least 1")),
        },
    }
}

/// A filesystem error on a read, in the words the engine sees: a missing file is `not found`.
fn io_error(err: io::Error, label: &str) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => format!("not found: {label}"),
        _ => format!("cannot read {label}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh scratch workspace, and a scratch directory beside it that is outside it.
    fn scratch(name: &str) -> (Workspace, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("apz-acp-fs-{}-{name}-{unique}", std::process::id()));
        let inside = base.join("workspace");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&inside).expect("a scratch workspace");
        fs::create_dir_all(&outside).expect("a scratch directory beside it");
        (Workspace::at(inside), outside)
    }

    /// `path` as the absolute string an engine would send.
    fn abs(path: &Path) -> String {
        path.to_str().expect("a UTF-8 scratch path").to_string()
    }

    #[test]
    fn label_names_a_file_relative_to_the_workspace() {
        let (workspace, _) = scratch("label");
        let root = workspace.dir().to_path_buf();
        assert_eq!(
            label(&workspace, &abs(&root.join("src/main.rs"))),
            Ok("src/main.rs".to_string())
        );
        let canonical = fs::canonicalize(&root).expect("the scratch root resolves");
        assert_eq!(
            label(&workspace, &abs(&canonical.join("a.txt"))),
            Ok("a.txt".to_string())
        );
        assert_eq!(label(&workspace, &abs(&root)), Ok(".".to_string()));
    }

    #[test]
    fn read_returns_the_whole_file() {
        let (workspace, _) = scratch("read-whole");
        let file = workspace.dir().join("notes.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        assert_eq!(
            read_text_file(&workspace, &json!({ "path": abs(&file) })),
            Ok(json!({ "content": "one\ntwo\nthree\n" }))
        );
    }

    #[test]
    fn read_serves_a_line_window_keeping_the_line_endings() {
        let (workspace, _) = scratch("read-window");
        let file = workspace.dir().join("notes.txt");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let path = abs(&file);
        let window = |extra: Value| {
            let mut params = json!({ "path": path });
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            read_text_file(&workspace, &params)
        };
        assert_eq!(
            window(json!({ "line": 2, "limit": 1 })),
            Ok(json!({ "content": "two\n" }))
        );
        assert_eq!(
            window(json!({ "line": 2 })),
            Ok(json!({ "content": "two\nthree" }))
        );
        assert_eq!(
            window(json!({ "limit": 2 })),
            Ok(json!({ "content": "one\ntwo\n" }))
        );
        assert_eq!(window(json!({ "line": 9 })), Ok(json!({ "content": "" })));
        assert!(window(json!({ "line": 0 })).is_err());
    }

    #[test]
    fn a_relative_path_is_refused() {
        let (workspace, _) = scratch("relative");
        let err = read_text_file(&workspace, &json!({ "path": "notes.txt" })).unwrap_err();
        assert!(err.contains("not an absolute path"), "{err}");
    }

    #[test]
    fn a_path_outside_the_workspace_is_refused() {
        let (workspace, outside) = scratch("outside");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "outside").unwrap();
        for path in ["/etc/passwd".to_string(), abs(&secret)] {
            let err = read_text_file(&workspace, &json!({ "path": path.clone() })).unwrap_err();
            assert!(err.starts_with("outside the workspace: "), "{err}");
            let err =
                write_text_file(&workspace, &json!({ "path": path, "content": "x" })).unwrap_err();
            assert!(err.starts_with("outside the workspace: "), "{err}");
        }
        assert_eq!(fs::read_to_string(&secret).unwrap(), "outside");
    }

    #[test]
    fn a_dot_dot_that_climbs_out_is_refused() {
        let (workspace, outside) = scratch("dotdot");
        fs::write(outside.join("secret.txt"), "outside").unwrap();
        let climb = format!("{}/../outside/secret.txt", workspace.dir().display());
        let err = read_text_file(&workspace, &json!({ "path": climb })).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn the_harness_state_directory_is_refused() {
        let (workspace, _) = scratch("plaza");
        let state = workspace.plaza_dir();
        fs::create_dir_all(&state).unwrap();
        let file = state.join("threads.json");
        fs::write(&file, "{}").unwrap();
        let err = read_text_file(&workspace, &json!({ "path": abs(&file) })).unwrap_err();
        assert!(err.contains("own state"), "{err}");
        assert!(
            write_text_file(&workspace, &json!({ "path": abs(&file), "content": "x" })).is_err()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "{}");
    }

    #[test]
    fn a_symlink_that_leaves_the_workspace_is_refused() {
        let (workspace, outside) = scratch("symlink");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "outside").unwrap();
        let link = workspace.dir().join("link.txt");
        symlink(&secret, &link).unwrap();
        let err = read_text_file(&workspace, &json!({ "path": abs(&link) })).unwrap_err();
        assert!(err.contains("outside the workspace"), "{err}");
        assert!(
            write_text_file(&workspace, &json!({ "path": abs(&link), "content": "x" })).is_err()
        );
        assert_eq!(fs::read_to_string(&secret).unwrap(), "outside");
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let (workspace, _) = scratch("missing");
        let file = workspace.dir().join("missing.txt");
        assert_eq!(
            read_text_file(&workspace, &json!({ "path": abs(&file) })),
            Err("not found: missing.txt".to_string())
        );
    }

    #[test]
    fn write_creates_the_folders_and_round_trips() {
        let (workspace, _) = scratch("write");
        let file = workspace.dir().join("deep/er/new.txt");
        assert_eq!(
            write_text_file(
                &workspace,
                &json!({ "path": abs(&file), "content": "hello\n" })
            ),
            Ok(Value::Null)
        );
        assert_eq!(
            read_text_file(&workspace, &json!({ "path": abs(&file) })),
            Ok(json!({ "content": "hello\n" }))
        );
    }

    #[test]
    fn an_oversize_write_is_refused_and_leaves_no_file() {
        let (workspace, _) = scratch("oversize-write");
        let file = workspace.dir().join("big.txt");
        let content = "a".repeat(MAX_WRITE_BYTES + 1);
        let err = write_text_file(
            &workspace,
            &json!({ "path": abs(&file), "content": content }),
        )
        .unwrap_err();
        assert!(err.contains("write cap"), "{err}");
        assert!(!file.exists());
    }

    #[test]
    fn a_read_over_the_cap_is_refused() {
        let (workspace, _) = scratch("oversize-read");
        let file = workspace.dir().join("big.txt");
        fs::write(&file, vec![b'a'; MAX_READ_BYTES + 1]).unwrap();
        let err = read_text_file(&workspace, &json!({ "path": abs(&file) })).unwrap_err();
        assert!(err.contains("read cap"), "{err}");
    }

    #[test]
    fn a_non_utf8_file_is_refused() {
        let (workspace, _) = scratch("binary");
        let file = workspace.dir().join("blob.bin");
        fs::write(&file, [0xff, 0xfe, b'a']).unwrap();
        let err = read_text_file(&workspace, &json!({ "path": abs(&file) })).unwrap_err();
        assert!(err.contains("not UTF-8"), "{err}");
    }
}
