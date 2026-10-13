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


SERVING_KEYS = ("input", "output", "features", "inputs", "allowed_roles")


class ServingError(RuntimeError):
    """The recipe has no serving contract the registry would accept, so no model card is written."""


def input_width(example):
    """The model's input width: one column per feature. Each example tensor is (batch, columns), so the
    widths add up over the graph's inputs (a rung predictor takes numeric and categorical columns)."""
    return sum(int(t.shape[1]) for t in example)


def check_coverage(recipe, inputs, features, example):
    """The serving block's `inputs` (one entry per graph input, in graph order) must cover the example
    graph: one entry per graph input, and every serving feature in exactly one entry, with no entry
    naming a feature the serving block does not declare. Fails closed with ServingError."""
    if len(inputs) != len(example):
        raise ServingError(f"recipe {recipe.id}: the ONNX graph has {len(example)} inputs but the serving block lists {len(inputs)}; export is refused")
    feeds = {}
    for graph_input in inputs:
        for name in graph_input.get("features", []):
            feeds.setdefault(name, []).append(graph_input.get("name"))
    for feature in features:
        name = feature.get("name")
        count = len(feeds.get(name, []))
        if count != 1:
            raise ServingError(f"recipe {recipe.id}: feature {name!r} is in {count} inputs; each serving feature must feed exactly one graph input, so export is refused")
    unknown = sorted(set(feeds) - {f.get("name") for f in features})
    if unknown:
        raise ServingError(f"recipe {recipe.id}: inputs list {', '.join(unknown)}, which are not serving features; export is refused")


def check_graph_inputs(recipe, inputs, example):
    """Each graph input's features are as many as its example tensor's columns, and its dtype is the tensor's."""
    for graph_input, tensor in zip(inputs, example):
        width = int(tensor.shape[1])
        listed = len(graph_input.get("features", []))
        if listed != width:
            raise ServingError(f"recipe {recipe.id}: input {graph_input.get('name')!r} takes {width} columns but lists {listed} features; export is refused")
        dtype = str(tensor.dtype).replace("torch.", "")
        if graph_input.get("dtype") != dtype:
            raise ServingError(f"recipe {recipe.id}: input {graph_input.get('name')!r} has dtype {graph_input.get('dtype')!r} but the graph takes {dtype}; export is refused")


def serving_block(recipe):
    """The model card's `serving` block, from the recipe's serving_schema() hook. Fails closed: a recipe
    without the hook, or whose block the registry would refuse, is not exported."""
    decl = recipe.serving_schema()
    if decl is None:
        raise ServingError(f"recipe {recipe.id} declares no serving_schema() hook, so export is refused "
                           "(fail closed). Add the hook to its recipe.py, returning input, output, "
                           "inputs, features and allowed_roles.")
    missing = [k for k in SERVING_KEYS if k not in decl]
    if missing:
        raise ServingError(f"recipe {recipe.id}: serving_schema() is missing {', '.join(missing)}")
    props = decl["input"].get("properties", {})
    for feature in decl["features"]:
        if feature.get("name") not in props:
            raise ServingError(f"recipe {recipe.id}: feature {feature.get('name')!r} is not an input property")
    if not hasattr(recipe, "example_input"):
        raise ServingError(f"recipe {recipe.id} declares no example_input(), so its ONNX input width cannot be checked against the serving block; export is refused")
    example = recipe.example_input()
    check_coverage(recipe, decl["inputs"], decl["features"], example)
    width = input_width(example)
    if width != len(decl["features"]):
        raise ServingError(f"recipe {recipe.id}: the ONNX input takes {width} features but the serving block lists {len(decl['features'])}; export is refused so the card and the graph cannot disagree")
    check_graph_inputs(recipe, decl["inputs"], example)
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
