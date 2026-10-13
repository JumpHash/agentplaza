import os
import subprocess
import sys

import numpy as np

from apz_train import hub as hubmod
from apz_train import wire
from conftest import TRAINER, make_data, parse_events, write_job, write_plan

ISLANDS = [{"id": "i1", "backend": "cpu", "machines": [{"id": "n0", "devices": [0]}], "leader": "n0"},
           {"id": "i2", "backend": "cpu", "machines": [{"id": "n1", "devices": [0]}], "leader": "n1"}]


def test_loose_two_islands_through_hub_converge_and_agree(tmp_path):
    tasks, frozen = make_data(tmp_path, n=2000)
    out = str(tmp_path / "out")
    job = write_job(tmp_path / "job.json")
    plan = write_plan(tmp_path / "plan.json", out, tasks, frozen, mode="loose", islands=ISLANDS,
                      inner_steps=50, rounds=4, train={"steps": 200, "eval_every": 0, "batch_size": 128})
    hub = hubmod.Hub(["i1", "i2"], wait_s=120)
    server = hubmod.serve(hub, "127.0.0.1", 0)
    url = f"http://127.0.0.1:{server.server_address[1]}"
    procs = {}
    try:
        for isl in ("i1", "i2"):
            cmd = [sys.executable, "-m", "apz_train.run", "--job", job, "--plan", plan,
                   "--island", isl, "--hub", url, "--dump-weights"]
            procs[isl] = subprocess.Popen(cmd, cwd=TRAINER, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        results = {}
        for isl, p in procs.items():
            stdout, stderr = p.communicate(timeout=900)
            assert p.returncode == 0, stderr[-3000:]
            results[isl] = parse_events(stdout)
    finally:
        server.shutdown()
    assert hub.round == 4
    for isl, events in results.items():
        done = [e for e in events if e["ev"] == "done"][0]
        assert done["metrics"]["val"]["accuracy"] > 0.7, (isl, done["metrics"])
        assert done["metrics"]["val"]["expected_cost"] <= done["metrics"]["val"]["cascade_cost"]
        assert len([e for e in events if e["ev"] == "round"]) == 4
    a = wire.unpack(open(os.path.join(out, "i1", "rank0.apzw"), "rb").read())[1]
    b = wire.unpack(open(os.path.join(out, "i2", "rank0.apzw"), "rb").read())[1]
    for k in a:
        np.testing.assert_allclose(a[k], b[k], atol=1e-6, err_msg=k)
