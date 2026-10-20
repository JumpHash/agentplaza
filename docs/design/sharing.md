# Sharing artifacts

A shared artifact has a public link that opens outside the owner's hub. The owner makes the link, and the hub keeps
the record of it. An agent may ask for a share, but only the owner decides. This document covers the link options,
the hub's store and owner API, the public routes, the agentplaza.io relay that carries requests to a hub that sits
behind a firewall, the agent request flow, and the consoles. How artifacts are made and shown is in
`docs/design/artifacts.md`.

## 1. Links

An artifact is private until its owner shares it. Once it has a share, anyone with the link can open it. A share
is a record in the hub store (section 2) with these options:

| Option | Meaning |
|---|---|
| Version | A pinned artifact version, or none to serve the latest. The owner can change it, and clearing it serves the latest again. |
| Password | Optional. The hub stores an argon2 hash (a PHC string), never the password. A visitor enters it once and gets an unlock token. |
| Expiry | Optional. A Unix-millisecond time after which the link stops working. |
| Downloads | Whether visitors may download the artifact's files and exports. Off by default. |

The share's id is 32 lowercase hex characters, which is 128 random bits. The id is the token in the link, so there is
no separate secret.

Links in the Share dialog (`apps/web/src/ui/share.ts`, the links row at line 252):

| Button | Address |
|---|---|
| Copy link | `https://agentplaza.io/artifact/{share}`, served through the relay (section 3) |
| Copy network link | `{hub}/artifact/{share}`, served by the hub itself, for visitors on the owner's network |

The hub returns both addresses in the share's `links` field. The desktop dialog is `apps/desktop/src/console/share.rs`.

Stop sharing revokes the share. The record stays, with `revoked_ms` set, and a revoked link never works again. The
relay is told to release the share, and the artifact is marked not shared.

Activity lists each use of a share (section 2): a view, a download, or a failed unlock, and whether it came through
the network link or the relay.

## 2. The hub: store and owner API

### Records

`crates/apid/src/shares/mod.rs` holds the types and the store. The schema is in
`crates/apid/sql/migrations/0015_shares.sql`, with two tables: `shares` and `share_events`.

| Type | Fields |
|---|---|
| `Share` | `id`, `target` (`kind` and `id`), `version`, `password_hash`, `expires_ms`, `allow_download`, `created_by`, `created_ms`, `revoked_ms`, `title` |
| `Target` | `kind` and `id`. The kinds are `artifact` and `file` (`routes.rs`, line 231). A version applies only to an artifact (line 77). |
| `ShareEvent` | `share`, `at_ms`, `kind` (`view`, `download`, or `unlock_failed`), `via` (`network` or `relay`) |
| `Options` | `version`, `password` (an empty string means none), `expires_ms`, `allow_download` |

The store (`Store`) keeps the shares, the events, and the counts behind one lock, and it writes its files under that
lock (the comment at the top of `Store`). The in-memory event list is capped at `EVENTS_IN_MEMORY` (10,000).

| Method | What it does |
|---|---|
| `create(target, title, opts, created_by)` | Make a share with a fresh id, and hash the password if there is one |
| `change(...)` | Change the password, expiry, downloads, or version |
| `revoke(id)` | Set `revoked_ms` |
| `get`, `list(target)`, `live()` | Read shares. `live()` returns the shares that work now. |
| `access(id, now, unlocked)` | Decide what a request gets: `Ok`, `Missing`, `Revoked`, `Expired`, or `NeedsPassword` |
| `check_password`, `unlock_token`, `token_ok` | Check the password and issue or check the unlock token |
| `record(share, kind, via)`, `events(share, limit)`, `counts(share)` | Log and read the events and the view and download counts |

### Owner routes

`crates/apid/src/shares/routes.rs` registers the owner's routes. The owner's token applies, as for the other `/v1`
routes.

| Route | Purpose |
|---|---|
| `GET /v1/shares` | List shares, optionally for one target |
| `POST /v1/shares` | Create a share (`version`, `password`, `expires` in days, `allow_download`) |
| `PATCH /v1/shares/{id}` | Change a share. A `version` of `null` clears the pin. |
| `DELETE /v1/shares/{id}` | Revoke the share (stop sharing) |
| `GET /v1/shares/{id}/events` | The activity, at most `MAX_EVENTS` (1,000) entries |

### Public routes

Visitors reach the public routes at `/artifact/...` on the hub, outside `/v1`. They have their own checks and do not go
through the owner's token. `crates/apid/src/shares/public.rs` registers them (lines 40 to 62):

| Route | Purpose |
|---|---|
| `GET /artifact/_assets/{file}` | The viewer's own files |
| `GET /artifact/{share}` and `/artifact/{share}/` | The viewer page (`apps/web/src/viewer.html`, `apps/web/src/viewer.ts`) |
| `GET /artifact/{share}/data` | The artifact's data for the viewer, at the share's version |
| `POST /artifact/{share}/unlock` | Check the password and return an unlock token |
| `GET /artifact/{share}/blobs/{blob}` | A blob's bytes |
| `GET /artifact/{share}/rows` | A dataset's rows, paged |
| `GET /artifact/{share}/export` | An export of a panel (CSV, JSON, or PNG) |
| `GET /artifact/{share}/file` | A file of an HTML artifact, or a file target |
| `GET /artifact/{share}/frame/{panel}` and `.../{path..}` | An HTML panel's frame and its files |

Each handler calls `Store::access` first. A missing, revoked, or expired share answers with no detail, and a share
with a password answers `NeedsPassword` until the visitor unlocks it. Downloads and exports check `allow_download`.
Every handled request records an event (`record`).

Headers and framing come from constants at the top of `public.rs`:

- `FRAME_SANDBOX` is `sandbox allow-scripts allow-popups allow-downloads`, the sandbox on frames.
- `FRAME_ANCESTORS` is `'self' https://agentplaza.io`, which limits which pages may embed a frame.
- `SANDBOXED` is `default-src 'none'; sandbox`, the policy on the non-frame responses.

The same sandbox and origin rules apply to the artifact frames as in `docs/design/artifacts.md`, section 5.

A test in `public.rs` (around line 650) checks that a request to `/artifact/x/data` is public for `GET`, and that
`/v1/settings` is not. So the artifact paths bypass the owner's token, and the `/v1` API never answers them.

## 3. The relay

A hub behind a firewall cannot take inbound connections. The relay lets `https://agentplaza.io/artifact/{id}` reach it
anyway. The relay keeps no artifact content. It keeps the share table (which hub owns which share), the reports, and
each hub's public key. The bytes stay on the hub, and the hub answers each request.

### The Worker

The relay is a second Cloudflare Worker, `agentplaza-relay`, in the agentplaza-landing repository (on this machine,
`/mnt/fast/agentplaza-landing`). It lives in `cloudflare/relay/`:

| File | What it holds |
|---|---|
| `worker.js` | The Worker, and the `HubSocket` Durable Object, one per hub |
| `wrangler.toml` | The routes (`agentplaza.io/artifact/*` and `agentplaza.io/relay/*`), the Durable Object binding, and the KV binding `SHARES` |
| `relay.test.mjs` | Tests for the pure helpers (`node --test cloudflare/relay/relay.test.mjs`) |
| `README.md` | The deploy steps and the protocol |

The site's own Worker, `cloudflare/worker.js`, is separate. KV keys:

| Key | Value |
|---|---|
| `hub:{hub}` | The hub's Ed25519 public key (base64url), written once by `register` |
| `share:{id}` | The hub id that owns the share |
| `blocked:{id}` | The report reason. A blocked share answers 451 until its owner releases it. |

### Keys and signatures

A hub's id is the first 32 hex characters of SHA-256 over its raw 32-byte Ed25519 public key. Keys and signatures are
base64url without padding. A signature covers the text `apz-relay-{kind}` followed by each part, joined with newlines.
A timestamp is Unix seconds and must be within 300 seconds of the relay's clock (`FRESH_SECONDS`).

### Protocol

HTTP endpoints:

| Endpoint | Purpose |
|---|---|
| `POST /relay/register` | A hub registers its key. A hub id that already has a different key gets 409. |
| `GET /relay/connect` (WebSocket upgrade) | A hub opens its one socket. A newer socket replaces an older one, which closes with code 4000. |
| `POST /relay/report` | Report a share (`id` and a reason of at most 500 characters). Writes `blocked:{id}`, tells the owning hub if it is connected, and answers 202. |
| `GET`, `HEAD`, `POST /artifact/{id}` and below | A visitor's request, forwarded to the owning hub |

Messages on the hub's socket are JSON text:

| Message | Direction | Meaning |
|---|---|---|
| `claim` | hub to relay | Take ownership of a share id. The relay writes `share:{id}` and acks. A share owned by another hub is refused as `taken`. |
| `release` | hub to relay | Give up a share. The relay deletes `share:{id}` and `blocked:{id}`. |
| `res` | hub to relay | One chunk of a response: status, headers, a base64 body, and `more`. Chunks are joined in order until `more` is false. |
| `pong` | hub to relay | The answer to a ping. A socket silent for 90 seconds is closed. |
| `req` | relay to hub | A visitor's request: `rid`, `id`, method, `path` after `/artifact/{id}`, query, headers, and body |
| `ping` | relay to hub | Every 30 seconds |
| `reported` | relay to hub | A share was reported |

The answers a visitor gets from the relay:

| Status | When |
|---|---|
| 451 | The share is blocked (reported and not yet released) |
| 404 | No share has this id, or the id is not well formed (16 to 64 characters of `a-z0-9`) |
| 503 | The owning hub is offline, or it does not answer within 25 seconds |
| 429 | More than 240 requests a minute from one visitor address to one share |
| 413 | A request body over 1 MiB |
| 502 | A response over 16 MiB |

When the relay answers by itself (404, 451, or 503), it sends a plain page with no scripts and no hub details.

Response headers: the hub's headers pass through, except the hop-by-hop ones and `content-length`. The relay sets
`X-Robots-Tag: noindex`, `Referrer-Policy: no-referrer`, and `X-Content-Type-Options: nosniff`. It sets
`Cache-Control: no-store` unless the hub sends `x-apz-cache: public`, which becomes `public, max-age=3600`. Only cookies
named `apz_share_*` pass in either direction. A frame (under `/frame/`) gets the sandbox policy as a second
`Content-Security-Policy` value after the hub's own. The request headers that pass to the hub are a short whitelist
(`accept`, `range`, `if-none-match`, and a few more).

### The hub client

`crates/apid/src/shares/relay.rs` is the hub's side.

- The hub's Ed25519 key is in `<state_dir>/relay_key` (`key_path`). The client signs each message with it (`signed`).
- The relay address is `https://agentplaza.io` (`DEFAULT_BASE`). `APZ_RELAY_URL` names another relay.
- `APZ_RELAY=off` turns the client off. `start` checks it, and only one client runs per process.
- The client keeps its socket open while shares are live (`Store::live`). `claim(id)` and `release(id)` (lines 137
  and 142) tell the relay when a share starts or stops.
- Timing: a tick every second, an idle poll every 2 seconds, and a silence limit of 90 seconds. Reconnects back off
  up to 60 seconds (`BACKOFF_MAX_SECS`).
- A relayed request runs through the hub's own router, so the answer is the one the network link gives. A response
  is sent in chunks of up to 512 KiB (`CHUNK_BYTES`), under the WebSocket message limit.

Reports: a reported share answers 451 until its owner releases it. The hub hears the report over the socket
(`reported`), and the owner releases the share from the console.

## 4. Agents ask, the owner decides

An agent never creates a public link itself. It asks, and the owner's approval creates the share. The flow is in
`crates/apid/src/shares/request.rs`:

1. The agent calls `POST /internal/v1/agents/{cid}/gateway/artifacts/{id}/share` with a reason (at most 500
   characters, `MAX_REASON`) and an expiry (at most 365 days, `MAX_DAYS`).
2. `share_route` checks the artifact and submits an approval of kind `artifact.share` (`approvals::submit`). The
   approval's detail names the artifact, its title, and the approval id.
3. The owner decides the item in Approvals. `approvals.rs` (line 507) sends the decision to `on_decided`.
4. Approving calls `on_decided`, which creates the share with the owner's options, marks the artifact shared, and
   claims the share with the relay. Rejecting creates nothing.
5. The decision is reported back to the agent's transcript. The agent sees the outcome and the artifact's title, and
   never receives the link.

The law `artifact.share` (`crates/apid/src/laws.rs`, line 1712) gives the action its name in the laws' text, so a law
can refuse a share request before it reaches the owner.

## 5. The consoles

| Console | File | What it does |
|---|---|---|
| Web | `apps/web/src/ui/share.ts` | The Share dialog: the options, the two links with their copy buttons, Stop sharing, and the activity |
| Desktop | `apps/desktop/src/console/share.rs` | The same dialog, with the same options and links |

The Share dialog is opened from the artifact screen (web `#/artifact/<id>`, desktop `Id::Artifact`, see
`docs/design/artifacts.md`, section 4). Both dialogs use the owner's `/v1/shares` routes, and both show the same
events from `GET /v1/shares/{id}/events`.
