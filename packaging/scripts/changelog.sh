#!/usr/bin/env bash
# Writes a release's notes (Markdown) to stdout: the downloads for every platform and the changes
# since the previous tag, grouped by the part of the system they touch (the commit subject's
# "<scope>:" prefix).
#   changelog.sh <version> <previous-tag or ""> <owner/repo> [auto|file|manual]
# The last argument says how the version was chosen (next-version.sh's `source`); the notes open
# with it.
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$1 previous=$2 repo=$3 source=${4:-auto}
dl="https://github.com/$repo/releases/download/v$version"

case $source in
  file) how='set by the merged change (the `VERSION` file)' ;;
  manual) how="set by hand when the release was run" ;;
  *) how="numbered automatically${previous:+, the next after $previous}" ;;
esac
echo "**Version $version**, $how."
echo

cat <<EOF
## Downloads

| Platform | Package |
|---|---|
| **Windows** x64 | [MSI installer]($dl/AgentPlaza-$version-windows-x64.msi) · [zip]($dl/AgentPlaza-$version-windows-x64.zip) |
| **Windows** arm64 | [MSI installer]($dl/AgentPlaza-$version-windows-arm64.msi) · [zip]($dl/AgentPlaza-$version-windows-arm64.zip) |
| **macOS** Apple Silicon | [DMG]($dl/AgentPlaza-$version-macos-arm64.dmg) |
| **macOS** Intel | [DMG]($dl/AgentPlaza-$version-macos-x86_64.dmg) |
| **Ubuntu / Debian** x86_64 | [.deb]($dl/agentplaza_${version}_amd64.deb) |
| **Ubuntu / Debian** arm64 | [.deb]($dl/agentplaza_${version}_arm64.deb) |
| **Fedora** x86_64 | [.rpm]($dl/agentplaza-$version-1.x86_64.rpm) |
| **Fedora** aarch64 | [.rpm]($dl/agentplaza-$version-1.aarch64.rpm) |
| **Arch Linux** x86_64 | [.pkg.tar.zst]($dl/agentplaza-$version-1-x86_64.pkg.tar.zst) |
| **Arch Linux ARM** aarch64 | [.pkg.tar.zst]($dl/agentplaza-$version-1-aarch64.pkg.tar.zst) |
| **Linux** AppImage (any distribution: download, make executable, double-click) | [x86_64]($dl/AgentPlaza-$version-linux-x86_64.AppImage) · [aarch64]($dl/AgentPlaza-$version-linux-aarch64.AppImage) |
| **Linux** tarball | [x86_64]($dl/agentplaza-$version-linux-x86_64.tar.gz) · [aarch64]($dl/agentplaza-$version-linux-aarch64.tar.gz) |
| **Raspberry Pi 4/5** (Raspberry Pi OS 64-bit) | [.deb]($dl/agentplaza_${version}_arm64-raspberrypi.deb) · [AppImage]($dl/AgentPlaza-$version-linux-aarch64.AppImage) · [tarball]($dl/agentplaza-$version-raspberrypi-arm64.tar.gz) |

Checksums: [SHA256SUMS]($dl/SHA256SUMS). Agents run in microVMs on Linux (KVM, x86_64 and arm64, Raspberry Pi 4/5 included) and Apple Silicon Macs; Windows and Intel Macs run the hub and the desktop app and use nodes elsewhere.

EOF

if [ -z "$previous" ]; then
  echo "## Changes"
  echo
  echo "The first release of AgentPlaza: the hub, nodes and their microVMs, the web console, the desktop app and the model gateway. What went into it, by part:"
  echo
  git log --no-merges --format='%s' | sed -n 's/^\([A-Za-z0-9_./ -]\{1,40\}\): .*/\1/p' | sort | uniq -c | sort -rn | head -25 \
    | awk '{n=$1; $1=""; sub(/^ /,""); printf "- **%s**: %d changes\n", $0, n}'
  exit 0
fi

echo "## Changes since $previous"
echo
# "<scope>: subject" → grouped by scope; anything else under "Other".
git log --no-merges --reverse --format='%h%x09%s' "$previous..HEAD" | awk -F '\t' -v repo="$repo" '
  {
    sha = $1; subject = $2; scope = "Other"
    if (match(subject, /^[A-Za-z0-9_.\/ ,-]{1,40}!?:[ ]/)) {
      scope = substr(subject, 1, RLENGTH - 2); sub(/!$/, "", scope); subject = substr(subject, RLENGTH + 1)
    }
    if (!(scope in seen)) { seen[scope] = 1; order[++n] = scope }
    lines[scope] = lines[scope] sprintf("- %s ([%s](https://github.com/%s/commit/%s))\n", subject, sha, repo, sha)
  }
  END { for (i = 1; i <= n; i++) printf "### %s\n\n%s\n", order[i], lines[order[i]] }'
