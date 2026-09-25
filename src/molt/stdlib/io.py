"""Capability-gated CPython file I/O surface for Molt."""

from __future__ import annotations

import os
from typing import IO, Any

from _intrinsics import require_intrinsic as _require_intrinsic


_CAP_REQUIRE = None
_MOLT_FILE_OPEN_EX = None
_MOLT_IO_CLASS = None


def _ensure_caps() -> None:
    global _CAP_REQUIRE
    if _CAP_REQUIRE is not None:
        return
    _CAP_REQUIRE = _require_intrinsic("molt_capabilities_require")


def _ensure_io_intrinsics() -> None:
    global _MOLT_FILE_OPEN_EX
    if _MOLT_FILE_OPEN_EX is None:
        _MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex")


def _ensure_io_class() -> None:
    global _MOLT_IO_CLASS
    if _MOLT_IO_CLASS is not None:
        return
    _MOLT_IO_CLASS = _require_intrinsic("molt_io_class")


def _io_class(name: str):
    _ensure_io_class()
    if _MOLT_IO_CLASS is None:
        raise RuntimeError("io intrinsics unavailable")
    return _MOLT_IO_CLASS(name)


class UnsupportedOperation(OSError, ValueError):
    pass


SEEK_SET = 0
SEEK_CUR = 1
SEEK_END = 2

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
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
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
