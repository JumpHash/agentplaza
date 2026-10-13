import json
from pathlib import Path

import pytest

from apz_train import export, recipes
from conftest import TRAINER, make_data, run_cli, write_job, write_plan

ROLES = ["orchestrator", "builder", "plaza"]
RECIPE_DIR = Path(TRAINER) / "recipes" / "rung_predictor"


def _meta():
    return json.loads((RECIPE_DIR / "recipe.json").read_text(encoding="utf-8"))


def _recipe_without_hook(tmp_path):
    """A copy of the bundled recipe with the serving_schema() hook cut off the end of recipe.py."""
    dst = tmp_path / "recipes" / "rung_predictor"
    dst.mkdir(parents=True, exist_ok=True)
    text = (RECIPE_DIR / "recipe.py").read_text(encoding="utf-8")
    assert "\ndef serving_schema" in text
    (dst / "recipe.py").write_text(text[:text.index("\ndef serving_schema")], encoding="utf-8")
    (dst / "recipe.json").write_text((RECIPE_DIR / "recipe.json").read_text(encoding="utf-8"), encoding="utf-8")
    return str(tmp_path / "recipes")


def test_serving_block_matches_the_recipe_declaration():
    meta = _meta()
    names = meta["input"]["numeric"] + meta["input"]["categorical"]
    block = export.serving_block(recipes.load("rung-predictor@1"))

    assert block["allowed_roles"] == ROLES
    assert [f["name"] for f in block["features"]] == names
    assert [f["kind"] for f in block["features"]] == ["numeric"] * 6 + ["hash"] * 4
    assert all(f["buckets"] == meta["input"]["hash_buckets"] for f in block["features"][6:])
    assert block["input"]["type"] == "object"
    assert set(block["input"]["properties"]) == set(names)
    assert block["input"]["required"] == names
    assert block["output"]["type"] == "object"
    assert block["output"]["properties"]["label"] == {"type": "string", "enum": ["cheap", "mid", "frontier"]}
    assert block["output"]["required"] == ["label", "probabilities"]
    assert block["labels"] == ["cheap", "mid", "frontier"]


def test_exported_card_carries_the_serving_block(tmp_path):
    tasks, frozen = make_data(tmp_path, n=800)
    job = write_job(tmp_path / "job.json")
    plan = write_plan(tmp_path / "plan.json", str(tmp_path / "out"), tasks, frozen,
                      train={"steps": 40, "eval_every": 0})
    rc, events, err = run_cli(["--job", job, "--plan", plan])
    assert rc == 0, err
    done = [e for e in events if e["ev"] == "done"][0]
    card = json.loads(Path(done["model_card_path"]).read_text(encoding="utf-8"))

    assert card["serving"] == json.loads(json.dumps(export.serving_block(recipes.load("rung-predictor@1"))))
    assert card["serving"]["allowed_roles"] == ROLES
    assert [f["name"] for f in card["serving"]["features"]] == _meta()["input"]["numeric"] + _meta()["input"]["categorical"]


def test_recipe_without_serving_hook_fails_export(tmp_path):
    rp = recipes.load("rung-predictor@1", _recipe_without_hook(tmp_path))
    assert rp.serving_schema() is None
    with pytest.raises(export.ServingError, match="declares no serving_schema"):
        export.serving_block(rp)


def test_run_refuses_a_recipe_without_serving_hook(tmp_path):
    tasks, frozen = make_data(tmp_path, n=300)
    out = tmp_path / "out"
    job = write_job(tmp_path / "job.json")
    plan = write_plan(tmp_path / "plan.json", str(out), tasks, frozen)
    rc, events, _ = run_cli(["--job", job, "--plan", plan, "--recipes", _recipe_without_hook(tmp_path)])
    assert rc == 1
    assert events[-1]["ev"] == "error" and "serving_schema" in events[-1]["message"]
    assert not (out / "model_card.json").exists()
