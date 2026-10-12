# The environment a local Forgejo runner gives the Linux CI jobs (label ubuntu-22.04 →
# docker://agentplaza-ci:<arch>, built locally with `docker build -t agentplaza-ci:x86_64 .github/ci`):
# GitHub's own ubuntu-22.04 runners have the same base. Ubuntu 22.04 (glibc 2.35, so the packages run on every
# distribution as new or newer) with everything a job needs already installed — the build tools,
# the desktop app's headers, the guest kernel's toolchain, the packagers (fpm for deb/rpm/pacman),
# Rust, cargo-nextest, sccache and Node.js (JavaScript actions run inside the job's container on a local
# Forgejo runner; GitHub mounts its own) — so a job spends its minutes compiling, not installing.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git file xz-utils zstd cpio sudo \
      build-essential pkg-config lld \
      libxkbcommon-dev libwayland-dev \
      flex bison bc libelf-dev libssl-dev busybox-static \
      ruby ruby-dev rpm libarchive-tools \
 && gem install --no-document fpm \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
 && arch="$(uname -m)" \
 && curl -LsSf "https://get.nexte.st/latest/linux$( [ "$arch" = aarch64 ] && echo -arm )" | tar zxf - -C "$CARGO_HOME/bin" \
 && cargo nextest --version \
 && curl -fsSL "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-$arch-unknown-linux-musl.tar.gz" \
      | tar -xz -C "$CARGO_HOME/bin" --strip-components=1 "sccache-v0.18.0-$arch-unknown-linux-musl/sccache" \
 && sccache --version \
 && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Node.js for JavaScript actions under a local runner (Forgejo Actions / act).
ENV NODE_VERSION=24.21.0
RUN arch="$(uname -m)"; [ "$arch" = x86_64 ] && narch=x64 || narch=arm64; \
    curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$narch.tar.xz" \
      | tar -xJ -C /usr/local --strip-components=1 \
 && node --version
