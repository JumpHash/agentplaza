#!/usr/bin/env python3
"""Records a gate job's results as data, for the comments check-results.yml posts on the pull request.

  results.py <out-dir> <kind> <title> [--junit FILE] [--data FILE] [--part NAME=OUTCOME[:LOG]]...

Writes <out-dir>/meta.json: the kind of check (tests, lint, console, coverage), the job's title, and each part's
outcome (success, failure, cancelled, skipped, or empty for not run), with the last lines of the part's
log when it failed. With --junit, copies nextest's JUnit report beside it. With --data, copies a JSON data
file (the coverage's coverage.json) beside it as data.json. The job then uploads <out-dir> as an artifact
named results-<something>.

Only data goes in: check-results.yml, which runs from main with write access, reads it as untrusted
input and never runs anything from it.
"""

import argparse
import json
import os
import re
import shutil
import sys

TAIL = 80
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07")


def tail(path, lines=TAIL):
    try:
        with open(path, "rb") as f:
            text = f.read()[-200_000:].decode("utf-8", "replace")
    except OSError:
        return ""
    text = ANSI.sub("", text).replace("\r\n", "\n").replace("\r", "\n")
    return "\n".join(text.rstrip("\n").split("\n")[-lines:])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out")
    parser.add_argument("kind", choices=["tests", "lint", "console", "coverage"])
    parser.add_argument("title")
    parser.add_argument("--junit")
    parser.add_argument("--data")
    parser.add_argument("--part", action="append", default=[], metavar="NAME=OUTCOME[:LOG]")
    args = parser.parse_args()

    os.makedirs(args.out, exist_ok=True)
    parts = []
    for spec in args.part:
        name, _, rest = spec.partition("=")
        outcome, _, log = rest.partition(":")
        part = {"name": name, "outcome": outcome}
        if outcome == "failure" and log:
            part["log"] = tail(log)
        parts.append(part)

    meta = {"kind": args.kind, "title": args.title, "parts": parts, "junit": False, "data": False}
    if args.junit and os.path.isfile(args.junit):
        shutil.copyfile(args.junit, os.path.join(args.out, "junit.xml"))
        meta["junit"] = True
    elif args.junit and any(p["name"] == "tests" and p["outcome"] in ("success", "failure") for p in parts):
        # The tests ran but left no report: the comment would show no counts.
        print(f"::warning::no JUnit report at {args.junit}: the Tests comment will have no counts")
    if args.data and os.path.isfile(args.data):
        shutil.copyfile(args.data, os.path.join(args.out, "data.json"))
        meta["data"] = True
    elif args.data:
        print(f"::warning::no data at {args.data}: the comment will have no data")
    with open(os.path.join(args.out, "meta.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=1)
    print(f"results: {args.title}: " + ", ".join(f"{p['name']} {p['outcome'] or 'not run'}" for p in parts))


if __name__ == "__main__":
    sys.exit(main())
