#!/usr/bin/env python3
"""File and process probes for the canonical Molt dev driver."""

from __future__ import annotations

import os
import time
from pathlib import Path


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
    """Liveness of a pid via os.kill(pid, 0) (never `ps`/`kill -0` shell text)."""
    alive: bool
    detail = ""
    try:
        os.kill(pid, 0)
        alive = True
    except ProcessLookupError:
        alive = False
    except PermissionError:
        # Exists but owned by another user: still ALIVE for liveness purposes.
        alive = True
        detail = "owned by another user"
    except OSError as exc:
        alive = False
        detail = str(exc)
    return {"pid": pid, "alive": alive, "detail": detail}
