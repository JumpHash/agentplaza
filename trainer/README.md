# apz-train: the Foundry trainer runtime

The training loop for Foundry jobs. One package, three modes:

- **single**: one device.
- **tight**: one island, several devices, under `torchrun` (DDP; FSDP when the recipe says `shard: true`).
- **loose**: several islands, DiLoCo-style. Each island trains locally for `inner_steps`, then sends its
  pseudo-gradient to the hub (`apz_train.hub`), which averages and applies the outer step. See `PROTOCOL.md`.

```
apz_train/
  run.py        entry point: job + plan -> training -> ONNX export -> model card
  trainer.py    the loop: wrapping, batches, eval, checkpoints (resumable), GPU-hour budget
  diloco.py     island rounds, the outer math, the hub client
  hub.py        the averaging hub (stdlib HTTP)
  search.py     grid, optuna (TPE), pbt
  export.py     ONNX export (opset 17) and the onnxruntime check
  backend.py    cuda / rocm / xpu / mps / cpu, process group (nccl on GPUs, gloo otherwise)
  recipes.py    recipe loader: <recipes>/<name>/recipe.json and recipe.py
  wire.py       the binary payload (JSON header + float32 bytes)
  events.py     JSON-line events on stdout (rank 0)
  synthetic.py  synthetic rung data for tests and smoke runs
recipes/rung_predictor/   rung-predictor@1, the first vetted recipe
tests/                    pytest, CPU only
Dockerfile.{cuda,rocm,cpu}
```

## Run

```sh
uv sync                       # CPU torch from the PyTorch CPU index on Linux
uv run pytest -q              # the suite (about 70 s)

uv run python -m apz_train.synthetic --out /tmp/rungs --n 4000
uv run python -m apz_train.run --job job.json --plan plan.json
torchrun --standalone --nproc_per_node=2 -m apz_train.run --job job.json --plan plan.json
uv run python -m apz_train.hub --islands i1,i2 --port 8800 &
uv run python -m apz_train.run --job job.json --plan plan.json --island i1 --hub http://127.0.0.1:8800
```

Stdout is the event stream (see `PROTOCOL.md`); the last line is `done` or `error`. Outputs go to the
plan's `output_dir` (per island in loose mode): `model.onnx`, `model_card.json`, `ckpt.pt` (resume state
only, loaded with `weights_only=True`).

## Recipes

A recipe is a directory with `recipe.json` (id, input and output names, the feature schema, defaults,
`shard`) and `recipe.py` with `build_model(cfg)`, `dataset(paths, split, cfg)` (`train`, `val`, `frozen`),
`loss(batch, out)`, `metrics(batch, out)`, `example_input()`, `serving_schema()` (the registry's inference contract; export refuses a recipe without
it), and optionally `bind(cfg)`. Batch items are
dicts; the model's `forward` takes the input names in order.

`rung-predictor@1`: an MLP over numeric features (signed log) and hashed categorical features (blake2b,
4096 buckets, 8-dim embeddings). Its loss is expected escalation cost (`E[C[k, y]]` over the softmax, with
prices normalised to the frontier rung) plus a cross-entropy term. Metrics include expected cost, the
plain-cascade baseline, accuracy, over- and under-routing rates, and ECE. Data split: the newest
`val_fraction` of tasks by time. Frozen-set tasks are removed from train and validation.

## Images

`docker build -f Dockerfile.cuda -t apz-trainer:cuda .` (also `rocm`, `cpu`), from this directory. The
recipes directory can be mounted over `/opt/apz/recipes` or pointed at with `APZ_RECIPES`.

## Notes

- Search runs in one process and is not resumable at the search level; each trial's checkpoints are in its
  own directory.
- FSDP, XPU and MPS paths are written but not exercised by the CPU tests.
- Loose islands must not use FSDP (their weights are broadcast whole).
