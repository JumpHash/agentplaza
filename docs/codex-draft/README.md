# Codex draft

> Draft reference for the work engine. The built system is described in docs/architecture.md and docs/design/.

The live laws are the JHRL specs in `crates/jhrl/specs/`; the hub loads those, not this draft. Laws are checked on every tool call, and the owner decides the approvals.

- `roles/`: one directory per role (the builder and reviewer seats, planners, architects and the rest). Each role has a prompt, a handbook and evals; `roles/build/` generates them.
- `playbooks/`: the how-to method each role follows (for example `coding-standards.md`, `review-checklist.md`, `planning-guide.md`, `release-runbook.md`).
- `schemas/`: JSON Schema output contracts for everything a role hands in (task claims, plans, reviews, design notes, release plans, research briefs, audit and device reports, memory changes, training jobs).
- `tools/catalog.yaml`: every tool a role can use, its risk class and the policy action it is checked under; `submits` names the output schema for tools that hand in work.
- `flags.yaml`: the classifier and check flags the laws trigger on, and who may set each.
- `schema.cedarschema`: the policy schema draft (principals, resources and actions).
- `tests/check_roles.py`: cross-checks the schemas, tool catalog, playbooks and roles.

Check it:

```sh
pip install pyyaml jsonschema && python -I tests/check_roles.py
```
