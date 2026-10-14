# Plan: the hub, its nodes, and a desktop app at parity

Owner's decisions (2026-10-12): the hub is today's apid (control plane, state in Postgres, laws,
console, model gateway, git, files) plus node management, hostable anywhere. A node is a machine
running noded (VMs) and its own agent gateway (vsockd), dialling out to the hub. The desktop app
stays native (iced) and is ported to full parity with the web console. Join codes are the only way
a node enters a hub, and they are hardened.

> Status (checked against the code): built, with the gaps noted under each section. Crates: apid (the
> hub), apz-link (the node link), apz-trust (tokens, keys, signatures), noded and vsockd (the node's own
> daemons), apps/desktop (the iced app).

## Topology

```
              owner (web console / desktop app)
                          │ HTTPS, owner token
                          ▼
   ┌──────────────── hub (apid) ─────────────────┐
   │ state (Postgres) · laws · git · files · LLMs│
   │ node registry · join codes · command queues │
   └──────────────────────▲──────────────────────┘
                          │ HTTPS (pinned cert), every request signed by the node's key
        ┌─────────────────┼──────────────────┐
     node A (local)    node B             node C            (each: apz-link + noded + vsockd + VMs)
```

- **Local mode** ("Start locally"): hub and one node on this machine, over loopback — what runs
  today. Loopback needs no owner token.
- **Distributed mode** ("Start distributed"): either *host a hub* (bind to an address, TLS on,
  owner token required, issue join codes) or *join a hub* as a node (paste a join token).

## Node identity and join codes (hardening)

Status: built (`crates/apz-trust/src/token.rs`, `crates/apid/src/nodes.rs`). Codes are `apzj1.` tokens that
last at most 60 minutes (default 15). Redemption is limited to 5 a minute per source address and 3 tries per
code, and the hub stores only the secret's SHA-256.

- **Join token**: `apzj1.<base64url(json)>` with `{u: hub URL, f: SHA-256 of the hub's TLS
  certificate (base64url), s: 128-bit random secret (base64url), x: expiry (unix s)}`.
  - Issued only by the owner, shown once.
  - Valid ≤ 60 minutes (default 15), single use, and revocable before use.
  - The hub stores only `SHA-256(s)`, compared in constant time.
  - Redemption is rate-limited per source address (5/min) and per code (3 tries); failures are
    logged.
- **Pinning**: the node connects only if the hub's certificate hashes to `f`. A self-signed hub
  certificate is therefore safe.
- **Node key**:
  - On join the node generates an Ed25519 key pair locally. The private key never leaves the node
    (stored 0600). The hub records the public key and assigns a node id and a disjoint guest-CID
    range (so agent ids stay unique across the plaza).
- **Every node request is signed**: headers `x-apz-node`, `x-apz-ts`, `x-apz-nonce`,
  `x-apz-sig = Ed25519(method \n path \n ts \n nonce \n hex(SHA-256(body)))`.
  - The timestamp must be within ±60 s.
  - Nonces are remembered for 5 minutes (no replay).
  - The node must not be revoked, and an agent route only accepts cids in the node's range (the range
    check on agent routes is not verified here).
  - Built: the headers are `x-apz-node`, `x-apz-ts`, `x-apz-nonce` and `x-apz-sig` (`apz-trust/src/node.rs`).
- **Revocation**: the owner revokes a node in the console (`POST /v1/nodes/{id}/revoke`); its signatures
  stop working at once, and its agents go offline. Built.
- **Owner auth**:
  - Off loopback, every owner route needs `Authorization: Bearer <owner token>` or the session cookie a
    sign-in gave (built, `crates/apid/src/hub_auth.rs`).
  - The token is made at the hub's first start and written to `<state>/owner-token` (mode 0600); the hub
    stores only its hash.
  - The web console has a sign-in screen. It trades the token for an HttpOnly, Secure, SameSite=Strict
    session cookie and does not keep the token (this replaces the sessionStorage design).
  - Tokens can be rotated (`POST /v1/hub/owner-token/rotate`).
- **TLS**:
  - Built as `apid --tls`: the hub serves HTTPS with a self-signed certificate made on first start
    (`<state>/tls/cert.pem`, `key.pem`). The option to use a certificate the owner gives is not verified.
  - `GET /v1/hub/identity` gives its name, version and certificate fingerprint (built).

## The node link (apz-link)

Status: built. `join.rs` checks the pin and redeems the token, `run.rs` long-polls the command queue and posts
results and heartbeats, and `forward.rs` carries agent traffic to the hub's `/internal/…` routes. A node's
commands and results go through `GET /v1/nodes/{id}/commands`, `POST /v1/nodes/{id}/results` and
`POST /v1/nodes/{id}/heartbeat` (`crates/apid/src/nodes.rs`).

One small daemon per node:
- `apz-link join <token>`: checks the pin, generates the key, redeems the token, stores identity.
- `apz-link run`:
  - **Command channel:** long-polls the hub's command queue (`GET /v1/nodes/{id}/commands?wait=25`,
    signed) and runs each command against the local noded / vsockd (spawn runs, per-VM restart /
    wipe / stop, send a host message to an agent, stats), posting results back.
  - **Agent traffic:** serves vsockd's `--apid` on loopback and forwards each agent request to the
    hub's `/internal/…` routes over pinned TLS, signed.
  - **Heartbeat:** every poll is one, carrying the node's machine summary.

## The hub's fleet routing

Status: built. `crates/apid/src/fleet.rs` maps a CID to its node (the hub's own machine is `n0`, reached over
loopback; a joined node gets a 100-CID range from 1000 up) and routes remote calls through the node's command
queue, with a 30 s answer limit. Per-node routes are under `/v1/nodes/{id}/…`. The fleet module's header says
every agent-gateway and noded call goes through it; the call-site coverage is not checked here.

apid no longer assumes one local noded / vsockd:
- **`fleet` module:** maps cid → node (by CID range) and node → transport. Local node:
  direct HTTP as today. Remote node: its command queue.
- **Call sites:** every `app.hub.*` / `app.noded.*` call goes through `fleet` (send to agent,
  spawn on a node, per-VM controls, stats).

## Desktop app at parity

Status: built as an iced 0.14 app (`apps/desktop`, sources in `apps/desktop/src/console/`). The startup
chooser is `console/boot.rs`, and the remembered choice is kept in `~/.config/agentplaza/desktop.json`.
Service start and stop is in `console/services.rs`. Feature-level parity with the web console is not
checked here.

Native iced, same design language as the web console (`apps/web/DESIGN.md`).
- **Startup chooser:** *Start locally* (starts Postgres/Forgejo/Garage if present, apid, vsockd,
  noded on this machine), *Host a hub*, *Join a hub* (paste a token; starts apz-link, noded,
  vsockd), *Connect to a hub* (URL + owner token, console only).
  - The choice is remembered.
  - Running local services are listed, with start/stop.
- **Screens:** one module per screen (`apps/desktop/src/console/screens/<name>.rs`) with its own state,
  messages, load, update and view, registered in one table (`NAV` in `screens/mod.rs`). The modules for the
  parity list below exist. The shell is the sidebar (icons, resizable,
  rail, hidden), the top bar (STOP ALL) and the held banner.
- **Parity list:**
  - Overview, Agents (+ agent detail: transcript, chat, model + effort, harness, permissions,
    memory, profile, restart / wipe / retire), Projects, Tasks, Git, Library, Approvals (incl.
    agent proposals), Floor;
  - Society, Conversations, Board, Council, Treasury, Rules;
  - Models, Tools, Machines (+ Nodes, join codes);
  - the agent wizard, the model picker.
