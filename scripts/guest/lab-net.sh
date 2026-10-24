#!/bin/sh
# Applied at boot by the lab image (PLAN 22.6, round 2): the segment's hosts file and this VM's interface, as the
# topology runner handed them over (crates/apz-topology/src/run.rs, hand_guest_net). Without a config the VM has no
# segment, and that is not an error: a single-VM topology has none.
set -eu
cfg=/etc/apz/net
[ -f "$cfg/hosts" ] || exit 0
cat "$cfg/hosts" >> /etc/hosts
sh "$cfg/net.sh"
