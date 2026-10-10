"""Red-team catalog validator.

Run: python3 -I docs/security/redteam/check_catalog.py   (needs pyyaml)
Finds the repo from its own path, so it works from any directory.
Prints ok/FAIL lines and a coverage table; exits non-zero on any failure.
"""
import glob
import os
import re
import sys

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
DOCS = os.path.normpath(os.path.join(HERE, "..", ".."))
CATALOG = os.path.join(HERE, "catalog.yaml")

fails = 0


def bad(msg):
    global fails
    fails += 1
    print("FAIL  " + msg)


def ok(msg):
    print("ok    " + msg)


def read(rel):
    with open(os.path.join(DOCS, rel), encoding="utf-8") as f:
        return f.read()


# ---------------------------------------------------------------- reference indexes
def decision_numbers():
    return {os.path.basename(p)[:4] for p in glob.glob(os.path.join(DOCS, "decisions", "[0-9][0-9][0-9][0-9]-*.md"))}


def law_index():
    """{law_id: set(clauses)} from @law/@clause in constitution and statutes."""
    laws = {}
    for p in glob.glob(os.path.join(DOCS, "codex-draft", "constitution", "*.cedar")) + \
             glob.glob(os.path.join(DOCS, "codex-draft", "statutes", "*.cedar")):
        text = open(p, encoding="utf-8").read()
        for law, clause in re.findall(r'@law\("([^"]+)"\)\s*@clause\("([^"]+)"\)', text):
            laws.setdefault(law, set()).add(clause)
    return laws


def slug(heading):
    s = heading.strip().lower()
    s = re.sub(r"[^\w\- ]", "", s)
    return s.replace(" ", "-")


def heading_slugs(rel):
    return {slug(m) for m in re.findall(r"^#+\s+(.*)$", read(rel), re.M)}


def tripwire_names():
    text = read(glob.glob(os.path.join(DOCS, "decisions", "0072-*.md"))[0].replace(DOCS + os.sep, ""))
    return {slug(n) for n in re.findall(r"^\|\s*\*\*([^*]+)\*\*", text, re.M)}


def cedar_labels():
    return re.findall(r'^expect\s+(?:ALLOW|DENY)\s+"([^"]+)"', read("codex-draft/tests/check.sh"), re.M)


def eval_ids():
    ids = set()
    for p in glob.glob(os.path.join(DOCS, "codex-draft", "roles", "*", "evals.yaml")):
        role = os.path.basename(os.path.dirname(p))
        data = yaml.safe_load(open(p, encoding="utf-8")) or {}
        for s in data.get("scenarios", []):
            ids.add(f"{role}/{s['id']}")
    return ids


def tla_names():
    """Invariants/properties actually checked in a .cfg, plus mutant and witness names."""
    names = set()
    for p in glob.glob(os.path.join(DOCS, "formal", "*.cfg")):
        section = None
        for line in open(p, encoding="utf-8"):
            word = line.strip()
            if word in ("INVARIANTS", "PROPERTIES", "INVARIANT", "PROPERTY"):
                section = "inv"
                continue
            if word.isupper() and word:
                section = None
                continue
            if section == "inv" and word:
                names.update(word.split())
    for p in glob.glob(os.path.join(DOCS, "formal", "mutants", "*.cfg")):
        names.add(os.path.basename(p)[:-4])
    return names


def protocol_labels():
    text = read("protocol-draft/tests/test_protocol.py")
    return re.findall(r'check\(\s*f?"([^"]+)"', text)


def ledger_labels():
    return re.findall(r"await test\('([^']+)'", read("treasury-draft/tests/ledger.test.mjs"))


def unique_substring(ref, labels):
    hits = [l for l in labels if ref in l]
    return hits


# ---------------------------------------------------------------- load
try:
    cat = yaml.safe_load(open(CATALOG, encoding="utf-8"))
except Exception as e:  # noqa: BLE001
    print(f"FAIL  cannot load catalog: {e}")
    sys.exit(2)

DECISIONS = decision_numbers()
LAWS = law_index()
TRIPWIRES = tripwire_names()
CEDAR = cedar_labels()
EVALS = eval_ids()
TLA = tla_names()
PROTO = protocol_labels()
LEDGER = ledger_labels()

attackers = cat.get("attackers") or {}
boundaries = cat.get("boundaries") or {}
required_tags = cat.get("required_tags") or []
attacks = cat.get("attacks") or []

SEVERITIES = ["critical", "high", "medium", "low"]
MILESTONES = ["MVP", "M2", "M3", "Later"]
STATUSES = ["covered", "planned-only", "gap"]
DEF_KINDS = ["decision", "law", "design", "tripwire"]
TEST_KINDS = ["cedar", "eval", "tla", "protocol", "ledger", "planned"]
REQUIRED = ["id", "title", "attacker", "boundary", "technique", "impact", "severity", "milestone",
            "defences", "tests", "status", "tags"]
STRONG = {"decision", "law", "tripwire"}

if len(attackers) != 6:
    bad(f"header: expected 6 attacker classes (threat model), found {len(attackers)}")
if not 40 <= len(attacks) <= 60:
    bad(f"catalog has {len(attacks)} attacks; expected 40-60")

seen_ids, planned_slugs, used_attackers, used_tags = set(), set(), set(), set()


def check_decision_text(aid, field, text):
    for n in re.findall(r"\b(0\d{3})\b", text or ""):
        if n not in DECISIONS:
            bad(f"{aid}: {field} mentions decision {n}, which doesn't exist")


for a in attacks:
    aid = a.get("id", "<no id>")
    before = fails
    for k in REQUIRED:
        if k not in a or a[k] in (None, "", []) and k not in ("defences", "tests"):
            bad(f"{aid}: missing field '{k}'")
    if not re.fullmatch(r"RT-\d{3}", str(aid)):
        bad(f"{aid}: id must be RT-NNN")
    if aid in seen_ids:
        bad(f"{aid}: duplicate id")
    seen_ids.add(aid)
    if a.get("attacker") not in attackers:
        bad(f"{aid}: attacker '{a.get('attacker')}' not in header")
    used_attackers.add(a.get("attacker"))
    if a.get("boundary") not in boundaries:
        bad(f"{aid}: boundary '{a.get('boundary')}' not in header")
    if a.get("severity") not in SEVERITIES:
        bad(f"{aid}: severity '{a.get('severity')}'")
    if a.get("milestone") not in MILESTONES:
        bad(f"{aid}: milestone '{a.get('milestone')}'")
    status = a.get("status")
    if status not in STATUSES:
        bad(f"{aid}: status '{status}'")
    for t in a.get("tags") or []:
        used_tags.add(t)

    # defences
    defs = a.get("defences") or []
    for d in defs:
        kind, ref, how = d.get("kind"), str(d.get("ref", "")), d.get("how")
        if kind not in DEF_KINDS:
            bad(f"{aid}: defence kind '{kind}'")
            continue
        if not how:
            bad(f"{aid}: defence {ref} has no 'how'")
        if kind == "decision":
            if ref not in DECISIONS:
                bad(f"{aid}: decision {ref} doesn't exist")
        elif kind == "law":
            m = re.fullmatch(r"([CS]-\d{3})([a-z]?)", ref)
            if not m or m.group(1) not in LAWS or (m.group(2) and m.group(2) not in LAWS[m.group(1)]):
                bad(f"{aid}: law {ref} isn't an @law/@clause in the Codex")
        elif kind == "design":
            path, _, anchor = ref.partition("#")
            if not os.path.isfile(os.path.join(DOCS, path)):
                bad(f"{aid}: design file {path} doesn't exist")
            elif anchor and path.endswith(".md") and anchor not in heading_slugs(path):
                bad(f"{aid}: design anchor {ref} matches no heading")
        elif kind == "tripwire":
            if ref not in TRIPWIRES:
                bad(f"{aid}: tripwire '{ref}' not in 0072 (known: {sorted(TRIPWIRES)})")

    # tests
    tests = a.get("tests") or []
    real = [t for t in tests if t.get("kind") != "planned"]
    for t in tests:
        kind, ref, how = t.get("kind"), str(t.get("ref", "")), t.get("how")
        if kind not in TEST_KINDS:
            bad(f"{aid}: test kind '{kind}'")
            continue
        if not how:
            bad(f"{aid}: test {ref} has no 'how'")
        if kind == "cedar":
            hits = unique_substring(ref, CEDAR)
            if len(hits) != 1:
                bad(f"{aid}: cedar ref '{ref}' matches {len(hits)} labels in check.sh (need exactly 1)")
        elif kind == "eval":
            if ref not in EVALS:
                bad(f"{aid}: eval {ref} not found in codex-draft/roles/*/evals.yaml")
        elif kind == "tla":
            if ref not in TLA:
                bad(f"{aid}: tla '{ref}' is not a checked invariant/property in formal/*.cfg or a mutant/witness")
        elif kind == "protocol":
            hits = unique_substring(ref, PROTO)
            if len(hits) != 1:
                bad(f"{aid}: protocol ref '{ref}' matches {len(hits)} check labels (need exactly 1)")
        elif kind == "ledger":
            hits = unique_substring(ref, LEDGER)
            if len(hits) != 1:
                bad(f"{aid}: ledger ref '{ref}' matches {len(hits)} test names (need exactly 1)")
        elif kind == "planned":
            if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", ref):
                bad(f"{aid}: planned ref '{ref}' must be a lowercase slug")
            if ref in planned_slugs:
                bad(f"{aid}: planned slug '{ref}' used twice")
            planned_slugs.add(ref)

    # status rules (see README "How coverage is computed")
    strong = [d for d in defs if d.get("kind") in STRONG]
    if status == "covered":
        if not real:
            bad(f"{aid}: covered needs at least one non-planned test")
        if not strong:
            bad(f"{aid}: covered needs a decision, law or tripwire defence")
    elif status == "planned-only":
        if real:
            bad(f"{aid}: planned-only but has non-planned tests (should be covered)")
        if not tests:
            bad(f"{aid}: planned-only needs at least one planned test")
        if not strong:
            bad(f"{aid}: planned-only needs a decision, law or tripwire defence (else it's a gap)")
    elif status == "gap":
        if strong:
            bad(f"{aid}: gap but lists decision/law/tripwire defences; only design notes allowed")
        if real:
            bad(f"{aid}: gap but has non-planned tests")
        if not a.get("gap_note"):
            bad(f"{aid}: gap needs a gap_note")
    if status in ("gap", "planned-only") and not a.get("next"):
        bad(f"{aid}: {status} needs 'next' (one-line proposed fix)")
    if a.get("severity") == "critical" and status == "gap" and not a.get("gap_note"):
        bad(f"{aid}: critical gap without gap_note")
    check_decision_text(aid, "gap_note", a.get("gap_note"))
    check_decision_text(aid, "next", a.get("next"))
    if fails == before:
        ok(f"{aid}  {status:12} {a.get('title')}")

for k in attackers:
    if k not in used_attackers:
        bad(f"attacker class '{k}' is used by no attack")
for t in required_tags:
    if t not in used_tags:
        bad(f"required tag '{t}' is used by no attack")
for t in used_tags:
    if t not in required_tags:
        bad(f"tag '{t}' isn't listed in required_tags")

# ---------------------------------------------------------------- summary
print()
cols = STATUSES + ["total"]
w = max(len(k) for k in list(attackers) + ["attacker"]) + 2
print("| " + "attacker".ljust(w) + " | " + " | ".join(c.rjust(12) for c in cols) + " |")
print("|" + "-" * (w + 2) + "|" + "|".join("-" * 14 for _ in cols) + "|")
totals = {c: 0 for c in cols}
for k in attackers:
    row = {s: sum(1 for a in attacks if a.get("attacker") == k and a.get("status") == s) for s in STATUSES}
    row["total"] = sum(row.values())
    for c in cols:
        totals[c] += row[c]
    print("| " + k.ljust(w) + " | " + " | ".join(str(row[c]).rjust(12) for c in cols) + " |")
print("| " + "all".ljust(w) + " | " + " | ".join(str(totals[c]).rjust(12) for c in cols) + " |")
print(f"\n{len(attacks)} attacks, {fails} failure(s)")
sys.exit(1 if fails else 0)
