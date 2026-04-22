"""Intrinsic-backed ``shelve`` module for Molt.

Uses ``dbm.dumb`` Rust intrinsics as the persistence backend with
per-key pickle serialization, matching CPython's shelve semantics.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import dbm
import pickle
from typing import Any, Iterator

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

__all__ = ["Shelf", "BsdDbShelf", "DbfilenameShelf", "open"]


class Shelf:
    """A shelf wraps a dbm database with pickle-based serialization.

    Keys must be strings. Values can be any picklable object.
    """

    def __init__(
        self,
        dict: object,
        protocol: int | None = None,
        writeback: bool = False,
        keyencoding: str = "utf-8",
    ) -> None:
        self.dict = dict
        self._protocol = protocol
        self.writeback = writeback
        self.keyencoding = keyencoding
        self.cache: dict[str, Any] = {}
        self._closed = False

    def _ensure_open(self) -> None:
        if self._closed:
            raise ValueError("invalid operation on closed shelf")

    def __contains__(self, key: str) -> bool:
        self._ensure_open()
        if self.writeback and key in self.cache:
            return True
        return key.encode(self.keyencoding) in self.dict  # type: ignore[operator]

    def __getitem__(self, key: str) -> Any:
        self._ensure_open()
        if self.writeback and key in self.cache:
            return self.cache[key]
        raw = self.dict[key.encode(self.keyencoding)]  # type: ignore[index]
        value = pickle.loads(raw)
        if self.writeback:
            self.cache[key] = value
        return value

    def __setitem__(self, key: str, value: Any) -> None:
        self._ensure_open()
        if self.writeback:
            self.cache[key] = value
        raw = pickle.dumps(value, self._protocol)
        self.dict[key.encode(self.keyencoding)] = raw  # type: ignore[index]

    def __delitem__(self, key: str) -> None:
        self._ensure_open()
        del self.dict[key.encode(self.keyencoding)]  # type: ignore[arg-type]
        if self.writeback:
            self.cache.pop(key, None)

    def __iter__(self) -> Iterator[str]:
        self._ensure_open()
        for k in self.keys():
            yield k

    def __len__(self) -> int:
        self._ensure_open()
        return len(list(self.keys()))

    def keys(self) -> list[str]:
        self._ensure_open()
        raw_keys = self.dict.keys()  # type: ignore[union-attr]
        result: list[str] = []
        for k in raw_keys:
            if isinstance(k, bytes):
                result.append(k.decode(self.keyencoding))
            else:
                result.append(str(k))
        return result

    def values(self) -> list[Any]:
        self._ensure_open()
        return [self[k] for k in self.keys()]

    def items(self) -> list[tuple[str, Any]]:
        self._ensure_open()
        return [(k, self[k]) for k in self.keys()]

    def get(self, key: str, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: str, *args: Any) -> Any:
        try:
            val = self[key]
            del self[key]
            return val
        except KeyError:
            if args:
                return args[0]
            raise

    def update(self, other: Any = (), **kwargs: Any) -> None:
        if hasattr(other, "items"):
            for k, v in other.items():
                self[k] = v
        elif hasattr(other, "keys"):
            for k in other.keys():
                self[k] = other[k]
        else:
            for k, v in other:
                self[k] = v
        for k, v in kwargs.items():
            self[k] = v

    def setdefault(self, key: str, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            self[key] = default
            return default

    def sync(self) -> None:
        self._ensure_open()
        if self.writeback and self.cache:
            self._writeback()
        if hasattr(self.dict, "sync"):
            self.dict.sync()  # type: ignore[union-attr]

    def _writeback(self) -> None:
        for key, value in self.cache.items():
            raw = pickle.dumps(value, self._protocol)
            self.dict[key.encode(self.keyencoding)] = raw  # type: ignore[index]

    def close(self) -> None:
        if self._closed:
            return
        try:
            self.sync()
        finally:
            self._closed = True
            if hasattr(self.dict, "close"):
                self.dict.close()  # type: ignore[union-attr]
            self.cache.clear()

    def __enter__(self) -> "Shelf":
        self._ensure_open()
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def __del__(self) -> None:
        if not self._closed:
            self.close()


class BsdDbShelf(Shelf):
    """Shelf using a BSD db-style dict (same interface as Shelf in Molt)."""

    pass


class DbfilenameShelf(Shelf):
    """Shelf that opens a dbm database by filename."""

    def __init__(
        self,
        filename: str,
        flag: str = "c",
        protocol: int | None = None,
        writeback: bool = False,
    ) -> None:
        d = dbm.open(filename, flag)
        super().__init__(d, protocol=protocol, writeback=writeback)


def open(
    filename: str,
    flag: str = "c",
    protocol: int | None = None,
    writeback: bool = False,
) -> Shelf:
    """Open a persistent dictionary backed by a dbm database."""
    return DbfilenameShelf(filename, flag=flag, protocol=protocol, writeback=writeback)


globals().pop("_require_intrinsic", None)
