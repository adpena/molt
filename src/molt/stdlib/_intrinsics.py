"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""

_REGISTRY_NAME = "_molt_intrinsics"


def _lookup_builtin_obj(builtins_obj, name):
    if isinstance(builtins_obj, dict):
        return builtins_obj.get(name)
    return getattr(builtins_obj, name, None)


def _lookup_name_and_alias(builtins_obj, name):
    value = _lookup_builtin_obj(builtins_obj, name)
    if value is not None:
        return value
    if name.startswith("molt_"):
        alias = f"_molt_{name[5:]}"
        value = _lookup_builtin_obj(builtins_obj, alias)
        if value is not None:
            return value
    return None


def _lookup_registry(builtins_obj, name):
    reg = _lookup_builtin_obj(builtins_obj, _REGISTRY_NAME)
    if isinstance(reg, dict):
        value = reg.get(name)
        if value is not None:
            return value
    return None


def _lookup_from_builtins_obj(builtins_obj, name):
    if builtins_obj is None:
        return None
    value = _lookup_registry(builtins_obj, name)
    if value is not None:
        return value
    return _lookup_name_and_alias(builtins_obj, name)


def require_intrinsic(name, namespace=None):
    if namespace is not None:
        getter = getattr(namespace, "get", None)
        if getter is not None:
            value = getter(name)
            if value is not None:
                return value
            value = getter(_REGISTRY_NAME)
            if isinstance(value, dict):
                hit = value.get(name)
                if hit is not None:
                    return hit
            caller_builtins = getter("__builtins__")
            value = _lookup_from_builtins_obj(caller_builtins, name)
            if value is not None:
                return value
        else:
            # Allow direct intrinsic values to be passed in as the namespace.
            return namespace

    module_registry = globals().get(_REGISTRY_NAME)
    if isinstance(module_registry, dict):
        value = module_registry.get(name)
        if value is not None:
            return value

    module_builtins = globals().get("__builtins__")
    value = _lookup_from_builtins_obj(module_builtins, name)
    if value is not None:
        return value

    raise RuntimeError(f"intrinsic unavailable: {name}")


def load_intrinsic(name, namespace=None):
    return require_intrinsic(name, namespace)
