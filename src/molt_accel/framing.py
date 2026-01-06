from __future__ import annotations

import select
import struct
import time
from typing import IO


class FrameTooLargeError(ValueError):
    pass


def _wait_for_read(stream: IO[bytes], timeout: float | None) -> None:
    if timeout is None:
        return
    peek = getattr(stream, "peek", None)
    if callable(peek):
        try:
            if peek(0):
                return
        except Exception:
            pass
    fd = stream.fileno()
    ready, _, _ = select.select([fd], [], [], timeout)
    if not ready:
        raise TimeoutError("Timed out waiting for IPC frame")


def _read_exact(stream: IO[bytes], size: int, timeout: float | None) -> bytes:
    buf = bytearray()
    deadline = None if timeout is None else time.monotonic() + timeout
    while len(buf) < size:
        remaining = None
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Timed out waiting for IPC frame")
        _wait_for_read(stream, remaining)
        chunk = stream.read(size - len(buf))
        if not chunk:
            raise EOFError("IPC stream closed")
        buf.extend(chunk)
    return bytes(buf)


def read_frame(
    stream: IO[bytes],
    *,
    timeout: float | None = None,
    max_size: int = 64 * 1024 * 1024,
) -> bytes:
    header = _read_exact(stream, 4, timeout)
    (size,) = struct.unpack("<I", header)
    if size > max_size:
        raise FrameTooLargeError(f"Frame size {size} exceeds max {max_size}")
    return _read_exact(stream, size, timeout)


def write_frame(
    stream: IO[bytes],
    payload: bytes,
    *,
    max_size: int = 64 * 1024 * 1024,
) -> None:
    size = len(payload)
    if size > max_size:
        raise FrameTooLargeError(f"Frame size {size} exceeds max {max_size}")
    stream.write(struct.pack("<I", size))
    stream.write(payload)
    stream.flush()
