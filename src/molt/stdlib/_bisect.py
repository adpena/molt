"""Binary search helpers for Molt (_bisect intrinsics)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_BISECT_LEFT = _require_intrinsic("molt_bisect_left", globals())
_MOLT_BISECT_RIGHT = _require_intrinsic("molt_bisect_right", globals())
_MOLT_INSORT_LEFT = _require_intrinsic("molt_insort_left", globals())
_MOLT_INSORT_RIGHT = _require_intrinsic("molt_insort_right", globals())

__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]

T = TypeVar("T")

_bisect_left = _require_intrinsic("molt_bisect_left", globals())
_bisect_right = _require_intrinsic("molt_bisect_right", globals())
_insort_left = _require_intrinsic("molt_insort_left", globals())
_insort_right = _require_intrinsic("molt_insort_right", globals())

__all__ = ["bisect_left", "bisect_right", "insort_left", "insort_right"]

class _IntrinsicBuiltin:
    __slots__ = ("_impl", "_returns_none", "__name__", "__qualname__", "__module__")

    def __init__(self, name: str, impl, *, returns_none: bool):
        self._impl = impl
        self._returns_none = returns_none
        self.__name__ = name
        self.__qualname__ = name
        self.__module__ = "_bisect"

    def __call__(self, a, x, lo=0, hi=None, *, key=None):
        out = self._impl(a, x, lo, hi, key)
        if self._returns_none:
            return None
        return out

    def __repr__(self):
        return f"<built-in function {self.__name__}>"


class builtin_function_or_method(_IntrinsicBuiltin):
    pass


bisect_left = builtin_function_or_method(
    "bisect_left", _bisect_left, returns_none=False
)
bisect_right = builtin_function_or_method(
    "bisect_right", _bisect_right, returns_none=False
)
insort_left = builtin_function_or_method("insort_left", _insort_left, returns_none=True)
insort_right = builtin_function_or_method("insort_right", _insort_right, returns_none=True)
del builtin_function_or_method
