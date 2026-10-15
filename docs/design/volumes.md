# Shared read-only data volumes

Status (2026-10-15): built on Linux x86_64 with KVM, tested end to end on that machine. The hub routes are in (apid and
noded), and the web (Volumes), desktop (Volumes) and CLI (`agentplaza volumes`) surfaces cover them (see "Fifth
pass" in `parity.md`). Volumes are supported on KVM, WHP and Hypervisor.framework. WHP is compile-checked and has not run on
Windows; Hypervisor.framework (HVF) is compile-checked for `aarch64-apple-darwin` only and has not run on a Mac.

A data volume is a read-only disk that several VMs can mount at once. Agents use it for shared reference data, such as
a corpus or a model cache. Nothing in a VM can write to it.

## The image

A volume is an ext4 image on a node. The file lives in noded's `--volume-dir` (default `volumes/` beside the
snapshots). The caller makes it from an absolute path on the node:

| source | what noded does |
|---|---|
| an ext4 image file | copies it, reflink-cloned when the filesystem can |
| a directory | packs it with `mkfs.ext4 -d`, sized at the files' bytes × 5/4 + 64 MiB |

A name is 1 to 20 characters from `a-z`, `0-9` and `-`. It cannot start with `-`.

## Routes

noded and apid serve the same routes. apid proxies to the noded on the hub machine.

| route | what it does |
|---|---|
| `GET /v1/volumes` | lists the volumes. Each one has `in_use`: the guest CIDs of the live VMs that mount it. |
| `POST /v1/volumes` | creates a volume from `{name, source}`. |
| `DELETE /v1/volumes/{name}` | removes a volume. Returns 409 while a VM mounts it. |

## Giving volumes to agents

Volumes reach agents by role.

- The apid setting `role_volumes` maps each role to a list of volume names. A role takes at most 4. An agent spawned
  after the change carries the volumes of its role.
- noded's `POST /v1/runs` takes `volumes: [names]` for a run's VMs.

## In the VM

- Each volume is a read-only virtio-blk device (`VIRTIO_BLK_F_RO`). A write gets an I/O error. The write never reaches
  the file.
- The device id is the volume name. The guest init mounts each disk whose id is a volume name at `/data/<name>` with
  `MS_RDONLY|MS_NOSUID|MS_NODEV`. The workspace is the disk with an empty id.
- One image can back any number of VMs at once, because nothing writes it.
- A VM takes up to 4 volumes. Each has its own MMIO slot and interrupt. On x86_64 the interrupts are GSI 16 to 19, after
  the balloon's slot. On aarch64 each volume is a device tree node, on KVM and on Hypervisor.framework (GSI `8 + i`).

## Snapshots and moves

- A snapshot saves the transport state of each volume, and its device id (the volume's name). A restore or a wake must
  have the same number of volumes, with the same names in the same order. If not, it is refused. Snapshots taken before
  the id was saved are checked by count only.
- Sleep, wake, checkpoints and forks keep the run's volumes.
- A VM with volumes boots cold. Warm templates carry no volumes.
- A VM moved to another node needs the same volume name on that node.

## Not yet

| item | why |
|---|---|
| Hypervisor.framework | Volumes attach in the aarch64 layout's slots, as on KVM aarch64. The code is compile-checked for `aarch64-apple-darwin` only and has not run on a Mac. WHP attaches them in the KVM x86_64 slots; it has not run on Windows. |
| E2B `volumeMounts` | Not mapped to volumes. |

## Verified

On KVM, end to end:

- Two VMs mount one volume at the same time and read it.
- A write is refused with EROFS. The guest's `data` console command shows it.
- The mount survives a sleep and a wake, and a cold wake.
- A fork from a named checkpoint mounts the volume.
