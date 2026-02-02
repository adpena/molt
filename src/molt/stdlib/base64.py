"""Base64 encoding/decoding for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement b32/b16/a85/b85 and full base64 API parity.

from __future__ import annotations

from typing import Any

__all__ = [
    "b64encode",
    "b64decode",
    "standard_b64encode",
    "standard_b64decode",
    "urlsafe_b64encode",
    "urlsafe_b64decode",
    "encodebytes",
    "decodebytes",
    "encodestring",
    "decodestring",
]

_B64_ALPHABET = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
_B64_URLSAFE = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"


def _to_bytes(data: Any, name: str) -> bytes:
    if isinstance(data, bytes):
        return data
    if isinstance(data, (bytearray, memoryview)):
        return bytes(data)
    if isinstance(data, str):
        return data.encode("ascii")
    raise TypeError(f"{name} must be bytes-like or str")


def _b64_encode_bytes(data: bytes, alphabet: bytes) -> bytes:
    if not data:
        return b""
    out = bytearray()
    for idx in range(0, len(data), 3):
        chunk = data[idx : idx + 3]
        pad = 3 - len(chunk)
        val = int.from_bytes(chunk, "big") << (pad * 8)
        out.append(alphabet[(val >> 18) & 0x3F])
        out.append(alphabet[(val >> 12) & 0x3F])
        out.append(alphabet[(val >> 6) & 0x3F])
        out.append(alphabet[val & 0x3F])
        if pad:
            out[-pad:] = b"=" * pad
    return bytes(out)


def _b64_decode_bytes(data: bytes, *, validate: bool) -> bytes:
    if not data:
        return b""
    rev = {val: idx for idx, val in enumerate(_B64_ALPHABET)}
    if not validate:
        filtered = bytearray()
        for ch in data:
            if ch in rev or ch == ord("="):
                filtered.append(ch)
        data = bytes(filtered)
        data = b"".join(data.split())
    else:
        for ch in data:
            if ch in (ord("\n"), ord("\r"), ord("\t"), ord(" ")):
                raise ValueError("invalid base64 input")
            if ch not in rev and ch != ord("="):
                raise ValueError("invalid base64 input")
    if len(data) % 4:
        if validate:
            raise ValueError("incorrect padding")
        data += b"=" * ((4 - len(data) % 4) % 4)
    out = bytearray()
    for idx in range(0, len(data), 4):
        chunk = data[idx : idx + 4]
        if len(chunk) < 4:
            break
        pad = chunk.count(b"=")
        if pad and chunk[-pad:] != b"=" * pad:
            if validate:
                raise ValueError("invalid padding")
            pad = 0
        val = 0
        for ch in chunk:
            if ch == ord("="):
                val <<= 6
            else:
                val = (val << 6) | rev.get(ch, 0)
        raw = val.to_bytes(3, "big")
        if pad:
            out.extend(raw[:-pad])
        else:
            out.extend(raw)
    return bytes(out)


def b64encode(s: Any, altchars: Any | None = None) -> bytes:
    data = _to_bytes(s, "data")
    alphabet = _B64_ALPHABET
    if altchars is not None:
        alt = _to_bytes(altchars, "altchars")
        if len(alt) != 2:
            raise TypeError("altchars must be a length-2 bytes-like object")
        trans = bytes.maketrans(b"+/", alt)
        alphabet = alphabet.translate(trans)
    return _b64_encode_bytes(data, alphabet)


def b64decode(s: Any, altchars: Any | None = None, validate: bool = False) -> bytes:
    data = _to_bytes(s, "data")
    if altchars is not None:
        alt = _to_bytes(altchars, "altchars")
        if len(alt) != 2:
            raise TypeError("altchars must be a length-2 bytes-like object")
        data = data.translate(bytes.maketrans(alt, b"+/"))
    return _b64_decode_bytes(data, validate=validate)


def standard_b64encode(s: Any) -> bytes:
    return b64encode(s)


def standard_b64decode(s: Any) -> bytes:
    return b64decode(s)


def urlsafe_b64encode(s: Any) -> bytes:
    return _b64_encode_bytes(_to_bytes(s, "data"), _B64_URLSAFE)


def urlsafe_b64decode(s: Any) -> bytes:
    data = _to_bytes(s, "data")
    data = data.translate(bytes.maketrans(b"-_", b"+/"))
    return _b64_decode_bytes(data, validate=False)


def encodebytes(s: Any) -> bytes:
    raw = b64encode(s)
    if not raw:
        return b""
    lines = [raw[idx : idx + 76] for idx in range(0, len(raw), 76)]
    return b"\n".join(lines) + b"\n"


def decodebytes(s: Any) -> bytes:
    return b64decode(s)


def encodestring(s: Any) -> bytes:
    return encodebytes(s)


def decodestring(s: Any) -> bytes:
    return decodebytes(s)
