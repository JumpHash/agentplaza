"""Device selection and the process group. cuda and rocm both run through torch.cuda; nccl under
torchrun for those, gloo for everything else (CPU, MPS, XPU in this runtime)."""

import os
from dataclasses import dataclass

import torch
import torch.distributed as dist


@dataclass
class DistCtx:
    backend: str
    device: torch.device
    rank: int
    world: int
    local_rank: int

    @property
    def is_main(self):
        return self.rank == 0

    @property
    def is_cuda(self):
        return self.device.type == "cuda"


def detect():
    if torch.cuda.is_available():
        return "rocm" if getattr(torch.version, "hip", None) else "cuda"
    if hasattr(torch, "xpu") and torch.xpu.is_available():
        return "xpu"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def setup(requested="auto"):
    name = detect() if requested in (None, "", "auto") else requested
    world = int(os.environ.get("WORLD_SIZE", "1"))
    rank = int(os.environ.get("RANK", "0"))
    local = int(os.environ.get("LOCAL_RANK", "0"))
    if name in ("cuda", "rocm"):
        if not torch.cuda.is_available():
            raise RuntimeError(f"backend {name} requested but no GPU is visible")
        torch.cuda.set_device(local)
        device = torch.device("cuda", local)
    elif name == "xpu":
        torch.xpu.set_device(local)
        device = torch.device("xpu", local)
    elif name == "mps":
        device = torch.device("mps")
    elif name == "cpu":
        device = torch.device("cpu")
    else:
        raise ValueError(f"unknown backend {name!r}")
    if world > 1 and not dist.is_initialized():
        dist.init_process_group(backend="nccl" if device.type == "cuda" else "gloo")
    return DistCtx(backend=name, device=device, rank=rank, world=world, local_rank=local)


def teardown():
    if dist.is_initialized():
        dist.destroy_process_group()
