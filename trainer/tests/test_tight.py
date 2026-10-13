import json
import os
import subprocess
import sys

import numpy as np

from apz_train import wire
from conftest import TRAINER, make_data, parse_events, write_job, write_plan


def test_tight_two_gloo_ranks_end_with_same_weights(tmp_path):
    tasks, frozen = make_data(tmp_path, n=1500)
    out = str(tmp_path / "out")
    job = write_job(tmp_path / "job.json")
    plan = write_plan(tmp_path / "plan.json", out, tasks, frozen, mode="tight", train={"steps": 120})
    cmd = [sys.executable, "-m", "torch.distributed.run", "--standalone", "--nproc_per_node=2",
           "-m", "apz_train.run", "--job", job, "--plan", plan, "--dump-weights"]
    proc = subprocess.run(cmd, cwd=TRAINER, capture_output=True, text=True, timeout=900)
    assert proc.returncode == 0, proc.stderr[-3000:]
    events = parse_events(proc.stdout)
    assert len([e for e in events if e["ev"] == "done"]) == 1
    assert {e["rank"] for e in events if e["ev"] == "step"} == {0}
    a = wire.unpack(open(os.path.join(out, "rank0.apzw"), "rb").read())[1]
    b = wire.unpack(open(os.path.join(out, "rank1.apzw"), "rb").read())[1]
    assert a.keys() == b.keys()
    for k in a:
        np.testing.assert_allclose(a[k], b[k], atol=1e-6, err_msg=k)
