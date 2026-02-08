"""Intrinsic-backed textwrap subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["TextWrapper", "indent"]

_MOLT_TEXTWRAP_WRAP = _require_intrinsic("molt_textwrap_wrap", globals())
_MOLT_TEXTWRAP_FILL = _require_intrinsic("molt_textwrap_fill", globals())
_MOLT_TEXTWRAP_INDENT = _require_intrinsic("molt_textwrap_indent", globals())


class TextWrapper:
    def __init__(self, width: int = 70) -> None:
        self.width = int(width)

    def wrap(self, text: str) -> list[str]:
        out = _MOLT_TEXTWRAP_WRAP(text, self.width)
        if not isinstance(out, list) or not all(isinstance(item, str) for item in out):
            raise RuntimeError("textwrap.wrap intrinsic returned invalid value")
        return list(out)

    def fill(self, text: str) -> str:
        out = _MOLT_TEXTWRAP_FILL(text, self.width)
        if not isinstance(out, str):
            raise RuntimeError("textwrap.fill intrinsic returned invalid value")
        return out


def indent(text: str, prefix: str) -> str:
    out = _MOLT_TEXTWRAP_INDENT(text, prefix)
    if not isinstance(out, str):
        raise RuntimeError("textwrap.indent intrinsic returned invalid value")
    return out
