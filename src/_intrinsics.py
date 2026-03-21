"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""

import builtins as _REAL_BUILTINS

_REGISTRY_NAME = "_molt_intrinsics"
_LOOKUP_HELPER_NAME = "_molt_intrinsic_lookup"


def _is_intrinsic_value(value):
    return callable(value)


def _lookup_builtin_obj(builtins_obj, name):
    if isinstance(builtins_obj, dict):
        return builtins_obj.get(name)
    return getattr(builtins_obj, name, None)


def _lookup_name_and_alias(builtins_obj, name):
    value = _lookup_builtin_obj(builtins_obj, name)
    if _is_intrinsic_value(value):
        return value
    if name.startswith("molt_"):
        alias = f"_molt_{name[5:]}"
        value = _lookup_builtin_obj(builtins_obj, alias)
        if _is_intrinsic_value(value):
            return value
    return None


def _lookup_registry(builtins_obj, name):
    reg = _lookup_builtin_obj(builtins_obj, _REGISTRY_NAME)
    if isinstance(reg, dict):
        value = reg.get(name)
        if _is_intrinsic_value(value):
            return value
        # Lazy resolution: intrinsics are not eagerly populated at startup.
        # Call the runtime resolver which builds the function object on demand
        # and caches it in the registry dict for subsequent lookups.
        resolver = reg.get("_molt_lazy_resolve")
        if callable(resolver):
            resolved = resolver(name)
            if _is_intrinsic_value(resolved):
                return resolved
    return None


def _lookup_from_builtins_obj(builtins_obj, name):
    if builtins_obj is None:
        return None
    value = _lookup_registry(builtins_obj, name)
    if value is not None:
        return value
    return _lookup_name_and_alias(builtins_obj, name)


def _lookup_runtime_builtins(name):
    return _lookup_from_builtins_obj(_REAL_BUILTINS, name)


def _lookup_helper_obj(name):
    helper = globals().get(_LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return helper(name)
    helper = _lookup_builtin_obj(_REAL_BUILTINS, _LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return helper(name)
    return None


def runtime_active():
    """Check if the Molt runtime is active (intrinsics registry installed)."""
    helper = globals().get(_LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    if globals().get("_molt_runtime", False) or globals().get("_molt_intrinsics_strict", False):
        return True
    reg = _lookup_builtin_obj(_REAL_BUILTINS, _REGISTRY_NAME)
    if isinstance(reg, dict):
        return True
    helper = _lookup_builtin_obj(_REAL_BUILTINS, _LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    return bool(
        getattr(_REAL_BUILTINS, "_molt_runtime", False)
        or getattr(_REAL_BUILTINS, "_molt_intrinsics_strict", False)
    )


def require_intrinsic(name, namespace=None):
    if namespace is not None:
        getter = getattr(namespace, "get", None)
        if getter is not None:
            caller_builtins = getter("__builtins__")
            value = _lookup_from_builtins_obj(caller_builtins, name)
            if value is not None:
                return value
        else:
            caller_builtins = getattr(namespace, "__builtins__", None)
            value = _lookup_from_builtins_obj(caller_builtins, name)
            if value is not None:
                return value

    value = _lookup_helper_obj(name)
    if _is_intrinsic_value(value):
        return value

    module_builtins = globals().get("__builtins__")
    value = _lookup_from_builtins_obj(module_builtins, name)
    if value is not None:
        return value

    value = _lookup_runtime_builtins(name)
    if value is not None:
        return value

    if not runtime_active():
        raise RuntimeError("runtime inactive")

    raise RuntimeError(f"intrinsic unavailable: {name}")


def load_intrinsic(name, namespace=None):
    """Load an intrinsic, returning None if unavailable instead of raising."""
    try:
        return require_intrinsic(name, namespace)
    except RuntimeError:
        return None
