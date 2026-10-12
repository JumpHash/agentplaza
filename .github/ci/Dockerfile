# The environment every Linux CI job runs in: on the org's self-hosted runner (an x86_64 host) the
# jobs build it (`docker build -t agentplaza-ci .github/ci`, its layers cached on the host) and run
# their builds inside it; a local Forgejo runner uses it as its ubuntu-22.04 image. Ubuntu 22.04
# (glibc 2.35, so the packages run on every distribution as new or newer) with everything a job
# needs already installed: the build tools, the desktop app's headers, the guest kernel's
# toolchain, the arm64 cross toolchain (gcc, libc, Rust target, an arm64 busybox for its guest,
# qemu to run its tests), the packagers (fpm for deb/rpm/pacman), Rust, cargo-nextest, the gh CLI
# and Node.js — so a job spends its time compiling, not installing.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl wget git file xz-utils zstd cpio sudo \
      build-essential pkg-config lld \
      libxkbcommon-dev libwayland-dev \
      flex bison bc libelf-dev libssl-dev busybox-static \
      ruby ruby-dev rpm libarchive-tools \
      gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user \
 && gem install --no-document fpm \
 && rm -rf /var/lib/apt/lists/*

# An arm64 busybox for the aarch64 guest (mk-initrd.sh, APZ_BUSYBOX), from Ubuntu's ports archive.
RUN sed -i 's/^deb http/deb [arch=amd64] http/' /etc/apt/sources.list \
 && echo 'deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy main universe' > /etc/apt/sources.list.d/ports.list \
 && dpkg --add-architecture arm64 && apt-get update \
 && cd /tmp && apt-get download busybox-static:arm64 \
 && dpkg-deb -x busybox-static_*_arm64.deb /tmp/bb && install -D -m 755 /tmp/bb/bin/busybox /opt/arm64/busybox \
 && rm -rf /tmp/bb /tmp/*.deb /var/lib/apt/lists/*
ENV APZ_BUSYBOX_AARCH64=/opt/arm64/busybox

# The GitHub CLI, for the release job.
ENV GH_VERSION=2.89.0
RUN curl -fsSL "https://github.com/cli/cli/releases/download/v$GH_VERSION/gh_${GH_VERSION}_linux_amd64.tar.gz" \
      | tar -xz -C /usr/local --strip-components=1 "gh_${GH_VERSION}_linux_amd64/bin/gh" \
 && gh --version

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    PKG_CONFIG_ALLOW_CROSS=1
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
 && arch="$(uname -m)" \
 && curl -LsSf "https://get.nexte.st/latest/linux$( [ "$arch" = aarch64 ] && echo -arm )" | tar zxf - -C "$CARGO_HOME/bin" \
 && rustup target add aarch64-unknown-linux-gnu \
 && cargo nextest --version \
 && curl -fsSL "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-$arch-unknown-linux-musl.tar.gz" \
      | tar -xz -C "$CARGO_HOME/bin" --strip-components=1 "sccache-v0.18.0-$arch-unknown-linux-musl/sccache" \
 && sccache --version \
 && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Node.js: the web console's build, and JavaScript actions under a local runner (Forgejo / act).
ENV NODE_VERSION=24.21.0
RUN arch="$(uname -m)"; [ "$arch" = x86_64 ] && narch=x64 || narch=arm64; \
    curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$narch.tar.xz" \
      | tar -xJ -C /usr/local --strip-components=1 \
 && node --version
