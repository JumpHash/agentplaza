import os

import numpy as np

from apz_train import wire
from conftest import make_data, run_cli, write_job, write_plan


def test_single_converges_and_onnx_reproduces(tmp_path):
    tasks, frozen = make_data(tmp_path)
    out = str(tmp_path / "out")
    job = write_job(tmp_path / "job.json")
    plan = write_plan(tmp_path / "plan.json", out, tasks, frozen, train={"steps": 400})
    rc, events, err = run_cli(["--job", job, "--plan", plan, "--dump-weights"])
    assert rc == 0, err
    assert any(e["ev"] == "step" for e in events)
    assert any(e["ev"] == "eval" for e in events)
    done = [e for e in events if e["ev"] == "done"]
    assert len(done) == 1
    val = done[0]["metrics"]["val"]
    assert val["accuracy"] > 0.7, val
    assert val["expected_cost"] <= val["cascade_cost"], val
    assert done[0]["onnx_check"]["max_abs_diff"] < 1e-4
    card = done[0]["model_card"]
    assert card["data"]["train"]["excluded_frozen"] > 0
    assert card["metrics"]["frozen"]["n"] == len(open(frozen).readlines())
    assert os.path.getsize(done[0]["onnx"]) > 0
    assert all(e["rank"] == 0 for e in events)


def test_grid_search_runs_trials_and_exports_best(tmp_path):
    tasks, frozen = make_data(tmp_path, n=1500)
    out = str(tmp_path / "out")
    job = write_job(tmp_path / "job.json", search={"method": "grid", "trials": 2,
                                                  "space": {"lr": [0.001, 0.003], "hidden": 32}})
    plan = write_plan(tmp_path / "plan.json", out, tasks, frozen, train={"steps": 60, "eval_every": 0})
    rc, events, err = run_cli(["--job", job, "--plan", plan])
    assert rc == 0, err
    trials = [e for e in events if e["ev"] == "trial"]
    assert len(trials) == 2
    done = [e for e in events if e["ev"] == "done"][0]
    assert done["model_card"]["search"]["best_params"]["lr"] in (0.001, 0.003)
    assert os.path.isfile(done["onnx"])


def test_pbt_runs_population(tmp_path):
    tasks, frozen = make_data(tmp_path, n=1500)
    out = str(tmp_path / "out")
    job = write_job(tmp_path / "job.json", search={"method": "pbt", "trials": 4,
                                                  "space": {"lr": {"low": 0.0005, "high": 0.01, "log": True}}})
    plan = write_plan(tmp_path / "plan.json", out, tasks, frozen, train={"steps": 60, "eval_every": 0})
    rc, events, err = run_cli(["--job", job, "--plan", plan])
    assert rc == 0, err
    assert len([e for e in events if e["ev"] == "trial"]) >= 4
    assert [e for e in events if e["ev"] == "done"]


def test_bad_recipe_reports_error_event(tmp_path):
    tasks, frozen = make_data(tmp_path, n=300)
    job = write_job(tmp_path / "job.json")
    job_obj = open(job).read().replace("rung-predictor@1", "../etc@1")
    open(job, "w").write(job_obj)
    plan = write_plan(tmp_path / "plan.json", str(tmp_path / "out"), tasks, frozen)
    rc, events, _ = run_cli(["--job", job, "--plan", plan])
    assert rc == 1
    assert events[-1]["ev"] == "error" and "bad recipe id" in events[-1]["message"]
