#!/usr/bin/env python3
"""Turns a gate run's results (results.py's data, one directory per job) into the pull request's
comments, one per kind of check: Tests (every platform), Lint, Web console, Coverage.

  check-comments.py <results-dir> <run-url> <commit>

Prints a JSON list of {"marker", "body"}; check-results.yml creates or updates the comment that
starts with each marker. The data comes from the pull request's own jobs, so it is untrusted:
names and output go into code spans and code blocks (no markdown, links or mentions of theirs reach
the comment), everything is capped in length, and a JUnit file larger than 20 MB is not read. The
coverage data (coverage.py's coverage.json) is checked before use: only whole counts, covered at most
total, and the rows it names are limited to the known test types; every percentage is worked out here.
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
    "coverage": "Coverage",
}
MAX_FAILURES = 15
MAX_OUTPUT_LINES = 40
MAX_JUNIT = 20 * 1024 * 1024
MAX_DATA = 20 * 1024 * 1024
MAX_GROUPS = 200
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
        if meta.get("kind") == "coverage" and meta.get("data") is True:
            meta["coverage"] = read_data(os.path.join(meta["dir"], "data.json"))
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


# ---- coverage ----------------------------------------------------------------------------------------

# The test types of the coverage data (.github/ci/coverage.py), in the order the comment lists them.
COVERAGE_ROWS = [
    ("rust-unit", "Rust unit tests"),
    ("rust-integration", "Rust integration tests"),
    ("rust-all", "Rust unit and integration together"),
    ("hub-api", "Hub API (Bruno), code"),
    ("web", "Web console"),
    ("all", "**All test types**"),
]
# The columns of the tables by service and app, and by crate.
COVERAGE_COLUMNS = [
    ("rust-unit", "Rust unit"),
    ("rust-integration", "Rust integration"),
    ("hub-api", "Hub API"),
    ("web", "Web console"),
    ("all", "All"),
]
NAME = re.compile(r"[A-Za-z0-9 _.@/()+:-]{1,80}")


def whole(value):
    """A count from the data: a whole number, not a boolean, from 0 to a trillion."""
    if type(value) is int and 0 <= value <= 10**12:
        return value
    return None


def count_pair(value):
    """[covered, total] from the data, when both are counts and covered is at most total."""
    if isinstance(value, list) and len(value) == 2:
        covered, total = whole(value[0]), whole(value[1])
        if covered is not None and total is not None and covered <= total:
            return covered, total
    return None


def stats(group, metric):
    """{test type: (covered, total) or None} of a group's "lines" or "functions", for the known test types only."""
    raw = group.get(metric) if isinstance(group, dict) else None
    raw = raw if isinstance(raw, dict) else {}
    return {t: count_pair(raw.get(t)) for t, _ in COVERAGE_ROWS}


def percent(pair):
    if pair is None or pair[1] == 0:
        return "–"
    return f"{100 * pair[0] / pair[1]:.1f}%"


def detail(pair):
    """`61.2% (12,340 of 20,160)`, or a dash when nothing was measured."""
    if pair is None or pair[1] == 0:
        return "–"
    return f"{percent(pair)} ({pair[0]:,} of {pair[1]:,})"


def name_cell(value):
    """A package or service name in a code span; anything that is not a plain name shows as ?."""
    text = str(value)
    return code_span(text if NAME.fullmatch(text) else "?")


def group_cell(name, label):
    cell = name_cell(name)
    label = plain(label, 60)
    return cell if label in ("?", str(name)) else f"{cell} {label}"


def groups_of(data, key):
    """[(name, label, lines)] of the data's services, apps or crates, in the order the data gives them."""
    items = data.get(key) if isinstance(data.get(key), list) else []
    out = []
    for item in items[:MAX_GROUPS]:
        if isinstance(item, dict):
            out.append((str(item.get("name", "")), str(item.get("label", "")), stats(item, "lines")))
    return out


def group_table(first, groups):
    rows = [
        f"| {first} | " + " | ".join(label for _, label in COVERAGE_COLUMNS) + " |",
        "|" + "---|" * (len(COVERAGE_COLUMNS) + 1),
    ]
    for name, label, lines in groups:
        cells = []
        for t, _ in COVERAGE_COLUMNS:
            pair = lines.get(t)
            if t == "all" and pair is not None and pair[1] > 0:
                cells.append(f"{percent(pair)} of {pair[1]:,}")
            else:
                cells.append(percent(pair))
        rows.append(f"| {group_cell(name, label)} | " + " | ".join(cells) + " |")
    return rows


def crate_order(group):
    """Crates with the least covered lines first; the ones nothing measured last."""
    pair = group[2].get("all")
    if pair is None or pair[1] == 0:
        return (1, 0.0, group[0])
    return (0, pair[0] / pair[1], group[0])


def coverage_headline(data):
    pair = stats(data.get("global"), "lines")["all"]
    if pair is None or pair[1] == 0:
        return "### Coverage: no lines measured"
    return f"### Coverage: {percent(pair)} of lines"


def coverage_body(data):
    if not isinstance(data.get("global"), dict):
        return None
    glob = data["global"]
    lines, funcs = stats(glob, "lines"), stats(glob, "functions")
    files = glob.get("files") if isinstance(glob.get("files"), dict) else {}
    sources = data.get("web_sources") if isinstance(data.get("web_sources"), dict) else {}
    types = ["| Test type | Lines | Functions | Files |", "|---|---|---|---|"]
    for t, label in COVERAGE_ROWS:
        count = whole(files.get(t))
        if t == "web" and whole(sources.get("measured")) is not None and whole(sources.get("total")) is not None:
            shown = f"{sources['measured']:,} of {sources['total']:,} sources"
        else:
            shown = "–" if count is None else f"{count:,}"
        types.append(f"| {label} | {detail(lines[t])} | {detail(funcs[t])} | {shown} |")
    spec = data.get("spec") if isinstance(data.get("spec"), dict) else None
    spec_tags = []
    if spec is not None and count_pair([spec.get("tested"), spec.get("operations")]) is not None:
        operations, tested = spec["operations"], spec["tested"]
        pct = f" ({100 * tested / operations:.1f}%)" if operations else ""
        types.insert(6, f"| Hub API spec operations | {tested:,} of {operations:,} operations{pct} | – | – |")
        for item in (spec.get("tags") or [])[:MAX_GROUPS]:
            if isinstance(item, dict) and count_pair([item.get("tested"), item.get("total")]) is not None:
                tag = plain(item.get("tag"), 60)
                spec_tags.append(f"| {tag} | {item['tested']:,} | {item['total']:,} |")

    services = groups_of(data, "services")
    apps = groups_of(data, "apps")
    crates = sorted(groups_of(data, "crates"), key=crate_order)
    body = [
        "Measured on this commit: the Rust tests (unit, integration, and both together), the hub API's Bruno "
        "collection against an instrumented hub, and the web console's tests. A line counts once in **All**, "
        "when any test type covers it.",
        "",
        "**By test type**",
        "",
        *types,
        "",
        "**By service and app**",
        "",
        *group_table("Service or app", services + apps),
        "",
        f"<details><summary>By crate ({len(crates)}), least covered first</summary>\n\n"
        + "\n".join(group_table("Crate", crates))
        + "\n\n</details>",
    ]
    if spec_tags:
        body.append(
            "\n<details><summary>Hub API spec operations by tag</summary>\n\n"
            + "\n".join(["| Tag | Tested | Operations |", "|---|---|---|", *spec_tags])
            + "\n\n</details>"
        )
    return "\n".join(body)


def read_data(path):
    """coverage.json as data (no larger than 20 MB), or None."""
    try:
        if os.path.getsize(path) > MAX_DATA:
            return None
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def section(kind, mine):
    """The heading and body of one kind's comment."""
    if kind == "coverage":
        data = next((j["coverage"] for j in mine if isinstance(j.get("coverage"), dict)), None)
        body = coverage_body(data) if data is not None and all(outcome_of(j) == "success" for j in mine) else None
        if body is not None:
            return coverage_headline(data), body
        return headline(kind, mine), parts_body(mine)
    return headline(kind, mine), (tests_body(mine) if kind == "tests" else parts_body(mine))


def main():
    root, run_url, commit = sys.argv[1], sys.argv[2], sys.argv[3][:10]
    jobs = load(root)
    comments = []
    for kind in KINDS:
        mine = [j for j in jobs if j["kind"] == kind]
        if not mine:
            continue
        heading, body = section(kind, mine)
        text = (
            f"<!-- check-results:{kind} -->\n{heading}\n\n{body}\n\n"
            f"<sub>Commit {commit} · [the run]({run_url})</sub>"
        )
        comments.append({"marker": f"<!-- check-results:{kind} -->", "body": text[:60000]})
    json.dump(comments, sys.stdout)


if __name__ == "__main__":
    main()
