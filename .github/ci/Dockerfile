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
      libxkbcommon-dev libwayland-dev libpipewire-0.3-dev libclang-dev \
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

# AppImage tooling for the Linux AppImages (packaging/linux/build.sh): appimagetool, unpacked (a
# container has no FUSE), and the static type2 runtimes the AppImages start with (no libfuse2 needed
# on the user's system), one per architecture. Pinned and checked by SHA-256.
RUN curl -fsSL -o /tmp/appimagetool https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
 && echo "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0  /tmp/appimagetool" | sha256sum -c - \
 && chmod +x /tmp/appimagetool && cd /opt && /tmp/appimagetool --appimage-extract >/dev/null \
 && mv /opt/squashfs-root /opt/appimagetool && ln -s /opt/appimagetool/AppRun /usr/local/bin/appimagetool \
 && rm /tmp/appimagetool \
 && mkdir -p /opt/appimage-runtime \
 && curl -fsSL -o /opt/appimage-runtime/runtime-x86_64 https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
 && curl -fsSL -o /opt/appimage-runtime/runtime-aarch64 https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-aarch64 \
 && printf '%s\n' \
      "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d  /opt/appimage-runtime/runtime-x86_64" \
      "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444  /opt/appimage-runtime/runtime-aarch64" \
    | sha256sum -c - \
 && appimagetool --version
ENV APZ_APPIMAGE_RUNTIMES=/opt/appimage-runtime

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
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    PKG_CONFIG_ALLOW_CROSS=1
# The runner's -L reaches only the test binary. A process a test starts (the fake ACP agent, a helper) is an
# aarch64 binary the kernel hands to binfmt's qemu without -L, so it could not find /lib/ld-linux-aarch64.so.1
# and died at once. qemu reads the same prefix from this variable, which every child inherits.
ENV QEMU_LD_PREFIX=/usr/aarch64-linux-gnu
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
 && arch="$(uname -m)" \
 && curl -LsSf "https://get.nexte.st/latest/linux$( [ "$arch" = aarch64 ] && echo -arm )" | tar zxf - -C "$CARGO_HOME/bin" \
 && rustup target add aarch64-unknown-linux-gnu wasm32-unknown-unknown \
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
