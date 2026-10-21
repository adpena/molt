#!/usr/bin/env python3
"""File and process probes for the canonical Molt dev driver."""

from __future__ import annotations

import time
from pathlib import Path

from molt import process_model


def probe_path(path: Path) -> dict:
    """Size + mtime for a file via os.stat (never `ls`/`stat` shell tools)."""
    if not path.exists():
        return {"path": str(path), "exists": False}
    st = path.stat()
    return {
        "path": str(path),
        "exists": True,
        "size": st.st_size,
        "mtime": st.st_mtime,
        "mtime_iso": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(st.st_mtime)),
        "age_s": round(time.time() - st.st_mtime, 3),
    }


def probe_pid(pid: int) -> dict:
    """The `probe --pid` record: liveness from the process model, never `ps`.

    An exited, unreaped child (a zombie) is not alive.
    """
    return {"pid": pid, "alive": process_model.pid_alive(pid)}
