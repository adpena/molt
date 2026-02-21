"""Helpers for importlib.resources."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_RESOURCES_ONLY = _require_intrinsic(
    "molt_importlib_resources_only", globals()
)


def only(iterable, default=None, too_long=None):
    return _MOLT_IMPORTLIB_RESOURCES_ONLY(iterable, default, too_long)
