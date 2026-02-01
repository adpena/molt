"""Spawn entrypoint for Molt multiprocessing."""

from __future__ import annotations

import multiprocessing as _multiprocessing
import os


def _debug_spawn(message: str) -> None:
    trace = getattr(_multiprocessing, "_spawn_trace", None)
    if callable(trace):
        try:
            trace(message)
        except Exception:
            pass


def _get_env(key: str, default: str = "") -> str:
    try:
        value = _molt_env_get_raw(key, default)  # type: ignore[name-defined]  # noqa: F821
        return str(value)
    except NameError:  # pragma: no cover - host fallback
        pass
    except Exception:
        return default
    try:
        data = _molt_env_snapshot()  # type: ignore[name-defined]  # noqa: F821
    except NameError:  # pragma: no cover - host fallback
        data = None
    except Exception:
        data = None
    if isinstance(data, dict):
        try:
            return str(data.get(key, default))
        except Exception:
            return default
    try:
        return os.environ.get(key, default)
    except Exception:
        return default


_spawn_flag = _get_env("MOLT_MP_SPAWN")
_entry_override = _get_env("MOLT_ENTRY_MODULE")

if _spawn_flag == "1" or _entry_override == "multiprocessing.spawn":
    _debug_spawn(
        f"spawn.py _spawn_main type={type(getattr(_multiprocessing, '_spawn_main', None)).__name__}"
    )
    _spawn_main = getattr(_multiprocessing, "_spawn_main", None)
    if not callable(_spawn_main):
        raise RuntimeError("multiprocessing._spawn_main is unavailable")
    _spawn_main()
