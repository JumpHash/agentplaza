#!/usr/bin/env bash
# Runs apid's database tests against a throwaway Postgres 17 container.
#
# Starts postgres:17 (must already be present locally: this script never pulls) on a random free
# 127.0.0.1 port with a random password and --rm, exports APZ_TEST_DATABASE_URL to it, runs
# `cargo test -p apid db` and the registry's database test, and removes the container on exit.
# It never reads APZ_DATABASE_URL, never uses port 5432, and leaves every other container alone.
#
#   scripts/test-db.sh            # the db tests
#   scripts/test-db.sh migrate    # only tests whose path matches the filter
#   scripts/test-db.sh all        # every apid test, with the database set
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${APZ_TEST_POSTGRES_IMAGE:-postgres:17}"
FILTER="${1:-db}"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "test-db: image $IMAGE is not present locally; this script does not pull images" >&2
    exit 1
fi

PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
PASSWORD="$(python3 -c 'import secrets; print(secrets.token_hex(16))')"
NAME="apz-sqltest-$$-$RANDOM"

cleanup() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run --rm -d --name "$NAME" -e POSTGRES_PASSWORD="$PASSWORD" \
    -p "127.0.0.1:$PORT:5432" "$IMAGE" >/dev/null

ready=""
for _ in $(seq 1 90); do
    if docker exec "$NAME" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 1
done
if [ -z "$ready" ]; then
    echo "test-db: container $NAME did not become ready" >&2
    exit 1
fi

export APZ_TEST_DATABASE_URL="postgres://postgres:$PASSWORD@127.0.0.1:$PORT/postgres"
unset APZ_DATABASE_URL

cd "$ROOT"
echo "test-db: throwaway container $NAME on 127.0.0.1:$PORT"
if [ "$FILTER" = all ]; then
    cargo test -p apid
else
    cargo test -p apid "$FILTER"
    cargo test -p apid registry
fi
