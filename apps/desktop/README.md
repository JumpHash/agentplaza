# AgentPlaza desktop register

An iced + wgpu (WebGPU) desktop dashboard for the node daemon's fleet API: the
fleet as a register — an index of runs on the left, a ruled sheet on the right,
stamps for every state the daemon reports — and, beside it, the plaza's own
**ledger**: the tasks the daemon holds, and the claims, finishes and
withdrawals it accepts on them, its **treasury**: the accounts, their
totals and the double-entry journal that moved them, read only, its
**market**: the calls its roled tasks offer, the bids on them and the award
that closed each, read only, and its **talk**: the conversations between the
plaza's agents — every exchange newest first, the pairs folded into a filter
and one pair's thread when it is narrowed, read only. With
`--hub-url` it also reads the
hub's read-only status surface, so the **roster** (`w`) can show not only who
the register says is running but who is actually **connected to the
orchestrator** right now — the two halves joined by guest CID, never merged.

It is **its own cargo workspace** (`apps/desktop/Cargo.toml` declares
`[workspace]`, with its own `Cargo.lock`): the root workspace's members are
`crates/*` and never include this app.

```
cd apps/desktop
cargo build      # -> target/debug/agentplaza-desktop
cargo run
cargo test
```

## Using it

```
agentplaza-desktop [--url <url>] [--hub-url <host:port>]
                   [--refresh-ms <milliseconds>] [--timeout-secs <seconds>]
```

| | |
|---|---|
| `--url <url>` | the node daemon to read; `http://127.0.0.1:8899` by default |
| `--hub-url <host:port>` | the hub's read-only status surface (`vsockd --status-port`), whose `GET /status` says who is connected to the orchestrator right now; **off** by default |
| `--refresh-ms <n>` | how often a healthy daemon is polled, 250–30000 ms; 2000 by default |
| `--timeout-secs <n>` | how long one call may take, 1–600; 5 by default |
| `-h`, `--help` | the usage text |
| `APZ_DESKTOP_URL` | the same as `--url` |
| `APZ_DESKTOP_HUB` | the same as `--hub-url` |
| `APZ_DESKTOP_REFRESH_MS` | the same as `--refresh-ms` |
| `APZ_DESKTOP_TIMEOUT_SECS` | the same as `--timeout-secs` |

### Keys

| | |
|---|---|
| `r` | the fleet register |
| `m` | the machine (the index also lists it) |
| `f` | the fleet map |
| `w` | the roster: who is running right now, and — with `--hub-url` — who is connected |
| `l` | the ledger: the plaza's tasks, with the claim, finish and withdrawal the daemon accepts on them |
| `y` | the treasury: the accounts, their totals and the journal, read only |
| `u` | the market: the calls the plaza's roled tasks offer, the bids on them and their awards, read only |
| `i` | the floor: the plaza's agents as robots, their conversations walking a pair adjacent, read only |
| `j` | the board: the plaza's public posts, newest first, with the task each links and who has read it, read only |
| `k` | the talk: the plaza's conversation log, newest first, as two identities and the agent's own words, with a pairs filter and a pair's thread, read only |
| `t` | the telemetry: the session's own figures, one observation per poll |
| `?` | the help sheet, which lists every binding (`Escape` or `?` closes it) |
| `Escape` | back to the register (closes the floor's inspector, or widens the talk pane's thread, when either is open) |
| `↑` / `↓` | walk the console's command history; on the floor, cycle the robots; on the talk pane, walk the pairs filter |
| `Enter` | open the picked robot's inspector on the floor, or narrow the talk pane to its picked pair (again widens) |

While the help sheet is up it is *over* the register — a scrim across the
window with the sheet centred on it — and no other key reaches a pane until it
is closed. A focused text box captures its own keys, so typing a console line
never trips a shortcut.

An argument wins over the environment. Only a **loopback** host is accepted,
and only an exact one: `127.0.0.1` (or `localhost`, normalized to it), never
resolved through DNS. Any other host is refused before a socket is opened.
`--hub-url` is held to the same policy and additionally **requires its port**:
the status surface is off unless the daemon was started with `--status-port`,
so there is no default to fall back to and a bare host is refused rather than
pointed at whatever holds the daemon's own port.

```sh
cargo run -- --url http://127.0.0.1:8899
APZ_DESKTOP_URL=http://127.0.0.1:9000 cargo run
cargo run -- --refresh-ms 500
cargo run -- --url http://127.0.0.1:8899 --hub-url 127.0.0.1:19080
APZ_DESKTOP_HUB=127.0.0.1:19080 cargo run
```

## What it does

* polls `GET /api/v0/runs` (and `GET /api/v0/machine` while the machine or the
  map is on the sheet) at `--refresh-ms`, backing off to a 30 s ceiling while
  calls fail, and reports the connection as a stamp: `LIVE`, `STALE`, `DOWN`;
* **follows `GET /api/v0/events`** — the daemon's event stream — the whole time
  it runs: run and VM state changes and each VM's serial deltas arrive as they
  happen, so a run appears, moves to `DONE`/`KILLED` and writes its console
  between polls. The bus has no backlog, so the stream's head is the
  subscription and the polls still supply the initial state; a serial delta is
  merged by its byte offset (append / clip / re-fetch on a gap), so a
  reconnected stream cannot duplicate a byte of a transcript. A stream that
  ends is reconnected with a bounded backoff (250 ms doubling to 30 s, reset by
  any event), and a stream that is down is *not* a daemon that is down: the
  polls remain the source of truth and the stream's events never clear a poll's
  own failure;
* the index column: the fleet entry, the **roster**, the **machine**, the
  **ledger**, the **fleet map**, the **telemetry**, the **treasury**, the
  **market**, the **floor** and the **board**, every
  run, the fleet tally (runs, VMs up, distinct guest CIDs, vsock runs), the
  **guest memory the live VMs have committed** against the host's total as the
  machine reports it (in critical ink, with `· overcommit`, when the fleet has
  been asked for more than the host has), the poll cursor, a **Help** button,
  and the keys;
* the fleet register: every run with its state, **its role** (the role itself
  for a run booted as one role — `noded --agents` boots one long-running VM per
  role of `docs/agents/initial-roles.json` — the count for a run carrying
  several, and a dash for every VM that carries none), live/total VMs, vCPUs,
  memory, vsock base CID and age. The age reads in days once a run is a day
  old (`3d 07h`, never `79h`) and a creation is stamped with its **day**
  rather than a bare time of day whenever that day is not today's: a fleet run
  stays `running` for hours, and a clock with no date, read three days later,
  names no particular instant. The run pane's created/ended facts read the
  same way;
* the **machine** sheet: `GET /api/v0/machine` as a ruled table — name, arch,
  backend, irqchip, class, cpus, memory, the vsock transport mode, when the
  daemon started, and its uptime ticking with the poll;
* the **roster** (`w`): *who is running right now* — and, when `--hub-url`
  names one, *who is connected to the orchestrator right now*. Two surfaces,
  two halves, joined by guest CID and **never merged**:
  * the **register's half**, from `GET /api/v0/runs` and nothing else: every VM
    whose row carries a role, with the role it accepted, the run and VM it is,
    its guest CID, the state right now and the serial rate the activity feed
    has for it (a rate needs two reads of a VM's capture, so a freshly opened
    roster starts at zero and moves as the polls land);
  * the **hub's half**, from `GET /status` on the hub's status surface
    (`vsockd --status-port`), read on its own schedule at the same cadence: a
    header — `hub · 6 cid(s) served · 2 agent(s) connected · 2 session(s)
    active · up 1h 04m` — and, per agent, the session the hub actually holds
    (when it connected, how long ago it last beat, the calls it has served, and
    the role the handshake accepted).

  The two are joined on **guest CID**: an agent the register reports and the
  hub does not is drawn `registered, not connected`, and a session the hub
  holds that no VM row claims is drawn `connected, not registered` — *never*
  folded into a row it does not match, so a session the register cannot account
  for is visible rather than dropped. A CID both halves hold is one row with
  both sides; when the register's role and the hub's accepted role disagree,
  the row keeps the register's word and marks the hub's in the caution's amber.
  A hub-only row draws no button, because there is no run to open. With no
  `--hub-url` the register's half is drawn exactly as it was before there was a
  hub, and the sheet says `hub unknown (no --hub-url)` rather than leaving the
  absence to be guessed at; a hub that cannot be read says `hub unreachable
  (<reason>)`, keeps the register's half working and claims nothing about who
  is connected (a connection list is a statement about *now*), and the hub's
  failures are its own — a second process on a second port that never marks the
  daemon down, never slows its polls and never counts as one of its failures.
  The index's roster entry carries the hub's own count too once it has
  answered (`21 role(s) · 21 agent(s) up · 21 connected`), so the entry the
  operator reads first cannot quietly disagree with the sheet.

  Grouped by role, in the order the roles appear, each group with its own tally
  (`N agent(s) · M up · K connected · L line(s)/s`); the sheet scrolls, because
  twenty-one roles do not fit one window. Pressing an agent's row opens its run
  with that agent's console selected, the same target the map cells use. A
  register that reports no role on any VM — an older daemon, or one not started
  with `--agents` — gets the honest empty state that says so rather than an
  empty table;
* the **fleet map**: iced's own canvas (`iced::widget::canvas`). The machine is
  the bounded substrate region, labelled with its model; each run is a lane
  placed by run index; each VM is a cell placed by VM index. A VM's state is
  the plaza's outlined mark (up: filled dot, exited: square, killed: diamond),
  a vsock VM has a wire through its mark, and a running VM carries a live bar
  whose height is its serial rate in lines per second, measured from the deltas
  between consecutive polls. Pressing a cell opens its run with that VM
  selected. The plate is a fixed scale, so a lane wider than the plate draws
  the cells that fit and prints `N of M cells · K past` in the caution's amber
  beside its label: the register and the run pane are where all fifty stay
  legible;
* the run pane: per-VM **role** (stamped; a dash for a VM that is not a role
  agent), state, guest CID, **exit kind** (`SLEPT` from
  `reboot: Power down`, `RESET` from `machine restart`/`Restarting system`, a
  dash when the tail says nothing), serial rate, and the latest line each VM
  wrote; the run's facts; the guest's serial transcript (the last 400 lines of
  the *selected* VM) with a follow toggle. The transcript is drawn as a ledger:
  every row carries the byte of the capture it starts at in the margin
  (right-aligned, the same measure the stream's `serial` events carry), and the
  heading over it names the line count and the byte span on the sheet —
  `Serial · vm 0 · 3 line(s) · byte 0–52` — so a transcript that only ever grew
  reads as offsets that only ever went up;
* the **serial console**: an input line, `SEND`, the guest's own quick commands
  (`help`, `uname`, `cpu`, `mem`, `uptime`, `poweroff` — the same set the web
  console offers), and an up/down history. A line is written to the selected VM
  with `POST /api/v0/runs/{id}/input` and the daemon's answer moves the
  transcript on;
* **transcript export**: a path box (iced offers no file dialog and the crate
  takes no dependency for one) pre-filled with
  `{temp}/plaza-{run}-{vm}.txt`, a `this vm` / `all vms` / `report` switch, and
  an `EXPORT` button that writes the transcript — the pane's own bytes, with a
  header line naming the run, the VM, its role, its state, CID and exit kind —
  with plain `std::fs`.
  A run export writes one labelled section per VM. The **report**
  (`{temp}/plaza-{run}-report.txt`) is the run as one document: a facts header
  (state, shape, the creation and end read against the run's own clock — a
  time of day on the same UTC day, the date on another, the elapsed time in
  the day tier — a tally of the sessions its captures showed, and each VM's
  role, state, CID and exit kind), then every VM's transcript under the same
  labelled section rule, each with a `session:` line derived from its own
  capture alone (`session: hello→welcome→echo✓→uptime✓→bye, 2 tool calls`;
  `session: keepalive, 3 heartbeats, 2 tool calls, no bye seen`;
  `session: none seen` — a line appears only where the capture supports it, so
  a fragment is never claimed as a session). A write that cannot land is a
  refusal in the notice line, in the disk's own words, and never touches the
  daemon's connection stamp. A path you type is kept; emptying the box hands it
  back to the derived default;
* the **telemetry** pane (`t`): the session's own figures, one observation per
  poll that *observed* something — a failed poll writes nothing, so a blank
  stretch in a line is time nobody observed, and a dotted rule in the margin's
  red marks each stretch the register missed. A capped rolling window
  (120 observations, ≈2 min at the default cadence) of the fleet's committed
  guest RAM, live VMs and serial throughput, drawn as iced canvases with ruled
  gridlines, mono figures, a ceiling on a figure a person would write, and the
  host's own total ruled inside the RAM chart when the fleet has been asked for
  more than it has. A run picked from the row above the charts decomposes the
  RAM and live-VM charts into that run's closed bands (the picked run's band in
  the register's one hot stroke) and lays its own line over the fleet's
  throughput. Fewer than three observations is not a figure: it is an honest
  note that says so;
* the run pane's **serial rate chart**: the selected VM's own lines/s over the
  session, in a 28-px canvas over a faint baseline, with the numeric rate still
  in every VM row. One chart for the VM the console targets rather than one in
  every row — a fifty-VM run would otherwise rebuild fifty canvas programs on
  every poll, and a small canvas beside a number is hard to read at any size;
* **session CSV export**: `observed_at, elapsed_s, scope, vms_live, ram_mib,
  lines_per_s`, long form — one `fleet` row per observation and one row per run
  of it, because runs arrive and leave mid-session and a wide sheet would lie
  with empty cells. Only run ids and figures: never anything the guest wrote.
  The path box is pre-filled with `{temp}/plaza-telemetry-{first observation}.csv`,
  derived once from where the session began, and it goes to disk through the
  same single `std::fs::write` as a transcript;
* **New run**: the daemon's own create fields — `n`, `vcpus`, `memory_mib`,
  `beats`, `vsock_cid` — validated client-side with the daemon's ranges
  (`n`/`vcpus` 1–64, `memory_mib` ≥ 128, `vsock_cid` ≥ 3, and the run's last
  guest CID inside the kernel's CID space) and the daemon's wording, plus three
  **quick-boot presets** that only *fill the form* (nothing is sent until
  `BOOT RUN`), each with its footprint as static label text:
  `2 VMs + vsock` (n=2, 2 vCPU, 512 MiB, cid 10), `8 VMs` (n=8, 512 MiB) and
  `50-VM scale` (n=50, 1 vCPU, 128 MiB ≈ 6.3 GiB guest RAM);
* **KILL**: `DELETE /api/v0/runs/{id}`, two presses (arm, then confirm);
* the **ledger** (`l`): the plaza's account book — every task the daemon holds,
  as `GET /api/v0/tasks` reports it, oldest first, and the four moves the
  daemon accepts on one. One ruled row per task: its id, what it is for, the
  role it was filed for, its budget, the state it is in as a stamp, who holds
  it, the guest CID it is held by, and when it was filed and last moved (a task
  filed on another day is stamped with **that day** rather than a bare time of
  day, the register's own rule). Pressing a row opens its detail: every fact
  the daemon reports about the row — its state *and*, once it has ended, the
  schema's `outcome` beside it (`done · accepted`; the two are not the same
  word), its budget, role, assignee, agent CID, its verifier (always none at
  v0, and said so rather than drawn as a name nobody recorded), its creation
  and last move with the age read in the day tier — the goal and the acceptance
  criteria in full, and the agent's note. The moves are the daemon's own:
  **create** (the goal, the acceptance, the budget and the role as free-text
  fields, validated here with `noded`'s own bounds and wording — a blank
  optional box is *absent*, not empty), **claim** (the claiming agent's guest
  CID in a numeric box, with the register's live agents — its running role VMs
  — offered beside it as one-press CIDs, so a claim is made as a CID the daemon
  will actually accept), **finish** (`FINISHED`/`FAILED` with the agent's note),
  and **withdraw** (two presses: arm, then confirm, like a run's kill). A task
  that has ended is final and the sheet says so; the moves it cannot accept are
  not drawn at all. Every refusal — the daemon's or one of these guards — is the
  notice line, in the daemon's own words: the `409` a claim from a CID no live
  agent holds answers with names the CID, and it is shown **verbatim**. Task
  events arrive on the event stream and patch the rows in place between the
  polls (`event: task` carries an id, a state and an assignee): a task the
  ledger holds is moved where it stands, and an event for a task it has **not**
  read triggers one coalesced read of the ledger rather than a row invented
  from a frame that carries no goal, no budget and no clock. The index's ledger
  entry carries the daemon's own counts (`3 task(s) · 1 open · 1 claimed · 1
  ended`) once the ledger has answered, and says `not read yet` before that: a
  ledger nobody read is not an empty ledger. The detail's **artifact section**
  reads a task's work product back out of its agent's workspace: the paths the
  agent's finish note names (`artifacts: <path>[, <path>…] (n tool call(s))`,
  read out of the note by `ledger::artifact_paths`) are offered as selectable
  rows, and pressing one drives that agent's `read_file` through the daemon
  (`POST /api/v0/tasks/{id}/artifact`) — the one read-through, which is a POST
  because it drives the agent's session. What comes back is the agent's own
  file, drawn as ruled text with every line's byte in the margin, bounded and
  scrolled, and **displayed, never interpreted** (it is the guest's bytes). The
  read is in one of four honest states: in flight, read (with the file's size,
  the span and the CID it was read through), refused — the hub's `403` role
  policy, the agent's `422`, an unassigned or unconnected `409` — in the
  daemon's own words, or **not configured**, the `503` of a daemon never told
  where the hub's admin surface is, which is a different fact from a refusal.
  A read the agent declines is the daemon answering, so it neither marks the
  register down nor backs the poll off; only an unreachable daemon is a failed
  call;
* the **treasury** (`y`): the plaza's money, read straight from the daemon's
  own treasury service and **read-only from this console** — every entry is
  posted by the daemon, and nothing here can post one. One read covers both
  halves (`GET /api/v0/treasury` and `GET /api/v0/treasury/journal` at once),
  on **its own schedule** with **its own failure count**, exactly as the hub's
  status surface has: a daemon that serves the register but not the treasury —
  an older daemon — never marks the register down. The sheet draws the
  **accounts** (kind, owner, credit limit and balance, every amount in credits
  *and* the dollars it stands for — whole credits at 1 credit = $0.001 of real
  spend, the register's own money idiom, no invented rate), the **totals line**
  (accounts, entries, credits, debits and the sum of every balance, zero while
  the journal balances), the daemon's own note on real spend **verbatim**
  (`spent 0 cr · $0.00` — `v0 meters no real spend; a done task refunds its
  whole escrow`), and the **journal**: one row per entry (its id, kind, task,
  memo, size and the day it was posted, the register's day-tier stamp), each
  with its **postings ruled as the debit and credit sides they are** — a side
  is the sign of the amount, and the two sides of an entry are the same size,
  because every entry balances to zero. The accounts and the journal are
  bounded and **scrollable**, and the index's entry carries the daemon's own
  counts (`2 account(s) · 2 entry(s)`) once the pane has read it, saying `not
  read yet` before that: a treasury nobody read is not an empty treasury. A
  `treasury` event arrives on the event stream when an entry is posted
  (`event: treasury` carries an entry id, a kind, a size and a task): an entry
  the pane already holds is a duplicate and changes nothing, while an entry
  past the newest it holds means the journal moved on, and the pane reads the
  journal again — **once** for a burst of entries, and never growing the
  journal from a frame that carries no memo, no postings and no clock. A read
  that fails keeps the last one on the sheet (the journal is append-only, so
  it is incomplete rather than wrong) and says how old it is and why it has
  not moved;
* the **market** (`u`): the plaza's calls — the offers its roled tasks make —
  each with the bids it holds and the award that closed it, read straight from
  the daemon's agora (`GET /api/v0/agora`) and **read-only from this console**:
  a bid is placed and an award is made by the daemon's own surface
  (`POST /api/v0/tasks/{id}/bid` and `POST /api/v0/tasks/{id}/award`), and
  nothing here can place either. One read covers the whole board, on **its own
  schedule** with **its own failure count**, exactly as the treasury's and the
  hub's do: a daemon that serves the register but not the agora — an older
  daemon — never marks the register down, and a console that never opened the
  sheet never reads the board at all. The sheet draws the **board**: one row
  per call (its id and capability, its ceiling in *max price* — whole credits
  *and* the dollars they stand for, the register's own money idiom — its
  **state as a stamp** (`OFFERED` for an offer, `AWARDED` once an award closed
  it), and the day it opened, the register's day-tier stamp), the acceptance a
  task named under its call (`spec · …`), and beneath each call its **bids**
  (the bidder, its guest CID, the price and when it was made), with the bid
  the award was made on **marked** — the award binds the price the agent bid,
  so the call's award names its winning bid by the bidder's CID and that
  price, and a higher losing bid is drawn but not marked. The **award** is
  drawn under its call: the contractor, the CID, the price and the **treasury
  entry that funded the call**, labelled as the treasury's own funding entry
  (the market names it and does **not** jump to the treasury sheet to draw it —
  the `y` sheet carries it). A call that is still open with no bid reads `no
  bid yet` rather than a blank row; a call's `task` is the offer's id and the
  ledger's own, one thing at v0. The board is bounded and **scrollable**, and
  the index's entry carries the daemon's own counts (`5 call(s) · 3 bid(s)`)
  once the pane has read it, saying `not read yet` before that: a board nobody
  read is not an empty board. An `agora` event arrives on the event stream when
  a bid or an award moves a call (`event: agora` carries the task id, `bid` or
  `award`, the agent's CID and the price): a frame the board already reflects
  (a bid it holds from that agent at that price, or the award it holds) is a
  duplicate and changes nothing, while anything else means the board moved
  past what the pane saw, and the pane reads the board again — **once** for a
  burst of bids, and never growing a call or a bid from a frame that carries no
  bidder, no escrow entry and no clock. A read that fails keeps the last one on
  the sheet (a bid the daemon recorded is a fact, so it is incomplete rather
  than wrong) and says how old it is and why it has not moved;
* the **floor** (`i`): the plaza's agents as small **robots on a 3D RTS floor**,
  drawn in the register's own canvas (`iced::widget::canvas`) as real 3D
  geometry — a **perspective projection** through a fixed RTS camera (**55°
  tilt, 20° yaw**, distance 10, focal `0.85·height`, near plane 0.2), a
  projected ground plane with a grid, **painter-ordered** polygons. It is not
  a custom `wgpu` pipeline: iced 0.14 *does* ship a `shader` widget, but using
  it needs a direct `iced_wgpu`/`iced_wgpu::primitive` dependency and a WGSL
  pipeline, which this app's no-new-deps, canvas-only policy does not carry —
  so the geometry is 3D in the mathematics (a true perspective divide) drawn
  in the canvas that both the wgpu and the tiny-skia backends already render.
  Every position is a pure function of real state and nothing over the wire:
  the **role-station layout** derives from the connected register's role order
  — the role at index `i` gets the pad at grid `(i % 5, i / 5)`, agents sharing
  a role spread along `x` by their index within the role — and a **conversation
  moves the pair adjacent**: the two robots walk to spots straddling the
  midpoint of their stations, on an **eased (smoothstep), bounded (700 ms)**
  clock. Each robot carries a **ground status ring** (a projected ellipse):
  `working` when its serial rate is above the floor's threshold (the console's
  own telemetry), `talking` when it is in a pair the social API calls active,
  `idle` otherwise, and `unreachable` when a hub is configured and does not
  report it connected — the legend names every ring and says the floor is a
  diagram of real state. The **arrows cycle the robots**, `Enter` opens the
  **inspector** (the role, CID and state, its **contacts** from
  `GET /api/v0/agents/{cid}/contacts`, its **last board post**, and the
  **thread** `GET /api/v0/conversations/{a}/{b}` with a pressed contact), and a
  press on a robot picks it. Like the treasury's and the market's, the read is
  `GET /api/v0/conversations` and `GET /api/v0/board` **together**, on **its own
  schedule** with **its own failure count**, so a daemon that serves the
  register but not the social layer never marks the register down. Nothing here
  writes: every position is a derivation of real state;
* the **board** (`j`): the plaza's public posts — `GET /api/v0/board`, newest
  first — one ruled block per post with its author and role, the **task it
  links** (the board is where discovery happens) and **`read by N of M`**
  derived from the daemon's per-agent **unread counts** against the board's own
  newest post. A post is the daemon's own, composed by the agents' drivers from
  real task data; this sheet writes nothing. A `board` frame arrives when an
  agent posts or advances its read cursor: a post the pane already holds is a
  duplicate, while a fresh post (or a `read` frame, which carries a cursor but
  no counts) means the board moved, and the pane reads its own surface again —
  once per burst. A `conversation` frame for an exchange the floor does not
  hold reads the floor again, only while the floor is showing;
* the **talk** (`k`): the plaza's conversation log — `GET /api/v0/conversations`,
  newest first — one ruled block per exchange: **the pair as two
  identity-and-role chips**, the **words as the agent's own prose** (drawn as
  text, never as anything a reader could act on), the **delivery mark** the
  node gave (the green `delivered` when the peer's session took the call, the
  hub's own reason *verbatim* when it did not, and the honest dash when the
  node said neither — no mark is invented), and the stamp. Beneath the lede is
  the pane's **tally** (the exchanges carried, the pairs the daemon folded,
  how many are **talking** inside the daemon's window, and how many carried
  exchanges were **refused**). The **pairs filter** walks the daemon's own
  pair summaries — the arrows pick a row, **`Enter` narrows to its thread**
  (`GET /api/v0/conversations/{a}/{b}`, oldest first) and `Enter` again or
  `Escape` widens — with the **active window stated from the view's own
  `active_secs`** and the **`TALKING` mark labelled as the floor's walk**
  (the floor draws such a pair adjacent). The narrow read is the pair API, not
  the log's own rows, and an empty thread is stated as an empty thread. The
  log is read on **its own schedule** with **its own failure count** (the
  board's, the market's and the treasury's pattern), and the narrowed pair's
  thread rides the same clock. A `conversation` frame for an exchange the pane
  does not hold reads the log again — **once per burst**, and only while the
  pane is showing — and never grows a row: the frame carries no text and no
  roles. A daemon that serves no social layer is stated as the **absence of a
  layer** (a plain `no social layer` notice on a 404), and any other failure is
  named beside the log it keeps. Nothing here writes;
* every failure is a notice line and an error in the index — no request path
  panics, and nothing is swallowed.

## How it is built

* `iced` with its **wgpu** renderer (the WebGPU-compatible backend); the
  **floor** is 3D geometry painted in iced's own `canvas` (which both the wgpu
  and the tiny-skia backends render), not a custom `wgpu` pipeline: iced 0.14
  does ship a `shader` widget, but it needs a direct `iced_wgpu::primitive`
  dependency and WGSL, which the no-new-deps policy does not carry. The
  projection math lives in `src/floor.rs` (pure, unit-tested) and the canvas in
  `src/floorview.rs`;
  `tiny-skia` software renderer stays enabled as the compositor fallback, the
  `canvas` feature supplies the map, and the executor is `smol` —
  deliberately not `tokio`.
* The API client is a tiny blocking HTTP/1.1 client over `TcpStream`, loopback
  only, with no HTTP client dependency — the same shape
  `crates/noded/tests/common/mod.rs` uses to test the daemon. The **hub's**
  read-only status surface (`vsockd --status-port`'s `GET /status`) is read
  with that same client and held to that same exact-loopback policy, with one
  stricter rule: its port is required, since the surface is off unless the
  daemon was asked for it and there is no default to fall back to. The
  desktop asks it directly because it *can* — the surface is loopback-only and
  a browser cannot fetch it (no CORS), which is exactly why the connected view
  belongs in this console.
* The event stream (`src/events.rs`) is the same kind of thing: a raw
  `TcpStream`, the request, the head that confirms the subscription, and a
  server-sent-events parser fed by whatever the socket returns — a thread
  follows it, reconnecting with a bounded backoff, and `src/app.rs` maps what
  it reads into messages. `src/serial.rs` is the offset-idempotent merge of the
  serial deltas and the capture bookkeeping it needs: both are pure, so the
  frame matrix and the merge matrix are unit-tested without a socket.
* The model (`src/model.rs`) — the panes, the reducer, the roster's grouping
  and its tallies, the ledger's state arithmetic, the age/instant arithmetic —
  the map's geometry and activity
  (`src/map.rs`),
  the exit kinds (`src/exit.rs`), the form and its presets (`src/form.rs`), the
  ledger's create form and the finish note's artifact paths
  (`src/ledger.rs`), the export (`src/export.rs`), the
  frozen vsock session read back out of a serial
  capture (`src/session.rs`), the session's rolling history
  (`src/history.rs`), the charts' arithmetic (`src/chart.rs`), the client
  (`src/api.rs`) and the CLI (`src/cli.rs`) know nothing about iced, so the
  reducer, the layout and its hit-testing, the rate arithmetic, the observation
  ring and its gap segmentation, the CSV bytes, validation, preset bodies,
  transcript and report bytes, the session derivation, parsing and poll
  arithmetic are unit-tested headlessly; `src/app.rs` is the
  only place that turns an effect into an iced `Task` (and the only place that
  writes a file), and `src/mapview.rs` and `src/telemetryview.rs` the only
  places that paint.
* `src/theme.rs` and the widgets in `src/view.rs` echo
  `docs/ux-draft/owner-console.html`: the Plaza palette, mono for data and
  serif for prose, ruled sheets, and stamps for state.

## Tests

```
cargo test          # 451 lib + 55 client + 83 render = 589, 0 failed
```

* `src/**` — unit tests: the API's JSON against fixture bodies (including the
  machine and the console write, and a role's presence, absence and explicit
  null; and the hub's `/status` — the frozen shape parsed field for field, an
  unknown extra field tolerated at every level, a missing field named in the
  error, and the hub url's port requirement), the reducer and every pane
  transition (including the roster's grouping
  and its tally, and the register⇄hub join: register-only, hub-only, matched,
  the two labels that say which half is missing, a role the two surfaces
  disagree about, and the hub's own schedule, backoff and failures — none of
  which may touch the register's health), the age/date/instant arithmetic, the
  map's
  layout/hit-test/rate arithmetic, its lane overflow and its role labels'
  character-wise elision, the exit-kind
  matrix, the console's history walk, the spawn form's boundaries and the
  presets' fill + byte-for-byte bodies, the export path's token arithmetic and
  its write (bytes read back off the disk), the report's byte-exact assembly
  (facts, sections, the derived `session:` lines and the tally) and the session
  derivation's whole matrix (a frozen session read past interleaved boot lines
  and CRLF; a keepalive session's beats and served calls with no bye, from a
  welcome that confirms it, from a bare heartbeat and from the hello's own ask;
  a keepalive session the hub stopped; a partial or absent session reading as
  `none seen`; malformed, non-object and unknown-kind payloads skipped and never
  believed; a result stamped on the call it names; a fresh hello restarting the
  reading), the help sheet's toggling, the poll
  schedule and backoff at more than one cadence, the serial tail (with the byte
  each row starts at), the floor's projection table (a point + camera → the
  expected screen coordinates), its layout derivation (a fixed role order →
  fixed station positions), its walk endpoints and easing bounds, its role-ink
  stability, and the board's read-count derivation (including a boundary unread
  count and a board with no ids) — all with the social views' parse matrices
  (every column of `Posts`, `Contacts`, `Conversations` and `Thread` required
  at every level with the error naming it, the nullable columns null-vs-absent,
  an unknown kind tolerated) and the framer's `board` and `conversation` frames
  (the daemon's own bytes, an embedded post as the board's own object, a field
  named while the next frame reads) — the key bindings (`w`, `l`, `y`, `u`, `i`,
  `j` and `k` among them), the CLI.
* the ledger, in the same unit tests: the `Task` model's whole parse matrix — a
  body as the daemon writes it read field for field, **every** column required
  (the nullable ones included: `null` and "absent" are not the same fact, and a
  body missing one is an error that names it), an unknown column and an unknown
  state or outcome tolerated, and the create/claim/finish bodies byte for byte;
  the create form's boundaries in `noded`'s own words (a blank goal, a field
  past its bound, a budget that is not a whole number, every broken field named
  at once); and the reducer's ledger paths — a task event patching a row in
  place without inventing the goal, budget or clock a frame does not carry, an
  event for a task the ledger has not read asking for the ledger **once** for a
  whole burst and never growing the ledger by a row, a settled task that no
  event or older list answer walks back, a duplicate event as a no-op, a reused
  task id inheriting nothing, the claim's guards and the daemon's `409` kept
  verbatim in the notice line, the finish's note and its absence, the withdraw's
  arm/confirm, and the live agents the register's running role VMs offer. The
  artifact read-through is here too: the `TaskArtifact` parse matrix (the live
  body field for field, every column required — none of them nullable — and an
  unknown column tolerated), the finish note's artifact paths read out of the
  line the dispatcher writes (one path, several, a zero-call empty list, and
  every other note shape naming nothing), and the reducer's read paths — a row
  asking the daemon for exactly that path on the task the ledger has open, the
  answer landing as the file the agent read or as the daemon's refusal in its
  own words, a `503` drawn as the read-through being *off* rather than refused,
  an unreachable daemon as the one failed call, an answer for a task that is no
  no longer open dropped, and a selection change or a task leaving the ledger
  taking its read with it.
* the treasury, in the same unit tests: the `Treasury`, `Journal` and `Posting`
  parse matrices — a body as the live daemon wrote it read field for field
  (every account, both totals, and the one cycle's mint and refund with their
  postings), **every** column required at every level with the error naming it
  (`null` and "absent" are not the same fact for the two nullable entry
  columns, and a posting's own four are all required), an unknown account or
  entry kind and an unknown column tolerated, a journal cut to its limit
  parsed as exactly that, and a posting knowing its own side; the pane's own
  schedule — the sheet's key reading the treasury once, a tick while the sheet
  is up reading it on **its** cadence and never before it, the register polled
  either way, and nothing read at all off the sheet; its failures being its own
  (a treasury read that fails leaves the register's failures, error and health
  untouched, backs off on its own count, and clears when it comes back) and its
  answer keeping the last read beside the failure; and the `treasury` frame's
  reducer paths — an entry the pane already holds as a no-op that invents no
  row, an entry past the newest it holds asking for the journal **once** for a
  whole burst, a frame off the sheet asking for nothing, and the money idiom
  (whole credits and the dollars they stand for, a debit's sign kept on both),
  the totals line, the journal's tally and its truncation, and the postings'
  account labels.
* the market, in the same unit tests: the `Board`, `Call`, `Bid` and `Award`
  parse matrices — a board as the daemon writes it read field for field (an
  awarded call with two bids and its escrow entry, an open call with no bid,
  and a call whose award names no escrow entry), **every** column required at
  every level with the error naming it (`null` and "absent" are not the same
  fact for the three nullable columns — a call's `spec_ref` and `award`, an
  award's `escrow_entry` — and a bid's own five are all required), an unknown
  call state (`awarded` and the task's own words are carried; anything else is
  `Unknown`) and an unknown column tolerated at every level, a board cut to its
  limit parsed as exactly that, and the winning-bid rule (the award's own
  contractor and price mark the bid it was made on, not a higher losing one);
  the pane's own schedule — the sheet's key reading the board once, a tick
  while the sheet is up reading it on **its** cadence and never before it, the
  register polled either way, and nothing read at all off the sheet; its
  failures being its own (a market read that fails leaves the register's
  failures, error and health untouched, backs off on its own count, and clears
  when it comes back) and its answer keeping the last read beside the failure;
  the `agora` frame's reducer paths — a frame the board already reflects (a bid
  it holds from that agent at that price, or the award it holds) as a no-op
  that invents no call, a frame past what the board holds asking for the board
  **once** for a whole burst of bids, a re-bid at another price a fresh read, a
  frame off the sheet asking for nothing; and the market's lines (the index's
  counts, the board's tally and its truncation, the offer and award stamps, and
  the award line naming the treasury's own funding entry — or the honest `none`
  for a call whose task named no budget).
* the stream: the frame parser's whole matrix (the daemon's frames, a frame
  split mid-line across reads, a heartbeat, CRLF frames, a leading blank line,
  an unknown event kind, a data line with no kind, a malformed frame followed
  by a good one, a state this console does not know, a serial frame's escaped
  bytes, and a `task` frame from the daemon's own bytes — with a state this
  console does not know, and with a field missing, which is named while the next
  frame still reads; a `treasury` frame from the daemon's own bytes — the
  task named and null, a kind this console does not know, and a missing field
  named while the next frame still reads; and an `agora` frame from the
  daemon's own bytes — a bid and an award, a kind this console does not know,
  and a missing field named while the next frame still reads), the head that confirms the subscription (a real socket, and three
  heads that are not a stream), the reconnect ladder's bounds (`250 ms` doubling
  to `30 s`, reset by any event), the offset merge's whole matrix (append,
  clip, replay, gap, a clip that would split a character, per-VM independence,
  retention), the bridge between the reader thread and the runtime (the waker it
  parks on, a message that arrives without a wake, the stream ending when its
  thread does), and every reducer path an event drives (a known run's state in
  place, one poll for a run the register has not read, an ended state never
  walked back, a list answer read before a run ended, a serial delta landing in
  the transcript, the transcript landing on exactly the polled bytes, a gap
  re-reading the open run, the rate the events feed, an event keeping the stamp
  fresh without hiding a refused poll).
* `tests/client.rs` — the client over real sockets against a `std::net` daemon
  stub (`tests/common/mod.rs`), asserting the exact request line, headers and
  bodies (create **and** console write, and the ledger's own four: the create
  body, the claim's `agent_cid`, the finish's outcome and note, and the
  withdraw's path), the malformed/refused/unreachable
  paths, and the event stream: the frames off a real socket (the heartbeat is
  not an event, an unknown kind is not an event) and a reconnect whose second
  event comes off a second connection after the backoff. The ledger's paths are
  read the same way, off their own stub: the list parsed oldest first with every
  column, one task on its own path (`GET /api/v0/tasks/t1`), a missing task as a
  recognizable 404, an id that could change the path refused before a socket is
  opened, and a refused claim carrying the daemon's own `409` — `no live agent
  has guest CID 4` — through unchanged. The artifact read-through is read the
  same way: the `POST /api/v0/tasks/t3/artifact` request and its
  `{"path":…}` body, the file the agent read parsed field for field, and the
  refusals carried verbatim — the hub's `403` role policy, the agent's `422`
  (`denied: path "../escape" contains ".."; every path is inside the
  workspace`), and the `503` of a read-through that was never configured. The
  hub's status
  surface is read the same way, off its own stub: `GET /status` on the status
  path, the frozen body parsed field for field, an empty registry as an answer
  rather than a failure, an unknown extra field tolerated, a missing field
  named, a daemon without a status surface as a recognizable 404, and the
  loopback policy's negatives — subdomains, a suffix (`127.0.0.1.evil.tld`),
  a trailing dot, case variants, `::1`, `0.0.0.0`, a path, a query, port 0 and
  a missing port — every one refused before a socket is opened. The treasury is
  read the same way, off its own stub: both paths in one read
  (`GET /api/v0/treasury` then `GET /api/v0/treasury/journal`, both GETs with no
  body), the mint and the escrow parsed with their derived balances and the
  journal's postings with the sign that is their side, a treasury the daemon
  does not serve carried as its own `404`, and a journal half that cannot be
  read failing the whole read rather than half of it. The market is read the
  same way, off its own stub: one `GET /api/v0/agora` for the whole board, the
  awarded call's bids and its award parsed field for field with the winning bid
  marked and the higher losing bid not, an open call with no bid and no award
  parsed with its `null` columns as none, and a market the daemon does not
  serve carried as its own `404`.
* `tests/render.rs` — headless rendering with iced's own harness: the register
  drawn with the stub's runs, a click on a run fetching it and rendering the
  guest's serial, a click on `New run` opening the form, the presets row and a
  preset click filling the form then sending exactly its body, the machine
  sheet's every column, the map's legend and its plate (a real canvas frame
  whose substrate and marks are asserted in pixels, plus a fifty-VM lane's
  clipped tail and its amber `15 of 50 cells · 35 past` note), the console's
  quick buttons driving a real `POST .../input` whose body is read back off the
  stub, the export button writing two real transcripts whose bytes are compared
  against the pane's and read back off the disk, the `report` target writing the
  run's report to `plaza-r1-report.txt` whose bytes are compared against
  `run_report`'s and read back off the disk (both derived sessions and the tally
  in the file), an export that cannot land drawn as a `REFUSED` notice, the
  help `?` sheet drawn over a dimmed register
  (its box, its scrim and the ink still showing through asserted in pixels),
  the telemetry pane over a real six-observation session (the honest
  `<3 observations` notes, the captions with their window and peak, a real
  canvas frame whose fleet line is ink and which holds no signal until a run is
  picked and then holds its band in the signal, the host's total ruled in amber
  on an overcommitted RAM chart, and the CSV button writing the scale run's
  ramp — `0, 6,400, 6,400, 1,408, 0` MiB — whose rows are asserted byte-exact
  and read back off the disk, twice), the run pane's selected-VM rate chart
  with its numeric column kept, the **whole application following the event
  stream** (its own subscription, its own reader thread, its own client and its
  own reducer against a stub that streams frames: with the cadence at the 30 s
  ceiling and exactly one boot poll, the register moves a run the list still
  calls `running` to `DONE`, the pane's transcript lands on exactly the polled
  capture plus the streamed bytes once, the ledger heading names that span, and
  the frame is drawn), the **role fleet** (the register stamping a run's role —
  the role itself for one, the count for several, nothing at all for a run whose
  VMs carry none — a role run's run pane stamping each agent's own role, and the
  roster grouping the live register by role with its own per-group tally, its
  rates off the activity feed, its rows opening their run with that agent
  selected, and the honest empty state for a daemon that reports no role; plus
  the long-running register: a three-day-old `running` run reading `3d 07h` and
  its creation stamped `2025-10-06`), the **hub's half of the roster** (a hub
  that answered: its header line — its served CIDs, its connected agents, its
  live sessions and its uptime — and its capacity line, the join drawing one
  row per agent with the register's half and the hub's, `registered, not
  connected` for a live VM nobody is connected to, `connected, not registered`
  for a session no VM row claims *and* not a button (there is no run to open),
  the per-group `K connected` count, the `hub unknown (no --hub-url)` state
  that claims nothing about connections it never read, `hub unreachable
  (cannot reach …)` with the register's half still drawn and the daemon still
  `LIVE`, a hub with nobody connected as an answer rather than a failure, and
  the **whole application** asking the hub's `/status` over a real socket),
  the **ledger** (the table drawn from the stub's tasks — the tally, every row
  with its goal, role, budget and state stamp — a click on a row opening its
  detail with the goal and acceptance in full, the outcome beside the state for
  an ended task, and only the moves the daemon would accept: no claim on a task
  in hand, no finish before a claim, no move at all on one that has ended; the
  claim row offering the register's live agents as one-press CIDs and saying so
  when no role agent is up; a claim the daemon refuses `409` drawn in its own
  words — `task t2 claim: 409: no live agent has guest CID 4` — with nothing
  changed; the create form's own refusal at an empty goal and then a real
  `POST /api/v0/tasks` whose answer is filed, selected and named in the notice
  line; the withdraw's arm/confirm; the index entry and the `l` key reaching the
  sheet; a scrollable ledger drawn over a real frame whose claimed and done
  stamps are asserted in pixels; the **artifact section** over a real frame —
  the note's path offered as a pressable row, a click on it reading the file
  through the daemon, the read in flight named while it is, the agent's file
  drawn as ruled text (its span, its size, the CID it was read through, and a
  long file's tail with the lines the ledger's window left out counted above
  it), a `403` refusal drawn in the register's red in the hub's own words, and a
  `503` drawn as the read-through being *off*; and the **whole application
  following a task event** between polls — the state and assignee off the
  stream, the ledger still holding exactly the daemon's rows), the **treasury**
  (the sheet over the stub's real treasury: the accounts with every column and
  both figures — `1500 cr · $1.50` — the totals line, the daemon's note on real
  spend verbatim, the journal's two entries with their memos, sizes and
  debit/credit postings, and a posting from another day stamped with its day;
  the index entry and the `y` key reaching the sheet, the entry saying `not
  read yet` before the pane has read it; a fresh daemon's **empty journal**
  drawn as an empty journal rather than a blank sheet; a read that fails after
  a good one keeping the last read and naming the failure beside it with the
  register still `LIVE`; and the **whole application following a `treasury`
  frame** between polls — the frame driving one read of the pane's own surface,
  the entry that arrives the daemon's own row with the postings a frame could
  not describe, and no register poll), the **market** (the board over the
  stub's real board: the call rows with their capability, ceiling in credits
  and dollars (`2000 cr · $2.00`) and state stamps (`OFFERED`, `AWARDED`), the
  acceptance a task named under its call, each call's bid rows with the winning
  bid marked `AWARD BID` and a higher losing bid unmarked, the award drawn with
  its contractor, CID and **the treasury entry that funded the call** labelled
  as the treasury's own funding entry, and an open call with no bid reading `no
  bid yet`; the index entry and the `u` key reaching the sheet, the entry
  saying `not read yet` before the pane has read it; a fresh daemon's **empty
  board** drawn as an empty board rather than a blank sheet; a read that fails
  after a good one keeping the last read and naming the failure beside it with
  the register still `LIVE`; and the **whole application following an `agora`
  frame** between polls — the frame driving one read of the pane's own surface,
  the bid that arrives the daemon's own row with the bidder and clock a frame
  could not describe, and no register poll), the **talk** (the log over the
  stub's real log — the live plaza's own shape, three exchanges across two
  pairs: the pane's tally `3 exchange(s) carried of 3 · 2 pair(s) of 2 · 1
  talking · 0 refused`, the pairs filter with both fold rows and the talking
  pair's `TALKING` stamp asserted in pixels against the same model gone quiet,
  each exchange's two identity-and-role chips, the words as prose, the delivery
  mark, and the newer pair's `NARROWED`/thread path — `Enter` narrowing to the
  pair API's thread (its own tally and the two messages), `Escape` widening
  back to the log; the delivery mark's three cases over real frames (the hub's
  own reason verbatim, `delivered`, and the dash the node said neither for); a
  fresh plaza's **empty log** drawn as no exchange and no pair; a narrowed pair
  whose **thread is empty** drawn as an empty thread; a daemon with **no social
  layer** (a 404) as a plain `no social layer` notice with the register still
  `LIVE`, and an unreachable daemon naming its error beside the log it keeps;
  the index entry and the `k` key reaching the sheet, the entry saying `not
  read yet` before the pane has read it; and the **whole application following
  a `conversation` frame** between polls — the frame driving one read of the
  pane's own surface, the exchange that arrives the daemon's own row with the
  text and role a frame could not describe, and no register poll), and a full
  application frame
  written to
  `target/tmp/fleet-frame.rgba` (and the map's to `target/tmp/fleet-map-frame.rgba`,
  the telemetry pane's to `target/tmp/telemetry-frame.rgba`, the stream's to
  `target/tmp/stream-frame.rgba`).

Set `APZ_DESKTOP_LIVE=http://127.0.0.1:8899` to have `tests/client.rs` also
read a running daemon (read-only: it reads the machine, lists runs, reads
each one with its serial and exit kind, reads the ledger — every task, and
each task by its own id — reads the treasury, both halves at once, checking
every entry balances, reads the market's board, every call with its bids
and its award and the treasury entry that funded it, and reads the talk log —
every exchange with the delivery mark the daemon gave, the pairs it folded and
one pair's thread; it mutates nothing; it
also reads each task's artifact
back through that task's agent, the one POST of the walk, which drives the
agent's `read_file` and touches nothing but the file it reports). Set
`APZ_DESKTOP_LIVE_HUB=host:port`
to have it read a running hub's status surface too (read-only: the served CIDs,
the uptime, the session count, and every connected agent's role, call count and
heartbeat age). Neither is set by default, so the suite runs on a host with
neither process.
