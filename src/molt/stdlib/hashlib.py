"""Hashlib shim for Molt (pure-Python SHA1)."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement md5/sha2/sha3/blake2 + hmac parity with deterministic policy.

from __future__ import annotations

from typing import Any

__all__ = [
    "sha1",
    "new",
    "algorithms_available",
    "algorithms_guaranteed",
]


def _to_bytes(data: Any) -> bytes:
    if isinstance(data, bytes):
        return data
    if isinstance(data, (bytearray, memoryview)):
        return bytes(data)
    if isinstance(data, str):
        return data.encode("utf-8")
    raise TypeError("data must be bytes-like or str")


def _left_rotate(value: int, shift: int) -> int:
    return ((value << shift) | (value >> (32 - shift))) & 0xFFFFFFFF


class _Sha1:
    __slots__ = ("_h", "_unprocessed", "_message_byte_length")

    digest_size = 20
    block_size = 64
    name = "sha1"

    def __init__(self, data: Any | None = None) -> None:
        self._h = [
            0x67452301,
            0xEFCDAB89,
            0x98BADCFE,
            0x10325476,
            0xC3D2E1F0,
        ]
        self._unprocessed = b""
        self._message_byte_length = 0
        if data is not None:
            self.update(data)

    def copy(self) -> "_Sha1":
        new = _Sha1()
        new._h = list(self._h)
        new._unprocessed = self._unprocessed
        new._message_byte_length = self._message_byte_length
        return new

    def update(self, data: Any) -> None:
        chunk = _to_bytes(data)
        if not chunk:
            return None
        self._message_byte_length += len(chunk)
        chunk = self._unprocessed + chunk
        block_size = self.block_size
        for idx in range(0, len(chunk) - block_size + 1, block_size):
            self._process_chunk(chunk[idx : idx + block_size])
        self._unprocessed = chunk[(len(chunk) // block_size) * block_size :]
        return None

    def _process_chunk(self, chunk: bytes) -> None:
        w = [0] * 80
        for i in range(16):
            w[i] = int.from_bytes(chunk[i * 4 : i * 4 + 4], "big")
        for i in range(16, 80):
            w[i] = _left_rotate(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1)

        a, b, c, d, e = self._h
        for i in range(80):
            if i <= 19:
                f = (b & c) | (~b & d)
                k = 0x5A827999
            elif i <= 39:
                f = b ^ c ^ d
                k = 0x6ED9EBA1
            elif i <= 59:
                f = (b & c) | (b & d) | (c & d)
                k = 0x8F1BBCDC
            else:
                f = b ^ c ^ d
                k = 0xCA62C1D6
            temp = (_left_rotate(a, 5) + f + e + k + w[i]) & 0xFFFFFFFF
            e = d
            d = c
            c = _left_rotate(b, 30)
            b = a
            a = temp

        self._h[0] = (self._h[0] + a) & 0xFFFFFFFF
        self._h[1] = (self._h[1] + b) & 0xFFFFFFFF
        self._h[2] = (self._h[2] + c) & 0xFFFFFFFF
        self._h[3] = (self._h[3] + d) & 0xFFFFFFFF
        self._h[4] = (self._h[4] + e) & 0xFFFFFFFF

    def _produce_digest(self) -> bytes:
        message_len_bits = self._message_byte_length * 8
        chunk = self._unprocessed + b"\x80"
        pad_len = (56 - (len(chunk) % 64)) % 64
        chunk += b"\x00" * pad_len
        chunk += message_len_bits.to_bytes(8, "big")
        for idx in range(0, len(chunk), 64):
            self._process_chunk(chunk[idx : idx + 64])
        return b"".join(h.to_bytes(4, "big") for h in self._h)

    def digest(self) -> bytes:
        return self.copy()._produce_digest()

    def hexdigest(self) -> str:
        return self.digest().hex()


algorithms_available = {"sha1"}
algorithms_guaranteed = {"sha1"}


def sha1(data: Any = b"") -> _Sha1:
    return _Sha1(data)


def new(name: str, data: Any = b"") -> _Sha1:
    if name.lower() != "sha1":
        raise ValueError("unsupported hash type")
    return _Sha1(data)
