"""Intrinsic resolution helpers for Molt stdlib modules.

Missing intrinsics must raise immediately; fallback is not permitted.
"""
# fmt: off
# pylint: disable=all
# ruff: noqa

_REGISTRY_NAME = "_molt_intrinsics"
_LOOKUP_HELPER_NAME = "_molt_intrinsic_lookup"

# Cache the bootstrap builtins module as a fallback, but prefer the current
# runtime builtins module on each lookup. Compiled runtimes may replace or
# finish populating the builtins module after this wrapper is imported.
import builtins as _bootstrap_builtins


def _is_intrinsic_value(value):
    return callable(value)


def _lookup_builtin_obj(builtins_obj, name):
    if isinstance(builtins_obj, dict):
        return builtins_obj.get(name)
    return getattr(builtins_obj, name, None)


def _lookup_name_and_alias(
    builtins_obj,
    name,
    _lookup_obj=_lookup_builtin_obj,
    _is_value=_is_intrinsic_value,
):
    value = _lookup_obj(builtins_obj, name)
    if _is_value(value):
        return value
    if name.startswith("molt_"):
        alias = f"_molt_{name[5:]}"
        value = _lookup_obj(builtins_obj, alias)
        if _is_value(value):
            return value
    return None


def _lookup_registry(
    builtins_obj,
    name,
    _registry_name=_REGISTRY_NAME,
    _lookup_obj=_lookup_builtin_obj,
    _is_value=_is_intrinsic_value,
):
    reg = _lookup_obj(builtins_obj, _registry_name)
    if isinstance(reg, dict):
        value = reg.get(name)
        if _is_value(value):
            return value
        resolver = reg.get("_molt_lazy_resolve")
        if callable(resolver):
            resolved = resolver(name)
            if _is_value(resolved):
                return resolved
    return None


def _lookup_from_builtins_obj(
    builtins_obj,
    name,
    _lookup_reg=_lookup_registry,
    _lookup_alias=_lookup_name_and_alias,
):
    if builtins_obj is None:
        return None
    value = _lookup_reg(builtins_obj, name)
    if value is not None:
        return value
    return _lookup_alias(builtins_obj, name)


def _runtime_builtins_obj(_bootstrap=_bootstrap_builtins):
    try:
        return __import__("builtins")
    except Exception:
        return _bootstrap


def _lookup_runtime_builtins(
    name,
    _runtime_builtins=_runtime_builtins_obj,
    _lookup_builtins=_lookup_from_builtins_obj,
):
    return _lookup_builtins(_runtime_builtins(), name)


def runtime_active(
    _helper_name=_LOOKUP_HELPER_NAME,
    _runtime_builtins=_runtime_builtins_obj,
    _lookup_obj=_lookup_builtin_obj,
    _is_value=_is_intrinsic_value,
):
    builtins_obj = _runtime_builtins()
    helper = globals().get(_helper_name)
    if _is_value(helper):
        return True
    if globals().get("_molt_runtime", False) or globals().get("_molt_intrinsics_strict", False):
        return True
    helper = _lookup_obj(builtins_obj, _helper_name)
    if _is_value(helper):
        return True
    return bool(
        _lookup_obj(builtins_obj, "_molt_runtime")
        or _lookup_obj(builtins_obj, "_molt_intrinsics_strict")
    )


def require_intrinsic(name, namespace=None):
    # Keep this path self-contained. During early import bootstrap this function
    # is often called from modules that are still being initialized, and the
    # resolver must not depend on cross-module global rebinding correctness.
    registry_name = "_molt_intrinsics"
    helper_name = "_molt_intrinsic_lookup"

    def is_intrinsic_value(value):
        return callable(value)

    def lookup_obj(obj, key):
        if isinstance(obj, dict):
            return obj.get(key)
        return getattr(obj, key, None)

    def runtime_builtins_obj():
        try:
            return __import__("builtins")
        except Exception:
            return globals().get("_bootstrap_builtins")

    def lookup_name_and_alias(builtins_obj):
        value = lookup_obj(builtins_obj, name)
        if is_intrinsic_value(value):
            return value
        if name.startswith("molt_"):
            value = lookup_obj(builtins_obj, f"_molt_{name[5:]}")
            if is_intrinsic_value(value):
                return value
        return None

    def lookup_registry(builtins_obj):
        reg = lookup_obj(builtins_obj, registry_name)
        if isinstance(reg, dict):
            value = reg.get(name)
            if is_intrinsic_value(value):
                return value
            resolver = reg.get("_molt_lazy_resolve")
            if callable(resolver):
                resolved = resolver(name)
                if is_intrinsic_value(resolved):
                    return resolved
        return None

    # 1. Check module-level lookup helper first (injected by runtime).
    helper = globals().get(helper_name)
    if callable(helper):
        value = helper(name)
        if value is not None:
            return value

    # 2. Check the current runtime builtins registry and direct aliases.
    builtins_obj = runtime_builtins_obj()
    value = lookup_registry(builtins_obj)
    if value is not None:
        return value
    value = lookup_name_and_alias(builtins_obj)
    if value is not None:
        return value

    # 3. Check the current runtime builtins module for the lookup helper.
    builtins_helper = lookup_obj(builtins_obj, helper_name)
    if callable(builtins_helper):
        value = builtins_helper(name)
        if value is not None:
            return value

    active = (
        callable(helper)
        or bool(globals().get("_molt_runtime", False))
        or bool(globals().get("_molt_intrinsics_strict", False))
        or callable(builtins_helper)
        or bool(lookup_obj(builtins_obj, "_molt_runtime"))
        or bool(lookup_obj(builtins_obj, "_molt_intrinsics_strict"))
    )
    if not active:
        raise RuntimeError("runtime inactive")

    raise RuntimeError(f"intrinsic unavailable: {name}")


def load_intrinsic(name, namespace=None):
    try:
        return require_intrinsic(name, namespace)
    except RuntimeError:
        return None
