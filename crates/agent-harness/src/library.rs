//! Agent-made tools: `tool_package` bundles a tool the agent built in its workspace (its
//! files, command, schema and needs) for the plaza's library, and [`run`] unpacks and runs
//! a library program apid sends with a tool decision — always inside this VM.
//!
//! A tool's `dir` is read from the workspace: regular files only. Symlinks, `.git`,
//! `node_modules`, `__pycache__`, `.venv` and `.plaza` are skipped; at most 50 files and
//! 512 KiB in all. Its `hash` is SHA-256 over the files, each as its path, a NUL, its
//! length as 8 big-endian bytes and its bytes, in path order.
//!
//! [`run`] unpacks a program once into `<workspace>/.plaza/tools/<name>@<version>-<hash[..12]>/`
//! (staged, then renamed into place) and installs its `requires` with apk at that point.
//! The command runs there as the agent user with the call's input JSON on **stdin**, and
//! the same input is also written to a per-call file inside that directory whose absolute
//! path is in the env var **`TOOL_INPUT`**. The file is removed when the run ends.
//! Stdout is the tool's output; a failure is its output as `Err`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_proto::wire::Program;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::bash::{Privilege, execute, install_packages};
use crate::files::{PLAZA_DIR, Workspace};

/// The most files a packaged tool holds.
const MAX_FILES: usize = 50;
/// The most bytes a packaged tool holds, in all: 512 KiB.
const MAX_TOTAL_BYTES: u64 = 512 * 1024;
/// The most argv strings a tool's command holds.
const MAX_COMMAND_ARGS: usize = 10;
/// The longest description, in characters.
const MAX_DESCRIPTION_CHARS: usize = 500;
/// Entry names a package walk never takes.
const SKIPPED_NAMES: [&str; 4] = [".git", "node_modules", "__pycache__", ".venv"];
/// A library program's timeout bounds, in seconds.
const MIN_TIMEOUT_S: u64 = 5;
const MAX_TIMEOUT_S: u64 = 600;
/// The shell line a program runs under. `$0` is the input file's absolute path: it is
/// exported as `TOOL_INPUT` and fed to the command on stdin; `$@` is the command.
const RUN_SCRIPT: &str = "TOOL_INPUT=\"$0\"; export TOOL_INPUT; exec \"$@\" < \"$0\"";
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// Distinguishes the staging directories and input files of concurrent calls.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// One file read from a tool's directory.
struct Collected {
    path: String,
    bytes: Vec<u8>,
    executable: bool,
}

/// `deliver_file`: uploads a workspace file to the plaza's Library (through the egress
/// proxy to plaza.gateway, which the hub relays to apid) and returns its id and URL.
pub fn deliver(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("deliver_file is not available yet".to_string())
}

/// `tool_package`: reads the tool's directory and returns its bundle as JSON (apid files it
/// in the library when the call is reported).
///
/// Input: `name` (`^[a-z][a-z0-9_]{2,39}$`), `description` (1 to 500 characters), `dir`
/// (relative, inside the workspace, not `.plaza`), `command` (1 to 10 non-empty strings),
/// `input_schema` (an object), and optionally `requires` (apk names) and `publish` (bool).
///
/// Output: `{"package": {name, description, command, input_schema, requires, publish,
/// hash, files: [{path, content_b64, executable}]}}`, the files sorted by path.
pub fn package(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let name = str_field(input, "name")?;
    if !name_ok(name) {
        return Err(invalid(format!(
            "{name:?} is not a tool name; use 3 to 40 lowercase letters, digits and underscores, starting with a letter"
        )));
    }
    let description = str_field(input, "description")?;
    let described = description.chars().count();
    if described == 0 || described > MAX_DESCRIPTION_CHARS {
        return Err(invalid(format!(
            "\"description\" is {described} characters; give 1 to {MAX_DESCRIPTION_CHARS}"
        )));
    }
    let rel = str_field(input, "dir")?;
    let dir = tool_dir(workspace, rel)?;
    let command = strings(input, "command")?;
    if command.is_empty() || command.len() > MAX_COMMAND_ARGS {
        return Err(invalid(format!(
            "\"command\" has {} strings; give 1 to {MAX_COMMAND_ARGS}",
            command.len()
        )));
    }
    let schema = match input.get("input_schema") {
        Some(schema) if schema.is_object() => schema.clone(),
        _ => return Err(invalid("\"input_schema\" must be an object")),
    };
    let requires = match input.get("requires") {
        None | Some(Value::Null) => Vec::new(),
        Some(_) => strings(input, "requires")?,
    };
    let publish = match input.get("publish") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(invalid("\"publish\" must be true or false")),
    };

    let mut files = Vec::new();
    let mut total = 0;
    collect(&dir, "", &mut files, &mut total)?;
    if files.is_empty() {
        return Err(invalid(format!("{rel:?} holds no files to package")));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let pairs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|file| (file.path.as_str(), file.bytes.as_slice()))
        .collect();
    let hash = bundle_hash(&pairs);
    let listed: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "content_b64": base64_encode(&file.bytes),
                "executable": file.executable,
            })
        })
        .collect();
    Ok(json!({
        "package": {
            "name": name,
            "description": description,
            "command": command,
            "input_schema": schema,
            "requires": requires,
            "publish": publish,
            "hash": hash,
            "files": listed,
        }
    })
    .to_string())
}

/// Runs a library program with the call's input; its stdout is the tool's output.
///
/// The program's files are checked against its hash, unpacked on first use, and run with
/// `command` in the unpacked directory, the input on stdin and in `TOOL_INPUT`.
pub fn run(workspace: &Workspace, program: &Program, input: &Value) -> Result<String, String> {
    if !name_ok(&program.name) {
        return Err(format!("the library program's name {:?} is not a tool name", program.name));
    }
    if program.command.is_empty() {
        return Err("the library program has no command".to_string());
    }
    if program.hash.len() < 12 || !program.hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the library program's hash is not a hex digest".to_string());
    }
    let mut files = Vec::with_capacity(program.files.len());
    for file in &program.files {
        if !file_path_ok(&file.path) {
            return Err(format!(
                "the library program's file {:?} is not a path inside its directory",
                file.path
            ));
        }
        let bytes = base64_decode(&file.content_b64)
            .map_err(|err| format!("the library program's file {:?}: {err}", file.path))?;
        files.push((file.path.clone(), bytes, file.executable));
    }
    if files.is_empty() {
        return Err("the library program has no files".to_string());
    }
    let pairs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes, _)| (path.as_str(), bytes.as_slice()))
        .collect();
    if bundle_hash(&pairs) != program.hash {
        return Err("the library program's files do not match its hash".to_string());
    }

    let dir = workspace.plaza_dir().join("tools").join(format!(
        "{}@{}-{}",
        program.name,
        program.version,
        &program.hash[..12]
    ));
    if !dir.is_dir() {
        unpack(workspace, &dir, program, &files)?;
    }

    let timeout_s = program.timeout_s.clamp(MIN_TIMEOUT_S, MAX_TIMEOUT_S);
    let input_path = dir.join(format!(
        ".input-{}-{}.json",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&input_path, input.to_string())
        .map_err(|err| format!("cannot write the tool's input: {err}"))?;
    let input_arg = input_path.to_string_lossy().into_owned();
    let mut args = vec!["-c", RUN_SCRIPT, input_arg.as_str()];
    args.extend(program.command.iter().map(String::as_str));
    let result = execute(&dir, "/bin/sh", &args, timeout_s, Privilege::Agent);
    let _ = fs::remove_file(&input_path);
    result
}

/// Writes a program's files into a staging directory, renames it to `dir` and installs
/// the program's `requires`. A failed install removes the directory, so the next call
/// unpacks and installs again.
fn unpack(
    workspace: &Workspace,
    dir: &Path,
    program: &Program,
    files: &[(String, Vec<u8>, bool)],
) -> Result<(), String> {
    let tools = dir
        .parent()
        .ok_or_else(|| "the tool directory has no parent".to_string())?;
    fs::create_dir_all(tools).map_err(|err| format!("cannot create {}: {err}", tools.display()))?;
    let staging = tools.join(format!(
        ".unpacking-{}-{}",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(err) = write_tree(&staging, files).and_then(|()| give_to_agent(&staging)) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err);
    }
    if let Err(err) = fs::rename(&staging, dir) {
        let _ = fs::remove_dir_all(&staging);
        // Another call unpacked the same program first; its copy is the one to run.
        if dir.is_dir() {
            return Ok(());
        }
        return Err(format!("cannot unpack {}: {err}", dir.display()));
    }
    if !program.requires.is_empty() {
        if let Err(err) = install_packages(workspace, &json!({ "packages": program.requires })) {
            if !err.contains("already installed") {
                let _ = fs::remove_dir_all(dir);
                return Err(format!("cannot install the tool's requirements: {err}"));
            }
        }
    }
    Ok(())
}

/// Writes each file under `root`, creating its folders, with mode 0755 when executable
/// and 0644 otherwise.
fn write_tree(root: &Path, files: &[(String, Vec<u8>, bool)]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|err| format!("cannot create {}: {err}", root.display()))?;
    for (rel, bytes, executable) in files {
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
        }
        fs::write(&target, bytes).map_err(|err| format!("cannot write {rel}: {err}"))?;
        let mode = if *executable { 0o755 } else { 0o644 };
        fs::set_permissions(&target, fs::Permissions::from_mode(mode))
            .map_err(|err| format!("cannot set the mode of {rel}: {err}"))?;
    }
    Ok(())
}

/// Reads the regular files under `dir` into `files`, `prefix` being the path of `dir`
/// relative to the tool. Refuses past the file and byte caps before reading the file
/// that would exceed them.
fn collect(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<Collected>,
    total: &mut u64,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("a name under {} is not UTF-8", dir.display()))?;
        if name == PLAZA_DIR || SKIPPED_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_dir() {
            collect(&path, &rel, files, total)?;
        } else if kind.is_file() {
            if files.len() == MAX_FILES {
                return Err(invalid(format!(
                    "the tool holds more than {MAX_FILES} files; a library tool takes at most {MAX_FILES}"
                )));
            }
            *total += meta.len();
            if *total > MAX_TOTAL_BYTES {
                return Err(invalid(format!(
                    "the tool is over the {} KiB size cap",
                    MAX_TOTAL_BYTES / 1024
                )));
            }
            let bytes = fs::read(&path).map_err(|err| format!("cannot read {rel}: {err}"))?;
            files.push(Collected {
                path: rel,
                bytes,
                executable: meta.permissions().mode() & 0o111 != 0,
            });
        }
    }
    Ok(())
}

/// The workspace directory a package reads: relative, inside the workspace, not the
/// harness's own state, and a real directory once symlinks are followed.
fn tool_dir(workspace: &Workspace, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Err(invalid("\"dir\" must name a directory inside the workspace"));
    }
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) if part == PLAZA_DIR => {
                return Err(invalid(format!(
                    "{rel:?} is the harness's own state, which a tool cannot package"
                )));
            }
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(invalid(format!("{rel:?} must be a path inside the workspace"))),
        }
    }
    let root = fs::canonicalize(workspace.dir())
        .map_err(|err| format!("the workspace is not available: {err}"))?;
    let dir = match fs::canonicalize(root.join(rel)) {
        Ok(dir) if dir.is_dir() => dir,
        _ => return Err(invalid(format!("{rel:?} is not a directory in the workspace"))),
    };
    if !dir.starts_with(&root) || dir.starts_with(root.join(PLAZA_DIR)) {
        return Err(invalid(format!(
            "{rel:?} resolves outside the workspace or into its own state"
        )));
    }
    Ok(dir)
}

/// SHA-256 over `files` in path order, each as its path, a NUL, its length as 8
/// big-endian bytes and its bytes. The hex digest is the program's `hash`.
fn bundle_hash(files: &[(&str, &[u8])]) -> String {
    let mut sorted = files.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut hasher = Sha256::new();
    for (path, bytes) in sorted {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A tool name: 3 to 40 characters of `[a-z][a-z0-9_]*`.
fn name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (3..=40).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// A program file's path: relative, with normal components only.
fn file_path_ok(rel: &str) -> bool {
    !rel.is_empty() && Path::new(rel).components().all(|part| matches!(part, Component::Normal(_)))
}

fn invalid(reason: impl std::fmt::Display) -> String {
    format!("invalid input: {reason}")
}

fn str_field<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{key:?} must be a string")))
}

fn strings(input: &Value, key: &str) -> Result<Vec<String>, String> {
    let items = input
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("{key:?} must be a list of strings")))?;
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(text) if !text.is_empty() => Ok(text.to_string()),
            _ => Err(invalid(format!("{key:?} must be a list of non-empty strings"))),
        })
        .collect()
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (first << 16) | (second << 8) | third;
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { BASE64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { BASE64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let bad = || "the content is not valid base64".to_string();
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(bad());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for quad in bytes.chunks(4) {
        let pad = quad.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 {
            return Err(bad());
        }
        let mut n: u32 = 0;
        for (index, &byte) in quad.iter().enumerate() {
            let value = if byte == b'=' {
                if index < 4 - pad {
                    return Err(bad());
                }
                0
            } else {
                u32::from(sextet_of(byte).ok_or_else(bad)?)
            };
            n = (n << 6) | value;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

fn sextet_of(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_proto::wire::ProgramFile;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-harness-library-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
        fs::write(path, bytes).expect("a file");
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("a mode");
    }

    /// A tool directory `tool` with `main.sh` (the given script, executable) and `sub/data.txt`.
    fn echo_tool(ws: &Workspace, script: &[u8]) {
        let tool = ws.dir().join("tool");
        write(&tool.join("main.sh"), script);
        set_mode(&tool.join("main.sh"), 0o755);
        write(&tool.join("sub/data.txt"), b"hello");
    }

    fn input(dir: &str) -> Value {
        json!({
            "name": "echo_tool",
            "description": "echoes its input",
            "dir": dir,
            "command": ["/bin/sh", "main.sh"],
            "input_schema": {"type": "object"},
        })
    }

    /// The library program apid would send for `package`'s output.
    fn program_from(packaged: &str) -> Program {
        let value: Value = serde_json::from_str(packaged).expect("json");
        let pkg = &value["package"];
        Program {
            name: pkg["name"].as_str().expect("name").to_string(),
            version: 1,
            hash: pkg["hash"].as_str().expect("hash").to_string(),
            files: pkg["files"]
                .as_array()
                .expect("files")
                .iter()
                .map(|file| ProgramFile {
                    path: file["path"].as_str().expect("path").to_string(),
                    content_b64: file["content_b64"].as_str().expect("content").to_string(),
                    executable: file["executable"].as_bool().expect("executable"),
                })
                .collect(),
            command: pkg["command"]
                .as_array()
                .expect("command")
                .iter()
                .map(|arg| arg.as_str().expect("arg").to_string())
                .collect(),
            requires: Vec::new(),
            timeout_s: 10,
        }
    }

    #[test]
    fn a_tool_packages_its_files_hash_and_executable_bits() {
        let ws = scratch("package");
        echo_tool(&ws, b"cat\n");
        write(&ws.dir().join("tool/.git/config"), b"x");
        write(&ws.dir().join("tool/node_modules/pkg/index.js"), b"x");

        let out = package(&ws, &input("tool")).expect("packages");
        let value: Value = serde_json::from_str(&out).expect("json");
        let pkg = &value["package"];
        assert_eq!(pkg["name"], "echo_tool");
        assert_eq!(pkg["publish"], false);
        assert_eq!(pkg["requires"], json!([]));
        let files = pkg["files"].as_array().expect("files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "main.sh");
        assert_eq!(files[0]["executable"], true);
        assert_eq!(files[1]["path"], "sub/data.txt");
        assert_eq!(files[1]["executable"], false);
        assert_eq!(base64_decode(files[1]["content_b64"].as_str().expect("content")).expect("base64"), b"hello");
        let expected = bundle_hash(&[("main.sh", &b"cat\n"[..]), ("sub/data.txt", &b"hello"[..])]);
        assert_eq!(pkg["hash"], expected.as_str());
    }

    #[test]
    fn run_unpacks_the_program_and_feeds_it_its_input() {
        let ws = scratch("run");
        echo_tool(&ws, b"cat\necho\ntest -f \"$TOOL_INPUT\" && echo has-input\n");
        let program = program_from(&package(&ws, &input("tool")).expect("packages"));

        let output = run(&ws, &program, &json!({"text": "hi"})).expect("exit 0");
        assert_eq!(output, "{\"text\":\"hi\"}\nhas-input\nexit 0");

        let dir = ws
            .plaza_dir()
            .join("tools")
            .join(format!("echo_tool@1-{}", &program.hash[..12]));
        assert_eq!(fs::read(dir.join("sub/data.txt")).expect("unpacked"), b"hello");
        let mode = fs::metadata(dir.join("main.sh")).expect("unpacked").permissions().mode();
        assert!(mode & 0o111 != 0, "{mode:o}");
        let leftovers = fs::read_dir(&dir)
            .expect("the tool directory")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(".input-"));
        assert!(!leftovers, "the input file is removed after the run");
    }

    #[test]
    fn run_refuses_a_program_whose_files_do_not_match_its_hash() {
        let ws = scratch("tamper");
        echo_tool(&ws, b"cat\n");
        let mut program = program_from(&package(&ws, &input("tool")).expect("packages"));
        program.hash = "0".repeat(64);
        let err = run(&ws, &program, &json!({})).expect_err("hash mismatch");
        assert!(err.contains("hash"), "{err}");
    }

    #[test]
    fn packaging_refuses_more_than_fifty_files() {
        let ws = scratch("count");
        for index in 0..=MAX_FILES {
            write(&ws.dir().join("tool").join(format!("f{index:02}.txt")), b"x");
        }
        let err = package(&ws, &input("tool")).expect_err("too many files");
        assert!(err.contains("more than 50 files"), "{err}");
    }

    #[test]
    fn packaging_refuses_more_than_512_kib() {
        let ws = scratch("size");
        write(&ws.dir().join("tool/big.bin"), &vec![7u8; 512 * 1024 + 1]);
        let err = package(&ws, &input("tool")).expect_err("too big");
        assert!(err.contains("512 KiB"), "{err}");
    }

    #[test]
    fn packaging_names_what_is_wrong_with_the_input() {
        let ws = scratch("bad");
        write(&ws.dir().join("tool/a.txt"), b"x");
        let long = "a".repeat(41);
        for name in ["Bad", "ab", "1abc", "a-b-c", long.as_str()] {
            let mut bad = input("tool");
            bad["name"] = json!(name);
            let err = package(&ws, &bad).expect_err(name);
            assert!(err.contains("tool name"), "{err}");
        }

        let mut empty = input("tool");
        empty["description"] = json!("");
        assert!(package(&ws, &empty).expect_err("empty").contains("description"));
        let mut long_description = input("tool");
        long_description["description"] = json!("d".repeat(501));
        assert!(package(&ws, &long_description).expect_err("long").contains("description"));

        assert!(package(&ws, &input("nope")).expect_err("missing").contains("not a directory"));
        assert!(package(&ws, &input("../tool")).expect_err("escape").contains("inside the workspace"));
        assert!(package(&ws, &input(".plaza/tools")).is_err());

        let mut no_command = input("tool");
        no_command["command"] = json!([]);
        assert!(package(&ws, &no_command).expect_err("no command").contains("command"));
        let mut no_schema = input("tool");
        no_schema["input_schema"] = json!("object");
        assert!(package(&ws, &no_schema).expect_err("schema").contains("input_schema"));
    }

    #[test]
    fn packaging_refuses_a_directory_with_no_files() {
        let ws = scratch("empty");
        write(&ws.dir().join("tool/.git/config"), b"x");
        let err = package(&ws, &input("tool")).expect_err("no files");
        assert!(err.contains("no files"), "{err}");
    }

    #[test]
    fn base64_round_trips_with_padding() {
        for (plain, text) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg==")] {
            assert_eq!(base64_encode(plain.as_bytes()), text);
            assert_eq!(base64_decode(text).expect("decodes"), plain.as_bytes());
        }
        assert!(base64_decode("Zm9").is_err());
        assert!(base64_decode("Z!9v").is_err());
    }
}

/// In a root guest, hands the unpacked tree to the agent user (uid 1000), so a tool that
/// writes beside itself works when it runs as the agent.
fn give_to_agent(dir: &Path) -> Result<(), String> {
    if !crate::bash::runs_as_root().unwrap_or(false) {
        return Ok(());
    }
    let id = crate::bash::UNPRIVILEGED;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        std::os::unix::fs::lchown(&path, Some(id), Some(id)).map_err(|e| format!("cannot hand {} to the agent: {e}", path.display()))?;
        if path.is_dir() {
            for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                stack.push(entry.map_err(|e| e.to_string())?.path());
            }
        }
    }
    Ok(())
}

/// What the model hears after `tool_package`: the tool's name, file count, size and hash.
pub fn summary(bundle: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(bundle) else {
        return "packaged".to_string();
    };
    let p = &value["package"];
    let files = p["files"].as_array().map_or(0, Vec::len);
    let bytes: usize = p["files"].as_array().map_or(0, |f| f.iter().filter_map(|x| x["content_b64"].as_str()).map(|s| s.len() * 3 / 4).sum());
    format!(
        "packaged x_{} ({files} files, ~{} KiB, hash {}) for the plaza's library{}",
        p["name"].as_str().unwrap_or("?"),
        bytes.div_ceil(1024),
        p["hash"].as_str().map_or("?", |h| &h[..h.len().min(12)]),
        if p["publish"].as_bool() == Some(true) { "; publishing to every agent awaits review" } else { "; it is yours to use now as a tool" },
    )
}
