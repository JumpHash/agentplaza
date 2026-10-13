"""Writes the two-input test fixture for the Rust loader: crates/apid/testdata/tiny_two_input.onnx and
tiny_two_input.json. The graph has the rung predictor's shape: num (float32, [batch, 6]) and cat (int64,
[batch, 4], values 0..7) go through an nn.Embedding(8, 4) on cat, a flatten to [batch, 16], a torch.cat with
num to [batch, 22], then Linear(22, 3) to logits. The JSON holds two rows of inputs and the logits that
onnxruntime returns for them at batch 2.

Run: bash -c 'cd /mnt/fast/AgentPlaza/trainer && uv run python tests/make_two_input_onnx.py'
"""

import json
import os

import numpy as np
import onnxruntime as ort
import torch
import torch.nn as nn

TRAINER = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(os.path.dirname(TRAINER), "crates", "apid", "testdata")
ONNX_PATH = os.path.join(OUT_DIR, "tiny_two_input.onnx")
JSON_PATH = os.path.join(OUT_DIR, "tiny_two_input.json")


class TwoInput(nn.Module):
    def __init__(self):
        super().__init__()
        self.emb = nn.Embedding(8, 4)
        self.head = nn.Linear(22, 3)

    def forward(self, num, cat):
        # Reshape by the runtime batch (cat.shape[0]), never by a constant, so the batch axis stays dynamic.
        emb = self.emb(cat).reshape(cat.shape[0], -1)
        return self.head(torch.cat([num, emb], dim=1))


def main():
    torch.manual_seed(7)
    model = TwoInput().eval()
    num = torch.randn(2, 6)
    cat = torch.randint(0, 8, (2, 4), dtype=torch.long)
    os.makedirs(OUT_DIR, exist_ok=True)
    torch.onnx.export(
        model, (num, cat), ONNX_PATH,
        input_names=["num", "cat"], output_names=["logits"], opset_version=17,
        dynamic_axes={"num": {0: "batch"}, "cat": {0: "batch"}, "logits": {0: "batch"}},
        dynamo=False,
    )
    session = ort.InferenceSession(ONNX_PATH, providers=["CPUExecutionProvider"])
    logits = session.run(None, {"num": num.numpy(), "cat": cat.numpy()})[0]
    with torch.no_grad():
        ref = model(num, cat).numpy()
    diff = float(np.max(np.abs(ref - logits)))
    if diff > 1e-5:
        raise RuntimeError(f"onnxruntime differs from torch by {diff:.3g}")
    payload = {"num": num.tolist(), "cat": cat.tolist(), "logits": logits.tolist()}
    with open(JSON_PATH, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)
        f.write("\n")
    print(f"wrote {ONNX_PATH} and {JSON_PATH} (onnxruntime vs torch max diff {diff:.3g})")


if __name__ == "__main__":
    main()
