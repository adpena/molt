"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""

_REGISTRY_NAME = "_molt_intrinsics"


def _registry():
    builtins_obj = globals().get("__builtins__")
    if isinstance(builtins_obj, dict):
        reg = builtins_obj.get(_REGISTRY_NAME)
    else:
        reg = getattr(builtins_obj, _REGISTRY_NAME, None)
    if isinstance(reg, dict):
        return reg
    try:
        import builtins as _builtins
    except Exception:
        return None
    reg = getattr(_builtins, _REGISTRY_NAME, None)
    if isinstance(reg, dict):
        return reg
    return None


def load_intrinsic(name, namespace=None):
    if namespace is not None:
        getter = getattr(namespace, "get", None)
        if getter is not None:
            value = getter(name)
            if value is not None:
                return value
        else:
            # Allow direct intrinsic values to be passed in as the namespace.
            return namespace
    reg = _registry()
    if reg is not None:
        value = reg.get(name)
        if value is not None:
            return value
    raise RuntimeError(f"intrinsic unavailable: {name}")


def require_intrinsic(name, namespace=None):
    return load_intrinsic(name, namespace)
