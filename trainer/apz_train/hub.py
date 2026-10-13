"""The averaging hub for loose (DiLoCo) training: a stdlib HTTP server (see PROTOCOL.md).

Round r closes when every island has contributed, or `wait_s` has passed since the first contribution
(the slow islands then simply contribute fewer samples next round). A delta computed against a round
more than `staleness` behind the hub, or ahead of it, is refused with 409 and the reply carries the
current global weights so the island can rebase. In the plaza apid implements this same protocol.

    python -m apz_train.hub --islands i1,i2 --port 8800 --init weights.apzw
"""

import argparse
import http.server
import threading
import time

from . import wire
from .diloco import average_deltas, outer_update
from .events import log


class Hub:
    def __init__(self, islands, outer_lr=0.7, momentum=0.9, staleness=2, wait_s=120.0):
        self.quorum = len(islands)
        self.outer_lr, self.momentum = outer_lr, momentum
        self.staleness, self.wait_s = staleness, wait_s
        self.cond = threading.Condition()
        self.weights, self.velocity = None, None
        self.round = 0
        self.pending, self.first = {}, None

    def _copy(self):
        return {k: v.copy() for k, v in self.weights.items()}

    def init(self, weights):
        """The first island's initial weights become the global model; later calls get the same."""
        with self.cond:
            if self.weights is None:
                self.weights = {k: v.astype("float32").copy() for k, v in weights.items()}
                self.velocity = {k: v * 0 for k, v in self.weights.items()}
                log(f"hub initialised with {len(self.weights)} tensors")
            return self.round, self._copy()

    def state(self):
        with self.cond:
            return self.round, self._copy()

    def _close(self):
        avg = average_deltas(self.pending)
        outer_update(self.weights, self.velocity, avg, self.outer_lr, self.momentum)
        log(f"round {self.round} closed with {len(self.pending)} islands: {sorted(self.pending)}")
        self.pending, self.first = {}, None
        self.round += 1
        self.cond.notify_all()

    def submit(self, island, round_, samples, delta):
        with self.cond:
            if self.weights is None:
                raise RuntimeError("hub has no global weights yet; call /init first")
            if round_ < self.round - self.staleness or round_ > self.round:
                return False, self.round, self._copy()
            target = self.round
            if not self.pending:
                self.first = time.monotonic()
            self.pending[island] = (float(samples), delta)
            if len(self.pending) >= self.quorum:
                self._close()
            while self.round == target:
                left = self.first + self.wait_s - time.monotonic()
                if left <= 0:
                    self._close()
                    break
                self.cond.wait(timeout=left)
            return True, self.round, self._copy()


def make_handler(hub):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def _send(self, status, data):
            self.send_response(status)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/health":
                return self._send(200, wire.pack({"ok": True}, {}))
            if self.path == "/state":
                rnd, weights = hub.state()
                return self._send(200, wire.pack({"round": rnd}, weights))
            self._send(404, wire.pack({"error": "no such path"}, {}))

        def do_POST(self):
            try:
                body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                meta, tensors = wire.unpack(body)
                if self.path == "/init":
                    rnd, weights = hub.init(tensors)
                    return self._send(200, wire.pack({"round": rnd}, weights))
                if self.path == "/round":
                    ok, rnd, weights = hub.submit(meta["island"], int(meta["round"]),
                                                  float(meta["samples"]), tensors)
                    return self._send(200 if ok else 409, wire.pack({"round": rnd, "accepted": ok}, weights))
                self._send(404, wire.pack({"error": "no such path"}, {}))
            except (ValueError, KeyError, RuntimeError) as e:
                self._send(400, wire.pack({"error": str(e)}, {}))

    return Handler


def serve(hub, host="127.0.0.1", port=0):
    server = http.server.ThreadingHTTPServer((host, port), make_handler(hub))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def main(argv=None):
    p = argparse.ArgumentParser(description="DiLoCo averaging hub")
    p.add_argument("--islands", required=True, help="comma-separated island ids (the quorum)")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8800)
    p.add_argument("--staleness", type=int, default=2)
    p.add_argument("--wait", type=float, default=120.0)
    p.add_argument("--outer-lr", type=float, default=0.7)
    p.add_argument("--momentum", type=float, default=0.9)
    args = p.parse_args(argv)
    hub = Hub(args.islands.split(","), args.outer_lr, args.momentum, args.staleness, args.wait)
    server = http.server.ThreadingHTTPServer((args.host, args.port), make_handler(hub))
    log(f"hub listening on {args.host}:{server.server_address[1]} for {hub.quorum} islands")
    server.serve_forever()


if __name__ == "__main__":
    main()
