# E2B-compatible sandboxes

Status (2026-10-15): built. The hub serves the E2B REST surface under `/e2b`, the node's gateway (`vsockd`) relays the
SDK's sandbox requests into the guest, and the in-VM daemon (`agent-harness/src/envd`) answers as E2B's `envd`. Tested
end to end on Linux x86_64 with KVM.

E2B's SDKs (Python and JavaScript) create and drive sandboxes: small VMs for running code. The plaza offers the same
API, so the SDKs work unchanged against the hub. A plaza sandbox is a governed sandbox. Its laws, egress rules, STOP ALL,
transcript and audit all apply to it, as they do to an agent. It is a plaza VM, not a third-party service.

The protocol definitions (the REST paths, the envd message shapes and the Connect framing) follow E2B (Apache-2.0,
Copyright E2B). The code says where each comes from (`crates/agent-harness/src/envd/mod.rs`).

## Layers

| layer | where | what it does |
|---|---|---|
| hub | `crates/apid/src/e2b.rs` | Keys, the REST surface, the record of each sandbox, its end time, its snapshots, the reaper. |
| node | `crates/vsockd/src/e2b.rs` | The gateway on `127.0.0.1:49982`: relays each SDK request into the guest's daemon. |
| node | `crates/noded` | Runs each sandbox as an ordinary run with a sandbox VM (`sandbox: true`), with a per-run workspace and warm templates. |
| guest | `crates/agent-harness/src/envd` | The sandbox daemon, on vsock port 49983. Serves `process`, `filesystem`, `/files`, `/init` and `/health`. |

A sandbox VM is started with `plaza.sandbox=1`. The harness runs no agent session in it. The guest keeps no network
device: its outbound traffic goes through the egress proxy and the plaza's laws, as an agent's does.

## Setup

Point the SDK at the hub and the gateway, and give it a key:

```sh
export E2B_API_URL=http://<hub>/e2b
export E2B_API_KEY=e2b_...
export E2B_SANDBOX_URL=http://127.0.0.1:49982
```

`E2B_SANDBOX_URL` is the node's gateway. The SDK sends every sandbox request to that one loopback address, and names
the sandbox in the `E2b-Sandbox-Id` header (or in a `Host` of the form `49983-<id>.<anything>`). The gateway's port is
`vsockd --e2b-port` (default off), and `apid --e2b-gateway-port` names it for the hub (default 49982).

## Keys

The owner makes keys. The hub stores only each key's SHA-256, in `e2b-keys.json` (mode 0600).

| route | who | what |
|---|---|---|
| `POST /v1/e2b/keys` `{label}` | owner | Creates a key. The reply carries the key once: `{id, label, key, created_ms}`. |
| `GET /v1/e2b/keys` | owner | Lists the keys (`{id, label, created_ms}`). |
| `DELETE /v1/e2b/keys/{id}` | owner | Revokes a key. |

Every `/e2b` route takes a key, as `X-API-Key: e2b_...` or as `Authorization: Bearer e2b_...`. The owner token alone is not
a key, so a console session cannot drive a sandbox through the SDK by accident.

## The REST surface

These routes are implemented, with the SDK's own shapes (`NewSandbox`, `Sandbox`, `ListedSandbox`, `SandboxDetail`,
`SnapshotInfo`):

| route | reply |
|---|---|
| `POST /e2b/sandboxes` | `201 Sandbox`. The `templateID` names the base image (`base` by default; any template name is accepted). |
| `GET /e2b/sandboxes` | `[ListedSandbox]`: the running ones. |
| `GET /e2b/v2/sandboxes?state=running,paused` | `[ListedSandbox]`, for the states asked. |
| `GET /e2b/sandboxes/{id}` | `SandboxDetail`. |
| `DELETE /e2b/sandboxes/{id}` | `204`. |
| `POST /e2b/sandboxes/{id}/timeout` `{timeout}` | `204`: sets the end time. |
| `POST /e2b/sandboxes/{id}/refreshes` `{duration}` | `204`: extends the end time. |
| `POST /e2b/sandboxes/{id}/pause` | `204`. |
| `POST /e2b/sandboxes/{id}/resume` `{timeout?}` | `201 Sandbox`. |
| `POST /e2b/sandboxes/{id}/connect` `{timeout}` | `201 Sandbox` when it was woken, `200` when it was already running. |
| `POST /e2b/sandboxes/{id}/snapshots` `{name?}` | `201 SnapshotInfo`. |
| `GET /e2b/snapshots` | `[SnapshotInfo]`. |

Errors are E2B's `{code, message}`, with apid's machine-readable `error_code` beside them.

**Not implemented:** `StreamInput` (the process input stream; the unary input calls are), metadata filtering on the
sandbox lists, starting a sandbox from a snapshot as a template, and a cold resume.

The owner's console routes for the same records: `GET /v1/e2b/sandboxes` lists every sandbox, with its CID and run, and
`DELETE /v1/e2b/sandboxes/{id}`, `POST .../{id}/pause` and `POST .../{id}/resume` end, pause or resume one.

## The in-VM daemon

The daemon serves E2B's `envd` on vsock port 49983 (`PORT` in `envd/mod.rs`). The gateway relays the SDK's HTTP/1.1
requests to it over the VM's vsock device.

- **Connect RPC with the JSON codec.** A unary call is a `POST /<package>.<Service>/<Method>` with `application/json`.
  A server-streaming call is one enveloped message (`application/connect+json`) and an end-of-stream envelope.
  Field names are protobuf's JSON names, `bytes` are base64, 64-bit integers are strings, and enums are their names.
- **Services.** `process.Process/*` (start, list, connect, input, signals, PTY) and `filesystem.Filesystem/*` (stat,
  list, make directory, move, remove, watch).
- **REST.** `GET /health` (`204`), `GET` and `POST /files` (download and upload), and `POST /init`.
- **`/init`** merges `envVars` into the context every later process gets, sets the clock when `timestamp` (RFC 3339) is
  given, and mixes `entropy` (64 hex digits, a plaza extension) into the kernel pool. The hub sends fresh entropy on
  every `/init`, so no two sandboxes restored from one template share the template's random state (see
  [`vm-snapshots.md`](vm-snapshots.md), "Clock and entropy"). `accessToken`, `defaultUser` and `defaultWorkdir` are
  accepted and not applied: the sandbox's user and home are fixed.

## The gateway

`vsockd`'s gateway (`crates/vsockd/src/e2b.rs`) is the node's end of the SDK's traffic. For each request it:

1. Reads the sandbox ID and asks apid which VM it is (`GET /internal/e2b/sandboxes/{id}`, which gives the CID and the
   envd access token).
2. Checks the sandbox's access token, and that the port is the one the daemon serves.
3. Opens the VM's vsock muxer socket (`{uds-dir}/vm{cid}`), sends `CONNECT 49983`, waits for `OK 49983`, and relays
   the request.

One connection serves one request. The guest is sent `Connection: close`, and the client is closed once the guest has
closed, so the SDK reconnects for its next call. One audit line is printed per request: the sandbox, method, path
(without its query), status, bytes each way and milliseconds. No header value is ever printed, since the access token
travels in one.

## Governance

A sandbox is a run, so the owner's controls reach it the same way they reach an agent.

- **Laws and egress.** The sandbox VM has the role `sandbox` for the laws and for the egress credential rules (see
  [`egress-credentials.md`](egress-credentials.md)). Its outbound connections go through the same egress proxy and
  relay as an agent's.
- **STOP ALL.** While the plaza is held, every sandbox is ended, and creates, wakes and connects are refused with 409.
- **Transcript and bus.** Each sandbox event is a transcript entry under the sandbox's CID, and an `e2b` event on the
  bus.
- **The reaper.** Every 5 seconds the reaper ends a running sandbox past its end time, or pauses it when it auto-pauses.
  The sandbox records load when the hub starts, and the reaper starts with them.

## Measured

Measured on this machine (x86_64, KVM, 1 vCPU, 256 to 512 MiB guests). p50 unless noted.

| operation | result |
|---|---|
| Sandbox ready from a template | ~200 ms (the first one, which builds the template: ~740 ms) |
| Pause | ~80 ms |
| Resume | ~210 ms |

The VM underneath is the one [`vm-snapshots.md`](vm-snapshots.md) measures: a sandbox is a restore of its template, and a
pause is a snapshot.

See also [`architecture.md`](../architecture.md), "`apid` HTTP API", for the hub's route families.

## In the consoles

The E2B API screen of the web console and the desktop app mints keys (shown once, with the three environment lines an
SDK needs), revokes them, and lists the sandboxes with Pause, Resume and End; the CLI has `agentplaza e2b keys
list|create|revoke`, `agentplaza e2b sandboxes` and `agentplaza e2b kill|pause|resume`.
