"""Minimal shlex support for Molt."""

from __future__ import annotations

__all__ = ["quote"]

# Mirror CPython's shlex.quote escaping rules without regex support.
_SAFE_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_@%+=:,./-"
)


def _is_safe(s: str) -> bool:
    for ch in s:
        if ch not in _SAFE_CHARS:
            return False
    return True


def quote(s: str) -> str:
    if not s:
        return "''"
    if _is_safe(s):
        return s
    return "'" + s.replace("'", "'\"'\"'") + "'"
