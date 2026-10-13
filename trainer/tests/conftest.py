import json
import os
import subprocess
import sys

TRAINER = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TRAIN_CFG = {"steps": 300, "batch_size": 128, "lr": 0.003, "warmup": 20, "eval_every": 100,
             "checkpoint_every": 0, "log_every": 25}


def make_data(tmp_path, n=3000, seed=0):
    from apz_train import synthetic
    return synthetic.write(str(tmp_path / "data"), n=n, seed=seed, frozen_fraction=0.05)


def write_json(path, obj):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f)
    return str(path)


def write_job(path, search=None):
    job = {"job_id": "tj-test", "recipe": "rung-predictor@1", "datasets": ["blake3:synthetic"],
           "budget_gpu_hours": 1.0, "export": "onnx"}
    if search:
        job["search"] = search
    return write_json(path, job)


def write_plan(path, out, tasks, frozen, mode="single", islands=None, train=None, **extra):
    plan = {"mode": mode,
            "islands": islands or [{"id": "i1", "backend": "cpu", "machines": [{"id": "n0", "devices": [0]}],
                                    "leader": "n0"}],
            "datasets": {"blake3:synthetic": tasks}, "frozen": frozen, "output_dir": out,
            "train": {**TRAIN_CFG, **(train or {})}}
    plan.update(extra)
    return write_json(path, plan)


def parse_events(stdout):
    return [json.loads(line) for line in stdout.splitlines() if line.startswith("{")]


def run_cli(args, timeout=900):
    proc = subprocess.run([sys.executable, "-m", "apz_train.run", *args], cwd=TRAINER,
                          capture_output=True, text=True, timeout=timeout)
    return proc.returncode, parse_events(proc.stdout), proc.stderr
