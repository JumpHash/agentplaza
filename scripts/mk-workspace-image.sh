#!/bin/sh
# Builds the ext4 image the VMM hands to a guest as its writable workspace
# volume: the backing file of the guest's virtio-blk device, passed to the VMM
# as `vmctl --workspace <image>` (or `noded --workspace <image>`). Each VM of a
# run gets its own copy of it (see `vmm::spawner::materialize_workspaces`), so
# the image built here is a *base*: the run never shares it.
#
# The volume is the only writable device a guest has. The guest's kernel and
# root filesystem live in the read-only initramfs this repo also builds
# (scripts/mk-initrd.sh), and there is no NIC by design (plaza rule
# 0010), so `/workspace` is where an agent's work is written. The guest's init
# mounts the volume at /workspace and `wspace <text>` appends to
# /workspace/notes.txt there.
#
# 4 GiB, and sparse. The volume is big enough to hold the toolchains an agent
# installs on its workspace (Python, Go, Node), but `truncate` makes the file
# sparse: the host pays only for the blocks written, and the per-VM copies a run
# makes (`vmm::spawner::copy_sparse`) keep those holes. The mkfs flags
# lazy_itable_init and lazy_journal_init defer zeroing the inode tables and the
# journal, so building the image does not write its full length. Override with
# APZ_WORKSPACE_MIB.
#
# The image keeps its ext4 journal: a VM can be killed at any moment (the host
# reaps it, and a guest that crashes leaves the filesystem dirty), and a
# journal is what makes the next mount of the same image come up consistent
# instead of asking for a full check.
#
# Output: target/guest/workspace.ext4   (gitignored build output, like the
#         initrd; mkfs.ext4 is the only tool needed, from e2fsprogs)
#
# Env overrides:
#   APZ_WORKSPACE_MIB=32       the image's size in MiB (default 4096)
#   APZ_WORKSPACE_IMAGE=/path  the image's path (default target/guest/workspace.ext4)
set -eu

out_dir="target/guest"
image="${APZ_WORKSPACE_IMAGE:-$out_dir/workspace.ext4}"
size_mib="${APZ_WORKSPACE_MIB:-4096}"
label="plaza-workspace"
# A pinned filesystem UUID, label and superblock timestamp (the last through
# SOURCE_DATE_EPOCH, which mke2fs honours): the image's *identity* is stable
# across builds, so a guest and an operator see the same volume every time. The
# bytes are not reproducible: mke2fs generates the journal's UUID itself and
# this e2fsprogs cannot pin it (`-J uuid=` is not a supported journal option),
# which is why the build prints a hash instead of claiming one. The image is a
# small build artifact, not a cacheable one.
#
# The UUID only has to be stable, not unique: no two copies are ever mounted at
# once (each VM sees exactly one volume), so every copy may carry it.
uuid="0f1a2b3c-4d5e-4f60-8a9b-0c1d2e3f4a5b"

if ! command -v mkfs.ext4 >/dev/null 2>&1; then
    echo "mk-workspace: mkfs.ext4 not found: the workspace image needs e2fsprogs" >&2
    exit 1
fi

case "$size_mib" in
    '' | *[!0-9]*)
        echo "mk-workspace: APZ_WORKSPACE_MIB must be a whole number of MiB, not '$size_mib'" >&2
        exit 1
        ;;
esac
if [ "$size_mib" -lt 4 ]; then
    echo "mk-workspace: APZ_WORKSPACE_MIB must be at least 4 (ext4's own metadata)" >&2
    exit 1
fi

mkdir -p "$(dirname "$image")"
rm -f "$image"

# A sparse file: mkfs writes the metadata and the rest stays holes until the
# guest writes to it, so the host only pays for the blocks in use.
truncate -s "${size_mib}M" "$image"

# mke2fs stamps the superblock and the inodes with the current time unless
# SOURCE_DATE_EPOCH asks for a fixed one (e2fsprogs honours it, so the times are
# pinned; the journal's UUID below is not).
: "${SOURCE_DATE_EPOCH:=0}"
export SOURCE_DATE_EPOCH

mkfs.ext4 -q -F -E lazy_itable_init=1,lazy_journal_init=1 -L "$label" -U "$uuid" -m 0 "$image"

# Read-only check of what was just written: a failed build must not look like a
# built image.
if command -v e2fsck >/dev/null 2>&1; then
    e2fsck -fn "$image" >/dev/null
    echo "e2fsck: clean"
fi

echo "workspace image: $image"
echo "  size:  ${size_mib} MiB ($(du -h "$image" | cut -f1) of it written to disk)"
echo "  label: $label  uuid: $uuid"
if command -v sha256sum >/dev/null 2>&1; then
    echo "  sha256: $(sha256sum "$image" | cut -d' ' -f1)"
fi
echo "use it with: vmctl ... --workspace $image"
