# Visual parity

The parity harness shows whether the web console and the desktop app look the same. For each built-in
theme, each mode (light, dark) and each screen in `screens.json`, it captures the web console in headless
Chromium and the desktop app through its own snapshot (`APZ_SNAPSHOT`), then compares the two images
with a perceptual diff.

## Files

- `shots.mjs`: the driver. It signs in to the hub, captures each combination, diffs it and writes the report.
- `diff.mjs`: the perceptual diff. Pixels are compared as YIQ colour distances (after pixelmatch). A pixel
  that differs only on an anti-aliased edge in either image is not counted. Images of different sizes are
  compared at their smaller common size, nearest-neighbour resampled.
- `png.mjs`: a dependency-free PNG codec (8-bit RGB and RGBA, all filters, non-interlaced).
- `screens.json`: the screens, with the web hash route, the desktop `APZ_SCREEN` label and the wait.
- `diff.test.mjs`: unit tests for the diff and the codec.

## What it checks

Each screen is compared at 1360 by 880 pixels, DPR 1. The web console gets the look through the device
override in localStorage (`apz.appearance.device`). The desktop app gets it through `APZ_APPEARANCE`, which
is applied at start-up and never saved. A screen fails when its mismatch ratio is above `--max-ratio`
(default 0.08). Text rendering differs between the two engines, so a small ratio on text-heavy screens is
expected; a large ratio means a real difference in colour, layout or theme.

## Running it locally

Use the hub that is already running. Never start a second hub (one hub rule). The owner token is in
`<state>/owner-token`.

```
node scripts/parity/shots.mjs --hub http://127.0.0.1:7700 --token-file <state>/owner-token \
  --out <scratch>/parity --themes loom-clay,graphite-cobalt --modes light,dark --screens overview,agents
```

Requirements: Node 22 or later, Chromium (`CHROMIUM` overrides `/usr/bin/chromium`), a built desktop binary
(`target/release/agentplaza-desktop` or `target/debug/...`, or `--binary`), and `xvfb-run` on a machine with
no display. Captures run one at a time. The desktop capture is the app's own snapshot, so it never grabs the
owner's screen. Keep `--out` outside the repository.

The unit tests need no hub and no browser:

```
node --test scripts/parity/diff.test.mjs
```

## Reading the report

`report.md` has one row per theme, mode and screen, with the mismatched pixels, the ratio and the result.
`report.json` has the same rows with the sizes and any error. For each row the out directory holds
`<theme>-<mode>-<screen>-web.png`, `-desktop.png` and `-diff.png`. In the diff image, red pixels are
mismatches, yellow pixels are anti-aliased differences that were ignored, and the rest is a faint grey of
the web capture. Start with the diff image of the worst row.

The exit code is 1 when any row fails or errors.

## CI

`.github/workflows/parity.yml` runs by hand only (`workflow_dispatch`, with optional `themes` and `screens`
inputs). It builds the web console and the desktop app in the CI image, starts a hub with `--dev-models` in
a state directory under the workspace, runs the harness under `xvfb-run`, and uploads the PNGs and the
report as the `parity-report` artifact. The state directory holds the owner token, so the job deletes it
before the upload.
