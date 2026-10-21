#!/usr/bin/env python3
"""File and process probes for the canonical Molt dev driver."""

from __future__ import annotations

import sys
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
    """Query a positive PID without signaling; unknown never proves absence."""
    alive: bool | None = None
    detail = ""
    maximum = 0xFFFFFFFF if sys.platform == "win32" else 0x7FFFFFFF
    if type(pid) is not int or not 0 < pid <= maximum:
        return {"pid": pid, "alive": None, "detail": "invalid positive process ID"}
    try:
        alive = process_model.pid_alive(pid)
    except OSError as exc:
        detail = str(exc)
    return {"pid": pid, "alive": alive, "detail": detail}
