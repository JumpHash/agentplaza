"""Cross-check roles, tools, schemas, statutes and evals (0113). Needs pyyaml and jsonschema.
Run: python -I tests/check_roles.py   (from docs/codex-draft)"""
import glob, json, os, re, sys
import yaml
from jsonschema import Draft202012Validator

os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
fail = 0
def bad(msg):
    global fail; fail += 1; print("FAIL  " + msg)

HANDBOOK = ["Your seat", "Who you work with", "Your laws in plain language", "Situations you'll face", "Words you'll meet"]
PLAYBOOK = ["When to use", "Steps", "Checklist", "Pitfalls"]
SECTIONS = ["Purpose", "You receive", "Workflow", "Decision rules", "Output", "Escalate when",
            "Budget and stopping", "Working with others", "Avoid", "Example", "Anti-example", "Reference", "Untrusted content", "Laws"]
UNIVERSAL = ["statutes/S-000"]          # statutes that apply to every agent

schema_actions = set(re.findall(r'action\s+"([^"]+)"', open("schema.cedarschema").read()))
def statute_files(ids):
    files = []
    for i in ids:
        m = glob.glob(i + "-*.cedar") + glob.glob(i + ".cedar")
        if not m: bad(f"statute {i} not found")
        files += m
    return files
def permitted_actions(files):
    acts = set()
    for f in files:
        for pol in re.findall(r"permit\s*\((.*?)\)", open(f).read(), re.S):
            acts |= set(re.findall(r'Action::"([^"]+)"', pol))
    return acts

# ---- schemas ----
schemas = {}
for p in sorted(glob.glob("schemas/*.schema.json")):
    name = os.path.basename(p)[:-len(".schema.json")]
    s = json.load(open(p))
    try: Draft202012Validator.check_schema(s)
    except Exception as e: bad(f"schema {name} invalid: {e}"); continue
    if not s.get("examples"): bad(f"schema {name} has no example")
    for ex in s.get("examples", []):
        errs = list(Draft202012Validator(s).iter_errors(ex))
        if errs: bad(f"schema {name} example invalid: {errs[0].message}")
    schemas[name] = s
print(f"ok    {len(schemas)} output schemas valid, examples pass")

# ---- tool catalog ----
catalog = yaml.safe_load(open("tools/catalog.yaml"))
for t, spec in catalog.items():
    if spec.get("cedar_action") not in schema_actions: bad(f"tool {t}: cedar action {spec.get('cedar_action')} not in schema")
    if spec.get("risk_class") not in ("read", "write", "irreversible"): bad(f"tool {t}: bad risk class")
    if "submits" in spec:
        if spec["submits"] not in schemas: bad(f"tool {t}: submits unknown schema {spec['submits']}")
    elif "input" in spec:
        try: Draft202012Validator.check_schema(spec["input"])
        except Exception as e: bad(f"tool {t}: input schema invalid: {e}")
    else: bad(f"tool {t}: needs input or submits")
print(f"ok    {len(catalog)} tools: actions exist in the Cedar schema, inputs valid")

def check_evals(name, ev):
    sc = ev.get("scenarios", [])
    if len(sc) < 8: bad(f"{name}: needs at least 8 eval scenarios, has {len(sc)}")
    kinds = [x.get("kind") for x in sc]
    if kinds.count("adversarial") < 2: bad(f"{name}: needs at least 2 adversarial scenarios")
    if kinds.count("multi-step") < 1: bad(f"{name}: needs at least 1 multi-step scenario")
    ids = [x.get("id") for x in sc]
    if len(ids) != len(set(ids)): bad(f"{name}: duplicate scenario ids")
    for x in sc:
        if x.get("kind") not in ("single", "multi-step", "adversarial"): bad(f"{name}: scenario {x.get('id')} bad kind")
        if x.get("kind") == "multi-step" and len(x.get("steps", [])) < 2: bad(f"{name}: {x.get('id')} needs 2+ steps")
        if x.get("kind") != "multi-step" and not x.get("situation"): bad(f"{name}: {x.get('id')} needs a situation")
        if not x.get("expect") or not x.get("must_not"): bad(f"{name}: scenario {x.get('id')} needs expect and must_not")

law_ids = set()
for f in glob.glob("constitution/*.cedar") + glob.glob("statutes/*.cedar") + glob.glob("bylaws/*/*.cedar") + glob.glob("contracts/*.cedar"):
    for lid, cl in re.findall(r'@law\("([^"]+)"\) @clause\("([^"]+)"\)', open(f).read()):
        law_ids.add(lid); law_ids.add(lid + cl)
decisions = {os.path.basename(p)[:4] for p in glob.glob("../decisions/*.md")}
def check_refs(path):
    """Every law id and decision number a document cites must exist (guards against invented references)."""
    text = open(path).read()
    for ref in set(re.findall(r"\b([CSG]-\d{3,4}[a-z]?|B-[a-z][a-z0-9-]*-\d{3}[a-z]?)\b", text)):
        if ref not in law_ids: bad(f"{path}: cites unknown law {ref}")
    for d in set(re.findall(r"\((\d{4})\)", text)) | set(re.findall(r"\b(\d{4})[,)]", text)):
        if d.startswith("0") and d not in decisions: bad(f"{path}: cites unknown decision {d}")
def check_doc(path, sections, kind):
    if not os.path.exists(path): bad(f"missing {kind}: {path}"); return
    text = open(path).read()
    if not text.startswith("# "): bad(f"{path}: must start with a title")
    for sec in sections:
        if f"## {sec}\n" not in text: bad(f"{path}: missing section '{sec}'")
    check_refs(path)
used_playbooks = set()
for p in sorted(glob.glob("playbooks/*.md")):
    check_doc(p, PLAYBOOK, "playbook")

# ---- roles ----
role_dirs = sorted(d for d in glob.glob("roles/*/") if os.path.exists(d + "role.yaml"))
names = {os.path.basename(d.rstrip("/")) for d in role_dirs}
overlay = yaml.safe_load(open("roles/council-seat/overlay.yaml"))
seats = []
for d in role_dirs:
    name = os.path.basename(d.rstrip("/"))
    r = yaml.safe_load(open(d + "role.yaml"))
    for k in ("role", "permits", "model", "tools", "outputs", "budget", "prompt", "evals"):
        if k not in r: bad(f"{name}: missing {k}")
    perms = statute_files(r["permits"] + UNIVERSAL + (overlay["permits"] if r.get("council_seat") else []))
    allowed = permitted_actions(perms)
    tools = r["tools"] + (overlay["tools"] if r.get("council_seat") else [])
    for t in tools:
        if t not in catalog: bad(f"{name}: tool {t} not in catalog"); continue
        a = catalog[t]["cedar_action"]
        if catalog[t].get("grant_only"):
            if a in allowed: bad(f"{name}: {t} is grant-only but a standing statute permits {a} (0079)")
            if not glob.glob("contracts/*.cedar"): bad("no example contract-tier grant")
            continue
        if a not in allowed: bad(f"{name}: tool {t} needs Cedar action {a}, not permitted by {', '.join(r['permits'])}")
    for o in r["outputs"]:
        if o not in schemas: bad(f"{name}: output {o} has no schema")
        if not any(catalog.get(t, {}).get("submits") == o for t in r["tools"]): bad(f"{name}: no tool submits {o}")
    df = r["model"].get("differ_from")
    if df and df not in ("defendant", "prosecutor", "builder"): bad(f"{name}: differ_from {df} unknown")
    if r.get("council_seat"):
        seats.append(name)
        if r["role"] == "legal-gov": bad(f"{name}: legal-gov can't hold a council seat (0034)")
    text = open(d + r["prompt"]).read()
    for sec in SECTIONS:
        if f"## {sec}\n" not in text: bad(f"{name}: prompt missing section '{sec}'")
    ev = yaml.safe_load(open(d + r["evals"]))
    check_evals(name, ev)
    check_doc(d + r.get("handbook", "handbook.md"), HANDBOOK, "handbook")
    for pb in r.get("playbooks", []):
        used_playbooks.add(pb)
        if not os.path.exists(pb): bad(f"{name}: playbook {pb} missing")
print(f"ok    {len(role_dirs)} roles checked: statutes, tools vs permits, outputs, prompts (with example and anti-example), evals (≥8, ≥2 adversarial, ≥1 multi-step)")
check_evals("council-seat", yaml.safe_load(open("roles/council-seat/evals.yaml")))
check_doc("roles/council-seat/handbook.md", HANDBOOK, "handbook")
used_playbooks |= set(overlay.get("playbooks", []))
for p in glob.glob("playbooks/*.md"):
    if p not in used_playbooks: bad(f"{p}: no role uses this playbook")
print(f"ok    {len(glob.glob('playbooks/*.md'))} playbooks and handbooks: sections present, cited laws and decisions exist, every playbook used")
expected_seats = {"finance", "legal-agents-counsel", "incident", "mayor-office"}   # 0034: four department heads
if set(seats) != expected_seats: bad(f"council seats {sorted(seats)} != {sorted(expected_seats)} (0034)")
else: print("ok    council seats match 0034 (Finance, Legal (Agents), Incident response, Mayor Office)")
print(f"\n{fail} failure(s)"); sys.exit(1 if fail else 0)
