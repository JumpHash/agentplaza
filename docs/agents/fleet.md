# The agent fleet: long-running harnesses, the hub, and the initial role setup

Owner directive: **every VM runs a harness, every harness is connected to the
hub, the instances are long-running, and the roles are defined up front so the
service starts with an initial setup of agents.**
This note is the contract for the slices that implement it. It is additive:
the demo task's one-shot session (the frozen protocol the acceptance suites
drive) stays exactly as it is.

## 1. Protocol v2 (additive over v1)

v1 (unchanged, the demo task): `hello{vm,guest_cid}` → `welcome{echo}` →
`tool_call echo` → `tool_result` → `tool_call uptime` → `tool_result` →
`bye{calls}`, session over, the guest acts on it (reboot/exit).

v2 adds, on the same port and framing (JSON lines):

- `hello` carries `"role": "<role>"` (from the guest's `plaza.role=` cmdline
  flag; absent/empty when the guest has none) and `"keepalive": true` when the
  guest runs a long-lived instance (from `plaza.keepalive=1`).
- `welcome` echoes `{"echo": "...", "role": "<the role the hub accepted>",
  "keepalive": <bool>}` — the hub's acceptance of the role is the agent's
  identity for its lifetime.
- **Keepalive sessions do not end with `bye`.** After `welcome`:
  - the guest sends `{"type":"heartbeat","seq":N}` every
    `HEARTBEAT_SECS = 5`;
  - the hub sends `tool_call`s at will; the guest answers `tool_result`s
    (the harness's tools: `echo`, `uptime`, the social channel
    `conversation`, and the `/workspace` file tools — the table lives in
    `crates/agent-harness/README.md`);
  - the hub may send `{"type":"stop","reason":"..."}`; the guest answers
    `{"type":"bye","calls":N}` and the session ends cleanly;
  - a guest whose connection drops reconnects with backoff
    (250 ms → 30 s, reset by a successful welcome) — a hub restart does not
    kill an agent, it re-adopts it.
- The hub's idle deadline for a keepalive session is
  `KEEPALIVE_IDLE_SECS = 300` without a heartbeat (a dead guest is reaped,
  never leaked); a v1 session keeps today's `SESSION_TIMEOUT`.

## 2. The harness (crates/agent-harness)

- New cmdline inputs: `plaza.role=<role>`, `plaza.keepalive=1`.
- v1 mode (default when `plaza.harness=1` and no keepalive): unchanged.
- Keepalive mode: connect → hello(role, keepalive) → welcome → loop
  (heartbeat every 5 s; serve `tool_call`s; exit the loop on `stop`, then send
  `bye`) → reconnect on any I/O failure with the backoff above → never exits
  on its own account (a stop is the hub's decision; a fatal setup error prints
  and retries — the guest stays up).
- The transcript lines stay: `[vsock->]` / `[vsock<-]` per frame, plus
  `agent-harness: role <role> accepted` and `agent-harness: heartbeat N`.

## 3. The guest init (scripts/mk-initrd.sh's init.c)

- `plaza.keepalive=1` sets `plaza.harness=1` implicitly.
- In keepalive mode the init **does not reboot** when the harness exits: the
  reap path reports and the guest stays up (the harness owns its reconnect
  loop; if it exits for good, the VM is still inspectable and killable).
- Everything else (the beat/console/poweroff paths, the frozen v1 session
  path without keepalive) is untouched.

## 4. The hub (crates/vsockd)

- Sessions are held open for keepalive guests; the hub tracks per-CID state:
  `{cid, role, connected_at, last_heartbeat, calls}`.
- The registry is observable: a `status` line query is out of scope for this
  slice; the hub LOGS every transition in its existing voice
  (`vsockd: agent <cid> role <role> connected` / `... heartbeat <seq>` /
  `... reaped idle > 300s`) and keeps the registry in memory for a later
  status surface.
- A v1 session (no keepalive in hello) is served exactly as today.
- Caps unchanged: `MAX_SESSIONS=64`, `MAX_TOOL_CALLS` per session (keepalive
  sessions count tool calls the same way).

## 5. The initial setup (crates/noded)

- `docs/agents/initial-roles.json` is the manifest (generated from the
  codex's `roles v1.1`, statutes S-001…S-016): 21 roles with their title,
  permits, toolsets, budget.
- `noded --agents <manifest>` (default: none; the acceptance suites and the
  demo path are unchanged): at daemon start, the node boots **one
  long-running VM per role** — each with `plaza.role=<role>
  plaza.keepalive=1`, its own guest CID (`--agents-cid-base`, default 100,
  plus the role's `slot`: the manifest's `"slot"` field, unique per role, so a
  role keeps its CID when roles are added; a manifest with no slots numbers the
  roles alphabetically),
  `--agents-image` memory (default 128 MiB — the validated per-VM scale
  figure), 1 vCPU, no beats — and registers them as a run (state
  `running`, never `done` until stopped). The register's VM rows carry the
  role (a new field) so the console can show who is who.
- The fleet is the service's initial setup: the owner sees 21 connected
  agents on the hub at first boot, each role's VM live and inspectable.

## 6. Acceptance impact

- `tests/acceptance/**` is untouched and must stay green: every suite drives
  the v1 demo path (no keepalive, no roles) — the additions are opt-in.
- A new acceptance case (later slice) should cover: a keepalive guest
  reconnecting across a hub restart and the hub's idle reap.
