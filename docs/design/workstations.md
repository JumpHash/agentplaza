# Workstations

Status: plan (2026-10-14). Nothing here is built. No code changes are part of this document.

Owner's request: agents that work through the hub should be able to use real machines (a Mac mini, a Linux desktop,
a Windows laptop) and full VMs, with complete control: the GUI, the shell, the files, the GPUs, and every program
installed there (Blender, compilers, browsers, anything).

## 1. Goal and the owner's decisions

A **workstation** is a machine agents operate. It is not a machine that hosts agents: that is a node. An agent lives
in its own VM as today and works a workstation remotely, through the hub.

Decisions taken with the owner (2026-10-14):

| Question | Decision |
|---|---|
| Dedicated or mixed machines | Mixed. A workstation can be a machine the owner also uses. |
| Approvals | None by default. The agents just use the machine. |
| Install | The plaza's software is installed and the existing setup gets a new option, **Add as a workstation**. |
| Physical or VM | Both, with the same software. The agents do not care which. |
| OS account the agent works in | Chosen in setup: a separate OS user created for the agents, or the owner's own account. |
| Owner and agent at the same time | Allowed. The agent works while the owner works. |
| Agents per machine | Any number the machine supports. |
| Which agents | Like any other tool: granted by role and by the tool policy, used per task. |
| Node and workstation on one machine | Yes: one install, both roles, the GPU shared. |
| Results | Both: deliverables are copied into the Library, and a work folder syncs both ways. |
| Owner takeover | Configurable per workstation in its setup on the hub (section 7). |
| App support | One full, general adapter: the agents can work on anything installed, not only apps with a dedicated adapter (section 6). |
| In the apps | A new machine kind on Machines and the Floor with a live screen thumbnail, and a Workstations page. |
| Recordings | Configurable on the hub, from the web, the desktop app and the CLI/TUI. |

What stays true without approvals: the laws still classify every action, every action is audited, STOP ALL reaches
workstations, and the owner can watch and take over at any time.

## 2. Containment

The containment rule holds: an agent never runs on, or reaches, the hub's own host or another agent.

- A workstation is never the machine that runs `apid`. Setup refuses to add the hub's own host as a workstation.
- Agents get no network path to a workstation. Every action goes agent VM, then hub, then workstation daemon, and
  the hub checks it on the way, the same as tool calls today.
- The owner's account is used only when the owner chose it in setup. That choice is shown on the machine everywhere
  it appears, as a warning badge: "agents act as <user>".

## 3. The workstation daemon

One program, `apz-workstation`, installed by the plaza's installers on macOS, Windows and Linux, in a VM or on bare
metal. Setup's new option **Add as a workstation** (desktop app, CLI `agentplaza workstation join <code>`) enrolls it
with a join code, the way a node joins today. It dials out to the hub over the pinned TLS link `apz-link` uses, so the
machine opens no ports and can sit behind NAT.

It offers these capabilities to the hub:

| Capability | Built from |
|---|---|
| Shell and terminals | the IDE terminal (`ide_term_*` in the harness): PTY, write, polled read, resize, close |
| Files: list, read, write, search | the IDE file tools (`ide_list`, `ide_read`, `ide_write`, `ide_search`) and `apz-ide` path rules |
| Runs with timeout and output cap | the IDE runs (`ide_run_*`) |
| Screen: screenshots, live frames | per platform, section 4 |
| Input: mouse, keyboard, scroll, drag, clipboard | per platform, section 4 |
| UI tree: windows, controls by name, their actions | the OS accessibility API, section 4 |
| Apps: list installed, launch, focus, quit | per platform |
| Facts: OS, CPUs, RAM, GPUs and VRAM, displays | the way noded reports machine facts |

The shell, file and run code is the IDE's, moved into a shared crate that both the harness (inside agent VMs) and
the workstation daemon use, so the IDE can also open a workstation as a workspace (`station:<id>`).

## 4. Screen, input and UI tree by platform

| Platform | Screen | Input | UI tree | Owner grants once |
|---|---|---|---|---|
| macOS | ScreenCaptureKit | CGEvent | Accessibility (AXUIElement) | Screen Recording, Accessibility |
| Windows | Windows.Graphics.Capture | SendInput | UI Automation | none; a session helper runs in the user's session next to the service |
| Linux | PipeWire screencast portal, or X11 | libei or uinput | AT-SPI | the portal prompt on Wayland |

Windows' secure desktop (UAC prompts, the lock screen) cannot be driven. That is kept, not worked around.

## 5. Working alongside the owner

The owner chose "let the agent work while the user is working". The design makes that possible by keeping the agent
off the owner's pointer whenever it can:

1. **APIs and command lines first.** Blender headless (`blender -b file.blend --python job.py`) renders on the GPU with
   no window at all; the same for ffmpeg, builds and test runners. For deeper work the general adapter (section 6)
   reaches any app's own automation interface.
2. **The UI tree second.** Accessibility actions (Invoke, SetValue, Select, AXPress) operate a control without moving
   the owner's mouse or stealing focus.
3. **Pixels last.** A real click or keystroke takes the pointer. Where the platform allows it, pixel work happens in a
   separate session the owner never sees:
   - Linux: one virtual display per agent (a headless Wayland compositor or Xvfb) under the agent user.
   - macOS: a second user's session on a virtual display (Screen Sharing to another user).
   - Windows client editions allow one active desktop session. Pixel work there either uses the owner's session
     (the agent and the owner share the pointer; the IDE-style banner "agent at work" shows, and any owner input pauses
     the agent's pointer for a few seconds) or runs in a Windows VM workstation. Windows Server and VMs give one session
     per agent.

So any number of agents can work a machine at once through shells, runs, files, apps and the UI tree, and pixel
sessions are as many as the platform supports: one per agent on Linux and macOS, one shared on Windows client.

## 6. How agents use a workstation

Workstation tools join the agent's tool set like any other tool (granted by role and tool policy, visible in Tools):

- `station_list`, `station_lease` / `station_release` (a lease is per agent and task; leases are queued only for a
  pixel session that cannot be shared).
- `screenshot` (scaled, with an optional coordinate grid), `click`, `type`, `key`, `scroll`, `drag`, `wait`.
- `ui_tree`, `ui_act` (find a control by role and name, then act on it).
- `shell` (a terminal), `run`, `files_*`, `app_launch`, `app_list`.
- `app_script`: the general adapter. The owner chose one full adapter so the agents can work on anything, not a list
  of per-app adapters. It gives an agent every automation surface the OS and its apps already offer, so any installed
  program is reachable:
  - the program's command line and its embedded scripting (Blender's `--python`, GIMP's Script-Fu, LibreOffice's
    headless macros, Office through COM), discovered from `app_list` (each app's binary, its CLI help, its scripting
    entry points);
  - the OS scripting bridges: AppleScript and JXA on macOS, PowerShell and COM/UI Automation on Windows, D-Bus and
    AT-SPI on Linux;
  - the UI tree and pixels (above) for anything with no scripting surface.
  An agent that works an app often can save what it learned as a skill (the skill library), so the next agent
  starts from a working script rather than from pixels.

The tools are offered over the hub MCP surface as well, so Claude Code and other MCP clients can drive a workstation
through the hub with an owner token.

## 7. Hub side

- `crates/apid/src/stations.rs`: enrollment (join codes, drain, revoke, as `nodes.rs`), the station registry with
  facts and health, leases, and the tool routes the agents' calls go through. The laws classify every station action
  (`laws.rs`), and every action is audited in Postgres (an append-only `station_audit` table, like `ide_audit`).
- Live view: `WS /v1/stations/{id}/screen` streams frames to the owner (WebSockets only, project rule): compressed
  tiles for the first version, H.264 through WebCodecs later. `WS /v1/stations/{id}/control` carries the owner's
  takeover input.
- Recordings: per lease, screenshots and actions with timestamps, stored as blobs in Garage with an index row in
  Postgres, and each frame's capture time in `station_recording_frames`, which playback runs on. Settings on the hub: on or
  off, retention days, frame rate (1 to 5 a second, the hub's sampler cap), and whether to record the owner's takeovers.
  Edited from the web, the desktop app and the CLI/TUI (`agentplaza stations recordings ...`).
- Workstation setup on the hub (per workstation, edited from the web, the desktop app and the CLI/TUI): the agent OS
  account (chosen at enrollment), recording on or off and retention, the work folder, snapshot-and-reset for VMs, and
  the **takeover policy**: when the owner takes over a pixel session, either the agent waits and resumes when the
  owner gives the session back, or the agent's lease is released and its task goes back to the queue, or the agent
  keeps working through APIs, shells and the UI tree while only its pointer is paused.
- STOP ALL: the daemon freezes input and ends pixel sessions at once; shells and runs are stopped.
- Treasury: a lease's wall time (from its activation to its release) is charged to its task at the station's
  `rate_per_hour` (plaza credits, 1 credit is $0.001; 0, the default, is free), rounded up, capped at the task's
  escrow, as real spend. It shows in Usage as `workstation/<station>`. GPU time is not charged yet: the lease charge
  is the whole of it (`stations/hooks.rs`).

## 8. Results

Both paths the owner chose:

- **Deliverables:** an agent marks a file as a deliverable and it is copied into the plaza Library (Garage), exactly
  as files from agent VMs are today.
- **Work folder sync, both ways:** each workstation has a work folder (default `~/AgentPlaza` under the agent's OS
  user, changeable in the workstation's setup) that syncs with the hub continuously in both directions,
  content-addressed, so outputs land in the plaza even if nobody marks them, and files the owner or other agents put
  there on the hub appear on the machine. A conflicting edit on both sides keeps both copies and reports the conflict
  on the Workstations page.
  - A work folder that is missing or cannot be listed is reported as such, never as empty, and a pass that would
    delete more than 20 files and a quarter of the mirror pauses the station's sync instead: nothing moves, the
    station shows its reason and the deletions it holds, and the owner resumes it, applying the deletions or restoring
    the files from the hub.
  - An agent's `station_files` call with `deliver` puts a mirrored file into the Library, credited to the agent.

## 9. In the apps (web, desktop, CLI/TUI)

- Machines and the Floor: a new machine kind, **workstation**, with a live screen thumbnail, its OS, GPUs and current
  leases. On the Floor it gets its own 3D model per OS family.
- A **Workstations** page: list, enroll (join code), facts, leases and the queue, live view, take over and give back,
  recordings with playback, the work folder, and the recording settings.
- The IDE opens a workstation as a workspace (`station:<id>`) for files and terminals.
- CLI/TUI: `agentplaza workstation join|status|leave` on the machine itself, and `agentplaza stations ...` against the
  hub (list, show, leases, recordings, settings); a Workstations tab in the TUI.

## 10. Full VMs

A workstation VM runs the same daemon. The plaza can also make them:

- Linux and Windows desktop VMs on a node, with GPU passthrough (VFIO) when a task needs the real GPU, e.g. Blender
  with CUDA or OptiX, and a virtual GPU otherwise.
- Snapshots before a lease and reset after, when the owner wants clean machines (a per-workstation setting).
- macOS VMs only on Apple hardware (Apple's Virtualization framework), at most two per Mac by Apple's licence.

A Linux VM has no NIC (rule 0010), so its daemon reaches the hub over vsock through its node. The node relays the
station link to the hub byte for byte (`noded`'s `ws_relay`, at most four connections per VM), and the daemon's pinned
TLS runs end to end with the hub; the relay never reads it. The hub is `vsock://2:5100` as the VM dials it. The join
config (that address, the hub's pin and the join code) is served once on `vsock://2:5101` and never rides on the kernel
command line. The guest recipe is `scripts/mk-workstation-guest.sh`, which also builds the initramfs that mounts the
desktop disk.

Both backends join the same way. Every workstation VM has a CID, `4096` plus its sequence number (`cid_for`), and a VM
with a hub gets a vsock device with it. A plaza VM's connections reach the relay through the VMM's muxer, as unix sockets
`{vsock_dir}/vm{cid}_{port}`. A QEMU VM's connections reach it through the host's own `AF_VSOCK` listeners on ports 5100
and 5101 (`vmm::host_vsock`, started with the first QEMU VM and closed with the last). Those listeners serve a connection
only when its peer's CID belongs to a running QEMU workstation VM of this node. A VM's relay runs while the VM does.
The kernel command line carries only the workstation's name and `plaza.workstation_node=vsock://2:5101`, never the hub's
address or the code. A QEMU VM left running by a previous noded is taken back as running, with its relay restarted, when
its pidfile names a live QEMU and its QMP socket answers; otherwise it shows as stopped.

A Windows workstation VM runs under QEMU with UEFI, and its disk and network must come from the virtio-win drivers:

- `viostor`, the virtio-blk storage driver. The launch attaches the disk over AHCI (SATA), which Windows boots without a
  driver, so `viostor` is not needed for the current launch; the image carries it so a switch to virtio-blk needs no
  rebuild.
- `viosock`, the virtio-vsock driver, for the daemon's AF_VSOCK link to the relay. Without it the guest has no vsock and
  cannot join.
- `netkvm` is not needed: a workstation VM has no NIC (rule 0010).

The daemon is installed as a logon task (not a Windows service), so it starts when the owner signs in, in that user's
session. Windows has no kernel command line, so its daemon uses the node address
`vsock://2:5101` directly. The Windows daemon connects through the viosock provider (`apz-workstation`'s `vsock.rs`: a
WinSock AF_VSOCK socket, family 40, with the same `SOCKADDR_VM` layout as Linux). A guest without the driver fails with
a message that names it. The node side is the same for both backends.

## 11. Phases

1. **Enroll and headless work.** `apz-workstation` with join codes and setup's new option, shells, files and runs (the
   IDE code, shared), facts, the hub registry, station tools for agents, the Workstations page without live view, the
   IDE opening `station:<id>`. Most of the value (Blender headless, GPU jobs, builds) for the least new code.
2. **GUI on Linux and Windows.** Screen, input, UI tree, pixel sessions (virtual displays on Linux, the shared session
   on Windows), live view, takeover, recordings and their settings.
3. **macOS** and the third OS's polish; the general adapter's scripting bridges (`app_script`), the two-way work
   folder sync, and the takeover policies.
4. **Plaza-made VMs:** snapshots, reset per lease, GPU passthrough.

Each phase ships on web, desktop and CLI together (the parity rule).

## 12. Risks

- **Shared Windows sessions.** An agent's pixel work and the owner's typing collide. Mitigation: APIs and the UI tree
  first, the owner-input pause, a Windows VM for heavy pixel work.
- **The owner's own account.** When chosen, agents can see everything that account sees. Setup says so plainly and the
  badge stays visible.
- **GPU contention** between agent VMs (node role) and workstation jobs on one machine. Mitigation: GPU time is leased
  and charged like the Lab's devices.
- **Platform permissions** (macOS TCC, the Wayland portal) need the owner once per machine; setup walks through them.

## 13. Answered questions (2026-10-14)

1. Node and workstation on one machine: yes.
2. Work folder sync: both ways (default folder `~/AgentPlaza`, changeable per workstation).
3. Takeover: configurable per workstation in its setup on the hub (section 7).
4. App adapters: one full, general adapter, so the agents can work on anything (section 6).
