"""Minimal tempfile shim for Molt."""

from __future__ import annotations

from molt.stdlib import os as _os

__all__ = ["TemporaryDirectory", "gettempdir", "gettempdirb", "mkdtemp"]

_TEMP_DIR: str | None = None
_TEMP_COUNTER = 0


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


def mkdtemp(suffix: str = "", prefix: str = "tmp", dir: str | None = None) -> str:
    global _TEMP_COUNTER
    base = dir or gettempdir()
    for _ in range(10000):
        name = f"{prefix}{_TEMP_COUNTER}"
        _TEMP_COUNTER += 1
        candidate = _os.path.join(base, f"{name}{suffix}")
        try:
            _os.makedirs(candidate)
            return candidate
        except FileExistsError:
            continue
    raise FileExistsError("No usable temporary directory name")


def _rmtree(path: str) -> None:
    try:
        entries = _os.listdir(path)
    except Exception:
        entries = []
    for name in entries:
        entry = _os.path.join(path, name)
        try:
            if _os.path.isdir(entry):
                _rmtree(entry)
                _os.rmdir(entry)
            else:
                _os.unlink(entry)
        except Exception:
            pass
    try:
        _os.rmdir(path)
    except Exception:
        pass


class TemporaryDirectory:
    def __init__(
        self, suffix: str = "", prefix: str = "tmp", dir: str | None = None
    ) -> None:
        self.name = mkdtemp(suffix=suffix, prefix=prefix, dir=dir)
        self._closed = False

    def cleanup(self) -> None:
        if self._closed:
            return
        _rmtree(self.name)
        self._closed = True

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, exc_type, exc, tb) -> None:
        self.cleanup()
