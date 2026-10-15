# Continuous integration

The workflows live in `.github/workflows`. Workflows run on main only, never on pull requests or other branches. Most platform builds run on the organisation's self-hosted runners.

`main` is protected: every change reaches it through a pull request (no direct pushes, no force pushes, no deletion, administrators included). A pull request runs no workflow; merging it is the push to `main` that tests, packages and releases it.

| Workflow | Runs on | What it does |
|---|---|---|
| `linux.yml` | main (from `release.yml`), or by hand | Tests with nextest and builds the packages for x86_64 and arm64 (arm64 tests run under qemu); installs and runs them in Ubuntu, Debian, Fedora and Arch containers |
| `macos.yml` | by hand, or from `release.yml` | Tests on Apple silicon and builds the Intel and Apple silicon binaries and the DMGs |
| `windows.yml` | by hand, or from `release.yml` | Tests the Windows workspace and builds the MSI and ZIP for x64 and arm64 |
| `guest.yml` | from `release.yml` | Builds the agents' guest kernel and initramfs per architecture |
| `raspberry-pi.yml` | from `release.yml` | Packages the Raspberry Pi `.deb` and tarball |
| `relay.yml` | from `release.yml` | Builds the MCP relay container |
| `console.yml` | from `release.yml` | Builds and tests the web console (`apps/web/dist`, not committed) once per commit; the package jobs restore it from the cache |
| `migrations.yml` | every push to main | Checks that applied database migrations are not edited (`scripts/check-migrations.sh`) |
| `trainer.yml` | pushes to main that touch `trainer/`, or by hand | Runs the trainer's tests (`pytest`) |
| `release.yml` | every push to main, or by hand | Picks the next version (`packaging/scripts/next-version.sh`), runs the platform workflows, and publishes the GitHub Release with its changelog and deliverables |

For the stage layout and the version rules, read [`packaging/README.md`](../../packaging/README.md).
