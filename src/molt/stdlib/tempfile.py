"""Intrinsic-backed tempfile for Molt -- all operations delegated to Rust."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import os as _os

_MOLT_TEMPFILE_GETTEMPDIR = _require_intrinsic("molt_tempfile_gettempdir")
_MOLT_TEMPFILE_GETTEMPDIRB = _require_intrinsic("molt_tempfile_gettempdirb")
_MOLT_TEMPFILE_MKDTEMP = _require_intrinsic("molt_tempfile_mkdtemp")
_MOLT_TEMPFILE_MKSTEMP = _require_intrinsic("molt_tempfile_mkstemp")
_MOLT_TEMPFILE_NAMED = _require_intrinsic("molt_tempfile_named")
_MOLT_TEMPFILE_TEMPDIR = _require_intrinsic("molt_tempfile_tempdir")
_MOLT_TEMPFILE_CLEANUP = _require_intrinsic("molt_tempfile_cleanup")

__all__ = [
    "NamedTemporaryFile",
    "TemporaryDirectory",
    "gettempdir",
    "gettempdirb",
    "mkdtemp",
    "mkstemp",
]


def gettempdir() -> str:
    """Return the name of the directory used for temporary files."""
    return str(_MOLT_TEMPFILE_GETTEMPDIR())


def gettempdirb() -> bytes:
    """Return the name of the directory used for temporary files, as bytes."""
    return _MOLT_TEMPFILE_GETTEMPDIRB()


def mkdtemp(
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
) -> str:
    """Create and return a temporary directory (secure, via Rust intrinsic)."""
    return str(_MOLT_TEMPFILE_MKDTEMP(suffix, prefix, dir))


def mkstemp(
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
    text: bool = False,
) -> tuple[int, str]:
    """Create a temporary file (secure, via Rust intrinsic) and return (fd, name)."""
    result = _MOLT_TEMPFILE_MKSTEMP(suffix, prefix, dir)
    return (int(result[0]), str(result[1]))


class _NamedTemporaryFile:
    """Wrapper for a named temporary file backed by Rust intrinsic creation."""

    __slots__ = ("_handle", "name", "delete", "_closed")

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
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | None = None,
    delete: bool = True,
):
    """Create a named temporary file (secure, via Rust intrinsic).

    The file is created securely by the Rust tempfile crate, then opened
    with the requested Python mode.
    """
    result = _MOLT_TEMPFILE_NAMED(suffix, prefix, dir, delete)
    fd = int(result[0])
    path = str(result[1])
    should_delete = bool(result[2])

    # Close the raw fd and re-open with the requested mode
    _os.close(fd)
    handle = open(path, mode, buffering=buffering, encoding=encoding, newline=newline)
    return _NamedTemporaryFile(handle, path, should_delete)


class TemporaryDirectory:
    """Create a temporary directory (secure, via Rust intrinsic).

    Cleanup is handled by the Rust intrinsic on __exit__.
    """

    __slots__ = ("name", "_closed")

    def __init__(
        self,
        suffix: str | None = None,
        prefix: str | None = None,
        dir: str | None = None,
    ) -> None:
        self.name = str(_MOLT_TEMPFILE_TEMPDIR(suffix, prefix, dir))
        self._closed = False

    def cleanup(self) -> None:
        """Remove the temporary directory and all its contents."""
        if self._closed:
            return
        _MOLT_TEMPFILE_CLEANUP(self.name)
        self._closed = True

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, exc_type, exc, tb) -> None:
        self.cleanup()


globals().pop("_require_intrinsic", None)
