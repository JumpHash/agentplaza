# AgentPlaza desktop

The AgentPlaza console as a native desktop app (iced + wgpu), at parity with the web console and
in the same design language (`apps/web/DESIGN.md`).

It opens on a startup chooser:

- **Start locally**: the hub and one node on this machine (loopback).
- **Host a hub**: a hub other machines join as nodes (HTTPS, owner token, join codes).
- **Join a hub**: this machine becomes a node of another hub (paste a join token).
- **Connect to a hub**: the console of a hub elsewhere (its URL and the owner token).

See `docs/plan-hub-nodes-desktop.md` for the hub/node design.

```
cargo run -p agentplaza-desktop
```

It is a member of the root workspace but not a default member, so a plain `cargo build` skips it.

The Floor and the machine stage draw the 3D engine of `crates/apz-engine`, the same engine the web
console runs as wasm; `console/floor_engine.rs` is the iced adapter. There is no three.js and no
`?wasm` switch: the engine is the 3D view on both.

Layout: `src/console/` — `mod.rs` (the app), `boot.rs` (the chooser), `services.rs` (local
services), `shell.rs` (sidebar, top bar, STOP ALL), `client.rs` (the hub client), `ui.rs` (the
design kit), `icons.rs` (the web console's lucide icons), `floor_engine.rs` (the 3D engine's
adapter), `picker.rs` (the model picker), `rich.rs` (chips and hover cards for agent text),
`screens/` (one module per screen, with its tests beside it). `src/theme.rs` holds the palette and
the type (Inter for the interface, Martian Mono for code). The shared board, IDE and rich-content
logic come from `crates/apz-board`, `crates/apz-ide` and `crates/apz-rich`.
