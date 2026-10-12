# AgentPlaza for macOS

`build-dmg.sh` turns an arm64 stage and an x86_64 stage into one universal
`AgentPlaza.app`, and wraps it in `AgentPlaza-<version>-macos-universal.dmg`.

    build-dmg.sh <stage-arm64> <stage-x86_64> <version> <out-dir>

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
