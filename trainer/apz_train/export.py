"""ONNX export (opset 17; weights are never pickled) and the onnxruntime round-trip check."""

import numpy as np
import torch


def export_onnx(model, example, path, input_names, output_names, opset=17):
    model = model.eval().cpu()
    dyn = {name: {0: "batch"} for name in list(input_names) + list(output_names)}
    torch.onnx.export(model, tuple(example), path, input_names=list(input_names),
                      output_names=list(output_names), opset_version=opset,
                      dynamic_axes=dyn, dynamo=False)


def check_onnx(model, path, check_inputs, input_names, atol=1e-4):
    """Runs the exported graph in onnxruntime on real inputs and compares with the torch output.
    Returns the max absolute difference; raises if it exceeds `atol`."""
    import onnxruntime as ort
    model = model.eval().cpu()
    session = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    feeds = {name: t.cpu().numpy() for name, t in zip(input_names, check_inputs)}
    with torch.no_grad():
        ref = model(*check_inputs).float().numpy()
    got = session.run(None, feeds)[0]
    diff = float(np.max(np.abs(ref - got)))
    if diff > atol:
        raise RuntimeError(f"ONNX output differs from torch by {diff:.3g} (tolerance {atol})")
    return diff
