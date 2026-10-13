import numpy as np
import torch

from apz_train import recipes
from apz_train.backend import DistCtx
from apz_train.trainer import Budget, Trainer
from conftest import TRAIN_CFG, make_data

CTX = DistCtx(backend="cpu", device=torch.device("cpu"), rank=0, world=1, local_rank=0)


def _cfg(**kw):
    recipe = recipes.load("rung-predictor@1")
    cfg = {**recipe.defaults, **TRAIN_CFG, "steps": 60, "checkpoint_every": 0, "frozen": None, **kw}
    return recipe, cfg


def _datasets(tmp_path, cfg):
    tasks, frozen = make_data(tmp_path, n=1200)
    recipe = recipes.load("rung-predictor@1")
    cfg["frozen"] = frozen
    return recipe.dataset([tasks], "train", cfg), recipe.dataset([tasks], "val", cfg)


def test_resume_continues_at_checkpoint_and_matches_uninterrupted(tmp_path):
    recipe, cfg = _cfg(checkpoint_every=20, seed=3)
    train, val = _datasets(tmp_path, cfg)

    torch.manual_seed(cfg["seed"])
    straight = Trainer(recipe, cfg, train, CTX, str(tmp_path / "a"), Budget(None), val_ds=val, resume=False)
    straight.fit(until=60)
    want = straight.full_state()

    # Run to step 25, then "crash": the checkpoint written at step 20 is the last one.
    torch.manual_seed(cfg["seed"])
    crashed = Trainer(recipe, cfg, train, CTX, str(tmp_path / "b"), Budget(None), val_ds=val, resume=False)
    crashed.fit(until=25, checkpoint_every=20)
    assert crashed.step == 25
    del crashed

    torch.manual_seed(cfg["seed"] + 1)  # a different init: the checkpoint must overwrite it
    resumed = Trainer(recipe, cfg, train, CTX, str(tmp_path / "b"), Budget(None), val_ds=val, resume=True)
    assert resumed.step == 20
    resumed.fit(until=60)
    got = resumed.full_state()
    assert resumed.step == 60
    for k in want:
        np.testing.assert_allclose(got[k].numpy(), want[k].numpy(), atol=1e-6, err_msg=k)
