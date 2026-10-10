#!/usr/bin/env bash
# Compile the protocol draft and run the tests.
# Needs protoc with Python output, e.g. `pip install grpcio-tools` and PROTOC="python -m grpc_tools.protoc",
# and PYTHON pointing at a Python that has the protobuf package.
set -euo pipefail
cd "$(dirname "$0")"
PROTOC="${PROTOC:-protoc}"
PYTHON="${PYTHON:-python3}"
out="$(mktemp -d)"; trap 'rm -rf "$out"' EXIT
$PROTOC -I. --descriptor_set_out="$out/ap.pb" --include_imports --python_out="$out" agentplaza/v1/*.proto
echo "ok: agentplaza/v1 compiles"
"$PYTHON" -I tests/test_protocol.py "$out"
