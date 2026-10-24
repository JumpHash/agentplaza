# AgentPlaza architecture

This page describes how the parts fit together as the code is now: the work engine in the hub that runs tasks as
sessions, the nodes that run the VMs, the store, and the consoles. When this page and the code disagree, fix one of
them in the same change. The [repository layout](repository-layout.md) lists every crate and app. The table-level
detail of the store is in [`design/database.md`](design/database.md), and the console parity is in
[`design/parity.md`](design/parity.md).

## 1. What runs where

| process | crate | listens | owns |
|---|---|---|---|
| `apid` (the hub) | `crates/apid` | `127.0.0.1:7700` (`--port`); `--bind` and `--tls` serve HTTPS off loopback | The control plane: the work engine (sessions, seats, threads, budgets, cards, chats), projects and tasks, the laws, the model gateway, the vault, the store, the Library, the Weft server, MCP servers, the hub's own MCP server, the IDE, the E2B sandbox API, the node and station registries, the public `/v1` API, the event socket `/v1/ws`, and the web console's files. |
| `noded` | `crates/noded` | `127.0.0.1:8899` (`--port`) | One machine's VMs: the agent, planner and check VMs it is asked to boot, workstation VMs, sandbox VMs, MCP server VMs, training jobs, data volumes, warm templates, sleep and wake, snapshots, forks, and the export and import of a sleeping VM's files. It holds no plaza state. With `--agents` it boots the role fleet at start. |
| `vsockd` | `crates/vsockd` | `127.0.0.1:19080` (`--http-port`); one Unix socket per guest CID | The node's relay. It serves the guest sessions on this node (vsock port 5000), relays the agent loop's lines to apid and back, serves the guest's egress proxy (vsock port 5001), and serves the E2B gateway when `--e2b-port` is set. It has no policy and no plaza state. |
| `apz-vmm` | `crates/vmm` | none | One jailed VM process on KVM, Hypervisor.framework or WHP, derived from Firecracker. A guest has no network device. Its only channel to the host is vsock. |
| agent-harness | `crates/agent-harness` | none (inside the guest) | The agent loop in each VM: the model's turns, the guest tools, the egress proxy on `127.0.0.1:3128`, and the ACP client for coding engines. |
| `apz-link` | `crates/apz-link` | none | Joins a machine to a hub with a join code (`agentplaza node join`), keeps the link open, and long-polls the hub's command queue for its node. |
| `apz-workstation` | `crates/apz-workstation` | none (dials the hub) | The daemon on a machine that agents operate through the hub. It holds a station link open and serves the hub's requests: files, terminals and runs confined to the work folder, screen sessions, screenshots, input, and STOP ALL. |
| `weft` | `crates/weft` | `weft serve` on the hub; `weft serve --cache` on each node | Version control. The hub's server keeps the signed log, the blobs, the state and the landing queues. A node's cache answers its misses. |
| `dispatchd` | `crates/dispatchd` | none | A separate fleet dispatcher binary. No other crate starts or calls it. The work loop in apid is what runs tasks. |
| `vmctl` | `crates/vmctl` | none | A developer tool that spawns N microVMs from the command line. It is not part of a running plaza. |

Consoles talk only to `apid`. `noded` and `vsockd` require the internal token on their HTTP APIs (section 6).

### Machines

A hub machine runs `apid`, `noded` and `vsockd`, and `weft serve` beside `apid`. Its own node is `n0`, reached over
loopback (`crates/apid/src/fleet.rs`). A joined machine runs `noded`, `vsockd` and `apz-link`. The hub reaches its
`noded` through the node's command queue, which `apz-link` long-polls. An agent lives on the node whose CID range
holds its CID.

### VMs

- **Agent VMs.** A step session (a builder or a reviewer of a task) gets a VM of its own, started on the node that
  placement chooses for its role (`crates/apid/src/placement.rs`). The VM is retired when the session ends. The
  fleet's orchestrator VM is long-lived: `noded --agents` boots it from the roles manifest, and sleep never applies to
  its role.
- **Planner VMs.** Each active project has one long-lived planner session, on a VM of its own (section 4).
- **Check VMs.** A review graph's check node is a work item that runs the project's commands in a fresh Weft
  checkout and reports measured scores (`crates/apid/src/review_graph/mod.rs`). The VM that runs it is the check
  VM. Weft's own crate names the same kind (`crates/weft/src/lib.rs`).
- **Workstation VMs.** Desktop guests a node makes for the hub, with their own disk, a virtual display and no network
  device. Their relay carries the join config over vsock (`crates/noded/src/workstation.rs`).
- **Sandbox VMs.** One `noded` run with `sandbox: true`, for the E2B-compatible sandbox API (`docs/design/e2b.md`).
- **MCP server VMs and training jobs.** `noded` also runs MCP server VMs (`mcp_vm.rs`) and the Foundry's training
  jobs (`train.rs`). A stdio MCP server that the hub runs is confined by the operating system's sandbox on the hub
  instead (`crates/apz-sandbox`).

No VM has keys or a model. A guest has no network interface.

### The agent loop

A guest's harness sends its lines over vsock. `vsockd` stamps the CID from the socket path (never from the line),
forwards every v3 line to `apid`, and writes apid's reply back to the guest. Model calls can take minutes, so `vsockd`
runs each forward on a worker thread and keeps reading heartbeats meanwhile.

```
guest harness ──model_call──▶ vsockd ──▶ apid: budget hold, model policy, gateway ──▶ provider (streamed)
              ◀──model_result─┘
guest harness ──tool_request─▶ vsockd ──▶ apid: laws and role grants decide
              ◀──allow / deny─┘          allow: the guest runs it, then tool_report
                                         host tool: apid runs it in its own process
guest harness ──work_done────▶ vsockd ──▶ apid closes the work item
```

The wire is `apz_proto::wire`: one JSON object per line, tagged by `type`. Guest lines are `hello`, `heartbeat`,
`bye`, `stats`, `tool_call` and `tool_result`, then `model_call`, `tool_request`, `tool_report`, `work_done` and
`compressed`, and `engine_update` for a coding engine. Host lines are `welcome`, `tool_call`, `tool_result`, `work`,
`model_result`, `tool_decision`, `permission_decision`, `cancel`, `stop`, `profile` and `control`. A `tool_decision` is
`allow`, `deny`, or `pending` with the approval the owner must decide.

## 2. Agent tools and containment

**The containment rule.** An agent never runs anything on the hub and never reaches the hub or another agent to run
something there. Commands run only in the agent's own VM. The agent chooses a tool and its arguments, never a command
for the host.

**Guest tools** run in the VM, in the harness. File tools (`read_file`, `write_file`, `edit_file`, `append_file`,
`multi_edit`, `list`, `glob`, `grep`, `search`) are confined to `/workspace`. Shells (`bash`, `bash_start`,
`bash_output`, `bash_kill`), `install_packages`, `todo_write`, `code_engine`, `tool_package`, `deliver_file`,
`share_file`, the `artifact_*` tools, the `checkpoint_*` tools, and the internet tools `fetch_url` and `download` (through the
egress proxy) run in the guest. Tool output is capped at 64 KiB (`wire::MAX_TOOL_OUTPUT`).

**Host tools** run in apid with the agent's identity. They are plaza operations: tasks (`plaza_post_task`), the board,
messages, files another agent chose to publish, skills, the toolbox, projects, the plaza's Weft repositories, the
agent's memory, and the workstation tools. `crates/apid/src/tools.rs` holds the split between guest and host tools,
and a test pins it.

**Outside services.** Two kinds are available to the owner's choice, and neither takes a command from an agent.
*Connectors* (`crates/apid/src/connectors.rs`) are called by apid at the orchestrator, with their credentials in the
vault. Each action is classed `read`, `write` or `irreversible` for the laws. *MCP servers* (`crates/apid/src/mcp`)
are run by the hub. Each tool a server offers becomes a plaza tool named `mcp__<id>__<tool>`, offered to the roles
and nodes its access allows. Agents never hold a server's credentials. An approved Library tool is offered to every
agent as `x_<name>`.

**What an agent is offered.** The in-VM tools and the always-on tools for every role, plus the role's grant list
(`docs/agents/initial-roles.json`, or a custom role). Tier- or seat-gated tools appear only where the tier or seat allows. Every offered tool must also be permitted by the laws
for that role (`crates/apid/src/laws.rs`, the JHRL law set). A request for a tool that is not offered is denied and
recorded.

**The owner's IDE tools are not guest tools.** The `ide_*` tools (`ide_list`, `ide_read`, `ide_write`,
`ide_changes`, `ide_checkpoints`, `ide_restore`, `ide_search`, `ide_run_start`, `ide_run_poll`, `ide_run_stop` and the
`ide_term_*` set) are served by the harness only for the owner. The hub reaches them through the `tell` call of
the agent's gateway (`crates/apid/src/ide.rs`, `crates/agent-harness/src/ide.rs`). The model is never offered them:
they are in neither the role grants nor the built-in specs. If the model asks for one anyway, the harness refuses
it by name, before any tool runs: `denied: ide_write is an owner tool; the model is not offered it`
(`run_guest_tool` in `crates/agent-harness/src/agent.rs`, with a test for `ide_write`).

## 3. How a task runs

### Projects

A project moves through the stages `brief`, `research`, `proposal`, `critique`, `approval`, `implementation`,
`review` and `delivered` (`crates/apid/src/projects.rs`). The orchestrator opens a project from the owner's goal and
writes its brief, in the project's chat. The planner writes the proposal. The owner starts the project from a `start`
card. Only a project in `implementation` or `review`, or one in maintenance, takes work
(`project_active` in `work_loop.rs`). A paused or archived project takes none.

A project in maintenance is looked after rather than built. Its watchers read what comes in about it and propose
work, and nothing runs until the owner picks a proposal (`crates/apid/src/maintenance.rs`).

### Tasks

The planner or the orchestrator posts a task, with `plaza_post_task`. A task is ready when it is open, every task it
lists in `after` is done, and its project is active. The work loop ticks from the scheduler's thread, every 2 seconds,
and nothing new starts while STOP ALL holds the plaza (`crates/apid/src/scheduler.rs`).

On each tick the loop does four things in order (`crates/apid/src/work_loop.rs`):

1. It ensures each active project has its planner (section 4).
2. It starts each ready task: it opens the task's seats from the `change` recipe and reserves the task's budget from
   its project. A project that cannot reserve the budget asks the owner with a `budget` card, and the task waits.
3. It starts a session for each ready seat, on a model the project allows for the seat's role, on a VM of its own.
4. It reaps sessions whose VM never said hello or whose VM went away. A seat then tries again.

### Seats and the change recipe

A seat is a role in a task, filled by one session at a time. The built-in `change` recipe
(`crates/apid/src/seats.rs`) has two steps:

- **build** on the `builder` seat. Any model the project allows for the role.
- **review** on the `reviewer` seat. A model that no other seat of the task used (`ModelPolicy::Differ`).

A review that fails sends the task back to the builder with its findings. It does this at most 3 times
(`CHANGE_ROUNDS`). After that the task fails and the planner is told. A seat gets up to 3 sessions for one round,
when a session fails to start or dies (`ATTEMPTS`).

### Sessions

A session is one fresh context working one step of one task. It has no name and no memory of its own: what it learns
goes into its task's journal. Its record is in memory and in `plaza.db` (table `session`). A session moves through
these states (`crates/apid/src/sessions.rs`):

```
queued ─► placing ─► forking ─► running ─► handing_over ─► ending ─► ended
   │         │          │        │  ▲
   └─────────┴──────────┴────────┴──┴──► failed      paused (budget, allowance, disk, card, node)
```

A session's VM starts from the node placement chooses. The session stays `forking` until the VM says `hello`, and it
fails if that takes longer than 5 minutes (`HELLO_DEADLINE_MS`). On `hello`, the session's work item goes out on the
VM's lane: the task's goal and acceptance, what a review sent back, and any messages waiting on its seat. When the
work item ends, or the reviewer gives its verdict, the seats move on (build, then review, then done, or back to the
builder). The session writes its handover and ends, and its VM is retired.

The engine is the plaza's own harness (`ENGINE = "harness"`). Inside a VM, `code_engine` can run one of twelve coding
engines as an ACP agent (`crates/agent-harness/src/acp`). Each engine is pinned to one version and installed in the
VM on first use.

A code task's builder seals its Weft lane when it finishes. The reviewer then reviews a fixed revision.

### Budgets

Budgets are in dollars or in tokens (`crates/apid/src/budgets.rs`). The unit is set per budget. Every amount is
held in both units, so a task whose unit differs from its project's is checked in its own unit and charged to the
project in the project's.

- **A project's budget** has a `cap`, what it has `spent`, and what its running tasks have `reserved`. A planner, a
  long-lived session, charges the project directly, under the project's `monthly_cap`.
- **A task's budget** is reserved from its project when the task starts. The reservation is the task's own cap if it
  has one in the project's unit, else the project's default task budget, else 2,000,000 micro-dollars or tokens
  (`DEFAULT_RESERVE_USD`, `DEFAULT_RESERVE_TOKENS`). The unspent part goes back when the task ends.
- **Each model call is held before it is sent.** The hold is the call's worst case: its request's size (about three
  bytes a token) and the answer cap it names, or 32,000 tokens when it names none. A call that does not fit in what
  the task has left is not sent. Its session pauses with the reason `budget`, and the owner gets a `budget` card.
- **After the call** the actual cost is charged and the hold released (`settle`). Every change is a row in the
  budget ledger.

Writes to budget state go through to `plaza.db` at full durability before a call is sent, so a hold the hub has
acknowledged survives a power loss.

### Landing through Weft

When a review passes, a code task's change joins its repo's landing queue (`queue.add` on the Weft hub). Weft's queue
(`crates/weft/src/landing.rs`) merges the head onto the ref when its base is current. Where the repo asks for checks,
the change waits for their verdict first. A clash moves the change onto the head with the clashing files marked, and
the hub treats it as a failed review: the task goes back to its builder with the landing's prompt (`clashed`).

When the change lands, the task is done: its sessions end, its seats close, and its budget is released (`landed`).
A task without a code change is done when its review passes.

### Cards, chats and threads

**Chats** are the owner's way in. There is one main chat, with the orchestrator. A project has as many chats as the
owner opens, each with the project's planner. A task has one chat, with its seats. Each message in a chat is from the
owner, a session, or the system, and is a plain message, a question (a card), or a notice (what the AIs decided or
did).

**Cards** are the decisions that only the owner can make. A card is shown in the chat it belongs to, with its choices
as buttons. The kinds are `start`, `budget`, `allowance`, `disk`, `connector`, `tool`, `engine`, `recipe`, `promote`,
`memory`, `review`, `land`, `adopt`, `yank`, `seat`, `release`, `deploy`, `tool_call`, `incident` and `proposal`
(`crates/apid/src/cards.rs`). A card is open, answered, or withdrawn. When the owner answers, `on_card_answered`
decides what the answer sets going. Everything else is decided by the AIs and reported as a notice.

**Threads** are direct talk between seats, not sessions. A message goes to a seat, such as `T12/builder` or
`planner`. The hub delivers it to the session filling that seat. If none fills it, the message waits and is the first
thing the next session of that seat reads. If the task is done, the message goes to the planner
(`crates/apid/src/threads.rs`). Two seats that exchange more than 8 messages (`LOOP_LIMIT`) in one thread, with no
output or close between them, are reported to the planner as a loop.

**The owner's messages** to a chat are delivered to the session the chat is with (`deliver_owner_message`).

## 4. Long-lived sessions

### The main orchestrator

The orchestrator is the one long-lived session that talks to the owner in the main chat. Its role is `orchestrator`,
the only role in the fleet (`docs/agents/initial-roles.json`, `fleet`). `noded --agents` boots its VM at start, and
starts it again after a crash. When the orchestrator's VM says `hello` with no session, the hub makes that VM's
session the main orchestrator's, and ends the plaza's other live long-lived sessions (`adopt_orchestrator`). Its calls
are the plaza's own (its project is `plaza`). Its role is never put to sleep (`sleep_never_roles`).

### Planners

A project's planner is a long-lived session on a VM of its own, on the project's model for the `planner` role. The
loop starts one for each active project when the plaza has the `planner` role (`ensure_planners`). A project whose
last planner failed within 10 minutes is not given another (`PLANNER_BACKOFF_MS`). In maintenance the planner is the
one who sweeps the project (`maintenance.rs`). A planner writes the project's proposal, plans its tasks, decides what
follows a failed task, and answers its project chats.

### Compaction

A long-lived session keeps its conversation within the model's window. The harness summarises the older part of the
conversation once it passes `compress_at` and records the summary as a `compressed` line. The session record has a
`compacting` state, which a running session may enter and leave for `running` or `paused`. No code in apid sets it yet.

### Helper sessions

The session kind `helper` (a bounded job a session starts in its own fork) is defined. No code in apid creates one yet.

## 5. Storage

### The state folder

The hub keeps its state in one folder (`--state-dir`, by default the platform's state directory from `apz-os`). It
holds:

- `plaza.db`, `transcripts.db` and `metrics.db`: the three SQLite files (below).
- `store.lock`: one hub per folder. A second hub on the same folder refuses to start.
- `internal.token`: the internal token that `noded` and `vsockd` check, and that apid and `apz-link` send (section 6).
- `vault.key`: the vault's key file. It is created on first start and refused when group or world readable.
- `blobs/`: the object store (below).
- `backups/`: a backup of a file, taken before a migration runs on it.

### The three SQLite files

`crates/apid/src/store/mod.rs` opens three files:

| file | durability | holds |
|---|---|---|
| `plaza.db` | `FULL` (on macOS with `fullfsync`) | Every component of the hub: the work engine's records (sessions, seats, threads, budgets and their ledger, cards, chats), projects and tasks, agents and roles, nodes and stations, the vault's tables, laws and their overrides, settings, the usage ledger, the Library's index, the review graph, the IDE audit and the station audit. An acknowledged write here survives a power loss. |
| `transcripts.db` | `NORMAL` | Every agent's transcript, one row per entry, typed. Rows carry the ids of what they belong to in `plaza.db`. |
| `metrics.db` | `NORMAL` | Metric samples and rollups, written by apid's metrics module (`crates/apid/src/metrics`, the `sqlite` store) and read by the Metrics view. |

Each file has one writer thread, which runs its writes in transactions. Readers use a pool of connections, and WAL
lets them read a consistent snapshot without waiting for the writer (`crates/apz-store`). The components keep an
in-memory working copy. The hub's saver writes what changed to `plaza.db`. A message the owner sends is written at
once. Migrations are embedded SQL, numbered from 1 (`sql/<file>/NNNN_*.sql`). A file newer than the code is refused.

The table-level design is in [`design/database.md`](design/database.md). The code and the `sql/` folders are the
record.

### The object store

`apz-blobs` keeps bytes by their BLAKE3 hash, in append-only packs (`pack-000001.wpk`, and so on) under
`<state-dir>/blobs`. Where each blob lives is indexed in SQLite (`index.db`). A put is durable when it returns. A blob
over a size threshold is cut into content-defined chunks, and its hash names a manifest of them. Deletion is mark and
sweep. The hub's users are the Library (deliverables that agents hand in), artifacts, skill files, station
recordings, and the review graph's change trees (`crates/apid/src/state.rs`).

## 6. The hub's API

`apid` serves the `/v1` API on its port, and a route module per domain adds its routes to one router
(`crates/apid/src/api.rs`). The route families below are the ones `docs/api/openapi.json` lists:

- **Work**: `/v1/sessions`, `/v1/chats`, `/v1/cards`, `/v1/threads`, and the budget routes under `/v1/projects/{id}/budget`
  and `/v1/tasks/{id}/budget` (`crates/apid/src/work_routes.rs` lists them).
- **Projects and tasks**: `/v1/projects`, `/v1/tasks`, `/v1/board`, `/v1/project-modes`, `/v1/recipes`.
- **Agents and machines**: `/v1/agents` (with `tell`, `transcript` and checkpoints), `/v1/nodes`, `/v1/machines`,
  `/v1/stations` (workstations), `/v1/runs`, `/v1/volumes`, `/v1/templates`, `/v1/foundry` (training), `/v1/node-link`.
- **Plaza control**: `/v1/plaza/stop` (STOP ALL), `/v1/laws`, `/v1/settings`, `/v1/providers`, `/v1/prices`, `/v1/usage`.
- **Knowledge and tools**: `/v1/library`, `/v1/artifacts`, `/v1/skills`, `/v1/prompts`, `/v1/toolbox`, `/v1/connectors`,
  `/v1/mcp`, `/v1/hub-mcp`, `/v1/vault`.
- **Code**: `/v1/weft` (the Weft server's API, under the hub's front door), `/v1/ide`, `/v1/reviews`.
- **Sandboxes**: `/e2b` (the E2B-compatible API, with its own keys).
- **Metrics and events**: `/v1/metrics`, and `/v1/ws`, the event socket. An event carries a `seq`, and a socket
  opens with a `hello` event that names the hub's boot and schema. A client that sees a gap revalidates its lists.

The routes that nodes and agents call are under `/internal/v1` (`crates/apid/src/internal.rs`). They are the agent
loop's lines, the node's events, and the gateway that a VM's coding engine calls for its model
(`crates/apid/src/llm_proxy.rs`). The hub serves the consoles' static files from `--console` (`apps/web/dist` by
default in a checkout).

**Who may call what.** Loopback callers with a local `Host` are trusted (`crates/apid/src/hub_auth.rs`). Any other
caller of an owner route sends the owner's bearer token. The HTTP APIs of `noded` and `vsockd` (except `GET /healthz`
on `vsockd`) require the internal token in the `x-apz-internal` header. The receiver reads the token from its
`--internal-token-file` on each request, and apid and `apz-link` send it on every call they make to these daemons.
The daemons bind loopback only.

## 7. The consoles

Each console is one client of the same `/v1` API. None of them talks to a node.

- **Web** (`apps/web`): TypeScript and esbuild, with no framework. `build.mjs` writes `dist/`, which apid serves. The
  Machines screen's 3D view is `crates/apz-engine` as WebAssembly (`apz-engine-web`).
- **Desktop** (`apps/desktop`): native, on iced and wgpu. It opens on a startup chooser (start locally, host a hub, join a
  hub as a node, or connect to a hub), then shows the console's shell around one screen at a time
  (`apps/desktop/src/console`).
  The legacy register stays reachable with `--legacy`.
- **CLI and terminal screen** (`apps/cli`): the `agentplaza` command. Every console feature is a command, `agentplaza chats`
  talks to the orchestrator and the planners, `agentplaza tui` shows the plaza in a terminal, and
  `agentplaza node` joins a machine to a network.

The screen-by-screen parity between the three is in [`design/parity.md`](design/parity.md).

## 8. Library crates

The daemons share these crates. Each one's module docs say what it holds.

- `apz-proto`: the wire and chat types, with no I/O.
- `apz-http`: the one small HTTP/1.1 server, client and SSE implementation, with the loopback `Host` guard.
- `apz-plaza`: the plaza's domain state (the task ledger and the board) as pure state, with no I/O: `apid` owns the
  store and the clock.
- `apz-store`: the SQLite store (section 5).
- `apz-blobs`: the object store (section 5).
- `apz-cache`: caches kept true by generations, not by events.
- `apz-sandbox`: runs a child process under the operating system's own sandbox (Seatbelt, Landlock, AppContainer).
  MCP stdio servers and training jobs run through it, with no container runtime.
- `apz-egress-rules`: the addresses an egress proxy refuses, in one place. Both proxies (vsockd's for VMs, apid's for
  MCP servers) use it.
- `apz-setup`: first-run checks of a plaza's requirements, for the desktop app and the CLI. It installs nothing.
- `apz-trust`: join tokens, node keys, pinned TLS and owner tokens.
- `apz-metrics`: the metrics core, with series, histograms, rollups and alert rules. No I/O.
- `apz-station`: the hub-to-workstation link protocol.
- `apz-ide`: the IDE's shared model (workspaces, trees, buffers, diffs, paths, tab state). The desktop and the web share it.
- `jhrl`: the law language the laws are written in.
- `acpi-tables`: the ACPI tables a VM boots with.

## 9. Ports and paths

| what | default | flag |
|---|---|---|
| apid's API | `127.0.0.1:7700` | `--port` (`--bind`, `--tls` for HTTPS off loopback) |
| apid's content origin (HTML artifacts) | the API port + 1 | `--content-port` |
| apid's MCP egress proxy | `127.0.0.1:19128` | `--mcp-proxy-port` |
| apid's E2B gateway (vsockd's) | `127.0.0.1:49982` | `--e2b-gateway-port` |
| apid's upstream to vsockd (`/v1/hub`) | `127.0.0.1:19080` | `--hub-port` |
| noded | `127.0.0.1:8899` | `--port` |
| vsockd's HTTP surface | `127.0.0.1:19080` | `--http-port` |
| vsockd's guest sessions and egress | vsock 5000 and 5001 per CID | `--port`, `--egress` |
| the guest's egress proxy | `127.0.0.1:3128` in the guest | fixed |
| the hub's state folder | the platform's state directory | `--state-dir` |

The hub's roles manifest is `docs/agents/initial-roles.json` by default, and its role prompts are
`docs/codex-draft/roles` in a checkout (`--roles`, `--prompts`).
