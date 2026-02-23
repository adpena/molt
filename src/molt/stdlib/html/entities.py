"""Intrinsic-backed html.entities module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_html_entities_codepoint2name = _require_intrinsic(
    "molt_html_entities_codepoint2name", globals()
)
_molt_html_entities_name2codepoint = _require_intrinsic(
    "molt_html_entities_name2codepoint", globals()
)
_molt_html_entities_html5 = _require_intrinsic("molt_html_entities_html5", globals())

codepoint2name: dict[int, str] = _molt_html_entities_codepoint2name()
name2codepoint: dict[str, int] = _molt_html_entities_name2codepoint()
html5: dict[str, str] = _molt_html_entities_html5()
