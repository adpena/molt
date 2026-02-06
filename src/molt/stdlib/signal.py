"""Minimal signal support for Molt."""

from __future__ import annotations

from types import ModuleType
from typing import cast

from molt.stdlib import enum as _enum

_capabilities: ModuleType | None
try:
    from molt import capabilities as _capabilities_raw
except Exception:
    _capabilities = None
else:
    _capabilities = cast(ModuleType, _capabilities_raw)

__all__ = [
    "SIGINT",
    "SIG_DFL",
    "SIG_IGN",
    "Signals",
    "default_int_handler",
    "getsignal",
    "signal",
]


def _require_cap() -> None:
    if _capabilities is None:
        return
    if _capabilities.trusted():
        return
    _capabilities.require("process.signal")


SIGINT = 2


class _SigDefault:
    pass


class _SigIgnore:
    pass


SIG_DFL = _SigDefault()
SIG_IGN = _SigIgnore()


class Signals(_enum.IntEnum):
    SIGINT = SIGINT


def default_int_handler(_signum: int, _frame: object | None = None) -> None:
    raise KeyboardInterrupt


_handlers: dict[int, object] = {SIGINT: default_int_handler}


def getsignal(sig: int) -> object:
    _require_cap()
    return _handlers.get(int(sig), SIG_DFL)


def signal(sig: int, handler: object) -> object:
    _require_cap()
    sig_num = int(sig)
    prev = _handlers.get(sig_num, SIG_DFL)
    _handlers[sig_num] = handler
    return prev
