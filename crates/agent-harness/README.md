# agent-harness

The in-guest agent harness: static Rust, one AF_VSOCK connection to the
orchestrator, no policy of its own. It speaks the frozen one-shot session (v1,
the demo task) or the long-lived keepalive agent session (v2, the fleet). The
transcript, the sessions and the wire protocol are documented at the top of
`src/main.rs` and in `docs/agents/fleet.md` (the hub-side contract).

This file documents the **tools** — what the harness serves when the hub calls
it — for folding into `docs/agents/fleet.md` §1 (the tool table). It is the
harness side only: the hub dispatches `tool_call`s and counts `tool_result`s;
the harness owns the tools.

## The served tools

Nine tools: `echo`, `uptime`, and the file tools over the guest's writable
`/workspace` volume. The guest's init mounts that volume (ext4, from the image
the VMM was given with `--workspace`) before it starts the harness, so a
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
relative to `/workspace`. A missing `/workspace` is `unavailable`.

### Caps

| cap | value |
|-----|-------|
| `read_file` default `max_bytes` | 64 KiB |
| any file the tools read or grow | 1 MiB |
| one write/append payload, one edit replacement | 256 KiB |
| a `search` pattern or `edit_file` needle | 4 KiB |
| `list` entries returned / read | 1024 / 65536 |
| `search` matches / files / bytes / depth | 256 / 4096 / 8 MiB / 16 |
| bytes of a matching line `search` returns | 256 |

Nothing is silently clamped: a request over a cap is `limit exceeded`, and a
result cut short by a bound carries `"truncated": true`.

### The tools

Each tool's `output` is one JSON object (as a string):

| tool | arguments | output fields |
|------|-----------|---------------|
| `echo` | `text` (string) | the text itself (not JSON) |
| `uptime` | — | `/proc/uptime`'s first two fields (not JSON) |
| `read_file` | `path`, `max_bytes`? | `path`, `size`, `bytes`, `truncated`, `content` |
| `write_file` | `path`, `content` | `path`, `bytes`, `created` |
| `edit_file` | `path`, `find`, `replace` | `path`, `matches`, `size` |
| `append_file` | `path`, `content` | `path`, `bytes`, `size` |
| `list` | `path`? (default the root) | `path`, `entries[{name,kind,size}]`, `truncated`, `skipped` |
| `search` | `pattern`, `path`? | `pattern`, `path`, `matches[{file,line,text}]`, `files_scanned`, `bytes_scanned`, `skipped`, `truncated` |

* **`read_file`** returns the file's text. A file that is not UTF-8 is refused
  ("is not UTF-8 text") rather than returned as mojibake. A `max_bytes` cut that
  lands inside a multi-byte character drops that character and sets
  `truncated` (a whole-file read of a valid file stays valid).
* **`write_file`** creates or replaces, atomically (a sibling temp file, flushed,
  renamed over the target). The parent directory is never created: a path whose
  directory does not exist is `not found`.
* **`edit_file`** is a one-shot literal replacement requiring **exactly one**
  match. Zero occurrences is `not found`; more than one is `invalid input` ("the
  text to replace appears N times …; edit_file needs exactly one match"). It
  never guesses which occurrence was meant. `replace` may be empty (a deletion).
* **`append_file`** appends, creating the file if needed, and refuses an append
  that would grow the file past 1 MiB rather than truncating it.
* **`list`** is one directory, sorted by name, with `kind` one of `file`, `dir`,
  `symlink`, `other` and `size` for files only (`null` otherwise). Names that are
  not UTF-8 are counted in `skipped`, never mangled.
* **`search`** is a literal substring search, depth-first in sorted name order
  (so the same tree gives the same answer). Non-UTF-8 files are skipped and
  counted. `truncated` is true when any bound — matches, files, bytes, depth, an
  oversized file, or a line cut to 256 bytes — ended the search early.

## Notes for the hub side

* The harness holds no secrets and opens no socket but the one to the hub; the
  file tools are plain `std::fs` calls in the harness process (no shell, no
  subprocess), confined to `/workspace` by the rules above.
* `/workspace` is the only writable place in the guest; the root filesystem is
  the read-only initramfs.
* A tool call is served exactly once per `tool_call` frame. The keepalive session
  has no per-call journal yet, so a `tool_call` redelivered after a reconnect is
  served again — idempotent for `echo`/`uptime`/reads, *not* for writes (a
  redelivered `append_file` appends twice). The journal is the fleet contract's
  later slice.
