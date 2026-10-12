"""Compatibility surface for CPython `_multiprocessing`."""

from _intrinsics import require_intrinsic as _require_intrinsic


_molt_semlock_new = _require_intrinsic("molt_semaphore_new")
_molt_semlock_drop = _require_intrinsic("molt_semaphore_drop")


def sem_unlink(name: str, /) -> None:
    if not isinstance(name, str):
        raise TypeError("sem_unlink() argument must be str")
    if str.__contains__(name, "\x00"):
        raise ValueError("embedded null character")
    raise NotImplementedError("named semaphore unlink is unavailable in this runtime")


class SemLock:
    def __init__(self, value: int = 1) -> None:
        self._handle = _molt_semlock_new(value)

    def __del__(self, _drop=_molt_semlock_drop) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        object.__setattr__(self, "_handle", None)
        try:
            _drop(handle)
        except Exception:
            pass


flags = {}

__all__ = ["SemLock", "flags", "sem_unlink"]


globals().pop("_require_intrinsic", None)
