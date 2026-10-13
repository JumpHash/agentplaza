#!/usr/bin/env bash
# Points git at this repository's .githooks, so the pre-commit migration check runs on every commit.
# Run once per clone. Changes only this clone's core.hooksPath.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
git -C "$ROOT" config core.hooksPath .githooks
echo "install-hooks: core.hooksPath is now .githooks (pre-commit: scripts/check-migrations.sh --staged)"
