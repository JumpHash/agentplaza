# MCP servers: outside tools for agents, run by the hub

Status (2026-10-22): the hub side is `crates/apid/src/mcp/` (config, client, runtime, runtimes, stdio, http, egress,
catalog, routes). The config is the typed component `mcp` (`crates/apid/src/store/components.rs`). The console flow is
`apps/web/src/views/mcp.ts` and `apps/desktop/src/console/screens/mcp.rs`. The tests named at the end are in
`mcp/tests.rs`; the fixture server is `mcp/fixture.rs`, built as the `mcp_fixture` binary.

The owner adds Model Context Protocol servers. Agents then get their tools as ordinary plaza tools. As with
connectors, every call is made by the hub: agents never hold a server's credentials. A stdio server runs under the
OS sandbox, with no network except the hub's egress proxy. No server runs on the host unconfined, and no platform
needs Docker.

## A server

```
McpServer {
  id: "github",                       // [a-z][a-z0-9-]{1,31}; tools appear as mcp__<id>__<tool> (see "Tool names")
  name, enabled,
  transport: {kind: "stdio", package: Package, args: ["stdio"], workdir?}
           | {kind: "http",  url: "https://…/mcp"},
  env:     {VAR: {secret: "GITHUB_TOKEN"} | {value: "non-secret literal"}},      // stdio
  headers: {Header: {secret: "LINEAR_KEY", format: "Bearer {}"} | {value: "…"}}, // http
  storage: {volume: true},            // stdio: keep the server's data/ directory between runs
  tools:   {allow: "all" | ["search_issues", …], risk: {tool: "read"|"write"|"irreversible"}},
  access:  {roles: ["builder", …] | "all", nodes: ["n0", …] | "all", projects: "all" | [..]},
  limits:  {timeout_s: 60, max_output_kb: 256, idle_stop_min: 10},
  allow_private: false,               // lets the server reach private and loopback addresses
  created_ms, updated_ms,
}
Package = {kind: "npm",    name: "@scope/pkg", version: "1.2.3"}          // an exact version, no ranges
        | {kind: "pypi",   name: "mcp-server-time", version: "2026.10.19", entry?: "mcp-server-time"}
        | {kind: "binary", url: "https://…/server.tar.gz", sha256: "<64 hex>", entry: "server"}
```

- **Packages are pinned.** A stdio server names its package and its exact version (npm, PyPI) or its URL and SHA-256
  (a binary). A config holding an `image` is refused with a sentence saying to pick the package: a container image no
  longer runs a server.
- **The wire shape.** The console sends `transport` as above. Its tool listings are not in the server's JSON.
- **Storage.** `storage.volume` keeps the server's `data/` directory from one start to the next. Without it, `data/`
  is emptied at each start. A host path is never mounted; the server's files live under the hub's state directory.
  - Deleting the server keeps its data. `DELETE /v1/mcp/servers/{id}?purge=1` removes `data/`; installed packages stay.
  - http servers have no storage; a config that asks for it is refused.
- **The component.** The config is the typed table `mcp_server` in `plaza.db` (its children: the transport, its
  package, its args, the env and headers, the tool policy, the access picks, and the tools it last offered). It is saved
  with the other components. No file holds it. It never holds a secret value, only vault names.
- **Secrets.** Every `{secret}` reference resolves through the vault with binding `mcp:<id>`, on behalf of the calling
  agent, so node scopes apply. When the config flow creates a secret inline, the entry gets that binding. Referencing an
  existing secret adds the binding only after the owner confirms it in the flow.
  - Deleting the server removes `mcp:<id>` from every vault entry (`vault::Store::remove_binding`).
- **Reserved env.** `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, `https_proxy`, `NO_PROXY`, `NODE_USE_ENV_PROXY`,
  `NODE_OPTIONS`, `PATH`, `HOME` and `TMPDIR` are set by the hub on stdio servers; a config may not name them.
- **`{data}`.** A literal env value may name the server's data directory as `{data}`, expanded at each start (the memory
  preset keeps its graph at `{data}/memory.jsonl`).

## Running a server

- **Instances.** One instance per (server, node). An agent's call uses the instance of the node the agent lives on.
  Node-scoped secrets can differ per node, so each node runs its own instance with its own environment.
- **Runtimes.** Two runtimes are provisioned by the hub on first use, under `<state>/runtimes/`:
  - Node v24.22.1 (the LTS line), from nodejs.org/dist, runs npm packages.
  - uv 0.13.5 (github.com/astral-sh/uv/releases) runs PyPI packages.
  Each is pinned in code per platform (darwin-arm64, darwin-x64, linux-x64, linux-arm64, win-x64) by the SHA-256 the
  publisher lists (the nodejs.org `SHASUMS256.txt`, the GitHub release digests). A download goes through the hub's own
  HTTP client, https only, is checked against its SHA-256 before anything is unpacked, and is unpacked once. A missing
  or mismatched hash refuses with a sentence, and nothing is installed. A platform with no pin says so, and the remote
  (http) servers keep working.
- **Layout.** Each server has `<state>/mcp/<id>/` with `pkg/` (its npm prefix), `bin/` (downloaded binaries, by SHA-256),
  `data/` (its working directory), `home/`, `tmp/`, `cache/`, `tools/` and `python/`. A sandboxed process writes only
  under its own server's directory and the runtimes are read-only to it.
- **npm packages.** The first start installs the pinned package, sandboxed the same way as the server, with
  `node npm-cli.js install --prefix pkg --ignore-scripts --no-audit --no-fund --no-update-notifier name@version`. A marker
  (`pkg/.apz-installed`) records the install, so later starts skip it. The server then runs as `node <the package's bin>`.
  Lifecycle scripts do not run.
- **PyPI packages.** `uv tool run --from name==version <entry> <args>`, sandboxed. uv keeps its cache, its tools and its
  managed Pythons under the server's directory. The first start may take a while (the handshake allows 120 s).
- **Binary packages.** The URL is downloaded, checked against its SHA-256, unpacked if it is a `.tar.gz` or `.zip`, and
  the `entry` file is run directly, sandboxed. A raw file is the file itself.
- **The sandbox.** Every stdio process runs under `apz_sandbox`: the macOS Seatbelt profile (deny by default), Landlock
  on Linux (where the kernel has network rules, Linux 6.7 and later; an older kernel cannot confine the network, so
  a server that uses the network is refused there), and an AppContainer on Windows. The child's environment is the one
  the hub builds: nothing is inherited. Secret values are env vars of the server's own process, never argv or a file.
  - Memory and CPU caps are not set: macOS offers no per-process memory cap, and a process cap counts every process of
    the user.
  - **Windows** refuses the loopback port a server needs for the proxy, so stdio servers that use the network do not
    run there yet. Remote (http) servers work.
- **The network.** A stdio process reaches only the hub's egress proxy: the sandbox allows TCP to that loopback port and
  nothing else (`Network::LoopbackPort`). A server that ignores the proxy has no network at all.
- **Lifecycle.** The server starts on first use and stops after `idle_stop_min` without use. It restarts with backoff
  (1 s, 2 s, 4 s … 60 s) and gives up after 5 failures in 10 minutes, with status `error` and the last stderr lines.
  Stopping kills the server's process (the sandbox's child; a child process it started with `uv` may outlive it, but
  without network and with its proxy credential gone).

## microVMs

Where the local node can boot a microVM (its `GET /v1/mcp-vm`: a KVM host, a guest kernel, initrd and workspace base),
a stdio server runs in one instead of under the sandbox. The server's status says which: `launcher` is `vm`, or
`sandbox: <why>` (no KVM, a node that is not the hub's, no egress proxy). The install of an npm package still runs
sandboxed on the hub.

- **Volumes.** Each read-only directory the sandbox would have (the runtimes, the package, a binary's directory) is a
  read-only node volume, mounted at `/data/ro-<hash>`. The volume is copied from the directory once per change of that
  directory. The server's own directory is the guest's `/workspace`: `storage.volume` keeps its image on the node under
  `<persist>/mcp/<server>/`, and otherwise each run starts from a fresh copy.
- **Stdio.** The guest's initrd runs `/mcp-bridge` (the `vsockd` crate's bin) instead of the agent harness when the
  command line has `plaza.mcp=1`. The node sends the bridge one JSON line on vsock port 5200 (program, args, env with
  the secrets, cwd), then relays the server's stdin and stdout on that port; stderr goes on 5201. The hub reaches each
  through a Unix socket the node opens for the run.
- **Egress.** The guest has no network device. Its proxy variables name the lease's proxy URL on `127.0.0.1:3128`; the
  bridge tunnels each connection over vsock port 5202, and the node dials the hub's egress proxy port and nothing else.
  The lease's token is the server's own proxy credential, as in the sandbox.
- **Routes.** On the node: `GET /v1/mcp-vm`, `POST /v1/mcp-runs`, `GET` and `DELETE /v1/mcp-runs/{id}`,
  `DELETE /v1/mcp-storage/{server}`. The hub and its node share a machine: the sockets are local paths.
- **Tests.** The relays, the request and the lifecycle run without KVM. The full boot of a server is a test in the node
  that is skipped with a printed reason when `/dev/kvm` is missing.

## The egress proxy

- **Listener.** apid listens on `127.0.0.1:<port>` (default 19128, `--mcp-proxy-port`). It binds when the first stdio
  server starts.
- **Leases.** Each start of a stdio server takes a lease: a new 256-bit token, which the server holds as the password of
  its proxy URL (`http://apz:<token>@127.0.0.1:<port>`). Every request therefore says `Proxy-Authorization: Basic`, and
  the token names the server the request is policed for. The lease is forgotten when the process's lease drops, so the
  next start gets a new token. The token is never logged.
- **Handshake bounds.** A valid credential is always accepted, whatever has failed before: nothing counts failures
  against it.
  - A request with no credential, or a wrong one, is logged without the token and held until 2 s after it was accepted,
    then answered 407 (a tarpit).
  - At most 16 unauthenticated connections are pending at once. When a new one arrives with the set full, the oldest
    pending connection is closed to make room. Authenticated connections are not counted.
  - The request head's reads time out after 2 s, so a connection that sends nothing cannot hold a slot.
- **Policy.** For the server a credential names:
  - The fetch blocklist always applies.
  - The host is resolved. A private, loopback, link-local or metadata address (any resolved address) is refused, unless
    the server has `allow_private`.
  - It logs one line per server and host and verdict: `apid: mcp <server>: allowed|refused <host> (reason)`. No URL, path
    or query is logged.
- **Failures.** A failure leaves the server in `status: error` with a clear `last_error`: the runtime cannot be
  downloaded or does not match its pin; the package's install fails (its last stderr line is named); the sandbox cannot
  start the process (Windows for a network server; a kernel that cannot confine the network); the proxy cannot listen.
  Remote (http) servers keep working in every case.

## Tool names

- An agent's tool for an MCP tool is named `mcp__<id>__<tool>`. That fits the model APIs' `[a-zA-Z0-9_-]`.
- Every character outside `[A-Za-z0-9_-]` becomes `_`.
- A name over 64 characters is cut to 57 characters and gets `_` plus 6 hex digits of the sha256 of the
  original name (`mcp__<id>__<tool>` before sanitising).
- A tool whose name collides with an earlier tool of the same server after sanitising gets the same suffix.
- The hub keeps the mapping back to the real tool name and server (the registry). Calls and laws use the
  mapping; a name nobody registered is not an MCP tool.

## Agents' side

- **How tools appear.** Each allowed tool becomes a plaza tool, with the server's own JSON schema and
  description, prefixed `[MCP <name>]`. It is offered only to agents whose role and node pass `access`.
- **Risk.** The owner's override, else the tool's annotations (`destructiveHint` → irreversible,
  `readOnlyHint` → read, otherwise write). The annotations are typed (the MCP hints: title, readOnlyHint,
  destructiveHint, idempotentHint, openWorldHint). Irreversible calls go through the owner's approvals, as connector
  actions do: C-004a withholds them from offers, and `approvals::gate` holds any that run.
  - The law engine sees an MCP call as operation `mcp.call`, with `server`, `tool` and `risk`, so the owner's rules
    can forbid or limit them.
- **Results.** Text content is joined and image or audio content goes to the file store (as with connectors). Output is
  capped at `max_output_kb`, and `isError` becomes a tool error.
- **Metering.** Each call that reaches the server is a transcript event `mcp_call` with `server`, `tool`, `ms`, `ok` and
  `output_bytes`. The host tool's own `tool_request` and `tool_decision` events record the call as for connectors.

## Routes (owner auth)

```
GET    /v1/mcp/catalog                 → presets: id, name, description, transport, package (for this platform),
                                          args, url, env/header specs (name, secret?, required, help), fixed_env,
                                          volume, docs URL, status ("verified" | "binary needed"), note
GET    /v1/mcp/servers                 → [ServerView] (config + status: stopped|starting|running|error,
                                          tools count, last_error, last_used_ms)
POST   /v1/mcp/servers                 {config} → ServerView   (400 names the bad field; 409 on a duplicate id)
GET    /v1/mcp/servers/{id}            → ServerView
PATCH  /v1/mcp/servers/{id}            {partial config} → ServerView
DELETE /v1/mcp/servers/{id}[?purge=1]  → 204 (stops it; removes the vault bindings `mcp:<id>`; keeps the
                                          data unless purge=1)
POST   /v1/mcp/servers/{id}/test       {node?} → {ok, server_info, tools:[{name, agent_name, description, annotations, auto_risk, risk}],
                                          resources, prompts, ms, error?}   (starts, lists, stops if idle)
POST   /v1/mcp/servers/{id}/start | /stop → ServerView
GET    /v1/mcp/servers/{id}/tools      → [{name, tool, agent_name, description, input_schema, annotations, auto_risk, risk, allowed}]
GET    /v1/mcp/servers/{id}/logs?limit= → [{ts_ms, stream, line}] (redacted)
```

In every tool view: `agent_name` is the `mcp__<id>__<tool>` name agents see; `auto_risk` is the risk the annotations
give, before any owner override; `risk` is the effective risk (the override, else `auto_risk`); `tool` repeats
`agent_name` for the earlier console; `allowed` is whether the server's allow list offers the tool.

Changes are pushed on the plaza's WebSocket as `mcp.changed` and `mcp.status`.

## The console's configuration flow (web and desktop)

The MCP screen sits under Setup, after Tools. It lists the servers with their status, tool counts and last use. "Add
server" walks through these steps:

1. **Source.** A catalog preset (its package for this platform is filled in), a custom package (npm, PyPI or a binary
   URL with its SHA-256), or a remote URL.
2. **Credentials.** For each env var or header the server needs, pick a vault secret from a list filtered to plausible
   names, or enter a new value. A new value goes straight into the vault with binding `mcp:<id>` and a scope chosen here
   (plaza or nodes). Non-secret settings are plain fields. A preset's `fixed_env` and `volume` fill in the config.
3. **Test.** Run `test`. The flow shows the server's name, version and tools with their annotations, or the error and
   stderr tail.
4. **Tools.** Allow all or pick some, and adjust the risk per tool.
5. **Access.** Choose roles, nodes and projects.
6. **Enable.**

Editing a server reopens the same steps.

## Presets

Filesystem is not a preset: a host folder mount would put a host path inside the server. Each preset pins its package:

- GitHub: the release binary of github/github-mcp-server v2.0.3 for the platform, pinned by its SHA-256 from the GitHub
  release digests (darwin-arm64, darwin-x64, linux-x64, linux-arm64, win-x64), run as `github-mcp-server stdio`. The file
  name inside the archive and the `stdio` argument come from the project's README and GoReleaser config; they were not
  run offline.
- Fetch: PyPI `mcp-server-fetch` 2026.10.19. Time: PyPI `mcp-server-time` 2026.10.19.
- Memory: npm `@modelcontextprotocol/server-memory` 2.0.0, with `MEMORY_FILE_PATH={data}/memory.jsonl` and
  `storage.volume` true.
- Brave Search: npm `@brave/brave-search-mcp-server` 2.1.4 (`BRAVE_API_KEY`).
- Linear and Context7: hosted http endpoints.

## Tests

- A fixture MCP server lives in the repo (`crates/apid/src/mcp/fixture.rs`, built as the `mcp_fixture` binary, with no
  third-party packages). The runtime tests run it as a pinned binary package under the sandbox: its URL is served by a
  fake fetch from the built file, and its SHA-256 is computed in the test. Build it with `cargo build -p apid --bin
  mcp_fixture` before `cargo test -p apid --bins mcp` (the test binaries do not rebuild it).
- Covered: the handshake, tool listing with paging, a call, an error, a timeout, an output cap, a server that exits at
  start (backoff and give-up), an idle stop, and the start failure whose reason stays on the server.
- Sandbox: the fixture's `connect` mode runs under the sandbox with the proxy's port as its only reachable port: the
  proxy connects, another local listener and an address beyond loopback are refused.
- Redaction: a sentinel secret echoed by the fixture never comes back.
- Tool names: sanitising, the 64-character limit with its suffix, collisions, and the mapping back.
- `access` and risk gating, and approval for irreversible calls.
- Vault bindings: a server cannot resolve a secret not bound to it, and node scope applies through the calling agent.
- Config: validation names each bad field; a package must be pinned (an exact version, a hash, an https URL); a workdir
  stays inside the data directory; an `image` is refused; the wire shape is tagged. The store round-trips its rows.
- Runtimes: a download that does not match its pin is refused and named; a missing pin refuses before any download; a
  runtime unpacks once and is reused; a zip unpacks; a binary's entry must exist in the download. One ignored test
  downloads uv for the host from github.com and runs it (`cargo test -p apid --bins -- --ignored uv_really`).
- Proxy: a request without a lease credential gets 407; a lease is policed for its own server (loopback refused without
  `allow_private`); a lease is forgotten when it drops; the egress and env variables are set for the children.
- Redirects: a redirect to loopback is refused, a redirect to a blocklisted host is refused, an allowed hop works end to
  end, more than three hops are refused, and headers stay on the origin. The connection resolver refuses a loopback
  address it would connect to.
- Restart backoff and give-up.
