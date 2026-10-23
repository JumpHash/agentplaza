"""Cross-check roles, tools, output schemas, playbooks and evals.
Needs pyyaml and jsonschema. Run: python -I tests/check_roles.py   (from docs/codex-draft)"""
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
ROLE_KEYS = ("role", "tools", "outputs", "budget", "prompt", "evals")

schema_actions = set(re.findall(r'action\s+"([^"]+)"', open("schema.cedarschema").read()))

def check_doc(path, sections, kind):
    if not os.path.exists(path): bad(f"missing {kind}: {path}"); return
    text = open(path).read()
    if not text.startswith("# "): bad(f"{path}: must start with a title")
    for sec in sections:
        if f"## {sec}\n" not in text: bad(f"{path}: missing section '{sec}'")

# ---- output schemas ----
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
    if spec.get("cedar_action") not in schema_actions: bad(f"tool {t}: cedar action {spec.get('cedar_action')} not in schema.cedarschema")
    if spec.get("risk_class") not in ("read", "write", "irreversible"): bad(f"tool {t}: bad risk class")
    if "submits" in spec:
        if spec["submits"] not in schemas: bad(f"tool {t}: submits unknown schema {spec['submits']}")
    elif "input" in spec:
        try: Draft202012Validator.check_schema(spec["input"])
        except Exception as e: bad(f"tool {t}: input schema invalid: {e}")
    else: bad(f"tool {t}: needs input or submits")
print(f"ok    {len(catalog)} tools: actions exist in the policy schema, inputs valid")

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

# ---- playbooks ----
for p in sorted(glob.glob("playbooks/*.md")):
    check_doc(p, PLAYBOOK, "playbook")

# ---- roles ----
used_playbooks = set()
role_dirs = sorted(d for d in glob.glob("roles/*/") if os.path.exists(d + "role.yaml"))
for d in role_dirs:
    name = os.path.basename(d.rstrip("/"))
    r = yaml.safe_load(open(d + "role.yaml"))
    for k in ROLE_KEYS:
        if k not in r: bad(f"{name}: missing {k}")
    for t in r.get("tools", []):
        if t not in catalog: bad(f"{name}: tool {t} not in catalog")
    for o in r.get("outputs", []):
        if o not in schemas: bad(f"{name}: output {o} has no schema")
        if not any(catalog.get(t, {}).get("submits") == o for t in r.get("tools", [])): bad(f"{name}: no tool submits {o}")
    text = open(d + r["prompt"]).read()
    for sec in SECTIONS:
        if f"## {sec}\n" not in text: bad(f"{name}: prompt missing section '{sec}'")
    check_evals(name, yaml.safe_load(open(d + r["evals"])))
    check_doc(d + r.get("handbook", "handbook.md"), HANDBOOK, "handbook")
    for pb in r.get("playbooks", []):
        used_playbooks.add(pb)
        if not os.path.exists(pb): bad(f"{name}: playbook {pb} missing")
print(f"ok    {len(role_dirs)} roles checked: tools in catalog, outputs have schemas and submitting tools, prompts, evals, handbooks, playbooks")

for p in glob.glob("playbooks/*.md"):
    if p not in used_playbooks: bad(f"{p}: no role uses this playbook")
print(f"ok    {len(glob.glob('playbooks/*.md'))} playbooks: sections present, each used by a role")
print(f"\n{fail} failure(s)"); sys.exit(1 if fail else 0)
