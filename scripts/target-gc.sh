#!/bin/sh
# Keeps the build output small. Run by hand or by the daily user timer that
# scripts/install-target-gc.sh installs.
#
#   1. drops incremental-compilation caches untouched for $APZ_GC_STALE_DAYS
#      (default 3) days: they are the fastest-growing part of target/;
#   2. drops build artifacts (deps/, build/, .fingerprint/ entries) untouched
#      for $APZ_GC_STALE_DAYS days: what old toolchains and old dependency
#      versions leave behind;
#   3. if target/ is still over $APZ_TARGET_CAP_GB (default 6), drops every
#      intermediate (deps/, build/, incremental/, .fingerprint/) — the next
#      build starts from scratch, which is cheaper than an ever-growing disk.
#
# Never touched: target/guest/ (the guest initramfs and workspace image) and
# the final binaries at target/<profile>/<name> (running daemons spawn them).
# The web console's node_modules is not touched (npm owns it).
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target="${CARGO_TARGET_DIR:-$root/target}"
stale_days="${APZ_GC_STALE_DAYS:-3}"
cap_gb="${APZ_TARGET_CAP_GB:-6}"

[ -d "$target" ] || { echo "target-gc: no $target, nothing to do"; exit 0; }

size_kb() { du -sk "$target" | cut -f1; }
before=$(size_kb)

# 1 + 2: stale incremental caches and stale artifacts, per profile directory.
for profile in "$target"/*/ "$target"/*/*/; do
    [ -d "$profile" ] || continue
    for sub in incremental deps build .fingerprint; do
        [ -d "$profile$sub" ] || continue
        find "$profile$sub" -mindepth 1 -maxdepth 1 -mtime +"$stale_days" -exec rm -rf {} + 2>/dev/null || true
    done
done

after=$(size_kb)
cap_kb=$((cap_gb * 1024 * 1024))
if [ "$after" -gt "$cap_kb" ]; then
    echo "target-gc: $((after / 1024)) MiB is over the ${cap_gb} GiB cap; dropping all intermediates"
    for profile in "$target"/*/ "$target"/*/*/; do
        for sub in incremental deps build .fingerprint; do
            rm -rf "$profile$sub"
        done
    done
    after=$(size_kb)
fi

echo "target-gc: $((before / 1024)) MiB -> $((after / 1024)) MiB"
