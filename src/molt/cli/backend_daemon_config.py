from __future__ import annotations

import functools
import os


@functools.lru_cache(maxsize=32)
def _backend_daemon_enabled_cached(os_name: str, raw: str) -> bool:
    if os_name != "posix":
        return False
    return raw.strip().lower() not in {"0", "false", "no", "off"}


def _backend_daemon_enabled() -> bool:
    return _backend_daemon_enabled_cached(
        os.name,
        os.environ.get("MOLT_BACKEND_DAEMON", "1"),
    )
