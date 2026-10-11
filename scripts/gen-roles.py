#!/usr/bin/env python3
"""Regenerate docs/agents/initial-roles.json from the planning repo's role.yaml files.

Usage: python3 scripts/gen-roles.py <planning-repo>

Builds one manifest entry per planning role (the 15 in PLANNING_ROLES), keeps
the existing orchestrator entry unchanged, drops every other role, and copies
each role's prompt.md into docs/codex-draft/roles/<role>/prompt.md.
"""
import json
import pathlib
import shutil
import sys

import yaml

PLANNING_ROLES = [
    "architect",
    "auditor",
    "builder",
    "edge-operator",
    "finance",
    "incident",
    "librarian",
    "mayor",
    "mayor-office",
    "planner",
    "qa",
    "release",
    "researcher",
    "reviewer",
    "security-reviewer",
]
SOURCE = "planning repo drafts/codex/roles (role.yaml), plus orchestrator"

ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "docs" / "agents" / "initial-roles.json"
PROMPTS = ROOT / "docs" / "codex-draft" / "roles"


def budget_of(src):
    budget = src.get("budget", 0)
    if isinstance(budget, dict):
        return budget.get("default_credits", 0)
    return budget


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    planning = pathlib.Path(sys.argv[1]) / "drafts" / "codex" / "roles"
    current = json.loads(MANIFEST.read_text())
    old_roles = current["roles"]

    roles = {}
    for name in PLANNING_ROLES:
        src = yaml.safe_load((planning / name / "role.yaml").read_text())
        if src.get("role") != name:
            sys.exit(f"{name}: role.yaml declares role {src.get('role')!r}")
        old = old_roles.get(name, {})
        roles[name] = {
            "role": name,
            "title": old.get("title") or name.replace("-", " ").capitalize(),
            "permits": list(src.get("permits") or []),
            "toolsets": list(src.get("toolsets") or []),
            "budget": budget_of(src),
            "variant": src.get("variant") or None,
            "tools": list(src.get("tools") or []),
        }
        dest = PROMPTS / name
        dest.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(planning / name / "prompt.md", dest / "prompt.md")

    roles["orchestrator"] = old_roles["orchestrator"]
    MANIFEST.write_text(json.dumps({"source": SOURCE, "roles": roles}, indent=1) + "\n")
    print(f"wrote {len(roles)} roles to {MANIFEST}")


if __name__ == "__main__":
    main()
