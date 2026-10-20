# The environment every Linux CI job runs in: on the org's self-hosted runner (an x86_64 host) the
# jobs build it (`docker build -t agentplaza-ci .github/ci`, its layers cached on the host) and run
# their builds inside it; a local Forgejo runner uses it as its ubuntu-22.04 image. Ubuntu 22.04
# (glibc 2.35, so the packages run on every distribution as new or newer) with everything a job
# needs already installed: the build tools, the desktop app's headers, the guest kernel's
# toolchain, the arm64 cross toolchain (gcc, libc, Rust target, an arm64 busybox for its guest,
# qemu to run its tests), the packagers (fpm for deb/rpm/pacman), Rust (with clippy and rustfmt),
# cargo-nextest, cargo-deny, the gh CLI, Node.js, and the API tests' Bruno and Schemathesis — so a
# job spends its time compiling, not installing.
#
# It builds for the runner's own architecture. On the x86_64 runners it carries the arm64 cross
# toolchain, qemu as cargo's runner for --target aarch64 and the AppImage tooling (the packages,
# arm64's included, are built there). On the arm64 runners it builds and tests natively and has none
# of those.
ARG TARGETARCH
FROM ubuntu:22.04 AS common

ENV DEBIAN_FRONTEND=noninteractive
RUN case "$(dpkg --print-architecture)" in \
      amd64) cross="gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user" ;; *) cross="" ;; esac \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl wget git file xz-utils zstd cpio sudo \
      build-essential pkg-config lld cmake \
      libxkbcommon-dev libwayland-dev libpipewire-0.3-dev libclang-dev \
      libwebkit2gtk-4.1-dev xvfb libnss3 libgbm1 libasound2 \
      flex bison bc libelf-dev libssl-dev busybox-static \
      ruby ruby-dev rpm libarchive-tools \
      $cross \
 && gem install --no-document fpm \
 && rm -rf /var/lib/apt/lists/*

# An arm64 busybox for the aarch64 guest (mk-initrd.sh, APZ_BUSYBOX): Ubuntu's own on arm64, from
# its ports archive on x86_64.
RUN if [ "$(dpkg --print-architecture)" = arm64 ]; then install -D -m 755 /bin/busybox /opt/arm64/busybox; else \
    sed -i 's/^deb http/deb [arch=amd64] http/' /etc/apt/sources.list \
 && echo 'deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy main universe' > /etc/apt/sources.list.d/ports.list \
 && dpkg --add-architecture arm64 && apt-get update \
 && cd /tmp && apt-get download busybox-static:arm64 \
 && dpkg-deb -x busybox-static_*_arm64.deb /tmp/bb && install -D -m 755 /tmp/bb/bin/busybox /opt/arm64/busybox \
 && rm -rf /tmp/bb /tmp/*.deb /var/lib/apt/lists/*; fi
ENV APZ_BUSYBOX_AARCH64=/opt/arm64/busybox

# AppImage tooling for the Linux AppImages (packaging/linux/build.sh): appimagetool, unpacked (a
# container has no FUSE), and the static type2 runtimes the AppImages start with (no libfuse2 needed
# on the user's system), one per architecture. Pinned and checked by SHA-256. x86_64 only: the
# packages are built there.
RUN [ "$(dpkg --print-architecture)" = amd64 ] || exit 0; \
    curl -fsSL -o /tmp/appimagetool https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
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
RUN arch="$(dpkg --print-architecture)" \
 && curl -fsSL "https://github.com/cli/cli/releases/download/v$GH_VERSION/gh_${GH_VERSION}_linux_${arch}.tar.gz" \
      | tar -xz -C /usr/local --strip-components=1 "gh_${GH_VERSION}_linux_${arch}/bin/gh" \
 && gh --version

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    PKG_CONFIG_ALLOW_CROSS=1
# Rust is pinned, so clippy's lints (the pull request gate fails on warnings) change only when this
# line does. cargo-deny checks advisories, licences and sources (deny.toml).
ENV RUST_VERSION=1.99.0 CARGO_DENY_VERSION=0.20.2
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" --component clippy,rustfmt --no-modify-path \
 && arch="$(uname -m)" \
 && curl -LsSf "https://get.nexte.st/latest/linux$( [ "$arch" = aarch64 ] && echo -arm )" | tar zxf - -C "$CARGO_HOME/bin" \
 && rustup target add aarch64-unknown-linux-gnu wasm32-unknown-unknown \
 && cargo nextest --version \
 && curl -fsSL "https://github.com/mozilla/sccache/releases/download/v0.18.0/sccache-v0.18.0-$arch-unknown-linux-musl.tar.gz" \
      | tar -xz -C "$CARGO_HOME/bin" --strip-components=1 "sccache-v0.18.0-$arch-unknown-linux-musl/sccache" \
 && sccache --version \
 && curl -fsSL "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/cargo-deny-$CARGO_DENY_VERSION-$arch-unknown-linux-musl.tar.gz" \
      | tar -xz -C "$CARGO_HOME/bin" --strip-components=1 "cargo-deny-$CARGO_DENY_VERSION-$arch-unknown-linux-musl/cargo-deny" \
 && cargo deny --version \
 && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Node.js: the web console's build, and JavaScript actions under a local runner (Forgejo / act).
ENV NODE_VERSION=24.21.0
RUN arch="$(uname -m)"; [ "$arch" = x86_64 ] && narch=x64 || narch=arm64; \
    curl -fsSL "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-$narch.tar.xz" \
      | tar -xJ -C /usr/local --strip-components=1 \
 && node --version

# The API tests (api-tests/run.sh): Bruno's CLI, and Schemathesis with its own Python (it needs 3.11
# or newer; Ubuntu 22.04 has 3.10), installed through uv.
ENV BRUNO_VERSION=4.2.1 UV_VERSION=0.13.1 SCHEMATHESIS_VERSION=4.31.1 \
    UV_TOOL_DIR=/opt/uv/tools UV_TOOL_BIN_DIR=/usr/local/bin UV_PYTHON_INSTALL_DIR=/opt/uv/python
RUN npm install -g "@usebruno/cli@$BRUNO_VERSION" && bru --version \
 && curl -LsSf "https://astral.sh/uv/$UV_VERSION/install.sh" | env UV_INSTALL_DIR=/usr/local/bin UV_NO_MODIFY_PATH=1 sh \
 && uv tool install --python 3.12 "schemathesis==$SCHEMATHESIS_VERSION" && st --version \
 && chmod -R a+rX /opt/uv

# What differs per architecture is only the environment, so it comes last (BuildKit builds only the
# stage of the runner's architecture).
FROM common AS arch-amd64
# The arm64 cross toolchain, and qemu running the arm64 tests (cargo test --target aarch64-…).
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu" \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
# The runner's -L reaches only the test binary. A process a test starts (the fake ACP agent, a helper) is an
# aarch64 binary the kernel hands to binfmt's qemu without -L, so it could not find /lib/ld-linux-aarch64.so.1
# and died at once. qemu reads the same prefix from this variable, which every child inherits.
ENV QEMU_LD_PREFIX=/usr/aarch64-linux-gnu

FROM common AS arch-arm64
# Native: linked with lld like x86_64's.
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld"

FROM arch-${TARGETARCH}
