# Repository layout

Cargo workspace members (`Cargo.toml`), plus the non-Cargo parts.

| Path | Purpose |
|---|---|
| [`crates/apid`](../crates/apid) | The hub: plaza state, the API, model gateway, vault, MCP, git and Library, laws, the Foundry |
| [`crates/noded`](../crates/noded) | The node daemon: VM lifecycle, the fleet, serial capture, machine facts, the workstation VMs |
| [`crates/vsockd`](../crates/vsockd) | The node's vsock hub: guest sessions, heartbeats, relays guest traffic to the hub |
| [`crates/vmm`](../crates/vmm) | One jailed microVM process, derived from Firecracker (see `crates/vmm/NOTICE`) |
| [`crates/vmctl`](../crates/vmctl) | Spawns N microVMs from the command line |
| [`crates/acpi-tables`](../crates/acpi-tables) | The ACPI table builder used by the VMM |
| [`crates/agent-harness`](../crates/agent-harness) | The in-guest agent: the agent loop and the guest-side tools |
| [`crates/dispatchd`](../crates/dispatchd) | The dispatcher: claims open roled tasks for connected agents and drives them |
| [`crates/apz-link`](../crates/apz-link) | Joins a machine to a hub with a join code and carries the hub's commands to its node |
| [`crates/apz-workstation`](../crates/apz-workstation) | The workstation daemon: joins a hub, serves files, terminals, runs, screens, input and STOP ALL |
| [`crates/apz-station`](../crates/apz-station) | The hub-to-workstation link protocol |
| [`crates/apz-desktop`](../crates/apz-desktop) | The workstation desktop-control trait, frame helpers and a fake desktop for tests |
| `crates/apz-desktop-`[`x11`](../crates/apz-desktop-x11), [`wayland`](../crates/apz-desktop-wayland), [`atspi`](../crates/apz-desktop-atspi), [`macos`](../crates/apz-desktop-macos), [`windows`](../crates/apz-desktop-windows) | Workstation desktop backends per platform (screen, input, accessibility tree, apps) |
| [`crates/apz-ops`](../crates/apz-ops) | Confined file operations, searches, bounded runs and PTY terminals |
| [`crates/apz-vt`](../crates/apz-vt) | The terminal emulator core (VT screen, scrollback) |
| [`crates/apz-plaza`](../crates/apz-plaza) | The plaza's domain: tasks, treasury, agora, board, conversations, council (pure state) |
| [`crates/apz-gateway`](../crates/apz-gateway) | The model gateway: one client for every model provider API |
| [`crates/apz-proto`](../crates/apz-proto) | Wire types: the guest-to-hub line protocol and the provider-neutral chat model |
| [`crates/apz-http`](../crates/apz-http) | The one small HTTP/1.1 server, client and SSE implementation, with the loopback Host guard |
| [`crates/apz-uds`](../crates/apz-uds) | Unix-domain sockets on every platform |
| [`crates/apz-trust`](../crates/apz-trust) | Join tokens, node keys, signed requests, pinned TLS and owner tokens |
| [`crates/apz-os`](../crates/apz-os) | Platform differences: state locations, private files, randomness, detached processes |
| [`crates/apz-setup`](../crates/apz-setup) | First-run setup of a local plaza: checks requirements and fixes the missing ones |
| [`crates/apz-engine`](../crates/apz-engine) | The 3D engine for the Machines screen and the machine view (wgpu), platform-agnostic |
| [`crates/apz-engine-web`](../crates/apz-engine-web) | The browser shell of the engine (wasm-bindgen; WebGPU, WebGL2 fallback) |
| [`crates/apz-board`](../crates/apz-board) | The hardware-board model shared by the desktop and the web (WASM) |
| [`crates/apz-ide`](../crates/apz-ide) | The IDE model: workspaces, entries, buffers, diffs, path rules, state reducer |
| [`crates/apz-ide-web`](../crates/apz-ide-web) | The browser bindings of the IDE model |
| [`crates/apz-graph`](../crates/apz-graph) | The task graph's layout for the Work screen: columns by links, card order, routed links (golden fixtures in `assets/graphs`) |
| [`crates/apz-graph-web`](../crates/apz-graph-web) | The browser bindings of the task graph layout |
| [`crates/apz-rich`](../crates/apz-rich) | The rich-content classifier: which references become chips and hover cards |
| [`crates/apz-skill`](../crates/apz-skill) | The skill library core: SKILL.md parsing, checks, zip import and export |
| [`crates/apz-prompt`](../crates/apz-prompt) | The prompt library core: checks and scores for system prompts and AGENTS.md |
| [`crates/jhrl`](../crates/jhrl) | The JHRL law engine: lexer, parser, dimension checker, evaluator, model checker |
| [`crates/mcp-relay`](../crates/mcp-relay) | The relay container for MCP servers (a pipe to the hub's egress proxy) |
| [`apps/web`](../apps/web) | The web console: TypeScript, esbuild, committed `dist/` (CSP, `textContent` only) |
| [`apps/desktop`](../apps/desktop) | The native desktop app (iced + wgpu), at parity with the web console |
| [`apps/cli`](../apps/cli) | The `agentplaza` command and its terminal UI |
| [`apps/shared`](../apps/shared) | Shared app data (onboarding) |
| [`tools/apz-pack`](../tools/apz-pack) | Builds the web engine for wasm: cargo, wasm-bindgen, wasm-opt, content-hashed names |
| [`trainer/`](../trainer) | The Foundry trainer (`apz_train`, Python, with CPU, CUDA and ROCm images) |
| [`tests/acceptance`](../tests/acceptance) | The acceptance harness (Python standard library): suites for the demo, red team, kill and resume, jail, budgets, fleet, treasury, social, agora and council |
| [`fuzz/`](../fuzz) | Fuzz targets (the block-request parser) |
| [`packaging/`](../packaging) | Stage and package scripts for Linux, macOS, Windows and Raspberry Pi |
| [`scripts/`](../scripts) | Guest kernel and initramfs builders, database test runner, migration checks, hooks |
| [`docs/`](../docs) | Architecture, design notes, agent contracts and the red-team catalog (see [Documentation](../README.md#documentation)) |

## Snapshot, egress and sandbox modules

Modules inside the crates above that carry the VM snapshot, egress credential and E2B work. Each has a module
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
| `sleep.rs` | `apid` | The idle rule, sleep, and waking before work is delivered |
| `vm_snapshots.rs` | `apid` | Checkpoints, rollback with a safety checkpoint, forks, automatic checkpoints |
| `placement.rs` | `apid` | Choosing a node for a new agent, moving an agent, draining a node |
| `egress_inject.rs` | `apid` | Egress credential rules, the egress CA and leaves, the per-request decision |
| `e2b.rs` | `apid` | The E2B-compatible sandbox API: keys, sandboxes, snapshots, the reaper |
| `egress_tls.rs` | `vsockd` | TLS interception for hosts with a rule: header injection, framing, audit |
| `e2b.rs` | `vsockd` | The E2B gateway: relays the SDK's requests into a sandbox's daemon |
| `restore.rs` | `agent-harness` | Clock and entropy catch-up after a restore, from the hub's `HostFacts` |
| `trust.rs` | `agent-harness` | The guest's trust in the egress CA: the file, the bundle and the system store |
| `envd/` | `agent-harness` | The sandbox daemon, E2B's envd: `process`, `filesystem`, `files`, `/init` and `/health` |

The design notes for these are [`design/vm-snapshots.md`](design/vm-snapshots.md),
[`design/egress-credentials.md`](design/egress-credentials.md) and [`design/e2b.md`](design/e2b.md).
