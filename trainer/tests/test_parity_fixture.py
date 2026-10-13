"""Parity between the featurizer and the card's serving spec (the contract the Rust hub implements).

tests/fixtures/featurize_parity.json (written by tests/make_parity_fixture.py) holds the serving block
and rows with the features the Python featurizer produced. These tests check that the current code
still produces them, that the card spec alone reproduces them, and that the block is the one the
recipe declares.
"""

import hashlib
import json
import math
from pathlib import Path

from apz_train import export, recipes
from conftest import TRAINER

FIXTURE = Path(TRAINER) / "tests" / "fixtures" / "featurize_parity.json"
TOL = 1e-12


def _fixture():
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


def _from_card(spec, row):
    """Features computed from the card's serving spec alone, with the rules the Rust side follows."""
    out = []
    for f in spec["features"]:
        name = f["name"]
        if f["kind"] == "numeric":
            assert f["transform"] == "signlog"
            x = float(row[name])
            out.append(math.copysign(math.log1p(abs(x)), x))
        else:
            assert f["kind"] == "hash" and f["hash"] == "blake2b" and f["input"] == "key=value"
            digest = hashlib.blake2b(f"{name}={row[name]}".encode("utf-8"), digest_size=f["digest_size"]).digest()
            out.append(float(int.from_bytes(digest, f["byte_order"]) % f["buckets"]))
    return out


def test_fixture_has_eight_rows_with_one_feature_per_card_entry():
    fx = _fixture()
    assert len(fx["rows"]) == 8
    assert all(len(r["features"]) == len(fx["serving"]["features"]) for r in fx["rows"])


def test_python_featurizer_reproduces_fixture_features():
    fx = _fixture()
    mod = recipes.load("rung-predictor@1").mod
    for row in fx["rows"]:
        num, cat = mod.encode(row["input"])
        got = num + [float(c) for c in cat]
        assert len(got) == len(row["features"])
        for a, b in zip(got, row["features"]):
            assert abs(a - b) <= TOL


def test_card_spec_alone_reproduces_fixture_features():
    fx = _fixture()
    for row in fx["rows"]:
        got = _from_card(fx["serving"], row["input"])
        assert len(got) == len(row["features"])
        for a, b in zip(got, row["features"]):
            assert abs(a - b) <= TOL


def test_fixture_serving_block_matches_recipe_declaration():
    fx = _fixture()
    rp = recipes.load("rung-predictor@1")
    assert fx["serving"] == json.loads(json.dumps(export.serving_block(rp)))
    assert fx["serving"]["output_activation"] == "logits"
