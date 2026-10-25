# Agent density and wake latency

Status (2026-10-16): built and measured on Linux x86_64 with KVM. The doze tier pages memory out on Linux only;
Hypervisor.framework and WHP keep their pause without a page-out (`VmControl::reclaim` returns 0 there).

A host runs thousands of agents only if an idle agent costs almost nothing and comes back fast when work arrives. This
page covers what keeps idle cost down (doze, sleep, balloon, swap), what makes a wake fast, and what keeps the hub's
and the gateway's work proportional to events instead of to the number of agents. Snapshots, sleep and forks themselves
are in [vm-snapshots.md](vm-snapshots.md).

## The tiers

| tier | VM | memory on the host | back in | who decides |
|---|---|---|---|---|
| running | vCPUs run | what the guest touched, minus what the balloon and free page reporting gave back | — | — |
| dozing | paused, process kept | paged out (`MADV_PAGEOUT`): clean snapshot pages dropped, the rest compressed in zswap or swapped | ~1 ms (`resume`), no reconnect | apid, after `doze_idle_s` (off by default; 60 s is the intended value) |
| asleep | snapshot on disk, process gone | none | ~50 ms (restore, reconnect, hello) | apid, after `idle_sleep_min` (10 min) |

The doze tier is off until a node turns it on (`doze_idle_s` 0 by default): it becomes the default once it has passed a
soak on a real node. `crates/apid/src/sleep.rs` runs both: the sweep dozes an idle agent first and puts a dozing one to sleep later. Roles in
`sleep_never_roles` do neither. A doze is three steps: the agent's gateway holds its session quiet (vsockd `POST
/v1/agents/{cid}/quiet`, so the paused guest's silence is not reaped), noded pauses the VM and pages it out (`POST
/v1/runs/{run}/vms/{vm}/doze`), and the agent shows as `dozing`. A wake of a dozing agent is a `resume` and the quiet
flag cleared: the session never ended, so there is no hello. A doze that fails is undone and not retried until the agent
is active again.

Doze needs somewhere for the guest's anonymous pages to go. A restored VM's untouched pages are file-backed and simply
dropped, but what the guest wrote needs swap: zswap (compressed, in RAM) or an encrypted swap device. A VM in its own
process has a cgroup whose `memory.swap.max` is `0` unless noded runs with `APZ_VMM_SWAP=ram` (`crates/vmm/src/cgroup.rs`).
Guest memory holds the agent's secrets, so swap to disk must be encrypted, as the snapshot directory already must be.

## A fast wake

- **The harness runs from RAM.** The guest's init binds the initramfs's own `agent-harness` into the Alpine root
  (`scripts/mk-initrd.sh`, `prepare_sysroot`). A copy on the ext4 workspace volume was page cache that a guest held to a
  small balloon reclaimed, and a restored guest's reconnect then re-read its own code at ~1 GB/s for seconds.
- **Reconnect at once.** After a restore's vsock transport reset, or any drop after the hub accepted it, the harness retries
  every few milliseconds for a short window before its normal backoff (`crates/agent-harness/src/keepalive.rs`).
- **No missed interrupt.** A restored serial console raises an interrupt the guest had not taken
  (`SerialDevice::raise_pending_interrupt`); without it every console write could block until something else read the
  UART.
- **Working set read ahead.** A snapshot records the guest's resident pages (`workingset.bin`, from `mincore`; a doze keeps
  the set it saw before paging out). A restore asks the kernel to read those ranges of the memory file ahead
  (`POSIX_FADV_WILLNEED`), so the first seconds after a wake do not fault page by page from a cold cache.
- **Wake on intent.** The console starts a wake when the owner opens an agent's chat or IDE, or types to it
  (`apps/web/src/prewake.ts`), before the message is sent.
- **The hub never polls for it.** `ensure_awake` waits on a roster notification. Wakes run on a pool: interactive ones
  (a person or a tool call waits) before background ones (queued work), at most four per node at a time.

## Forks and checkpoints

A checkpoint or fork of a restored VM is captured, not written, while the VM is paused: its state and the pages it changed
(its diff) are copied into RAM and its workspace is reflinked, then the guest resumes and the captured pages are written
(`VmControl::capture`, `write_captured`). The pause is that memcpy and the device state, not the write. A booted VM's
snapshot holds every page it touched, so it is written at once, as before. A fork's intermediate snapshot only has to
outlive the restores made from it, so it skips the fsyncs (`SnapshotKind::DiffUnsynced`), and its copies restore in
parallel. Checkpoints and sleeps stay durable.

A restored VM's next snapshot is a diff over the memory it was restored from. The VMM copies that memory into the new
snapshot itself, from the descriptor it keeps (which works in a jail and after the old snapshot is removed): noded no
longer makes a copy of its own at every restore and every snapshot. On a filesystem without reflinks each such copy was
a full read and write of the memory file.

## Idle cost on the host

- **vsockd** serves each session on a thread that sleeps until the guest, a hub command or an apid answer is ready, not on
  a 20 ms read slice. The session cap is `--max-sessions` (4096 by default) and the fd limit is raised at start. A quiet
  (dozing) session wakes at most once a minute. Accept loops and egress tunnels block until there is work; a tunnel's two
  pumps wake each other on hang-up instead of polling every second.
- **noded** backs its run watcher off to 250 ms while nothing changes; the VMM's serial pump waits for output.
- **The guest** ticks at 100 Hz, hands freed memory back through free page reporting, and its init no longer prints a
  heartbeat every 5 s once the harness runs (each console byte is a VM exit).
- **apid** keeps a status index on the roster, so timers ask for the idle or dozing agents instead of cloning everyone;
  an agent turning idle kicks the scheduler's dispatch, and the 2 s pass stays as the safety net. The memory autoscaler
  reads every node's runs in one request per pass, and placement admits on measured memory with reservations.

## Scale mode: what the host needs

`noded --check-scale` (and `GET /v1/host/scale`) reports whether a host is fit for many mostly idle agents:

| check | fit when | why |
|---|---|---|
| `zswap` | enabled | a dozing guest's pages compress in RAM |
| `swap` | present, and every device zram or dm-crypt | doze has somewhere to put written pages; guest memory holds secrets |
| `reflink` | the snapshot directory shares blocks: `FICLONE` (XFS, Btrfs, bcachefs), APFS on macOS, ReFS or a Dev Drive on Windows | without it sleeps, forks and workspaces copy whole files |
| `cgroups` | the cpu, memory and pids controllers are delegated | per-VM limits |
| `ksm` | off | merging pages across VMs is a timing side channel between agents, which must never reach each other |

KSM stays off by default for that reason. If it is ever offered, it should be an explicit owner opt-in, limited to
agents of one role.

## Measuring

`python3 tests/acceptance/run.py lifecycle` times sleep, wake (to the guest's hello), checkpoint and fork end to end, and
with `--soak N` holds N idle agents and samples RSS, CPU, threads and wakeups per agent. Every daemon serves
`GET /v1/metrics` (Prometheus text) and prints `timing op=... total_ms=...` lines for wakes, sleeps, dozes, checkpoints,
forks and spawns.

## Measured

Release builds of `origin/main` (6821d1d3) and this work (aa1a5588), the same lifecycle suite against each:
`python3 tests/acceptance/run.py lifecycle --soak 40 --soak-seconds 60`. Host: i9-12900K (24 threads), 125 GiB,
Linux 7.2.9, the host's kernel as guest kernel, snapshots on tmpfs (no reflinks), load average ~5-15 from other work.
Agents are 1 vCPU, 1024 MiB with 32 MiB usable at boot (the balloon). 2026-10-16.

| | main | this work |
|---|---|---|
| wake from sleep to the guest's hello, p50 / p99 | 252 / 261 ms | 25 / 34 ms |
| sleep (snapshot and stop), p50 | 131 ms | 67 ms |
| checkpoint of a booted agent, p50 | 49 ms | 47 ms |
| checkpoint of a restored agent: guest paused | the whole write | 17-35 ms |
| fork, two copies, to both hellos, p50 | 383 ms | 78 ms |
| host CPU for 40 idle agents (noded, its VMs, vsockd) | 23% of a core | 11% of a core |
| voluntary wakeups per idle agent (noded process, VMs included) | 76/s | 31/s |
| host RSS per idle agent, resident | 23.9 MiB | 23.6 MiB |
| host RSS per idle agent, dozing | — | 0.5 MiB |
| connected agents per vsockd | 64 (hard cap) | 4096 by default (`--max-sessions`) |

With the snapshots on a reflink filesystem (btrfs on a loop file, same host, release build of this work): wake p50 45 ms,
checkpoint p50 17 ms, fork of two copies to both hellos p50 54 ms (24 ms of it in noded). A booted agent's first full
snapshot onto that loop device took up to 8 s with its fsync (the sleep p99 and checkpoint p95 there).

Connected sessions, one vsockd with guests stood in by sockets (`run.py lifecycle --soak-sessions N`): 2,000 sessions for
60 s at 3-4% of one core (203 MiB, 6,002 threads); 5,000 at 8.7% (501 MiB, 15,002 threads, `--max-sessions 8192`); none
refused, dropped or reaped. Each CID costs three threads (two accept loops and its session), all asleep while idle.

Per thread, an idle agent costs noded 6/s and vsockd 2/s wakeups (main: 48/s and 58/s); the rest is the guest's vCPU
(~23/s). Under heavy host load (load average ~60) main's wake p99 reached 5.8 s and this work's stayed near 50 ms.

## Not built

- **Forks from a shared memory file** (memfd, mapped privately by the source and every copy, so a fork writes nothing).
  Gated on 5.1–5.3 missing the fork target (two copies connected in 60 ms). With reflinks they meet it (54 ms above); on
  tmpfs, without reflinks, a fork is 78 ms because its memory is copied in full. Scale mode requires a reflink filesystem.
- **Agents without a VM** (a hub-side loop that attaches a VM on demand). An owner decision, to be made with soak data
  from real fleets.
