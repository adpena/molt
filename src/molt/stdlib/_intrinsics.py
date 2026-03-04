"""Canonical intrinsic loader for Molt runtime and stdlib bootstrap."""


def _runtime_helper_ref() -> object | None:
    try:
        return _molt_intrinsic_lookup  # type: ignore[name-defined]  # noqa: F821
    except NameError:
        return None


def runtime_active() -> bool:
    helper = _runtime_helper_ref()
    if callable(helper):
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
    if not callable(helper):
        return None
    value = helper(name)
    if callable(value):
        return value
    return None


def require_intrinsic(name: str, namespace: object = None) -> object:
    del namespace
    helper = _runtime_helper_ref()
    if callable(helper):
        value = helper(name)
        if callable(value):
            return value
        raise RuntimeError(f"intrinsic unavailable: {name}")
    if not runtime_active():
        raise RuntimeError("Molt runtime intrinsics unavailable (runtime inactive)")
    raise RuntimeError(f"intrinsic unavailable: {name}")
