import json

import pytest
import torch

from apz_train import recipes
from conftest import make_data


def test_cost_matrix_escalation_costs():
    rp = recipes.load("rung-predictor@1")
    mod = rp.mod
    C = mod.cost_matrix([1.0, 10.0, 100.0])
    assert C.shape == (3, 3)
    # Need cheap (y=0): cheap costs 0.01, mid routing costs 0.1, frontier 1.0 (normalised to 100)
    assert torch.allclose(C[:, 0], torch.tensor([0.01, 0.1, 1.0]))
    # Need frontier (y=2): routing cheap pays all three rungs on the way up
    assert torch.isclose(C[0, 2], torch.tensor(1.11), atol=1e-6)
    assert torch.isclose(C[2, 2], torch.tensor(1.0))


def test_dataset_splits_are_time_ordered_and_exclude_frozen(tmp_path):
    tasks, frozen = make_data(tmp_path, n=800)
    rp = recipes.load("rung-predictor@1")
    cfg = {**rp.defaults, "frozen": frozen}
    tr = rp.dataset([tasks], "train", cfg)
    va = rp.dataset([tasks], "val", cfg)
    fz = rp.dataset([tasks], "frozen", cfg)
    assert tr.provenance["ts_range"][1] < va.provenance["ts_range"][0]
    assert len(fz) == sum(1 for _ in open(frozen))
    assert tr.provenance["excluded_frozen"] == len(fz)
    assert len(tr) + len(va) + len(fz) == len(open(tasks).readlines())


def test_unknown_recipe_is_rejected():
    with pytest.raises(ValueError):
        recipes.load("../x@1")
