"""Spawn entrypoint for Molt multiprocessing."""

from __future__ import annotations

import multiprocessing as _multiprocessing

from _intrinsics import require_intrinsic as _require_intrinsic



def _debug_spawn(message: str) -> None:
    trace = getattr(_multiprocessing, "_spawn_trace", None)
    if callable(trace):
        try:
            trace(message)
        except Exception:
            pass


_MOLT_ENV_GET = _require_intrinsic("molt_env_get", globals())


def _get_env(key: str, default: str = "") -> str:
    return str(_MOLT_ENV_GET(key, default))


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
