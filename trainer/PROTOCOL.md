# Trainer protocol

Three contracts: the hub protocol (loose mode, HTTP), the event stream (stdout JSON lines), and the
job and plan files the runtime reads. The hub in this directory (`apz_train.hub`) is a stand-in; in the
plaza apid implements the same hub protocol.

## 1. Hub protocol (loose mode)

Transport: HTTP/1.1, `application/octet-stream` bodies, no auth here (the plaza's node link provides it).

Payload (`apz_train.wire`), used for every request and response body:

```
b"APZW1" | uint32 big-endian N | N bytes of UTF-8 JSON | raw tensor bytes
JSON: {"meta": {...}, "tensors": [{"name", "shape", "dtype": "float32", "offset", "nbytes"}, ...]}
```

Tensors are sorted by name, float32 only, little-endian, C order. `offset` is relative to the first byte
after the JSON. Nothing is pickled.

| Method and path | Request | Response |
|---|---|---|
| `POST /init` | meta `{island}`, tensors = island's initial weights | 200, meta `{round}`, tensors = global weights. The first call sets the global model; later calls get it back. |
| `GET /state` | none | 200, meta `{round}`, tensors = global weights |
| `POST /round` | meta `{island, round, samples}`, tensors = `delta = base - local` (the island's pseudo-gradient) | 200 meta `{round, accepted: true}` when the round closed or the island was counted in time; 409 meta `{round, accepted: false}` when refused (stale or ahead). Tensors are the global weights for `round` in both cases. |
| `GET /health` | none | 200 |

Round rules:
- A round `r` closes when every island in the quorum has contributed for it, or `wait_s` has passed since
  the round's first contribution. Islands that have not contributed are simply absent from that average.
- A delta with `round < hub_round - staleness`, or `round > hub_round`, is refused with 409. The island
  rebases on the returned weights and continues from `round = hub_round`.
- The aggregate is the sample-weighted mean of the deltas: `avg = sum(s_i * d_i) / sum(s_i)`.
- The outer step is DiLoCo's Nesterov momentum, with defaults outer lr `0.7` and momentum `0.9`:
  `v = mu * v + avg`, then `theta = theta - lr * (avg + mu * v)`.

## 2. Event stream (stdout)

One JSON object per line, each with `ev`, `t` (unix seconds), `island`, `rank`. Only rank 0 of an island
prints `step`, `eval`, `checkpoint`, `round`, `trial` and `done`. Any rank may print `error`. Logs go to
stderr.

| `ev` | Fields |
|---|---|
| `step` | `step`, `loss`, `lr`, `samples` |
| `eval` | `step`, `split`, `metrics` (and `round` in loose mode) |
| `checkpoint` | `step`, `path` |
| `round` | `round`, `accepted`, `hub_round`, `step` (loose mode) |
| `trial` | `trial`, `params`, `objective`, `metrics`, `gpu_hours` (and `generation` for PBT) |
| `done` | `metrics` {`val`, `frozen`}, `onnx`, `onnx_check` {`max_abs_diff`, `opset`}, `model_card`, `model_card_path` |
| `error` | `message`, `type` (exit code 1) |

`done` is the last line on success. `model_card` also lists the data provenance per split (row counts,
time range, label balance, rows excluded because they are in the frozen set), the config, the GPU-hours
used, and the search result when there was one.

## 3. Job and plan

The job is a `training-job` spec (`docs/codex-draft/schemas/training-job.schema.json`). Search space
convention in `search.space`: a list is a set of choices (grid, optuna, pbt perturbs ranges only), a
`{low, high, log?, int?}` object is a range, a scalar is a fixed value and is applied to the training
config.

The plan is written by the planner and carries what the job cannot: `mode` (`single` | `tight` | `loose`),
`islands` (`{id, backend, machines, leader, rdzv}`), `datasets` (dataset ref to local JSONL path),
`frozen` (local path of the frozen-set JSONL), `output_dir`, `train` (overrides of the recipe defaults),
and for loose mode `inner_steps` and `rounds` (or derived from `steps`).

Dataset refs (`blake3:…`, `audit:…`) resolve through `datasets` in the plan, then as file paths, then as
`<data-dir>/<ref with : replaced by _>.jsonl`.

Recipes are looked up as `<recipes>/<name with - replaced by _>/recipe.json`, where `<recipes>` is
`--recipes`, `$APZ_RECIPES`, or the bundled `trainer/recipes`. The `id` in `recipe.json` must match the
job's `recipe`.
