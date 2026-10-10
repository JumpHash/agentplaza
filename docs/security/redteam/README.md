# Red-team attack catalog

- **Status:** draft
- **Files:** [catalog.yaml](catalog.yaml) (60 attacks), [check_catalog.py](check_catalog.py) (validator)
- **Sources:** [threat model](../../design/threat-model.md); decisions 0105–0108, 0070, 0010, 0028, 0072, 0096, 0102, 0116; the Codex, the Cedar checks, role evals, the TLA+ models and the protocol and ledger tests.

## Purpose

The threat model names six attacker classes and the controls against each. This catalog turns
that into a checklist an attacker would use: one entry per concrete attack, each pointing at the
decisions, laws, design notes and tripwires meant to stop it, and at the tests that prove those
defences hold. It exists so we can answer two questions honestly: *is every attacker class
covered?* and *where is a control claimed but not actually tested?* Gaps are first-class entries,
not omissions; an honest gap is more useful than a defence that only looks real.

## How to read an entry

Each attack has an `id` (RT-NNN), a `title`, the `attacker` class (keys in the `attackers:`
header), the `boundary` crossed (keys in `boundaries:`), a concrete `technique`, the `impact`,
a `severity`, and the `milestone` at which the target surface first exists.

- **defences**: a list of `{kind, ref, how}`. `kind` is `decision` (a 4-digit ADR), `law`
  (a Codex `@law`/`@clause` id such as `C-002a` or `S-016b`), `design` (a design doc, optionally
  `#heading-slug`), or `tripwire` (a node tripwire from 0072). `how` is one sentence.
- **tests**: a list of `{kind, ref, how}`. `kind` is `cedar` (a substring of a label in
  `codex-draft/tests/check.sh`), `eval` (`<role>/<scenario-id>` in `codex-draft/roles/*/evals.yaml`),
  `tla` (an invariant, property, mutant or witness in `docs/formal`), `protocol` (a check label in
  `test_protocol.py`), `ledger` (a test name in `ledger.test.mjs`), or `planned` (a test to write
  when the code exists, named by a slug).
- **status**: `covered`, `planned-only` or `gap` (defined below). Gaps and planned-only entries
  carry a one-line `next`; gaps also carry a `gap_note` explaining why nothing stops the attack.

## How coverage is computed

The validator fixes the meaning of each status so the label can't drift:

- **covered** — at least one non-planned test (one that exists and runs today) *and* at least one
  "strong" defence (a `decision`, `law` or `tripwire`, i.e. something enforced, not just described).
- **planned-only** — a strong defence exists, but every test is `planned`: the control is decided,
  the proof is not yet written (usually because the code doesn't exist). Needs a `next`.
- **gap** — no strong defence. Only `design` notes (or nothing) stand between the attacker and the
  impact, and there are no real tests. Needs a `gap_note` and a `next`. Every `critical` gap must
  carry a `gap_note`.

`check_catalog.py` also checks the schema of every entry, unique ids, that all six attacker classes
are used, that every referenced decision/law/test actually resolves, and that each required coverage
tag is present. Run it until it prints `0 failure(s)`; fix the catalog, never the checks:

```
python3 -I docs/security/redteam/check_catalog.py
```

It locates the repo from its own path, needs only `pyyaml`, and exits non-zero on any failure.

## Coverage

```
| attacker   |      covered | planned-only |          gap |        total |
|------------|--------------|--------------|--------------|--------------|
| injected   |           22 |            3 |            1 |           26 |
| machine    |            4 |            0 |            1 |            5 |
| supply     |            5 |            0 |            1 |            6 |
| relay      |            0 |            1 |            2 |            3 |
| device     |            1 |            0 |            1 |            2 |
| insider    |           17 |            0 |            1 |           18 |
| all        |           49 |           11 |            0 |           60 |
```
The strongest coverage is where real tests already exist: Cedar authorisation, the TLA+ session,
approval and economy models, and the protocol and ledger test suites. The relay class has no
`covered` entries because the relay (M2) and its end-to-end encryption are not built yet.

## Gaps (no decided defence)

None. The seven gaps found when the catalog was first written (RT-009, RT-017, RT-034, RT-036, RT-037, RT-038, RT-042) were decided by the owner in round 35 ([0128](../../decisions/0128-red-team-gaps-closed.md)) and are now planned-only.

## Planned-only (defence decided, test still to write)

- **RT-008 egress shim bypass** — *write a test asserting the booted guest has no network device or route.*
- **RT-012 vsock stream exhaustion** — *implement and test per-VM stream concurrency caps in the node daemon.*
- **RT-035 relay MITM** — *implement the Noise handshake (0106) and test that the relay cannot read or alter frames.*
- **RT-045 VMM escape via a virtio bug** — *stand up cargo-fuzz targets for each virtio parser and gate releases on them.*

## What building the catalog found

- **Constitution clauses without Cedar tests.** C-001b (tamper signals), C-001c (obfuscation) and C-003a/b (impersonation) had no checks in `codex-draft/tests/check.sh`. Writing them exposed two missing rights: no policy let an agent set its own name (0045), and no policy allowed ordinary sandbox events, so every event, not only tamper signals, read as a violation. S-000d and S-000e add them, and seven new checks cover the clauses (0126). RT-026 is now `covered` for the law; its detection tests are still planned.
- **Egress attribution.** Non-allowlisted egress is stopped by the role's default deny (e.g. S-001b), not by a C-002 forbid; C-002b applies only when a request is flagged as credential-seeking. The egress-storm tripwire is evidence, not an offense in itself.

## Red-team cadence (0108)

- **Every release:** the fixed credential red-team and injection suite (0027, 0040) runs on all three
  platforms. Zero leaks are required, with no tolerance (0070).
- **Weekly:** a run against the live setup with fresh scenarios, generated by an agent and reviewed by
  the owner before they run. Any new scenario that finds something joins the fixed suite.
- Red-team spend is metered like any task and reported weekly.

When a weekly run finds a new attack, add it here as an RT-NNN entry (status `gap` or `planned-only`
until a defence and a test exist), and keep the validator at `0 failure(s)`.
