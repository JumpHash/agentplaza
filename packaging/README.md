# Packaging AgentPlaza

Every release ships, for every platform, packages built from one **stage**: the same files laid
out the same way, which each platform's packager maps into its own format.

| Platform | Deliverables | Built by |
|---|---|---|
| Windows x64, arm64 | `AgentPlaza-<v>-windows-<arch>.msi`, `AgentPlaza-<v>-windows-<arch>.zip` | `.github/workflows/windows.yml` → `packaging/windows/build.ps1` |
| macOS Apple Silicon, Intel | `AgentPlaza-<v>-macos-<arm64\|x86_64>.dmg` | `.github/workflows/macos.yml` → `packaging/macos/build-dmg.sh` |
| Linux x86_64, arm64 | `agentplaza_<v>_<amd64\|arm64>.deb` (Ubuntu, Debian), `agentplaza-<v>-1.<x86_64\|aarch64>.rpm` (Fedora), `agentplaza-<v>-1-<x86_64\|aarch64>.pkg.tar.zst` (Arch, Arch Linux ARM), `agentplaza-<v>-linux-<x86_64\|aarch64>.tar.gz` | `.github/workflows/linux.yml` → `packaging/linux/build.sh` |
| Raspberry Pi 4/5 (Raspberry Pi OS 64-bit) | `agentplaza_<v>_arm64-raspberrypi.deb`, `agentplaza-<v>-raspberrypi-arm64.tar.gz` | `.github/workflows/raspberry-pi.yml` → `packaging/raspberry-pi/build.sh` |

`.github/workflows/release.yml` runs on every push to `main` except one that touches only `docs/`,
Markdown files or `VERSION`. It builds the guest (`guest.yml`) and the relay (`relay.yml`),
picks the next version (semantic versioning, see below), runs the four platform workflows with
packaging on, and publishes a GitHub Release `v<version>` (tag + changelog + all deliverables with
`SHA256SUMS`), then writes the version back to `main`'s `VERSION` file.

## The stage (`packaging/scripts/stage.sh`)

```
stage.sh <target-triple> <cargo-out-dir> <stage-dir> <version>
```

`<cargo-out-dir>` holds the release binaries (`target/<triple>/release`). It writes:

```
<stage>/bin/        apid, apz-link, agentplaza (the CLI), agentplaza-desktop  (every platform)
                    Linux, Apple Silicon: noded, vsockd, apz-vmm, vmctl
                    Windows x64: noded, vsockd, vmctl
                    (+ .exe on Windows; Windows arm64 and Intel Macs: none, hub/desktop/link only)
                    every bundle: mcp_relay-x86_64, mcp_relay-aarch64 (the container relay, static musl)
<stage>/share/agentplaza/
    console/        the web console (apps/web/dist), served by apid
    roles/initial-roles.json
    prompts/        the roles' prompts (docs/codex-draft/roles)
    icons/          agentplaza-<n>.png, agentplaza.ico, agentplaza.icns
    guest/<arch>/       vmlinux, initrd.gz, guest.json — the agents' VM kernel and initrd, found by noded
                        (stages with the VM node only)
    VERSION         the version, one line
```

The stage refuses a console `dist` without the wasm engine and the IDE's wasm: rebuild it with
`apps/web/build.mjs` first.

The guest comes from `scripts/mk-guest.sh` (built per architecture by `.github/workflows/guest.yml`,
cached) and is read from `target/guest` (or `APZ_GUEST_DIR`). The relay binaries are read from
`APZ_RELAY_DIR` (the relay job's artifact) or from the dev tree's musl builds.

At run time `apz_os::dirs::resources()` finds `share/agentplaza` next to the binaries
(`<bin>/../share/agentplaza`; inside a macOS bundle `Contents/Resources/share/agentplaza`), so
`apid` and the desktop app need no paths on the command line.

Windows x64 runs agents through the Windows Hypervisor Platform backend (the optional "Windows
Hypervisor Platform" feature must be enabled; see `docs/windows-whp.md`). Windows arm64 runs the
hub, the desktop app and the node link.

The console includes the 3D engine: `console/apz_engine_web-<hash>_bg.wasm` (with `.br`/`.gz`
siblings, served as `application/wasm`) and its glue and layout worker. CI builds it with
`APZ_REQUIRE_ENGINE=1`, and `stage.sh` refuses a `dist` without the engine. To run it after
install: start the hub (`systemctl --user start agentplaza-hub` on Linux, or `apid` anywhere) and
open `http://127.0.0.1:7700`; the desktop app starts against that same local hub.

## Where things go

| | Linux (deb/rpm/pacman) | macOS (`AgentPlaza.app`) | Windows (MSI) |
|---|---|---|---|
| binaries | `/usr/bin` | `Contents/MacOS` (`agentplaza-desktop` is the app) | `%ProgramFiles%\AgentPlaza\bin` (on `PATH`) |
| share | `/usr/share/agentplaza` | `Contents/Resources/share/agentplaza` | `%ProgramFiles%\AgentPlaza\share\agentplaza` |
| launcher | `/usr/share/applications/agentplaza.desktop`, icons in `/usr/share/icons/hicolor` | the bundle | Start-menu shortcut |
| services | systemd user units `agentplaza-hub.service`, `agentplaza-node.service` (`/usr/lib/systemd/user`) | started by the desktop app | started by the desktop app |

## Versions

Every push to `main` that changes code is a release `v<major>.<minor>.<patch>`, tagged on the merged commit; the
repository's `VERSION` file holds the last released version (`release.yml` writes it back to
`main` after each release). The version is chosen by `packaging/scripts/next-version.sh`:

- **Automatic** (the default): the last release plus one, each place rolling over past 9 into the
  next one: 0.0.1 → 0.0.2 … 0.0.9 → 0.1.0 … 0.1.9 → 0.2.0 … 0.9.9 → 1.0.0 … 9.9.9 → 10.0.0.
  The first release is 0.0.1.
- **Set by a pull request**: change `VERSION` in the PR to any version above the last release
  (say `1.0.0`); when it is merged, that is the release. A value that is not above the last
  release is ignored and the automatic one is used.
- **By hand**: run `release.yml` from the Actions tab with a `version` input.

The release page opens with the version and how it was chosen.

The binaries report the version (`--version`) from the shipped `share/agentplaza/VERSION`
(`apz_os::version()`; `CARGO_PKG_VERSION` in a development build). It is read at run time, not
compiled in, so a release changes no compiled code and CI's caches stay warm.

## Signing

Signing is optional and switches on with secrets. Without them the DMG is ad-hoc signed (the VMM
carries the `com.apple.security.hypervisor` entitlement it needs) and the MSI is unsigned. Setting
`APPLE_SIGNING_IDENTITY` and `APPLE_NOTARY_PROFILE` (`macos.yml`) adds Developer ID signing and
notarization; `WINDOWS_CERT_PFX_BASE64` and `WINDOWS_CERT_PASSWORD` (`windows.yml`) add Authenticode
signing of the exes and the MSI. See `packaging/macos/README.md` and `packaging/windows/README.md`.
