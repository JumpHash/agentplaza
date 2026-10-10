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
agentplaza-desktop [--url <url>] [--timeout-secs <seconds>]
```

| | |
|---|---|
| `--url <url>` | the node daemon to read; `http://127.0.0.1:8899` by default |
| `--timeout-secs <n>` | how long one call may take, 1–600; 5 by default |
| `-h`, `--help` | the usage text |
| `APZ_DESKTOP_URL` | the same as `--url` |
| `APZ_DESKTOP_TIMEOUT_SECS` | the same as `--timeout-secs` |

An argument wins over the environment. Only a **loopback** host is accepted,
and only an exact one: `127.0.0.1` (or `localhost`, normalized to it), never
resolved through DNS. Any other host is refused before a socket is opened.

```sh
cargo run -- --url http://127.0.0.1:8899
APZ_DESKTOP_URL=http://127.0.0.1:9000 cargo run
```

## What it does

* polls `GET /api/v0/runs` every two seconds, backing off to 30 s while calls
  fail, and reports the connection as a stamp: `LIVE`, `STALE`, `DOWN`;
* the fleet register: every run with its state, live/total VMs, vCPUs, memory,
  vsock base CID and age;
* the run pane: per-VM state, guest CID and serial byte count, the run's facts,
  and the guest's serial transcript (the last 400 lines) with a follow toggle
  that keeps the newest line in view;
* **New run**: the daemon's own create fields — `n`, `vcpus`, `memory_mib`,
  `beats`, `vsock_cid` — validated client-side with the daemon's ranges
  (`n`/`vcpus` 1–64, `memory_mib` ≥ 128, `vsock_cid` ≥ 3, and the run's last
  guest CID inside the kernel's CID space) and the daemon's wording;
* **KILL**: `DELETE /api/v0/runs/{id}`, two presses (arm, then confirm);
* every failure is a notice line and an error in the index — no request path
  panics, and nothing is swallowed.

## How it is built

* `iced` with its **wgpu** renderer (the WebGPU-compatible backend); the
  `tiny-skia` software renderer stays enabled as the compositor fallback, and
  the executor is `smol` — deliberately not `tokio`.
* The API client is a tiny blocking HTTP/1.1 client over `TcpStream`, loopback
  only, with no HTTP client dependency — the same shape
  `crates/noded/tests/common/mod.rs` uses to test the daemon.
* The model (`src/model.rs`), the form (`src/form.rs`), the client
  (`src/api.rs`) and the CLI (`src/cli.rs`) know nothing about iced, so the
  reducer, validation, parsing and poll arithmetic are unit-tested headlessly;
  `src/app.rs` is the only place that turns an effect into an iced `Task`.
* `src/theme.rs` and the widgets in `src/view.rs` echo
  `docs/ux-draft/owner-console.html`: the Plaza palette, mono for data and
  serif for prose, ruled sheets, and stamps for state.

## Tests

```
cargo test
```

* `src/**` — unit tests: the API's JSON against fixture bodies, the reducer and
  every pane transition, the spawn form's boundaries, the poll schedule and
  backoff, the serial tail, the CLI.
* `tests/client.rs` — the client over real sockets against a `std::net` daemon
  stub (`tests/common/mod.rs`), asserting the exact request line, headers and
  create body, and the malformed/refused/unreachable paths.
* `tests/render.rs` — headless rendering with iced's own harness: the register
  drawn with the stub's runs, a click on a run fetching it and rendering the
  guest's serial, a click on `New run` opening the form, and a full
  application frame whose pixels are asserted (the page, the ink, a running
  stamp, a killed stamp) and written to `target/tmp/fleet-frame.rgba`.

Set `APZ_DESKTOP_LIVE=http://127.0.0.1:8899` to have `tests/client.rs` also
read a running daemon (read-only: it lists runs and reads each one).
