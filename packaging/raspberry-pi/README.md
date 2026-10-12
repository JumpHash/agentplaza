# AgentPlaza for Raspberry Pi 4 and 5

A Debian package for arm64 boards: Raspberry Pi 4 or Pi 5 running Raspberry Pi OS 64-bit (bookworm), with 4 GB of RAM or more. Agents run as KVM virtual machines, so the board needs the default kernel that ships with that OS.

## Install

    sudo apt install ./agentplaza_<v>_arm64-raspberrypi.deb
    sudo usermod -aG kvm "$USER"        # then log out and back in
    systemctl --user enable --now agentplaza-hub agentplaza-node

The install script prints a note for anything missing: the 64-bit kernel, `/dev/kvm`, or the `kvm` group.

## Tune the node

Edit `/etc/default/agentplaza-node` (the arguments for `noded`), then restart it:

    systemctl --user restart agentplaza-node

A Pi 4 with 4 GB runs about 4 agents at 512 MiB each; a Pi 5 with 8 GB about 10. The file has the flags and the memory notes.

## Build

    packaging/raspberry-pi/build.sh <stage-dir> <version> <out-dir>

Needs bash and `fpm`. Writes `agentplaza_<v>_arm64-raspberrypi.deb` and `agentplaza-<v>-raspberrypi-arm64.tar.gz` (the same root tree) to `<out-dir>`.
