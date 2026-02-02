"""Minimal hmac support for Molt."""

from __future__ import annotations

from typing import Any

from molt.stdlib import hashlib

__all__ = ["new", "digest", "compare_digest"]


def _to_bytes(data: Any) -> bytes:
    if isinstance(data, bytes):
        return data
    if isinstance(data, (bytearray, memoryview)):
        return bytes(data)
    if isinstance(data, str):
        return data.encode("utf-8")
    raise TypeError("data must be bytes-like or str")


def _hash_new(digestmod: Any):
    if isinstance(digestmod, str):
        return hashlib.new(digestmod)
    if callable(digestmod):
        return digestmod()
    raise TypeError("digestmod must be a name or callable")


def compare_digest(a: Any, b: Any) -> bool:
    a_bytes = _to_bytes(a)
    b_bytes = _to_bytes(b)
    if len(a_bytes) != len(b_bytes):
        return False
    res = 0
    for x, y in zip(a_bytes, b_bytes):
        res |= x ^ y
    return res == 0


class _Hmac:
    def __init__(self, key: Any, msg: Any | None, digestmod: Any) -> None:
        self._digestmod = digestmod
        key_bytes = _to_bytes(key)
        self._inner = _hash_new(digestmod)
        self._outer = _hash_new(digestmod)
        block_size = getattr(self._inner, "block_size", 64)
        if len(key_bytes) > block_size:
            h = _hash_new(digestmod)
            h.update(key_bytes)
            key_bytes = h.digest()
        if len(key_bytes) < block_size:
            key_bytes = key_bytes + b"\x00" * (block_size - len(key_bytes))
        o_key_pad = bytes((b ^ 0x5C) for b in key_bytes)
        i_key_pad = bytes((b ^ 0x36) for b in key_bytes)
        self._inner.update(i_key_pad)
        self._outer.update(o_key_pad)
        if msg is not None:
            self.update(msg)

    def update(self, msg: Any) -> None:
        self._inner.update(_to_bytes(msg))

    def copy(self) -> "_Hmac":
        other = _Hmac(b"", None, self._digestmod)
        other._inner = self._inner.copy()
        other._outer = self._outer.copy()
        return other

    def digest(self) -> bytes:
        inner = self._inner.copy().digest()
        outer = self._outer.copy()
        outer.update(inner)
        return outer.digest()

    def hexdigest(self) -> str:
        return self.digest().hex()


def new(key: Any, msg: Any | None = None, digestmod: Any | None = None) -> _Hmac:
    if digestmod is None:
        digestmod = "sha1"
    return _Hmac(key, msg, digestmod)


def digest(key: Any, msg: Any, digestmod: Any) -> bytes:
    return new(key, msg, digestmod).digest()
