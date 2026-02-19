"""Intrinsic-backed filename matching helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["filter", "fnmatch", "fnmatchcase", "translate"]


_MOLT_FNMATCH = _require_intrinsic("molt_fnmatch", globals())
_MOLT_FNMATCHCASE = _require_intrinsic("molt_fnmatchcase", globals())
_MOLT_FNMATCH_FILTER = _require_intrinsic("molt_fnmatch_filter", globals())
_MOLT_FNMATCH_TRANSLATE = _require_intrinsic("molt_fnmatch_translate", globals())


def fnmatch(name: str | bytes, pat: str | bytes) -> bool:
    return bool(_MOLT_FNMATCH(name, pat))


def fnmatchcase(name: str | bytes, pat: str | bytes) -> bool:
    return bool(_MOLT_FNMATCHCASE(name, pat))


def filter(names, pat: str | bytes):
    matches = _MOLT_FNMATCH_FILTER(names, pat, False)
    if not isinstance(matches, list):
        raise RuntimeError("fnmatch filter intrinsic returned invalid value")
    if isinstance(pat, bytes):
        expected = bytes
    else:
        expected = str
    for item in matches:
        if not isinstance(item, expected):
            raise RuntimeError("fnmatch filter intrinsic returned invalid value")
    return matches


def translate(pat: str) -> str:
    out = _MOLT_FNMATCH_TRANSLATE(pat)
    if not isinstance(out, str):
        raise RuntimeError("fnmatch translate intrinsic returned invalid value")
    return out
