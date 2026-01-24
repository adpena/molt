"""Errno constants for Molt.

In compiled code, errno constants come from the runtime via `_molt_errno_constants()`.
"""

from __future__ import annotations


def _load_errno_constants() -> tuple[dict[str, int], dict[int, str]]:
    # Prefer runtime-provided constants for deterministic, platform-correct values.
    try:
        res = _molt_errno_constants()  # type: ignore[name-defined]
    except NameError:
        res = None
    except Exception:
        res = None
    if isinstance(res, tuple) and len(res) == 2:
        left, right = res
        if isinstance(left, dict) and isinstance(right, dict):
            return left, right

    # Host-Python fallback (used by tools/tests that import `molt.stdlib.errno`).
    try:
        import importlib as _importlib

        _py_errno = _importlib.import_module("errno")
    except Exception:
        _py_errno = None
    if _py_errno is None:
        return {}, {}

    constants = {
        name: getattr(_py_errno, name) for name in dir(_py_errno) if name.isupper()
    }
    errorcode = dict(getattr(_py_errno, "errorcode", {}))
    return constants, errorcode


constants, errorcode = _load_errno_constants()
globals().update(constants)
__all__ = sorted(constants) + ["errorcode"]
