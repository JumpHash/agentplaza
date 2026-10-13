"""Synthetic rung-predictor data for tests and smoke runs (never for validation of a real model).

    python -m apz_train.synthetic --out DIR --n 4000 --frozen-fraction 0.05 --seed 0

Writes DIR/tasks.jsonl (every row, frozen ones included, so the recipe must exclude them) and
DIR/frozen.jsonl (the frozen set). The label is a noisy latent score mapped to a rung."""

import argparse
import json
import math
import os
import random

KINDS = {"qa": 0.0, "code": 0.8, "agentic": 1.6}


def generate(n, seed=0, start_ts=1_700_000_000):
    rng = random.Random(seed)
    rows = []
    for i in range(n):
        kind = rng.choice(list(KINDS))
        tokens = max(8, int(rng.lognormvariate(6.0, 0.8)))
        tools = rng.randint(0, 3)
        failures = rng.choice([0, 0, 0, 1, 2])
        latent = 0.9 * math.log1p(tokens) / 4 + 0.5 * tools + 1.2 * failures + KINDS[kind] + rng.gauss(0, 0.4)
        label = "cheap" if latent < 2.0 else ("mid" if latent < 3.0 else "frontier")
        rows.append({
            "task_id": f"t{i:06d}", "ts": start_ts + 60 * i,
            "task_kind": kind, "domain": rng.choice(["legal", "finance", "general", "infra"]),
            "tenant_tier": rng.choice(["free", "pro", "enterprise"]),
            "requested_model": rng.choice(["small", "large"]),
            "prompt_chars": tokens * 4, "prompt_tokens": tokens, "tool_count": tools,
            "history_turns": rng.randint(0, 20), "attachments": rng.randint(0, 2),
            "prior_failures": failures, "label": label,
        })
    return rows


def write(out_dir, n=4000, seed=0, frozen_fraction=0.05):
    os.makedirs(out_dir, exist_ok=True)
    rows = generate(n, seed)
    rng = random.Random(seed + 1)
    frozen = [r for r in rows if rng.random() < frozen_fraction]
    with open(os.path.join(out_dir, "tasks.jsonl"), "w", encoding="utf-8") as f:
        f.writelines(json.dumps(r) + "\n" for r in rows)
    with open(os.path.join(out_dir, "frozen.jsonl"), "w", encoding="utf-8") as f:
        f.writelines(json.dumps(r) + "\n" for r in frozen)
    return os.path.join(out_dir, "tasks.jsonl"), os.path.join(out_dir, "frozen.jsonl")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--out", required=True)
    p.add_argument("--n", type=int, default=4000)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--frozen-fraction", type=float, default=0.05)
    a = p.parse_args()
    print(json.dumps(dict(zip(("tasks", "frozen"), write(a.out, a.n, a.seed, a.frozen_fraction)))))
