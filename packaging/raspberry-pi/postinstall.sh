#!/bin/sh
# AgentPlaza on Raspberry Pi: runs after the package is installed.
# It only prints notes and never fails, so the install always completes.

linux_post=/usr/share/agentplaza/linux-postinstall.sh
if [ -f "$linux_post" ]; then
    sh "$linux_post" "$@" || true
fi

note() {
    echo "agentplaza: $*"
}

# The package is built for one Debian architecture (arm64 or armhf). uname -m
# reports the kernel's, which is aarch64 on a 64-bit kernel even with a 32-bit
# userland, so ask dpkg first and fall back to uname -m when dpkg is missing.
arch="$(dpkg --print-architecture 2>/dev/null || true)"
if [ -z "$arch" ]; then
    arch="$(uname -m 2>/dev/null || echo unknown)"
fi
case "$arch" in
    aarch64|arm64)
        ;;
    *)
        note "agents need the 64-bit Raspberry Pi OS (this machine is $arch)"
        ;;
esac

if [ ! -e /dev/kvm ]; then
    note "KVM is missing: use Raspberry Pi OS 64-bit with its default kernel"
elif [ -n "${SUDO_USER:-}" ]; then
    if ! id -nG "$SUDO_USER" 2>/dev/null | tr ' ' '\n' | grep -qx kvm; then
        note "add $SUDO_USER to the kvm group: sudo usermod -aG kvm $SUDO_USER (then log in again)"
    fi
fi

if [ -r /proc/device-tree/model ]; then
    model="$(tr -d '\000' < /proc/device-tree/model 2>/dev/null || true)"
    note "board: ${model:-unknown}"
    case "$model" in
        *"Raspberry Pi 4"*|*"Raspberry Pi 5"*)
            ;;
        *)
            note "this package is for Raspberry Pi 4 and 5"
            ;;
    esac
fi

exit 0
