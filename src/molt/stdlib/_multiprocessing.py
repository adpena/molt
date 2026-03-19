"""Compatibility surface for CPython `_multiprocessing`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

sem_unlink = _require_intrinsic("molt_process_drop")
_molt_semlock_new = _require_intrinsic("molt_semaphore_new")
_molt_semlock_drop = _require_intrinsic("molt_semaphore_drop")


class SemLock:
    def __init__(self, value: int = 1) -> None:
        self._handle = _molt_semlock_new(value)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_semlock_drop(handle)
            except Exception:
                pass


flags = {}

__all__ = ["SemLock", "flags", "sem_unlink"]
