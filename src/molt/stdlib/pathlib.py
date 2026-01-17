"""Capability-gated pathlib stubs for Molt."""

from __future__ import annotations

from collections.abc import Iterator

from molt import capabilities
from molt.stdlib import io as _io
from molt.stdlib import os as _os


class Path:
    def __init__(self, path: str | Path | None = None) -> None:
        if path is None:
            self._path = "."
        elif isinstance(path, Path):
            self._path = path._path
        elif isinstance(path, str):
            self._path = path
        else:
            name = type(path).__name__
            raise TypeError(f"expected str, bytes or os.PathLike object, not {name}")
        # TODO(stdlib-compat, pathlib-pathlike): support os.PathLike inputs via
        # __fspath__ without triggering backend bind regressions.

    def __fspath__(self) -> str:
        return self._path

    def __str__(self) -> str:
        return self._path

    def __repr__(self) -> str:
        return f"Path({self._path!r})"

    def _wrap(self, path: str) -> Path:
        return Path(path)

    def joinpath(self, *others: str) -> Path:
        path = self._path
        for part in others:
            if part.startswith(_os.sep):
                path = part
            else:
                if path and not path.endswith(_os.sep):
                    path += _os.sep
                path += part
        return self._wrap(path)

    def __truediv__(self, key: str) -> Path:
        path = self._path
        if key.startswith(_os.sep):
            path = key
        else:
            if path and not path.endswith(_os.sep):
                path += _os.sep
            path += key
        return self._wrap(path)

    def open(
        self,
        mode: str = "r",
        buffering: int = -1,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
        closefd: bool = True,
        opener: object | None = None,
    ):
        return _io.open(
            self._path,
            mode,
            buffering,
            encoding,
            errors,
            newline,
            closefd,
            opener,
        )

    def read_text(self, encoding: str | None = None, errors: str | None = None) -> str:
        capabilities.require("fs.read")
        with _io.open(self._path, "r", encoding=encoding, errors=errors) as handle:
            return handle.read()

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        with _io.open(self._path, "rb") as handle:
            return handle.read()

    def write_text(
        self,
        data: str,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> int:
        capabilities.require("fs.write")
        with _io.open(
            self._path,
            "w",
            encoding=encoding,
            errors=errors,
            newline=newline,
        ) as handle:
            return handle.write(data)

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        with _io.open(self._path, "wb") as handle:
            return handle.write(data)

    def exists(self) -> bool:
        return _os.path.exists(self._path)

    def unlink(self) -> None:
        _os.path.unlink(self._path)

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        # TODO(stdlib-compat, pathlib-iterdir): implement listdir-backed iterdir
        # in the stdlib shim so compiled Molt code can iterate directories.
        raise NotImplementedError("pathlib.Path.iterdir is not supported yet")

    @property
    def name(self) -> str:
        return _os.path.basename(self._path)

    @property
    def suffix(self) -> str:
        return _os.path.splitext(self._path)[1]

    @property
    def parent(self) -> Path:
        parent = _os.path.dirname(self._path) or "."
        return self._wrap(parent)
