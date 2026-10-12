# AgentPlaza for macOS

`build-dmg.sh` turns one architecture's stage into `AgentPlaza.app` and wraps it
in `AgentPlaza-<version>-macos-<arch>.dmg`: `arm64` for Apple Silicon, `x86_64`
for Intel. Every release ships both.

    build-dmg.sh <stage> <arm64|x86_64> <version> <out-dir>

## Signing

- Today the app is ad-hoc signed (no Apple ID yet). Gatekeeper blocks a plain
  double-click on the first launch: right-click AgentPlaza, then choose Open.
- With a Developer ID, set `APPLE_SIGNING_IDENTITY`. The script then signs with
  a timestamp.
- Also set `APPLE_NOTARY_PROFILE` (a `notarytool` keychain profile) to notarize
  and staple the DMG. It needs `APPLE_SIGNING_IDENTITY`.

## What runs where

- Apple Silicon runs agents. The VMM (`apz-vmm`) uses Hypervisor.framework and
  carries the `com.apple.security.hypervisor` entitlement.
- Intel Macs run the hub and the desktop, and connect to nodes elsewhere.
