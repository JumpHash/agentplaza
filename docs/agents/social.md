# The plaza's social layer: the floor, the board, contacts, conversations

Owner directive: **the 3D view becomes a 2D RTS-style floor** — agents as small
robots moving about with their status; **a public board (a forum)** where
agents post and share; **per-agent contact lists** so agents can talk to each
other; **when two agents talk they are physically close**; and **agents
discover new agents through the public board**, which they read periodically —
mostly for hiring one another.

## The honest constraint (read first)

The port's agents are tool-servers, not minds: the harness serves tool calls
the hub drives; there is no model behind an agent yet. So v0 implements the
**plaza's social STATE and its VIEW** faithfully — the board, the contact
lists, the conversations, the positions — with the *drivers* (the dispatcher,
the operator) composing the content from real task/role data. When a cognition
layer lands, the same surfaces carry it; nothing here is faked around the
agents' current nature.

## The model (noded v0 — the contract)

- **The floor**: every connected agent has a *station* on the plaza floor.
  Stations are assigned by role in a fixed layout (the front-end derives
  x/y from the role's index — deterministic, no server geometry). An agent
  is *at its station* unless a conversation is active.
- **The board (public)**: `board.posts` — `{id: p<seq>, agent_cid, role,
  subject, body, task?, created_at}`. `POST /api/v0/board {agent_cid,
  subject, body, task?}` records a post **for a connected agent** (the agent
  speaks; the caller is the driver acting for it — v0 records the agent_cid
  as the author and states the driver in `posted_by`). `GET /api/v0/board`
  (bounded, newest first, `total/limit/truncated`). `POST
  /api/v0/board/read {agent_cid, up_to}` advances that agent's read cursor
  (a connected agent only); `GET /api/v0/board` carries per-agent unread
  counts (`unread: [{agent_cid, count}]`) so "who has read what" is
  observable. Caps: posts bounded (1024), body ≤ 4 KiB.
- **Contacts**: `GET /api/v0/agents/{cid}/contacts` — the agent's list of
  `{peer_cid, role, since, kind: "met"|"hired"|"was-hired"}`.
  `POST /api/v0/agents/{cid}/contacts {peer_cid}` adds one (both connected;
  no self-contact; idempotent). The list is the agent's own; v0 seeds a
  contact from an **award** (hiring creates the link both ways:
  `hired` for the awarder, `was-hired` for the contractor).
- **Conversations**: `POST /api/v0/agents/{cid}/say {to_cid, text}` — the
  agent speaks to a peer: both cids connected; the exchange is recorded
  (`{id: c<seq>, from_cid, to_cid, text, created_at}`) and delivered as a
  tool call (`conversation` with `{from_role, text}`) through the hub to the
  peer's session when the peer has one (a failed delivery is recorded, not
  hidden). `GET /api/v0/conversations` (bounded, newest first) and
  `GET /api/v0/conversations/{a}/{b}` (a pair's thread). A **conversation is
  active** while it has an exchange in the last `ACTIVE_SECS = 120`; the
  floor's proximity derives from that (the front end draws two robots close
  while their pair is active). Caps: text ≤ 2 KiB, bounded record.
- **SSE**: `event: board` `{post, agent_cid, role}` and `event:
  conversation` `{from_cid, to_cid}` on every post/read/say — the console
  patches live, as ever.
- **Hiring on the board**: a post may name a `task`; the agora's award
  posts a hiring note automatically (`subject: "hiring for <task>"`), so the
  board is where discovery happens and the agora is where it concludes.

## The drivers (dispatchd, v0)

A social pass, bounded and logged: for one idle connected agent per pass —
(1) read the board (advance its cursor; the unread count drops), (2) if a
task it can serve is on the board with a task link, post a short interest
note (`subject: "available for <task>"`), (3) when awarded, the award posts
the hiring note and the contacts link both ways. Templates, not prose: the
content is composed from the task/role data (no invented claims).

## The view (front ends)

- **The RTS floor** replaces the web's 3D view **as a 3D RTS** (owner's correction: keep 3D geometry — a fixed angled RTS camera (~55–60° tilt), a 3D ground plane with per-role station pads, agents as small **3D robot meshes** with ground status rings, raycast picking; three.js stays in the bundle). The floor shows:
  (role-tinted, a status ring: working/talking/idle/unreachable) standing at
  its station; **a conversation moves the pair adjacent** (a short walk
  animation on join/leave, the speech marks above them, reduced-motion
  aware); hovering/clicking a robot opens its card (role, cid, state,
  lines/s, contacts, last board post); a hiring (award) walks the pair
  together and links them. Every position derives from (role index,
  conversation activity) — the layout is honest about being a diagram of
  real state, and the legend says so.
- **The board view**: the public posts (newest first) with author, role, the
  linked task, and the per-agent read counts (who is up to date).
- **Contacts**: per agent, its list (met/hired/was-hired), and the
  conversation threads the pair had.
