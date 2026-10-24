//! Code mode (`run_script`): a short script runs in the workspace and calls the session's tools as functions. Only the
//! value it sets with `result(value)` comes back to the model. Each call it makes is decided as a model's call is (see
//! `agent.rs`), and reported under the script call it belongs to.
//!
//! The harness writes the script and a client library into a run directory of its own, opens a Unix socket in it, and
//! runs the interpreter as the session's user in the workspace. A tool call the script makes connects to the socket,
//! sends one JSON line `{name, input}`, and reads one JSON line `{ok, output}` back, once the harness has decided the
//! call. Python runs as `python3`; TypeScript and JavaScript run as `node`, TypeScript through node's type stripping
//! (node 22.6 or later). The library is generated from the session's tools, deferred ones included: a deferred tool the
//! model has not fetched is refused in a script as it is in a model's call.
//!
//! Extension point: a provider's programmatic tool calling (a model that emits code calling our tools) would map onto
//! [`Script`] and [`Session`]; nothing here depends on the model's API.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use apz_proto::chat::ToolSpec;
use apz_proto::wire::cap_output;
use serde_json::{Value, json};

use crate::bash::{self, Privilege};
use crate::files::Workspace;

/// The tool the model calls to run a script.
pub const TOOL: &str = "run_script";
/// How long a script may run when its call does not say.
pub const DEFAULT_TIMEOUT_S: u64 = 120;
/// The longest a script may run.
pub const MAX_TIMEOUT_S: u64 = 600;
/// The tool calls one script may make.
pub const MAX_CALLS: usize = 50;
/// The most the return value keeps; a longer one is cut, and the cut is marked.
pub const MAX_RESULT_BYTES: usize = 16 << 10;
/// The most a script may print (stdout and stderr together) before it is stopped.
pub const MAX_OUTPUT_BYTES: u64 = 256 << 10;
/// The end of a failed script's output that its error keeps.
const ERROR_TAIL_BYTES: usize = 4 << 10;
/// How long a call's line may take to arrive once its connection is open.
const LINE_WAIT: Duration = Duration::from_secs(30);
/// The first node version with type stripping (node 22.6; on by default from 23.6, on the flag before).
const TYPES_FROM_NODE: (u32, u32) = (22, 6);

/// The run directories this process has made, for their names.
static RUNS: AtomicU64 = AtomicU64::new(0);

/// The words a generated function may not take: the library's own names and the reserved words of the languages.
const RESERVED: &[&str] = &[
    "call",
    "result",
    "PlazaError",
    "plaza",
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "def",
    "default",
    "del",
    "delete",
    "do",
    "elif",
    "else",
    "enum",
    "eval",
    "except",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "global",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "is",
    "lambda",
    "let",
    "new",
    "nonlocal",
    "none",
    "not",
    "null",
    "or",
    "package",
    "pass",
    "private",
    "protected",
    "public",
    "raise",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// The languages a script may be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Python,
    TypeScript,
    JavaScript,
}

impl Language {
    fn named(name: &str) -> Option<Language> {
        match name {
            "python" => Some(Language::Python),
            "typescript" => Some(Language::TypeScript),
            "javascript" => Some(Language::JavaScript),
            _ => None,
        }
    }
}

/// A script as the model asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub language: Language,
    pub code: String,
    pub timeout_s: u64,
}

impl Script {
    /// The script `run_script`'s input asks for. A refusal names the input's fault.
    pub fn parse(input: &Value) -> Result<Script, String> {
        let language = input
            .get("language")
            .and_then(Value::as_str)
            .and_then(Language::named)
            .ok_or_else(|| {
                "INVALID_INPUT: language must be \"python\", \"typescript\" or \"javascript\""
                    .to_string()
            })?;
        let code = match input.get("code") {
            Some(Value::String(code)) if !code.trim().is_empty() => code.clone(),
            _ => return Err("INVALID_INPUT: code must be a non-empty string".to_string()),
        };
        let timeout_s = match input.get("timeout_s") {
            None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
            Some(value) => value
                .as_u64()
                .filter(|seconds| (1..=MAX_TIMEOUT_S).contains(seconds))
                .ok_or_else(|| {
                    format!("INVALID_INPUT: timeout_s must be a whole number of seconds from 1 to {MAX_TIMEOUT_S}")
                })?,
        };
        Ok(Script {
            language,
            code,
            timeout_s,
        })
    }
}

/// The identifier a tool's function takes in the library: its name with the characters an identifier cannot hold
/// replaced, a leading underscore before a digit, and a trailing one on a reserved word.
fn identifier(name: &str) -> String {
    let mut id: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if id.is_empty() || id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert(0, '_');
    }
    if RESERVED.contains(&id.as_str()) {
        id.push('_');
    }
    id
}

/// The tools the library offers as functions, with their identifiers, in the session's order. `run_script` is not one (a
/// script does not run a script), and a tool whose identifier an earlier tool already took is not offered.
fn functions(tools: &[ToolSpec]) -> Vec<(String, &ToolSpec)> {
    let mut taken = BTreeSet::new();
    tools
        .iter()
        .filter(|tool| tool.name != TOOL)
        .filter_map(|tool| {
            let id = identifier(&tool.name);
            taken.insert(id.clone()).then_some((id, tool))
        })
        .collect()
}

/// A tool's name as a string literal, which is also a valid literal in Python and JavaScript.
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// The text a tool's function documents itself with: its description, then its input schema.
fn describe(tool: &ToolSpec) -> String {
    format!("{}\n\nInput: {}", tool.description, tool.input_schema)
}

/// A `/** */` comment holding `text`, with no `*/` inside it.
fn comment(text: &str) -> String {
    let body = text.replace("*/", "*\\/");
    let mut out = String::from("/**\n");
    for line in body.lines() {
        if line.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(&format!(" * {line}\n"));
        }
    }
    out.push_str(" */\n");
    out
}

/// The Python library, `plaza.py`: `call`, `result`, `PlazaError`, and one function per tool.
pub fn python_library(tools: &[ToolSpec]) -> String {
    let mut out = String::from(PYTHON_HEAD);
    let mut names = vec![
        "call".to_string(),
        "result".to_string(),
        "PlazaError".to_string(),
    ];
    for (id, tool) in functions(tools) {
        let doc = describe(tool)
            .replace('\\', "\\\\")
            .replace("\"\"\"", "\\\"\\\"\\\"");
        out.push_str(&format!(
            "\n\ndef {id}(input=None):\n    \"\"\"{doc}\"\"\"\n    return call({}, input)\n",
            literal(&tool.name)
        ));
        names.push(id);
    }
    let listed: Vec<String> = names.iter().map(|name| literal(name.as_str())).collect();
    out.push_str(&format!("\n\n__all__ = [{}]\n", listed.join(", ")));
    out
}

const PYTHON_HEAD: &str = r#""""The client library of run_script: each tool is a function, and result(value) sets what the script returns."""
import json
import os
import socket


class PlazaError(Exception):
    """A tool call the plaza refused or that failed; the message is the plaza's text."""


def call(name, input=None):
    """Calls the tool `name` with `input` (a dict); returns its text output, and raises PlazaError when it fails."""
    request = json.dumps({"name": name, "input": {} if input is None else input}) + "\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as link:
        link.connect(os.environ["PLAZA_SOCKET"])
        link.sendall(request.encode("utf-8"))
        reply = b""
        while True:
            chunk = link.recv(65536)
            if not chunk:
                break
            reply += chunk
    if not reply:
        raise PlazaError("the harness closed the call without an answer")
    answer = json.loads(reply.decode("utf-8"))
    if not answer["ok"]:
        raise PlazaError(answer["output"])
    return answer["output"]


def result(value=None):
    """Sets the value the script returns to the model; the last call wins."""
    path = os.environ["PLAZA_RESULT"]
    with open(path + ".tmp", "w", encoding="utf-8") as out:
        json.dump({"value": value}, out)
    os.replace(path + ".tmp", path)
"#;

/// The Python runner, `runner.py`: runs the script at `argv[1]` with the library's names in scope, so the script's own
/// line numbers are the ones its errors name.
const PYTHON_RUNNER: &str = r#"import sys

import plaza

path = sys.argv[1]
with open(path, encoding="utf-8") as script:
    source = script.read()
scope = {"__name__": "__main__", "__file__": path}
scope.update({name: getattr(plaza, name) for name in plaza.__all__})
exec(compile(source, path, "exec"), scope)
"#;

/// The JavaScript or TypeScript library: `call`, `result`, `PlazaError`, and one function per tool. TypeScript gets the
/// declarations of each tool's input, from its schema; node strips them when it runs the file.
fn script_library(tools: &[ToolSpec], typed: bool) -> String {
    let mut out = String::new();
    out.push_str("// The client library of run_script: each tool is a function, and result(value) sets what the script returns.\n");
    out.push_str("import net from \"node:net\";\n");
    out.push_str("import { renameSync, writeFileSync } from \"node:fs\";\n\n");
    out.push_str("export class PlazaError extends Error {}\n\n");
    let (name_type, input_type, reply_type, socket, value_type) = if typed {
        (
            ": string",
            "input?: object",
            ": Promise<string>",
            "process.env.PLAZA_SOCKET as string",
            ": unknown",
        )
    } else {
        ("", "input", "", "process.env.PLAZA_SOCKET", "")
    };
    out.push_str(&format!(
        "/** Calls the tool `name` with `input`; resolves to its text output, and rejects with a PlazaError when it fails. */\n\
         export function call(name{name_type}, {input_type}){reply_type} {{\n\
         \x20 return new Promise((resolve, reject) => {{\n\
         \x20   const link = net.createConnection({socket});\n\
         \x20   let reply = \"\";\n\
         \x20   link.setEncoding(\"utf8\");\n\
         \x20   link.on(\"connect\", () => link.write(JSON.stringify({{ name, input: input ?? {{}} }}) + \"\\n\"));\n\
         \x20   link.on(\"data\", (chunk) => {{ reply += chunk; }});\n\
         \x20   link.on(\"error\", reject);\n\
         \x20   link.on(\"end\", () => {{\n\
         \x20     if (!reply) return reject(new PlazaError(\"the harness closed the call without an answer\"));\n\
         \x20     const answer = JSON.parse(reply);\n\
         \x20     if (answer.ok) resolve(answer.output);\n\
         \x20     else reject(new PlazaError(answer.output));\n\
         \x20   }});\n\
         \x20 }});\n\
         }}\n\n"
    ));
    out.push_str(&format!(
        "/** Sets the value the script returns to the model; the last call wins. */\n\
         export function result(value{value_type}){} {{\n\
         \x20 const path = process.env.PLAZA_RESULT{};\n\
         \x20 writeFileSync(path + \".tmp\", JSON.stringify({{ value: value ?? null }}));\n\
         \x20 renameSync(path + \".tmp\", path);\n\
         }}\n",
        if typed { ": void" } else { "" },
        if typed { " as string" } else { "" },
    ));
    for (id, tool) in functions(tools) {
        out.push('\n');
        out.push_str(&comment(&describe(tool)));
        let (input, reply) = if typed {
            (
                format!("input: {}", ts_type(&tool.input_schema)),
                ": Promise<string>",
            )
        } else {
            ("input".to_string(), "")
        };
        out.push_str(&format!(
            "export function {id}({input}){reply} {{\n  return call({}, input);\n}}\n",
            literal(&tool.name)
        ));
    }
    out
}

/// `plaza.mjs`: the JavaScript library.
pub fn javascript_library(tools: &[ToolSpec]) -> String {
    script_library(tools, false)
}

/// `plaza.mts`: the TypeScript library, with each tool's input typed from its schema.
pub fn typescript_library(tools: &[ToolSpec]) -> String {
    script_library(tools, true)
}

/// The TypeScript type of a JSON schema: the object, array, union and literal types a tool's input is made of. A schema
/// it does not read is `unknown`.
pub fn ts_type(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let literals: Vec<String> = values
            .iter()
            .map(|value| match value {
                Value::String(text) => literal(text),
                Value::Number(_) | Value::Bool(_) => value.to_string(),
                Value::Null => "null".to_string(),
                _ => "unknown".to_string(),
            })
            .collect();
        return literals.join(" | ");
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = schema.get(key).and_then(Value::as_array) {
            let types: Vec<String> = options.iter().map(ts_type).collect();
            return types.join(" | ");
        }
    }
    match schema.get("type") {
        Some(Value::Array(kinds)) => {
            let types: Vec<String> = kinds
                .iter()
                .filter_map(Value::as_str)
                .map(|kind| ts_type(&json!({ "type": kind })))
                .collect();
            types.join(" | ")
        }
        Some(Value::String(kind)) => match kind.as_str() {
            "string" => "string".to_string(),
            "number" | "integer" => "number".to_string(),
            "boolean" => "boolean".to_string(),
            "null" => "null".to_string(),
            "array" => {
                let items = schema.get("items");
                let text = items.map_or("unknown".to_string(), ts_type);
                if items.is_some_and(is_union) {
                    format!("({text})[]")
                } else {
                    format!("{text}[]")
                }
            }
            "object" => object_type(schema),
            _ => "unknown".to_string(),
        },
        _ => "unknown".to_string(),
    }
}

/// Whether a schema's type is a union of several types (so it needs parentheses inside an array type).
fn is_union(schema: &Value) -> bool {
    let several = |key: &str| {
        schema
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|options| options.len() > 1)
    };
    several("enum") || several("anyOf") || several("oneOf") || several("type")
}

/// The TypeScript type of an object schema: its properties, the required ones without a `?`, or a record when it names
/// none.
fn object_type(schema: &Value) -> String {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match schema.get("properties").and_then(Value::as_object) {
        Some(properties) if !properties.is_empty() => {
            let fields: Vec<String> = properties
                .iter()
                .map(|(key, sub)| {
                    let marker = if required.contains(&key.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    format!("{}{marker}: {}", property_key(key), ts_type(sub))
                })
                .collect();
            format!("{{ {} }}", fields.join("; "))
        }
        _ => match schema.get("additionalProperties") {
            Some(extra) if extra.is_object() => {
                format!("{{ [key: string]: {} }}", ts_type(extra))
            }
            _ => "Record<string, unknown>".to_string(),
        },
    }
}

/// A property name as a TypeScript key: bare when it is an identifier, else quoted.
fn property_key(key: &str) -> String {
    let mut chars = key.chars();
    let bare = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if bare { key.to_string() } else { literal(key) }
}

/// The module a script imports the library as, with its names destructured on the script's first line (so the line
/// numbers of the script's errors are its own).
fn prelude(tools: &[ToolSpec], module: &str) -> String {
    let mut names = vec!["call", "result", "PlazaError"];
    let ids: Vec<String> = functions(tools).into_iter().map(|(id, _)| id).collect();
    names.extend(ids.iter().map(String::as_str));
    format!(
        "import * as plaza from \"./{module}\"; const {{ {} }} = plaza; ",
        names.join(", ")
    )
}

/// The files a script run writes: the script, and the library it imports, by name.
pub fn files(script: &Script, tools: &[ToolSpec]) -> Vec<(&'static str, String)> {
    match script.language {
        Language::Python => vec![
            ("plaza.py", python_library(tools)),
            ("runner.py", PYTHON_RUNNER.to_string()),
            ("script.py", script.code.clone()),
        ],
        Language::JavaScript => vec![
            ("plaza.mjs", javascript_library(tools)),
            ("script.mjs", prelude(tools, "plaza.mjs") + &script.code),
        ],
        Language::TypeScript => vec![
            ("plaza.mts", typescript_library(tools)),
            ("script.mts", prelude(tools, "plaza.mts") + &script.code),
        ],
    }
}

/// `(major, minor)` of a `node --version` line such as `v22.6.0`.
fn node_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().strip_prefix('v')?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Checks that node in the workspace can strip TypeScript's types, which a TypeScript script needs.
fn node_strips_types(workspace: &Workspace) -> Result<(), String> {
    let found = bash::command("node", &["--version"], workspace.dir(), Privilege::Agent)?
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| node_version(&String::from_utf8_lossy(&output.stdout)));
    match found {
        Some(version) if version >= TYPES_FROM_NODE => Ok(()),
        Some((major, minor)) => Err(format!(
            "TypeScript scripts need node {}.{} or later for type stripping; this VM has node {major}.{minor}. Write the script in Python or JavaScript.",
            TYPES_FROM_NODE.0, TYPES_FROM_NODE.1
        )),
        None => Err(format!(
            "TypeScript scripts need node {}.{} or later, and this VM has no node. Write the script in Python.",
            TYPES_FROM_NODE.0, TYPES_FROM_NODE.1
        )),
    }
}

/// A tool call the script made: its tool and input, and the connection its answer goes back on.
pub struct Request {
    pub name: String,
    pub input: Value,
    link: UnixStream,
}

impl Request {
    /// Sends the call's answer to the script (`ok`, and the text), and closes the connection.
    pub fn answer(mut self, ok: bool, output: &str) {
        let line = json!({ "ok": ok, "output": output }).to_string();
        let _ = self.link.write_all(format!("{line}\n").as_bytes());
    }
}

/// Reads one call's line on its connection and hands it to the run, or answers that it is not a call.
fn read_call(link: UnixStream, calls: &Sender<Request>) {
    let _ = link.set_nonblocking(false);
    let _ = link.set_read_timeout(Some(LINE_WAIT));
    let Ok(reader) = link.try_clone() else {
        return;
    };
    let mut line = String::new();
    if BufReader::new(reader).read_line(&mut line).is_err() {
        return;
    }
    let call = serde_json::from_str::<Value>(&line).ok().and_then(|call| {
        let name = call.get("name")?.as_str()?.to_string();
        Some((name, call.get("input").cloned()))
    });
    match call {
        Some((name, input)) => {
            let input = match input {
                None | Some(Value::Null) => json!({}),
                Some(input) => input,
            };
            let _ = calls.send(Request { name, input, link });
        }
        None => Request {
            name: String::new(),
            input: Value::Null,
            link,
        }
        .answer(
            false,
            "INVALID_INPUT: the call is not a JSON line with a tool name",
        ),
    }
}

/// A run directory, removed with its files when the run ends.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A running script: its directory, its socket, its process and what it has written.
pub struct Session {
    child: Option<Child>,
    status: Option<ExitStatus>,
    socket: UnixListener,
    stdout: PathBuf,
    stderr: PathBuf,
    result: PathBuf,
    /// Last: the directory goes once the process has.
    _dir: Dir,
}

impl Session {
    /// Writes the script and its library into a new run directory, opens the socket, and starts the interpreter in the
    /// workspace. A refusal says why the script cannot run here.
    pub fn start(
        workspace: &Workspace,
        script: &Script,
        tools: &[ToolSpec],
    ) -> Result<Session, String> {
        if script.language == Language::TypeScript {
            node_strips_types(workspace)?;
        }
        let path = std::env::temp_dir().join(format!(
            "apz-script-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::SeqCst)
        ));
        // Only the run's own user may reach the directory, so only the script can reach the socket.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|err| format!("internal: cannot make the script's directory: {err}"))?;
        let dir = Dir(path);
        for (name, text) in files(script, tools) {
            let file = dir.0.join(name);
            fs::write(&file, text)
                .map_err(|err| format!("internal: cannot write {name}: {err}"))?;
        }
        let socket_path = dir.0.join("plaza.sock");
        let socket = UnixListener::bind(&socket_path)
            .map_err(|err| format!("internal: cannot open the script's socket: {err}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|err| format!("internal: cannot open the script's socket: {err}"))?;
        let stdout = dir.0.join("stdout");
        let stderr = dir.0.join("stderr");
        let result = dir.0.join("result.json");
        let (program, args): (&str, Vec<String>) = match script.language {
            Language::Python => (
                "python3",
                vec![
                    dir.0.join("runner.py").display().to_string(),
                    dir.0.join("script.py").display().to_string(),
                ],
            ),
            Language::JavaScript => ("node", vec![dir.0.join("script.mjs").display().to_string()]),
            Language::TypeScript => (
                "node",
                vec![
                    "--experimental-strip-types".to_string(),
                    dir.0.join("script.mts").display().to_string(),
                ],
            ),
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = bash::command(program, &args, workspace.dir(), Privilege::Agent)?;
        command
            .env("PLAZA_SOCKET", &socket_path)
            .env("PLAZA_RESULT", &result)
            .stdout(
                File::create(&stdout)
                    .map_err(|err| format!("internal: cannot open the script's output: {err}"))?,
            )
            .stderr(
                File::create(&stderr)
                    .map_err(|err| format!("internal: cannot open the script's output: {err}"))?,
            );
        let child = command
            .spawn()
            .map_err(|err| format!("the script cannot start {program}: {err}"))?;
        Ok(Session {
            child: Some(child),
            status: None,
            socket,
            stdout,
            stderr,
            result,
            _dir: dir,
        })
    }

    /// Takes the calls the script has begun since the last look. Each connection's line is read on a thread of its own,
    /// so a call that waits does not hold the others.
    pub fn accept(&mut self, calls: &Sender<Request>) {
        while let Ok((link, _)) = self.socket.accept() {
            let calls = calls.clone();
            thread::spawn(move || read_call(link, &calls));
        }
    }

    /// Whether the script's process has ended.
    pub fn exited(&mut self) -> bool {
        if self.status.is_some() {
            return true;
        }
        match self.child.as_mut().map(Child::try_wait) {
            Some(Ok(Some(status))) => {
                self.status = Some(status);
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// What the script has printed so far, stdout and stderr together.
    pub fn output_bytes(&self) -> u64 {
        [&self.stdout, &self.stderr]
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// Ends the script's process, and everything it started, if it still runs.
    fn stop(&mut self) {
        if self.exited() {
            return;
        }
        if let Some(child) = self.child.as_mut() {
            bash::kill_group(child);
            self.status = child.wait().ok();
        }
    }

    /// Ends the run and returns what the model is told: the script's value, or why it did not finish. `stopped` is why
    /// the harness ended the script, when it did.
    pub fn finish(mut self, stopped: Option<String>) -> Result<String, String> {
        self.stop();
        let tail = self.tail();
        if let Some(reason) = stopped {
            return Err(format!("[{reason}]{tail}"));
        }
        match self.status {
            Some(status) if status.success() => Ok(self.value()),
            Some(status) => Err(format!(
                "the script failed ({}):{tail}",
                bash::describe_exit(status)
            )),
            None => Err("internal: the script's exit status is unknown".to_string()),
        }
    }

    /// The value the script set with `result`, as the model reads it: a string as it is, anything else as JSON, cut to
    /// [`MAX_RESULT_BYTES`].
    fn value(&self) -> String {
        let Ok(text) = fs::read_to_string(&self.result) else {
            return "the script set no result: call result(value) to return one".to_string();
        };
        let value = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|mut wrapped| wrapped.get_mut("value").map(Value::take))
            .unwrap_or(Value::Null);
        let text = match value {
            Value::String(text) => text,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        cap_output(&text, MAX_RESULT_BYTES)
    }

    /// The end of what the script printed to stderr (or to stdout, when stderr is empty): where an error is.
    fn tail(&self) -> String {
        let error = read_tail(&self.stderr);
        let text = if error.trim().is_empty() {
            read_tail(&self.stdout)
        } else {
            error
        };
        if text.trim().is_empty() {
            String::new()
        } else {
            format!("\n{}", text.trim_end())
        }
    }
}

impl Drop for Session {
    /// A run that ends without its finish (a failed call, a closed host) still stops its script.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The last [`ERROR_TAIL_BYTES`] of a file, as text.
fn read_tail(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(ERROR_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-script-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn tool(name: &str, description: &str, schema: Value) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: schema,
            deferred: false,
        }
    }

    /// The tools of a session that a script may call: `read`, a tool with a dashed name, and `run_script` itself.
    fn tools() -> Vec<ToolSpec> {
        vec![
            tool(
                "read",
                "read the files named",
                json!({"type": "object", "properties": {"files": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}, "required": ["files"]}),
            ),
            tool("run-tests", "run the tests", json!({"type": "object"})),
            tool(TOOL, "run a script", json!({"type": "object"})),
        ]
    }

    /// Serves the script's calls the way the agent does: each call is answered by `answer` until the script ends. Returns
    /// the finished run's result.
    fn drive(
        mut session: Session,
        mut answer: impl FnMut(&Request) -> (bool, String),
    ) -> Result<String, String> {
        let (calls, requests) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if session.exited() {
                break;
            }
            if Instant::now() > deadline {
                return session.finish(Some("the test's deadline passed".to_string()));
            }
            session.accept(&calls);
            while let Ok(request) = requests.recv_timeout(Duration::from_millis(5)) {
                let (ok, output) = answer(&request);
                request.answer(ok, &output);
            }
        }
        session.finish(None)
    }

    #[test]
    fn a_script_names_its_language_code_and_timeout_and_refuses_the_rest() {
        let script = Script::parse(&json!({"language": "python", "code": "print(1)"})).unwrap();
        assert_eq!(
            script,
            Script {
                language: Language::Python,
                code: "print(1)".into(),
                timeout_s: DEFAULT_TIMEOUT_S,
            }
        );
        let typed =
            Script::parse(&json!({"language": "typescript", "code": "result(1)", "timeout_s": 5}))
                .unwrap();
        assert_eq!((typed.language, typed.timeout_s), (Language::TypeScript, 5));
        for bad in [
            json!({"language": "ruby", "code": "x"}),
            json!({"language": "python", "code": "  "}),
            json!({"language": "python", "code": "x", "timeout_s": 0}),
            json!({"language": "python", "code": "x", "timeout_s": MAX_TIMEOUT_S + 1}),
            json!({"language": "python", "code": "x", "timeout_s": "5"}),
        ] {
            let err = Script::parse(&bad).expect_err("a refused script");
            assert!(err.starts_with("INVALID_INPUT: "), "{err}");
        }
    }

    #[test]
    fn a_tool_name_becomes_an_identifier_the_languages_accept() {
        assert_eq!(identifier("run-tests"), "run_tests");
        assert_eq!(identifier("9lives"), "_9lives");
        assert_eq!(identifier("class"), "class_");
        assert_eq!(identifier("plaza_tasks"), "plaza_tasks");
        // run_script is not a function of the library, and a second tool with the same identifier is not offered.
        let ids: Vec<String> = functions(&[
            tool("run-tests", "", json!({})),
            tool("run_tests", "", json!({})),
            tool(TOOL, "", json!({})),
        ])
        .into_iter()
        .map(|(id, _)| id)
        .collect();
        assert_eq!(ids, vec!["run_tests"]);
    }

    #[test]
    fn the_python_library_has_a_function_per_tool_and_lists_them() {
        let library = python_library(&tools());
        assert!(library.contains("def read(input=None):"), "{library}");
        assert!(
            library.contains("return call(\"read\", input)"),
            "{library}"
        );
        assert!(library.contains("def run_tests(input=None):"), "{library}");
        assert!(
            library.contains("return call(\"run-tests\", input)"),
            "{library}"
        );
        assert!(!library.contains("def run_script"), "{library}");
        assert!(
            library.contains(
                "__all__ = [\"call\", \"result\", \"PlazaError\", \"read\", \"run_tests\"]"
            ),
            "{library}"
        );
        // A schema with a triple quote in its text cannot close the docstring early.
        let quoted = python_library(&[tool("odd", "say \"\"\" then stop", json!({}))]);
        assert!(quoted.contains(r#"say \"\"\" then stop"#), "{quoted}");
    }

    #[test]
    fn the_typescript_library_types_a_tool_with_nested_input() {
        let edit = tool(
            "edit",
            "change the files",
            json!({
                "type": "object",
                "properties": {
                    "edits": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "lines": {"type": "object", "properties": {"from": {"type": "integer"}, "to": {"type": "integer"}}, "required": ["from"]},
                            "mode": {"enum": ["replace", "insert"]},
                        },
                        "required": ["path"],
                    }},
                    "dry_run": {"type": "boolean"},
                },
                "required": ["edits"],
            }),
        );
        let library = typescript_library(&[edit]);
        // Properties are typed in the schema's order, and an optional one is marked with `?`.
        assert!(
            library.contains(
                "export function edit(input: { edits: { path: string; lines?: { from: number; to?: number }; mode?: \"replace\" | \"insert\" }[]; dry_run?: boolean }): Promise<string> {"
            ),
            "{library}"
        );
        assert!(
            library.contains("export class PlazaError extends Error {}"),
            "{library}"
        );
        assert!(
            library
                .contains("export function call(name: string, input?: object): Promise<string> {"),
            "{library}"
        );
        // The JavaScript library is the same functions without the types.
        let plain = javascript_library(&[tool("edit", "change", json!({"type": "object"}))]);
        assert!(plain.contains("export function edit(input) {"), "{plain}");
        assert!(!plain.contains(": Promise<string>"), "{plain}");
    }

    #[test]
    fn a_schema_the_types_do_not_read_is_unknown_and_a_map_is_a_record() {
        assert_eq!(
            ts_type(&json!({"type": "string", "enum": ["a", "b"]})),
            "\"a\" | \"b\""
        );
        assert_eq!(
            ts_type(&json!({"anyOf": [{"type": "string"}, {"type": "null"}]})),
            "string | null"
        );
        assert_eq!(
            ts_type(&json!({"type": ["number", "string"]})),
            "number | string"
        );
        assert_eq!(
            ts_type(
                &json!({"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "number"}]}})
            ),
            "(string | number)[]"
        );
        assert_eq!(
            ts_type(&json!({"type": "object"})),
            "Record<string, unknown>"
        );
        assert_eq!(
            ts_type(&json!({"type": "object", "additionalProperties": {"type": "number"}})),
            "{ [key: string]: number }"
        );
        assert_eq!(ts_type(&json!({"format": "date"})), "unknown");
        assert_eq!(
            ts_type(&json!({"type": "object", "properties": {"two words": {"type": "string"}}})),
            "{ \"two words\"?: string }"
        );
    }

    #[test]
    fn a_script_file_keeps_its_own_lines_and_imports_the_names_on_the_first() {
        let code = "const sum = 1;\nresult(sum);";
        let files = files(
            &Script {
                language: Language::JavaScript,
                code: code.into(),
                timeout_s: 5,
            },
            &tools(),
        );
        let (name, text) = &files[1];
        assert_eq!(*name, "script.mjs");
        assert!(text.starts_with("import * as plaza from \"./plaza.mjs\"; const { call, result, PlazaError, read, run_tests } = plaza; const sum = 1;"), "{text}");
        assert!(text.ends_with("\nresult(sum);"), "{text}");
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn a_node_version_is_read_and_checked_against_the_first_with_type_stripping() {
        assert_eq!(node_version("v22.6.0\n"), Some((22, 6)));
        assert_eq!(node_version("v24.19.0"), Some((24, 19)));
        assert_eq!(node_version("node 22"), None);
        assert!((22, 6) >= TYPES_FROM_NODE);
        assert!((20, 11) < TYPES_FROM_NODE);
    }

    #[test]
    fn a_python_script_calls_a_tool_and_sets_its_value() {
        let workspace = scratch("python-value");
        std::fs::write(workspace.dir().join("a.txt"), "alpha\n").unwrap();
        let script = Script {
            language: Language::Python,
            code: "text = read({\"files\": [{\"path\": \"a.txt\"}]})\nresult(text.upper())\n"
                .into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &script, &tools()).expect("a session");
        let mut asked = Vec::new();
        let value = drive(session, |request| {
            asked.push((request.name.clone(), request.input.clone()));
            (
                true,
                format!("contents of {}", request.input["files"][0]["path"]),
            )
        })
        .expect("the script finishes");
        assert_eq!(value, "CONTENTS OF \"A.TXT\"");
        assert_eq!(asked[0].0, "read");
    }

    #[test]
    fn a_refused_call_raises_in_the_script_and_its_message_is_the_error() {
        let workspace = scratch("python-refused");
        let script = Script {
            language: Language::Python,
            code: "try:\n    run_tests({})\nexcept PlazaError as err:\n    result(str(err))\n"
                .into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &script, &tools()).expect("a session");
        let value =
            drive(session, |_| (false, "the owner denied it".to_string())).expect("finishes");
        assert_eq!(value, "the owner denied it");
        // Uncaught, the same refusal ends the script, and its message is what the error says.
        let uncaught = Script {
            language: Language::Python,
            code: "run_tests({})\n".into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &uncaught, &tools()).expect("a session");
        let err = drive(session, |_| (false, "the owner denied it".to_string()))
            .expect_err("the script fails");
        assert!(err.starts_with("the script failed (exit 1):"), "{err}");
        assert!(err.contains("PlazaError: the owner denied it"), "{err}");
    }

    #[test]
    fn a_script_past_its_time_is_killed_and_says_so() {
        let workspace = scratch("python-time");
        let script = Script {
            language: Language::Python,
            code: "import time\ntime.sleep(60)\n".into(),
            timeout_s: 1,
        };
        let mut session = Session::start(&workspace, &script, &tools()).expect("a session");
        let started = Instant::now();
        let mut stopped = None;
        while !session.exited() {
            if started.elapsed() > Duration::from_millis(800) {
                stopped = Some("timed out after 1 s; the script was killed".to_string());
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let err = session.finish(stopped).expect_err("the script was stopped");
        assert!(
            err.starts_with("[timed out after 1 s; the script was killed]"),
            "{err}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn a_long_return_value_is_cut_and_the_cut_is_marked() {
        let workspace = scratch("python-cut");
        let script = Script {
            language: Language::Python,
            code: "result('a' * 40000)\n".into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &script, &tools()).expect("a session");
        let value = drive(session, |_| (true, String::new())).expect("finishes");
        assert!(value.len() <= MAX_RESULT_BYTES + 64, "{}", value.len());
        assert!(value.contains("bytes cut"), "{value}");
    }

    #[test]
    fn a_script_that_sets_no_result_says_so() {
        let workspace = scratch("python-none");
        let script = Script {
            language: Language::Python,
            code: "x = 1\n".into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &script, &tools()).expect("a session");
        let value = drive(session, |_| (true, String::new())).expect("finishes");
        assert!(value.starts_with("the script set no result"), "{value}");
    }

    #[test]
    fn a_javascript_script_calls_a_tool_and_sets_its_value() {
        if !node_present() {
            println!("skipped: no node on this machine, so the JavaScript script cannot run here");
            return;
        }
        let workspace = scratch("js-value");
        std::fs::write(workspace.dir().join("a.txt"), "alpha\n").unwrap();
        let script = Script {
            language: Language::JavaScript,
            code:
                "const text = await read({ files: [{ path: \"a.txt\" }] });\nresult(text.length);\n"
                    .into(),
            timeout_s: 20,
        };
        let session = Session::start(&workspace, &script, &tools()).expect("a session");
        let value = drive(session, |_| (true, "12345".to_string())).expect("finishes");
        assert_eq!(value, "5");
    }

    #[test]
    fn a_typescript_script_runs_through_node_type_stripping() {
        if !node_present() {
            println!("skipped: no node on this machine, so the TypeScript script cannot run here");
            return;
        }
        let workspace = scratch("ts-value");
        let script = Script {
            language: Language::TypeScript,
            code: "const twice: number = 2;\nconst text: string = await read({ files: [] });\nresult(text + twice);\n".into(),
            timeout_s: 20,
        };
        match Session::start(&workspace, &script, &tools()) {
            Ok(session) => {
                let value = drive(session, |_| (true, "ok".to_string())).expect("finishes");
                assert_eq!(value, "ok2");
            }
            // A node without type stripping is refused with its sentence, not run.
            Err(reason) => assert!(
                reason.contains("TypeScript scripts need node 22.6"),
                "{reason}"
            ),
        }
    }

    /// Whether node is on this machine's PATH (the tests of the Linux image have none).
    fn node_present() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }
}
