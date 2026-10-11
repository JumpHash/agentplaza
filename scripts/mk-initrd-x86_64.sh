#!/bin/sh
# Builds a minimal x86_64 guest initramfs for the AgentPlaza VMM demo: a static
# PID 1 that mounts the pseudo-filesystems, prints a heartbeat to the serial
# console every 5 s and accepts simple commands over the serial console.
#
# Cmdline flags:
#   plaza.beats=N    power off cleanly after N heartbeats (churn mode for the
#                    respawn supervisor). "Power off" is a guest reboot
#                    (RB_AUTOBOOT -> KVM reset -> GuestExited{reset=true}), the
#                    reset path, kept separate from ACPI S5 below.
#   plaza.acpi=0     disable the ACPI S5 poweroff path: the `poweroff` command
#                    then reboots (RB_AUTOBOOT) instead of writing the sleep
#                    control register. ACPI S5 is the default when the flag is
#                    absent, and a comparison run sets it to 0.
#   plaza.halt=1     park forever in pause() after the banner (a deliberately
#                    unresponsive guest: exercises the host kill path).
#   plaza.harness=1  load the guest's vsock module set and run the AgentPlaza
#                    agent harness (this initramfs' /agent-harness), whose
#                    whole session prints to this same serial console. A
#                    session that ends with the guest's `bye` reboots the guest
#                    (the same clean-exit path plaza.beats uses); a failed one
#                    is reported and the guest stays up for its transcript.
#   plaza.keepalive=1  run the agent fleet's long-lived session: it implies
#                    plaza.harness=1, and the reap path then NEVER reboots the
#                    guest. The harness owns its reconnect loop, so an exit is
#                    either the hub's `stop` or the harness giving up for good;
#                    either way the VM stays up (inspectable and killable) and
#                    the guest keeps serving its console. The guest's role
#                    (plaza.role=<role>) and the keepalive flag are read by the
#                    harness itself; the flag is read here too, because only
#                    the guest knows whether an exit means "reboot".
#   plaza.port=N     the orchestrator's vsock port (default 5000), read by the
#                    harness itself.
#   plaza.vm=N       this VM's index within the spawn (default 0), read by the
#                    harness itself.
#
# Serial commands: help, echo <text>, uname, uptime, mem, cpu, interrupts,
# wspace [text], sync, poweroff, reboot, halt. `interrupts` prints the kernel's
# IRQ table (which device is on which IRQ), `wspace` reads and writes the
# workspace volume, and `sync` flushes the guest's filesystems.
#
# The `poweroff` serial command uses ACPI S5 (RB_POWER_OFF): the guest kernel
# evaluates the DSDT `_S5` object and writes SLP_EN to the PM1-control port,
# and the VMM stops the VM cleanly (GuestExited{reset=false}).
#
# The guest's CID is NOT on the command line: one command line is shared by
# every VM of a spawn, while the VMM gives each VM its own CID through the
# virtio-vsock device's configuration space. The harness asks the vsock stack
# for the CID it was actually assigned (IOCTL_VM_SOCKETS_GET_LOCAL_CID), so
# `hello` names the CID its packets carry.
#
# Guest ↔ orchestrator is vsock only (plaza rule 0010: no NIC). The guest's
# half of the transport is four modules - the vsock core, the virtio transport
# and its shared code, and the virtio-mmio bus driver that gives the device its
# window - which the VMM publishes on the kernel command line as
# `virtio_mmio.device=<size>@<addr>:<irq>`. That parameter belongs to the
# virtio_mmio module, so init hands it straight to finit_module(2) (a modprobe
# would read /proc/cmdline itself; init loads the module directly). One
# `virtio_mmio.device=` entry exists per virtio-mmio device: the vsock device's
# and, when the VMM was given one, the block device's.
#
# The workspace volume: the VMM's second virtio-mmio device is a virtio-blk
# device backed by a host file (the writable workspace image). When the guest
# sees it, init mounts it (ext4) at /workspace and says so on the console; the
# `wspace <text>` serial command appends a line to /workspace/notes.txt and
# prints the file back, which is the guest-side proof that the volume is
# writable and survives the VM. A guest with no such device has an empty
# /workspace directory (not a volume), and `wspace` says so.
#
# The agent's shell: busybox (the host's /usr/lib/initcpio/busybox, dynamically
# linked, so its ELF interpreter and libraries are copied in at their absolute
# paths) with applet symlinks in /bin, /bin/sh among them. The harness runs
# `/bin/sh -c` as uid/gid 1000 in /workspace: init chowns /workspace to that
# uid, whether or not a volume is mounted there. /etc/passwd and /etc/group name
# root and agent (1000, home /workspace); /tmp is mode 1777.
#
# Without flags the guest heartbeats and serves commands forever
# (console=ttyS0 gives PID 1 stdin/stdout on the serial line).
#
# Output: target/guest/initrd.x86_64.gz   (usage: vmctl --initrd <path> [--console])
#         (the workspace image itself is built by scripts/mk-workspace-image.sh
#          and handed to the VMM with --workspace)
#
# Env overrides: APZ_HARNESS_BIN=/path/to/agent-harness  skip the cargo build.
#                APZ_KERNEL_RELEASE=7.2.9-arch1-1  take the guest's modules from
#                /lib/modules/<release> (default: the running kernel, uname -r).
set -eu

out_dir="target/guest"
root="$out_dir/initroot"
kernel_release="${APZ_KERNEL_RELEASE:-$(uname -r)}"
mod_root="/lib/modules/$kernel_release"

rm -rf "$root"
mkdir -p "$root/dev" "$root/proc" "$root/sys" "$root" "$root/lib/modules"

# ---------------------------------------------------------------------------
# The guest's virtio modules: resolved from modules.dep at build time, in
# dependency order (a module after every module it depends on), and written
# into the initramfs as /lib/modules/load-order for the guest's init.
#
# virtio_mmio is a module on this kernel; the block device's driver
# (virtio_blk) and the workspace volume's filesystem (ext4) are built in
# (CONFIG_VIRTIO_BLK=y, CONFIG_EXT4_FS=y), so the guest needs no module for
# them. That is checked here rather than assumed: a kernel that ships either as
# a module gets it added to the load order, because the guest's init mounts the
# volume right after loading the modules.
# ---------------------------------------------------------------------------
modules_dir="$root/lib/modules"
want="vsock vmw_vsock_virtio_transport_common vmw_vsock_virtio_transport virtio_mmio"

if [ ! -r "$mod_root/modules.dep" ]; then
    echo "mk-initrd: $mod_root/modules.dep not found: cannot resolve the guest's virtio modules" >&2
    exit 1
fi

# A kernel feature is either built in (no module to load) or a module that must
# load before the guest can use it. `modules.builtin` lists the built-ins by
# path, so an entry ending in `/<name>.ko` is exactly that feature.
for feature in virtio_blk ext4; do
    if grep -q "/${feature}\.ko\$" "$mod_root/modules.builtin" 2>/dev/null; then
        echo "guest kernel feature: $feature is built in"
    else
        echo "guest kernel feature: $feature is a module; adding it to the load order"
        want="$want $feature"
    fi
done

# The balloon driver, optional. noded boots the agent VMs with a balloon
# (--agents-balloon-mib), so a guest that has the driver can hand its unused
# pages back and grow later. Built in: nothing to load. A module: it joins the
# load order with the virtio modules it stands on (the ones that are modules; a
# built-in one needs no entry). Neither: skipped with a message, and the build
# goes on - the balloon is then inert in the guest.
if grep -q "/virtio_balloon\.ko\$" "$mod_root/modules.builtin" 2>/dev/null; then
    echo "guest kernel feature: virtio_balloon is built in"
elif grep -q "/virtio_balloon\.ko\(\.zst\)\?:" "$mod_root/modules.dep" 2>/dev/null; then
    balloon_wanted="virtio_balloon"
    for dep in virtio virtio_ring; do
        if grep -q "/${dep}\.ko\(\.zst\)\?:" "$mod_root/modules.dep" 2>/dev/null; then
            balloon_wanted="$balloon_wanted $dep"
        fi
    done
    echo "guest kernel feature: virtio_balloon is a module; adding it to the load order"
    want="$want $balloon_wanted"
else
    echo "mk-initrd: skipping virtio_balloon: not built in and not in $mod_root/modules.dep (the guest's balloon stays inert)"
fi

order=$(awk -v want="$want" -v dep_file="$mod_root/modules.dep" '
function visit(m,   k, j, d) {
    if (state[m] == 2) return
    if (state[m] == 1) { print "mk-initrd: module dependency cycle at " m > "/dev/stderr"; bad = 1; return }
    state[m] = 1
    k = split(dep[m], d, " ")
    for (j = 1; j <= k; j++) if (d[j] != "") visit(d[j])
    state[m] = 2
    printf "%s\n", found[m]
}
BEGIN {
    n = split(want, w, " ")
    for (i = 1; i <= n; i++) wanted[w[i]] = 1
}
{
    path = $0
    sub(/:.*/, "", path)
    base = path
    sub(/.*\//, "", base)
    sub(/\.ko(\.zst)?$/, "", base)
    if (!(base in wanted)) next
    found[base] = path
    count = split(substr($0, length(path) + 2), d, " ")
    for (i = 1; i <= count; i++) {
        if (d[i] == "") continue
        db = d[i]
        sub(/.*\//, "", db)
        sub(/\.ko(\.zst)?$/, "", db)
        if (db in wanted) dep[base] = dep[base] " " db
    }
}
END {
    for (m in found) {
        visit(m)
        if (bad) exit 1
    }
    for (m in wanted) {
        if (!(m in found)) {
            printf "mk-initrd: guest module %s is not in %s\n", m, dep_file > "/dev/stderr"
            bad = 1
        }
    }
    if (bad) exit 1
}
' "$mod_root/modules.dep")

: > "$modules_dir/load-order"
for module in $order; do
    name=${module##*/}
    name=${name%.zst}
    src="$mod_root/$module"
    case "$name" in
        *.ko) ;;
        *)
            echo "mk-initrd: $src is not a .ko file" >&2
            exit 1
            ;;
    esac
    case "$module" in
        *.zst)
            if ! command -v zstd >/dev/null 2>&1; then
                echo "mk-initrd: $src is zstd-compressed and zstd is not installed" >&2
                exit 1
            fi
            zstd -q -d -c "$src" > "$modules_dir/$name"
            ;;
        *)
            cp "$src" "$modules_dir/$name"
            ;;
    esac
    printf '%s\n' "$name" >> "$modules_dir/load-order"
done

echo "guest modules (load order):"
sed 's/^/  /' "$modules_dir/load-order"

# ---------------------------------------------------------------------------
# The agent harness: the guest end of the plaza's core loop. Built for
# x86_64-unknown-linux-gnu with +crt-static so the initramfs needs no dynamic
# loader and no shared libraries. The explicit --target matters: without it the
# RUSTFLAGS reach the proc-macro crates too (serde_derive), which cannot be
# built static.
# ---------------------------------------------------------------------------
harness="${APZ_HARNESS_BIN:-}"
if [ -z "$harness" ]; then
    RUSTFLAGS="-C target-feature=+crt-static" \
        cargo build --release -p agent-harness --target x86_64-unknown-linux-gnu
    harness="target/x86_64-unknown-linux-gnu/release/agent-harness"
fi
if [ ! -x "$harness" ]; then
    echo "mk-initrd: no agent harness binary at $harness" >&2
    exit 1
fi
if command -v file >/dev/null 2>&1; then
    if ! file -b "$harness" | grep -q static; then
        echo "mk-initrd: $harness is not statically linked; the guest has no dynamic loader" >&2
        exit 1
    fi
else
    # file(1) is missing: the static-link check cannot run. A dynamic binary
    # would only fail at guest exec, so say so instead of shipping silently.
    echo "mk-initrd: warning: file(1) not found, cannot verify $harness is statically linked" >&2
fi
cp "$harness" "$root/agent-harness"
if command -v strip >/dev/null 2>&1; then
    strip "$root/agent-harness" 2>/dev/null || true
fi
echo "agent harness: $harness -> /agent-harness"

# ---------------------------------------------------------------------------
# The agent's shell: the host's busybox, dynamically linked, so its ELF
# interpreter and every library ldd lists go in at their absolute paths (the
# interpreter is /lib64/ld-linux-x86-64.so.2; the image has no ld.so.cache, so
# ld.so searches its default directories, /usr/lib among them). Applets are
# symlinks to busybox, and only names `busybox --list` prints are linked.
# /workspace is made by init (own_workspace), not here.
# ---------------------------------------------------------------------------
bb="/usr/lib/initcpio/busybox"
if [ ! -x "$bb" ]; then
    echo "mk-initrd: no busybox at $bb (the agent's shell needs it)" >&2
    exit 1
fi
mkdir -p "$root/bin" "$root/etc" "$root/tmp"
cp "$bb" "$root/bin/busybox"
chmod 755 "$root/bin/busybox"
libs="$(ldd "$bb" | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^\//) print $i }' | sort -u)"
for lib in $libs; do
    mkdir -p "$root$(dirname "$lib")"
    cp -L "$lib" "$root$lib"
done
applets="$("$bb" --list)"
for applet in sh ash cat ls cp mv rm mkdir rmdir touch echo printf grep sed awk \
    head tail wc sort uniq cut tr find xargs diff tee date sleep env pwd \
    basename dirname stat chmod test '[' true false gzip tar md5sum sha256sum \
    which seq expr; do
    if printf '%s\n' "$applets" | grep -Fxq "$applet"; then
        ln -sf busybox "$root/bin/$applet"
    else
        echo "mk-initrd: busybox has no $applet applet; not linked" >&2
    fi
done
if [ ! -e "$root/bin/sh" ]; then
    echo "mk-initrd: busybox links no sh; the agent's shell cannot run" >&2
    exit 1
fi
# The guest's users (the harness runs commands as agent, uid/gid 1000) and a
# world-writable /tmp with the sticky bit.
printf '%s\n' 'root:x:0:0:root:/root:/bin/sh' \
    'agent:x:1000:1000:agent:/workspace:/bin/sh' > "$root/etc/passwd"
printf '%s\n' 'root:x:0:' 'agent:x:1000:' > "$root/etc/group"
chmod 1777 "$root/tmp"
echo "agent shell: $bb -> /bin/busybox, $(ls "$root/bin" | wc -l) entries in /bin"

# ---------------------------------------------------------------------------
# PID 1
# ---------------------------------------------------------------------------
cat > "$out_dir/init.c" <<'EOF'
// Minimal static PID 1 for the AgentPlaza guest (x86_64). Public domain.
//
// Mounts the pseudo-filesystems, beats a heartbeat to the serial console every
// 5 s, serves the serial command loop and - when the kernel command line says
// `plaza.harness=1` - loads the guest's virtio modules and runs the AgentPlaza
// agent harness, which prints its whole session to this same console.
//
// `plaza.keepalive=1` implies the harness and changes what an exit means: the
// agent fleet's sessions are long-lived and the harness owns its reconnect
// loop, so this init never reboots such a guest - it reports the harness's end
// and keeps the VM up, inspectable and killable.
//
// The same module load carries the workspace volume's transport: when the VMM
// gave the VM a workspace image, its virtio-blk device appears as /dev/vda and
// init mounts it (ext4) at /workspace. `wspace <text>` proves the volume is
// writable from the guest's side.
#include <errno.h>
#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned long parse_flag(const char *key)
{
    char buf[1024];
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return 0;
    buf[n] = '\0';
    const char *kv = strstr(buf, key);
    return kv ? strtoul(kv + strlen(key), NULL, 10) : 0;
}

// Whether `key=` appears anywhere on the kernel command line (its value is not
// looked at). Needed to tell `plaza.acpi=0` (present, disabled) apart from an
// absent flag, which parse_flag cannot: both read back as 0.
static int flag_present(const char *key)
{
    char buf[1024];
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return 0;
    buf[n] = '\0';
    return strstr(buf, key) != NULL;
}

// ACPI S5 is the default poweroff path now. `plaza.acpi=0` restores the
// pre-S5 behaviour (poweroff == reboot(RB_AUTOBOOT), the reset path) for
// comparison runs.
static int poweroff_is_acpi(void)
{
    if (flag_present("plaza.acpi=") && parse_flag("plaza.acpi=") == 0)
        return 0;
    return 1;
}

static void put(const char *s)
{
    write(1, s, strlen(s));
}

static void read_file_head(const char *path, unsigned lines)
{
    char buf[512];
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        put("cannot open ");
        put(path);
        put("\n");
        return;
    }
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        put("(empty)\n");
        return;
    }
    buf[n] = '\0';
    for (ssize_t i = 0; i < n && lines; i++) {
        if (buf[i] == '\n')
            if (lines-- == 1) {
                buf[i + 1] = '\0';
                break;
            }
    }
    put(buf);
}

// Flushes the guest's filesystems.
//
// PID 1 owns the guest's data until the kernel is asked to stop, and neither
// the kernel's own poweroff/restart path nor this VMM's reset waits for the
// filesystems to be flushed — a guest that stops with uncommitted writes
// (ext4's journal commits every 5 s) loses them. The workspace volume is where
// an agent's work lives, so the guest syncs before it asks the kernel to stop,
// and `sync` lets an operator do it on demand.
static void do_sync(void)
{
    sync();
    put("plaza guest: filesystems synced\n");
}

// Prints the kernel's interrupt table: which IRQ each device is on.
//
// This is how the guest proves the VMM's interrupt wiring: the legacy devices
// and the virtio-mmio transports must be on the identity-mapped IRQs the VMM
// registered them on (serial 4, i8042 1, vsock 5, workspace 6), and the count
// for a virtio device grows as its queue is used.
static void do_interrupts(void)
{
    char buf[2048];
    int fd = open("/proc/interrupts", O_RDONLY);
    if (fd < 0) {
        put("cannot open /proc/interrupts\n");
        return;
    }
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        put("(empty /proc/interrupts)\n");
        return;
    }
    buf[n] = '\0';
    unsigned lines = 16;
    for (ssize_t i = 0; i < n && lines; i++) {
        if (buf[i] == '\n')
            if (lines-- == 1) {
                buf[i + 1] = '\0';
                break;
            }
    }
    put(buf);
}

static void do_cpu(void)
{
    char buf[8192];
    int fd = open("/proc/cpuinfo", O_RDONLY);
    if (fd < 0) {
        put("cannot open /proc/cpuinfo\n");
        return;
    }
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        put("(empty cpuinfo)\n");
        return;
    }
    buf[n] = '\0';
    unsigned cpus = 0;
    char model[128] = "(model unknown)";
    for (ssize_t i = 0; i < n; i++) {
        if (buf[i] == '\n')
            buf[i] = '\0';
    }
    for (ssize_t i = 0; i < n; i += strlen(&buf[i]) + 1) {
        if (!strncmp(&buf[i], "processor", 9))
            cpus++;
        if (!strncmp(&buf[i], "model name", 10)) {
            const char *tab = strstr(&buf[i], ": ");
            if (tab && strlen(tab + 2) < sizeof model)
                snprintf(model, sizeof model, "%s", tab + 2);
        }
    }
    char out[192];
    int len = snprintf(out, sizeof out, "cpus: %u (%s)\n", cpus, model);
    write(1, out, (size_t)len);
}

// Defined with the rest of the workspace code, below; the serial command loop
// is above it because it is the guest's console protocol.
static void workspace_show(void);
static void workspace_append(const char *text);

static void do_command(const char *cmd)
{
    if (!strcmp(cmd, "help")) {
        put("commands: help, echo <text>, uname, uptime, mem, cpu, interrupts, wspace [text], sync, poweroff, reboot, halt\n");
    } else if (!strcmp(cmd, "interrupts")) {
        do_interrupts();
    } else if (!strcmp(cmd, "sync")) {
        do_sync();
    } else if (!strncmp(cmd, "wspace ", 7)) {
        workspace_append(cmd + 7);
    } else if (!strcmp(cmd, "wspace")) {
        workspace_show();
    } else if (!strncmp(cmd, "echo ", 5)) {
        put(cmd + 5);
        put("\n");
    } else if (!strcmp(cmd, "echo")) {
        put("\n");

    } else if (!strcmp(cmd, "cpu")) {
        do_cpu();
    } else if (!strcmp(cmd, "uname")) {
        put("Linux plaza-vm 7.2.8-arch1-2 #1 SMP PREEMPT_DYNAMIC x86_64\n");
    } else if (!strcmp(cmd, "uptime")) {
        read_file_head("/proc/uptime", 1);
    } else if (!strcmp(cmd, "mem")) {
        read_file_head("/proc/meminfo", 3);
    } else if (!strcmp(cmd, "poweroff")) {
        do_sync();
        if (poweroff_is_acpi()) {
            put("plaza guest: powering off (ACPI S5)\n");
            reboot(RB_POWER_OFF);
        } else {
            put("plaza guest: rebooting (plaza.acpi=0, no ACPI S5)\n");
            reboot(RB_AUTOBOOT);
        }
    } else if (!strcmp(cmd, "reboot")) {
        do_sync();
        put("plaza guest: rebooting\n");
        reboot(RB_AUTOBOOT);
    } else if (!strcmp(cmd, "halt")) {
        put("plaza guest: parking forever\n");
        for (;;)
            pause();
    } else {
        put("unknown command: ");
        put(cmd);
        put(" (try 'help')\n");
    }
}

// --- vsock: the guest's modules and the agent harness ----------------------

// Copies every `virtio_mmio.<param>=<value>` token of the kernel command line
// into `out` as `<param>=<value>`, space separated - the prefix stripped, so
// the string is exactly what finit_module(2) parses for the module.
//
// The vsock device's window and IRQ are published by the VMM this way
// (`virtio_mmio.device=<size>@<addr>:<irq>`). The parameter belongs to
// virtio_mmio; a modprobe reads /proc/cmdline for it by itself, and init loads
// the module directly, so init has to pass it along. Returns how many
// parameters were copied.
static int virtio_mmio_params(char *out, size_t cap)
{
    static const char prefix[] = "virtio_mmio.";
    char buf[1024];
    int count = 0;
    size_t used = 0;

    out[0] = '\0';
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return 0;
    buf[n] = '\0';

    for (char *token = strtok(buf, " \t\n"); token; token = strtok(NULL, " \t\n")) {
        if (strncmp(token, prefix, sizeof prefix - 1))
            continue;
        token += sizeof prefix - 1;
        size_t len = strlen(token);
        if (used + len + 2 > cap)
            break;
        if (used)
            out[used++] = ' ';
        memcpy(out + used, token, len + 1);
        used += len;
        count++;
    }
    return count;
}

// Loads one module from the initramfs with finit_module(2); `params` may be "".
static void insmod(const char *name, const char *params)
{
    char path[160];
    int len = snprintf(path, sizeof path, "/lib/modules/%s", name);
    if (len <= 0 || (size_t)len >= sizeof path)
        return;
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        put("plaza guest: cannot open ");
        put(path);
        put(": ");
        put(strerror(errno));
        put("\n");
        return;
    }
    long rc = syscall(SYS_finit_module, fd, params, 0);
    int err = errno;
    close(fd);
    if (rc < 0) {
        put("plaza guest: cannot load ");
        put(name);
        put(": ");
        put(strerror(err));
        put("\n");
    }
}

// Loads the guest's virtio module set in the order the initramfs was built
// with, giving virtio_mmio the device windows the command line published.
//
// A guest whose command line carries no `virtio_mmio.device=` has no
// virtio-mmio device at all (neither a vsock device nor a workspace volume), so
// there is nothing to load and nothing to report unless the caller asked for
// the harness and would otherwise wait for a device that cannot appear
// (`verbose`).
static void load_guest_modules(int verbose)
{
    char params[256];
    char line[160];
    const int published = virtio_mmio_params(params, sizeof params);
    if (published <= 0) {
        if (verbose)
            put("plaza guest: the command line has no virtio_mmio.device=; the vsock device is missing\n");
        return;
    }
    FILE *order = fopen("/lib/modules/load-order", "r");
    if (!order) {
        put("plaza guest: no /lib/modules/load-order in the initramfs\n");
        return;
    }
    while (fgets(line, sizeof line, order)) {
        size_t len = strlen(line);
        while (len && (line[len - 1] == '\n' || line[len - 1] == '\r'))
            line[--len] = '\0';
        if (!len)
            continue;
        // Only virtio_mmio has a command-line parameter here; the vsock and
        // block modules take none.
        insmod(line, strncmp(line, "virtio_mmio", 11) ? "" : params);
    }
    fclose(order);
    char msg[160];
    int len = snprintf(msg, sizeof msg,
                       "plaza guest: virtio_mmio took %d command-line parameter(s)\n",
                       published);
    write(1, msg, (size_t)len);
}

// --- workspace: the writable volume the block device carries -----------------

// Mounts the workspace volume at /workspace, if the VM has one.
//
// The volume is a virtio-blk device on a virtio-mmio transport, so it appears
// as /dev/vda once the driver probed it - the module load above is
// synchronous, and the disk node exists by the time finit_module(2) returns,
// but a short poll costs nothing and covers a deferred probe. A VM with no
// block device (no `virtio_mmio.device=` for one) simply has no /workspace:
// the guest is still a valid guest, so a missing device is not an error.
// Set once the volume is mounted at /workspace (workspace_ready reads it).
static int workspace_mounted;

static void mount_workspace(void)
{
    int fd = -1;
    for (int attempt = 0; attempt < 200; attempt++) {
        fd = open("/dev/vda", O_RDONLY | O_CLOEXEC);
        if (fd >= 0)
            break;
        usleep(10000);
    }
    if (fd < 0)
        return;
    close(fd);

    if (mkdir("/workspace", 0755) < 0 && errno != EEXIST) {
        put("plaza guest: cannot create /workspace: ");
        put(strerror(errno));
        put("\n");
        return;
    }
    if (mount("/dev/vda", "/workspace", "ext4", 0, "") < 0) {
        char msg[160];
        int len = snprintf(msg, sizeof msg,
                           "plaza guest: cannot mount /dev/vda on /workspace: %s\n",
                           strerror(errno));
        write(1, msg, (size_t)len);
        return;
    }
    workspace_mounted = 1;
    struct statvfs vfs;
    unsigned long mib = 0;
    if (statvfs("/workspace", &vfs) == 0 && vfs.f_frsize)
        mib = (unsigned long)(((unsigned long long)vfs.f_blocks * vfs.f_frsize) >> 20);
    char msg[128];
    int len = snprintf(msg, sizeof msg, "plaza guest: /workspace mounted (ext4, %lu MiB)\n", mib);
    write(1, msg, (size_t)len);
}

// The agent's shell runs as uid 1000 in /workspace, so that directory must
// belong to uid 1000. Called after mount_workspace(): with a volume mounted the
// chown lands on its root directory, without one on the empty mount point.
static void own_workspace(void)
{
    if (mkdir("/workspace", 0755) < 0 && errno != EEXIST) {
        put("plaza guest: cannot create /workspace: ");
        put(strerror(errno));
        put("\n");
        return;
    }
    if (chown("/workspace", 1000, 1000) < 0) {
        put("plaza guest: cannot chown /workspace: ");
        put(strerror(errno));
        put("\n");
    }
}

// Whether the VM has a mounted workspace volume.
static int workspace_ready(void)
{
    return workspace_mounted;
}

// Prints /workspace/notes.txt (bounded: it is the guest's own data).
static void workspace_show(void)
{
    if (!workspace_ready()) {
        put("plaza guest: no workspace volume mounted\n");
        return;
    }
    char buf[4096];
    int fd = open("/workspace/notes.txt", O_RDONLY);
    if (fd < 0) {
        put("plaza guest: no /workspace/notes.txt yet\n");
        return;
    }
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n < 0)
        n = 0;
    buf[n] = '\0';
    write(1, buf, (size_t)n);
    if (n > 0 && buf[n - 1] != '\n')
        put("\n");
}

// Appends `text` as one line of /workspace/notes.txt and prints the file back:
// the guest-side proof that the volume is writable, and - read back after a
// reboot on the same image - that it persists.
static void workspace_append(const char *text)
{
    if (!workspace_ready()) {
        put("plaza guest: no workspace volume mounted\n");
        return;
    }
    int fd = open("/workspace/notes.txt", O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (fd < 0) {
        put("plaza guest: cannot open /workspace/notes.txt: ");
        put(strerror(errno));
        put("\n");
        return;
    }
    write(fd, text, strlen(text));
    write(fd, "\n", 1);
    close(fd);
    put("plaza guest: appended to /workspace/notes.txt; contents:\n");
    workspace_show();
}

// Starts the agent harness; it inherits this console as its stdout.
static pid_t start_harness(void)
{
    pid_t pid = fork();
    if (pid < 0) {
        put("plaza guest: cannot fork the agent harness\n");
        return -1;
    }
    if (pid == 0) {
        execl("/agent-harness", "agent-harness", (char *)NULL);
        put("plaza guest: cannot exec /agent-harness: ");
        put(strerror(errno));
        put("\n");
        _exit(127);
    }
    char msg[96];
    int len = snprintf(msg, sizeof msg, "plaza guest: agent harness running as pid %d\n", (int)pid);
    write(1, msg, (size_t)len);
    return pid;
}

// Reaps the harness once it has finished.
//
// Without keepalive (the v1 session), a completed session reboots the guest
// (RB_AUTOBOOT -> reset), the same clean path `plaza.beats` uses; the serial
// `poweroff` command is the only ACPI S5 (shutdown) path. A failed session is
// reported and the guest stays up, so its transcript can be read.
//
// With keepalive (the agent fleet) the guest NEVER reboots: the harness owns
// its own reconnect loop, so an exit means the hub stopped the agent or the
// harness gave up for good - in both cases the VM stays up, inspectable and
// killable, and this init keeps serving its console.
static void reap_harness(pid_t *harness, int keepalive)
{
    if (*harness <= 0)
        return;
    int status = 0;
    if (waitpid(*harness, &status, WNOHANG) != *harness)
        return;
    *harness = -1;
    if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
        if (keepalive) {
            put("plaza guest: agent harness stopped by the hub; staying up\n");
            return;
        }
        put("plaza guest: agent harness session complete, rebooting\n");
        do_sync();
        reboot(RB_AUTOBOOT);
    }
    char msg[96];
    int len;
    if (keepalive) {
        len = snprintf(msg, sizeof msg,
                       "plaza guest: agent harness ended (status %d); staying up\n", status);
    } else if (WIFEXITED(status)) {
        len = snprintf(msg, sizeof msg,
                       "plaza guest: agent harness failed (exit %d); staying up\n",
                       WEXITSTATUS(status));
    } else if (WIFSIGNALED(status)) {
        len = snprintf(msg, sizeof msg,
                       "plaza guest: agent harness killed by signal %d; staying up\n",
                       WTERMSIG(status));
    } else {
        len = snprintf(msg, sizeof msg, "plaza guest: agent harness ended (status %d); staying up\n", status);
    }
    write(1, msg, (size_t)len);
}

// Brings the loopback interface up, so a proxy on 127.0.0.1 inside the guest
// is reachable. Failure is reported on the serial line and does not stop init.
static void lo_up(void)
{
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) {
        put("plaza guest: lo: socket failed\n");
        return;
    }
    struct ifreq ifr;
    memset(&ifr, 0, sizeof ifr);
    strncpy(ifr.ifr_name, "lo", IFNAMSIZ - 1);
    if (ioctl(fd, SIOCGIFFLAGS, &ifr) < 0) {
        put("plaza guest: lo: SIOCGIFFLAGS failed\n");
    } else {
        ifr.ifr_flags |= IFF_UP | IFF_RUNNING;
        if (ioctl(fd, SIOCSIFFLAGS, &ifr) < 0)
            put("plaza guest: lo: SIOCSIFFLAGS failed\n");
    }
    close(fd);
}

int main(void)
{
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");
    mount("proc", "/proc", "proc", 0, "");
    mount("sysfs", "/sys", "sysfs", 0, "");
    lo_up();

    const unsigned long beats = parse_flag("plaza.beats=");
    const unsigned long halt = parse_flag("plaza.halt=");
    const unsigned long keepalive = parse_flag("plaza.keepalive=");
    // A keepalive agent is a harness by definition: the flag says "long-lived
    // session", and the harness is what speaks it.
    const unsigned long harness = parse_flag("plaza.harness=") || keepalive;
    pid_t harness_pid = -1;

    if (halt) {
        put("plaza guest: parking forever (plaza.halt)\n");
        for (;;)
            pause();
    }

    if (harness)
        put("plaza guest: plaza.harness=1: loading the vsock modules\n");
    // The virtio modules carry every virtio-mmio device this VM has: the vsock
    // device and/or the block device of its workspace volume.
    load_guest_modules((int)harness);
    // The workspace volume, if this VM was given one: mounted before anything
    // the harness does, so its sessions (and the serial `wspace` command) can
    // use it.
    mount_workspace();
    own_workspace();
    if (harness)
        harness_pid = start_harness();

    for (unsigned long beat = 1;; beat++) {
        reap_harness(&harness_pid, (int)keepalive);

        char line[160];
        int len = snprintf(line, sizeof line,
                           "plaza guest alive: heartbeat %lu uptime~%lus\n",
                           beat, (beat - 1) * 5);
        write(1, line, (size_t)len);
        if (beats > 0 && beat >= beats) {
            len = snprintf(line, sizeof line,
                           "plaza guest: %lu beats done, rebooting\n", beats);
            write(1, line, (size_t)len);
            do_sync();
            reboot(RB_AUTOBOOT);
        }

        // Serve one serial command per heartbeat window, if any input arrives.
        struct pollfd pfd = { .fd = 0, .events = POLLIN };
        for (int left = 5; left > 0;) {
            int r = poll(&pfd, 1, left * 1000);
            if (r <= 0)
                break;
            char in[64];
            ssize_t n = read(0, in, sizeof(in) - 1);
            if (n <= 0)
                break;
            in[n] = '\0';
            for (ssize_t i = 0; i < n; i++)
                if (in[i] == '\r' || in[i] == '\n')
                    in[i] = '\0';
            do_command(in);
            left = 5; // a command resets the heartbeat window
        }
    }
}
EOF

gcc -static -Os -Wall -Wextra -o "$root/init" "$out_dir/init.c"
strip "$root/init"

(cd "$root" && find . | cpio -o -H newc --quiet) | gzip -9 > "$out_dir/initrd.x86_64.gz"

echo "built $out_dir/initrd.x86_64.gz"
ls -la "$out_dir/initrd.x86_64.gz"
