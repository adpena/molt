"""CPython shims removed.

Molt no longer supports CPython fallback or bridge shims. Use differential
tests or native Molt binaries to validate behavior.
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
