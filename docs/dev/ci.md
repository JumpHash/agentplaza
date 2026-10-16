# Continuous integration

The workflows live in `.github/workflows` and run on the organisation's self-hosted runners.

**Pull requests are tested; main only builds.** Every pull request to `main` runs the gate
(`pr.yml`). It merges through the merge queue, which runs the gate again on the exact commit that
will land. The push to `main` then builds, packages and releases that commit without testing it
again. A nightly run tests every platform, to catch what the gate's path filter let through.

**No code, no run.** `.github/ci/changes.sh` decides what a change touches, for the gate, the
release and the nightly run alike. A change with no code in it (Markdown, `docs/`, `.gitignore`,
`.gitattributes`, `.editorconfig`, `LICENSE`, `VERSION`, `.github/CODEOWNERS`) builds nothing,
tests nothing, and cuts no version or release. `docs/agents/` and `docs/api/` are the exception:
the hub compiles them in or tests against them, so they count as code.

`main` takes squash merges of pull requests only, through the merge queue. `ci-ok` is its one
required check, and changes under `.github/` need a code owner's review (CODEOWNERS).

## The gate (`pr.yml`)

| Job | Runs when | What it checks |
|---|---|---|
| `changes` | always | What the change touches (`changes.sh`) |
| `lint` | code or CI changed | `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo deny check` (advisories, licences, sources: `deny.toml`) |
| `migrations` | code or CI changed | Applied migrations unchanged, new ones numbered in order (`scripts/check-migrations.sh`) |
| `console` | the console or what it compiles from changed | The web console's type-check, build and tests (`console.yml`) |
| `linux` | code or CI changed | The Linux tests (`linux.yml`). x86_64 runs the whole workspace. arm64 runs natively on its own runners, for the packages whose code differs there (`vmm`, `vmctl`, `noded`, `vsockd`, `agent-harness`, `apz-setup`, `acpi_tables`). Nightly runs arm64's full suite on x86_64 under qemu |
| `windows`, `macos` | platform code changed | The Windows and macOS tests |
| `ci-ok` | always | Passes when every job above passed or was skipped |

"Platform code" is the platform crates (`apz-desktop*`, `vmm`, `noded`, `vsockd`, `vmctl`,
`apz-uds`, `apz-os`, `acpi-tables`), the desktop app, packaging, `Cargo.toml`/`Cargo.lock`, the
toolchain and nextest config, the platform workflows, `.github/ci/`, and any Rust file with a
`cfg(...)` on the OS, family, vendor or architecture.

A newer push to a pull request cancels its older run; merge-queue runs are never cancelled.

**Only what changed.** A pull request runs the tests of the packages it changes and of every package
that depends on them (`changes.sh` → `tests`). A change outside a package's own directory, such as
`Cargo.lock`, a root manifest, `.github/`, `sql/`, `assets/` or `apps/web`, runs every test. The
merge queue always runs every test, on the exact commit that will land.

**Compile what can't run.** Windows arm64 and Apple Silicon tests can't run on our x64 runners.
On a pull request, `cargo check` only checks that they compile. The merge queue, nightly and
releases build and link them in full.

**The light runner.** `changes`, `migrations`, `ci-ok`, the release's `version` job and nightly's
`changes` check take seconds. They run on the org's light runner (`[self-hosted, light]`, 1 vCPU),
so they never wait behind a build.

**Outside contributors.** The repository is public and its jobs run on our own machines, so:
workflows from every outside contributor wait for an owner's approval before they run; no
pull-request job gets a secret; the token is read-only; and a pull request from a fork builds in its
own cache directory on the runners (`trusted: false`), never in the one releases build from.

Clippy runs on the CI image's pinned Rust (`.github/ci/Dockerfile`), so its lints change only when
that pin does. The workspace's allowed lints are in `[workspace.lints.clippy]` in `Cargo.toml`.

## Build caches

Every build reuses the runner's own state, so a run compiles only the crates its change touched and
whatever depends on them:

- **The build directory persists** on each runner, outside the workspace, in
  `~/.cache/agentplaza-ci/target` (`agentplaza-ci-untrusted` for forks). It holds cargo's registry
  and git checkouts too.
- **mtimes.** The runners start each job from a fresh clone, so every file would look new to cargo.
  `.github/ci/build-cache.py mtimes`, run right after the checkout, gives every file the runner has
  seen with the same content the mtime it had then. Only changed files look new, as if the working
  copy had persisted.
- **npm's download cache** persists for the console's `npm ci`.
- **Not the runner fleet's sccache.** Windows and macOS clear the fleet's `RUSTC_WRAPPER`, because its
  server died mid-build. Their persistent build directory keeps unchanged crates built.
- **Prune.** `build-cache.py prune` runs at the end of each build job. It reports the build
  directory's size and the disk's free space.
  - **The limits:** the directory's budget is 80 GB or 60% of the disk, whichever is less (40 GB on
    the x86_64 Linux runners, below). The disk
    counts as short below 25 GB free or 10% of the disk, whichever is less.
  - **What it removes:** over budget or short, it removes the units no build has used for 14, then
    7, 3 and 1 days. It empties the directory only if the disk is still short.
- **Checked, incremental test builds off the reference platform.**
  - **Linux x86_64** tests the `release` build that ships, and is the only platform where the timing
    budgets are enforced.
  - **Windows, macOS and Linux arm64** test with the `checked` profile (`Cargo.toml`): release
    optimisation, compiled incrementally, with debug assertions and overflow checks in our own
    crates. A change recompiles only what it touches, even in the big crates downstream of it.
  - **The packages** build with `release` everywhere.
- **Pinned Rust**: 1.99.0 everywhere (`.github/ci/Dockerfile`, `windows.yml`, `macos.yml`). A Rust
  release does not rebuild everything on the day it ships; bumping it is a deliberate change.

Each runner keeps its own caches. The first job on a new runner compiles everything once.

**The runner fleet's storage guard.** The x86_64 Linux runners are VMs whose disks share the host's.
After each job, the fleet's guard empties the job's workspace, so every job starts from a fresh
clone; `mtimes` covers that. When the host's disk passes its limit, the guard also deletes
`~/.cache`, which holds our build directory, and every Docker image, including the CI image, on
every runner. The next builds then start cold. So the Linux build directory is held to 40 GB, and
no second copy of compiled crates (sccache) is kept beside it.

## All workflows

| Workflow | Runs on | What it does |
|---|---|---|
| `pr.yml` | pull requests to main, merge-queue entries | The gate, above |
| `release.yml` | pushes to main with code in them, or by hand | Picks the next version (`packaging/scripts/next-version.sh`), builds and packages every platform (no tests: the queue tested the commit), and publishes the GitHub Release |
| `nightly.yml` | every night after a day with code changes, or by hand | Every platform's tests: Linux x86_64 and arm64, Windows, macOS, the console |
| `linux.yml` | from the above, or by hand | Tests with nextest (`tests`) and, with `package`, builds the packages for x86_64 and arm64 and installs them in Ubuntu, Debian, Fedora and Arch containers |
| `macos.yml` | from the above, or by hand | Tests on the Intel Mac; with `package`, the Intel and Apple silicon DMGs |
| `windows.yml` | from the above, or by hand | Tests the Windows workspace; with `package`, the MSI and ZIP |
| `console.yml` | from the above | Type-checks, builds and tests the web console (`apps/web/dist`, not committed); a release saves it for the package jobs |
| `guest.yml` | from `release.yml` | Builds the agents' guest kernel and initramfs per architecture |
| `raspberry-pi.yml` | from `release.yml` | Packages the Raspberry Pi `.deb` and tarball |
| `relay.yml` | from `release.yml` | Builds the MCP relay container |
| `trainer.yml` | pushes to main that touch `trainer/`, or by hand | Runs the trainer's tests (`pytest`) |

For the stage layout and the version rules, read [`packaging/README.md`](../../packaging/README.md).
