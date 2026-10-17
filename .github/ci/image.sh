#!/bin/sh
# image.sh: makes sure the CI image is on this runner, tagged by its Dockerfile's hash, and exports
# CI_IMAGE for the job's later steps. A runner without it pulls the image ci-image.yml publishes for
# that Dockerfile and this architecture (about a minute, against six to build). Without one (a pull
# request that changes the Dockerfile, or before ci-image.yml has run for it) it builds the image,
# its layers cached by Docker. The runner fleet's storage guard deletes every image when the host's
# disk is short, so this runs often.
set -eu
dir="$(dirname "$0")"
hash="$(sha256sum "$dir/Dockerfile" | cut -c1-16)"
image="agentplaza-ci:$hash"
case "$(uname -m)" in x86_64) arch=amd64 ;; aarch64 | arm64) arch=arm64 ;; *) arch="$(uname -m)" ;; esac
owner="$(printf %s "${GITHUB_REPOSITORY_OWNER:-jumphash}" | tr '[:upper:]' '[:lower:]')"
published="ghcr.io/$owner/agentplaza-ci:$hash-$arch"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  if docker pull -q "$published" >/dev/null 2>&1; then
    docker tag "$published" "$image"
    echo "pulled $published"
  else
    echo "no published $published: building the CI image"
    docker build -t "$image" "$dir"
  fi
fi
echo "CI_IMAGE=$image" >> "$GITHUB_ENV"
