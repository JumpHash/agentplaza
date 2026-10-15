# VM snapshots, warm templates and sleep

Status (2026-10-15): built on Linux x86_64 with KVM, tested end to end on that machine. The VMM code for aarch64 KVM is
ported and compiles, and it has not been run on hardware. Hypervisor.framework and WHP have no snapshot support (see
"Platform support").

An agent's microVM can be paused, written to disk, restored, copied, forked and moved to another node. The same
machinery gives the plaza warm starts: a new agent restores from a template instead of booting. The hub puts an agent
to sleep when it has nothing to do and wakes it before any work reaches it. The techniques follow CubeSandbox
(Apache-2.0, Copyright Tencent), where the code says so. Where AgentPlaza does something different, the section says
how.

## Layers

| layer | crate and module | what it does |
|---|---|---|
| VMM | `crates/vmm/src/vstate/snapshot.rs` | The snapshot format: state file, memory file, format and version checks. |
| VMM | `crates/vmm/src/vstate/snapshot_memory.rs` | Writes guest memory: touched pages only, or the changed pages of a restored VM. |
| VMM | `crates/vmm/src/control.rs` | `VmControl`: pause, resume and snapshot of a running VM, behind the device gate. |
| VMM | `crates/vmm/src/clonefile.rs` | Reflink copies of disk and memory images, with a sparse copy as the fallback. |
| VMM | `crates/vmm/src/cgroup.rs` | cgroup v2 limits for a VM that runs in its own process. |
| VMM | `crates/vmm/src/spawner.rs` | `VmSpec::restore`, `SnapshotKind`, `SnapshotInfo`, and the two boot modes. |
| node | `crates/noded/src/snapshots.rs` | Where snapshots live on the node, and their directory layout. |
| node | `crates/noded/src/templates.rs` | Warm templates: build on first use, restore for new VMs. |
| node | `crates/noded/src/migrate.rs` | Export and import of a sleeping VM, in chunks. |
| hub | `crates/apid/src/sleep.rs` | The idle rule, sleep, and wake before work. |
| hub | `crates/apid/src/vm_snapshots.rs` | Checkpoints, rollback with a safety checkpoint, forks, automatic checkpoints. |
| hub | `crates/apid/src/placement.rs` | Choosing a node for a new agent, moving an agent, draining a node. |
| guest | `crates/agent-harness/src/restore.rs` | Sets the guest's clock and mixes fresh entropy after a restore. |

## Snapshot format

A snapshot is a directory. The VMM writes two files into it:

| file | what it holds |
|---|---|
| `vmstate.json` | `VmSnapshot` as JSON: the format version, the VMM version and architecture, the memory layout, the VM's and every vCPU's KVM state, and the device state. |
| `memory.img` | Guest memory, each region at its offset. Pages the guest never touched are holes, so the file is sparse. |

The workspace volume is not in the VMM's files. The caller copies it (a reflink) beside the snapshot while the VM is
paused, and hands the copy back on restore. On the node, a checkpoint also holds `workspace.img`. A sleep snapshot does
not need one: the stopped VM's own workspace image stays unwritten until it wakes.

**Pause.** The vCPUs leave `KVM_RUN`, so no guest code runs. The snapshot holds the write half of the VM's device gate
for its whole length. The vsock device's event thread takes the read half around each pass, so no packet lands between
the device state and the memory dump.

**Full and diff.** `SnapshotKind::Full` writes every page the guest has touched. A VM that was booted has anonymous
memory, so the pages are found in `/proc/self/pagemap` and written as they are. `SnapshotKind::Diff` writes only the
anonymous pages, which are the pages changed since the VM was restored. The caller has already put a reflinked copy of
the memory file it was restored from where the diff is written, so the result is a complete memory file at the cost of
the changed pages.

**Restore is `MAP_PRIVATE`.** `VmSpec::restore` maps the memory file privately. The restored guest reads the file's
pages lazily, and its writes stay in its own anonymous memory. The memory file is never changed by the guest. This is
what makes a diff cheap, and it is why a restored VM never writes a `Full` snapshot: pages it never read exist only in
the file, so a full write would leave holes where they belong. Its snapshots are always a diff over a reflinked copy
of the file it was restored from. The control keeps that file open, so the copy still works after the file's directory
was removed.

**Versioning refusals.** `read_state` refuses a snapshot of another format, another VMM version or another
architecture (`SnapshotError::Incompatible`). A restore of state whose layout may have changed would corrupt the guest
silently, so the refusal comes first. The warm template key carries the same version (see below). A snapshot that does
not fit the VM it restores into (devices, vCPUs, memory) is refused as a mismatch.

**Clock and entropy.** A guest restored from a snapshot resumes with the clock and the kernel's random pool it had when
it was frozen. Two restored guests would share a random stream, and each would believe it is still the moment of the
freeze. The hub's `welcome` carries `HostFacts`: the host's wall clock in milliseconds and 32 bytes from the host's
CSPRNG. `restore::apply` sets the clock when it has drifted and mixes the bytes into the kernel pool, before the
session does anything else. A failure is printed, and the session goes on. The sandbox daemon does the same on each
`/init` (see [`e2b.md`](e2b.md)).

## Warm templates

A template is a VM of one exact shape: the guest kernel and initrd, the base workspace image, vCPUs, memory, balloon
and the whole kernel command line. The command line includes the role and `plaza.vm`, so a template is per role.

**Building.** The first VM of a shape boots with `plaza.template=1`. When its harness prints
`agent-harness: template ready`, the VM is paused and snapshotted in full, together with its workspace as it was at
that moment. The build runs once per key: two calls for the same key wait for the one build. A build that gets no
marker within `BUILD_TIMEOUT` is reported to its caller, which boots cold. The key is not retried for `RETRY_AFTER`.

**The key.** A SHA-256 over the shape and over the images' contents (each image's SHA-256, computed once per file and
cached by path, size and mtime), plus the VMM's version. A changed kernel, initrd or base image makes a new key, so a
rebuilt guest image never restores an old template. A template is never restored by a VMM that cannot read it.

**Starting an agent.** A new agent of the same shape is a reflinked copy of the template's workspace plus a restore of
its memory. The harness is still in its 25 ms connect loop when the restore resumes it, so it reaches the hub moments
later. The welcome sets its clock and entropy (see above). Its profile then arrives, and the agent starts its work.

**Placement uses templates.** `placement.rs` adds a 1024 MiB warm bonus to a node that holds a template for the role,
and takes 512 MiB off for each VM the node already runs. The highest score wins, and a tie goes to `n0`.

**Layout** under the snapshot root: `templates/<key>/{snap/, workspace.img, template.json}`.

| route | what it does |
|---|---|
| `GET /v1/templates` (noded) | Lists the built templates. |
| `DELETE /v1/templates/{key}` (noded) | Removes one. A VM already restored from it keeps running. |

## Sleep and wake

An agent sleeps when it has nothing to do. The hub's sweeper (`sleep.rs`) checks each agent. An agent sleeps only when
all of these are empty: its work queue, its running engines and terminals, its open IDE sessions, the approvals it
waits on, and its workstation leases. It must also have been idle for `idle_sleep_min` minutes.

| setting | default | meaning |
|---|---|---|
| `idle_sleep_min` | 10 | Minutes an idle agent waits before its VM sleeps. `0` means agents never sleep on their own. The most is 1440. |
| `sleep_never_roles` | `["orchestrator"]` | Roles whose agents never sleep on their own, so the owner can talk to the orchestrator at any time. |

Sleeping is a snapshot of the VM followed by a stop (`POST /v1/runs/{run}/vms/{vm}/sleep`). The agent shows as
`asleep`, and its memory is freed.

**Wake comes before work.** Every path that delivers work calls `wake` or `ensure_awake` first: the scheduler's
dispatch, a chat message, a `tell`, an IDE request and the owner's wake. The node restores the VM from its sleep
snapshot. The harness reconnects within moments, its hello marks it idle, and the queued work then goes out
(`Scheduler::dispatch_idle`). A request that cannot wait gets a 503 while the agent wakes.

| route | what it does |
|---|---|
| `POST /v1/agents/{cid}/sleep` (apid) | The owner puts the agent to sleep now. |
| `POST /v1/agents/{cid}/wake` (apid) | The owner wakes the agent. |
| `POST /v1/runs/{id}/vms/{index}/sleep` (noded) | Snapshots the VM and stops it. |
| `POST /v1/runs/{id}/vms/{index}/wake` (noded) | Restores the VM from its sleep snapshot. |

## Checkpoints, rollback, forks

A checkpoint is a snapshot of the whole VM: memory, disk and the running processes. The IDE's file checkpoints
(`/v1/ide/workspaces/{id}/checkpoints`) are a different thing, and they cover files only.

**Rollback takes a safety checkpoint.** Restoring a checkpoint first takes one labelled `before-restore`, and the
response returns it as `safety`. A rollback can itself be undone. A failed restore therefore loses nothing.

**Forks.** A fork is a new agent of the same role. Each fork gets a copy of the source agent's VM at the fork's
checkpoint, and a name of its own. `POST /v1/agents/{cid}/fork {n}` returns the forks and
the checkpoint they start from.

**Automatic checkpoints.** The owner's `vm_auto_checkpoints` setting (on by default) makes the hub take a checkpoint
before a tool call whose risk class is high (`is_high_risk`). "Undo the last risky step" then restores the whole
machine, not only its files.

| route | what it does |
|---|---|
| `GET /v1/agents/{cid}/vm-checkpoints` (apid) | Lists checkpoints, newest first. |
| `POST /v1/agents/{cid}/vm-checkpoints` (apid) | Takes one: `{label?}`. |
| `POST /v1/agents/{cid}/vm-checkpoints/{name}/restore` (apid) | Restores one, after a safety checkpoint. Returns `{ok, safety}`. |
| `DELETE /v1/agents/{cid}/vm-checkpoints/{name}` (apid) | Deletes one. |
| `POST /v1/agents/{cid}/fork` (apid) | `{n}`: forks the agent n times. |
| `GET`, `POST /v1/runs/{id}/vms/{index}/checkpoints` (noded) | The node's side of the same operations. |
| `POST /v1/runs/{id}/vms/{index}/checkpoints/{name}/restore` (noded) | Restores a checkpoint on the node. |
| `DELETE /v1/runs/{id}/vms/{index}/checkpoints/{name}` (noded) | Deletes a checkpoint on the node. |
| `POST /v1/runs/{id}/vms/{index}/fork` (noded) | Copies the VM for a fork. |

## Reflink copies

`clone_file` (`crates/vmm/src/clonefile.rs`) is `ioctl(FICLONE)` on a filesystem with reflinks (XFS, Btrfs,
bcachefs). The copy costs time proportional to its extents, takes no space until either side is written, and is what
makes workspace copies, snapshots and forks cheap. On any other filesystem it falls back to `copy_sparse`, which keeps
holes as holes. `reflink_supported` probes a directory once, so a daemon can say at start which path its copies take.

## Per-VM processes and cgroup limits

An in-process VM is a set of threads of the daemon that boots it. Nothing can bound one VM's memory or CPU without
bounding the daemon. With `noded --vm-processes` (or `APZ_VMM_SPAWN=process`, which the service plans set), each VM
runs in its own `apz-vmm` process, jailed, and the process is placed in a cgroup of its own.

The VM's cgroups sit under `agentplaza.slice`, in the user's systemd manager. The kernel enforces the VM's shape:

| file | limit |
|---|---|
| `memory.max` | The guest's RAM plus the VMM's overhead. |
| `cpu.max` | One period's quota per vCPU. |
| `pids.max` | The VM's threads. |

`io` is not delegated to the user, so no block-I/O limit is set. A host without a delegated hierarchy runs its VMs
unlimited, with the reason logged. Once a root exists, a limit that cannot be written fails the VM's start: the VM does
not run without a limit it should have.

## Placement, moves and drains

**Placement.** A new agent goes to the node with the highest score (see "Warm templates"). Only a node with room for one
agent is eligible. With none, the agent goes to `n0`, where noded says whether it fits.

**Moving an agent** (`POST /v1/agents/{cid}/move {node}`) carries its VM between two nodes:

1. The VM is put to sleep.
2. Its files are exported on the source (`POST /v1/runs/{run}/vms/{vm}/export`).
3. The files are copied in 4 MiB reads and written to the target in 512 KiB pieces. A piece that is all zeros is not
   sent: the hub reports the count, and the target leaves a hole. A sparse image crosses the wire at the size of its
   data.
4. The target finishes the import into a sleeping run. The placement is recorded, the target's gateway serves the CID,
   and the agent wakes on the target.
5. The source lets go of the agent: its gateway and its run.

A failure before step 4 aborts the import and wakes the agent where it was. The hub never holds a whole image. Exports
and imports live in memory and expire after their TTL, and an export holds its files open, so the bytes it hashed are
the bytes it sends.

**Draining.** `POST /v1/nodes/{id}/drain {on}` marks a node as draining. Every agent it runs is moved off, one at a
time. The owner's console and `agentplaza nodes` expose it.

| route | what it does |
|---|---|
| `POST /v1/runs/{run}/vms/{vm}/export` (noded) | Stops nothing: reads a sleeping VM's files into an export. |
| `GET /v1/exports/{export}/{name}` (noded) | One file of an export, in pieces. |
| `DELETE /v1/exports/{export}` (noded) | Drops an export. |
| `POST /v1/imports` (noded) | Opens an import on the target. |
| `POST /v1/imports/{import}/{name}` (noded) | Writes one file of an import, in pieces. |
| `POST /v1/imports/{import}/finish` (noded) | Finishes the import into a sleeping run. |
| `DELETE /v1/imports/{import}` (noded) | Drops an import and its staging directory. |
| `POST /v1/agents/{cid}/move` (apid) | Moves the agent to `{node}`. Returns `{ok, from, to, bytes, ms}`. |
| `POST /v1/nodes/{id}/drain` (apid) | `{on}`: drains the node or lifts the drain. |

## Platform support

| platform | backend | state |
|---|---|---|
| Linux x86_64 | KVM | Snapshot, restore, diff, sleep, checkpoints, forks, moves. Tested end to end on this machine. |
| Linux aarch64 | KVM | The aarch64 state ported and compiled (`cargo check --target aarch64-unknown-linux-gnu`). Not run on hardware here. |
| macOS (Apple Silicon) | Hypervisor.framework (`crates/vmm/src/hvf`) | No snapshots. Pause and snapshot calls answer that the backend cannot (`SpawnError::NoSnapshots`). |
| Windows | WHP (`crates/vmm/src/whp`) | No snapshots, the same answer. The snapshot format builds on every platform, so a Windows or macOS daemon can read snapshot metadata. |

## Measured

Measured on this machine (x86_64, KVM, 1 vCPU, 256 to 512 MiB guests). p50 unless noted.

| operation | result |
|---|---|
| Boot to first heartbeat | ~317 ms |
| Full snapshot: pause | ~18 ms, 56 MiB written for a fresh 256 MiB guest |
| Restore to a console answer | ~13 ms (~15 ms p95 at concurrency 5) |
| Diff snapshot of a restored VM | ~23 ms pause, ~0.4 to 1 MiB written |
| Host RSS per VM | ~56 MiB booted; ~5 MiB restored |
| New agent from a template, connected | ~170 ms after create (the first, which builds the template: ~840 ms) |
| Sleep | ~107 ms |
| Wake to reconnect | ~330 ms |
| Checkpoint | ~130 ms |
| Fork, two copies | ~210 ms |
| Move of a 512 MiB agent between nodes | 80 MB sent of 554 MB (zeros skipped), 3.4 s |

## Credits and differences

The techniques come from CubeSandbox (Apache-2.0, Copyright Tencent): the template restore that starts a sandbox from a
memory snapshot, the `MAP_PRIVATE` restore with page selection from `/proc/self/pagemap` for diffs (the code is in
`crates/vmm/src/vstate/snapshot_memory.rs`), reflink copies, and pausing an idle sandbox. See `crates/vmm/NOTICE`.

AgentPlaza differs in four ways:

- **Sleep follows the work, not the traffic.** CubeSandbox pauses a sandbox when its proxy sees no requests. The hub
  knows the agent's queue, engines, terminals, IDE sessions, approvals and leases, so it sleeps an agent only when none
  of them is busy.
- **A safety checkpoint before every rollback.** A rollback can be undone.
- **Checkpoints follow the laws.** A high-risk tool call takes a checkpoint first, when the owner's setting is on.
- **No network device in the guest.** The guest reaches the outside only through the hub's egress proxy and the
  plaza's laws (see [`egress-credentials.md`](egress-credentials.md)).

See also [`architecture.md`](../architecture.md) for the processes and the routes.
