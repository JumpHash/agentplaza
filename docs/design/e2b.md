# E2B-compatible sandboxes

Status (2026-10-15): built. The hub serves the E2B REST surface under `/e2b`, the node's gateway (`vsockd`) relays the
SDK's sandbox requests into the guest, and the in-VM daemon (`agent-harness/src/envd`) answers as E2B's `envd`. Tested
end to end on Linux x86_64 with KVM, and with the E2B Python SDK 2.56.0 (see [Verified with the SDK](#verified-with-the-sdk)).

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
| `POST /e2b/sandboxes` | `201 Sandbox`. The `templateID` names a built template (its id, name or `name:tag`; see [Template builds](#template-builds)), a snapshot (see [Snapshots and templates](#snapshots-and-templates)), or else the base image (`base` by default; any other name is accepted for it). |
| `POST /e2b/v2/sandboxes` | `201 Sandbox`. The create the current SDKs call. The same as v1, and always secured. |
| `GET /e2b/sandboxes` | `[ListedSandbox]`: the running ones, filtered by `metadata`. |
| `GET /e2b/v2/sandboxes?state=running,paused` | `[ListedSandbox]`, for the states asked, filtered by `metadata`. |
| `GET /e2b/sandboxes/{id}` | `SandboxDetail`. |
| `DELETE /e2b/sandboxes/{id}` | `204`. |
| `POST /e2b/sandboxes/{id}/timeout` `{timeout}` | `204`: sets the end time. |
| `POST /e2b/sandboxes/{id}/refreshes` `{duration}` | `204`: extends the end time. |
| `GET /e2b/sandboxes/{id}/metrics` | `[SandboxMetric]`: one sample, now, from the daemon's `GET /metrics` (CPU busy since the last sample, memory, the home disk). `[]` while paused. |
| `GET /e2b/sandboxes/metrics?sandbox_ids=a,b` | `{"sandboxes": {id: SandboxMetric}}` for the running ones (at most 100). |
| `GET /e2b/v2/sandboxes/{id}/logs` `?cursor&limit&direction&level&search` | `{"logs": [SandboxLogEntry]}`: the daemon's log (each process's start, with its command line, and end, with its exit code), filtered and paged as asked. The daemon keeps its last 1000 entries. |
| `GET /e2b/sandboxes/{id}/logs` `?start&limit` | `{"logs": [{timestamp, line}], "logEntries": [...]}`, the same entries. |
| `PUT /e2b/sandboxes/{id}/network` | `204`. Replaces the sandbox's `allowOut`, `denyOut` and `allow_internet_access` (an omitted list is cleared; omitted access is on). Applies to the next connection: vsockd forgets the decisions it kept for the sandbox. |
| `POST /e2b/sandboxes/{id}/pause` `{memory?}` | `204`. With `memory: false`, the VM stops with no memory image (a cold resume). |
| `POST /e2b/sandboxes/{id}/resume` `{timeout?, memory?}` | `201 Sandbox`. With `memory: false`, the sandbox boots from its disk. |
| `POST /e2b/sandboxes/{id}/connect` `{timeout}` | `201 Sandbox` when it was woken, `200` when it was already running. With `memory: false`, it boots from its disk. |
| `POST /e2b/v2/sandboxes/{id}/connect` `{timeout?}` | As the v1 connect. The timeout is optional, 300 s by default. |
| `POST /e2b/sandboxes/{id}/fork` `{timeout, count}` | Takes a checkpoint now, and starts `count` sandboxes (1 to 8) from it, each with its own access token. The reply has one entry per fork: `{"sandbox"}` or `{"error"}`. |
| `POST /e2b/sandboxes/{id}/snapshots` `{name?, memory?}` | `201 SnapshotInfo`. With `memory: false`, the disk only (see below). |
| `GET /e2b/snapshots` | `[SnapshotInfo]`. |
| `DELETE /e2b/templates/{id}` | `204`: deletes a built template (by id or name) with its builds' snapshots, or else a snapshot (its id, `<sandbox>:<checkpoint>`, or its unique name). `404` when unknown; `409` while a build of the template runs. |

Errors are E2B's `{code, message}`, with apid's machine-readable `error_code` beside them.

The lists take a `metadata` filter, as the SDK sends it (`metadata=k1%3Dv1%26k2%3Dv2`). Each key and value is percent-decoded
again.

**Network rules.** `network.allowOut` and `network.denyOut` at create, and `PUT .../network` later, narrow or open a
sandbox's egress. The proxy sees the host each connection names, so `allowOut` takes a name, a `*.` wildcard (any
subdomain, not the apex), an IP or a CIDR block, and `denyOut` an IP or a CIDR block; `0.0.0.0/0` denies every host. An
allowed entry wins over a denied one, as in E2B. Neither overrides the laws or the hub's blocklist of private ranges.

**Refused, not ignored:** `volumeMounts` (the plaza's volumes are read-only and given by role), `network.rules`
(credentials are added only by the owner's egress rules), `network.egressProxy` and `network.maskRequestHost`.

The owner's console routes for the same records: `GET /v1/e2b/sandboxes` lists every sandbox, with its CID and run, and
`DELETE /v1/e2b/sandboxes/{id}`, `POST .../{id}/pause` and `POST .../{id}/resume` end, pause or resume one.

## Snapshots and templates

A snapshot is a checkpoint of a sandbox, with its memory. A snapshot with `memory: false` (the SDK's
`mode="filesystem"`) flushes the guest and keeps its disk only (noded's disk-only checkpoint): a sandbox made from it
boots cold with the files, without the source's processes.

- A create whose `templateID` is a snapshot id, or a name that exactly one snapshot has, forks that checkpoint. The new
  sandbox has the source's environment variables, under the request's.
- A snapshot outlives its sandbox. A killed sandbox that has snapshots is retired: its VM is stopped, it has no memory
  image, and every sandbox route hides it. It stays retired until its last snapshot is deleted, and then its run ends.

**Cold resume.** `pause` with `memory: false` (the SDK's `mode="filesystem"`) flushes the guest (`POST /sync` on the
daemon, a plaza extension) and stops the VM with no memory image. The sandbox resumes cold: `resume` and `connect` with
`memory: false` boot it from its disk. The daemon starts fresh, so the hub sends the sandbox's environment again. The
record keeps `env_vars`, and the file is mode 0600.

## Template builds

The SDKs' `Template.build` (the v2 build API) is served. A build runs its steps in a sandbox of its own and keeps the
result as a memory snapshot; a sandbox created from the template forks that snapshot, so it starts warm, with what the
steps installed and what the start command left running.

| route | reply |
|---|---|
| `POST /e2b/v3/templates` `{name, tags?, cpuCount?, memoryMB?}` | `202 {templateID, buildID, names, aliases, tags, public}`. A new build, waiting for its steps; a new template when no template has the name. `name:tag` names a tag; no tag is `default`. 1 to 8 vCPUs, 128 to 16384 MiB (1 and 512 by default). `minFreeDiskMb` is accepted and not applied. |
| `GET /e2b/templates/{id}/files/{hash}` | `201 {present}`, or `{present: false, url, headers}`: a link to `PUT` the step's archive to, good for one upload within the hour. Its token travels in `headers`, which the SDK sends back. |
| `PUT /e2b/templates/{id}/files/{hash}` | `200`: the archive (tar or tar.gz, at most 512 MiB), kept by its hash under `<state>/e2b-template-files`. |
| `POST /e2b/v2/templates/{id}/builds/{build}` `TemplateBuildStartV2` | `202`: the steps are checked and the build starts. `429` while two builds run. |
| `GET /e2b/templates/{id}/builds/{build}/status?logsOffset&limit` | `TemplateBuildInfo`: `waiting`, `building`, `ready` or `error`, the build's log from `logsOffset` (100 entries a page), and on error the `reason` with its step (`base`, the step's number, or `finalize`). |
| `GET /e2b/templates/aliases/{name}` | `{templateID, public: false}`, or `404` (the SDK's `Template.exists`). |
| `GET /e2b/templates/{id}/tags` | `[{tag, buildID, createdAt}]`. |
| `POST /e2b/templates/tags` `{target, tags}` | `201 {buildID, tags}`: the tags move to the ready build `target` (`name:tag`) names. |
| `DELETE /e2b/templates/tags` `{name, tags}` | `204`: the tags leave the template's builds. |

- **Base.** `fromImage` `e2bdev/base` (the SDKs' default) is the plaza's base image. No other image is pulled: another
  `fromImage`, or a `fromImageRegistry`, is refused with `400`. `fromTemplate` builds on a built template's snapshot,
  with its environment, user and working directory.
- **Steps**, in order: `RUN` (`/bin/bash -l -c`, or `/bin/sh` without bash, as the step's user or the current one, with
  the build's environment, in its working directory); `COPY` (the uploaded archive is unpacked in the sandbox and copied
  by Docker's rules: a directory's contents into the destination; a file to it, or into it when it ends in `/`, is a
  directory, or the source is a pattern; owned by the step's user or the current one, files given the step's mode);
  `ENV`; `WORKDIR` (made when missing, owned by the current user); `USER` (`user` or `root`, the guest's two users). A
  step's output lines are the build's log; a step that exits non-zero fails the build. Each step may run 30 minutes.
- **Start and ready.** The start command runs in the background, its output appended to `/tmp/e2b-start-cmd.log`. The
  ready command is retried every second until it exits 0, for at most 10 minutes. Then the daemon is given the build's
  environment, user and working directory (`/init`'s `envVars`, `defaultUser`, `defaultWorkdir`), and the snapshot is
  taken. The build sandbox is killed and kept, retired, for that snapshot.
- **Picking a build.** `name:tag` is the ready build with that tag. A bare name (or the template id) is the `default`
  build, or the newest ready build when no build has that tag. When a build is ready the tags it asked for move to it,
  and an older ready build that holds no tag any more is deleted with its snapshot. No layer cache: every build runs
  every step.
- **Records.** Templates are kept in `<state>/e2b-templates.json`. Build logs are kept in memory; a build the hub was
  running when it stopped is marked failed at the next start, and its sandbox is ended.


The daemon serves E2B's `envd` on vsock port 49983 (`PORT` in `envd/mod.rs`). The gateway relays the SDK's HTTP/1.1
requests to it over the VM's vsock device.

- **Connect RPC with the JSON codec.** A unary call is a `POST /<package>.<Service>/<Method>` with `application/json`.
  A server-streaming call is one enveloped message (`application/connect+json`) and an end-of-stream envelope.
  Field names are protobuf's JSON names, `bytes` are base64, 64-bit integers are strings, and enums are their names.
- **Services.** `process.Process/*` (start, list, connect, input, StreamInput, signals, PTY) and `filesystem.Filesystem/*`
  (stat, list, make directory, move, remove, watch).
- **Input.** `StreamInput` takes stdin as a client stream. The body is buffered, so the input is delivered when the client
  finishes the stream. A command runs as the SDKs' `/bin/bash -l -c`, or under `/bin/sh` when the guest has no bash. The
  user name `user` (E2B's default) means the sandbox user.
- **Version.** Sandboxes report envd `0.5.7`. The SDK gates command stdin, the default user, `CloseStdin` and
  octet-stream uploads on that version. All four are served.
- **REST.** `GET /health` (`204`), `GET` and `POST /files` (download and upload), and `POST /init`.
- **`/init`** merges `envVars` into the context every later process gets, sets the clock when `timestamp` (RFC 3339) is
  given, and mixes `entropy` (64 hex digits, a plaza extension) into the kernel pool. The hub sends fresh entropy on
  every `/init`, so no two sandboxes restored from one template share the template's random state (see
  [`vm-snapshots.md`](vm-snapshots.md), "Clock and entropy"). `defaultUser` (`user` or `root`) and `defaultWorkdir`
  (an absolute directory) set who a request that names no user runs as, and where a process that names no `cwd` starts;
  a template build sets them. `accessToken` is accepted and not applied (the gateway checks the token).
- **`PWD`.** A process gets `PWD` set to the directory it starts in, so a shell's `pwd` shows `/home/user/...`, not the
  volume `/home/user` links to.

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

- **Laws and egress.** The sandbox VM has the role `sandbox` in the laws. Its outbound connections go through the same
  egress proxy and relay as an agent's, but the proxy never intercepts sandbox egress for credentials (see
  [`egress-credentials.md`](egress-credentials.md)). A sandbox is its own party.
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
pause with memory is a snapshot. A pause without memory keeps only the disk.

See also [`architecture.md`](../architecture.md), "`apid` HTTP API", for the hub's route families.

## Verified with the SDK

Checked with the real E2B Python SDK 2.56.0 against a scratch hub: `E2B_API_URL` at apid's `/e2b`, and `E2B_SANDBOX_URL`
at vsockd's gateway.

| step | result |
|---|---|
| create | ~750 ms |
| commands with env vars, files write/read/list, stdin | pass |
| metadata list, match and miss | pass |
| create_snapshot | ~65 ms |
| create from the snapshot | ~230 ms. The child reads the file and the env. |
| fork ×2 | ~490 ms |
| pause and connect from memory | ~290 ms |
| pause in filesystem mode, then connect | cold boot ~670 ms. The file is on disk. |
| kill with a snapshot kept, list_snapshots, create from that snapshot after the source was killed | pass |
| delete_snapshot | `true`, then `false` |
| get_metrics | one sample: 1 CPU, its busy share, memory and disk |
| create_snapshot in filesystem mode, create from it | cold boot ~670 ms. The child reads the file. |
| update_network: internet off, `allow_out` one host | that host connects, another gets 403 |
| update_network: `deny_out` 0.0.0.0/0, then cleared | refused, then open again, at once |
| logs (v2, backward, search) | the probe's command lines |

47 of 47 steps pass.

Template builds, with the same SDK (`Template`, `Template.build`, `Sandbox.create` from the template):

| step | result |
|---|---|
| build: `COPY` a file, a directory, a pattern (as root, mode 0600); `RUN`; `ENV`; `WORKDIR`; `RUN` as root; `USER`; a start command with a ready command | ready in ~2.5 s; the `RUN` output is in the build's log |
| `Template.exists` for the name, and for an unknown name | `true`, `false` |
| a sandbox from `name:v1` | ~270 ms. Its commands start in the `WORKDIR`, as `user`, with the `ENV`; the copied tree, owners and modes are as asked; the start command's loop is still running. |
| a sandbox from the bare name | the newest ready build |
| `assign_tags`, `get_tags`, create by the new tag, `remove_tags` | pass |
| `from_template` of it, then a sandbox of that | starts in the base's working directory, with its files and environment |
| a step that exits 3 | `BuildException`: the command exited with 3 |
| `from_image("python:3.12")` | `BuildException`: no other image is pulled |
| two builds of one name | the template id stays; the first build's snapshot is deleted; the name picks the second |
| `DELETE /templates/{name}` for each | `204`; no snapshot is left |

26 of 26 checks pass.

## In the consoles

The E2B API screen of the web console and the desktop app mints keys (shown once, with the three environment lines an
SDK needs), revokes them, and lists the sandboxes with Pause, Resume and End; the CLI has `agentplaza e2b keys
list|create|revoke`, `agentplaza e2b sandboxes` and `agentplaza e2b kill|pause|resume`.
