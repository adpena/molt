"""Errno constants for Molt.

In compiled code, errno constants come from the runtime via `molt_errno_constants`.
Missing or invalid intrinsic payloads are a hard error (no host fallback).
"""

from _intrinsics import require_intrinsic as _require_intrinsic


def _load_errno_constants():
    intrinsic = _require_intrinsic("molt_errno_constants", globals())
    payload = intrinsic()
    if not isinstance(payload, tuple):
        raise RuntimeError("errno intrinsics unavailable")
    if len(payload) != 2:
        raise RuntimeError("errno intrinsics unavailable")
    constants_map = payload[0]
    errorcode_map = payload[1]
    if not isinstance(constants_map, dict):
        raise RuntimeError("errno intrinsics unavailable")
    if not isinstance(errorcode_map, dict):
        raise RuntimeError("errno intrinsics unavailable")
    return constants_map, errorcode_map


_errno_payload = _load_errno_constants()
constants = _errno_payload[0]
errorcode = _errno_payload[1]
import sys as _sys

_mod = _sys.modules.get(__name__)
if _mod is not None:
    _mod.__dict__.update(constants)
else:
    import sys as _sys2
    _sys2.modules[__name__].__dict__.update(constants)
del _sys, _mod
__all__ = sorted(constants.keys()) + ["errorcode"]
