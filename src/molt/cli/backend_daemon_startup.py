from __future__ import annotations

import functools
import os


@functools.lru_cache(maxsize=32)
def _backend_daemon_start_timeout_cached(raw: str) -> float | None:
    value = raw.strip()
    if not value:
        # Cold daemon startup may compile stdlib batches. Too-small defaults
        # create false not-ready restarts that kill in-progress compilation.
        return 120.0
    try:
        parsed = float(value)
    except ValueError:
        return 120.0
    return parsed if parsed > 0 else None


def _backend_daemon_start_timeout() -> float | None:
    return _backend_daemon_start_timeout_cached(
        os.environ.get("MOLT_BACKEND_DAEMON_START_TIMEOUT", "")
    )


def _backend_daemon_spawn_probe_timeout(startup_timeout: float | None) -> float:
    if startup_timeout is None:
        return 0.25
    return min(startup_timeout, 0.25)
