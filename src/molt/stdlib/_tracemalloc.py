"""Low-level memory-tracing helpers used by `tracemalloc`.

CPython exposes _tracemalloc as a built-in module that backs the
`tracemalloc` Python module's allocation tracking. In molt's compiled-
binary contract there is no runtime allocation hook to insert traces
into — the binary's allocations go through specialized native paths
that bypass any Python-level tracing.

This module provides a deterministic shim that matches the import
surface so `import tracemalloc` succeeds. start() / stop() are no-ops;
get_traced_memory() returns (0, 0); is_tracing() returns False;
take_snapshot() raises RuntimeError matching CPython's behavior when
tracing is not active.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


_started = False
_traceback_limit = 1


def start(nframe: int = 1) -> None:
    """Mark tracing as started (no-op shim — molt has no allocation hooks)."""
    global _started, _traceback_limit
    if nframe < 1:
        raise ValueError("the number of frames must be at least 1")
    _started = True
    _traceback_limit = int(nframe)


def stop() -> None:
    """Mark tracing as stopped."""
    global _started
    _started = False


def is_tracing() -> bool:
    """Tracing always reports inactive — there are no traces to retrieve."""
    return _started


def clear_traces() -> None:
    """No-op — there are no traces to clear."""
    return None


def get_traceback_limit() -> int:
    return _traceback_limit


def get_traced_memory() -> tuple[int, int]:
    """Return (current, peak) traced memory in bytes.

    molt's compiled-binary contract bypasses Python-level allocation
    tracking, so both values are honest zeros.
    """
    return (0, 0)


def get_tracemalloc_memory() -> int:
    return 0


def reset_peak() -> None:
    return None


def _get_object_traceback(obj):
    """Return a tuple representing the traceback that allocated `obj`.

    Always returns None in this shim — no tracing has ever been active.
    """
    return None


def _get_traces():
    """Return the list of (size, traceback) tuples for all current traces.

    Always empty in this shim.
    """
    return []


__all__ = [
    "start",
    "stop",
    "is_tracing",
    "clear_traces",
    "get_traceback_limit",
    "get_traced_memory",
    "get_tracemalloc_memory",
    "reset_peak",
    "_get_object_traceback",
    "_get_traces",
]


globals().pop("_require_intrinsic", None)
