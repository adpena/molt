"""Public API surface shim for ``curses.has_key``."""

from __future__ import annotations


def has_key(_ch: int) -> bool:
    return False
