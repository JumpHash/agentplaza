"""rung-predictor@1: predicts the cheapest rung that passes, from task features.

Data: JSONL rows {task_id?, ts, <numeric features>, <categorical features>, label} where label is
0..2 or "cheap" | "mid" | "frontier" (the lowest rung that passed verification in the cascade).
Split: time-based (the newest `val_fraction` of tasks is validation). Tasks in the frozen set are
removed from train and validation, and the frozen set is scored only at the end (breeding guide).

Objective: expected cost. Routing a task to rung k when its true need is y costs
  price[k]                     if k >= y   (passes at k)
  sum(price[k..y])             if k <  y   (fails at k, escalates to y)
so loss = E_{k~softmax}[C[k, y]] + lambda_ce * CE, with prices normalised to the frontier rung.
"""

import hashlib
import json
import math
import os

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import Dataset

META = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "recipe.json"),
                      encoding="utf-8"))
NUM = META["input"]["numeric"]
CAT = META["input"]["categorical"]
BUCKETS = META["input"]["hash_buckets"]
CLASSES = META["output"]["classes"]
_NAMES = {name: i for i, name in enumerate(CLASSES)}
_CFG = {}  # set by build_model: prices, lambda_ce (the loss and metrics read them)


# Serving contract for the hashed categoricals: the card's "hash", "digest_size", "byte_order" fields
# are these constants, so the Rust hub and this code cannot drift apart.
HASH_ALGO = "blake2b"
HASH_DIGEST_SIZE = 8
HASH_BYTE_ORDER = "little"
HASH_MISSING = "__missing__"


def hash_index(key, value):
    digest = hashlib.new(HASH_ALGO, f"{key}={value}".encode("utf-8"), digest_size=HASH_DIGEST_SIZE).digest()
    return int.from_bytes(digest, byteorder=HASH_BYTE_ORDER) % BUCKETS


def numeric_feature(name):
    """Serving spec: value = sign(x) * ln(1 + |x|), as _signed_log computes it; 0 maps to 0."""
    return {"name": name, "kind": "numeric", "transform": "signlog"}


def hash_feature(name):
    """Serving spec: bucket = int.from_bytes(blake2b("key=value", digest_size=8), "little") % buckets."""
    return {"name": name, "kind": "hash", "hash": HASH_ALGO, "digest_size": HASH_DIGEST_SIZE,
            "buckets": BUCKETS, "input": "key=value", "byte_order": HASH_BYTE_ORDER}


def _signed_log(x):
    return math.copysign(math.log1p(abs(x)), x)


def encode(row):
    num = []
    for k in NUM:
        v = row.get(k)
        num.append(_signed_log(float(v)) if isinstance(v, (int, float)) else 0.0)
    cat = []
    for k in CAT:
        v = row.get(k)
        cat.append(hash_index(k, HASH_MISSING if v is None else v))
    return num, cat


def _label(v):
    return _NAMES[v] if isinstance(v, str) else int(v)


def _ts(v):
    if isinstance(v, str):
        from datetime import datetime
        return datetime.fromisoformat(v.replace("Z", "+00:00")).timestamp()
    return float(v)


def _key(row):
    return row.get("task_id") or hashlib.sha256(
        json.dumps({k: row.get(k) for k in NUM + CAT}, sort_keys=True).encode()).hexdigest()


class RungDataset(Dataset):
    def __init__(self, rows, provenance):
        self.num = torch.tensor([encode(r)[0] for r in rows], dtype=torch.float32).reshape(len(rows), len(NUM))
        self.cat = torch.tensor([encode(r)[1] for r in rows], dtype=torch.long).reshape(len(rows), len(CAT))
        self.label = torch.tensor([_label(r["label"]) for r in rows], dtype=torch.long)
        self.provenance = provenance

    def __len__(self):
        return len(self.label)

    def __getitem__(self, i):
        return {"num": self.num[i], "cat": self.cat[i], "label": self.label[i]}


def _read(paths):
    rows = []
    for p in paths:
        with open(p, encoding="utf-8") as f:
            rows.extend(json.loads(line) for line in f if line.strip())
    return rows


def dataset(paths, split, cfg):
    frozen_rows = _read([cfg["frozen"]]) if cfg.get("frozen") else []
    frozen_keys = {_key(r) for r in frozen_rows}
    if split == "frozen":
        return RungDataset(frozen_rows, {"split": "frozen", "rows": len(frozen_rows)})
    rows = sorted(_read(paths), key=lambda r: _ts(r["ts"]))
    kept = [r for r in rows if _key(r) not in frozen_keys]
    cut = int(len(kept) * (1.0 - float(cfg["val_fraction"])))
    part = kept[:cut] if split == "train" else kept[cut:]
    labels = [_label(r["label"]) for r in part]
    prov = {"split": split, "rows": len(part), "excluded_frozen": len(rows) - len(kept),
            "label_balance": {c: labels.count(i) for i, c in enumerate(CLASSES)},
            "ts_range": [min((_ts(r["ts"]) for r in part), default=None),
                         max((_ts(r["ts"]) for r in part), default=None)]}
    return RungDataset(part, prov)


class RungNet(nn.Module):
    def __init__(self, hidden, depth, emb):
        super().__init__()
        self.emb = nn.Embedding(BUCKETS, emb)
        layers, width = [], len(NUM) + len(CAT) * emb
        for _ in range(depth):
            layers += [nn.Linear(width, hidden), nn.GELU()]
            width = hidden
        layers.append(nn.Linear(width, len(CLASSES)))
        self.mlp = nn.Sequential(*layers)

    def forward(self, num, cat):
        return self.mlp(torch.cat([num, self.emb(cat).flatten(1)], dim=1))


def bind(cfg):
    _CFG.clear()
    _CFG.update(prices=[float(p) for p in cfg["prices"]], lambda_ce=float(cfg["lambda_ce"]))


def build_model(cfg):
    bind(cfg)
    return RungNet(int(cfg["hidden"]), int(cfg["depth"]), int(cfg.get("embedding_dim", META["input"]["embedding_dim"])))


def cost_matrix(prices):
    """C[k, y]: cost of routing to rung k when the task needs rung y, prices normalised to the max."""
    p = [x / max(prices) for x in prices]
    n = len(p)
    C = torch.zeros(n, n)
    for k in range(n):
        for y in range(n):
            C[k, y] = p[k] if k >= y else sum(p[k:y + 1])
    return C


def loss(batch, out):
    out = out.float()
    y = batch["label"]
    C = cost_matrix(_CFG["prices"]).to(out.device)
    probs = out.softmax(-1)
    expected = (probs * C[:, y].T).sum(-1).mean()
    return expected + _CFG["lambda_ce"] * F.cross_entropy(out, y)


def metrics(batch, out):
    y = batch["label"]
    prices = _CFG.get("prices", META["defaults"]["prices"])
    C = cost_matrix(prices)
    probs = out.softmax(-1)
    pred = probs.argmax(-1)
    conf = probs.max(-1).values
    correct = (pred == y).float()
    bins = torch.linspace(0, 1, 11)
    ece = 0.0
    for lo, hi in zip(bins[:-1], bins[1:]):
        m = (conf > lo) & (conf <= hi)
        if m.any():
            ece += m.float().mean().item() * abs(conf[m].mean().item() - correct[m].mean().item())
    return {
        "n": len(y),
        "expected_cost": C[pred, y].mean().item(),
        "cascade_cost": C[0, y].mean().item(),
        "always_frontier_cost": C[-1, y].mean().item(),
        "accuracy": correct.mean().item(),
        "under_route": (pred < y).float().mean().item(),
        "over_route": (pred > y).float().mean().item(),
        "ece": ece,
        "nll": F.cross_entropy(out, y).item(),
    }


def example_input():
    return (torch.zeros(1, len(NUM)), torch.zeros(1, len(CAT), dtype=torch.long))


def serving_schema():
    """The inference contract the registry imports (crates/apid/src/inference.rs). Input: the task
    features of the dataset rows, in card order. Output: the rung label and its class probabilities
    (the graph emits logits; the serving side applies softmax)."""
    props = {**{k: {"type": "number"} for k in NUM}, **{k: {"type": "string"} for k in CAT}}
    return {
        "input": {"type": "object", "properties": props, "required": NUM + CAT},
        "output": {
            "type": "object",
            "properties": {
                "label": {"type": "string", "enum": list(CLASSES)},
                "probabilities": {"type": "object"},
            },
            "required": ["label", "probabilities"],
        },
        "features": ([numeric_feature(k) for k in NUM] + [hash_feature(k) for k in CAT]),
        "labels": list(CLASSES),
        "output_activation": "logits",  # RungNet.forward returns logits; softmax is applied at serving
        "allowed_roles": ["orchestrator", "builder", "plaza"],
    }
