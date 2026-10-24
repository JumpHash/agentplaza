```toml
[advisory]
id = "RUSTSEC-2024-0003"
package = "h2"
date = "2024-03-11"
aliases = ["CVE-2024-27316"]
cvss = "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"
title = "h2 does not limit CONTINUATION frames"

[versions]
patched = [">= 0.3.26", ">= 0.4.3"]

[affected.functions]
"h2::codec::read_frame" = [">= 0.3.0"]
```

# h2 does not limit CONTINUATION frames

A flaw in h2's frame handling lets a peer exhaust memory with CONTINUATION frames.
