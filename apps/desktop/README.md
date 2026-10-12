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
cd apps/desktop
cargo run
```

It is its own cargo workspace (`[workspace]` in `Cargo.toml`, with its own `Cargo.lock`).

Layout: `src/console/` — `mod.rs` (the app), `boot.rs` (the chooser), `services.rs` (local
services), `shell.rs` (sidebar, top bar, STOP ALL), `client.rs` (the hub client), `ui.rs` (the
design kit), `icons.rs` (the web console's lucide icons), `screens/` (one module per screen).
