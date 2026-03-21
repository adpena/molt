"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""

import builtins as _builtins

_LOOKUP_NAME = "_molt_intrinsic_lookup"
_RUNTIME_FLAG = "_molt_runtime"
_STRICT_FLAG = "_molt_intrinsics_strict"


def _is_intrinsic_value(value):
    return callable(value)


def _lookup_helper():
    helper = globals().get(_LOOKUP_NAME)
    if callable(helper):
        return helper
    helper = getattr(_builtins, _LOOKUP_NAME, None)
    if callable(helper):
        return helper
    return None


def runtime_active():
    return bool(
        _lookup_helper() is not None
        or globals().get(_RUNTIME_FLAG, False)
        or globals().get(_STRICT_FLAG, False)
        or getattr(_builtins, _RUNTIME_FLAG, False)
        or getattr(_builtins, _STRICT_FLAG, False)
    )


def load_intrinsic(name, namespace=None):
    helper = _lookup_helper()
    if helper is None:
        return None
    value = helper(name)
    if _is_intrinsic_value(value):
        return value
    return None


def require_intrinsic(name, namespace=None):
    value = load_intrinsic(name, namespace)
    if value is not None:
        return value
    if not runtime_active():
        raise RuntimeError("Molt runtime intrinsics unavailable (runtime inactive)")
    raise RuntimeError(f"intrinsic unavailable: {name}")
