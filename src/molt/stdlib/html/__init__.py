"""Intrinsic-backed html module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_html_escape = _require_intrinsic("molt_html_escape")
_molt_html_unescape = _require_intrinsic("molt_html_unescape")


def escape(s: str, quote: bool = True) -> str:
    """Replace special characters "&", "<" and ">" to HTML-safe sequences.

    If the optional flag *quote* is true (the default), the quotation mark
    characters, both double quote (") and single quote ('), are also translated.
    """
    return str(_molt_html_escape(str(s), bool(quote)))


def unescape(s: str) -> str:
    """Convert all named and numeric character references (e.g. &gt;, &#62;,
    &x3e;) in the string s to the corresponding unicode characters."""
    return str(_molt_html_unescape(str(s)))


globals().pop("_require_intrinsic", None)
