"""Binary payload for the hub protocol (see PROTOCOL.md): a JSON header and raw float32 tensors.

Layout: b"APZW1" | uint32 big-endian header length | UTF-8 JSON header | tensor bytes.
The header is {"meta": {...}, "tensors": [{"name", "shape", "dtype", "offset", "nbytes"}, ...]}.
Only float32 is carried. Nothing is pickled.
"""

import json
import struct

import numpy as np

MAGIC = b"APZW1"


def pack(meta, tensors):
    index, chunks, offset = [], [], 0
    for name in sorted(tensors):
        arr = np.ascontiguousarray(tensors[name], dtype=np.float32)
        raw = arr.tobytes()
        index.append({"name": name, "shape": list(arr.shape), "dtype": "float32",
                      "offset": offset, "nbytes": len(raw)})
        chunks.append(raw)
        offset += len(raw)
    header = json.dumps({"meta": meta, "tensors": index}, separators=(",", ":")).encode("utf-8")
    return MAGIC + struct.pack(">I", len(header)) + header + b"".join(chunks)


def unpack(data):
    if not data.startswith(MAGIC):
        raise ValueError("not an APZW1 payload")
    (hlen,) = struct.unpack(">I", data[len(MAGIC):len(MAGIC) + 4])
    start = len(MAGIC) + 4
    header = json.loads(data[start:start + hlen].decode("utf-8"))
    body = memoryview(data)[start + hlen:]
    tensors = {}
    for t in header["tensors"]:
        if t["dtype"] != "float32":
            raise ValueError(f"unsupported dtype {t['dtype']}")
        arr = np.frombuffer(body, dtype=np.float32, count=t["nbytes"] // 4, offset=t["offset"])
        tensors[t["name"]] = arr.reshape(t["shape"]).copy()
    return header["meta"], tensors
