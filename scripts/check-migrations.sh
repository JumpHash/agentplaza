#!/usr/bin/env bash
# Applied migrations are immutable: add a new migration instead (docs/dev/sql.md).
#
#   scripts/check-migrations.sh <base-ref>   compares the merge base of <base-ref> and HEAD with HEAD
#                                            (CI: the pull request's base, or the previous commit on main)
#   scripts/check-migrations.sh --staged     compares the index with HEAD (the pre-commit hook)
#
# Fails when, against the base, in any crates/*/sql/migrations/ folder:
#   1. a migration file is modified, deleted or renamed (migrations.lock excepted);
#   2. an existing line of migrations.lock is changed or removed (lines may be appended);
#   3. a new migration's number is not exactly the base's maximum + 1, + 2, ... (no gaps, no reuse).
# Exits 0 when clean, 1 with one message per violation, 2 on a usage or git error.
set -euo pipefail

case "${1:-}" in
    --staged) mode=staged ;;
    "") echo "usage: scripts/check-migrations.sh <base-ref> | --staged" >&2; exit 2 ;;
    *) mode=committed; base_ref="$1" ;;
esac

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

EMPTY_TREE="$(git hash-object -t tree /dev/null)"
MIG_RE='^crates/[^/]+/sql/migrations/'
LOCK_RE='^crates/[^/]+/sql/migrations/migrations\.lock$'
SQL_RE='^crates/[^/]+/sql/migrations/[0-9]{4}_[^/]*\.sql$'
RULE='applied migrations are immutable: add a new migration instead'

if [ "$mode" = staged ]; then
    if git rev-parse --verify -q HEAD >/dev/null; then
        BASE=HEAD
    else
        BASE="$EMPTY_TREE"
    fi
    diff_cmd=(git diff --cached --name-status --find-renames "$BASE")
    head_show() { git show ":$1"; }
else
    BASE="$(git merge-base "$base_ref" HEAD 2>/dev/null)" || { echo "check-migrations: no merge base with $base_ref" >&2; exit 2; }
    diff_cmd=(git diff --name-status --find-renames "$BASE" HEAD)
    head_show() { git show "HEAD:$1"; }
fi

violations=()
added=()

# 1 and the list of new migration files (2 and 3 read from these).
while IFS=$'\t' read -r status p1 p2; do
    case "$status" in
        A)
            if [[ $p1 =~ $MIG_RE && $p1 == *.sql ]]; then added+=("$p1"); fi
            ;;
        M|T|D)
            if [[ $p1 =~ $MIG_RE && ! $p1 =~ $LOCK_RE ]]; then
                verb=modified; [ "$status" = D ] && verb=deleted; [ "$status" = T ] && verb=type-changed
                violations+=("$p1: $verb; $RULE")
            fi
            ;;
        R*)
            if [[ $p1 =~ $MIG_RE && ! $p1 =~ $LOCK_RE ]]; then
                violations+=("$p1: renamed to $p2; $RULE")
            fi
            if [[ $p2 =~ $MIG_RE && $p2 == *.sql ]]; then added+=("$p2"); fi
            ;;
        C*)
            if [[ $p2 =~ $MIG_RE && $p2 == *.sql ]]; then added+=("$p2"); fi
            ;;
    esac
done < <("${diff_cmd[@]}")

# 2: every line of each migrations.lock that the base has is still there, unchanged, in order.
while IFS= read -r lock; do
    [ -n "$lock" ] || continue
    git show "$BASE:$lock" >"$TMP/base.lock"
    lines="$(wc -l <"$TMP/base.lock")"
    if ! head_show "$lock" >"$TMP/head.lock" 2>/dev/null; then
        violations+=("$lock: removed; $RULE")
    elif ! cmp -s <(head -n "$lines" "$TMP/head.lock") "$TMP/base.lock"; then
        violations+=("$lock: an existing line changed or was removed; $RULE")
    fi
done < <(git ls-tree -r --name-only "$BASE" | grep -E "$LOCK_RE" || true)

# 3: per folder, the base's highest number, and the new numbers that must follow it exactly.
declare -A base_max=()
while IFS= read -r path; do
    [ -n "$path" ] || continue
    folder="${path%/sql/migrations/*}"
    version=$((10#$(basename "$path" | cut -c1-4)))
    if [ "$version" -gt "${base_max[$folder]:-0}" ]; then
        base_max[$folder]=$version
    fi
done < <(git ls-tree -r --name-only "$BASE" | grep -E "$SQL_RE" || true)

declare -A new_numbers=()
for path in "${added[@]+"${added[@]}"}"; do
    folder="${path%/sql/migrations/*}"
    new_numbers[$folder]+="$(basename "$path" | cut -c1-4) $(basename "$path") "
done
for folder in "${!new_numbers[@]}"; do
    expect=$(( ${base_max[$folder]:-0} + 1 ))
    # shellcheck disable=SC2086
    while read -r number file; do
        version=$((10#$number))
        if [ "$version" -ne "$expect" ]; then
            violations+=("$folder/sql/migrations/$file: number $number must be $(printf '%04d' "$expect"), the next after the base's $(printf '%04d' "${base_max[$folder]:-0}"), with no gaps or reuse")
        fi
        expect=$((version + 1))
    done < <(printf '%s\n' ${new_numbers[$folder]} | paste - - | sort -k1,1n)
done

if [ "${#violations[@]}" -gt 0 ]; then
    for violation in "${violations[@]}"; do
        echo "check-migrations: $violation" >&2
    done
    exit 1
fi
echo "check-migrations: ok against $(git rev-parse --short "$BASE")"
