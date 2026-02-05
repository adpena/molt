"""CPython shims removed.

Molt does not provide CPython fallback paths. Use Molt binaries or the
differential harness for comparisons.
"""

from __future__ import annotations

from typing import Any


class ShimsUnavailable(RuntimeError):
    pass


def _unavailable(*_args: Any, **_kwargs: Any) -> None:
    raise ShimsUnavailable("CPython shims are not available in Molt")


install = _unavailable
load_runtime = _unavailable

__all__ = ["ShimsUnavailable", "install", "load_runtime"]
