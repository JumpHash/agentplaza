#!/usr/bin/env python3
"""Turns the coverage reports .github/ci/coverage.sh writes into one data file, coverage.json.

  coverage.py <data-dir> <out.json> [--root DIR]

The data dir holds coverage.sh's output: metadata.json (cargo metadata, no dependencies), the lcov
reports rust-unit.lcov, rust-integration.lcov, rust-all.lcov (llvm-cov), hub-api.lcov (apid, instrumented,
under the Bruno run) and web.lcov (c8), and api-run/ (the Bruno run: bruno.json, site/summary.json).

Per file, each test type's line and function counts are kept as its report gives them. "all" is the
union of the test types in UNION: a line counts once, as covered when any of them covers it. Files
belong to exactly one package, so the groups (services, apps, crates, the whole) add up their files.

Groups: services are the daemons (SERVICES), apps the desktop app, the command line and the web console
(APPS), crates every package under crates/ and tools/ (the services among them too). The hub API's spec coverage is the
operations of docs/api/openapi.json that a Bruno request reached, matched the way
api-tests/report/build.mjs matches them, per tag.

Only data goes out (paths, names and counts); check-comments.py checks it again before it posts anything.
"""

import argparse
import json
import os
import posixpath
import sys
from collections import defaultdict
from urllib.parse import unquote, urlsplit

FORMAT = 1
# The test types, with their labels. "rust-all" is the Rust tests run together (its own report).
TYPES = [
    ("rust-unit", "Rust unit tests"),
    ("rust-integration", "Rust integration tests"),
    ("rust-all", "Rust unit and integration together"),
    ("hub-api", "Hub API (Bruno), code"),
    ("web", "Web console"),
]
UNION = ["rust-unit", "rust-integration", "hub-api", "web"]
REPORTS = {
    "rust-unit": "rust-unit.lcov",
    "rust-integration": "rust-integration.lcov",
    "rust-all": "rust-all.lcov",
    "hub-api": "hub-api.lcov",
    "web": "web.lcov",
}
# The daemons, by package: what runs on the hub, a node, a guest, a relay or a workstation.
SERVICES = [
    ("apid", "Hub (apid)"),
    ("noded", "Node daemon (noded)"),
    ("vsockd", "Orchestration (vsockd)"),
    ("apz-link", "Node link (apz-link)"),
    ("dispatchd", "Fleet dispatcher (dispatchd)"),
    ("vmm", "VM monitor (vmm)"),
    ("mcp-relay", "MCP relay (mcp-relay)"),
    ("agent-harness", "Guest agent harness (agent-harness)"),
    ("apz-workstation", "Workstation daemon (apz-workstation)"),
    ("weft", "Version control (weft)"),
]
# The apps: the desktop app and the command line are Rust packages, the web console is the TypeScript
# package under apps/web (its sources are apps/web/src, the only files its tests measure).
APPS = [
    ("agentplaza-desktop", "Desktop app"),
    ("agentplaza-cli", "Command line"),
    ("@agentplaza/web", "Web console"),
]
WEB_PACKAGE = ("@agentplaza/web", "apps/web/src")
# Test code is not measured: tests directories, benches, examples, test modules and build scripts.
NOT_CODE_DIRS = {"tests", "test", "benches", "examples"}
NOT_CODE_FILES = {"tests.rs", "build.rs"}


def relative(path, root, base=""):
    """`path` (absolute, or relative to `base` under root) as a path relative to root, or None outside it."""
    full = path if os.path.isabs(path) else os.path.join(root, base, path)
    rel = os.path.relpath(os.path.normpath(full), root)
    if rel == os.curdir or rel == os.pardir or rel.startswith(os.pardir + os.sep):
        return None
    return rel.replace(os.sep, "/")


def packages(root, metadata_path):
    """[(name, dir)] of every workspace package (dir relative to root), the web console's sources included."""
    with open(metadata_path, encoding="utf-8") as f:
        meta = json.load(f)
    found = []
    for pkg in meta.get("packages", []):
        manifest = pkg.get("manifest_path", "")
        if not manifest:
            continue
        found.append((pkg["name"], relative(os.path.dirname(manifest), root)))
    found.append(WEB_PACKAGE)
    # The longest directory first: a file belongs to the package whose directory is nearest.
    return sorted(((name, d) for name, d in found if d), key=lambda p: -len(p[1]))


def package_of(rel, pkgs):
    for name, d in pkgs:
        if rel == d or rel.startswith(d + "/"):
            return name
    return None


def is_code(rel):
    parts = rel.split("/")
    return not (set(parts[:-1]) & NOT_CODE_DIRS or parts[-1] in NOT_CODE_FILES or parts[-1].endswith("_tests.rs"))


def read_lcov(path, root, base=""):
    """{relative path: {"lines": {line: hits}, "functions": {(name, line): hits}}} for the files under root."""
    files = defaultdict(lambda: {"lines": {}, "functions": {}})
    current = None
    fn_lines = {}
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.strip()
            if line.startswith("SF:"):
                rel = relative(line[3:], root, base)
                current = files[rel] if rel is not None else None
                fn_lines = {}
            elif current is None:
                continue
            elif line.startswith("FN:"):
                ln, _, name = line[3:].partition(",")
                fn_lines[name] = int(ln)
                current["functions"].setdefault((name, int(ln)), 0)
            elif line.startswith("FNDA:"):
                hits, _, name = line[5:].partition(",")
                key = (name, fn_lines.get(name, 0))
                current["functions"][key] = max(current["functions"].get(key, 0), int(hits))
            elif line.startswith("DA:"):
                ln, _, hits = line[3:].partition(",")
                ln = int(ln)
                current["lines"][ln] = max(current["lines"].get(ln, 0), int(hits.split(",")[0]))
            elif line == "end_of_record":
                current = None
    return {rel: info for rel, info in files.items() if rel is not None}


def counts(hits):
    """[covered, total] of a {key: hits} map."""
    return [sum(1 for h in hits.values() if h > 0), len(hits)]


def by_first_line(functions):
    """{first line: hits}: a function is counted by its first line. The Rust names are mangled with each build's
    hash, so one function under the unit and the integration tests has two names but one line; closures on
    one line count once."""
    lines = {}
    for (_, ln), h in functions.items():
        lines[ln] = max(lines.get(ln, 0), h)
    return lines


def file_stats(per_type):
    """{type: {"lines": [c, t], "functions": [c, t]}} for one file, with "all" when a union type measured it."""
    stats = {}
    for t, info in per_type.items():
        stats[t] = {"lines": counts(info["lines"]), "functions": counts(by_first_line(info["functions"]))}
    union_lines, union_funcs = {}, {}
    for t in UNION:
        if t not in per_type:
            continue
        for ln, h in per_type[t]["lines"].items():
            union_lines[ln] = max(union_lines.get(ln, 0), h)
        for ln, h in by_first_line(per_type[t]["functions"]).items():
            union_funcs[ln] = max(union_funcs.get(ln, 0), h)
    if any(t in per_type for t in UNION):
        stats["all"] = {"lines": counts(union_lines), "functions": counts(union_funcs)}
    return stats


def group(files):
    """Sums the per-file stats of `files` into {"lines": {type: [c, t]}, "functions": {...}, "files": {type: n}}."""
    total = {"lines": {}, "functions": {}, "files": {}}
    for stats in files:
        for t, s in stats.items():
            for metric in ("lines", "functions"):
                acc = total[metric].setdefault(t, [0, 0])
                acc[0] += s[metric][0]
                acc[1] += s[metric][1]
            total["files"][t] = total["files"].get(t, 0) + 1
    return total


def operations(spec):
    """The spec's operations: (method, path pattern, tag)."""
    out = []
    for path, ops in (spec.get("paths") or {}).items():
        for method, op in (ops or {}).items():
            tags = op.get("tags") if isinstance(op, dict) else None
            out.append((method.upper(), path, str(tags[0]) if tags else ""))
    return out


def same_operation(pattern, path):
    """api-tests/report/build.mjs's matches(): segment by segment, a {parameter} matches any segment."""
    a = pattern.split("/")
    b = path.split("?")[0].split("/")
    if len(a) != len(b) and "..}" not in pattern:
        return False
    return all(seg.startswith("{") or (i < len(b) and seg == b[i]) for i, seg in enumerate(a))


def spec_coverage(openapi_path, bruno_path, summary_path):
    """The operations the Bruno run reached, per tag, or None without the spec or the run."""
    try:
        with open(openapi_path, encoding="utf-8") as f:
            ops = operations(json.load(f))
        with open(bruno_path, encoding="utf-8") as f:
            bruno = json.load(f)
    except (OSError, ValueError):
        return None
    # bruno.json is the reporter's array of runs (build.mjs reads the first), or one run.
    runs = (bruno[0] if bruno else {}) if isinstance(bruno, list) else bruno
    if not isinstance(runs, dict):
        runs = {}
    requests = []
    for result in runs.get("results") or []:
        url = urlsplit(str((result.get("request") or {}).get("url") or ""))
        method = str((result.get("request") or {}).get("method") or "").upper()
        requests.append((method, unquote(url.path)))
    per_tag = defaultdict(lambda: [0, 0])
    tested_total = 0
    for method, path, tag in ops:
        hit = any(m == method and same_operation(path, p) for m, p in requests)
        per_tag[tag][0] += 1
        per_tag[tag][1] += 1 if hit else 0
        tested_total += 1 if hit else 0
    result = {
        "operations": len(ops),
        "tested": tested_total,
        "tags": [{"tag": t or "(untagged)", "total": v[0], "tested": v[1]} for t, v in sorted(per_tag.items(), key=lambda kv: (-kv[1][0], kv[0]))],
        "matches_report": None,
    }
    try:
        with open(summary_path, encoding="utf-8") as f:
            summary = json.load(f)
        result["matches_report"] = summary.get("operations") == len(ops) and summary.get("tested") == tested_total
        if not result["matches_report"]:
            print(f"::warning::spec coverage {tested_total}/{len(ops)} differs from the report's summary", file=sys.stderr)
    except (OSError, ValueError):
        pass
    return result


def web_sources(root):
    """(measured, total): the web console's source files the tests load, and all of them."""
    src = os.path.join(root, "apps", "web", "src")
    total = 0
    for dirpath, _, names in os.walk(src):
        total += sum(1 for n in names if n.endswith((".ts", ".tsx", ".js")))
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("data")
    parser.add_argument("out")
    parser.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
    args = parser.parse_args()
    root = os.path.realpath(args.root)
    pkgs = packages(root, os.path.join(args.data, "metadata.json"))

    # Per file, per test type: the lcov counts (the web console's paths are relative to apps/web).
    per_file = defaultdict(dict)
    for t, name in REPORTS.items():
        path = os.path.join(args.data, name)
        if not os.path.isfile(path):
            print(f"coverage: no {name}: the {t} tests are not in the data", file=sys.stderr)
            continue
        for rel, info in read_lcov(path, root, "apps/web" if t == "web" else "").items():
            if is_code(rel) and package_of(rel, pkgs):
                per_file[rel][t] = info

    files = []
    for rel in sorted(per_file):
        stats = file_stats(per_file[rel])
        files.append({"path": rel, "package": package_of(rel, pkgs), "stats": stats})

    def stats_of(selected):
        return group([f["stats"] for f in selected])

    def entry(name, label, selected, extra=None):
        return {"name": name, "label": label, **stats_of(selected), **(extra or {})}

    services = [entry(n, l, [f for f in files if f["package"] == n]) for n, l in SERVICES]
    apps = [entry(n, l, [f for f in files if f["package"] == n]) for n, l in APPS]
    # Every package under crates/ and tools/, the services among them too (a daemon is also a crate).
    crates = []
    for name, d in sorted(pkgs):
        if not (d.startswith("crates/") or d.startswith("tools/")):
            continue
        crates.append(entry(name, name, [f for f in files if f["package"] == name]))

    measured, total_sources = sum(1 for f in files if "web" in f["stats"]), web_sources(root)
    data = {
        "format": FORMAT,
        "types": [{"id": t, "label": l} for t, l in TYPES] + [{"id": "all", "label": "All test types"}],
        "global": stats_of(files),
        "web_sources": {"measured": measured, "total": total_sources},
        "spec": spec_coverage(
            os.path.join(root, "docs", "api", "openapi.json"),
            os.path.join(args.data, "api-run", "bruno.json"),
            os.path.join(args.data, "api-run", "site", "summary.json"),
        ),
        "services": services,
        "apps": apps,
        "crates": crates,
        "files": [{"path": f["path"], "package": f["package"], **f["stats"]} for f in files],
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=1)
        f.write("\n")
    everything = data["global"]["lines"].get("all", [0, 0])
    print(f"coverage: {len(files)} files, {len(services)} services, {len(apps)} apps, {len(crates)} crates; "
          f"all test types cover {everything[0]} of {everything[1]} lines")
    return 0


if __name__ == "__main__":
    sys.exit(main())
