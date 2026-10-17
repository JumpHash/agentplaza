#!/usr/bin/env python3
"""Turns a gate run's results (results.py's data, one directory per job) into the pull request's
comments, one per kind of check: Tests (every platform), Lint, Web console.

  check-comments.py <results-dir> <run-url> <commit>

Prints a JSON list of {"marker", "body"}; check-results.yml creates or updates the comment that
starts with each marker. The data comes from the pull request's own jobs, so it is untrusted:
names and output go into code spans and code blocks (no markdown, links or mentions of theirs reach
the comment), everything is capped in length, and a JUnit file larger than 20 MB is not read.
"""

import json
import os
import re
import sys
import xml.etree.ElementTree as ET

KINDS = {
    "tests": "Tests",
    "lint": "Lint",
    "console": "Web console",
}
MAX_FAILURES = 15
MAX_OUTPUT_LINES = 40
MAX_JUNIT = 20 * 1024 * 1024
WORD = {"success": "passed", "failure": "**failed**", "cancelled": "cancelled", "skipped": "skipped", "": "not run"}


def plain(text, limit=80):
    """A job's title or a part's name: letters, digits, spaces and a little punctuation, nothing that
    markdown or HTML would read."""
    return re.sub(r"[^A-Za-z0-9 ()/_.,:+-]", "", str(text))[:limit].strip() or "?"


def code_span(text):
    """`text` as an inline code span that nothing in it can end early."""
    text = " ".join(str(text).split())[:200]
    fence = "`"
    while fence in text:
        fence += "`"
    return f"{fence} {text} {fence}" if text.startswith("`") or text.endswith("`") else f"{fence}{text}{fence}"


def code_block(text, lines=MAX_OUTPUT_LINES):
    """`text` as a fenced block, the last `lines` lines, which no line inside can close early."""
    body = "\n".join(str(text).rstrip("\n").split("\n")[-lines:])
    fence = "```"
    while fence in body:
        fence += "`"
    return f"{fence}text\n{body}\n{fence}"


def junit(path):
    """(passed, failed, seconds, [(name, output)]) from a nextest JUnit report, or None."""
    try:
        if os.path.getsize(path) > MAX_JUNIT:
            return None
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError):
        return None
    passed = failed = 0
    seconds = 0.0
    failures = []
    for case in root.iter("testcase"):
        bad = case.find("failure")
        if bad is None:
            bad = case.find("error")
        if bad is None:
            passed += 1
        else:
            failed += 1
            if len(failures) < MAX_FAILURES:
                name = f"{case.get('classname', '')} {case.get('name', '')}".strip()
                out = [bad.get("message") or "", bad.text or ""]
                for stream in ("system-out", "system-err"):
                    node = case.find(stream)
                    if node is not None and node.text:
                        out.append(node.text)
                failures.append((name, "\n".join(x for x in out if x.strip())))
    try:
        seconds = float(root.get("time") or 0)
    except ValueError:
        pass
    return passed, failed, seconds, failures


def load(root):
    jobs = []
    for name in sorted(os.listdir(root)):
        meta_path = os.path.join(root, name, "meta.json")
        try:
            with open(meta_path, encoding="utf-8") as f:
                meta = json.load(f)
        except (OSError, ValueError):
            continue
        if meta.get("kind") not in KINDS or not isinstance(meta.get("parts"), list):
            continue
        meta["dir"] = os.path.join(root, name)
        jobs.append(meta)
    return jobs


def outcome_of(job):
    outcomes = [str(p.get("outcome", "")) for p in job["parts"]]
    if "failure" in outcomes:
        return "failure"
    if "cancelled" in outcomes:
        return "cancelled"
    if outcomes and all(o in ("success", "skipped") for o in outcomes):
        return "success"
    return ""


def headline(kind, jobs):
    outcomes = [outcome_of(j) for j in jobs]
    if "failure" in outcomes:
        return f"### {KINDS[kind]}: failed"
    if "cancelled" in outcomes or "" in outcomes:
        return f"### {KINDS[kind]}: not finished"
    return f"### {KINDS[kind]}: passed"


def tests_body(jobs):
    rows = ["| Where | Result | Passed | Failed | Time |", "|---|---|---|---|---|"]
    details = []
    for job in jobs:
        title = plain(job.get("title"))
        result = junit(os.path.join(job["dir"], "junit.xml")) if job.get("junit") else None
        for part in job["parts"]:
            word = WORD.get(str(part.get("outcome", "")), "not run")
            name = plain(part.get("name"), 60)
            # A tests part is the job's own row; any other part names itself.
            where = title if part.get("name") == "tests" else name
            if part.get("name") == "tests" and result:
                passed, failed, seconds, failures = result
                rows.append(f"| {where} | {word} | {passed} | {failed} | {seconds:.0f} s |")
                for test, output in failures:
                    details.append(
                        f"<details><summary>{title}: {code_span(test)}</summary>\n\n{code_block(output)}\n\n</details>"
                    )
            else:
                rows.append(f"| {where} | {word} | | | |")
                if part.get("log"):
                    details.append(
                        f"<details><summary>{title}: {name}</summary>\n\n{code_block(part['log'])}\n\n</details>"
                    )
    body = "\n".join(rows)
    if details:
        body += "\n\n**Failures**\n\n" + "\n".join(details)
    return body


def parts_body(jobs):
    rows = ["| Check | Result |", "|---|---|"]
    details = []
    for job in jobs:
        for part in job["parts"]:
            name = plain(part.get("name"), 60)
            rows.append(f"| {name} | {WORD.get(str(part.get('outcome', '')), 'not run')} |")
            if part.get("log"):
                details.append(f"<details><summary>{name}</summary>\n\n{code_block(part['log'])}\n\n</details>")
    body = "\n".join(rows)
    if details:
        body += "\n\n" + "\n".join(details)
    return body


def main():
    root, run_url, commit = sys.argv[1], sys.argv[2], sys.argv[3][:10]
    jobs = load(root)
    comments = []
    for kind in KINDS:
        mine = [j for j in jobs if j["kind"] == kind]
        if not mine:
            continue
        body = tests_body(mine) if kind == "tests" else parts_body(mine)
        text = (
            f"<!-- check-results:{kind} -->\n{headline(kind, mine)}\n\n{body}\n\n"
            f"<sub>Commit {commit} · [the run]({run_url})</sub>"
        )
        comments.append({"marker": f"<!-- check-results:{kind} -->", "body": text[:60000]})
    json.dump(comments, sys.stdout)


if __name__ == "__main__":
    main()
