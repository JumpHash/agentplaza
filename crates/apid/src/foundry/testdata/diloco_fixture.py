"""Regenerate the apid Foundry test fixtures from the Python reference (apz_train).

    uv run --project /mnt/fast/AgentPlaza/trainer python -I \
        /mnt/fast/AgentPlaza/crates/apid/src/foundry/testdata/diloco_fixture.py

Writes wire_fixture.apzw next to this file (an APZW1 payload from apz_train.wire.pack) and prints
the Rust constants that the hub test in diloco.rs hard-codes: the initial weights, each island's
delta and sample count, and the global weights after every closed round.

The scenario: islands a, b, c (quorum 3, staleness 2). Round 0 closes on quorum; round 1 has only
island a and closes on the wait_s timeout; round 2 closes on quorum; island b's round-0 delta is
refused as stale and a round-9 delta as ahead; round 3 closes on quorum.
The inputs are dyadic rationals so they are exact in float32 on both sides.
"""

import pathlib
import sys
import threading
import time

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[5] / "trainer"))

from apz_train import wire  # noqa: E402
from apz_train.hub import Hub  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent


def f32(shape, vals):
    return np.array(vals, dtype=np.float32).reshape(shape)


def rust(arr):
    return "vec![" + ", ".join(str(np.float32(x)) for x in np.asarray(arr, dtype=np.float32).ravel()) + "]"


def dump(label, tensors):
    for k in sorted(tensors):
        print(f"{label}.{k} = {rust(tensors[k])}")


def submit_all(hub, subs):
    """Submit (island, round, samples, delta) from threads, one at a time, so that the hub's
    pending order (the float32 summation order) is island order. Returns {island: (accepted, round, weights)}."""
    out, threads = {}, []
    for island, rnd, samples, delta in subs:
        th = threading.Thread(target=lambda i=island, r=rnd, s=samples, d=delta:
                              out.__setitem__(i, hub.submit(i, r, s, d)))
        th.start()
        threads.append(th)
        while True:
            with hub.cond:
                if island in hub.pending or hub.round != rnd:
                    break
            time.sleep(0.001)
    for th in threads:
        th.join(timeout=60)
    return out


def main():
    init = {"w": f32((2, 3), [0.5, -1.0, 0.25, 2.0, -0.125, 0.75]),
            "b": f32((4,), [0.0, 1.5, -2.5, 4.0])}
    hub = Hub(["a", "b", "c"], outer_lr=0.7, momentum=0.9, staleness=2, wait_s=2.0)

    r0, w0 = hub.init(init)
    assert r0 == 0
    dump("W0", w0)

    d = {
        "D0a": {"w": f32((2, 3), [0.25, 0.5, -0.75, 1.0, -0.5, 0.125]), "b": f32((4,), [0.5, -1.0, 0.25, 2.0])},
        "D0b": {"w": f32((2, 3), [-0.5, 0.25, 0.5, 0.75, 0.0, -0.25]), "b": f32((4,), [1.0, 0.5, -0.5, -1.5])},
        "D0c": {"w": f32((2, 3), [1.5, -0.25, 0.0, -1.0, 0.375, 0.5]), "b": f32((4,), [-0.25, 0.0, 1.0, 0.5])},
        "D1a": {"w": f32((2, 3), [0.5] * 6), "b": f32((4,), [0.125, 0.25, 0.375, 0.5])},
        "D2a": {"w": f32((2, 3), [1.0, -1.0, 0.5, 0.25, 0.25, -0.5]), "b": f32((4,), [0.0, 0.5, 0.5, -0.5])},
        "D2b": {"w": f32((2, 3), [0.125, 0.0, -0.125, 0.5, -0.5, 0.0]), "b": f32((4,), [2.0, -1.0, 0.0, 0.25])},
        "D2c": {"w": f32((2, 3), [-0.5, 0.5, 0.25, 0.0, 1.0, -1.0]), "b": f32((4,), [0.75, 0.25, -0.25, 1.0])},
        "D3a": {"w": f32((2, 3), [0.5, 0.25, 0.0, -0.25, 0.5, 1.0]), "b": f32((4,), [0.125, -0.125, 0.5, 0.0])},
        "D3b": {"w": f32((2, 3), [-1.0, 0.0, 0.75, 0.25, -0.5, 0.5]), "b": f32((4,), [0.5, 0.5, 0.5, 0.5])},
        "D3c": {"w": f32((2, 3), [0.0, 0.5, -0.5, 0.5, 0.0, 0.25]), "b": f32((4,), [-0.5, 1.0, 0.0, -1.0])},
    }
    for name in sorted(d):
        dump(name, d[name])

    # Round 0: quorum a, b, c (samples 10, 30, 20).
    out = submit_all(hub, [("a", 0, 10.0, d["D0a"]), ("b", 0, 30.0, d["D0b"]), ("c", 0, 20.0, d["D0c"])])
    assert all(v[0] and v[1] == 1 for v in out.values())
    w1 = out["a"][2]
    assert all(out[i][2]["w"].tolist() == w1["w"].tolist() for i in out)
    dump("W1", w1)

    # Round 1: only a (samples 7); closes on the wait_s timeout.
    t0 = time.monotonic()
    out = submit_all(hub, [("a", 1, 7.0, d["D1a"])])
    print(f"# round 1 timeout took {time.monotonic() - t0:.2f}s", file=sys.stderr)
    assert out["a"][0] and out["a"][1] == 2
    w2 = out["a"][2]
    dump("W2", w2)

    # Round 2: quorum a, b, c (samples 5, 5, 10).
    out = submit_all(hub, [("a", 2, 5.0, d["D2a"]), ("b", 2, 5.0, d["D2b"]), ("c", 2, 10.0, d["D2c"])])
    assert all(v[0] and v[1] == 3 for v in out.values())
    w3 = out["a"][2]
    dump("W3", w3)

    # Refusals: island b's round-0 delta is stale (0 + 2 < 3); c's round-9 delta is ahead.
    stale = hub.submit("b", 0, 30.0, d["D0b"])
    ahead = hub.submit("c", 9, 20.0, d["D0c"])
    assert stale[0] is False and stale[1] == 3 and ahead[0] is False and ahead[1] == 3
    assert stale[2]["w"].tolist() == w3["w"].tolist()
    print(f"# stale refusal: accepted={stale[0]} round={stale[1]}")
    print(f"# ahead refusal: accepted={ahead[0]} round={ahead[1]}")

    # Round 3: quorum a, b, c (samples 4, 8, 12).
    out = submit_all(hub, [("a", 3, 4.0, d["D3a"]), ("b", 3, 8.0, d["D3b"]), ("c", 3, 12.0, d["D3c"])])
    assert all(v[0] and v[1] == 4 for v in out.values())
    w4 = out["a"][2]
    dump("W4", w4)

    # Averaging helpers in isolation: the float32 arithmetic the Rust code must reproduce.
    v = {k: np.zeros_like(x) for k, x in init.items()}
    print(f"# dtype check: {(0.9 * v['w']).dtype} {(np.float32(0.9) * v['w']).dtype}")

    # Binary payload fixture.
    meta = {"island": "a", "round": 3, "samples": 12.5}
    tensors = {"w": f32((2, 3), [0.5, -1.0, 0.25, 2.0, -0.125, 0.75]), "b": f32((4,), [1.0, -2.0, 0.5, 4.0])}
    blob = wire.pack(meta, tensors)
    (HERE / "wire_fixture.apzw").write_bytes(blob)
    print(f"# wire_fixture.apzw: {len(blob)} bytes")


if __name__ == "__main__":
    main()
