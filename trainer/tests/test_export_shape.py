"""The export refuses a graph whose input width differs from the card's serving block."""

import pytest
import torch

from apz_train import export


def _features(n):
    return [{"name": f"x{i}", "kind": "numeric", "transform": "signlog"} for i in range(n)]


class _Recipe:
    id = "shape-test@1"

    def __init__(self, example, n_features):
        self._example = example
        self._features = _features(n_features)

    def serving_schema(self):
        names = [f["name"] for f in self._features]
        inputs, start = [], 0
        for i, t in enumerate(self._example):
            width = int(t.shape[1])
            inputs.append({"name": f"in{i}", "dtype": str(t.dtype).replace("torch.", ""),
                           "features": names[start:start + width]})
            start += width
        return {
            "input": {"type": "object", "properties": {n: {"type": "number"} for n in names}, "required": names},
            "output": {"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"]},
            "features": self._features,
            "inputs": inputs,
            "allowed_roles": ["plaza"],
        }

    def example_input(self):
        return self._example


def test_matching_width_exports_the_block():
    block = export.serving_block(_Recipe((torch.zeros(1, 3),), 3))
    assert len(block["features"]) == 3


def test_numeric_and_categorical_columns_count_together():
    example = (torch.zeros(1, 2), torch.zeros(1, 1, dtype=torch.long))
    assert export.input_width(example) == 3
    assert len(export.serving_block(_Recipe(example, 3))["features"]) == 3


def test_width_mismatch_refuses_the_export():
    with pytest.raises(export.ServingError, match="takes 4 features but the serving block lists 3"):
        export.serving_block(_Recipe((torch.zeros(1, 4),), 3))
