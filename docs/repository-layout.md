# Repository layout

Cargo workspace members (`Cargo.toml`), plus the non-Cargo parts. Each crate's line is the first sentence of its
module documentation (`src/lib.rs` or `src/main.rs`).

## Daemons and the node

| Path | Purpose |
|---|---|
| [`crates/apid`](../crates/apid) | The hub: the work engine (sessions, seats, threads, budgets, cards, chats), projects and tasks, the laws, the model gateway, the vault, the store, the Library, Weft's server, MCP servers, the IDE and the `/v1` API |
| [`crates/noded`](../crates/noded) | The node daemon: one machine's microVMs over a loopback HTTP API (its routes are listed in `crates/noded/src/lib.rs`) |
| [`crates/vsockd`](../crates/vsockd) | The node-side relay: the host end of the guest protocol, and the guest's egress proxy |
| [`crates/vmm`](../crates/vmm) | The multi-microVM monitor, derived from Firecracker (Apache-2.0; see `crates/vmm/NOTICE`) |
| [`crates/vmctl`](../crates/vmctl) | A developer tool that spawns N microVMs from the command line and reports how they stopped |
| [`crates/acpi-tables`](../crates/acpi-tables) | The ACPI tables a VM boots with |
| [`crates/agent-harness`](../crates/agent-harness) | The in-guest agent: the agent loop, the guest tools, the egress proxy and the sandbox daemon (`envd`) |
| [`crates/dispatchd`](../crates/dispatchd) | A separate fleet dispatcher binary; no other crate starts it |
| [`crates/apz-link`](../crates/apz-link) | One per node: joins a machine to a hub with a join token, and runs the hub's commands for its node |
| [`crates/apz-workstation`](../crates/apz-workstation) | The daemon that makes a machine an AgentPlaza workstation |
| [`crates/apz-station`](../crates/apz-station) | The hub-to-workstation link protocol |
| [`crates/apz-desktop`](../crates/apz-desktop) | The workstation's desktop control interface |
| `crates/apz-desktop-`[`x11`](../crates/apz-desktop-x11), [`wayland`](../crates/apz-desktop-wayland), [`atspi`](../crates/apz-desktop-atspi), [`macos`](../crates/apz-desktop-macos), [`windows`](../crates/apz-desktop-windows) | The workstation desktop backends, one per platform |
| [`crates/apz-setup`](../crates/apz-setup) | First-run checks of a plaza's requirements, for the desktop app and the CLI; it installs nothing |

## Weft and the workspace

| Path | Purpose |
|---|---|
| [`crates/weft`](../crates/weft) | Agent-native version control: one binary, the hub's history server, the node cache and the CLI (see [`design/weft.md`](design/weft.md)) |
| [`crates/apz-ide`](../crates/apz-ide) | The IDE's shared model: workspaces, trees, buffers, diffs, path rules and tab state |
| [`crates/apz-ide-web`](../crates/apz-ide-web) | The browser bindings of the IDE model |
| [`crates/apz-ops`](../crates/apz-ops) | The workspace operations the IDE and the agents share |
| [`crates/apz-vt`](../crates/apz-vt) | A terminal emulator's screen, pure Rust, with no UI or I/O |
| [`crates/apz-rich`](../crates/apz-rich) | The rich-content classifier: which references in agent text become chips and hover cards |
| [`crates/apz-graph`](../crates/apz-graph) | The task graph's layout for the Work screen |
| [`crates/apz-graph-web`](../crates/apz-graph-web) | The browser bindings of the task graph layout |
| [`crates/apz-artifact`](../crates/apz-artifact) | Compiles the scripts of HTML artifacts to plain JavaScript on the hub, with oxc |

## Storage, cache and blobs

| Path | Purpose |
|---|---|
| [`crates/apz-store`](../crates/apz-store) | The hub's store: SQLite in one folder, with the folder lock, the writer, the readers, migrations, backups and the lint |
| [`crates/apz-store-derive`](../crates/apz-store-derive) | `derive(Table)` and `derive(Column)` for `apz-store` |
| [`crates/apz-blobs`](../crates/apz-blobs) | The object store: blobs by BLAKE3 hash, in append-only packs |
| [`crates/apz-cache`](../crates/apz-cache) | Caches kept true by generations, not by events |
| [`crates/apz-plaza`](../crates/apz-plaza) | The plaza's domain state: the task ledger and the board, as pure state with no I/O |
| [`crates/apz-metrics`](../crates/apz-metrics) | The metrics core shared by the hub, the nodes and the CLI: series, histograms, rollups and alert rules; no I/O |
| [`crates/apz-metrics-view`](../crates/apz-metrics-view) | The Metrics view model |

## Security, sandboxes and transport

| Path | Purpose |
|---|---|
| [`crates/apz-sandbox`](../crates/apz-sandbox) | Runs a child process under the operating system's own sandbox (Seatbelt, Landlock, AppContainer), with no container runtime |
| [`crates/apz-egress-rules`](../crates/apz-egress-rules) | The destinations an egress proxy refuses, held in one place |
| [`crates/apz-trust`](../crates/apz-trust) | Trust between the hub and its nodes, and the owner's access to the hub: join tokens, node keys, pinned TLS and owner tokens |
| [`crates/apz-proto`](../crates/apz-proto) | The wire types: the guest-to-hub line protocol and the provider-neutral chat model |
| [`crates/apz-http`](../crates/apz-http) | The one small HTTP/1.1 server, client and SSE implementation, with the loopback `Host` guard |
| [`crates/apz-uds`](../crates/apz-uds) | Unix-domain stream sockets with one API on every platform |
| [`crates/apz-os`](../crates/apz-os) | What differs between the platforms AgentPlaza runs on, in one place |
| [`crates/apz-gateway`](../crates/apz-gateway) | The model gateway: one blocking client for every model API |
| [`crates/apz-acp`](../crates/apz-acp) | Agent Client Protocol support, for the coding engines a VM runs |
| [`crates/jhrl`](../crates/jhrl) | The JHRL law engine, which the laws are written in |

## Consoles and shared views

| Path | Purpose |
|---|---|
| [`apps/web`](../apps/web) | The web console and the share viewer: TypeScript and esbuild, no framework; `dist/` is built by CI |
| [`apps/desktop`](../apps/desktop) | The native desktop app (iced and wgpu), at parity with the web console |
| [`apps/cli`](../apps/cli) | The `agentplaza` command and its terminal UI |
| [`apps/shared`](../apps/shared) | Shared app data: onboarding, the built-in themes and their generated tokens, the highlighter's rules |
| [`crates/apz-engine`](../crates/apz-engine) | The 3D engine for the Machines screen and the machine view (wgpu), platform-agnostic |
| [`crates/apz-engine-web`](../crates/apz-engine-web) | The browser shell of the 3D engine (wasm-bindgen; WebGPU, WebGL2 fallback) |
| [`crates/apz-theme`](../crates/apz-theme) | Themes for both consoles: the token list, `.apztheme.json`, the contrast pass and the backdrop |
| [`crates/apz-theme-web`](../crates/apz-theme-web) | The browser bindings of the theme engine |
| [`crates/apz-view`](../crates/apz-view) | The panel model both consoles render, the formatter and the artifact validation and caps |
| [`crates/apz-prompt`](../crates/apz-prompt) | The prompt library's core: checks and a score for system prompts and AGENTS.md files |
| [`crates/apz-skill`](../crates/apz-skill) | The skill library's core: SKILL.md parsing, checks, zip import and export |

## Tools, tests and packaging

| Path | Purpose |
|---|---|
| [`tools/apz-pack`](../tools/apz-pack) | Packages the web engine for the browser: cargo, wasm-bindgen, wasm-opt, content-hashed names |
| [`tools/apz-kit`](../tools/apz-kit) | The machine model kit: builds the Machines view's models as glTF binaries with KTX2 textures |
| [`trainer/`](../trainer) | The Foundry trainer (`apz_train`, Python, with CPU, CUDA and ROCm images) |
| [`tests/acceptance`](../tests/acceptance) | The acceptance harness (Python standard library) |
| [`fuzz/`](../fuzz) | Fuzz targets |
| [`packaging/`](../packaging) | Stage and package scripts for Linux, macOS, Windows and Raspberry Pi; `next-version.sh` picks the release version |
| [`scripts/`](../scripts) | Guest kernel and initramfs builders, the store test runner, the role generator and hooks |
| [`assets/`](../assets) | Fonts, the task graph's fixtures, and the models and data the 3D engine loads (`hardware`, `rooms`, `nav`, `robots`) |
| [`docs/`](../docs) | Architecture, design notes, the API and the developer guides (see [Documentation](../README.md#documentation)) |

The hardware board model and the relay container for MCP servers are gone: the hub reaches MCP servers through its own
egress proxy and the operating system's sandbox.

## Snapshot, egress and sandbox modules

Modules inside the crates above that carry the VM snapshot, egress credential and sandbox work. Each has a module
documentation comment that is the reference for its contract.

| Module | Crate | Purpose |
|---|---|---|
| `vstate/snapshot.rs` | `vmm` | The snapshot format: `vmstate.json` and `memory.img`, the version and architecture checks |
| `vstate/snapshot_memory.rs` | `vmm` | Guest memory in snapshots: touched pages, and the anonymous-page diff of a restored VM |
| `control.rs` | `vmm` | `VmControl`: pause, resume and snapshot of a running VM, behind the device gate |
| `clonefile.rs` | `vmm` | Reflink copies of images, with a sparse copy as the fallback |
| `cgroup.rs` | `vmm` | cgroup v2 limits for a VM in its own process |
| `spawner.rs` | `vmm` | `VmSpec::restore`, `SnapshotKind`, the in-process and per-process boot modes |
| `snapshots.rs` | `noded` | Where a node keeps its snapshots, per CID, and the snapshot views |
| `templates.rs` | `noded` | Warm templates: built on first use, restored for new VMs of the same shape |
| `migrate.rs` | `noded` | Export and import of a sleeping VM, in chunks, with zeros left as holes |
| `workstation.rs` | `noded` | The workstation VMs a node makes for the hub |
| `mcp_vm.rs`, `train.rs` | `noded` | MCP server VMs and the Foundry's training jobs |
| `sleep.rs` | `apid` | The idle rule, sleep, and waking before work is delivered |
| `vm_snapshots.rs` | `apid` | Checkpoints, rollback with a safety checkpoint, forks, automatic checkpoints |
| `placement.rs` | `apid` | Choosing a node for a new session, moving an agent |
| `mover.rs` | `apid` | Stopping and resuming nodes: moving their agents off and back, rebalancing |
| `egress_inject.rs` | `apid` | Egress credential rules, the egress CA and leaves, the per-request decision |
| `e2b.rs` | `apid` | The E2B-compatible sandbox API: keys, sandboxes, snapshots, the reaper |
| `egress_tls.rs` | `vsockd` | TLS interception for hosts with a rule: header injection, framing, audit |
| `e2b.rs` | `vsockd` | The E2B gateway: relays the SDK's requests into a sandbox's daemon |
| `restore.rs` | `agent-harness` | Clock and entropy catch-up after a restore, from the hub's `HostFacts` |
| `trust.rs` | `agent-harness` | The guest's trust in the egress CA: the file, the bundle and the system store |
| `envd/` | `agent-harness` | The sandbox daemon, E2B's envd: `process`, `filesystem`, `files`, `/init` and `/health` |

The design notes for these are [`design/vm-snapshots.md`](design/vm-snapshots.md),
[`design/egress-credentials.md`](design/egress-credentials.md) and [`design/e2b.md`](design/e2b.md).
