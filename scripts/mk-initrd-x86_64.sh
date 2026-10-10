#!/bin/sh
# Builds a minimal x86_64 guest initramfs for the AgentPlaza VMM demo: a static
# PID 1 that mounts the pseudo-filesystems, prints a heartbeat to the serial
# console every 5 s and — if the kernel cmdline carries plaza.beats=N — powers
# the guest off cleanly after N heartbeats (reboot(RB_POWER_OFF) -> KVM exit ->
# VmHandle::wait() == GuestExited). Without plaza.beats it runs forever and the
# host stops it with VmHandle::kill().
#
# Output: target/guest/initrd.x86_64.gz   (usage: vmctl --initrd <path>)
set -eu

out_dir="target/guest"
root="$out_dir/initroot"

rm -rf "$root"
mkdir -p "$root/dev" "$root/proc" "$root/sys" "$root"

cat > "$out_dir/init.c" <<'EOF'
// Minimal static PID 1 for the AgentPlaza guest (x86_64). Public domain.
#include <fcntl.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <unistd.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned long parse_beats(void)
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
    const char *kv = strstr(buf, "plaza.beats=");
    return kv ? strtoul(kv + strlen("plaza.beats="), NULL, 10) : 0;
}

int main(void)
{
    mount("devtmpfs", "/dev", "devtmpfs", 0, "");
    mount("proc", "/proc", "proc", 0, "");
    mount("sysfs", "/sys", "sysfs", 0, "");

    const unsigned long beats = parse_beats();

    for (unsigned long beat = 1;; beat++) {
        char line[128];
        int len = snprintf(line, sizeof line,
                           "plaza guest alive: heartbeat %lu uptime~%lus\n",
                           beat, (beat - 1) * 5);
        write(1, line, (size_t)len);
        if (beats > 0 && beat >= beats) {
            len = snprintf(line, sizeof line,
                           "plaza guest: %lu beats done, powering off\n", beats);
            write(1, line, (size_t)len);
            reboot(RB_POWER_OFF);
        }
        sleep(5);
    }
}
EOF

gcc -static -Os -Wall -Wextra -o "$root/init" "$out_dir/init.c"
strip "$root/init"

(cd "$root" && find . | cpio -o -H newc --quiet) | gzip -9 > "$out_dir/initrd.x86_64.gz"

echo "built $out_dir/initrd.x86_64.gz"
ls -la "$out_dir/initrd.x86_64.gz"
