# The Foundry and the Lab: breeders, training on any GPUs, model agents

Status: design (2026-10-13). Builds on the codex draft (`docs/codex-draft/playbooks/breeding-guide.md`,
`schemas/training-job.schema.json`, statute S-006, the Lab town in `docs/towns-draft/towns/lab.yaml`).

## What it is

- **Breeders** are LLM agents (role `breeder`) whose job is to turn the plaza's own records into small,
  cheap models: they export data, check it, write a training job for a **vetted recipe**, submit it, read
  the results and request shadow mode. They never run arbitrary code on a GPU machine (0058).
- **The Foundry** is the hub's training system: recipes, datasets, jobs, the GPU placement planner, the
  trainer runtime on every machine with a GPU (or none: CPU training works too), the model registry, shadow
  mode and promotion.
- **Model agents** are what a promoted model becomes: a non-LLM agent type. It has a typed input and output
  (from its model card), runs as ONNX inference, and can only be *invoked* — it can't chat, take tasks,
  hold a wallet or call tools. Other agents (and the plaza's own routing) call it with `invoke_model`.
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
"trainer": { "runtime": "docker-nvidia", "ok": true, "detail": "nvidia-container-runtime 1.17" },
"fabric": ["192.168.1.20", "10.8.0.3"]                                  // addresses other machines may reach
```

| vendor | backend | collective | how the trainer runs | where |
|---|---|---|---|---|
| NVIDIA | `cuda` | NCCL | container, `--gpus device=…` (NVIDIA Container Toolkit; on Windows through Docker Desktop's WSL2 GPU support) | Linux, Windows |
| AMD | `rocm` | RCCL (torch's `nccl` backend on ROCm) | container with `--device /dev/kfd --device /dev/dri` | Linux |
| Intel | `xpu` | XCCL / oneCCL | container with `--device /dev/dri` | Linux |
| Apple | `mps` | gloo (no collective on MPS) | native sandboxed runner (no GPU in containers on macOS) | macOS |
| none | `cpu` | gloo | container (or the native runner) | anywhere |

Detection lives in noded (`nvidia-smi --query-gpu`, `rocm-smi --showproductname --json`, `/sys/class/drm`
for Intel, `system_profiler SPDisplaysDataType` on macOS) and the runtime check (`docker info` runtimes,
`/dev/kfd`, …). Desktop setup gains an optional **GPU training** requirement that installs what's missing
after asking (NVIDIA Container Toolkit, ROCm container support, or nothing on macOS).

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

`trainer/` in the repo: a Python package `apz_train` plus container images (`apz-trainer:cuda`,
`:rocm`, `:xpu`, `:cpu`) built in CI and published with releases; the macOS native runner uses a pinned
`uv` environment with the same package.
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
- Isolation: no network except the collective (tight islands) and the hub's node link (loose); datasets and
  the recipe read-only; output to a job directory; GPU-hours metered from the device time actually used.

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
  no wallet, no tools, no chat) served by apid with **tract** (pure-Rust ONNX inference: no native deps, so
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
