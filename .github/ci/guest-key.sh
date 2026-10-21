#!/bin/sh
# The cache key of the guest image for one architecture: `guest-<arch>-<id>`. The id is made from git's own
# content ids of what the guest is built from (its scripts, the workspace's crates and manifests, the cargo
# config and the CI image), so it is the same on every runner and operating system, and a build that leaves
# files in the work tree does not change it. guest.yml saves the image under this key; the platform jobs
# restore it with the same key.
set -eu
arch=$1
id=$(git ls-tree HEAD -- \
  scripts/mk-guest.sh scripts/mk-guest-kernel.sh scripts/mk-initrd.sh scripts/guest \
  crates Cargo.toml Cargo.lock .cargo .github/ci/Dockerfile | git hash-object --stdin)
echo "guest-$arch-$id"
