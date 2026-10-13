import threading

import numpy as np

from apz_train import hub as hubmod
from apz_train import wire
from apz_train.diloco import HubClient, average_deltas, outer_update


def test_wire_roundtrip_is_exact():
    tensors = {"b": np.arange(6, dtype=np.float32).reshape(2, 3), "a": np.array([1.5, -2.25], np.float32)}
    meta, out = wire.unpack(wire.pack({"island": "i1", "round": 3}, tensors))
    assert meta == {"island": "i1", "round": 3}
    assert set(out) == {"a", "b"}
    np.testing.assert_array_equal(out["b"], tensors["b"])
    np.testing.assert_array_equal(out["a"], tensors["a"])


def test_average_is_sample_weighted():
    pending = {"i1": (3.0, {"w": np.array([1.0], np.float32)}), "i2": (1.0, {"w": np.array([5.0], np.float32)})}
    assert np.allclose(average_deltas(pending)["w"], [2.0])


def test_outer_step_with_no_momentum_is_plain_sync():
    base = {"w": np.array([1.0, 2.0], np.float32)}
    local = {"w": np.array([0.5, 2.5], np.float32)}
    delta = {"w": base["w"] - local["w"]}
    new, _ = outer_update({"w": base["w"].copy()}, {"w": np.zeros(2, np.float32)}, delta,
                          outer_lr=1.0, momentum=0.0)
    np.testing.assert_allclose(new["w"], local["w"])


def test_hub_quorum_closes_round_and_refuses_stale_delta():
    hub = hubmod.Hub(["a", "b"], staleness=1, wait_s=30)
    hub.init({"w": np.zeros(2, np.float32)})
    server = hubmod.serve(hub, "127.0.0.1", 0)
    client = HubClient(f"http://127.0.0.1:{server.server_address[1]}")
    try:
        out = {}
        def go(isl):
            out[isl] = client.submit(isl, 0, 10.0, {"w": np.ones(2, np.float32)})
        threads = [threading.Thread(target=go, args=(i,)) for i in ("a", "b")]
        [t.start() for t in threads]
        [t.join(30) for t in threads]
        assert hub.round == 1 and out["a"][0] and out["b"][0]
        np.testing.assert_allclose(out["a"][2]["w"], out["b"][2]["w"])
        # Nesterov with lr 0.7, momentum 0.9: first round moves the weights by -0.7 * (1 + 0.9) * 1
        np.testing.assert_allclose(hub.state()[1]["w"], np.full(2, -0.7 * 1.9), rtol=1e-6)
        # Round 0 is now one behind; with staleness 1 it is still accepted, but round -2 would not be.
        accepted, rnd, _ = client.submit("c", -2, 1.0, {"w": np.ones(2, np.float32)})
        assert accepted is False and rnd == 1
    finally:
        server.shutdown()
