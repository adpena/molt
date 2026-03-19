"""Intrinsic-backed gettext shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity.

__all__ = ["gettext", "ngettext"]

_MOLT_GETTEXT_GETTEXT = _require_intrinsic("molt_gettext_gettext")
_MOLT_GETTEXT_NGETTEXT = _require_intrinsic("molt_gettext_ngettext")


def gettext(message: object) -> object:
    return _MOLT_GETTEXT_GETTEXT(message)


def ngettext(singular: object, plural: object, n: object) -> object:
    return _MOLT_GETTEXT_NGETTEXT(singular, plural, n)

globals().pop("_require_intrinsic", None)
