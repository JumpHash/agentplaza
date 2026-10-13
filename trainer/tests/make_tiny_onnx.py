"""Writes crates/apid/testdata/tiny_dynamic.onnx: a 10-feature, 3-class linear model whose ONNX input
and output both carry a dynamic batch axis. This is the shape the apid tract loader must accept
(the declared output is ["batch", 3]). Run from trainer/: uv run python tests/make_tiny_onnx.py
"""

from pathlib import Path

import torch

WIDTH = 10
CLASSES = 3
OUT = Path(__file__).resolve().parents[2] / "crates" / "apid" / "testdata" / "tiny_dynamic.onnx"


class Tiny(torch.nn.Module):
    """Linear to CLASSES, then a reshape whose target takes the batch size from the input's shape.
    The export builds that target with Shape, Gather and Concat nodes, which is the Concat the
    loader's shape analysis trips on when the input is pinned to one row."""

    def __init__(self) -> None:
        super().__init__()
        self.fc = torch.nn.Linear(WIDTH, CLASSES)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.fc(x).reshape(x.shape[0], CLASSES)


def main() -> None:
    torch.manual_seed(0)
    model = Tiny().eval()
    example = torch.zeros(1, WIDTH)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        model,
        (example,),
        str(OUT),
        input_names=["input"],
        output_names=["output"],
        opset_version=17,
        dynamic_axes={"input": {0: "batch"}, "output": {0: "batch"}},
        dynamo=False,
    )
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
