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


SERVING_KEYS = ("input", "output", "features", "allowed_roles")


class ServingError(RuntimeError):
    """The recipe has no serving contract the registry would accept, so no model card is written."""


def serving_block(recipe):
    """The model card's `serving` block, from the recipe's serving_schema() hook. Fails closed: a recipe
    without the hook, or whose block the registry would refuse, is not exported."""
    decl = recipe.serving_schema()
    if decl is None:
        raise ServingError(f"recipe {recipe.id} declares no serving_schema() hook, so export is refused "
                           "(fail closed). Add the hook to its recipe.py, returning input, output, "
                           "features and allowed_roles.")
    missing = [k for k in SERVING_KEYS if k not in decl]
    if missing:
        raise ServingError(f"recipe {recipe.id}: serving_schema() is missing {', '.join(missing)}")
    props = decl["input"].get("properties", {})
    for feature in decl["features"]:
        if feature.get("name") not in props:
            raise ServingError(f"recipe {recipe.id}: feature {feature.get('name')!r} is not an input property")
    if not decl["allowed_roles"]:
        raise ServingError(f"recipe {recipe.id}: allowed_roles is empty, so no role could use the model")
    activation = decl.get("output_activation", "probabilities")
    if activation not in ("logits", "probabilities"):
        raise ServingError(f"recipe {recipe.id}: output_activation {activation!r} is not 'logits' or 'probabilities'")
    block = {k: decl[k] for k in SERVING_KEYS}
    block["output_activation"] = activation
    if "labels" in decl:
        block["labels"] = decl["labels"]
    return block
