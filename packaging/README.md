# Packaging AgentPlaza

Every release ships, for every platform, packages built from one **stage**: the same files laid
out the same way, which each platform's packager maps into its own format.

| Platform | Deliverables | Built by |
|---|---|---|
| Windows x64, arm64 | `AgentPlaza-<v>-windows-<arch>.msi`, `AgentPlaza-<v>-windows-<arch>.zip` | `.github/workflows/windows.yml` → `packaging/windows/build.ps1` |
| macOS (Apple Silicon + Intel, universal) | `AgentPlaza-<v>-macos-universal.dmg` | `.github/workflows/macos.yml` → `packaging/macos/build-dmg.sh` |
| Linux x86_64, arm64 | `agentplaza_<v>_<amd64\|arm64>.deb` (Ubuntu, Debian), `agentplaza-<v>-1.<x86_64\|aarch64>.rpm` (Fedora), `agentplaza-<v>-1-<x86_64\|aarch64>.pkg.tar.zst` (Arch, Arch Linux ARM), `agentplaza-<v>-linux-<x86_64\|aarch64>.tar.gz` | `.github/workflows/linux.yml` → `packaging/linux/build.sh` |
| Raspberry Pi 4/5 (Raspberry Pi OS 64-bit) | `agentplaza_<v>_arm64-raspberrypi.deb`, `agentplaza-<v>-raspberrypi-arm64.tar.gz` | `.github/workflows/raspberry-pi.yml` → `packaging/raspberry-pi/build.sh` |

`.github/workflows/release.yml` runs on every push to `main`: it picks the next version
(semantic versioning, see below), runs the four platform workflows with packaging on, and
publishes a GitHub Release `v<version>` (tag + changelog + all deliverables).

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
<stage>/share/agentplaza/
    console/        the web console (apps/web/dist), served by apid
    roles/initial-roles.json
    prompts/        the roles' prompts (docs/codex-draft/roles)
    icons/          agentplaza-<n>.png, agentplaza.ico, agentplaza.icns
    guest/<arch>/       vmlinux, initrd.gz, guest.json — the agents' VM kernel and initrd, found by noded
    VERSION         the version, one line
```

The guest comes from `scripts/mk-guest.sh` (built per architecture by `.github/workflows/guest.yml`,
cached) and is read from `target/guest` (or `APZ_GUEST_DIR`).

At run time `apz_os::dirs::resources()` finds `share/agentplaza` next to the binaries
(`<bin>/../share/agentplaza`; inside a macOS bundle `Contents/Resources/share/agentplaza`), so
`apid` and the desktop app need no paths on the command line.

Windows x64 runs agents through the Windows Hypervisor Platform backend (the optional "Windows
Hypervisor Platform" feature must be enabled; see `docs/windows-whp.md`). Windows arm64 runs the
hub, the desktop app and the node link.

## Where things go

| | Linux (deb/rpm/pacman) | macOS (`AgentPlaza.app`) | Windows (MSI) |
|---|---|---|---|
| binaries | `/usr/bin` | `Contents/MacOS` (`agentplaza-desktop` is the app) | `%ProgramFiles%\AgentPlaza\bin` (on `PATH`) |
| share | `/usr/share/agentplaza` | `Contents/Resources/share/agentplaza` | `%ProgramFiles%\AgentPlaza\share\agentplaza` |
| launcher | `/usr/share/applications/agentplaza.desktop`, icons in `/usr/share/icons/hicolor` | the bundle | Start-menu shortcut |
| services | systemd user units `agentplaza-hub.service`, `agentplaza-node.service` (`/usr/lib/systemd/user`) | started by the desktop app | started by the desktop app |

## Versions

Semantic versioning, from the commits since the last `v*` tag (`release.yml`):

- **major**: a commit marks a breaking change (`BREAKING CHANGE:` in its body, or `<scope>!:` in
  its subject); while the major version is 0 this bumps the minor version instead.
- **minor**: a commit carries the trailer `Release: minor`.
- **patch**: everything else.
- `Release: major|minor|patch` in a commit forces that bump; a manual run of `release.yml` can too.

The binaries report the version (`--version`) from `APZ_VERSION`, set at build time
(`apz_os::VERSION`; `CARGO_PKG_VERSION` in a local build).

## Signing

Not yet: the DMG is ad-hoc signed (the VMM carries the `com.apple.security.hypervisor`
entitlement it needs), the MSI is unsigned. Developer ID notarization and an Authenticode
certificate are wired in by adding their secrets (see the workflows' `sign` steps).
