"""Capability-gated file I/O stubs for Molt."""

from __future__ import annotations

import os
from typing import IO, Any

from molt import capabilities
from molt import net


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
        chunk = self._handle.read(self._chunk_size)
        if not chunk:
            self._done = True
            self._handle.close()
            raise StopIteration
        return chunk


class StringIO:
    def __init__(self, initial_value: str = "") -> None:
        if not isinstance(initial_value, str):
            name = type(initial_value).__name__
            raise TypeError(f"initial_value must be str, not {name}")
        self._value = initial_value
        self._pos = 0
        self.closed = False

    def write(self, s: str) -> int:
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        if not isinstance(s, str):
            name = type(s).__name__
            raise TypeError(f"string argument expected, got {name}")
        if self._pos > len(self._value):
            self._value += "\x00" * (self._pos - len(self._value))
        if self._pos == len(self._value):
            self._value += s
        else:
            prefix = self._value[: self._pos]
            suffix_start = self._pos + len(s)
            suffix = (
                self._value[suffix_start:] if suffix_start < len(self._value) else ""
            )
            self._value = prefix + s + suffix
        self._pos += len(s)
        return len(s)

    def read(self, n: int = -1) -> str:
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        if n is None or n < 0:
            out = self._value[self._pos :]
            self._pos = len(self._value)
            return out
        end = min(len(self._value), self._pos + n)
        out = self._value[self._pos : end]
        self._pos = end
        return out

    def getvalue(self) -> str:
        return self._value

    def seek(self, pos: int, whence: int = 0) -> int:
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        if whence == 0:
            new_pos = pos
        elif whence == 1:
            new_pos = self._pos + pos
        elif whence == 2:
            new_pos = len(self._value) + pos
        else:
            raise ValueError("invalid whence")
        if new_pos < 0:
            raise ValueError("negative seek position")
        self._pos = new_pos
        return self._pos

    def tell(self) -> int:
        return self._pos

    def close(self) -> None:
        self.closed = True


def _require_caps_for_mode(mode: str) -> None:
    needs_read = "r" in mode or "+" in mode
    needs_write = "w" in mode or "a" in mode or "x" in mode or "+" in mode
    if needs_read:
        capabilities.require("fs.read")
    if needs_write:
        capabilities.require("fs.write")


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
    import builtins as _builtins

    return _builtins.open(
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
) -> net.Stream:
    _require_caps_for_mode(mode)
    import builtins as _builtins

    handle = _builtins.open(file, mode, **kwargs)

    return net.Stream(_StreamIter(handle, chunk_size))
