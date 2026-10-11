#!/bin/bash
exec "/tmp/apz-pin4/apz-vmm" "$@" 2> >(while IFS= read -r line; do printf '%s %s\n' "$$" "$line" >> "/tmp/apz-matrix-QdNj1n/jail-proc-api-childlogs/child-stderr.log"; printf '%s\n' "$line" >&2; done)
