#!/bin/sh
# Installs (or, with --remove, removes) a systemd user timer that runs
# scripts/target-gc.sh daily for this checkout.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
name=apz-target-gc

if [ "${1:-}" = "--remove" ]; then
    systemctl --user disable --now "$name.timer" 2>/dev/null || true
    rm -f "$units/$name.service" "$units/$name.timer"
    systemctl --user daemon-reload
    echo "removed $name.timer"
    exit 0
fi

mkdir -p "$units"
cat > "$units/$name.service" <<EOF
[Unit]
Description=Trim the AgentPlaza build output ($root/target)

[Service]
Type=oneshot
Nice=19
IOSchedulingClass=idle
Environment=PATH=$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/bin/sh $root/scripts/target-gc.sh
EOF
cat > "$units/$name.timer" <<EOF
[Unit]
Description=Trim the AgentPlaza build output daily

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=30min

[Install]
WantedBy=timers.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now "$name.timer"
systemctl --user list-timers "$name.timer" --no-pager
