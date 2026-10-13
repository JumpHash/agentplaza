"""The training loop: wrapping (DDP or FSDP), inner steps, evaluation, checkpoints, GPU-hour budget.

Batches are a deterministic function of (step, seed, world, rank), so a resumed run sees exactly the
data an uninterrupted run would have seen. Checkpoints hold resumable state only (weights, optimizer,
step, budget); the deliverable is always the ONNX export.
"""

import contextlib
import math
import os
import time

import torch
import torch.distributed as dist
from torch.utils.data import default_collate

from .events import emit, log


class Budget:
    """GPU-hours metered from wall time times devices; shared by all trainers of one job."""

    def __init__(self, limit_gpu_hours=None, world=1):
        self.limit = limit_gpu_hours
        self.world = world
        self.base = 0.0
        self.t0 = time.monotonic()

    def seconds(self):
        return self.base + (time.monotonic() - self.t0) * self.world

    def hours(self):
        return self.seconds() / 3600.0

    def exhausted(self):
        return self.limit is not None and self.seconds() >= self.limit * 3600.0


class Trainer:
    def __init__(self, recipe, cfg, train_ds, ctx, out_dir, budget, *, val_ds=None, frozen_ds=None,
                 shard=False, resume=True):
        self.recipe, self.cfg, self.ctx, self.budget = recipe, dict(cfg), ctx, budget
        self.train_ds, self.val_ds, self.frozen_ds = train_ds, val_ds, frozen_ds
        self.bs = int(self.cfg["batch_size"])
        self.world = ctx.world
        if len(train_ds) < self.bs * self.world:
            raise ValueError(f"training split has {len(train_ds)} rows, need at least {self.bs * self.world}")
        self.shard = bool(shard) and self.world > 1
        self.out_dir = out_dir
        self.inputs = recipe.inputs
        self.step, self.samples = 0, 0
        self.best, self.bad, self.stop_early = None, 0, False
        self.stopped = None
        self.last_ckpt_step = None
        self._perm = (None, None)
        self.amp = bool(self.cfg.get("amp", True)) and ctx.is_cuda and torch.cuda.is_bf16_supported()
        os.makedirs(out_dir, exist_ok=True)

        self.model = recipe.build_model(self.cfg)
        ck = self._read_ckpt() if resume else None
        self.loaded_extra = dict(ck.get("extra", {})) if ck is not None else {}
        if ck is not None:
            self.model.load_state_dict(ck["model"])
            self.step, self.samples = int(ck["step"]), int(ck["samples"])
            budget.base = float(ck.get("budget_seconds", 0.0))
            log(f"resuming from step {self.step}")
        self.model.to(ctx.device)
        self.net = self._wrap()
        self.opt = torch.optim.AdamW(self.model.parameters(), lr=float(self.cfg["lr"]),
                                     weight_decay=float(self.cfg["weight_decay"]))
        if ck is not None:
            if self.shard:
                from torch.distributed.fsdp import FullyShardedDataParallel as FSDP
                self.opt.load_state_dict(FSDP.optim_state_dict_to_load(self.net, self.opt, ck["opt"]))
            else:
                self.opt.load_state_dict(ck["opt"])

    # ----- setup helpers -------------------------------------------------------------------------
    def _read_ckpt(self):
        path = os.path.join(self.out_dir, "ckpt.pt")
        if not os.path.exists(path):
            return None
        return torch.load(path, map_location="cpu", weights_only=True)

    def _wrap(self):
        if self.world <= 1:
            return self.model
        if self.shard:
            from torch.distributed.fsdp import FullyShardedDataParallel as FSDP
            return FSDP(self.model, device_id=self.ctx.device if self.ctx.is_cuda else None,
                        use_orig_params=True)
        from torch.nn.parallel import DistributedDataParallel as DDP
        return DDP(self.model, device_ids=[self.ctx.local_rank] if self.ctx.is_cuda else None)

    def _amp(self):
        if self.amp:
            return torch.autocast("cuda", dtype=torch.bfloat16)
        return contextlib.nullcontext()

    # ----- schedule and data order ---------------------------------------------------------------
    def lr_at(self, step):
        base, warm = float(self.cfg["lr"]), int(self.cfg["warmup"])
        total = max(1, int(self.cfg["steps"]))
        if step < warm:
            return base * (step + 1) / warm
        progress = min(1.0, (step - warm) / max(1, total - warm))
        return base * (0.1 + 0.9 * 0.5 * (1.0 + math.cos(math.pi * progress)))

    def _indices(self, step):
        """This rank's slice of the global batch for `step` (epoch-wise permutation, seeded by epoch)."""
        n = len(self.train_ds)
        global_bs = self.bs * self.world
        per_epoch = max(1, n // global_bs)
        epoch, pos = divmod(step, per_epoch)
        if self._perm[0] != epoch:
            gen = torch.Generator().manual_seed(int(self.cfg["seed"]) + epoch)
            self._perm = (epoch, torch.randperm(n, generator=gen))
        start = pos * global_bs + self.ctx.rank * self.bs
        return self._perm[1][start:start + self.bs].tolist()

    def _batch(self, idx):
        batch = default_collate([self.train_ds[i] for i in idx])
        return {k: v.to(self.ctx.device) for k, v in batch.items()}

    # ----- stepping ------------------------------------------------------------------------------
    def train_step(self):
        self.recipe.bind(self.cfg)
        for group in self.opt.param_groups:
            group["lr"] = self.lr_at(self.step)
        batch = self._batch(self._indices(self.step))
        self.net.train()
        self.opt.zero_grad(set_to_none=True)
        with self._amp():
            out = self.net(*[batch[k] for k in self.inputs])
            loss = self.recipe.loss(batch, out)
        loss.backward()
        if not self.shard:
            torch.nn.utils.clip_grad_norm_(self.model.parameters(), 1.0)
        self.opt.step()
        self.step += 1
        self.samples += self.bs * self.world
        return loss.detach()

    def _stop_now(self):
        """Budget decision agreed by all ranks, so no rank leaves a collective alone."""
        flag = torch.tensor([1.0 if self.budget.exhausted() else 0.0], device=self.ctx.device)
        if self.world > 1:
            dist.all_reduce(flag, op=dist.ReduceOp.MAX)
        return bool(flag.item())

    def fit(self, until, *, eval_every=0, checkpoint_every=0, log_every=50):
        """Train until global step `until`, emitting events from the main rank."""
        while self.step < until and not self.stop_early:
            if self._stop_now():
                self.stopped = "budget"
                log("GPU-hour budget exhausted")
                break
            loss = self.train_step()
            if self.ctx.is_main and self.step % log_every == 0:
                emit("step", step=self.step, loss=float(loss), lr=self.lr_at(self.step - 1),
                     samples=self.samples)
            if eval_every and self.step % eval_every == 0 and self.val_ds is not None:
                m = self.evaluate(self.val_ds)
                emit("eval", step=self.step, split="val", metrics=m)
                self._track_early(m)
            if checkpoint_every and self.step % checkpoint_every == 0:
                self.save_ckpt()

    def _track_early(self, metrics):
        patience = int(self.cfg.get("patience", 0))
        obj = metrics["expected_cost"]
        if self.best is None or obj < self.best - 1e-9:
            self.best, self.bad = obj, 0
        else:
            self.bad += 1
            if patience and self.bad >= patience:
                self.stop_early = True
                log(f"early stop at step {self.step}")

    @torch.no_grad()
    def evaluate(self, ds, chunk=1024):
        """Metrics on a split. Runs on every rank (FSDP forward is collective); callers emit."""
        if ds is None or len(ds) == 0:
            return None
        self.recipe.bind(self.cfg)
        self.net.eval()
        outs, parts = [], {}
        for s in range(0, len(ds), chunk):
            batch = default_collate([ds[i] for i in range(s, min(len(ds), s + chunk))])
            out = self.net(*[batch[k].to(self.ctx.device) for k in self.inputs])
            outs.append(out.float().cpu())
            for k, v in batch.items():
                parts.setdefault(k, []).append(v)
        self.net.train()
        batch_all = {k: torch.cat(v) for k, v in parts.items()}
        return self.recipe.metrics(batch_all, torch.cat(outs))

    # ----- state in and out ----------------------------------------------------------------------
    def full_state(self):
        """CPU state dict of the unwrapped model. Under FSDP this is collective (rank0 gets it)."""
        if self.shard:
            from torch.distributed.fsdp import FullyShardedDataParallel as FSDP, StateDictType
            from torch.distributed.fsdp import FullStateDictConfig
            cfg = FullStateDictConfig(offload_to_cpu=True, rank0_only=True)
            with FSDP.state_dict_type(self.net, StateDictType.FULL_STATE_DICT, cfg):
                return self.net.state_dict()
        return {k: v.detach().cpu().clone() for k, v in self.model.state_dict().items()}

    def load_weights(self, state):
        """Set the model on every rank from rank 0's state (a dict of tensors, or None off rank 0)."""
        if self.ctx.is_main:
            self.model.load_state_dict({k: torch.as_tensor(v) for k, v in state.items()})
        if self.world > 1:
            for t in self.model.state_dict().values():
                t_dev = t.to(self.ctx.device)
                dist.broadcast(t_dev, src=0)
                t.copy_(t_dev.cpu())

    def save_ckpt(self, extra=None):
        sd = self.full_state()
        if self.shard:
            from torch.distributed.fsdp import FullyShardedDataParallel as FSDP
            opt_sd = FSDP.optim_state_dict(self.net, self.opt)
        else:
            opt_sd = self.opt.state_dict()
        if self.ctx.is_main:
            path = os.path.join(self.out_dir, "ckpt.pt")
            tmp = path + ".tmp"
            torch.save({"step": self.step, "samples": self.samples, "model": sd, "opt": opt_sd,
                        "budget_seconds": self.budget.seconds(), "extra": extra or {}}, tmp)
            os.replace(tmp, path)
            self.last_ckpt_step = self.step
            emit("checkpoint", step=self.step, path=path)
        if self.world > 1:
            dist.barrier()

    def get_state(self):
        """In-memory state for population search: model and optimizer, on CPU."""
        return {"model": {k: v.detach().cpu().clone() for k, v in self.model.state_dict().items()},
                "opt": self.opt.state_dict(), "step": self.step, "samples": self.samples}

    def set_state(self, state):
        self.model.load_state_dict(state["model"])
        self.opt.load_state_dict(state["opt"])
        self.step, self.samples = state["step"], state["samples"]
