#!/bin/bash
exec "/mnt/fast/apz-accept/pin/apz-vmm" "$@" 2> >(while IFS= read -r line; do printf '%s %s\n' "$$" "$line" >> "/mnt/fast/apz-accept/run-server/jail-proc-api-childlogs/child-stderr.log"; printf '%s\n' "$line" >&2; done)
