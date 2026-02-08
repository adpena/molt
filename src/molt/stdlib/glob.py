"""Intrinsic-backed glob support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["glob", "iglob", "has_magic"]


_MOLT_GLOB_HAS_MAGIC = _require_intrinsic("molt_glob_has_magic", globals())
_MOLT_GLOB = _require_intrinsic("molt_glob", globals())


def has_magic(pathname: str) -> bool:
    return bool(_MOLT_GLOB_HAS_MAGIC(pathname))


def glob(pathname: str) -> list[str]:
    matches = _MOLT_GLOB(pathname)
    if not isinstance(matches, list):
        raise RuntimeError("glob intrinsic returned invalid value")
    for match in matches:
        if not isinstance(match, str):
            raise RuntimeError("glob intrinsic returned invalid value")
    return matches


def iglob(pathname: str):
    yield from glob(pathname)
