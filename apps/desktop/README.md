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
* the index column: the fleet entry, the **machine**, the **fleet map**, every
  run, the fleet tally (runs, VMs up, distinct guest CIDs, vsock runs), the
  **guest memory the live VMs have committed** against the host's total as the
  machine reports it (in critical ink, with `· overcommit`, when the fleet has
  been asked for more than the host has), the poll cursor, a **Help** button,
  and the keys;
* the fleet register: every run with its state, live/total VMs, vCPUs, memory,
  vsock base CID and age;
* the **machine** sheet: `GET /api/v0/machine` as a ruled table — name, arch,
  backend, irqchip, class, cpus, memory, the vsock transport mode, when the
  daemon started, and its uptime ticking with the poll;
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
* the run pane: per-VM state, guest CID, **exit kind** (`SLEPT` from
  `reboot: Power down`, `RESET` from `machine restart`/`Restarting system`, a
  dash when the tail says nothing), serial rate, and the latest line each VM
  wrote; the run's facts; the guest's serial transcript (the last 400 lines of
  the *selected* VM) with a follow toggle;
* the **serial console**: an input line, `SEND`, the guest's own quick commands
  (`help`, `uname`, `cpu`, `mem`, `uptime`, `poweroff` — the same set the web
  console offers), and an up/down history. A line is written to the selected VM
  with `POST /api/v0/runs/{id}/input` and the daemon's answer moves the
  transcript on;
* **transcript export**: a path box (iced offers no file dialog and the crate
  takes no dependency for one) pre-filled with
  `{temp}/plaza-{run}-{vm}.txt`, a `this vm` / `all vms` switch, and an `EXPORT`
  button that writes the transcript — the pane's own bytes, with a header line
  naming the run, the VM, its state, CID and exit kind — with plain `std::fs`.
  A run export writes one labelled section per VM. A write that cannot land is
  a refusal in the notice line, in the disk's own words, and never touches the
  daemon's connection stamp. A path you type is kept; emptying the box hands it
  back to the derived default;
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
* The model (`src/model.rs`), the map's geometry and activity (`src/map.rs`),
  the exit kinds (`src/exit.rs`), the form and its presets (`src/form.rs`), the
  export (`src/export.rs`), the client (`src/api.rs`) and the CLI
  (`src/cli.rs`) know nothing about iced, so the reducer, the layout and its
  hit-testing, the rate arithmetic, validation, preset bodies, transcript
  bytes, parsing and poll arithmetic are unit-tested headlessly; `src/app.rs`
  is the only place that turns an effect into an iced `Task` (and the only
  place that writes a transcript), and `src/mapview.rs` the only place that
  paints.
* `src/theme.rs` and the widgets in `src/view.rs` echo
  `docs/ux-draft/owner-console.html`: the Plaza palette, mono for data and
  serif for prose, ruled sheets, and stamps for state.

## Tests

```
cargo test
```

* `src/**` — unit tests: the API's JSON against fixture bodies (including the
  machine and the console write), the reducer and every pane transition, the
  map's layout/hit-test/rate arithmetic and its lane overflow, the exit-kind
  matrix, the console's history walk, the spawn form's boundaries and the
  presets' fill + byte-for-byte bodies, the export path's token arithmetic and
  its write (bytes read back off the disk), the help sheet's toggling, the poll
  schedule and backoff at more than one cadence, the serial tail, the key
  bindings, the CLI.
* `tests/client.rs` — the client over real sockets against a `std::net` daemon
  stub (`tests/common/mod.rs`), asserting the exact request line, headers and
  bodies (create **and** console write), and the malformed/refused/unreachable
  paths.
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
  and a full application frame written to `target/tmp/fleet-frame.rgba` (and
  the map's to `target/tmp/fleet-map-frame.rgba`).

Set `APZ_DESKTOP_LIVE=http://127.0.0.1:8899` to have `tests/client.rs` also
read a running daemon (read-only: it reads the machine, lists runs, and reads
each one with its serial and exit kind).
