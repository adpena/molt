"""Intrinsic-backed compatibility surface for CPython's `_io`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from io import (
    BufferedIOBase,
    BufferedRandom,
    BufferedReader,
    BufferedWriter,
    BytesIO,
    DEFAULT_BUFFER_SIZE,
    FileIO,
    IOBase,
    RawIOBase,
    SEEK_CUR,
    SEEK_END,
    SEEK_SET,
    StringIO,
    TextIOBase,
    TextIOWrapper,
    UnsupportedOperation,
    open,
)

_MOLT_IO_CLASS = _require_intrinsic("molt_io_class")

__all__ = [
    "BufferedIOBase",
    "BufferedRandom",
    "BufferedReader",
    "BufferedWriter",
    "BytesIO",
    "DEFAULT_BUFFER_SIZE",
    "FileIO",
    "IOBase",
    "RawIOBase",
    "SEEK_CUR",
    "SEEK_END",
    "SEEK_SET",
    "StringIO",
    "TextIOBase",
    "TextIOWrapper",
    "UnsupportedOperation",
    "open",
]
