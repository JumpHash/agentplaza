"""Writes trainer/tests/fixtures/featurize_parity.json: the card's serving block plus 8 rows, each with
the features the Python featurizer (recipe.encode) produces. The Rust hub must reproduce these.

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


def main():
    rp = recipes.load("rung-predictor@1")
    mod = rp.mod
    block = json.loads(json.dumps(export.serving_block(rp)))
    rows = []
    for i in range(8):
        inp = {}
        for j, k in enumerate(mod.NUM):
            inp[k] = NUM_POOL[(i + j) % len(NUM_POOL)]
        for j, k in enumerate(mod.CAT):
            inp[k] = CAT_POOL[(i + j) % len(CAT_POOL)]
        num, cat = mod.encode(inp)
        rows.append({"input": inp, "features": num + [float(c) for c in cat]})
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump({"serving": block, "rows": rows}, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {OUT} ({len(rows)} rows)")


if __name__ == "__main__":
    main()
