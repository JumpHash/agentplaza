#!/bin/sh
# image.sh: makes sure the CI image is on this runner, tagged by its Dockerfile's hash (built once
# per Dockerfile change, its layers cached by Docker), and exports CI_IMAGE for the job's later steps.
set -eu
dir="$(dirname "$0")"
image="agentplaza-ci:$(sha256sum "$dir/Dockerfile" | cut -c1-16)"
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" "$dir"
echo "CI_IMAGE=$image" >> "$GITHUB_ENV"
