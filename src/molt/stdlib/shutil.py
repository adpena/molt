"""Intrinsic-backed shutil subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["copyfile", "rmtree", "which"]


_MOLT_SHUTIL_COPYFILE = _require_intrinsic("molt_shutil_copyfile", globals())
_MOLT_SHUTIL_WHICH = _require_intrinsic("molt_shutil_which", globals())
_MOLT_SHUTIL_RMTREE = _require_intrinsic("molt_shutil_rmtree", globals())


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


def rmtree(path: str, ignore_errors: bool = False) -> None:
    if ignore_errors:
        try:
            _MOLT_SHUTIL_RMTREE(path)
        except OSError:
            return
        return
    _MOLT_SHUTIL_RMTREE(path)
