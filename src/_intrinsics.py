"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""
# fmt: off
# pylint: disable=all
# ruff: noqa

_REGISTRY_NAME = "_molt_intrinsics"
_LOOKUP_HELPER_NAME = "_molt_intrinsic_lookup"

# Cache a reference to the real builtins module at import time so that
# later monkeypatching of sys.modules["builtins"] cannot subvert lookup.
import builtins as _real_builtins


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
    return _lookup_from_builtins_obj(_real_builtins, name)


def runtime_active():
    helper = globals().get(_LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    if globals().get("_molt_runtime", False) or globals().get("_molt_intrinsics_strict", False):
        return True
    reg = _lookup_builtin_obj(_real_builtins, _REGISTRY_NAME)
    if isinstance(reg, dict):
        return True
    helper = _lookup_builtin_obj(_real_builtins, _LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    return bool(
        getattr(_real_builtins, "_molt_runtime", False)
        or getattr(_real_builtins, "_molt_intrinsics_strict", False)
    )


def require_intrinsic(name, namespace=None):
    # 1. Check module-level lookup helper first (injected by runtime).
    helper = globals().get(_LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        value = helper(name)
        if value is not None:
            return value

    # 2. Check real builtins for lookup helper (cached at import time).
    builtins_helper = getattr(_real_builtins, _LOOKUP_HELPER_NAME, None)
    if _is_intrinsic_value(builtins_helper):
        value = builtins_helper(name)
        if value is not None:
            return value

    # 3. Check the builtins-backed intrinsic registry used by host-side tests
    # and runtime bootstrap shims.
    value = _lookup_runtime_builtins(name)
    if value is not None:
        return value

    if not runtime_active():
        raise RuntimeError("runtime inactive")

    raise RuntimeError(f"intrinsic unavailable: {name}")


def load_intrinsic(name, namespace=None):
    try:
        return require_intrinsic(name, namespace)
    except RuntimeError:
        return None
