"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""
# fmt: off
# pylint: disable=all
# ruff: noqa

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
    try:
        import builtins as _builtins
    except Exception:
        return None
    return _lookup_from_builtins_obj(_builtins, name)


def runtime_active():
    helper = globals().get(_LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    if globals().get("_molt_runtime", False) or globals().get("_molt_intrinsics_strict", False):
        return True
    try:
        import builtins as _builtins
    except Exception:
        return False
    reg = _lookup_builtin_obj(_builtins, _REGISTRY_NAME)
    if isinstance(reg, dict):
        return True
    helper = _lookup_builtin_obj(_builtins, _LOOKUP_HELPER_NAME)
    if _is_intrinsic_value(helper):
        return True
    return bool(
        getattr(_builtins, "_molt_runtime", False)
        or getattr(_builtins, "_molt_intrinsics_strict", False)
    )


def require_intrinsic(name, namespace=None):
    if namespace is not None:
        getter = getattr(namespace, "get", None)
        if getter is not None:
            value = getter(name)
            if _is_intrinsic_value(value):
                return value
            value = getter(_REGISTRY_NAME)
            if isinstance(value, dict):
                hit = value.get(name)
                if _is_intrinsic_value(hit):
                    return hit
                resolver = value.get("_molt_lazy_resolve")
                if callable(resolver):
                    resolved = resolver(name)
                    if _is_intrinsic_value(resolved):
                        return resolved
            caller_builtins = getter("__builtins__")
            value = _lookup_from_builtins_obj(caller_builtins, name)
            if value is not None:
                return value
            value = _lookup_runtime_builtins(name)
            if value is not None:
                return value
        else:
            if _is_intrinsic_value(namespace):
                return namespace

    module_registry = globals().get(_REGISTRY_NAME)
    if isinstance(module_registry, dict):
        value = module_registry.get(name)
        if _is_intrinsic_value(value):
            return value
        resolver = module_registry.get("_molt_lazy_resolve")
        if callable(resolver):
            resolved = resolver(name)
            if _is_intrinsic_value(resolved):
                return resolved

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
    try:
        return require_intrinsic(name, namespace)
    except RuntimeError:
        return None
