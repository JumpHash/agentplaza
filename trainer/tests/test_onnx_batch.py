"""The rung predictor's ONNX export is batch-agnostic: the graph runs in onnxruntime at batch 1 and 4."""

import importlib.util
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch

from apz_train import export

_RECIPE = Path(__file__).resolve().parents[1] / "recipes" / "rung_predictor" / "recipe.py"
_spec = importlib.util.spec_from_file_location("rung_predictor_recipe", _RECIPE)
recipe = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(recipe)


def test_rung_predictor_onnx_runs_at_batch_1_and_4(tmp_path):
    torch.manual_seed(0)
    cfg = {"hidden": 16, "depth": 2, "prices": [1.0, 10.0, 100.0], "lambda_ce": 0.1, "embedding_dim": 4}
    model = recipe.build_model(cfg)
    path = str(tmp_path / "model.onnx")
    input_names = recipe.META["input"]["names"]
    output_names = recipe.META["output"]["names"]
    export.export_onnx(model, recipe.example_input(), path, input_names, output_names, 17)

    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    for batch in (1, 4):
        feeds = {
            "num": torch.randn(batch, len(recipe.NUM)).numpy(),
            "cat": torch.randint(0, recipe.BUCKETS, (batch, len(recipe.CAT))).numpy(),
        }
        (out,) = session.run(None, feeds)
        assert out.shape == (batch, len(recipe.CLASSES))
        assert np.isfinite(out).all()
