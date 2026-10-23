# The Foundry and the Lab: breeders, training on any GPUs, model agents

Status: built (2026-10-14), with the Hugging Face and text-model parts (below) added after the design. The trainer
runs natively on every OS now, with no containers (see The trainer runtime); the container text below is the
2026-10-14 build. The hub is
`crates/apid/src/foundry/` (jobs, devices, plan, recipes, datasets, the registry with shadow, promote and retire, model
agents on tract), the trainer is `trainer/` (`apz_train`), the Lab is in both apps (`apps/web/src/views/lab*.ts`,
`apps/desktop/src/console/screens/lab*.rs`), and the shadow runs are `crates/apid/src/usage.rs`. Builds on the codex draft (`docs/codex-draft/playbooks/breeding-guide.md`,
`schemas/training-job.schema.json`).

## What it is

- **Breeders** are LLM agents (role `breeder`) whose job is to turn the plaza's own records into small,
  cheap models: they export data, check it, write a training job for a **vetted recipe**, submit it, read
  the results and request shadow mode. They never run arbitrary code on a GPU machine (0058).
- **The Foundry** is the hub's training system: recipes, datasets, jobs, the GPU placement planner, the
  trainer runtime on every machine with a GPU (or none: CPU training works too), the model registry, shadow
  mode and promotion.
- **Model agents** are what a promoted model becomes: a non-LLM agent type. It has a typed input and output
  (from its model card), runs as ONNX inference, and can only be *invoked* — it can't chat, take tasks or call tools. Other agents (and the plaza's own routing) call it with `invoke_model`.
- **The Lab** is the section in both apps (web console and desktop, 1:1) where the owner manages all of it.

## Hardware: any machine, any mix

Every machine reports its accelerators in `/v1/machine` (and so in `/v1/fleet`) as a list of devices:

```jsonc
"accelerators": [{
  "index": 0, "vendor": "nvidia", "name": "NVIDIA GeForce RTX 3090",
  "memory_mib": 24576, "backend": "cuda", "capability": "8.6",        // compute capability / gfx arch / chip
  "link": "pcie",                                                       // pcie | nvlink | xgmi | soc
  "busy": false, "util_pct": 3, "mem_used_mib": 512
}],
"trainer": { "runtime": "native-nvidia", "ok": true, "detail": "native-nvidia runs trainers under the landlock sandbox",
             "trainer_env": { "torch": "cuda", "matches": true, "detail": "torch is the cuda build, the one this machine's accelerators need" } },
"fabric": ["192.168.1.20", "10.8.0.3"]                                  // addresses other machines may reach
```

| vendor | backend | collective | how the trainer runs | where |
|---|---|---|---|---|
| NVIDIA | `cuda` | NCCL | native runner under the OS sandbox, the GPU's device nodes granted; torch's `cuda` build | Linux (Windows: CPU only, the AppContainer grants no GPU device) |
| AMD | `rocm` | RCCL (torch's `nccl` backend on ROCm) | native runner, `/dev/kfd` and `/dev/dri` granted; torch's `rocm` build | Linux |
| Intel | `xpu` | XCCL / oneCCL | native runner, `/dev/dri` granted; torch's `xpu` build | Linux |
| Apple | `mps` | gloo (no collective on MPS) | native runner under Seatbelt; torch's PyPI wheel (MPS) | macOS |
| none | `cpu` | gloo | native runner; torch's `cpu` build | anywhere |

Detection lives in noded (`nvidia-smi --query-gpu`, `rocm-smi --showproductname --json`, `/sys/class/drm`
for Intel, `system_profiler SPDisplaysDataType` on macOS). The runtime check is the node's probe: the OS
sandbox, the device nodes it can grant (`/dev/nvidia*`, `/dev/kfd`, `/dev/dri`), `uv`, and the trainer's torch
build (below).

## Coupling: how a job spreads over GPUs

The planner groups the job's eligible devices into **islands**: devices that can run one tightly coupled
collective — the same backend family and mutually reachable on a fast network (probed: the hub asks each
machine to open a TCP connection to the others' `fabric` addresses and measures round-trip time; < 2 ms
counts as a LAN). A single machine is always one island per backend.

Three modes, picked automatically from the islands (the job may pin one):

1. **single** — one device.
2. **tight** — one island, many devices: data parallel with `torchrun` (DDP, or FSDP when the recipe says
   the model doesn't fit one device). Single machine multi-GPU: `--nproc_per_node=G`. Several machines on a
   LAN: `--nnodes=N --rdzv_backend=c10d --rdzv_endpoint=<island leader fabric address>:<port>` with each
   machine's own `--nproc_per_node` (heterogeneous GPU counts are fine); `NCCL_SOCKET_IFNAME` is set to the
   interface of the fabric address; containers use the host network for the collective.
3. **loose** — several islands (different vendors, different sites, NAT, slow links, or just the owner's
   choice): **DiLoCo-style low-communication training**. Each island trains its replica locally (itself
   single or tight) for `H` inner steps, then sends its pseudo-gradient (the change of its weights since the
   last sync) to the hub; the hub averages them (weighted by tokens/samples processed), applies the outer
   optimizer (Nesterov momentum) and returns the new global weights; islands continue from them. Islands
   never talk to each other — only to the hub, over the plaza's existing node link, so it works across any
   network the nodes already use. Slow islands just contribute fewer samples per round (asynchronous
   rounds with staleness bound `S`).

Any combination falls out of this: one GPU; 8 GPUs in one box; 4 boxes × 2 GPUs on a LAN; an NVIDIA box
plus a Mac plus an AMD box across the internet (three islands, loose); two LAN clusters at two sites (two
tight islands, loose between them).

The plan is shown before submission and recorded with the job:
```jsonc
"plan": { "mode": "loose", "islands": [
  { "id": "i1", "backend": "cuda", "machines": [{ "id": "n0", "devices": [0, 1] }, { "id": "n3", "devices": [0] }],
    "leader": "n0", "rdzv": "192.168.1.20:29400" },
  { "id": "i2", "backend": "mps", "machines": [{ "id": "n5", "devices": [0] }] } ],
  "inner_steps": 500, "staleness": 2 }
```

## The trainer runtime

`trainer/` in the repo: a Python package `apz_train`, run natively on every OS, with no container image. A node
prepares its trainer once per accelerator, with uv from the trainer project: `uv sync --project trainer --extra
cuda` (NVIDIA), `--extra rocm` (AMD), `--extra xpu` (Intel), or `--extra cpu` (no accelerator). On macOS every
extra takes the PyPI wheel, which has MPS. The extras exclude each other, since one environment holds one torch
build; a node with several vendors trains on NVIDIA first, then AMD, then Intel. uv installs the environment (it
needs the network), never a job. After the extras change, `uv.lock` is refreshed with `uv lock`.
noded's probe reads the environment's `torch/version.py` and reports the build as `trainer_env` in the node's
facts; when the build does not serve the node's accelerators, its detail names the `uv sync --extra` that does.
Each job runs `uv run --frozen --no-sync --project trainer python -m apz_train.run <args>` under the OS sandbox,
in its own directory: the datasets and the recipes are host paths the sandbox grants read-only, and the job
directory is read-write.
- Entry: `python -m apz_train.run --job job.json --plan plan.json --role <island leader|worker>` (under
  `torchrun` for tight islands).
- Recipes are Python modules from the owner-signed `foundry-recipes` repo (Forgejo), checked out at the
  job's pinned commit, mounted read-only. A recipe implements `build_model(params)`, `dataset(paths, split)`,
  `loss(batch, out)`, `metrics(...)`, `example_input()` (for the ONNX export) and declares its input/output
  schema. `apz_train` supplies the loop: DDP/FSDP wrapping, mixed precision where the backend supports
  it, checkpointing (every N steps, resumable), the DiLoCo inner/outer protocol, early stopping, evaluation
  on the validation split, the frozen-set evaluation at the end, and ONNX export (`torch.onnx.export`,
  opset 17; never pickled weights).
- Hyperparameter search (`search.method`: `grid`, `optuna`, `pbt`) runs trials as sequential or parallel
  sub-runs within the job's GPU-hour budget; the best trial is exported.
- Progress: one JSON line per event on stdout (`{"ev":"step","step":120,"loss":0.41,"lr":…,"samples":…}`,
  `eval`, `checkpoint`, `trial`, `done`, `error`) — noded streams them to the hub, the hub publishes them on
  the bus (`training` events) and keeps the series for the Lab's charts.
- Isolation: no network except the collective (tight islands) and, for a loose island, the hub's port on
  loopback: `network: hub` grants the one port the job's `--hub` URL names (on Linux Landlock filters the port,
  not the address). A loose island runs on the hub's own machine until nodes have an egress for jobs; on another
  node it is refused ("a loose job needs the hub on its node's machine until nodes have an egress for jobs").
  Datasets and the recipe read-only; output to a job directory; GPU-hours metered from the device time actually
  used.

## The hub: jobs, registry, model agents

apid gains `foundry` (jobs, recipes, datasets, registry, placement) and `inference` (model agents):
- `POST /v1/foundry/jobs` — a training-job document (the codex schema, extended with optional `devices`
  constraints and `mode`), checked against the laws (`training.submit`: breeders, ≤ 8 GPU-hours unless a
  per-case grant), against the recipe registry (vetted commit only) and the dataset hygiene flags; the
  planner proposes a plan; the job is queued, dispatched to the machines' noded (`POST /v1/train`), and
  tracked. `GET /v1/foundry/jobs[/{id}]`, `/events`, `POST …/{id}/cancel`.
- `GET /v1/foundry/devices` — every accelerator on every machine with its state and the current
  allocation; `POST /v1/foundry/plan` — a dry-run plan for a job.
- `GET/POST /v1/foundry/recipes` — the recipes at their vetted commits; the owner vets a commit.
- `GET/POST /v1/foundry/datasets` — exports (cascade outcomes, tasks, usage) and owner uploads, with
  provenance, time range, label balance, taint flags and the frozen validation set marked.
- `GET /v1/foundry/models[/{id}]` — model cards (data provenance, metrics, calibration, size, input/output
  schema), state `trained → shadow → promoted → retired`; `POST …/{id}/shadow`, `/promote`, `/retire`
  (owner only for promote/retire).
- Model agents: a promoted model gets an agent entry (`kind: "model"`, a CID from a reserved range, no VM,
  no tools, no chat) served by apid with **tract** (pure-Rust ONNX inference: no native deps, so
  it runs on every platform the hub runs on). `invoke_model(model, input)` is a tool for permitted roles,
  validates input/output against the card's schemas, meters the call, and records it. In shadow mode the
  model runs beside the path it would replace and its decisions are logged for comparison, not used.

## Laws

S-006 (breeder bundle) moves into the JHRL statutes: breeders may exec in their own sandbox, read cascade
data and the Foundry, submit jobs up to 8 GPU-hours; `invoke_model` is permitted per the model card's
allowed roles; model agents can't be assigned tasks or messaged (forbid). The rules audit (separate task)
proves every tool path goes through the law check.

## The Lab section (web + desktop)

Sidebar section **Lab** (after Social): **Overview** (GPUs across the fleet: per machine, per device,
utilization, what runs where; queue; recent models), **Jobs** (list + detail: plan diagram of islands,
live loss/metric charts, per-device utilization, logs, cancel), **Recipes**, **Datasets**, **Models**
(registry, cards, shadow results, promote/retire), **Breeders** (the breeder agents, their jobs, budget use).
Uses the shared chart modules.

## Verification

- Unit: the planner (islands from devices + reachability; mode choice; tight/loose plans; heterogeneous
  counts; vendor separation), the DiLoCo averaging math, schema checks, laws.
- The trainer on CPU in CI: a tight run with 2 processes (gloo) and a loose run with 2 islands through a
  local stand-in for the hub; ONNX export and a tract round trip of the exported model.
- On this box: a real CUDA job in `apz-trainer:cuda` (native Docker with the NVIDIA runtime), single GPU;
  plus 2 tight processes sharing the GPU via gloo and a loose 2-island run (GPU island + CPU island).
- Multi-machine tight/loose on real hardware needs more machines; CI covers the protocol with processes.

## Shadow runs on the cascade

Every model call metered by `usage::meter` (gateway and harness) carries a rung record in its `usage::Call`:
- `rung`: `cheap`, `mid` or `frontier`, from the served model's output price (`datasets::rung_for_price`, the
  cut points the dataset export uses). Absent when no price is known, and on records saved before rungs.
- `escalated`: true when the call's task already had a call on a lower rung. It is set when the call is filed
  (`Ledger::file`), from the retained call rows, so pruned calls no longer count. Untasked calls never escalate.
The routing itself has no escalation yet; these fields record what the cascade did, to score the predictor against.

The shadow hook is `usage::shadow_on_cascade`, called in `meter` after the real rung is set and before the record
is filed. Each registry model in state `shadow` whose card id starts with `rung` runs through
`breeder::shadow_run`, with the call's features (model, task, via, token counts, estimated cost) as input and
`{"label": rung}` as the real decision. The runs happen on a thread from the app's weak handle, so the call never
waits for them, and they only read the call: the real rung stays as filed. Each run logs one stderr line (agreed,
differed or did not run); agreement and cost saved go to the model's shadow stats.

## Hugging Face: search, import, base models (the hub side)

Status: built (2026-10-14; the contract was written 2026-10-13), in `crates/apid/src/foundry/hf.rs` (the HF client, the search and
detail mapping, the importable hints, the egress rules) and `foundry/hf_import.rs` (the import queue, the
resumable downloads, the verification, the registration). Owner only: the gate in front of `/v1` admits
the owner, so these routes are never reachable from a VM. Every change is a bus frame, never SSE.

### Routes

| Method | Path | What |
|---|---|---|
| GET | `/v1/foundry/hf/status` | `{hf_token_set, hf_max_gb, free_bytes, downloads_dir}` (`free_bytes` null when unknown) |
| PUT | `/v1/foundry/hf/token` | body `{value}`; stores the token (0600, never echoed); reply `{hf_token_set: true}` |
| DELETE | `/v1/foundry/hf/token` | removes it; reply `{hf_token_set: false}` |
| GET | `/v1/foundry/hf/models` | search, see below |
| GET | `/v1/foundry/hf/models/{id}` | detail; `{id}` may contain a slash (`org/name`), so the route captures the rest of the path |
| POST | `/v1/foundry/hf/import` | starts an import (202), see below |
| GET | `/v1/foundry/hf/imports` | `{imports: [...]}`, newest first |
| GET | `/v1/foundry/hf/imports/{n}` | one import |
| DELETE | `/v1/foundry/hf/imports/{n}` | a running import is cancelled (its `.part` files stay, so a re-import resumes); a finished, failed or cancelled one is removed from the list and its `.part` files are deleted (downloaded files stay: the registry may point at them) |
| GET | `/v1/foundry/models?kind=base` | the registry's records whose `source.as` is `base`; without `kind`, all records as before |
| POST | `/v1/foundry/models/{id}/invoke` | runs a shadow or promoted model on the JSON body (`{text}` for a text model), answers `{output, ms}`; owner only. The breeder's `invoke_model` runs the same path |
| POST | `/v1/foundry/models/{id}/rederive` | re-reads an `imported` model's download and upgrades it to `trained` when the hub can serve it (a text model, or a feature contract). An import from before the text runtime has no recorded task, so the repo's task is looked up first; the id is kept for text models. Owner only |

Search: `GET /v1/foundry/hf/models?search=&task=&library=&sort=&limit=&cursor=`
- `task` maps to HF `pipeline_tag`; `library` maps to HF `library` (`onnx`, `safetensors`, `transformers`, `gguf`, ...).
- `sort`: `downloads` (default), `likes`, `lastModified`, `trending` (sent to HF as `trendingScore`). Always descending.
- `limit`: 1..=100, default 30. `cursor`: the opaque value from `next_cursor`.
- Reply:
```jsonc
{ "items": [{ "id": "Xenova/distilbert-base-uncased-finetuned-sst-2-english", "author": "Xenova",
              "task": "text-classification", "library": "transformers.js", "tags": ["onnx", "..."],
              "downloads": 26244, "likes": 14, "last_modified": "2025-06-30T05:13:56.000Z",
              "gated": false, "private": false, "license": "apache-2.0" }],
  "next_cursor": "eyJ..." }   // null on the last page
```

Detail: `GET /v1/foundry/hf/models/{id}?revision=` (`revision` defaults to `main`)
```jsonc
{ "id": "...", "author": "Xenova", "sha": "0b69...", "url": "https://huggingface.co/...",
  "task": "text-classification", "library": "transformers.js", "tags": [], "license": "apache-2.0",
  "gated": false, "private": false, "downloads": 26244, "likes": 14, "last_modified": "...",
  "summary": "The README's first paragraph, trimmed to 400 characters ('' when there is no README)",
  "files": [{ "path": "onnx/model.onnx", "size": 267955711, "sha256": "252c...", "lfs": true }],
  "importable": {
    "servable": false, "servable_reason": "...",
    "base": true, "base_reason": null } }
```
- `files[].sha256` is the LFS oid from the tree API; `null` for git-stored files.
- `importable.servable`: the repo has an ONNX graph and either a `model_card.json` (the feature contract, checked for real at import) or a `tokenizer.json` with a served task (`text-classification` or `feature-extraction`: the text runtime, below). For any other task the reason says so.
- `importable.base`: a `config.json` plus `*.safetensors` or `pytorch_model.bin`. Usable as a fine-tune base by trainer recipes.

Import: `POST /v1/foundry/hf/import` body `{id, revision?, files?: [...], as: "base"|"servable"}`
- `revision` defaults to `main`; it is resolved to a commit sha first, and the download directory is keyed by that sha.
- `files`: paths from the tree. Without it the default set is: for `base`, `config.json`, the tokenizer files, `*.safetensors`, `*.safetensors.index.json`, `pytorch_model.bin`, `generation_config.json`; for `servable`, `config.json`, `model_card.json`, `onnx/model.onnx` or `model.onnx`, and `tokenizer.json`. Paths must be relative with no `..` (refused otherwise).
- Refusals: 400 bad id, `as` or `files`; 403 a gated model the owner has not accepted (`{error, url}`; the owner accepts the terms on huggingface.co, never us); 403 `huggingface.co` on the owner's `fetch_block`; 404 unknown repo or revision; 409 the same repo and revision is already importing; 413 the selected files total more than `foundry.hf_max_gb`; 507 not enough free disk (or free disk unreadable); 502 Hugging Face did not answer as expected.
- Reply 202: `{import: <import record>}`.

Import record:
```jsonc
{ "n": 3, "id": "Xenova/distilbert-base-uncased-finetuned-sst-2-english", "revision": "main", "sha": "0b69...",
  "as": "servable", "state": "downloading", "dir": "<state>/foundry/hf/Xenova/distilbert-base-uncased-finetuned-sst-2-english/0b69.../",
  "files": [{ "path": "onnx/model.onnx", "size": 267955711, "sha256": "252c...", "bytes": 1048576, "state": "downloading" }],
  "bytes": 1048576, "total": 268000000,
  "model_id": null, "servable": null, "reason": null, "error": null,
  "created_ms": 0, "updated_ms": 0 }
```
- `state`: `queued`, `downloading`, `verifying`, `registering`, `done`, `failed`, `cancelled`, `interrupted`. `interrupted` means apid restarted during the import: POST it again to resume from the `.part` files.
- `files[].state`: `pending`, `downloading`, `verified` (LFS files compared to their sha256; others to their size), `done`, `failed`.
- `model_id`: the registry id once registered. `servable`: true when the model has a serving contract (served), false when it is registered as `imported`. `reason`: why it is not servable.

Downloads: one import runs at a time (the rest are `queued`). Streamed to `<file>.part` and resumed with
`Range: bytes=<n>-` (a server that ignores the range restarts the file). The sha256 of an LFS file is verified
before the rename to the final name; a mismatch deletes the `.part` and fails the file. The size of the
selection is checked against `foundry.hf_max_gb` (GiB, default 20, range 1..=500) and against free disk
(`df`; the import needs its remaining bytes plus 1 GiB free). The bearer token goes only to `huggingface.co`,
never to the CDN hosts. Redirects are followed by hand: each hop must be `https` and on the allowed hosts
(`huggingface.co`, `hf.co` and their subdomains: `cdn-lfs*.hf.co`, `cas-bridge.xethub.hf.co`, ...), and no hop
may be on the owner's `fetch_block`. Every request has timeouts and the User-Agent `apid-foundry/<version>`.

Registration: a finished import is registered in the registry with `source: {hf, revision, as, path, servable_reason?}`.
- `servable` with a `model_card.json` that has a `serving` block and an ONNX graph: registered through the same
  path as `models/import` (state `trained`), then the source is attached.
- A `servable` text model (an ONNX graph and a `tokenizer.json`, for a served task): its contract is derived from
  the download's `config.json` (labels from `id2label`, length from `max_position_embeddings`, capped at 128) and
  written into the download's `model_card.json` as `serving: {kind: "text", task, max_len, labels, graph}`.
  It is registered as state `trained` (promote and shadow work as for any model), and its card's `allowed_roles`
  is `["breeder"]`.
- Otherwise (every `base` import, and a `servable` without a serving contract): registered as state `imported`.
  It is not servable, is never compiled, and promote and shadow refuse it; `servable_reason` says why.
- Registration is idempotent: the same repo and revision registers once.

Progress frames (bus kind `foundry`; the data is the frame):
`{"kind":"hf_import","import":3,"id":"Xenova/...","file":"onnx/model.onnx","bytes":1048576,"total":268000000,"state":"downloading"}`
Byte progress is sent at most every 512 KiB or 500 ms per file, plus one frame at each file and import state
change. `state` is the import's or the file's state; `file` is empty on import-level frames.

Token: `PUT /v1/foundry/hf/token` stores it in `<state>/foundry/hf/token` (mode 0600, the way provider secrets
are stored). It is never sent to a VM, never logged, never returned; the API reports only `hf_token_set`.

Settings: `foundry.hf_max_gb` (in `GET/PUT /v1/settings`; default 20, 1..=500).

Files: the router learns one pattern, `{name..}` (the last segment captures the rest of the path), in
`crates/apz-http/src/router.rs`. Everything else is `foundry/hf.rs` and `foundry/hf_import.rs`, plus small hooks
in `foundry/mod.rs`, `foundry/routes.rs`, `foundry/registry.rs` (State `imported`, `ModelRecord.source`),
`settings.rs` and `state.rs` (the import queue on `App`).

### Text models (phase 2)

A text model runs in the hub with tract (the ONNX graph) beside the `tokenizers` crate (`tokenizer.json`, read
offline; nothing is fetched at runtime). The graph keeps its symbolic batch and sequence dims; each call pads or
truncates the input to `max_len` and runs `[1, max_len]` tensors. Two tasks:
- `text-classification`: input `{text}`, output `{label, scores: [{label, score}]}`; softmax scores, highest first,
  labels from config.json's `id2label`.
- `feature-extraction`: input `{text}`, output `{embedding: [...]}`; the mean of the unmasked token vectors, L2-normalised.
Any other task is refused at import with its reason. Invoke a shadow or promoted model with
`POST /v1/foundry/models/{id}/invoke` (body `{text}`), or through `invoke_model` for permitted roles.
Models imported before this version stay `imported`: the registry matches an import by repo and revision, so
`POST /v1/foundry/models/{id}/rederive` upgrades them without a new download. The breeder has a read-only `foundry_hf_search` tool (the route's compact shape); imports stay owner-only.

Not done in this version: deleting a registered model's files; more than one import at a time; a model card for `base` records beyond the HF summary; upgrading imports made before phase 2 (they stay `imported`).
