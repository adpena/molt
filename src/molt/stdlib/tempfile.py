"""Minimal tempfile shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import os as _os

_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join", globals())

__all__ = [
    "NamedTemporaryFile",
    "TemporaryDirectory",
    "gettempdir",
    "gettempdirb",
    "mkdtemp",
]

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
        candidate = _MOLT_PATH_JOIN(base, f"{name}{suffix}")
        try:
            _os.makedirs(candidate)
            return candidate
        except FileExistsError:
            continue
    raise FileExistsError("No usable temporary directory name")


class _NamedTemporaryFile:
    def __init__(self, handle, name: str, delete: bool) -> None:
        self._handle = handle
        self.name = name
        self.delete = delete
        self._closed = False

    def __getattr__(self, name: str):
        return getattr(self._handle, name)

    def __enter__(self):
        return self

    def close(self) -> None:
        if self._closed:
            return
        try:
            self._handle.close()
        finally:
            if self.delete:
                try:
                    _os.unlink(self.name)
                except FileNotFoundError:
                    pass
            self._closed = True

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()


def NamedTemporaryFile(
    mode: str = "w+b",
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str = "",
    prefix: str = "tmp",
    dir: str | None = None,
    delete: bool = True,
):
    global _TEMP_COUNTER
    base = dir or gettempdir()
    open_mode = mode
    if "x" not in open_mode:
        if "w" in open_mode:
            open_mode = open_mode.replace("w", "x", 1)
        elif "a" in open_mode:
            open_mode = open_mode.replace("a", "x", 1)
        else:
            open_mode = "x" + open_mode
    for _ in range(10000):
        name = f"{prefix}{_TEMP_COUNTER}"
        _TEMP_COUNTER += 1
        path = _MOLT_PATH_JOIN(base, f"{name}{suffix}")
        try:
            handle = open(
                path, open_mode, buffering=buffering, encoding=encoding, newline=newline
            )
        except FileExistsError:
            continue
        return _NamedTemporaryFile(handle, path, delete)
    raise FileExistsError("No usable temporary file name")


def _rmtree(path: str) -> None:
    try:
        entries = _os.listdir(path)
    except Exception:
        entries = []
    for name in entries:
        entry = _MOLT_PATH_JOIN(path, name)
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
