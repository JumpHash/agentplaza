# Continuous integration

The workflows live in `.github/workflows` and run on the organisation's self-hosted runners.

**Pull requests are tested; main only builds.** Every pull request to `main` runs the gate
(`pr.yml`). It merges through the merge queue, which runs the gate again on the exact commit that
will land. The push to `main` then builds, packages and releases that commit without testing it
again. A nightly run tests every platform, to catch what the gate's path filter let through.

**No code, no run.** `.github/ci/changes.sh` decides what a change touches, for the gate, the
release and the nightly run alike. A change with no code in it (Markdown, `docs/`, `.gitignore`,
`.gitattributes`, `.editorconfig`, `LICENSE`, `VERSION`, `.github/CODEOWNERS`) builds nothing,
tests nothing, and cuts no version or release.

`main` takes squash merges of pull requests only, through the merge queue. `ci-ok` is its one
required check, and changes under `.github/` need a code owner's review (CODEOWNERS).

## The gate (`pr.yml`)

| Job | Runs when | What it checks |
|---|---|---|
| `changes` | always | What the change touches (`changes.sh`) |
| `lint` | code or CI changed | `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo deny check` (advisories, licences, sources: `deny.toml`) |
| `migrations` | code or CI changed | Applied migrations unchanged, new ones numbered in order (`scripts/check-migrations.sh`) |
| `console` | the console or what it compiles from changed | The web console's type-check, build and tests (`console.yml`) |
| `linux` | code or CI changed | The Linux tests (`linux.yml`): x86_64, and arm64 under qemu when platform code changed |
| `windows`, `macos` | platform code changed | The Windows and macOS tests |
| `api` | code or CI changed | The hub API against a scratch hub (`api-tests/run.sh`): the Bruno collection (a failure blocks the merge), timings, and Schemathesis fuzzing from the spec (reported only) |
| `ci-ok` | always | Passes when every job above passed or was skipped |

"Platform code" is the platform crates (`apz-desktop*`, `vmm`, `noded`, `vsockd`, `vmctl`,
`apz-uds`, `apz-os`, `acpi-tables`), the desktop app, packaging, `Cargo.toml`/`Cargo.lock`, the
toolchain and nextest config, the platform workflows, `.github/ci/`, and any Rust file with a
`cfg(...)` on the OS, family, vendor or architecture.

A newer push to a pull request cancels its older run; merge-queue runs are never cancelled.

**Outside contributors.** The repository is public and its jobs run on our own machines, so:
workflows from every outside contributor wait for an owner's approval before they run; no
pull-request job gets a secret; the token is read-only; and a pull request from a fork builds in its
own cache directory on the runners (`trusted: false`), never in the one releases build from.

Clippy runs on the CI image's pinned Rust (`.github/ci/Dockerfile`), so its lints change only when
that pin does. The workspace's allowed lints are in `[workspace.lints.clippy]` in `Cargo.toml`.

## The hub API report

`api-tests/run.sh` starts a scratch hub (empty config, no database, no node daemon, no git host) and runs:
- the Bruno collection in `api-tests/bruno`, one folder per area;
- timings: every parameterless GET in `docs/api/openapi.json`, 20 times each;
- Schemathesis, with requests generated from the spec, looking for 5xx answers and dropped connections.

Routes marked `x-outbound` in the spec reach the internet, so the timings and the fuzzing skip them.

The gate keeps the results as the `api-results` artifact. `api-report.yml` then runs from main, builds
the page with main's `api-tests/report/build.mjs`, and publishes it to GitHub Pages:
- `api/pr/<n>/` for a pull request, linked in a comment on it;
- `api/main/` for the merge queue's run, which is the baseline the timings compare against.

`api-report-cleanup.yml` removes a pull request's report when it closes. The publishing never runs or
serves anything from the pull request: the artifact holds only data, and the builder escapes it and
scrubs tokens and secrets from it.

Locally: `BRU=bru ST=st api-tests/run.sh target/debug/apid api-out`, with `APZ_API_FUZZ=0` to skip the
fuzzing. Then open `api-out/site/index.html`.

## All workflows

| Workflow | Runs on | What it does |
|---|---|---|
| `pr.yml` | pull requests to main, merge-queue entries | The gate, above |
| `release.yml` | pushes to main with code in them, or by hand | Picks the next version (`packaging/scripts/next-version.sh`), builds and packages every platform (no tests: the queue tested the commit), and publishes the GitHub Release |
| `api-report.yml` | after each `pr.yml` run (from main) | Builds the hub API report from the run's results and publishes it to Pages; links it on the pull request |
| `api-report-cleanup.yml` | a pull request closing | Removes its API report from Pages |
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
