#!/bin/sh
# Builds the desktop guest a workstation VM boots (docs/design/workstations.md, section 10): the disk image (a light X
# session on a virtual display, Xvfb with Openbox, the AT-SPI accessibility bus for the UI tree, and the apz-workstation
# daemon) and the initramfs that mounts that disk as the root and switches into it.
#
#   scripts/mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]
#
# Nothing is downloaded here. Bring the inputs yourself:
#   - the Alpine minirootfs for the target architecture (the tarball Alpine publishes), and
#   - apz-workstation built for that architecture against musl, Alpine's C library, e.g.
#       cargo build --release -p apz-workstation --target x86_64-unknown-linux-musl
#   - on the host: apk-tools (a static apk is fine), tar, truncate, e2fsprogs (mkfs.ext4 -d), cpio, gzip, and a
#     busybox for the initramfs (Debian/Ubuntu: busybox-static; or APZ_BUSYBOX=/path). Run it as root, or under
#     fakeroot, so the image's files keep their owners.
#
# Output, in <out-dir> (default target/guest/workstation):
#   disk.ext4   sparse, APZ_WORKSTATION_MIB MiB (default 8192). noded copies it for each workstation VM, so the image
#               carries no machine identity: the first boot of each disk makes its own /var/lib/apz/machine-id, and the
#               hub refuses a station that reports the hub's own id.
#   initrd.gz   the initramfs: busybox, an /init that waits for the desktop disk (/dev/vda, ext4), mounts it and
#               switch_roots into it. Point noded at both:
#                 noded --workstation-kernel <vmlinux> --workstation-initrd <out-dir>/initrd.gz \
#                       --workstation-image <out-dir>/disk.ext4
#               The kernel is the guest kernel (scripts/mk-guest-kernel.sh) with virtio-mmio, virtio-blk, ext4 and
#               vsock built in, as the agents' guests have them.
#
# How the VM reaches its hub (rule 0010: no NIC): the daemon's hub is vsock://2:5100, the node's relay (noded's
# ws_relay), which forwards the daemon's pinned TLS to the hub byte for byte. The join config (the hub's address, its
# pin and the code) is read once from the node's config port, vsock://2:5101, by `apz-workstation join --from-node`,
# so the join code is never on the kernel command line. The node names that port with `plaza.workstation_node=`.
#
# Not built here:
#   - GPU passthrough. noded validates a vfio GPU and refuses the boot until apz-vmm can attach a PCI device.
#   - The image's own init. The initramfs execs /sbin/init of the disk, which is Alpine's busybox init with OpenRC
#     (its /etc/inittab runs the default runlevel, which starts the `local` service below). Not yet booted end to end.
set -eu

tarball=${1:?usage: mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]}
binary=${2:?usage: mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]}
out=${3:-target/guest/workstation}
size_mib=${APZ_WORKSTATION_MIB:-8192}
packages="openrc dbus xvfb openbox at-spi2-core mesa-dri-gallium font-dejavu"

[ -f "$tarball" ] || { echo "mk-workstation-guest: no rootfs at $tarball" >&2; exit 1; }
[ -x "$binary" ] || { echo "mk-workstation-guest: $binary is not an executable file" >&2; exit 1; }
for tool in apk tar truncate mkfs.ext4 cpio gzip ldd; do
  command -v "$tool" >/dev/null 2>&1 || { echo "mk-workstation-guest: needs $tool on the host" >&2; exit 1; }
done

# The first busybox for the initramfs: APZ_BUSYBOX, then the usual places (mk-initrd.sh looks in the same ones).
busybox_path=""
for candidate in ${APZ_BUSYBOX:-} /bin/busybox /usr/bin/busybox /usr/lib/initcpio/busybox; do
  if [ -x "$candidate" ]; then busybox_path=$candidate; break; fi
done
[ -n "$busybox_path" ] || { echo "mk-workstation-guest: no busybox for the initramfs (install busybox-static, or set APZ_BUSYBOX)" >&2; exit 1; }

root="$out/rootfs"
image="$out/disk.ext4"
rm -rf "$root"
mkdir -p "$root"
tar -xf "$tarball" -C "$root"
apk --root "$root" --initdb --no-cache add $packages

install -m 0755 "$binary" "$root/usr/local/bin/apz-workstation"
install -d -m 0755 "$root/etc/local.d" "$root/etc/runlevels/default" "$root/var/lib/apz"
# No /etc/machine-id in the image: the start script makes one per disk.
rm -f "$root/etc/machine-id"
# A workstation VM has a serial console and no virtual terminals: Alpine's gettys on tty1..tty6 would fail and be
# respawned forever, flooding the console. They go; the console stays the kernel's and init's.
sed -i '/^tty[0-9]*::respawn:/d' "$root/etc/inittab"

cat > "$root/etc/local.d/apz-workstation.start" <<'EOF'
#!/bin/sh
# Runs at boot (OpenRC's local service): one workstation, its virtual display and its daemon. The node names the
# port its join config is served on (plaza.workstation_node=, a vsock address); the config is read once, on the first
# boot, and holds the join code, so the code never comes from the command line. Later boots of the same disk (after a
# revert too) start the daemon, which is enrolled.
set -u

name="workstation"
node=""
for word in $(cat /proc/cmdline); do
    case $word in
        plaza.workstation_node=*) node=${word#plaza.workstation_node=} ;;
        plaza.workstation_name=*) name=${word#plaza.workstation_name=} ;;
    esac
done

hostname "$name"
[ -s /var/lib/apz/machine-id ] || tr -d - < /proc/sys/kernel/random/uuid > /var/lib/apz/machine-id
cp /var/lib/apz/machine-id /etc/machine-id
id agents >/dev/null 2>&1 || adduser -D agents
mkdir -p /var/lib/apz/state /run/apz /home/agents
chown -R agents /var/lib/apz /home/agents

export DISPLAY=:99
Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp > /var/log/xvfb.log 2>&1 &
i=0
while [ ! -S /tmp/.X11-unix/X99 ] && [ "$i" -lt 30 ]; do sleep 1; i=$((i + 1)); done

if [ -n "$node" ] && [ ! -e /var/lib/apz/joined ]; then
    if su agents -c "apz-workstation join --from-node '$node' --name '$name' --account agent:agents --state-dir /var/lib/apz/state" \
        > /var/log/apz-join.log 2>&1; then
        touch /var/lib/apz/joined
    else
        echo "apz-workstation: the join did not complete" >&2
    fi
fi

su agents -c "DISPLAY=:99 HOME=/home/agents dbus-run-session -- sh -c 'openbox & /usr/libexec/at-spi-bus-launcher --launch-immediately & exec apz-workstation run --state-dir /var/lib/apz/state'" \
    > /var/log/apz-workstation.log 2>&1 &
EOF
chmod 0755 "$root/etc/local.d/apz-workstation.start"
ln -sf /etc/init.d/local "$root/etc/runlevels/default/local"

mkdir -p "$out"
rm -f "$image"
truncate -s "${size_mib}M" "$image"
mkfs.ext4 -q -F -L plaza-workstation -E lazy_itable_init=1,lazy_journal_init=1 -d "$root" "$image"
rm -rf "$root"
echo "mk-workstation-guest: $image ($size_mib MiB, sparse)"

# ---------------------------------------------------------------------------
# The initramfs. The same approach as scripts/mk-initrd.sh: the host's busybox, dynamically linked, so its interpreter
# and libraries (ldd) go in at their absolute paths, and applets are symlinks to busybox. Only what /init needs.
# ---------------------------------------------------------------------------
initrd_root="$out/initrd"
rm -rf "$initrd_root"
mkdir -p "$initrd_root/bin" "$initrd_root/sbin" "$initrd_root/proc" "$initrd_root/sys" "$initrd_root/dev" \
  "$initrd_root/newroot"
cp "$busybox_path" "$initrd_root/bin/busybox"
chmod 0755 "$initrd_root/bin/busybox"
for lib in $(ldd "$busybox_path" 2>/dev/null | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^\//) print $i }' | sort -u); do
  mkdir -p "$initrd_root$(dirname "$lib")"
  cp -L "$lib" "$initrd_root$lib"
done
applets="$("$busybox_path" --list 2>/dev/null || true)"
for applet in sh mount umount mkdir sleep switch_root echo cat ls test true false; do
  if printf '%s\n' "$applets" | grep -qx "$applet"; then
    ln -sf busybox "$initrd_root/bin/$applet"
  else
    echo "mk-workstation-guest: busybox has no $applet applet; the initramfs needs it" >&2
    exit 1
  fi
done

cat > "$initrd_root/init" <<'EOF'
#!/bin/sh
# PID 1 of a workstation VM. Mounts the pseudo-filesystems, waits for the desktop disk (/dev/vda, the virtio-blk
# device the node attaches), mounts it as the root and switches into it, where the image's own init starts the
# workstation. A disk that does not mount leaves a shell on the console, so the failure is visible, not a hang.
export PATH=/bin:/sbin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || mount -t tmpfs tmpfs /dev
i=0
while [ ! -b /dev/vda ] && [ "$i" -lt 30 ]; do
    sleep 1
    i=$((i + 1))
done
if ! mount -t ext4 /dev/vda /newroot; then
    echo "plaza workstation: the desktop disk /dev/vda does not mount; a shell is left here" > /dev/console
    exec /bin/sh
fi
echo "plaza workstation: the desktop disk is mounted; switching root" > /dev/console
# The mounts made here go with the root: the disk's own /dev is empty (no udev or mdev runs in the image), so without
# the device filesystem the desktop's services cannot open /dev/null.
for fs in dev proc sys; do
    mkdir -p "/newroot/$fs"
    mount --move "/$fs" "/newroot/$fs"
done
exec switch_root /newroot /sbin/init
EOF
chmod 0755 "$initrd_root/init"

(cd "$initrd_root" && find . | cpio -o -H newc --quiet) | gzip -9 > "$out/initrd.gz"
rm -rf "$initrd_root"
echo "mk-workstation-guest: $out/initrd.gz (initramfs: busybox, /init, switch_root to /dev/vda)"
