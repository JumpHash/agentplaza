#!/bin/bash
exec "/mnt/fast/AgentPlaza/target/debug/apz-vmm" "$@" 2> >(while IFS= read -r line; do printf '%s %s\n' "$$" "$line" >> "/tmp/apz-acceptance-server-zktac4h5/jail-proc-api-childlogs/child-stderr.log"; printf '%s\n' "$line" >&2; done)
