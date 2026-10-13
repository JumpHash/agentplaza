"""Progress events: one JSON object per line on stdout. Step and eval events come from rank 0 only;
human-readable logs go to stderr so stdout stays parseable."""

import json
import sys
import time

_CTX = {"island": None, "rank": 0}


def set_context(island=None, rank=0):
    _CTX.update(island=island, rank=rank)


def is_main():
    return _CTX["rank"] == 0


def _plain(o):
    if hasattr(o, "item"):
        return o.item()
    if hasattr(o, "tolist"):
        return o.tolist()
    return str(o)


def _write(ev, fields):
    line = {"ev": ev, "t": round(time.time(), 3), "island": _CTX["island"], "rank": _CTX["rank"], **fields}
    sys.stdout.write(json.dumps(line, default=_plain) + "\n")
    sys.stdout.flush()


def emit(ev, **fields):
    """Rank 0 only. Use for step, eval, checkpoint, trial and done."""
    if is_main():
        _write(ev, fields)


def emit_error(message, **fields):
    """Any rank may report an error; the caller exits non-zero afterwards."""
    _write("error", {"message": message, **fields})


def log(msg):
    sys.stderr.write(f"[apz_train rank={_CTX['rank']}] {msg}\n")
    sys.stderr.flush()
