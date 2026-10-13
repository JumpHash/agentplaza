#!/usr/bin/env bash
# Tests scripts/check-migrations.sh, scripts/migrations.sh and the --staged mode the pre-commit hook
# uses. Each case builds a throwaway git repository with a locked migration history, makes one
# change, and expects the check to pass, or to fail with the named file.
set -euo pipefail

SCRIPTS="$(cd "$(dirname "$0")" && pwd)"
CHECK="$SCRIPTS/check-migrations.sh"
LOCKTOOL="$SCRIPTS/migrations.sh"
FOLDER=crates/apid/sql/migrations
RULE='applied migrations are immutable: add a new migration instead'
TMP="$(mktemp -d "${TMPDIR:-/tmp}/apz-migcheck.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
passed=0
failed=0

# A repository with migration 0001 locked and committed; leaves the shell in it.
fresh_repo() {
    local dir="$TMP/$1"
    mkdir -p "$dir/$FOLDER"
    cd "$dir"
    git init -q
    git config user.email test@example.invalid
    git config user.name test
    git config commit.gpgsign false
    printf -- '-- 0001 test\nCREATE TABLE a (x int);\n' >"$FOLDER/0001_init.sql"
    "$LOCKTOOL" lock >/dev/null
    git add -A
    git commit -qm base
}

commit_all() {
    git add -A
    git commit -qm change
}

sha_of() {
    sha256sum "$1" | cut -d' ' -f1
}

# run LABEL pass|fail NEEDLE COMMAND...: the command must exit 0 (pass) or 1 (fail); for fail, its
# output must contain NEEDLE.
run() {
    local label="$1" want="$2" needle="$3"
    shift 3
    local out status=0
    out="$("$@" 2>&1)" || status=$?
    local ok=yes
    if [ "$want" = pass ]; then
        [ "$status" -eq 0 ] || ok=no
    else
        [ "$status" -eq 1 ] || ok=no
        case "$out" in *"$needle"*) ;; *) ok=no ;; esac
    fi
    if [ "$ok" = yes ]; then
        passed=$((passed + 1))
        echo "ok    $label"
    else
        failed=$((failed + 1))
        echo "FAIL  $label (want $want, exit $status)"
        printf '%s\n' "$out" | sed 's/^/        /'
    fi
}

# 1. Modifying an applied migration fails, naming the file.
fresh_repo modify
printf -- '-- edited\nCREATE TABLE a (x int, y int);\n' >"$FOLDER/0001_init.sql"
commit_all
run "modify: fails" fail "0001_init.sql: modified; $RULE" "$CHECK" HEAD~1

# 2. Deleting an applied migration fails.
fresh_repo delete
git rm -q "$FOLDER/0001_init.sql"
commit_all
run "delete: fails" fail "0001_init.sql: deleted; $RULE" "$CHECK" HEAD~1

# 3. Renaming an applied migration fails.
fresh_repo rename
git mv "$FOLDER/0001_init.sql" "$FOLDER/0001_renamed.sql"
commit_all
run "rename: fails" fail "0001_init.sql: renamed to crates/apid/sql/migrations/0001_renamed.sql; $RULE" "$CHECK" HEAD~1

# 4. A new migration with the next number, locked, passes.
fresh_repo new
printf -- '-- 0002 add y\nALTER TABLE a ADD COLUMN y int;\n' >"$FOLDER/0002_add_y.sql"
"$LOCKTOOL" lock >/dev/null
commit_all
run "new migration: passes" pass "" "$CHECK" HEAD~1

# 5. A gap (0003 after 0001) fails.
fresh_repo gap
printf -- '-- 0003 skips two\nSELECT 1;\n' >"$FOLDER/0003_skips.sql"
printf '0003 skips %s\n' "$(sha_of "$FOLDER/0003_skips.sql")" >>"$FOLDER/migrations.lock"
commit_all
run "gap: fails" fail "0003_skips.sql" "$CHECK" HEAD~1
run "gap: the message names the expected number" fail "number 0003 must be 0002" "$CHECK" HEAD~1

# 6. Reusing a number (two 0002 files) fails.
fresh_repo reuse
printf -- '-- 0002 a\nSELECT 1;\n' >"$FOLDER/0002_a.sql"
printf -- '-- 0002 b\nSELECT 2;\n' >"$FOLDER/0002_b.sql"
printf '0002 a %s\n0002 b %s\n' "$(sha_of "$FOLDER/0002_a.sql")" "$(sha_of "$FOLDER/0002_b.sql")" >>"$FOLDER/migrations.lock"
commit_all
run "reuse: fails" fail "0002_b.sql" "$CHECK" HEAD~1

# 7. Changing an existing lock line fails, even with no migration file touched.
fresh_repo lockedit
printf '0001 init %s\n' 0000000000000000000000000000000000000000000000000000000000000000 >"$FOLDER/migrations.lock"
commit_all
run "lock edit: fails" fail "migrations.lock: an existing line changed or was removed; $RULE" "$CHECK" HEAD~1

# 8. Removing a lock line fails.
fresh_repo lockremove
: >"$FOLDER/migrations.lock"
commit_all
run "lock removal: fails" fail "migrations.lock: an existing line changed or was removed" "$CHECK" HEAD~1

# 9. Appending a lock line for a new migration passes (the lock is append-only).
fresh_repo lockappend
printf -- '-- 0002 add y\nALTER TABLE a ADD COLUMN y int;\n' >"$FOLDER/0002_add_y.sql"
"$LOCKTOOL" lock >/dev/null
commit_all
run "lock append: passes" pass "" "$CHECK" HEAD~1

# 10. The lock tool refuses to rewrite a line whose file changed, and writes nothing.
fresh_repo tool
printf -- '-- edited\nSELECT 2;\n' >"$FOLDER/0001_init.sql"
before="$(cat "$FOLDER/migrations.lock")"
run "lock tool: refuses an edited applied file" fail "0001_init.sql changed after it was locked" "$LOCKTOOL" lock
[ "$before" = "$(cat "$FOLDER/migrations.lock")" ] && echo "ok    lock tool: the lock is left as it was" && passed=$((passed + 1)) || { echo "FAIL  lock tool rewrote the lock"; failed=$((failed + 1)); }

# 11. The pre-commit mode: a staged edit of an applied migration is refused.
fresh_repo staged-modify
printf -- '-- edited\nSELECT 3;\n' >"$FOLDER/0001_init.sql"
git add -A
run "pre-commit: a staged modify fails" fail "0001_init.sql: modified; $RULE" "$CHECK" --staged

# 12. The pre-commit mode: a staged new locked migration passes.
fresh_repo staged-new
printf -- '-- 0002 add y\nSELECT 4;\n' >"$FOLDER/0002_add_y.sql"
"$LOCKTOOL" lock >/dev/null
git add -A
run "pre-commit: a staged new migration passes" pass "" "$CHECK" --staged

# 13. The pre-commit mode: a staged deletion of an applied migration is refused.
fresh_repo staged-delete
git rm -q "$FOLDER/0001_init.sql"
run "pre-commit: a staged delete fails" fail "0001_init.sql: deleted; $RULE" "$CHECK" --staged

echo
echo "check-migrations tests: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
