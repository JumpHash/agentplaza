"""Parity between the featurizer and the card's serving spec (the contract the Rust hub implements).

tests/fixtures/featurize_parity.json (written by tests/make_parity_fixture.py) holds the serving block
and rows with the features the Python featurizer produced: regular rows, then edge rows (a missing or
null categorical, a missing, string or null numeric, a boolean numeric). These tests check that the
current code still produces them, that the card spec alone reproduces them, that the edge rows follow
the trainer's missing rules, and that the block is the one the recipe declares.
"""

import copy
import hashlib
import json
import math
from pathlib import Path

import pytest

from apz_train import export, recipes
from conftest import TRAINER

FIXTURE = Path(TRAINER) / "tests" / "fixtures" / "featurize_parity.json"
TOL = 1e-12
# The edge rows make_parity_fixture.py writes, one per case.
EDGE_CASES = [
    "missing hash field",
    "None hash field",
    "missing numeric field",
    "string where a number is expected",
    "JSON null numeric",
    "boolean numeric",
]


def _fixture():
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


def _from_card(spec, row):
    """Features computed from the card's serving spec alone, with the rules the Rust side follows:
    a numeric takes an int, float or bool as its number and anything else as 0; a hash field that is
    missing or null is the bucket of "__missing__"."""
    out = []
    for f in spec["features"]:
        name = f["name"]
        if f["kind"] == "numeric":
            assert f["transform"] == "signlog"
            v = row.get(name)
            x = float(v) if isinstance(v, (int, float)) else 0.0
            out.append(math.copysign(math.log1p(abs(x)), x))
        else:
            assert f["kind"] == "hash" and f["hash"] == "blake2b" and f["input"] == "key=value"
            v = row.get(name)
            text = "__missing__" if v is None else v
            digest = hashlib.blake2b(f"{name}={text}".encode("utf-8"), digest_size=f["digest_size"]).digest()
            out.append(float(int.from_bytes(digest, f["byte_order"]) % f["buckets"]))
    return out


def test_fixture_has_eight_rows_plus_the_edge_rows_with_one_feature_per_card_entry():
    fx = _fixture()
    assert len(fx["rows"]) == 8 + len(EDGE_CASES)
    assert [r["case"] for r in fx["rows"] if "case" in r] == EDGE_CASES
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


def test_edge_rows_follow_the_trainer_missing_rules():
    fx = _fixture()
    mod = recipes.load("rung-predictor@1").mod
    rows = {r["case"]: r["input"] for r in fx["rows"] if "case" in r}
    assert set(rows) == set(EDGE_CASES)
    hashes = [f["name"] for f in fx["serving"]["features"] if f["kind"] == "hash"]
    nums = [f["name"] for f in fx["serving"]["features"] if f["kind"] == "numeric"]
    # The fields really are missing, null, a string and a bool, as the case names say.
    assert hashes[0] not in rows["missing hash field"]
    assert rows["None hash field"][hashes[-1]] is None
    assert nums[0] not in rows["missing numeric field"]
    assert isinstance(rows["string where a number is expected"][nums[1]], str)
    assert rows["JSON null numeric"][nums[2]] is None
    assert rows["boolean numeric"][nums[-1]] is True
    # The trainer's encode gives them the missing bucket, 0.0, and 1.0 for a true bool.
    assert mod.encode(rows["missing hash field"])[1][0] == mod.hash_index(hashes[0], mod.HASH_MISSING)
    assert mod.encode(rows["None hash field"])[1][-1] == mod.hash_index(hashes[-1], mod.HASH_MISSING)
    assert mod.encode(rows["missing numeric field"])[0][0] == 0.0
    assert mod.encode(rows["string where a number is expected"])[0][1] == 0.0
    assert mod.encode(rows["JSON null numeric"])[0][2] == 0.0
    assert abs(mod.encode(rows["boolean numeric"])[0][-1] - math.log(2)) <= TOL


def test_fixture_serving_block_matches_recipe_declaration():
    fx = _fixture()
    rp = recipes.load("rung-predictor@1")
    assert fx["serving"] == json.loads(json.dumps(export.serving_block(rp)))
    assert fx["serving"]["output_activation"] == "logits"


def test_fixture_rows_split_inputs_by_kind():
    fx = _fixture()
    feats = fx["serving"]["features"]
    num_idx = [i for i, f in enumerate(feats) if f["kind"] == "numeric"]
    hash_idx = [i for i, f in enumerate(feats) if f["kind"] == "hash"]
    for row in fx["rows"]:
        assert row["inputs"]["num"] == [row["features"][i] for i in num_idx]
        assert row["inputs"]["cat"] == [int(row["features"][i]) for i in hash_idx]
        assert all(isinstance(c, int) for c in row["inputs"]["cat"])


def test_serving_block_lists_the_two_graph_inputs_in_graph_order():
    fx = _fixture()
    mod = recipes.load("rung-predictor@1").mod
    assert fx["serving"]["inputs"] == [
        {"name": "num", "dtype": "float32", "features": list(mod.NUM)},
        {"name": "cat", "dtype": "int64", "features": list(mod.CAT)},
    ]


class _StubRecipe:
    """A recipe whose serving_schema() is the real one, with a test's edit applied to it."""
    id = "rung-predictor@1"

    def __init__(self, decl, example):
        self._decl = decl
        self._example = example

    def serving_schema(self):
        return copy.deepcopy(self._decl)

    def example_input(self):
        return self._example


def _stub_recipe(mutate):
    mod = recipes.load("rung-predictor@1").mod
    decl = mod.serving_schema()
    mutate(decl)
    return _StubRecipe(decl, mod.example_input())


def test_export_refuses_a_feature_in_no_input():
    def drop_last_hash_from_cat(decl):
        decl["inputs"][1]["features"] = decl["inputs"][1]["features"][:-1]
    with pytest.raises(export.ServingError, match="is in 0 inputs"):
        export.serving_block(_stub_recipe(drop_last_hash_from_cat))


def test_export_refuses_a_feature_in_two_inputs():
    def copy_numeric_into_cat(decl):
        decl["inputs"][1]["features"].append(decl["inputs"][0]["features"][0])
    with pytest.raises(export.ServingError, match="is in 2 inputs"):
        export.serving_block(_stub_recipe(copy_numeric_into_cat))
