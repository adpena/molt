"""Internal asyncio error reporting, independent of host environment grants."""

from __future__ import annotations

import sys


def _debug_write(message: str) -> None:
    err = getattr(sys, "stderr", None)
    if err is None or not hasattr(err, "write"):
        err = getattr(sys, "__stderr__", None)
    if err is not None and hasattr(err, "write"):
        err.write(f"{message}\n")
        flush_fn = getattr(err, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    out = getattr(sys, "stdout", None)
    if out is not None and hasattr(out, "write"):
        out.write(f"{message}\n")
        flush_fn = getattr(out, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    print(message)

