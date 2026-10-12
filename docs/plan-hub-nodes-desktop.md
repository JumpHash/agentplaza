# Plan: the hub, its nodes, and a desktop app at parity

Owner's decisions (2026-10-12): the hub is today's apid (control plane, state in Postgres, laws,
console, model gateway, git, files) plus node management, hostable anywhere. A node is a machine
running noded (VMs) and its own agent gateway (vsockd), dialling out to the hub. The desktop app
stays native (iced) and is ported to full parity with the web console. Join codes are the only way
a node enters a hub, and they are hardened.

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
  - The node must not be revoked, and an agent route only accepts cids in the node's range.
- **Revocation**: the owner revokes a node in the console; its signatures stop working at once,
  and its agents go offline.
- **Owner auth**:
  - Off loopback, every owner route needs `Authorization: Bearer <owner token>`.
  - The token is generated at the hub's first start, stored hashed, and shown once.
  - The web console has a sign-in screen and keeps the token in sessionStorage.
  - Tokens can be rotated.
- **TLS**:
  - The hub generates a self-signed certificate on first start (or uses one the owner gives) and
    serves HTTPS with rustls when not on loopback.
  - `GET /v1/hub/identity` gives its name, version and certificate fingerprint.

## The node link (apz-link)

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

apid no longer assumes one local noded / vsockd:
- **`fleet` module:** maps cid → node (by CID range) and node → transport. Local node:
  direct HTTP as today. Remote node: its command queue.
- **Call sites:** every `app.hub.*` / `app.noded.*` call goes through `fleet` (send to agent,
  spawn on a node, per-VM controls, stats).

## Desktop app at parity

Native iced, same design language as the web console (`apps/web/DESIGN.md`).
- **Startup chooser:** *Start locally* (starts Postgres/Forgejo/Garage if present, apid, vsockd,
  noded on this machine), *Host a hub*, *Join a hub* (paste a token; starts apz-link, noded,
  vsockd), *Connect to a hub* (URL + owner token, console only).
  - The choice is remembered.
  - Running local services are listed, with start/stop.
- **Screens:** one module per screen (`src/screens/<name>.rs`) with its own state, messages,
  load, update and view, registered in one table. The shell is the sidebar (icons, resizable,
  rail, hidden), the top bar (STOP ALL) and the held banner.
- **Parity list:**
  - Overview, Agents (+ agent detail: transcript, chat, model + effort, harness, permissions,
    memory, profile, restart / wipe / retire), Projects, Tasks, Git, Library, Approvals (incl.
    agent proposals), Floor;
  - Society, Conversations, Board, Council, Treasury, Rules;
  - Models, Tools, Machines (+ Nodes, join codes);
  - the agent wizard, the model picker.
