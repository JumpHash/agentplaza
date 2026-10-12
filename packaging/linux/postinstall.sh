#!/bin/sh
# AgentPlaza post-install: refresh caches if the tools exist, hint about KVM.
# Never fails.

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications >/dev/null 2>&1 || true
fi
if [ ! -e /dev/kvm ]; then
  echo "AgentPlaza: /dev/kvm not found; agents need KVM. Enable virtualization in firmware (VT-x or AMD-V); on a Raspberry Pi use the 64-bit OS."
fi

exit 0
