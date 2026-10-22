# agent-harness

The in-guest agent harness: static Rust, one AF_VSOCK connection to the
orchestrator, no policy of its own. It speaks the frozen one-shot session (v1,
the demo task) or the long-lived keepalive agent session (v2, the fleet), and
inside a keepalive session the model-driven agent loop (v3, `src/agent.rs`). The
transcript, the sessions and the wire protocol are documented at the top of
`src/main.rs` and in `docs/agents/fleet.md` (the hub-side contract).

This file documents the **tools** — what the harness serves when the hub calls
it — for folding into `docs/agents/fleet.md` §1 (the tool table). It is the
harness side only: the hub dispatches `tool_call`s and counts `tool_result`s;
the harness owns the tools.

## The served tools

The table in `src/tools.rs` has 27 tools: `echo`, `uptime`, the message channel
(`conversation`), the file tools over the guest's writable `/workspace` volume,
`fetch_url` and `download` (the network, through the egress proxy), and the
owner's operator tools `ide_*` and `ide_term_*` (`src/ide.rs`, `src/term.rs`;
the model never gets these). The guest's init mounts that volume (ext4, from the
image the VMM was given with `--workspace`) before it starts the harness, so a
keepalive agent has a real filesystem to work on. A guest without the volume
answers every file tool `ok:false`, "unavailable: the workspace /workspace is
not mounted: …".

### Wire shape

Unchanged: the hub sends `{"type":"tool_call","call":N,"name":"<tool>","args":{…}}`
and the harness answers
`{"type":"tool_result","call":N,"ok":<bool>,"output":"<string>"}`. `output` is
the tool's own result on success, and on failure the reason, whose first word
is the error kind:

```
no such tool: <name>          the tool is not in the table
invalid input: <why>          the arguments are not the tool's schema
denied: <why>                 a path that leaves /workspace, a permission
not found: <why>              the file, directory or text is not there
limit exceeded: <why>         the call or its result is over a cap
conflict: <current hash>      an owner write named a stale hash (or "missing")
unavailable: <why>            the guest has no workspace (or no /proc/uptime)
internal: <why>               a filesystem failure that is this side's own
```

A refused tool is data on a live session, not a disconnect: the session goes on
and the hub learns which tool refused and why.

### Path discipline (every file tool)

Paths are **relative to `/workspace`** and are refused — with the reason, before
any filesystem call — when they are

* absolute (`/etc/passwd`) → `denied: … is absolute; every path is inside the workspace`;
* carry a `..` component (`../x`, `a/../../x`) → `denied: … contains ".." …`;
* a symlink, or pass through one, whose **resolved** path lands outside
  `/workspace` → `denied: … resolves to <path> outside the workspace`.

The escape check is on the resolved path: the deepest existing ancestor of the
target is canonicalized (following every link) and must be inside the canonical
workspace root. `search` never follows a symlink at all, so a link cannot walk a
scan out of the volume. No absolute path is ever echoed back: results name paths
relative to `/workspace`. A missing `/workspace` is `unavailable`. The harness's
own state folder, `.plaza`, is refused by name (`denied`), and `list` at the root
does not show it.

### Caps

The v2 tools ([`src/v2`](src/v2)) each carry a budget for their output, and a
result cut short by it says where to continue (`continue`, as a call of the tool
that resumes it). The shared caps:

| cap | value |
|-----|-------|
| any file the tools read or grow | 8 MiB read (`read`), 1 MiB edited (`edit`) |
| one write, or one edit's replacement | 256 KiB |
| a `conversation` message's `text` | 2 KiB |
| an edit's `old` text, a `search` pattern | 4 KiB |
| `read`: files per call, output per call | 32 files, 48 KiB |
| `find`: entries (`max`), output per call | 200 by default, 2000 at most, 16 KiB |
| `search`: matches (`max`), output per call | 100 by default, 1000 at most, 24 KiB; context 20 lines at most |
| `run`: output per command | 32 KiB (the head and the tail are kept) |
| `run_tests`: failures per call | 16 KiB |

Nothing is silently clamped: a request over a cap is `limit exceeded`, and a
result cut short by a bound carries `"truncated": true`.

### The tools

Each tool's `output` is one JSON object (as a string), except `echo` and
`uptime`. A result carries `notes` when something arrived since the last one:
a file the agent read changed underneath it (`"<path> changed since you read it"`),
or a background job ended (its exit status and last lines).

| tool | arguments | output |
|------|-----------|--------|
| `echo` | `text` | the text itself (not JSON) |
| `uptime` | — | `/proc/uptime`'s first two fields (not JSON) |
| `conversation` | `from_role`, `text`, `from_cid`? | `received`, `log`, `lines`, `size` |
| `read` | `files: [{path, start?, end?}]` | `files: [{path, lines, content, outline?, continue?, unchanged_since?, binary?, error?}]` |
| `edit` | `edits: [{path, old, new, replace_all?}]`, `create: [{path, content}]` | `files: [{path, ok, lines_changed, created?, replacements?, error?}]` |
| `find` | `glob?`, `content?`, `path?`, `max?`, `after?` | `files`, `matches: [{path, line, text}]` (with `content`), `truncated?`, `continue?` |
| `search` | `pattern`, `path?`, `glob?`, `context?`, `max?`, `after?` | `matches: [{path, line, text, before, after}]`, `skipped`, `truncated?`, `continue?` |
| `run` | `command`, `cwd?`, `timeout_s?`, `background?` | `output`, `exit`, `timed_out?`, `truncated?`, `continue?`, `job?` |
| `run_tests` | `command?`, `filter?` | `runner`, `command`, `exit`, `passed`, `failed`, `parsed`, `failures: [{name, file?, line?, message, output}]` |
| `fetch_url` | `url` | the page's text, prefixed `Fetched <url>:` (not JSON) |
| `download` | `url`, `path` | `path`, `bytes`, `created` |

* **`conversation`** is the plaza's message channel, and it is **not a file
  tool**: a peer's message arrives as a `conversation` call and the harness
  appends one line to its own `/workspace/messages.log` —
  `<yyyy-mm-ddThh:mm:ssZ> from <role> (cid <N>): <text>`, the `(cid <N>)` part
  omitted when the wire carried no `from_cid`. The stamp is **this guest's own
  clock** (UTC); the role and the text have every control character escaped
  (`\x0a`), so one message is one line and a hostile text cannot forge a line
  from another agent. The agent never touches the peer: the hub delivered the
  message, and the harness only writes it down.
* **`read`** numbers lines like `cat -n`. A file over 400 lines read without a
  range returns its outline (top-level definitions, by per-language patterns) and
  its first 200 lines. A file read again while unchanged, for lines already shown,
  answers `unchanged_since` with no content. Tracking is per work item.
* **`edit`** applies each file's edits together or not at all; an `old` that does
  not occur, or occurs more than once without `replace_all`, refuses that file and
  says why. A written file is recorded as read, so no re-read is needed to edit on.
* **`find`** lists files by glob and/or searches their contents by regex, from an
  index of the workspace that honours `.gitignore` and is rebuilt when a directory
  changes. **`search`** returns the matches with their context lines.
* **`run`** is one persistent shell per work item: its directory and exported
  variables carry from one call to the next, and a command past its timeout kills
  it. `background: true` starts a job that reports its end in a later result; its
  output is in `.plaza/jobs/<n>.log`.
* **`run_tests`** picks the runner from the workspace root (Cargo.toml: cargo
  test, or nextest when installed; go.mod; pytest.ini or conftest.py; package.json
  with jest or vitest), runs it through `run`, and parses the failures of cargo
  test, nextest, go test, pytest, jest and vitest.

## Notes for the hub side

* The harness holds no secrets. Its sockets are the one to the hub and the egress
  shim's loopback proxy (`src/egress.rs`), which `fetch_url` and `download` go
  through. The file tools are plain `std::fs` calls in the harness process (no
  shell, no subprocess), confined to `/workspace` by the rules above; `run`
  is the one tool that starts a process, and it runs as the VM's agent.
* `conversation` is a **basis** tool, like `echo` and `uptime`: the grant table
  (`TOOL_GRANTS` in `crates/dispatchd/src/roles.rs`) gives it no grant source,
  so a role cannot be refused the message channel by the manifest. A role that can be
  spoken to is the plaza's basis, not a privilege.
* `/workspace` is the only writable place in the guest; the root filesystem is
  the read-only initramfs.
* A tool call is served exactly once per `tool_call` frame. The keepalive session
  has no per-call journal yet, so a `tool_call` redelivered after a reconnect is
  served again — idempotent for `echo`/`uptime`/reads, *not* for appends (a
  redelivered `edit` that creates a file writes it again, and a redelivered `conversation`
  writes the message a second time: both are one fsynced line, and neither is
  deduped). The journal is the fleet contract's later slice.
