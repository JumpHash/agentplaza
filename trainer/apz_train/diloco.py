"""Loose mode: DiLoCo-style low-communication training across islands.

Each island trains H inner steps from the current global weights, then sends its pseudo-gradient
delta = base - local to the hub. The hub averages deltas weighted by samples and applies an outer
Nesterov step (see apz_train.hub). Islands never talk to each other. Only the island's rank 0 speaks
to the hub; the other ranks receive the new weights by broadcast.
"""

import time
import urllib.error
import urllib.request

import numpy as np
import torch
import torch.distributed as dist

from . import wire
from .events import emit, log


def average_deltas(pending):
    """pending: {island: (samples, delta)} -> sample-weighted mean of the deltas (float32)."""
    total = float(sum(s for s, _ in pending.values())) or 1.0
    keys = next(iter(pending.values()))[1].keys()
    return {k: sum(np.float32(s / total) * d[k] for s, d in pending.values()).astype(np.float32)
            for k in keys}


def outer_update(weights, velocity, avg_delta, outer_lr=0.7, momentum=0.9):
    """DiLoCo outer step: Nesterov momentum on the averaged pseudo-gradient, in place."""
    for k in weights:
        velocity[k] = (momentum * velocity[k] + avg_delta[k]).astype(np.float32)
        weights[k] = (weights[k] - outer_lr * (avg_delta[k] + momentum * velocity[k])).astype(np.float32)
    return weights, velocity


class HubClient:
    """Speaks the protocol in PROTOCOL.md with urllib; retries transient connection errors."""

    def __init__(self, url, timeout=900.0, attempts=5):
        self.url, self.timeout, self.attempts = url.rstrip("/"), timeout, attempts

    def _call(self, method, path, body=b""):
        last = None
        for i in range(self.attempts):
            req = urllib.request.Request(self.url + path, data=body or None, method=method,
                                         headers={"Content-Type": "application/octet-stream"})
            try:
                with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                    return resp.status, resp.read()
            except urllib.error.HTTPError as e:
                return e.code, e.read()
            except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
                last = e
                log(f"hub call {path} failed ({e}); retry {i + 1}")
                time.sleep(min(30.0, 2.0 ** i))
        raise ConnectionError(f"hub unreachable at {self.url}: {last}")

    def init(self, island, weights):
        _, data = self._call("POST", "/init", wire.pack({"island": island}, weights))
        meta, tensors = wire.unpack(data)
        return int(meta["round"]), tensors

    def state(self):
        _, data = self._call("GET", "/state")
        meta, tensors = wire.unpack(data)
        return int(meta["round"]), tensors

    def submit(self, island, round_, samples, delta):
        status, data = self._call("POST", "/round", wire.pack(
            {"island": island, "round": int(round_), "samples": float(samples)}, delta))
        meta, tensors = wire.unpack(data)
        return bool(meta.get("accepted", status == 200)), int(meta["round"]), tensors


def _bcast_int(ctx, value):
    if ctx.world <= 1:
        return int(value)
    t = torch.tensor([int(value)], dtype=torch.long, device=ctx.device)
    dist.broadcast(t, src=0)
    return int(t.item())


def run_island(trainer, hub_url, island, rounds, inner_steps, resume=None):
    """Drive the island's rounds. `resume` is (round, base) from a checkpoint, or None.
    Returns the final global weights as numpy arrays on rank 0 (None elsewhere)."""
    ctx = trainer.ctx
    client = HubClient(hub_url) if ctx.is_main else None
    samples_per_round = inner_steps * trainer.bs * ctx.world

    if resume is not None:
        r, base = resume
    else:
        base = None
        r = 0
        if ctx.is_main:
            init = {k: v.numpy() for k, v in trainer.full_state().items()}
            r, base = client.init(island, init)
        r = _bcast_int(ctx, r)
        trainer.load_weights(base)

    while r < rounds:
        trainer.fit(until=(r + 1) * inner_steps)
        if ctx.is_main:
            local = {k: v.numpy() for k, v in trainer.full_state().items()}
            delta = {k: (base[k] - local[k]).astype(np.float32) for k in base}
            accepted, new_round, base = client.submit(island, r, samples_per_round, delta)
            emit("round", round=r, accepted=accepted, hub_round=new_round, step=trainer.step)
            r_next = new_round
        else:
            r_next = 0
        stopped = trainer.stopped == "budget"
        r = _bcast_int(ctx, r_next)
        trainer.load_weights(base if ctx.is_main else None)
        extra = {"round": r, "base": {k: torch.from_numpy(v) for k, v in base.items()}} if ctx.is_main else {}
        trainer.save_ckpt(extra=extra)
        if trainer.val_ds is not None:
            m = trainer.evaluate(trainer.val_ds)
            emit("eval", step=trainer.step, round=r, split="val", metrics=m)
        if stopped:
            break
    return base if ctx.is_main else None
