#!/usr/bin/env bash
# Maintains crates/apid/sql/migrations/migrations.lock: one `NNNN name sha256` line per migration.
#
#   scripts/migrations.sh lock    appends a line for each migration file the lock does not list yet;
#                                 refuses (writes nothing) when a listed file no longer matches its
#                                 line, because an applied migration was edited
#   scripts/migrations.sh check   verifies the lock lists exactly the files, with their hashes
#
# Works on the git repository it is run in. Applied migrations are immutable: a change to one is a
# new migration file (docs/dev/sql.md).
set -euo pipefail

usage() {
    echo "usage: scripts/migrations.sh lock|check" >&2
    exit 2
}

ROOT="$(git rev-parse --show-toplevel)"
DIR="$ROOT/crates/apid/sql/migrations"
LOCK="$DIR/migrations.lock"
MODE="${1:-}"
[ "$MODE" = lock ] || [ "$MODE" = check ] || usage

shopt -s nullglob
files=("$DIR"/[0-9][0-9][0-9][0-9]_*.sql)
shopt -u nullglob

declare -A locked_name=()
declare -A locked_sha=()
locked_lines=0
if [ -f "$LOCK" ]; then
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        read -r version name sha <<<"$line"
        locked_name[$version]="$name"
        locked_sha[$version]="$sha"
        locked_lines=$((locked_lines + 1))
    done <"$LOCK"
fi

problems=()
new_lines=()
for file in "${files[@]}"; do
    base="$(basename "$file")"
    version="${base:0:4}"
    name="${base:5:${#base}-9}"
    sha="$(sha256sum "$file" | cut -d' ' -f1)"
    if [ -n "${locked_sha[$version]:-}" ]; then
        if [ "${locked_name[$version]}" != "$name" ] || [ "${locked_sha[$version]}" != "$sha" ]; then
            problems+=("$base changed after it was locked (applied migrations are immutable: add a new migration instead)")
        fi
    else
        new_lines+=("$version $name $sha")
    fi
done

# Lock lines whose file is gone.
for version in "${!locked_name[@]}"; do
    if [ ! -f "$DIR/${version}_${locked_name[$version]}.sql" ]; then
        problems+=("${version}_${locked_name[$version]}.sql is in migrations.lock but missing (applied migrations are immutable: add a new migration instead)")
    fi
done

if [ "${#problems[@]}" -gt 0 ]; then
    for problem in "${problems[@]}"; do
        echo "migrations: $problem" >&2
    done
    exit 1
fi

if [ "$MODE" = check ]; then
    if [ "${#new_lines[@]}" -gt 0 ]; then
        for line in "${new_lines[@]}"; do
            echo "migrations: not in migrations.lock yet: $line (run scripts/migrations.sh lock)" >&2
        done
        exit 1
    fi
    echo "migrations: lock matches ${#files[@]} migration file(s)"
    exit 0
fi

# lock: append only, after the new numbers are checked to continue the locked sequence.
expect=$((locked_lines + 1))
for line in "${new_lines[@]}"; do
    version="${line%% *}"
    if [ "$((10#$version))" -ne "$expect" ]; then
        echo "migrations: $version is not the next number ($(printf '%04d' "$expect")): numbers have no gaps or reuse" >&2
        exit 1
    fi
    expect=$((expect + 1))
done
if [ "${#new_lines[@]}" -eq 0 ]; then
    echo "migrations: lock already lists every migration file"
    exit 0
fi
for line in "${new_lines[@]}"; do
    printf '%s\n' "$line" >>"$LOCK"
    echo "migrations: locked $line"
done
