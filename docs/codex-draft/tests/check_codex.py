"""Codex consistency (0118): flags, sanctions and law metadata.
Run: python -I tests/check_codex.py   (from docs/codex-draft). Needs pyyaml."""
import glob, os, re, sys
import yaml
os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
fail = 0
def bad(m):
    global fail; fail += 1; print("FAIL  " + m)
flags = yaml.safe_load(open("flags.yaml"))
sched = yaml.safe_load(open("sanctions.yaml"))
types = set(sched["types"])
used_flags, laws = set(), 0
for f in sorted(glob.glob("constitution/*.cedar") + glob.glob("statutes/*.cedar") + glob.glob("bylaws/*/*.cedar") + glob.glob("contracts/*.cedar")):
    text = open(f).read()
    for block in re.split(r"\n(?=@law\()", text):
        if "@law(" not in block: continue
        laws += 1
        meta = dict(re.findall(r'@(\w+)\("([^"]*)"\)', block))
        lid = f"{meta.get('law')}{meta.get('clause','')}"
        for k in ("law", "clause", "tier", "enforcement", "statement", "severity", "sanctions"):
            if k not in meta: bad(f"{f}: {lid} missing @{k}")
        if meta.get("enforcement") not in ("preventive", "detective", "judgment"): bad(f"{lid}: bad enforcement")
        if meta.get("severity") not in list(sched["schedule"]) : bad(f"{lid}: severity {meta.get('severity')} not in schedule")
        for sanc in meta.get("sanctions", "").split(";"):
            sanc = sanc.strip()
            if sanc and sanc != "conduct-schedule" and sanc.split(":")[0] not in types:
                bad(f"{lid}: unknown sanction '{sanc}'")
        for fl in re.findall(r'context\.flags\.contains\("([^"]+)"\)', block):
            used_flags.add(fl)
            if fl not in flags: bad(f"{lid}: flag '{fl}' not in flags.yaml")
# Bylaws (0124): they may only narrow, only inside their own town, never touch protected rights,
# and carry light sanctions. Cedar has no "permit within a statute", so bylaws are forbid-only.
PROTECTED = {"approval.request", "report.submit", "petition.file", "brief.file", "case.read", "codex.read",
             "sandbox.event", "registry.set_name", "message.send"}
BYLAW_SEVERITIES, BYLAW_MAX_FINE = {"low", "medium"}, 100
bylaws = 0
for f in sorted(glob.glob("bylaws/*/*.cedar")):
    town = f.split("/")[1]
    text = open(f).read()
    for block in re.split(r"\n(?=@law\()", text):
        if "@law(" not in block: continue
        bylaws += 1
        meta = dict(re.findall(r'@(\w+)\("([^"]*)"\)', block))
        lid = f"{meta.get('law')}{meta.get('clause','')}"
        body = re.sub(r'@\w+\("[^"]*"\)', "", block)
        if meta.get("tier") != "bylaw": bad(f"{lid}: tier must be bylaw")
        if meta.get("town") != town: bad(f"{lid}: @town must be '{town}' (its folder)")
        if not (meta.get("law") or "").startswith(f"B-{town}-"): bad(f"{lid}: id must start with B-{town}-")
        if re.search(r"\bpermit\s*\(", body): bad(f"{lid}: bylaws may only forbid")
        if not re.search(r'forbid\s*\(\s*principal\s+in\s+Town::"' + re.escape(town) + r'"', body):
            bad(f"{lid}: scope must be principal in Town::\"{town}\"")
        others = set(re.findall(r'Town::"([^"]+)"', body)) - {town}
        if others: bad(f"{lid}: mentions other towns {sorted(others)}")
        for act in re.findall(r'Action::"([^"]+)"', body):
            if act in PROTECTED: bad(f"{lid}: may not restrict protected right {act}")
        if meta.get("severity") not in BYLAW_SEVERITIES: bad(f"{lid}: bylaw severity must be low or medium")
        for sanc in meta.get("sanctions", "").split(";"):
            kind, _, n = sanc.strip().partition(":")
            if kind not in ("warning", "fine", ""): bad(f"{lid}: bylaws may only warn or fine, not '{kind}'")
            if kind == "fine" and int(n) > BYLAW_MAX_FINE: bad(f"{lid}: fine {n} over the bylaw cap {BYLAW_MAX_FINE}")
print(f"ok    {bylaws} bylaw clauses: forbid-only, own town, no protected rights, light sanctions")
for fl, spec in flags.items():
    if not spec.get("setters"): bad(f"flag {fl} has no setter")
    if fl not in used_flags: bad(f"flag {fl} registered but no law uses it")
for sev, row in sched["schedule"].items():
    for n, s in row.items():
        for part in s.split(";"):
            if part.strip().split(":")[0] not in types: bad(f"schedule {sev}/{n}: unknown sanction {part.strip()}")
print(f"ok    {laws} law clauses: metadata complete, severities and sanctions known")
print(f"ok    {len(flags)} flags: every flag used is registered with a setter, every registered flag is used")
print(f"\n{fail} failure(s)"); sys.exit(1 if fail else 0)
