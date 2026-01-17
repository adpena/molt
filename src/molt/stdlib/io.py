"""Capability-gated file I/O stubs for Molt."""

from __future__ import annotations

import os
from typing import IO, Any

from molt import capabilities
from molt import net


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
