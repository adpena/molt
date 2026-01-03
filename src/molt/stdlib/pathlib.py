"""Capability-gated pathlib stubs for Molt."""

from __future__ import annotations

import pathlib as _pathlib
from collections.abc import Iterator
from typing import Any

from molt import capabilities
from molt.stdlib import io as _io


class Path:
    def __init__(self, *args: str | _pathlib.Path) -> None:
        self._path = _pathlib.Path(*args)

    def __fspath__(self) -> str:
        return self._path.__fspath__()

    def __str__(self) -> str:
        return str(self._path)

    def __repr__(self) -> str:
        return f"Path({self._path!r})"

    def _wrap(self, path: _pathlib.Path) -> Path:
        return Path(path)

    def joinpath(self, *other: str) -> Path:
        return self._wrap(self._path.joinpath(*other))

    def __truediv__(self, key: str) -> Path:
        return self.joinpath(key)

    def open(self, mode: str = "r", *args: Any, **kwargs: Any):
        return _io.open(self._path, mode, *args, **kwargs)

    def read_text(self, *args: Any, **kwargs: Any) -> str:
        capabilities.require("fs.read")
        return self._path.read_text(*args, **kwargs)

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        return self._path.read_bytes()

    def write_text(self, data: str, *args: Any, **kwargs: Any) -> int:
        capabilities.require("fs.write")
        return self._path.write_text(data, *args, **kwargs)

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        return self._path.write_bytes(data)

    def exists(self) -> bool:
        capabilities.require("fs.read")
        return self._path.exists()

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        for child in self._path.iterdir():
            yield self._wrap(child)

    @property
    def name(self) -> str:
        return self._path.name

    @property
    def suffix(self) -> str:
        return self._path.suffix

    @property
    def parent(self) -> Path:
        return self._wrap(self._path.parent)
