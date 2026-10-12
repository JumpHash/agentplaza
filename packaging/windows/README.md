# AgentPlaza for Windows

Two artifacts per architecture (x64, arm64):

- `AgentPlaza-<version>-windows-<arch>.msi`: per-machine installer. Installs to
  `Program Files\AgentPlaza`, adds `bin` to the system PATH, and creates a Start-menu shortcut.
- `AgentPlaza-<version>-windows-<arch>.zip`: the same staged tree, unpacked.

Build locally with `build.ps1` (PowerShell 7, WiX Toolset 5 is installed automatically):

    pwsh -File build.ps1 -Stage <stage> -Version 0.1.0 -Arch x64 -Out <out>

## Status

- Unsigned today. SmartScreen will warn: choose **More info**, then **Run anyway**.
  If the signing secrets `WINDOWS_CERT_PFX_BASE64` and `WINDOWS_CERT_PASSWORD` are set,
  the exes and the MSI are Authenticode-signed instead.
- Windows runs the hub, the desktop app and the node link.
- Agents run on Linux and macOS nodes until the Windows Hypervisor Platform backend lands.
