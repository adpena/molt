"""Minimal tempfile shim for Molt."""

from __future__ import annotations

from molt.stdlib import os as _os

__all__ = ["gettempdir", "gettempdirb"]

_TEMP_DIR: str | None = None


def _pick_tempdir() -> str:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement full tempfile
    # candidate search, permissions checks, and secure fallback semantics.
    for key in ("TMPDIR", "TEMP", "TMP"):
        try:
            val = _os.getenv(key)
        except PermissionError:
            # TODO(stdlib-compat, owner:stdlib, milestone:SL3): decide whether
            # temp dir probing should require env.read or allow a fallback.
            return "/tmp"
        if val:
            trimmed = val.rstrip("/\\")
            return trimmed or val
    if _os.name == "nt":
        # TODO(stdlib-compat, owner:stdlib, milestone:SL3): align Windows defaults
        # with CPython (USERPROFILE, HOMEPATH, and temp folder probing).
        return "C:\\TEMP"
    return "/tmp"


def gettempdir() -> str:
    global _TEMP_DIR
    if _TEMP_DIR is None:
        _TEMP_DIR = _pick_tempdir()
    return _TEMP_DIR


def gettempdirb() -> bytes:
    return gettempdir().encode("utf-8")
