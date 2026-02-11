"""Minimal shelve support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import builtins as _builtins
import os
import pickle
from typing import Any, Iterator

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial):
# lower shelve persistence + dbm backends into Rust intrinsics and match CPython
# backend selection semantics.

__all__ = ["Shelf", "DbfilenameShelf", "open"]


class Shelf:
    def __init__(
        self,
        filename: str,
        flag: str = "c",
        protocol: int | None = None,
        writeback: bool = False,
    ) -> None:
        if flag not in {"r", "w", "c", "n"}:
            raise ValueError(f"invalid flag: {flag!r}")
        self._filename = filename
        self._flag = flag
        self._protocol = protocol
        self._writeback = writeback
        self._closed = False
        self._dirty = False
        self._readonly = flag == "r"
        self._data: dict[str, Any] = {}
        self._load()

    def _ensure_open(self) -> None:
        if self._closed:
            raise ValueError("invalid operation on closed shelf")

    def _load(self) -> None:
        if self._flag == "n":
            self._data = {}
            self._dirty = True
            return

        if not os.path.exists(self._filename):
            if self._flag in {"r", "w"}:
                raise FileNotFoundError(self._filename)
            self._data = {}
            self._dirty = False
            return

        with _builtins.open(self._filename, "rb") as fh:
            raw = fh.read()
        if not raw:
            self._data = {}
            return
        loaded = pickle.loads(raw)
        if not isinstance(loaded, dict):
            raise ValueError("shelve store corrupted: expected dict payload")
        self._data = loaded

    def sync(self) -> None:
        self._ensure_open()
        if self._readonly or not self._dirty:
            return
        payload = pickle.dumps(self._data, protocol=self._protocol)
        with _builtins.open(self._filename, "wb") as fh:
            fh.write(payload)
        self._dirty = False

    def close(self) -> None:
        if self._closed:
            return
        try:
            self.sync()
        finally:
            self._closed = True

    def __enter__(self) -> "Shelf":
        self._ensure_open()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __getitem__(self, key: str) -> Any:
        self._ensure_open()
        return self._data[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self._ensure_open()
        if self._readonly:
            raise OSError("shelf is read-only")
        self._data[key] = value
        self._dirty = True

    def __delitem__(self, key: str) -> None:
        self._ensure_open()
        if self._readonly:
            raise OSError("shelf is read-only")
        del self._data[key]
        self._dirty = True

    def __iter__(self) -> Iterator[str]:
        self._ensure_open()
        return iter(self._data)

    def __len__(self) -> int:
        self._ensure_open()
        return len(self._data)


DbfilenameShelf = Shelf


def open(
    filename: str,
    flag: str = "c",
    protocol: int | None = None,
    writeback: bool = False,
) -> Shelf:
    return Shelf(filename, flag=flag, protocol=protocol, writeback=writeback)
