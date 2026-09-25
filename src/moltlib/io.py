"""Bounded file streams built on the public ``io`` protocol.

Compiled programs use Molt's capability-gated ``io.open``; this adapter has no
separate filesystem policy or dependency on networking or the compiler package.
Reads are synchronous, including during asynchronous iteration.
"""

from __future__ import annotations

import io
import operator
import os
from types import TracebackType
from typing import Any, IO

__all__ = ["FileStream", "stream"]


class FileStream:
    """A single-pass chunk iterator owning one file until EOF, error, or close.

    Use a synchronous or asynchronous context manager when iteration may stop
    early. Async iteration supplies backpressure, not nonblocking file reads.
    """

    __slots__ = ("_handle", "_chunk_size")

    def __init__(
        self,
        file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
        mode: str = "rb",
        chunk_size: int = 65536,
        **kwargs: Any,
    ) -> None:
        self._handle: IO[Any] | None = None
        self._chunk_size = operator.index(chunk_size)
        if self._chunk_size <= 0:
            raise ValueError("chunk_size must be positive")
        self._handle = io.open(file, mode, **kwargs)

    @property
    def closed(self) -> bool:
        return self._handle is None

    def close(self) -> None:
        handle = self._handle
        self._handle = None
        if handle is not None:
            handle.close()

    async def aclose(self) -> None:
        self.close()

    def __iter__(self) -> FileStream:
        return self

    def __next__(self) -> bytes | str:
        handle = self._handle
        if handle is None:
            raise StopIteration
        try:
            chunk = handle.read(self._chunk_size)
        except BaseException:
            self.close()
            raise
        if not chunk:
            self.close()
            raise StopIteration
        return chunk

    def __aiter__(self) -> FileStream:
        return self

    async def __anext__(self) -> bytes | str:
        try:
            return next(self)
        except StopIteration:
            raise StopAsyncIteration from None

    def __enter__(self) -> FileStream:
        if self.closed:
            raise ValueError("file stream is closed")
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    async def __aenter__(self) -> FileStream:
        return self.__enter__()

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()


def stream(
    file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
    mode: str = "rb",
    chunk_size: int = 65536,
    **kwargs: Any,
) -> FileStream:
    """Open a bounded file stream; ``io.open`` owns modes and capabilities."""
    return FileStream(file, mode, chunk_size, **kwargs)
