"""Minimal gettext shim for Molt."""

from __future__ import annotations

__all__ = ["gettext", "ngettext"]


def gettext(message: str) -> str:
    return message


def ngettext(singular: str, plural: str, n: int) -> str:
    return singular if int(n) == 1 else plural
