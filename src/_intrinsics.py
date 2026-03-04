"""Canonical intrinsic loader for Molt runtime and stdlib bootstrap."""

from collections.abc import Callable
from typing import cast


IntrinsicLookup = Callable[[str], object]


def _runtime_helper_ref() -> IntrinsicLookup | None:
    try:
        helper = _molt_intrinsic_lookup  # type: ignore[name-defined]  # noqa: F821
    except NameError:
        return None
    if not callable(helper):
        return None
    return cast(IntrinsicLookup, helper)


def runtime_active() -> bool:
    helper = _runtime_helper_ref()
    if helper is not None:
        return True
    try:
        if bool(_molt_runtime):  # type: ignore[name-defined]  # noqa: F821
            return True
    except NameError:
        pass
    try:
        if bool(_molt_intrinsics_strict):  # type: ignore[name-defined]  # noqa: F821
            return True
    except NameError:
        pass
    return False


def load_intrinsic(name: str, namespace: object = None) -> object | None:
    del namespace
    helper = _runtime_helper_ref()
    if helper is None:
        return None
    value = helper(name)
    if callable(value):
        return value
    return None


def require_intrinsic(name: str, namespace: object = None) -> object:
    del namespace
    helper = _runtime_helper_ref()
    if helper is not None:
        value = helper(name)
        if callable(value):
            return value
        raise RuntimeError(f"intrinsic unavailable: {name}")
    if not runtime_active():
        raise RuntimeError("Molt runtime intrinsics unavailable (runtime inactive)")
    raise RuntimeError(f"intrinsic unavailable: {name}")
