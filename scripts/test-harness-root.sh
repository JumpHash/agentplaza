#!/bin/sh
# Runs the agent-harness root tests (named root_, ignored by default) as root in a user namespace.
# Inside, the caller is root and uid 1100 (the engine user) is mapped from the caller's subordinate
# ids, so chown and setuid are real. The engine user is added to private copies of /etc/passwd and
# /etc/group, bind-mounted over the originals: the namespace cannot write the host's.
set -eu

cd "$(dirname "$0")/.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# APZ_HARNESS_TEST_EXE: a test executable built elsewhere (CI builds it in its image and runs it here, since a
# container's seccomp profile refuses the user namespace).
if [ -n "${APZ_HARNESS_TEST_EXE:-}" ]; then
  exe=$APZ_HARNESS_TEST_EXE
else
  cargo test --offline -p agent-harness --no-run --message-format=json >"$work/build.json"
  exe=$(jq -r 'select(.reason == "compiler-artifact" and .profile.test and .target.name == "agent-harness" and (.target.kind | index("bin"))) | .executable' "$work/build.json" | tail -n 1)
fi
[ -n "$exe" ] && [ -x "$exe" ] || { echo "no agent-harness test executable" >&2; exit 1; }
unshare --map-auto --map-root-user true 2>/dev/null || {
  echo "this machine gives no user namespace with subordinate ids (unshare --map-auto; /etc/subuid and /etc/subgid for $(id -un))" >&2
  exit 1
}

cp /etc/passwd "$work/passwd"
cp /etc/group "$work/group"
grep -q '^apz-engine:' "$work/passwd" || echo 'apz-engine:x:1100:1100:plaza coding engine:/nonexistent:/sbin/nologin' >>"$work/passwd"
grep -q '^apz-engine:' "$work/group" || echo 'apz-engine:x:1100:' >>"$work/group"

unshare --map-auto --map-root-user --mount sh -c '
    mount --bind "$1" /etc/passwd && mount --bind "$2" /etc/group && exec "$3" --ignored --test-threads=1 root_
' sh "$work/passwd" "$work/group" "$exe"
