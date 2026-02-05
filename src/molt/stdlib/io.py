"""Capability-gated file I/O stubs for Molt."""

from __future__ import annotations

import os
from typing import IO, Any, TYPE_CHECKING

from _intrinsics import require_intrinsic as _require_intrinsic


if TYPE_CHECKING:
    from molt import net as _net

    Stream = _net.Stream
else:
    Stream = Any


_CAP_REQUIRE = None
_MOLT_FILE_OPEN_EX = None
_MOLT_FILE_READ = None
_MOLT_FILE_CLOSE = None
_MOLT_IO_CLASS = None


def _ensure_caps() -> None:
    global _CAP_REQUIRE
    if _CAP_REQUIRE is not None:
        return
    _CAP_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())


def _ensure_io_intrinsics() -> None:
    global _MOLT_FILE_OPEN_EX, _MOLT_FILE_READ, _MOLT_FILE_CLOSE
    if _MOLT_FILE_OPEN_EX is None:
        _MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex", globals())
    if _MOLT_FILE_READ is None:
        _MOLT_FILE_READ = _require_intrinsic("molt_file_read", globals())
    if _MOLT_FILE_CLOSE is None:
        _MOLT_FILE_CLOSE = _require_intrinsic("molt_file_close", globals())


def _ensure_io_class() -> None:
    global _MOLT_IO_CLASS
    if _MOLT_IO_CLASS is not None:
        return
    _MOLT_IO_CLASS = _require_intrinsic("molt_io_class", globals())


def _io_class(name: str):
    _ensure_io_class()
    if _MOLT_IO_CLASS is None:
        raise RuntimeError("io intrinsics unavailable")
    return _MOLT_IO_CLASS(name)


class UnsupportedOperation(OSError, ValueError):
    pass


class _StreamIter:
    def __init__(self, handle, chunk_size: int) -> None:
        self._handle = handle
        self._chunk_size = chunk_size
        self._done = False

    def __iter__(self) -> _StreamIter:
        return self

    def __next__(self) -> bytes | str:
        if self._done:
            raise StopIteration
        if _MOLT_FILE_READ is None or _MOLT_FILE_CLOSE is None:
            raise RuntimeError("io intrinsics unavailable")
        chunk = _MOLT_FILE_READ(self._handle, self._chunk_size)
        if not chunk:
            self._done = True
            _MOLT_FILE_CLOSE(self._handle)
            raise StopIteration
        return chunk


DEFAULT_BUFFER_SIZE = 8192

IOBase = _io_class("IOBase")
RawIOBase = _io_class("RawIOBase")
BufferedIOBase = _io_class("BufferedIOBase")
TextIOBase = _io_class("TextIOBase")
FileIO = _io_class("FileIO")
BufferedReader = _io_class("BufferedReader")
BufferedWriter = _io_class("BufferedWriter")
BufferedRandom = _io_class("BufferedRandom")
TextIOWrapper = _io_class("TextIOWrapper")
BytesIO = _io_class("BytesIO")
StringIO = _io_class("StringIO")

__all__ = [
    "DEFAULT_BUFFER_SIZE",
    "IOBase",
    "RawIOBase",
    "BufferedIOBase",
    "TextIOBase",
    "FileIO",
    "BufferedReader",
    "BufferedWriter",
    "BufferedRandom",
    "TextIOWrapper",
    "BytesIO",
    "StringIO",
    "UnsupportedOperation",
    "open",
    "stream",
]


def _require_caps_for_mode(mode: str) -> None:
    _ensure_caps()
    if _CAP_REQUIRE is None:
        return None
    needs_read = "r" in mode or "+" in mode
    needs_write = "w" in mode or "a" in mode or "x" in mode or "+" in mode
    if needs_read:
        _CAP_REQUIRE("fs.read")
    if needs_write:
        _CAP_REQUIRE("fs.write")


def open(
    file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
    mode: str = "r",
    buffering: int = -1,
    encoding: str | None = None,
    errors: str | None = None,
    newline: str | None = None,
    closefd: bool = True,
    opener: Any | None = None,
) -> IO[Any]:
    _require_caps_for_mode(mode)
    _ensure_io_intrinsics()
    if _MOLT_FILE_OPEN_EX is None:
        raise RuntimeError("io intrinsics unavailable")
    return _MOLT_FILE_OPEN_EX(
        file,
        mode,
        buffering,
        encoding,
        errors,
        newline,
        closefd,
        opener,
    )


def stream(
    file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
    mode: str = "rb",
    chunk_size: int = 65536,
    **kwargs: Any,
) -> Stream:
    _require_caps_for_mode(mode)
    handle = open(file, mode, **kwargs)

    from molt import net as _net

    return _net.Stream(_StreamIter(handle, chunk_size))
