#!/usr/bin/env bash
# Load schema.sql into a real PostgreSQL 16 engine (PGlite, in-process; no server, no root) and run the behaviour tests.
# Needs Node 22+ and npm. The first run installs the pinned PGlite into tests/node_modules.
set -euo pipefail
cd "$(dirname "$0")/tests"
[ -d node_modules/@electric-sql/pglite ] || npm install --no-audit --no-fund --silent
node ledger.test.mjs
node mutants.mjs
