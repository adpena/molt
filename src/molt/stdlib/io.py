"""Capability-gated file I/O stubs for Molt."""

from __future__ import annotations

import builtins
import os
from collections.abc import Iterator
from typing import IO, Any

from molt import capabilities
from molt import net


def _require_caps_for_mode(mode: str) -> None:
    needs_read = "r" in mode or "+" in mode
    needs_write = any(flag in mode for flag in ("w", "a", "x", "+"))
    if needs_read:
        capabilities.require("fs.read")
    if needs_write:
        capabilities.require("fs.write")


def open(
    file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
    mode: str = "r",
    *args: Any,
    **kwargs: Any,
) -> IO[Any]:
    _require_caps_for_mode(mode)
    return builtins.open(file, mode, *args, **kwargs)


def stream(
    file: str | bytes | int | os.PathLike[str] | os.PathLike[bytes],
    *,
    mode: str = "rb",
    chunk_size: int = 65536,
) -> net.Stream:
    _require_caps_for_mode(mode)
    if "b" not in mode and "t" not in mode:
        mode = f"{mode}b"
    handle = builtins.open(file, mode)

    def _iter() -> Iterator[bytes | str]:
        with handle:
            while True:
                chunk = handle.read(chunk_size)
                if not chunk:
                    break
                yield chunk

    return net.Stream(_iter())
