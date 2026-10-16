#!/usr/bin/env python3
"""Keeps a runner's cargo build directory warm, so a run compiles only what changed.

  build-cache.py mtimes <cache-dir>
      Run right after the checkout, from the repository's root. The runners start every job from a
      fresh clone, so every file has a new mtime and cargo, which decides by mtime, would rebuild
      every workspace crate. This gives each file whose content (git blob) is the one this runner
      last saw the mtime it had then, and records the new ones: the working copy looks as if it had
      persisted, so an unchanged crate stays fresh and a changed file is newer than anything built
      before it. A manifest per checkout path lives in <cache-dir>/mtimes.

  build-cache.py prune <target-dir> [--max-gb N] [--min-free-gb N]
      Run at the end of a job. Reports the build directory's size and the disk's free space and,
      when the directory is over its budget or the disk is short, removes the build units no build
      has used for 14, then 7, 3 and 1 days (a unit's last use is when cargo last read or wrote its
      fingerprint). Only when the disk is still short does it empty the directory. Units in use are
      kept, so the next run does not start cold. The budget is --max-gb (80) or 60% of the disk,
      whichever is less; short is less free than --min-free-gb (25) or 10% of the disk, whichever is
      less, so a small disk is not short all the time.
"""

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time

GB = 1 << 30


def mtimes(cache):
    root = subprocess.run(["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True,
                          text=True).stdout.strip()
    os.chdir(root)
    tag = hashlib.sha256(os.path.realpath(root).encode()).hexdigest()[:16]
    path = os.path.join(cache, "mtimes", tag)
    seen = {}
    try:
        with open(path, "rb") as f:
            for record in f.read().split(b"\0"):
                if record:
                    blob, ns, name = record.split(b" ", 2)
                    seen[name] = (blob, int(ns))
    except FileNotFoundError:
        pass

    listing = subprocess.run(["git", "ls-files", "-s", "-z"], check=True, capture_output=True).stdout
    now = {}
    kept = changed = 0
    for entry in listing.split(b"\0"):
        if not entry:
            continue
        meta, name = entry.split(b"\t", 1)
        mode, blob, _stage = meta.split(b" ")
        # Links and submodules: nothing cargo reads the mtime of.
        if mode in (b"120000", b"160000"):
            continue
        last = seen.get(name)
        try:
            if last and last[0] == blob:
                os.utime(name, ns=(last[1], last[1]))
                now[name] = last
                kept += 1
            else:
                now[name] = (blob, os.stat(name).st_mtime_ns)
                changed += 1
        except FileNotFoundError:
            # Sparse or skipped: not in the working copy.
            continue

    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "wb") as f:
        f.write(b"".join(b"%s %d %s\0" % (blob, ns, name) for name, (blob, ns) in now.items()))
    os.replace(path + ".tmp", path)
    first = " (the first run here: everything counts as new)" if not seen else ""
    print(f"mtimes: {kept} files as this runner last built them, {changed} new or changed{first}")


def size(path):
    """Bytes on disk under `path` (allocated blocks where the OS reports them)."""
    total = 0
    for top, _dirs, files in os.walk(path):
        for name in files:
            try:
                st = os.lstat(os.path.join(top, name))
            except OSError:
                continue
            blocks = getattr(st, "st_blocks", None)
            total += blocks * 512 if blocks is not None else st.st_size
    return total


def profiles(target):
    """The directories cargo keeps units in: <target>/<profile> and <target>/<triple>/<profile>."""
    found = []
    for depth1 in os.scandir(target):
        if not depth1.is_dir():
            continue
        if os.path.isdir(os.path.join(depth1.path, ".fingerprint")):
            found.append(depth1.path)
        for depth2 in os.scandir(depth1.path):
            if depth2.is_dir() and os.path.isdir(os.path.join(depth2.path, ".fingerprint")):
                found.append(depth2.path)
    return found


def remove(entry):
    if entry.is_dir(follow_symlinks=False):
        shutil.rmtree(entry.path, ignore_errors=True)
    else:
        try:
            os.remove(entry.path)
        except OSError:
            pass


HASH = re.compile(r"-([0-9a-f]{16})(?:\.|$)")


def last_used(unit):
    latest = os.stat(unit).st_mtime
    for entry in os.scandir(unit):
        st = entry.stat(follow_symlinks=False)
        latest = max(latest, st.st_atime, st.st_mtime)
    return latest


def sweep(target, days):
    """Removes every unit whose fingerprint nothing has read or written for `days`."""
    cutoff = time.time() - days * 86400
    removed = 0
    for profile in profiles(target):
        fingerprints = os.path.join(profile, ".fingerprint")
        stale = set()
        for unit in os.scandir(fingerprints):
            m = HASH.search(unit.name)
            if m and unit.is_dir() and last_used(unit.path) < cutoff:
                stale.add(m.group(1))
                shutil.rmtree(unit.path, ignore_errors=True)
                removed += 1
        if not stale:
            continue
        for sub in ("deps", "build", "examples"):
            where = os.path.join(profile, sub)
            if not os.path.isdir(where):
                continue
            for entry in os.scandir(where):
                m = HASH.search(entry.name)
                if m and m.group(1) in stale:
                    remove(entry)
    return removed


def prune(target, max_gb, min_free_gb):
    if not os.path.isdir(target):
        print(f"prune: no build directory at {target}")
        return
    disk = shutil.disk_usage(target).total
    max_gb = min(max_gb, 0.6 * disk / GB)
    min_free_gb = min(min_free_gb, 0.1 * disk / GB)
    def measure():
        return size(target), shutil.disk_usage(target).free
    parts = {e.name: size(e.path) for e in os.scandir(target) if e.is_dir(follow_symlinks=False)}
    used, free = sum(parts.values()), shutil.disk_usage(target).free
    listing = ", ".join(f"{name} {n / GB:.1f}" for name, n in sorted(parts.items()))
    print(f"prune: build directory {used / GB:.1f} GB ({listing}); disk {free / GB:.1f} GB free of "
          f"{disk / GB:.0f}; budget {max_gb:.0f} GB, short under {min_free_gb:.1f} GB free")
    for days in (14, 7, 3, 1):
        if used <= max_gb * GB and free >= min_free_gb * GB:
            return
        units = sweep(target, days)
        used, free = measure()
        print(f"prune: removed {units} units unused for {days} days: now {used / GB:.1f} GB, "
              f"{free / GB:.1f} GB free")
    if free < min_free_gb * GB:
        print(f"prune: the disk is still short ({free / GB:.1f} GB free): emptying the build directory")
        for entry in os.scandir(target):
            remove(entry)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    m = sub.add_parser("mtimes")
    m.add_argument("cache")
    p = sub.add_parser("prune")
    p.add_argument("target")
    p.add_argument("--max-gb", type=float, default=80)
    p.add_argument("--min-free-gb", type=float, default=25)
    args = parser.parse_args()
    if args.command == "mtimes":
        mtimes(args.cache)
    else:
        prune(args.target, args.max_gb, args.min_free_gb)


if __name__ == "__main__":
    sys.exit(main())
