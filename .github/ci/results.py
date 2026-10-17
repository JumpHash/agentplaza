#!/usr/bin/env python3
"""Records a gate job's results as data, for the comments check-results.yml posts on the pull request.

  results.py <out-dir> <kind> <title> [--junit FILE] [--part NAME=OUTCOME[:LOG]]...

Writes <out-dir>/meta.json: the kind of check (tests, lint, console), the job's title, and each part's
outcome (success, failure, cancelled, skipped, or empty for not run), with the last lines of the part's
log when it failed. With --junit, copies nextest's JUnit report beside it. The job then uploads
<out-dir> as an artifact named results-<something>.

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
    parser.add_argument("kind", choices=["tests", "lint", "console"])
    parser.add_argument("title")
    parser.add_argument("--junit")
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

    meta = {"kind": args.kind, "title": args.title, "parts": parts, "junit": False}
    if args.junit and os.path.isfile(args.junit):
        shutil.copyfile(args.junit, os.path.join(args.out, "junit.xml"))
        meta["junit"] = True
    with open(os.path.join(args.out, "meta.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=1)
    print(f"results: {args.title}: " + ", ".join(f"{p['name']} {p['outcome'] or 'not run'}" for p in parts))


if __name__ == "__main__":
    sys.exit(main())
