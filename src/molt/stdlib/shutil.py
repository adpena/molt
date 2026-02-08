"""Intrinsic-backed shutil subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["copyfile", "which"]


_MOLT_SHUTIL_COPYFILE = _require_intrinsic("molt_shutil_copyfile", globals())
_MOLT_SHUTIL_WHICH = _require_intrinsic("molt_shutil_which", globals())


def copyfile(src: str, dst: str) -> str:
    out = _MOLT_SHUTIL_COPYFILE(src, dst)
    if not isinstance(out, str):
        raise RuntimeError("shutil.copyfile intrinsic returned invalid value")
    return out


def which(cmd: str, mode: int | None = None, path: str | None = None) -> str | None:
    del mode
    out = _MOLT_SHUTIL_WHICH(cmd, path)
    if out is None:
        return None
    if not isinstance(out, str):
        raise RuntimeError("shutil.which intrinsic returned invalid value")
    return out
