# AgentPlaza desktop register

An iced + wgpu (WebGPU) desktop dashboard for the node daemon's fleet API: the
fleet as a register — an index of runs on the left, a ruled sheet on the right,
stamps for every state the daemon reports.

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
agentplaza-desktop [--url <url>] [--refresh-ms <milliseconds>]
                   [--timeout-secs <seconds>]
```

| | |
|---|---|
| `--url <url>` | the node daemon to read; `http://127.0.0.1:8899` by default |
| `--refresh-ms <n>` | how often a healthy daemon is polled, 250–30000 ms; 2000 by default |
| `--timeout-secs <n>` | how long one call may take, 1–600; 5 by default |
| `-h`, `--help` | the usage text |
| `APZ_DESKTOP_URL` | the same as `--url` |
| `APZ_DESKTOP_REFRESH_MS` | the same as `--refresh-ms` |
| `APZ_DESKTOP_TIMEOUT_SECS` | the same as `--timeout-secs` |

### Keys

| | |
|---|---|
| `r` | the fleet register |
| `m` | the machine (the index also lists it) |
| `f` | the fleet map |
| `w` | the roster: who is running right now, grouped by role |
| `t` | the telemetry: the session's own figures, one observation per poll |
| `?` | the help sheet, which lists every binding (`Escape` or `?` closes it) |
| `Escape` | back to the register |
| `↑` / `↓` | walk the console's command history |

While the help sheet is up it is *over* the register — a scrim across the
window with the sheet centred on it — and no other key reaches a pane until it
is closed. A focused text box captures its own keys, so typing a console line
never trips a shortcut.

An argument wins over the environment. Only a **loopback** host is accepted,
and only an exact one: `127.0.0.1` (or `localhost`, normalized to it), never
resolved through DNS. Any other host is refused before a socket is opened.

```sh
cargo run -- --url http://127.0.0.1:8899
APZ_DESKTOP_URL=http://127.0.0.1:9000 cargo run
cargo run -- --refresh-ms 500
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
* the index column: the fleet entry, the **machine**, the **fleet map**, every
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
* the **roster** (`w`): *who is running right now*, read off the live register
  and nothing else — the daemon is asked for nothing new. Every VM whose row
  carries a role is a row: the role it accepted, the run and VM it is, its
  guest CID, the state right now, and the serial rate the activity feed has
  for it (a rate needs two reads of a VM's capture, so a freshly opened roster
  starts at zero and moves as the polls land). Grouped by role, in the order
  the roles appear in the register, each group with its own tally; the sheet
  scrolls, because twenty-one roles do not fit one window. Pressing an agent's
  row opens its run with that agent's console selected, the same target the map
  cells use. A daemon that reports no role on any VM — an older one, or one not
  started with `--agents` — gets the honest empty state that says so rather
  than an empty table;
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
  `{temp}/plaza-{run}-{vm}.txt`, a `this vm` / `all vms` switch, and an `EXPORT`
  button that writes the transcript — the pane's own bytes, with a header line
  naming the run, the VM, its role, its state, CID and exit kind — with plain
  `std::fs`.
  A run export writes one labelled section per VM. A write that cannot land is
  a refusal in the notice line, in the disk's own words, and never touches the
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
* every failure is a notice line and an error in the index — no request path
  panics, and nothing is swallowed.

## How it is built

* `iced` with its **wgpu** renderer (the WebGPU-compatible backend); the
  `tiny-skia` software renderer stays enabled as the compositor fallback, the
  `canvas` feature supplies the map, and the executor is `smol` —
  deliberately not `tokio`.
* The API client is a tiny blocking HTTP/1.1 client over `TcpStream`, loopback
  only, with no HTTP client dependency — the same shape
  `crates/noded/tests/common/mod.rs` uses to test the daemon.
* The event stream (`src/events.rs`) is the same kind of thing: a raw
  `TcpStream`, the request, the head that confirms the subscription, and a
  server-sent-events parser fed by whatever the socket returns — a thread
  follows it, reconnecting with a bounded backoff, and `src/app.rs` maps what
  it reads into messages. `src/serial.rs` is the offset-idempotent merge of the
  serial deltas and the capture bookkeeping it needs: both are pure, so the
  frame matrix and the merge matrix are unit-tested without a socket.
* The model (`src/model.rs`) — the panes, the reducer, the roster's grouping
  and its tallies, the age/instant arithmetic — the map's geometry and activity
  (`src/map.rs`),
  the exit kinds (`src/exit.rs`), the form and its presets (`src/form.rs`), the
  export (`src/export.rs`), the session's rolling history (`src/history.rs`),
  the charts' arithmetic (`src/chart.rs`), the client (`src/api.rs`) and the
  CLI (`src/cli.rs`) know nothing about iced, so the reducer, the layout and
  its hit-testing, the rate arithmetic, the observation ring and its gap
  segmentation, the CSV bytes, validation, preset bodies, transcript bytes,
  parsing and poll arithmetic are unit-tested headlessly; `src/app.rs` is the
  only place that turns an effect into an iced `Task` (and the only place that
  writes a file), and `src/mapview.rs` and `src/telemetryview.rs` the only
  places that paint.
* `src/theme.rs` and the widgets in `src/view.rs` echo
  `docs/ux-draft/owner-console.html`: the Plaza palette, mono for data and
  serif for prose, ruled sheets, and stamps for state.

## Tests

```
cargo test          # 245 lib + 19 client + 35 render = 299, 0 failed
```

* `src/**` — unit tests: the API's JSON against fixture bodies (including the
  machine and the console write, and a role's presence, absence and explicit
  null), the reducer and every pane transition (including the roster's grouping
  and its tally), the age/date/instant arithmetic, the map's
  layout/hit-test/rate arithmetic, its lane overflow and its role labels'
  character-wise elision, the exit-kind
  matrix, the console's history walk, the spawn form's boundaries and the
  presets' fill + byte-for-byte bodies, the export path's token arithmetic and
  its write (bytes read back off the disk), the help sheet's toggling, the poll
  schedule and backoff at more than one cadence, the serial tail (with the byte
  each row starts at), the key bindings (`w` among them), the CLI.
* the stream: the frame parser's whole matrix (the daemon's frames, a frame
  split mid-line across reads, a heartbeat, CRLF frames, a leading blank line,
  an unknown event kind, a data line with no kind, a malformed frame followed
  by a good one, a state this console does not know, a serial frame's escaped
  bytes), the head that confirms the subscription (a real socket, and three
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
  bodies (create **and** console write), the malformed/refused/unreachable
  paths, and the event stream: the frames off a real socket (the heartbeat is
  not an event, an unknown kind is not an event) and a reconnect whose second
  event comes off a second connection after the backoff.
* `tests/render.rs` — headless rendering with iced's own harness: the register
  drawn with the stub's runs, a click on a run fetching it and rendering the
  guest's serial, a click on `New run` opening the form, the presets row and a
  preset click filling the form then sending exactly its body, the machine
  sheet's every column, the map's legend and its plate (a real canvas frame
  whose substrate and marks are asserted in pixels, plus a fifty-VM lane's
  clipped tail and its amber `15 of 50 cells · 35 past` note), the console's
  quick buttons driving a real `POST .../input` whose body is read back off the
  stub, the export button writing two real transcripts whose bytes are compared
  against the pane's and read back off the disk, an export that cannot land
  drawn as a `REFUSED` notice, the help `?` sheet drawn over a dimmed register
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
  its creation stamped `2025-10-06`), and a full application frame written to
  `target/tmp/fleet-frame.rgba` (and the map's to `target/tmp/fleet-map-frame.rgba`,
  the telemetry pane's to `target/tmp/telemetry-frame.rgba`, the stream's to
  `target/tmp/stream-frame.rgba`).

Set `APZ_DESKTOP_LIVE=http://127.0.0.1:8899` to have `tests/client.rs` also
read a running daemon (read-only: it reads the machine, lists runs, and reads
each one with its serial and exit kind).
