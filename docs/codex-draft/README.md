# Codex draft

> Draft doctrine; the built system is described in docs/architecture.md and docs/design/.

A planning-time draft of the Codex. The real Codex is its own git repository on the hub, where only owner-signed commits take effect (0049).

- `schema.cedarschema`: the hub's policy schema, v0
- `constitution/`: the four Constitution articles (0051), written as annotated Cedar (0048)
- `flags.yaml`: registry of flags laws trigger on, and who may set them (0118)
- `sanctions.yaml`: sanction types and the conduct schedule (0118)
- `statutes/`: S-100–S-106 conduct laws (0118); S-000 every agent; S-009–S-016 roles v1.1 (0117); S-001 builder (0071); S-002 reviewer, S-003 planner, S-004 Mayor, S-005 departments (0080); S-006 breeder, S-007 legal, S-008 council seats (0113)
- `roles/`: thirteen roles plus the council-seat overlay, generated from `roles/build/` (0113)
- `playbooks/`: 17 best-practice playbooks (0120)
- `roles/*/handbook.md`: one handbook per role (0120)
- `schemas/`: output contracts (JSON Schema) for everything roles hand in
- `tools/catalog.yaml`: every tool, its risk class and Cedar action
- `contracts/`: example per-case grant (contract tier)
- `tests/`: `check.sh` (131 ALLOW/DENY decisions), `check_codex.py` (law metadata, flags, sanctions), `check_roles.py` (roles, tools, statutes, schemas, evals)

Check it with the Cedar CLI:

```sh
cargo install cedar-policy-cli && ./tests/check.sh
pip install pyyaml jsonschema && python -I tests/check_roles.py && python -I tests/check_codex.py
```
