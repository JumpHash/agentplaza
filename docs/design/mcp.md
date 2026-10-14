# MCP servers: outside tools for agents, run by the hub

Status (2026-10-14): built. The hub side is `crates/apid/src/mcp/` (config, client, runtime, stdio, http, egress, catalog,
relay, routes), the relay is `crates/mcp-relay/`, and the console flow is `apps/web/src/views/mcp.ts` and
`apps/desktop/src/console/screens/mcp.rs`. The tests named at the end are in `mcp/tests.rs`, and the fixture server is `mcp/fixture.rs`.

The owner adds Model Context Protocol servers. Agents then get their tools as ordinary plaza tools. As with
connectors, every call is made by the hub: agents never hold a server's credentials, and no server runs on
the host itself.

## A server

```
McpServer {
  id: "github",                       // [a-z][a-z0-9-]{1,31}; tools appear as mcp__<id>__<tool> (see "Tool names")
  name, enabled,
  transport: {kind: "stdio", image: "ghcr.io/…:tag" | preset image, command: [..], workdir?}
           | {kind: "http",  url: "https://…/mcp"},
  env:     {VAR: {secret: "GITHUB_TOKEN"} | {value: "non-secret literal"}},      // stdio
  headers: {Header: {secret: "LINEAR_KEY", format: "Bearer {}"} | {value: "…"}}, // http
  storage: {volume: true},            // stdio: a docker volume apz-mcp-<id> at /data (never a host path)
  tools:   {allow: "all" | ["search_issues", …], risk: {tool: "read"|"write"|"irreversible"}},
  access:  {roles: ["builder", …] | "all", nodes: ["n0", …] | "all"},
  limits:  {timeout_s: 60, max_output_kb: 256, idle_stop_min: 10},
  allow_private: false,               // lets the server reach private and loopback addresses
  created_ms, updated_ms,
}
```

- **Storage.** `storage.volume` mounts the named docker volume `apz-mcp-<id>` at `/data`. A host path is
  never mounted: agents have their own VM filesystems, and nothing in a container reaches the host.
  - Deleting the server keeps the volume. `DELETE /v1/mcp/servers/{id}?purge=1` removes it.
  - http servers have no storage; a config that asks for it is refused.
- **Storage.** The config is a Postgres component `mcp` with the file fallback `mcp.json`. It never holds a
  secret value, only vault names.
- **Secrets.** Every `{secret}` reference resolves through the vault with binding `mcp:<id>`, on behalf of
  the calling agent, so node scopes apply. When the config flow creates a secret inline, the entry gets
  that binding. Referencing an existing secret adds the binding only after the owner confirms it in the
  flow ("allow the github server to use GITHUB_TOKEN").
  - Deleting the server removes `mcp:<id>` from every vault entry (`vault::Store::remove_binding`, which
    re-encrypts each entry under the new AAD and audits `bindings`). An entry left with no bindings stays
    in the vault; nothing can resolve it.
- **Reserved env.** `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `NODE_USE_ENV_PROXY` and `NODE_OPTIONS` are set by
  the hub on stdio containers; a config may not name them.

## Running a server

- **Instances.** One instance per (server, node). An agent's call uses the instance of the node the agent
  lives on. Node-scoped secrets can differ per node, so each node runs its own instance with its own
  environment.
- **stdio servers run in a container,** never as a host process.
  - Flags: `docker run --rm -i --init --read-only --tmpfs /tmp:size=64m --cap-drop ALL
    --security-opt no-new-privileges --user 65534:65534 --memory 512m --cpus 1 --pids-limit 256
    --network apz-mcp --ip <address> [--mount type=volume,source=apz-mcp-<id>,target=/data] <image> <command…>`.
  - Secrets reach the container as `-e VAR` with no value: the docker client reads the value from its own
    environment, which apid sets only for that child. Values never appear in argv or on disk.
  - The hub keeps the stdio pipes. The server starts on first use and stops after `idle_stop_min` without
    use. It restarts with backoff (1 s, 2 s, 4 s … 60 s) and gives up after 5 failures in 10 minutes, with
    status `error` and the last stderr lines.
- **The networks.** Two docker networks:
  - `apz-mcp` (`--internal`, 172.29.250.0/24). MCP containers attach only to it. It has no route out and no
    address on the host.
  - `apz-mcp-uplink` (a normal bridge, 172.29.251.0/24). The relay attaches to it to reach the host. apid refuses
    a network of either name that does not have this shape.
- **The relay.** The relay is the container `apz-mcp-relay`. It is attached to `apz-mcp` at the fixed address
  172.29.250.2 (the proxy address the MCP containers get), and to `apz-mcp-uplink`.
  - It runs `mcp_relay`, our static binary: std only, built for musl (`cargo build -p mcp-relay --release --target
    x86_64-unknown-linux-musl`, or `aarch64-unknown-linux-musl`). It is copied into apid's state directory
    (`<state>/mcp/mcp_relay`) at each start and mounted read-only into `alpine:latest` from there, because Docker
    Desktop mounts only shared paths and the home directory is shared. The image must already be present
    (`--pull never`; apid never pulls).
  - The container runs read-only, with `--cap-drop ALL`, `--security-opt no-new-privileges`, uid 65534, and a memory
    and pids limit.
  - It is created with `--add-host=host.docker.internal:host-gateway`, so it reaches the host as
    `host.docker.internal`. Docker Desktop provides that name; a native Linux engine maps it to the bridge gateway.
  - It has no policy. For each connection it sends apid the session token, then a PROXY protocol v1 line carrying
    the connection's source address (the container's address on `apz-mcp`), then copies bytes both ways.
  - A stale relay container (stopped, or left by an apid that was killed) is removed before a new one is created.
  - Started with `docker start -a -i`, the relay's stdin is a pipe apid holds: when apid exits, the pipe closes and
    the relay exits. The relay is restarted on the next start if it has died.
- **The proxy.** apid listens on `127.0.0.1:<port>` (default 19128, `--mcp-proxy-port`), which Docker Desktop
  delivers `host.docker.internal` to. On a native Linux engine it also listens on the bridge gateway, found with
  `docker network inspect bridge`, when the host has that address; where it does not, loopback is enough.
  - The relay is authenticated by a 256-bit token, generated fresh at each start. apid passes it only to the relay
    container's environment, and the relay sends it as the mandatory first line; then the PROXY line. The token is
    compared in constant time. The relay's address is not checked, because NAT on Docker Desktop hides it.
  - Handshake bounds. A valid token is always accepted, whatever has failed before: nothing counts failures against
    it, so no local container can lock out the servers.
    - A failed handshake (no token, a wrong one, or no valid PROXY line) is logged without the token, and held
      until 2 s after it was accepted before it is closed (a tarpit).
    - At most 16 unauthenticated connections are pending at once. When a new one arrives with the set full, the
      oldest pending connection is closed to make room, and the new one is accepted. A real relay authenticates
      within milliseconds, so it is never the oldest for long; an attacker holding slots is churned out. A tarpitted
      connection counts as pending and is evictable. Authenticated connections are not counted.
    - The handshake's reads time out after 2 s, so a connection that sends nothing or part of a line cannot hold
      a slot. After the handshake the request has the usual idle timeout.
    - A connection's slot is released when it authenticates or when it is closed. An evicted connection is closed
      and never served, even if its handshake completes just after the eviction.
  - The PROXY source address names the calling container, and so the server (its lease on `apz-mcp`). That address
    is internal-only and assigned by apid, so it is reported truthfully. The policy then applies to that server:
    - The fetch blocklist always applies.
    - The host is resolved. A private, loopback, link-local or metadata address (any resolved address) is refused,
      unless the server has `allow_private`.
  - Containers get `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, `https_proxy` set to `http://172.29.250.2:3128`,
    `NO_PROXY` empty and `NODE_USE_ENV_PROXY=1`.
  - A server that ignores the proxy has no network at all. That is the secure default.
  - It logs one line per server and host and verdict: `apid: mcp <server>: allowed|refused <host> (reason)`. No
    URL, path or query is logged.
- **Ordering and failures.** Before any stdio server starts, apid binds the proxy, ensures `apz-mcp` and the uplink,
  installs the relay binary, and starts the relay. A failure leaves the server in `status: error` with a clear
  `last_error`, for example:
  - docker is missing, or its daemon is not reachable;
  - a network cannot be created, or exists with the wrong shape;
  - the relay image is not on this machine ("pull it yourself with `docker pull alpine:latest`");
  - the relay binary is not found ("build it with `cargo build -p mcp-relay`");
  - the relay does not report ready in time.
  Remote (http) servers keep working in every case.
- **Lifecycle.** The relay container is removed when apid's egress state is dropped. If apid is killed instead, the
  relay exits because its stdin closes, and the stopped container is removed at the next start.
- **Architecture.** A Docker engine runs containers of its own architecture, and Docker Desktop's VM decides it on
  macOS and Windows. apid asks `docker info --format '{{.Architecture}}'` at each start and uses the relay for that
  architecture: `x86_64` or `aarch64`. Anything else is an error, and the remote servers keep working.
- **Packaging.** Every bundle that can run container servers carries both relays beside apid, as `mcp_relay-x86_64`
  and `mcp_relay-aarch64`. These are the static musl builds from the relay job (`.github/workflows/relay.yml`),
  which builds both for every release. Linux, macOS, Windows and Raspberry Pi packages all take that artifact, so
  each platform's container servers have a relay for both architectures. `packaging/scripts/stage.sh` copies them
  from `APZ_RELAY_DIR` (the artifact), or from `target/<musl>/release/` in a dev tree.
  apid finds the relay in this order: the musl build in a development tree (`target/<musl>/release/mcp_relay`), then
  `APZ_MCP_RELAY`, then the bundle beside apid (`mcp_relay-<arch>`, or `mcp_relay`).
- **Environment notes (Docker Desktop 29.8.2, where the end-to-end runs were done).**
  - Docker Desktop keeps bridges inside its own VM: a new network got no host address, so apid cannot bind a bridge
    gateway there. It uses loopback, which Docker Desktop connects to `host.docker.internal`.
  - Containers cannot take their bridge's own gateway address (Docker reserves it).
  - Docker Desktop mounts only shared host paths, so the relay binary is copied under the home directory first.

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
  `readOnlyHint` → read, otherwise write). Irreversible calls go through the owner's approvals, as connector
  actions do: C-004a withholds them from offers, and `approvals::gate` holds any that run.
  - The law engine sees an MCP call as operation `mcp.call`, with `server`, `tool` and `risk`, so the
    owner's rules can forbid or limit them.
- **Results.** Text content is joined and image or audio content goes to the file store (as with
  connectors). Output is capped at `max_output_kb`, and `isError` becomes a tool error.
- **Metering.** Each call that reaches the server is a transcript event `mcp_call` with `server`, `tool`,
  `ms`, `ok` and `output_bytes`. The host tool's own `tool_request` and `tool_decision` events record the
  call as for connectors. Usage (`usage.rs`) counts model tokens only and has no notion of connector calls,
  so MCP calls are not counted there.

## Routes (owner auth)

```
GET    /v1/mcp/catalog                 → presets: id, name, description, transport, image|url, env/header specs
                                          (name, secret?, required, help), fixed_env, volume, docs URL
GET    /v1/mcp/servers                 → [ServerView] (config + status: stopped|starting|running|error,
                                          tools count, last_error, last_used_ms)
POST   /v1/mcp/servers                 {config} → ServerView   (400 names the bad field; 409 on a duplicate id)
GET    /v1/mcp/servers/{id}            → ServerView
PATCH  /v1/mcp/servers/{id}            {partial config} → ServerView
DELETE /v1/mcp/servers/{id}[?purge=1]  → 204 (stops it; removes the vault bindings `mcp:<id>`; keeps the
                                          storage volume unless purge=1)
POST   /v1/mcp/servers/{id}/test       {node?} → {ok, server_info, tools:[{name, agent_name, description, annotations, auto_risk, risk}],
                                          resources, prompts, ms, error?}   (starts, lists, stops if idle)
POST   /v1/mcp/servers/{id}/start | /stop → ServerView
GET    /v1/mcp/servers/{id}/tools      → [{name, tool, agent_name, description, input_schema, annotations, auto_risk, risk, allowed}]
GET    /v1/mcp/servers/{id}/logs?limit= → [{ts_ms, stream, line}] (redacted)
```

In every tool view: `agent_name` is the `mcp__<id>__<tool>` name agents see; `auto_risk` is the risk the
annotations give, before any owner override; `risk` is the effective risk (the override, else `auto_risk`);
`tool` repeats `agent_name` for the earlier console; `allowed` is whether the server's allow list offers the tool.

Changes are pushed on the plaza's WebSocket as `mcp.changed` and `mcp.status`.

## The console's configuration flow (web and desktop)

The MCP screen sits under Setup, after Tools. It lists the servers with their status, tool counts and last
use. "Add server" walks through these steps:

1. **Source.** A catalog preset, a custom container image and command, or a remote URL.
2. **Credentials.** For each env var or header the server needs, pick a vault secret from a list filtered
   to plausible names, or enter a new value. A new value goes straight into the vault with binding `mcp:<id>`
   and a scope chosen here (plaza or nodes). Non-secret settings are plain fields. A preset's `fixed_env`
   and `volume` fill in the config.
3. **Test.** Run `test`. The flow shows the server's name, version and tools with their annotations, or
   the error and stderr tail.
4. **Tools.** Allow all or pick some, and adjust the risk per tool.
5. **Access.** Choose roles and nodes.
6. **Enable.**

Editing a server reopens the same steps.

## Presets

Filesystem is not a preset: a host folder mount would put a host path inside a container. Memory keeps its
graph in the server's storage volume (`MEMORY_FILE_PATH=/data/memory.json`, `storage.volume` true).

## Tests

- A fixture MCP server lives in the repo (`crates/apid/src/mcp/fixture.rs`, built as the `mcp_fixture`
  binary, with no third-party packages). Tests run it as a plain child process; the container launch is
  behind a trait.
- Covered: the handshake, tool listing with paging, a call, an error, a timeout, and an output cap.
- Redaction: a sentinel secret echoed by the fixture never comes back.
- Tool names: sanitising, the 64-character limit with its suffix, collisions, and the mapping back.
- `access` and risk gating, and approval for irreversible calls.
- Vault bindings: a server cannot resolve a secret not bound to it, and node scope applies through the
  calling agent.
- Redirects: a redirect to loopback is refused, a redirect to a blocklisted host is refused, an allowed hop
  works end to end (the endpoint moves), more than three hops are refused, and headers stay on the origin. The
  connection resolver refuses a loopback address it would connect to.
- Relay: the relay binary's hello (token, then PROXY line) and its loopback round trip; the networks' and the relay's
  argv (fixed address, the token by name only, `--pull never`, the read-only binary, hardening, `--add-host`, the
  upstream `host.docker.internal:<port>`); the installed copy in the state directory; and the start order (networks,
  relay create, connect, start), with the token never in a command line.
- Proxy: the token gate (a wrong, missing or forged token is dropped without an answer, in constant time), the PROXY
  line, the tarpit (a failed handshake is held until 2 s), the eviction of the oldest pending connection (a relay among sixteen attackers is served at once, 50 times),
  the handshake timeout (a silent or partial connection is closed), 100 bad handshakes followed by a valid one, the policy by the PROXY source (blocklist, private and metadata
  refusal, `allow_private`), the forwarded request over loopback, and the listeners (loopback always; the bridge
  gateway when the host has it, and optional).
- Egress: the proxy policy (blocklist, private and metadata refusal, `allow_private`), the proxy's answers
  to CONNECT and forwarded requests over loopback, and the network-create argv. Docker is never run in tests.
- Container argv and env: the hardened flags, `-e` names without values, `--ip`, the storage volume and no
  host path, the proxy variables.
- Restart backoff and give-up.
