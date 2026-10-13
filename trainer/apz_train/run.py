"""Entry point:

    python -m apz_train.run --job job.json --plan plan.json [--island i1] [--hub URL]
    torchrun --standalone --nproc_per_node=G -m apz_train.run --job job.json --plan plan.json

The plan (written by the planner) says the mode (single | tight | loose), the islands and their
backends, the dataset refs resolved to local files, the frozen-set file and, for loose mode, the inner
steps and rounds. Progress goes to stdout as JSON lines (see events.py); the last line is `done` or
`error`.
"""

import argparse
import json
import os
import sys
import traceback

import torch
from torch.utils.data import default_collate

from . import backend, diloco, export, recipes, search, wire
from .events import emit, emit_error, log, set_context
from .trainer import Budget, Trainer

ONNX_OPSET = 17


def _load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _pick_island(plan, island_id):
    islands = plan.get("islands", [])
    if island_id is None:
        return islands[0] if islands else {}
    for isl in islands:
        if isl["id"] == island_id:
            return isl
    raise ValueError(f"island {island_id!r} is not in the plan")


def _resolve(ref, plan, data_dir):
    mapping = plan.get("datasets", {})
    if ref in mapping:
        return mapping[ref]
    if os.path.isfile(ref):
        return ref
    if data_dir:
        path = os.path.join(data_dir, ref.replace(":", "_") + ".jsonl")
        if os.path.isfile(path):
            return path
    raise FileNotFoundError(f"dataset {ref!r} is not in the plan or the data dir")


def _base_cfg(recipe, job, plan):
    """Recipe defaults, overridden by fixed values of the search space, then by the plan's train block."""
    cfg = dict(recipe.defaults)
    space = job.get("search", {}).get("space", {})
    cfg.update({k: v for k, v in space.items() if not isinstance(v, (list, dict))})
    cfg.update(plan.get("train", {}))
    return cfg


@torch.no_grad()
def _score(recipe, model, ds, chunk=2048):
    if ds is None or len(ds) == 0:
        return None
    model.eval()
    outs, parts = [], {}
    for s in range(0, len(ds), chunk):
        batch = default_collate([ds[i] for i in range(s, min(len(ds), s + chunk))])
        outs.append(model(*[batch[k] for k in recipe.inputs]).float())
        for k, v in batch.items():
            parts.setdefault(k, []).append(v)
    return recipe.metrics({k: torch.cat(v) for k, v in parts.items()}, torch.cat(outs))


def _finish(recipe, job, plan, ctx, cfg, state, budget, out_dir, datasets, extra):
    """Main rank only: score val and frozen on the final weights, export ONNX, check it, write the card."""
    recipe.bind(cfg)
    model = recipe.build_model(cfg)
    model.load_state_dict({k: torch.as_tensor(v) for k, v in state.items()})
    val_m = _score(recipe, model, datasets["val"])
    frozen_m = _score(recipe, model, datasets["frozen"])
    onnx_path = os.path.join(out_dir, "model.onnx")
    export.export_onnx(model, recipe.example_input(), onnx_path, recipe.inputs, recipe.outputs, ONNX_OPSET)
    check_ds = datasets["val"] if len(datasets["val"]) else datasets["train"]
    rows = default_collate([check_ds[i] for i in range(min(64, len(check_ds)))])
    diff = export.check_onnx(model, onnx_path, [rows[k] for k in recipe.inputs], recipe.inputs)
    card = {
        "recipe": recipe.id,
        "job_id": job.get("job_id"),
        "mode": plan.get("mode", "single"),
        "islands": [i.get("id") for i in plan.get("islands", [])],
        "backend": ctx.backend,
        "world_size": ctx.world,
        "config": {k: v for k, v in cfg.items() if k != "frozen"},
        "data": {"train": datasets["train"].provenance, "val": datasets["val"].provenance,
                 "frozen": datasets["frozen"].provenance if datasets["frozen"] is not None else None},
        "metrics": {"val": val_m, "frozen": frozen_m},
        "parameters": sum(p.numel() for p in model.parameters()),
        "onnx": {"path": onnx_path, "opset": ONNX_OPSET, "max_abs_diff": diff,
                 "bytes": os.path.getsize(onnx_path)},
        "gpu_hours": round(budget.hours(), 6),
        "objective": "expected_cost",
        "serving": export.serving_block(recipe),
        **extra,
    }
    card_path = os.path.join(out_dir, "model_card.json")
    with open(card_path, "w", encoding="utf-8") as f:
        json.dump(card, f, indent=2, default=str)
    return {"metrics": {"val": val_m, "frozen": frozen_m}, "onnx": onnx_path,
            "onnx_check": {"max_abs_diff": diff, "opset": ONNX_OPSET}, "model_card": card,
            "model_card_path": card_path}


def _run(args, job, plan):
    isl = _pick_island(plan, args.island)
    ctx = backend.setup(isl.get("backend", "auto"))
    set_context(island=args.island, rank=ctx.rank)
    recipe = recipes.load(job["recipe"], args.recipes)
    export.serving_block(recipe)  # fails before training when the recipe declares no serving contract
    cfg = _base_cfg(recipe, job, plan)
    cfg["frozen"] = plan.get("frozen")
    mode = plan.get("mode", "single")
    out = args.out or plan.get("output_dir", "out")
    if args.island:
        out = os.path.join(out, args.island)
    torch.manual_seed(int(cfg["seed"]))

    paths = [_resolve(ref, plan, args.data_dir) for ref in job["datasets"]]
    datasets = {"train": recipe.dataset(paths, "train", cfg), "val": recipe.dataset(paths, "val", cfg),
                "frozen": recipe.dataset(paths, "frozen", cfg) if cfg["frozen"] else None}
    budget = Budget(job.get("budget_gpu_hours"), ctx.world)
    train_ds, val_ds = datasets["train"], datasets["val"]
    log(f"{recipe.id} on {ctx.backend} x{ctx.world}: train={len(train_ds)} val={len(val_ds)} mode={mode}")

    search_spec = job.get("search", {})
    extra, state = {}, None
    if search_spec.get("method"):
        if ctx.world > 1:
            raise RuntimeError("hyperparameter search runs in one process")

        def make(params, name):
            return Trainer(recipe, {**cfg, **params}, train_ds, ctx, os.path.join(out, name), budget,
                           val_ds=val_ds, resume=False)

        best = search.run(search_spec["method"], make, search_spec.get("space", {}),
                          int(search_spec.get("trials", 1)), int(cfg["steps"]), val_ds, budget,
                          int(cfg["seed"]))
        state = best["state"]
        extra = {"search": {"method": search_spec["method"], "trials": search_spec.get("trials", 1),
                            "best_trial": best["trial"], "best_params": best["params"],
                            "objective": best["objective"]}, "steps": int(cfg["steps"])}
        cfg = {**cfg, **best["params"]}
    elif mode == "loose":
        inner = int(plan["inner_steps"])
        rounds = int(plan.get("rounds") or max(1, int(cfg["steps"]) // inner))
        cfg["steps"] = rounds * inner
        if not args.hub:
            raise ValueError("loose mode needs --hub")
        trainer = Trainer(recipe, cfg, train_ds, ctx, out, budget, val_ds=val_ds, resume=True)
        resume = None
        if trainer.loaded_extra.get("round") is not None:
            resume = (int(trainer.loaded_extra["round"]),
                      {k: v.numpy() for k, v in trainer.loaded_extra["base"].items()})
        base = diloco.run_island(trainer, args.hub, args.island or "i0", rounds, inner, resume=resume)
        if base is not None:
            state = {k: torch.from_numpy(v) for k, v in base.items()}
        extra = {"steps": trainer.step, "stopped": trainer.stopped, "rounds": rounds,
                 "inner_steps": inner, "samples": trainer.samples}
    else:
        trainer = Trainer(recipe, cfg, train_ds, ctx, out, budget, val_ds=val_ds,
                          shard=recipe.shard or bool(cfg.get("shard")), resume=True)
        trainer.fit(until=int(cfg["steps"]), eval_every=int(cfg["eval_every"]),
                    checkpoint_every=int(cfg["checkpoint_every"]), log_every=int(cfg["log_every"]))
        if trainer.last_ckpt_step != trainer.step:
            trainer.save_ckpt()
        state = trainer.full_state()
        extra = {"steps": trainer.step, "stopped": trainer.stopped, "samples": trainer.samples}

    if args.dump_weights and state is not None:
        with open(os.path.join(out, f"rank{ctx.rank}.apzw"), "wb") as f:
            f.write(wire.pack({}, {k: v.numpy() for k, v in state.items()}))
    if ctx.is_main and state is not None:
        return _finish(recipe, job, plan, ctx, cfg, state, budget, out, datasets, extra)
    return None


def main(argv=None):
    p = argparse.ArgumentParser(description="Foundry trainer runtime")
    p.add_argument("--job", required=True, help="training-job spec (training-job.schema.json)")
    p.add_argument("--plan", required=True, help="placement plan from the planner")
    p.add_argument("--island", help="island id in the plan (loose mode, or to pick its backend)")
    p.add_argument("--hub", help="DiLoCo hub URL (loose mode)")
    p.add_argument("--out", help="output directory (default: the plan's output_dir, else ./out)")
    p.add_argument("--recipes", help="recipes directory (default: $APZ_RECIPES, else the bundled ones)")
    p.add_argument("--data-dir", help="directory holding <ref>.jsonl for dataset refs not in the plan")
    p.add_argument("--dump-weights", action="store_true", help="write each rank's final weights (tests)")
    args = p.parse_args(argv)
    set_context(island=args.island, rank=int(os.environ.get("RANK", "0")))
    try:
        job, plan = _load(args.job), _load(args.plan)
        done = _run(args, job, plan)
        if done is not None:
            emit("done", **done)
        return 0
    except Exception as e:  # reported as a JSON event; the process exits non-zero
        emit_error(str(e), type=type(e).__name__)
        log(traceback.format_exc())
        return 1
    finally:
        backend.teardown()


if __name__ == "__main__":
    sys.exit(main())
