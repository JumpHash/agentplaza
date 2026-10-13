"""Writes trainer/tests/fixtures/featurize_parity.json: the card's serving block, 8 regular rows and the
edge rows (a missing or null categorical, a missing, string or null numeric, a boolean numeric), each
with the features the Python featurizer (recipe.encode) produces. The Rust hub must reproduce all of them.

Run: bash -c 'cd /mnt/fast/AgentPlaza/trainer && uv run python tests/make_parity_fixture.py'
"""

import json
import os
import sys

TRAINER = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, TRAINER)

from apz_train import export, recipes  # noqa: E402

OUT = os.path.join(TRAINER, "tests", "fixtures", "featurize_parity.json")
# Row i, numeric feature j takes NUM_POOL[(i + j) % 8]: every pool value reaches every feature.
NUM_POOL = [0, -3.5, 1000000.0, 1e-06, 0.25, -1000000.0, 7, -1e-06]
# Same rotation for categoricals; includes unicode, the empty string and a separator character.
CAT_POOL = ["chat", "code", "café", "日本語", "emoji 🙂", "a=b", "", "tenant-ß"]


def _at(seq, i):
    return seq[i % len(seq)]


def _input(mod, i):
    inp = {}
    for j, k in enumerate(mod.NUM):
        inp[k] = NUM_POOL[(i + j) % len(NUM_POOL)]
    for j, k in enumerate(mod.CAT):
        inp[k] = CAT_POOL[(i + j) % len(CAT_POOL)]
    return inp


def _row(mod, inp, case=None):
    num, cat = mod.encode(inp)
    row = {"input": inp, "features": num + [float(c) for c in cat], "inputs": {"num": num, "cat": cat}}
    if case:
        row["case"] = case
    return row


def _edge_rows(mod):
    """Missing and non-numeric values, each in its own row. The trainer's rules: a missing or None
    categorical is "__missing__"; a numeric that is not an int, float or bool is 0.0 (a bool is 1.0 or 0.0)."""
    inp = _input(mod, 0)
    del inp[_at(mod.CAT, 0)]
    yield "missing hash field", inp
    inp = _input(mod, 1)
    inp[_at(mod.CAT, -1)] = None
    yield "None hash field", inp
    inp = _input(mod, 2)
    del inp[_at(mod.NUM, 0)]
    yield "missing numeric field", inp
    inp = _input(mod, 3)
    inp[_at(mod.NUM, 1)] = "12.5"
    yield "string where a number is expected", inp
    inp = _input(mod, 4)
    inp[_at(mod.NUM, 2)] = None
    yield "JSON null numeric", inp
    inp = _input(mod, 5)
    inp[_at(mod.NUM, -1)] = True
    yield "boolean numeric", inp


def main():
    rp = recipes.load("rung-predictor@1")
    mod = rp.mod
    block = json.loads(json.dumps(export.serving_block(rp)))
    rows = [_row(mod, _input(mod, i)) for i in range(8)]
    rows += [_row(mod, inp, case) for case, inp in _edge_rows(mod)]
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump({"serving": block, "rows": rows}, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()
