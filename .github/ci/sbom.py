#!/usr/bin/env python3
"""sbom.py VERSION < metadata.json > sbom.cdx.json: a CycloneDX 1.5 SBOM of a release from `cargo metadata
--format-version 1 --locked` (the workspace's crates and every crate they depend on, with versions, sources
and licences). The release ships it next to its packages and their SHA256SUMS."""
import json
import sys
import uuid
from datetime import datetime, timezone


def purl(pkg):
    source = pkg.get("source") or ""
    if source.startswith("registry+"):
        return f"pkg:cargo/{pkg['name']}@{pkg['version']}"
    if source.startswith("git+"):
        return f"pkg:cargo/{pkg['name']}@{pkg['version']}?vcs_url={source[len('git+'):]}"
    return f"pkg:cargo/{pkg['name']}@{pkg['version']}?local=true"


def component(pkg, kind):
    entry = {
        "type": kind,
        "bom-ref": pkg["id"],
        "name": pkg["name"],
        "version": pkg["version"],
        "purl": purl(pkg),
    }
    if pkg.get("license"):
        entry["licenses"] = [{"expression": pkg["license"]}]
    if pkg.get("repository"):
        entry["externalReferences"] = [{"type": "vcs", "url": pkg["repository"]}]
    return entry


def main():
    version = sys.argv[1]
    meta = json.load(sys.stdin)
    members = set(meta["workspace_members"])
    packages = sorted(meta["packages"], key=lambda p: (p["name"], p["version"]))
    resolve = meta.get("resolve") or {"nodes": []}
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "component": {"type": "application", "bom-ref": "agentplaza", "name": "agentplaza", "version": version},
        },
        "components": [component(p, "application" if p["id"] in members else "library") for p in packages],
        "dependencies": [
            {"ref": node["id"], "dependsOn": sorted({dep["pkg"] for dep in node.get("deps", [])})}
            for node in resolve["nodes"]
        ],
    }
    json.dump(sbom, sys.stdout, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
