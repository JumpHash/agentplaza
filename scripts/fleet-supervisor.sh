#!/bin/sh
# Keeps a batch of N microVMs alive forever: every generation boots N guests
# (each lives plaza.beats heartbeats, then reboots cleanly), and when the
# generation has drained the supervisor boots the next one. Demo of the
# plaza's node-daemon VM lifecycle semantics (spawn -> run -> exit -> respawn)
# on top of the vmctl front-end.
#
# Tunables (env): APZ_KERNEL, APZ_INITRD, APZ_FLEET_N (default 8),
# APZ_FLEET_BEATS (default 3). Assumes `cargo build` has produced
# target/debug/vmctl (run from the workspace root).
set -eu

kernel="${APZ_KERNEL:-/boot/vmlinuz-linux}"
initrd="${APZ_INITRD:-target/guest/initrd.x86_64.gz}"
n="${APZ_FLEET_N:-8}"
beats="${APZ_FLEET_BEATS:-3}"

gen=1
while :; do
    echo "vmfleet-supervisor: spawning generation $gen ($n vms, beats=$beats)"
    if target/debug/vmctl \
        --kernel "$kernel" \
        --initrd "$initrd" \
        -n "$n" \
        --mem-mib 512 \
        --cmdline "console=ttyS0 plaza.beats=$beats" \
        --timeout-secs 120
    then
        echo "vmfleet-supervisor: generation $gen drained cleanly"
    else
        echo "vmfleet-supervisor: generation $gen ended with failure"
    fi
    gen=$((gen + 1))
done
