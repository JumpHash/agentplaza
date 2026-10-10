#!/bin/sh
# Builds a minimal x86_64 guest initramfs for the AgentPlaza VMM demo: a static
# PID 1 that mounts the pseudo-filesystems, prints a heartbeat to the serial
# console every 5 s and accepts simple commands over the serial console.
#
# Cmdline flags:
#   plaza.beats=N  power off cleanly after N heartbeats (churn mode for the
#                  respawn supervisor). The port has no ACPI S5 path yet, so
#                  "power off" is a guest reboot (RB_AUTOBOOT -> i8042/triple
#                  fault -> KVM exit -> VmHandle::wait() == GuestExited{reset}).
#   plaza.halt=1   park forever in pause() after the banner (a deliberately
#                  unresponsive guest: exercises the host kill path).
#
# Without flags it heartbeats and serves commands forever (console=ttyS0 gives
# PID 1 stdin/stdout on the serial line).
#
# Output: target/guest/initrd.x86_64.gz   (usage: vmctl --initrd <path> [--console])
set -eu

out_dir="target/guest"
root="$out_dir/initroot"

rm -rf "$root"
mkdir -p "$root/dev" "$root/proc" "$root/sys" "$root"

cat > "$out_dir/init.c" <<'EOF'
// Minimal static PID 1 for the AgentPlaza guest (x86_64). Public domain.
#include <fcntl.h>
#include <poll.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
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

int main(void)
{
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");
    mount("proc", "/proc", "proc", 0, "");
    mount("sysfs", "/sys", "sysfs", 0, "");

    const unsigned long beats = parse_flag("plaza.beats=");
    const unsigned long halt = parse_flag("plaza.halt=");

    if (halt) {
        put("plaza guest: parking forever (plaza.halt)\n");
        for (;;)
            pause();
    }

    for (unsigned long beat = 1;; beat++) {
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
