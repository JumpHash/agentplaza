"""Town configs (0124): validate templates and towns against town.schema.json and cross-check them
against the Codex draft (roles, bylaws), host groups and each other. Lists what each town widens
relative to its template, which needs owner ratification (0067).
Run: python -I check_towns.py   (needs pyyaml and jsonschema)"""
import glob, json, os, re, sys
import yaml
from jsonschema import Draft202012Validator

here = os.path.dirname(os.path.abspath(__file__))
codex = os.path.join(here, "..", "codex-draft")
fail = 0
def bad(m):
    global fail; fail += 1; print("FAIL  " + m)
def ok(m): print("ok    " + m)

schema = json.load(open(os.path.join(here, "town.schema.json")))
Draft202012Validator.check_schema(schema)
validator = Draft202012Validator(schema)
groups = yaml.safe_load(open(os.path.join(here, "host-groups.yaml")))
roles = {}
for f in glob.glob(os.path.join(codex, "roles", "*", "role.yaml")):
    roles[os.path.basename(os.path.dirname(f))] = yaml.safe_load(open(f))
GLOBAL = {"mayor", "finance", "incident", "mayor-office", "auditor", "legal-gov-judge", "legal-gov-prosecutor",
          "legal-agents-counsel", "legal-agents-defender", "legal-agents-ombudsman"}   # one per town, or departments (0033)
RUNGS = ["cheap", "mid", "frontier"]
bylaw_ids = {}
for f in glob.glob(os.path.join(codex, "bylaws", "*", "*.cedar")):
    town = os.path.basename(os.path.dirname(f))
    for lid in re.findall(r'@law\("([^"]+)"\)', open(f).read()):
        bylaw_ids.setdefault(town, set()).add(lid)

def check(path, cfg, is_template):
    name = os.path.relpath(path, here)
    errs = sorted(validator.iter_errors(cfg), key=lambda e: list(e.path))
    for e in errs: bad(f"{name}: {'/'.join(map(str, e.path)) or '(root)'}: {e.message}")
    if errs: return
    if not is_template:
        for k in ("id", "name", "template"):
            if k not in cfg: bad(f"{name}: a town needs '{k}'")
        if cfg.get("id") != os.path.splitext(os.path.basename(path))[0]: bad(f"{name}: id must match the file name")
    elif "id" in cfg or "name" in cfg: bad(f"{name}: templates carry no id or name")
    seen = set()
    for r in cfg["roster"]:
        if r["role"] in seen: bad(f"{name}: role {r['role']} listed twice")
        seen.add(r["role"])
        if r["role"] in GLOBAL: bad(f"{name}: {r['role']} is not a town role (Mayor is implicit; departments are global)")
        elif r["role"] not in roles: bad(f"{name}: unknown role {r['role']}")
        elif "rung" in r and RUNGS.index(r["rung"]) > RUNGS.index(roles[r["role"]]["model"]["max_rung"]):
            bad(f"{name}: {r['role']} rung {r['rung']} above its max {roles[r['role']]['model']['max_rung']}")
    b = cfg["budget"]
    if b["task_default_credits"] > b["monthly_credits"]: bad(f"{name}: task default above the monthly budget")
    for g in cfg["egress"]["host_groups"]:
        if g == "blocked": bad(f"{name}: the blocked group can't be allowed")
        elif g not in groups: bad(f"{name}: unknown host group {g}")
    blocked = set(groups.get("blocked", []))
    for h in cfg["egress"].get("extra_hosts", []):
        if h in blocked: bad(f"{name}: extra host {h} is blocked")
    has_edge = "edge-operator" in seen
    if cfg["tools"].get("devices") and not has_edge: bad(f"{name}: devices without an edge-operator")
    if has_edge and not cfg["placement"].get("require_labels"): bad(f"{name}: edge-operators need required machine labels")
    if has_edge and cfg["egress"]["host_groups"]: bad(f"{name}: edge towns get no general egress (S-016 towns stay offline)")
    for s in cfg["tools"]["scopes"]:
        if not re.fullmatch(r"[a-z0-9-]+:[a-z0-9-]+", s): bad(f"{name}: scope {s} is not service:permission")
    if not is_template:
        listed = set(cfg.get("bylaws", []))
        have = bylaw_ids.get(cfg["id"], set())
        for lid in listed - have: bad(f"{name}: bylaw {lid} has no file in codex-draft/bylaws/{cfg['id']}/")
        for lid in have - listed: bad(f"{name}: bylaw {lid} exists but isn't listed")
    if not fail: ok(f"{name}")

templates = {}
for f in sorted(glob.glob(os.path.join(here, "templates", "*.yaml"))):
    cfg = yaml.safe_load(open(f)); templates[os.path.splitext(os.path.basename(f))[0]] = cfg
    check(f, cfg, True)
towns = {}
for f in sorted(glob.glob(os.path.join(here, "towns", "*.yaml"))):
    cfg = yaml.safe_load(open(f)); towns[cfg.get("id")] = cfg
    check(f, cfg, False)
for t in bylaw_ids:
    if t not in towns: bad(f"bylaws exist for unknown town {t}")

# What each town widens relative to its template: these need owner ratification (0067).
print("\nWidenings over the template (owner ratifies):")
for tid, cfg in towns.items():
    tpl = templates.get(cfg.get("template"))
    if tpl is None: bad(f"town {tid}: unknown template {cfg.get('template')}"); continue
    w = []
    w += [f"host group {g}" for g in set(cfg["egress"]["host_groups"]) - set(tpl["egress"]["host_groups"])]
    w += [f"extra host {h}" for h in cfg["egress"].get("extra_hosts", [])]
    w += [f"tool scope {s}" for s in set(cfg["tools"]["scopes"]) - set(tpl["tools"]["scopes"])]
    tr = {r["role"]: r["max"] for r in tpl["roster"]}
    w += [f"role {r['role']} (not in template)" for r in cfg["roster"] if r["role"] not in tr]
    w += [f"{r['role']} max {tr[r['role']]} -> {r['max']}" for r in cfg["roster"] if r["role"] in tr and r["max"] > tr[r["role"]]]
    if cfg["budget"]["monthly_credits"] > tpl["budget"]["monthly_credits"]:
        w.append(f"monthly budget {tpl['budget']['monthly_credits']} -> {cfg['budget']['monthly_credits']}")
    if tpl["confidentiality"] == "sensitive" and cfg["confidentiality"] != "sensitive": w.append("confidentiality lowered")
    print(f"  {tid}: " + ("; ".join(sorted(w)) if w else "none"))
print(f"\n{len(templates)} templates, {len(towns)} towns, {fail} failure(s)")
sys.exit(1 if fail else 0)
