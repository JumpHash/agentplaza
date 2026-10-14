#!/bin/sh
# Builds the disk image of the desktop guest a workstation VM boots (docs/design/workstations.md, section 10): a
# light X session on a virtual display (Xvfb with Openbox), the AT-SPI accessibility bus for the UI tree, and the
# apz-workstation daemon. The node passes the join code, the hub and the name on the kernel command line
# (`plaza.workstation_join=`, `plaza.workstation_hub=`, `plaza.workstation_name=`); the start script reads them.
#
#   scripts/mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]
#
# Nothing is downloaded here. Bring the two inputs yourself:
#   - the Alpine minirootfs for the target architecture (the tarball Alpine publishes), and
#   - apz-workstation built for that architecture against musl, Alpine's C library, e.g.
#       cargo build --release -p apz-workstation --target x86_64-unknown-linux-musl
#   - on the host: apk-tools (a static apk is fine), tar, truncate and e2fsprogs (mkfs.ext4 -d). Run it as root, or
#     under fakeroot, so the image's files keep their owners.
#
# Output: <out-dir>/disk.ext4, sparse, APZ_WORKSTATION_MIB MiB (default 8192). The default out-dir is
# target/guest/workstation. noded copies this image for each workstation VM, so the image carries no machine
# identity: the first boot of each disk makes its own /var/lib/apz/machine-id, and the hub refuses a station that
# reports the hub's own id.
#
# Open, and not built by this recipe:
#   - the initramfs that mounts /dev/vda as the root and switch_roots into it. scripts/mk-initrd.sh builds the
#     agents' PID-1 harness initramfs, which has no disk root. noded refuses to boot a workstation until
#     --workstation-initrd names one.
#   - the daemon's way to the hub. A workstation VM has no NIC (rule 0010), so its daemon needs a vsock path to the
#     node's relay. That path is not built.
#   - GPU passthrough. noded validates a vfio GPU and refuses the boot until apz-vmm can attach a PCI device.
set -eu

tarball=${1:?usage: mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]}
binary=${2:?usage: mk-workstation-guest.sh <alpine-minirootfs.tar.gz> <apz-workstation> [out-dir]}
out=${3:-target/guest/workstation}
size_mib=${APZ_WORKSTATION_MIB:-8192}
packages="openrc dbus xvfb openbox at-spi2-core mesa-dri-gallium font-dejavu"

[ -f "$tarball" ] || { echo "mk-workstation-guest: no rootfs at $tarball" >&2; exit 1; }
[ -x "$binary" ] || { echo "mk-workstation-guest: $binary is not an executable file" >&2; exit 1; }
for tool in apk tar truncate mkfs.ext4; do
  command -v "$tool" >/dev/null 2>&1 || { echo "mk-workstation-guest: needs $tool on the host" >&2; exit 1; }
done

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

cat > "$root/etc/local.d/apz-workstation.start" <<'EOF'
#!/bin/sh
# Runs at boot (OpenRC's local service): one workstation, its virtual display and its daemon. The code is used
# once, on the first boot; later boots of the same disk (after a revert too) start the daemon, which is enrolled.
set -u

code=""
hub=""
name="workstation"
for word in $(cat /proc/cmdline); do
    case $word in
        plaza.workstation_join=*) code=${word#plaza.workstation_join=} ;;
        plaza.workstation_hub=*) hub=${word#plaza.workstation_hub=} ;;
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

if [ -n "$code" ] && [ -n "$hub" ] && [ ! -e /var/lib/apz/joined ]; then
    if su agents -c "apz-workstation join --hub '$hub' --code '$code' --name '$name' --account agent:agents --state-dir /var/lib/apz/state"; then
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
