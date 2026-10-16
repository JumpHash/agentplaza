#!/usr/bin/env python3
"""AgentPlaza acceptance runner — the four fixed suites plus the fleet,
treasury, social, agora and council suites.

```text
tests/acceptance/run.py [demo|redteam|killresume|budgets|fleet|treasury|social|agora|council|lifecycle|all] [options]
```

Design (the stated choice):

* **Python + the standard library, driving the real binaries over the real
  API.** The suites start their own ``noded`` and ``vsockd`` on ephemeral
  localhost ports in a private work directory, then speak HTTP to ``noded``
  and read the ``vsockd`` log; nothing links the crates and no dependency is
  added, so ``tests/acceptance`` is not a workspace member and
  ``cargo build --workspace`` is unaffected. The repository's other test trees
  follow the same shape (``docs/*/check.sh`` around a Python/Node suite).

* **Machine class is a flag.** ``--machine-class`` names one of the classes
  ``core.machines.class`` allows (``server``/``pi4``/``pi5``/``apple``). A
  suite whose class needs hardware this host does not have is reported
  ``BLOCKED`` with the reason — never skipped silently, never counted green.
  ``auto`` picks the class this host is, when that is unambiguous.

* **Every run carries a manifest.** Git SHA, VM image hashes, daemon binary
  hashes, host identity, kernel, seed, timestamps and artifact paths land in
  the results JSON under ``tests/acceptance/results/``.

Exit status: ``0`` all executed suites passed, ``1`` a suite failed,
``2`` blocked (a suite could not run here).
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import time
from pathlib import Path

# The suites live beside this file. `python3 -I` (isolated mode, which CI uses)
# does not put the script's directory on `sys.path`, so add it here rather than
# relying on the invocation, and keep the bytecode cache out of the repository
# so a CI run leaves the tree exactly as it found it.
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import apz
import suite_agora
import suite_budgets
import suite_council
import suite_demo
import suite_fleet
import suite_jail
import suite_killresume
import suite_lifecycle
import suite_redteam
import suite_social
import suite_treasury

SUITES = {
    "demo": suite_demo,
    "redteam": suite_redteam,
    "killresume": suite_killresume,
    "jail": suite_jail,
    "budgets": suite_budgets,
    "fleet": suite_fleet,
    "treasury": suite_treasury,
    "social": suite_social,
    "agora": suite_agora,
    "council": suite_council,
    "lifecycle": suite_lifecycle,
}


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="run.py",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("suite", choices=[*SUITES, "all"], help="which suite to run (default: all)")
    parser.add_argument(
        "--machine-class",
        default="auto",
        choices=[*apz.MACHINE_CLASSES, "auto"],
        help="the CI machine class this run is for (default: auto)",
    )
    parser.add_argument("--repo", default=str(apz.REPO), help="repository root")
    parser.add_argument("--bin-dir", default=str(apz.DEFAULT_BIN_DIR), help="where noded/vsockd live")
    parser.add_argument("--kernel", default=str(apz.DEFAULT_KERNEL), help="guest kernel image")
    parser.add_argument("--initrd", default=str(apz.DEFAULT_INITRD), help="guest initrd image")
    parser.add_argument(
        "--workspace-image",
        default=str(apz.DEFAULT_WORKSPACE),
        help=(
            "the base image of the guests' writable /workspace volume (the budgets suite's "
            "--workspace cells build it with scripts/mk-workspace-image.sh when it is absent)"
        ),
    )
    parser.add_argument("--results-dir", default=str(apz.RESULTS_DIR), help="where the results JSON goes")
    parser.add_argument("--artifacts", default=None, help="artifact root (default: a fresh mktemp -d; a non-empty one is refused: the harness needs a fresh work directory per run)")
    parser.add_argument("--keep", action="store_true", help="keep the artifact tree even on success")
    parser.add_argument(
        "--no-evidence",
        action="store_true",
        help="do not copy bounded artifact excerpts into results/evidence/",
    )
    parser.add_argument("--seed", default="apz-acceptance-v1", help="the fixed suite seed")
    parser.add_argument(
        "--soak",
        type=int,
        default=0,
        help="the lifecycle suite's idle-density case: hold this many idle keepalive agents (0 skips it)",
    )
    parser.add_argument(
        "--soak-seconds", type=float, default=60.0, help="how long the lifecycle soak holds its agents"
    )
    parser.add_argument(
        "--soak-sessions",
        type=int,
        default=0,
        help="the lifecycle suite's session case: one vsockd holds this many connected keepalive guests (no VMs; 0 skips it)",
    )
    parser.add_argument(
        "--timeout-scale",
        type=float,
        default=1.0,
        help="scale the per-run deadlines (slow machines)",
    )
    parser.add_argument(
        "--boot-p95-ms",
        type=float,
        default=None,
        help=(
            "the configured boot-time p95 bar in ms. No boot-time threshold is configured "
            "in this repository, so without this the boot component is measured and "
            "recorded but no PASS is asserted for it (see the budgets suite result)"
        ),
    )
    return parser.parse_args(argv)


def print_suite(suite: apz.Suite) -> None:
    counts = f"{sum(1 for case in suite.cases if case.ok)}/{len(suite.cases)} cases"
    print(f"[{suite.status}] {suite.name}  ({counts})")
    print(f"         bar: {suite.pass_bar}")
    if suite.blocked_reason:
        print(f"         BLOCKED: {suite.blocked_reason}")
    for note in suite.notes:
        print(f"         note: {note}")
    for case in suite.cases:
        if case.blocked_reason:
            # A blocked case is not a failed one: it never ran, and its tasks
            # are empty by construction, so there is nothing to report as a
            # failure. (A blocked case is still not a pass; the suite's status
            # makes that explicit.)
            print(f"         case BLOCKED: {case.name}")
            print(f"           reason: {case.blocked_reason}")
            continue
        if case.ok:
            continue
        print(f"         case FAILED: {case.name}")
        if case.vector:
            print(f"           vector: {case.vector}")
        for task in case.tasks:
            if not task["ok"]:
                print(f"           task      {task['name']}: {task['detail']}")
        for item in case.invariants:
            if not item["ok"]:
                print(f"           invariant {item['name']}: {item['detail']}")
    for finding in suite.findings:
        print(f"         finding: {finding['title']}")
        for key, value in finding.items():
            if key != "title":
                print(f"           {key}: {value}")


def gather_evidence(ctx: apz.Ctx, stamp: str) -> list[dict]:
    """Copy a bounded excerpt of every artifact into the committed evidence tree."""
    evidence_dir = ctx.results_dir / "evidence" / stamp
    evidence: list[dict] = []
    for path in ctx.artifacts:
        if not path.exists():
            continue
        ref = ctx.artifact_ref(path)
        name = f"{ref['sha256'][:8]}-{path.name}"
        (evidence_dir).mkdir(parents=True, exist_ok=True)
        target = evidence_dir / name
        target.write_bytes(ref["excerpt"].encode())
        evidence.append(
            {
                "source": ref["path"],
                "bytes": ref["bytes"],
                "sha256": ref["sha256"],
                "excerpt": str(target.relative_to(ctx.repo)) if _under(target, ctx.repo) else str(target),
                "excerpt_bytes": len(ref["excerpt"]),
            }
        )
    return evidence


def _under(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.machine_class == "auto":
        detected = apz.detect_machine_class()
        if detected is None:
            print("cannot auto-detect the machine class on this host: pass --machine-class", file=sys.stderr)
            return 2
        args.machine_class = detected
    try:
        ctx = apz.Ctx(args)
    except apz.LabError as err:
        # A refused work directory (or another invocation-level refusal) is not a
        # suite result: nothing ran, so this is BLOCKED, with the reason.
        print(str(err), file=sys.stderr)
        return 2
    ctx.results_dir.mkdir(parents=True, exist_ok=True)

    names = list(SUITES) if args.suite == "all" else [args.suite]
    print(f"== AgentPlaza acceptance — machine class {ctx.machine_class} "
          f"({ctx.host_os}/{ctx.host_arch}) on {ctx.artifacts_dir} ==")
    overall_started = time.monotonic()
    # The wall-clock instant the matrix began, for the kernel's own seccomp
    # audit window (`journalctl -k --since`): the soak below reads it back.
    run_started_epoch = time.time()
    # The binary the suites spawn their daemons from, before any of them runs:
    # compared against the same digests afterwards, so a `cargo build` that
    # rewrites the shared target while the matrix runs cannot quietly hand the
    # tail suites a different SUT (see `apz.binary_digest_changes`).
    bins_before = apz.binary_digests(ctx.bin_dir)
    # The SUT revision at the start, alongside the end one the manifest
    # records: a sibling's commit landing mid-run moves HEAD under a run whose
    # binaries were built from the earlier revision, and the two must not be
    # confused. The *binary digest* is the authoritative SUT identity; this is
    # context for it.
    sut_at_start = apz.git_revision()
    for name in names:
        module = SUITES[name]
        started = time.monotonic()
        if name == "budgets":
            suite = module.run(ctx, boot_p95_ms=args.boot_p95_ms)
        elif name == "lifecycle":
            suite = module.run(
                ctx, soak=args.soak, soak_seconds=args.soak_seconds, soak_sessions=args.soak_sessions
            )
        else:
            suite = module.run(ctx)
        suite.duration_s = time.monotonic() - started
        ctx.suites.append(suite)
        print_suite(suite)

    # The matrix is itself a jail soak: every suite above booted its VMs with
    # the jail armed (the shipped default), so the kernel's audit trail for this
    # window is the soak's evidence. A `type=1326` record for one of this port's
    # VMM threads is a daemon that reached a syscall its allowlist did not
    # cover; under the shipped `kill` arm that is a *death*, and the run's
    # verdict is FAIL whatever the suites' cases said — a jail that kills the
    # daemon is the defect to fix, never a flake.
    audit = apz.kernel_seccomp_audit(run_started_epoch)
    audit_path = ctx.artifacts_dir / "kernel-seccomp-audit.log"
    if audit["available"]:
        audit_path.write_text("\n".join(audit["lines"]) + "\n")
        ctx.register_artifact(audit_path)
    attributed = [
        line for line in audit.get("lines", []) if apz.vmm_audit_attribution(line, ctx.bin_dir)
    ]
    kills = [line for line in attributed if "code=0x80000000" in line]
    if kills:
        ctx.findings.append(
            {
                "suite": "(matrix soak)",
                "title": (
                    "a VMM thread hit a syscall the jail's allowlist does not cover "
                    "(SECCOMP_RET_KILL_PROCESS)"
                ),
                "classification": "product-defect",
                "detail": "; ".join(kills),
                "evidence": str(audit_path),
                "suspected_component": "crates/vmm/src/jail (the allowlist)",
            }
        )

    # A binary rewritten while the matrix ran means the suites did not all test
    # the same SUT: the run is a FAILURE with both digests, never a verdict
    # about "the build".
    bins_after = apz.binary_digests(ctx.bin_dir)
    bin_changes = apz.binary_digest_changes(bins_before, bins_after)
    if bin_changes:
        ctx.findings.append(
            {
                "suite": "(matrix)",
                "title": "the SUT binary changed while the matrix ran (the run tested two builds)",
                "classification": "infrastructure-defect",
                "detail": "; ".join(f"{name}: {old} -> {new}" for name, (old, new) in bin_changes.items()),
                "suspected_component": "a concurrent `cargo build` of the shared bin dir",
            }
        )

    stamp = f"{ctx.machine_class}-{ctx.stamp}"
    evidence = [] if args.no_evidence else gather_evidence(ctx, stamp)
    verdict = "PASS"
    if any(suite.status == "FAIL" for suite in ctx.suites):
        verdict = "FAIL"
    elif any(suite.status == "BLOCKED" for suite in ctx.suites):
        verdict = "BLOCKED"
    if kills or bin_changes:
        verdict = "FAIL"

    document = {
        "acceptance": "AgentPlaza MVP",
        "verdict": verdict,
        "machine_class": ctx.machine_class,
        "generated_at": apz.iso_now(),
        "wall_seconds": round(time.monotonic() - overall_started, 3),
        "manifest": ctx.manifest(args.suite, canary_planted=ctx.planted_canaries(), evidence=evidence),
        "suites_run": names,
        "kernel_seccomp_audit": {
            "available": audit["available"],
            "reason": audit.get("reason"),
            "since_epoch": audit.get("since_epoch"),
            "records": len(audit.get("lines", [])),
            "attributed": attributed,
            "kills": kills,
            "artifact": str(audit_path) if audit["available"] else None,
        },
        "sut_binary": {
            "bin_dir": str(ctx.bin_dir),
            "digests": bins_before,
            "changed_mid_run": bin_changes,
            "sut_git_at_start": sut_at_start,
        },
        "suites": [suite.view() for suite in ctx.suites],
        "findings": ctx.findings,
    }
    suffix = "" if args.suite == "all" else f"-{args.suite}"
    out = ctx.results_dir / f"acceptance-{ctx.machine_class}{suffix}-{time.strftime('%Y-%m-%d')}.json"
    out.write_text(json.dumps(document, indent=2) + "\n")

    print()
    if bin_changes:
        print(f"WARNING: the SUT binary changed while the matrix ran: {bin_changes}")
    if sut_at_start["head"] != apz.git_revision()["head"]:
        print(
            "NOTE: the repository HEAD moved during the run "
            f"({sut_at_start['head'][:12]} -> {apz.git_revision()['head'][:12]}); the binaries "
            f"under test came from before the move (see sut_binary.digests)"
        )
    if kills:
        print(f"WARNING: the kernel logged {len(kills)} seccomp refusal(s) for this port's VMM: {kills[0]}")
    for suite in ctx.suites:
        print(f"{suite.status}\t{suite.name}")
    print(f"verdict: {verdict}")
    print(f"results: {out}")
    if evidence:
        print(f"evidence: {ctx.results_dir / 'evidence' / stamp}")
    if ctx.keep:
        print(f"artifacts kept at: {ctx.artifacts_dir}")
    else:
        shutil.rmtree(ctx.artifacts_dir, ignore_errors=True)

    return {"PASS": 0, "FAIL": 1, "BLOCKED": 2}[verdict]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
