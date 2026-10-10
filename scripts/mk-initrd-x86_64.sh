#!/bin/sh
# Builds a minimal x86_64 guest initramfs for the AgentPlaza VMM demo: a static
# PID 1 that mounts the pseudo-filesystems, prints a heartbeat to the serial
# console every 5 s and accepts simple commands over the serial console.
#
# Cmdline flags:
#   plaza.beats=N    power off cleanly after N heartbeats (churn mode for the
#                    respawn supervisor). The port has no ACPI S5 path yet, so
#                    "power off" is a guest reboot (RB_AUTOBOOT -> i8042/triple
#                    fault -> KVM exit -> VmHandle::wait() == GuestExited{reset}).
#   plaza.halt=1     park forever in pause() after the banner (a deliberately
#                    unresponsive guest: exercises the host kill path).
#   plaza.harness=1  load the guest's vsock module set and run the AgentPlaza
#                    agent harness (this initramfs' /agent-harness), whose
#                    whole session prints to this same serial console. A
#                    session that ends with the guest's `bye` reboots the guest
#                    (the same clean-exit path plaza.beats uses); a failed one
#                    is reported and the guest stays up for its transcript.
#   plaza.cid=N      the guest's vsock CID, read by the harness itself.
#   plaza.port=N     the orchestrator's vsock port (default 5000), read by the
#                    harness itself.
#
# Guest ↔ orchestrator is vsock only (plaza rule 0010: no NIC). The guest's
# half of the transport is four modules - the vsock core, the virtio transport
# and its shared code, and the virtio-mmio bus driver that gives the device its
# window - which the VMM publishes on the kernel command line as
# `virtio_mmio.device=<size>@<addr>:<irq>`. That parameter belongs to the
# virtio_mmio module, so init hands it straight to finit_module(2) (a modprobe
# would read /proc/cmdline itself; init loads the module directly).
#
# Without flags the guest heartbeats and serves commands forever
# (console=ttyS0 gives PID 1 stdin/stdout on the serial line).
#
# Output: target/guest/initrd.x86_64.gz   (usage: vmctl --initrd <path> [--console])
#
# Env overrides: APZ_HARNESS_BIN=/path/to/agent-harness  skip the cargo build.
set -eu

out_dir="target/guest"
root="$out_dir/initroot"
kernel_release="$(uname -r)"
mod_root="/lib/modules/$kernel_release"

rm -rf "$root"
mkdir -p "$root/dev" "$root/proc" "$root/sys" "$root" "$root/lib/modules"

# ---------------------------------------------------------------------------
# The guest's vsock modules: resolved from modules.dep at build time, in
# dependency order (a module after every module it depends on), and written
# into the initramfs as /lib/modules/load-order for the guest's init.
# ---------------------------------------------------------------------------
modules_dir="$root/lib/modules"
want="vsock vmw_vsock_virtio_transport_common vmw_vsock_virtio_transport virtio_mmio"

if [ ! -r "$mod_root/modules.dep" ]; then
    echo "mk-initrd: $mod_root/modules.dep not found: cannot resolve the guest's vsock modules" >&2
    exit 1
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
if command -v file >/dev/null 2>&1 && ! file -b "$harness" | grep -q static; then
    echo "mk-initrd: $harness is not statically linked; the guest has no dynamic loader" >&2
    exit 1
fi
cp "$harness" "$root/agent-harness"
if command -v strip >/dev/null 2>&1; then
    strip "$root/agent-harness" 2>/dev/null || true
fi
echo "agent harness: $harness -> /agent-harness"

# ---------------------------------------------------------------------------
# PID 1
# ---------------------------------------------------------------------------
cat > "$out_dir/init.c" <<'EOF'
// Minimal static PID 1 for the AgentPlaza guest (x86_64). Public domain.
//
// Mounts the pseudo-filesystems, beats a heartbeat to the serial console every
// 5 s, serves the serial command loop and - when the kernel command line says
// `plaza.harness=1` - loads the guest's vsock modules and runs the AgentPlaza
// agent harness, which prints its whole session to this same console.
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
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

static void do_command(const char *cmd)
{
    if (!strcmp(cmd, "help")) {
        put("commands: help, echo <text>, uname, uptime, mem, cpu, poweroff, reboot, halt\n");
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
    } else if (!strcmp(cmd, "poweroff") || !strcmp(cmd, "reboot")) {
        put("plaza guest: rebooting (no ACPI S5 in the VMM yet)\n");
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

// Loads the guest's vsock module set in the order the initramfs was built
// with, giving virtio_mmio the device window the command line published.
static void load_vsock_modules(void)
{
    char params[256];
    char line[160];
    const int published = virtio_mmio_params(params, sizeof params);
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
        // Only virtio_mmio has a command-line parameter here; the vsock
        // modules take none.
        insmod(line, strncmp(line, "virtio_mmio", 11) ? "" : params);
    }
    fclose(order);
    if (published <= 0) {
        put("plaza guest: the command line has no virtio_mmio.device=; the vsock device is missing\n");
    } else {
        char msg[160];
        int len = snprintf(msg, sizeof msg,
                           "plaza guest: virtio_mmio took %d command-line parameter(s)\n",
                           published);
        write(1, msg, (size_t)len);
    }
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

// Reaps the harness once it has finished. A completed session reboots the
// guest: the VMM has no ACPI S5, so a guest-initiated exit is a reset - the
// same clean path `plaza.beats` uses. A failed session is reported and the
// guest stays up, so its transcript can be read.
static void reap_harness(pid_t *harness)
{
    if (*harness <= 0)
        return;
    int status = 0;
    if (waitpid(*harness, &status, WNOHANG) != *harness)
        return;
    *harness = -1;
    if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
        put("plaza guest: agent harness session complete, rebooting\n");
        reboot(RB_AUTOBOOT);
    }
    char msg[96];
    int len;
    if (WIFEXITED(status)) {
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

int main(void)
{
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");
    mount("proc", "/proc", "proc", 0, "");
    mount("sysfs", "/sys", "sysfs", 0, "");

    const unsigned long beats = parse_flag("plaza.beats=");
    const unsigned long halt = parse_flag("plaza.halt=");
    const unsigned long harness = parse_flag("plaza.harness=");
    pid_t harness_pid = -1;

    if (halt) {
        put("plaza guest: parking forever (plaza.halt)\n");
        for (;;)
            pause();
    }

    if (harness) {
        put("plaza guest: plaza.harness=1: loading the vsock modules\n");
        load_vsock_modules();
        harness_pid = start_harness();
    }

    for (unsigned long beat = 1;; beat++) {
        reap_harness(&harness_pid);

        char line[160];
        int len = snprintf(line, sizeof line,
                           "plaza guest alive: heartbeat %lu uptime~%lus\n",
                           beat, (beat - 1) * 5);
        write(1, line, (size_t)len);
        if (beats > 0 && beat >= beats) {
            len = snprintf(line, sizeof line,
                           "plaza guest: %lu beats done, rebooting\n", beats);
            write(1, line, (size_t)len);
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
