"""UUID support for Molt."""

from __future__ import annotations

from typing import Any

import builtins as _builtins
import enum as _enum
import hashlib as _hashlib
import random as _random
import time as _time

Int = _builtins.int
BytesLike = _builtins.bytes | bytearray | memoryview

__all__ = [
    "UUID",
    "SafeUUID",
    "uuid1",
    "uuid3",
    "uuid4",
    "uuid5",
    "getnode",
    "NAMESPACE_DNS",
    "NAMESPACE_URL",
    "NAMESPACE_OID",
    "NAMESPACE_X500",
    "RESERVED_NCS",
    "RFC_4122",
    "RESERVED_MICROSOFT",
    "RESERVED_FUTURE",
]

RESERVED_NCS = "reserved for NCS compatibility"
RFC_4122 = "specified in RFC 4122"
RESERVED_MICROSOFT = "reserved for Microsoft compatibility"
RESERVED_FUTURE = "reserved for future definition"

_UUID_EPOCH = 0x01B21DD213814000
_NODE: int | None = None
_CLOCK_SEQ: int | None = None
_LAST_TIMESTAMP: int | None = None


class SafeUUID(_enum.Enum):
    safe = 0
    unsafe = -1
    unknown = None


class UUID:
    __slots__ = ("_int", "_bytes", "_is_safe")

    def __init__(
        self,
        hex: str | None = None,
        bytes: BytesLike | None = None,
        bytes_le: BytesLike | None = None,
        fields: tuple[Int, Int, Int, Int, Int, Int] | None = None,
        int: Int | None = None,
        version: Int | None = None,
    ) -> None:
        int_value = int
        provided = sum(
            item is not None for item in (hex, bytes, bytes_le, fields, int_value)
        )
        if provided != 1:
            raise TypeError(
                "one of the hex, bytes, bytes_le, fields, or int arguments "
                "must be given"
            )

        if hex is not None:
            value = _int_from_hex(hex)
        elif bytes is not None:
            data = _to_bytes(bytes)
            if len(data) != 16:
                raise ValueError("bytes is not a 16-char string")
            value = _int_from_bytes(data)
        elif bytes_le is not None:
            data = _to_bytes(bytes_le)
            if len(data) != 16:
                raise ValueError("bytes_le is not a 16-char string")
            value = _int_from_bytes(_bytes_le_to_bytes(data))
        elif fields is not None:
            value = _int_from_fields(fields)
        else:
            value = _validate_int_value(int_value)

        if version is not None:
            if version not in (1, 2, 3, 4, 5):
                raise ValueError("illegal version number")
            value = _apply_version_variant(value, version)

        self._int = value
        self._bytes = _int_to_bytes(value, 16)
        self._is_safe = SafeUUID.unknown

    @property
    def bytes(self) -> _builtins.bytes:
        return self._bytes

    @property
    def bytes_le(self) -> _builtins.bytes:
        return _bytes_to_bytes_le(self._bytes)

    @property
    def fields(self) -> tuple[Int, Int, Int, Int, Int, Int]:
        return _fields_from_bytes(self._bytes)

    @property
    def time_low(self) -> Int:
        return self.fields[0]

    @property
    def time_mid(self) -> Int:
        return self.fields[1]

    @property
    def time_hi_version(self) -> Int:
        return self.fields[2]

    @property
    def clock_seq_hi_variant(self) -> Int:
        return self.fields[3]

    @property
    def clock_seq_low(self) -> Int:
        return self.fields[4]

    @property
    def node(self) -> Int:
        return self.fields[5]

    @property
    def time(self) -> Int:
        time_hi = self.time_hi_version & 0x0FFF
        return (time_hi << 48) | (self.time_mid << 32) | self.time_low

    @property
    def clock_seq(self) -> Int:
        return ((self.clock_seq_hi_variant & 0x3F) << 8) | self.clock_seq_low

    @property
    def hex(self) -> str:
        return _bytes_to_hex(self._bytes)

    @property
    def int(self) -> Int:
        return int(self._int)

    @property
    def urn(self) -> str:
        return f"urn:uuid:{self}"

    @property
    def variant(self) -> str:
        octet = self.clock_seq_hi_variant
        if octet & 0x80 == 0x00:
            return RESERVED_NCS
        if octet & 0xC0 == 0x80:
            return RFC_4122
        if octet & 0xE0 == 0xC0:
            return RESERVED_MICROSOFT
        return RESERVED_FUTURE

    @property
    def version(self) -> Int | None:
        if self.variant != RFC_4122:
            return None
        return (self._bytes[6] >> 4) & 0x0F

    @property
    def is_safe(self) -> SafeUUID:
        return self._is_safe

    def __repr__(self) -> str:
        return f"UUID('{self}')"

    def __str__(self) -> str:
        value = self.hex
        return (
            value[0:8]
            + "-"
            + value[8:12]
            + "-"
            + value[12:16]
            + "-"
            + value[16:20]
            + "-"
            + value[20:32]
        )

    def __eq__(self, other: object) -> bool:
        if isinstance(other, UUID):
            return self._int == other._int
        return NotImplemented

    def __hash__(self) -> Int:
        return hash(self._int)


def uuid1(node: int | None = None, clock_seq: int | None = None) -> UUID:
    global _CLOCK_SEQ, _LAST_TIMESTAMP
    if node is None:
        node = getnode()
    node = _validate_node(node)

    if clock_seq is not None:
        clock_seq = _normalize_clock_seq(clock_seq)
    elif _CLOCK_SEQ is None:
        _CLOCK_SEQ = _random.getrandbits(14)

    timestamp = _timestamp_100ns()
    if _LAST_TIMESTAMP is not None and timestamp <= _LAST_TIMESTAMP:
        timestamp = _LAST_TIMESTAMP + 1
        if clock_seq is None:
            _CLOCK_SEQ = ((_CLOCK_SEQ or 0) + 1) & 0x3FFF
    _LAST_TIMESTAMP = timestamp

    if clock_seq is None:
        clock_seq = _CLOCK_SEQ or 0

    time_low = timestamp & 0xFFFFFFFF
    time_mid = (timestamp >> 32) & 0xFFFF
    time_hi_version = (timestamp >> 48) & 0x0FFF
    time_hi_version |= 1 << 12
    clock_seq_low = clock_seq & 0xFF
    clock_seq_hi_variant = (clock_seq >> 8) & 0x3F
    clock_seq_hi_variant |= 0x80

    return UUID(
        fields=(
            time_low,
            time_mid,
            time_hi_version,
            clock_seq_hi_variant,
            clock_seq_low,
            node,
        )
    )


def uuid3(namespace: UUID, name: str | bytes) -> UUID:
    name_bytes = _coerce_name(namespace, name)
    digest = _md5(namespace.bytes + name_bytes)
    return _uuid_from_hash(digest, 3)


def uuid4() -> UUID:
    value = _random.getrandbits(128)
    data = _int_to_bytes(value, 16)
    return UUID(bytes=data, version=4)


def uuid5(namespace: UUID, name: str | bytes) -> UUID:
    name_bytes = _coerce_name(namespace, name)
    digest = _hashlib.sha1(namespace.bytes + name_bytes).digest()
    return _uuid_from_hash(digest, 5)


def getnode() -> int:
    global _NODE
    if _NODE is None:
        node = _random.getrandbits(48)
        node |= 0x010000000000
        _NODE = node
    return _NODE


def _timestamp_100ns() -> int:
    seconds = _time.time()
    return int(seconds * 10_000_000) + _UUID_EPOCH


def _uuid_from_hash(digest: bytes, version: int) -> UUID:
    data = digest[:16]
    return UUID(bytes=data, version=version)


def _coerce_name(namespace: UUID, name: str | bytes) -> bytes:
    if not isinstance(namespace, UUID):
        raise TypeError("namespace must be a UUID")
    if isinstance(name, str):
        return name.encode("utf-8")
    if isinstance(name, (bytes, bytearray, memoryview)):
        return _to_bytes(name)
    raise TypeError("name must be a string or bytes")


def _validate_int_value(value: int | None) -> int:
    if value is None:
        raise TypeError(
            "one of the hex, bytes, bytes_le, fields, or int arguments must be given"
        )
    value = _builtins.int(value)
    if value < 0 or value >= (1 << 128):
        raise ValueError("int is out of range (need a 128-bit value)")
    return value


def _validate_node(value: int) -> int:
    value = _builtins.int(value)
    if value < 0 or value >= (1 << 48):
        raise ValueError("node is out of range (need a 48-bit value)")
    return value


def _normalize_clock_seq(value: int) -> int:
    return _builtins.int(value) & 0x3FFF


def _int_from_fields(fields: tuple[int, int, int, int, int, int]) -> int:
    try:
        items = tuple(fields)
    except TypeError as exc:
        raise ValueError("fields is not a 6-tuple") from exc
    if len(items) != 6:
        raise ValueError("fields is not a 6-tuple")
    time_low, time_mid, time_hi, seq_hi, seq_low, node = items
    time_low = _validate_field(time_low, 32, 1)
    time_mid = _validate_field(time_mid, 16, 2)
    time_hi = _validate_field(time_hi, 16, 3)
    seq_hi = _validate_field(seq_hi, 8, 4)
    seq_low = _validate_field(seq_low, 8, 5)
    node = _validate_field(node, 48, 6)
    data = (
        time_low.to_bytes(4, "big")
        + time_mid.to_bytes(2, "big")
        + time_hi.to_bytes(2, "big")
        + _builtins.bytes([seq_hi, seq_low])
        + node.to_bytes(6, "big")
    )
    return _int_from_bytes(data)


def _validate_field(value: int, bits: int, index: int) -> int:
    value = _builtins.int(value)
    if value < 0 or value >= (1 << bits):
        if bits == 32:
            raise ValueError("field 1 out of range (need a 32-bit value)")
        if bits == 16:
            raise ValueError(f"field {index} out of range (need a 16-bit value)")
        if bits == 8:
            raise ValueError(f"field {index} out of range (need an 8-bit value)")
        if bits == 48:
            raise ValueError("field 6 out of range (need a 48-bit value)")
        raise ValueError(f"field {index} out of range")
    return value


def _apply_version_variant(value: int, version: int) -> int:
    data = bytearray(_int_to_bytes(value, 16))
    data[6] = (data[6] & 0x0F) | ((version & 0x0F) << 4)
    data[8] = (data[8] & 0x3F) | 0x80
    return _int_from_bytes(data)


def _int_from_hex(text: str) -> int:
    value = text.strip().lower()
    if value.startswith("urn:uuid:"):
        value = value[9:]
    if value.startswith("{") and value.endswith("}"):
        value = value[1:-1]
    value = value.replace("-", "")
    if len(value) != 32:
        raise ValueError("badly formed hexadecimal UUID string")
    try:
        return int(value, 16)
    except ValueError as exc:
        raise ValueError("badly formed hexadecimal UUID string") from exc


def _int_from_bytes(data: _builtins.bytes | bytearray) -> Int:
    value = 0
    for byte in data:
        value = (value << 8) | int(byte)
    return value


def _int_to_bytes(value: int, length: int) -> bytes:
    if value < 0:
        raise ValueError("value must be non-negative")
    out = bytearray(length)
    for idx in range(length - 1, -1, -1):
        out[idx] = value & 0xFF
        value >>= 8
    return bytes(out)


def _bytes_le_to_bytes(data: bytes) -> bytes:
    return data[0:4][::-1] + data[4:6][::-1] + data[6:8][::-1] + data[8:16]


def _bytes_to_bytes_le(data: bytes) -> bytes:
    return data[0:4][::-1] + data[4:6][::-1] + data[6:8][::-1] + data[8:16]


def _bytes_to_hex(data: bytes) -> str:
    chars = []
    for byte in data:
        chars.append(format(int(byte), "02x"))
    return "".join(chars)


def _fields_from_bytes(data: bytes) -> tuple[int, int, int, int, int, int]:
    return (
        int.from_bytes(data[0:4], "big"),
        int.from_bytes(data[4:6], "big"),
        int.from_bytes(data[6:8], "big"),
        data[8],
        data[9],
        int.from_bytes(data[10:16], "big"),
    )


def _to_bytes(value: Any) -> bytes:
    if isinstance(value, _builtins.bytes):
        return value
    if isinstance(value, (bytearray, memoryview)):
        return _builtins.bytes(value)
    raise TypeError("a bytes-like object is required")


def _left_rotate(value: int, shift: int) -> int:
    return ((value << shift) | (value >> (32 - shift))) & 0xFFFFFFFF


def _md5(data: bytes) -> bytes:
    message = _to_bytes(data)
    message_len_bits = (len(message) * 8) & 0xFFFFFFFFFFFFFFFF
    message += b"\x80"
    pad_len = (56 - (len(message) % 64)) % 64
    message += b"\x00" * pad_len
    message += message_len_bits.to_bytes(8, "little")

    a0 = 0x67452301
    b0 = 0xEFCDAB89
    c0 = 0x98BADCFE
    d0 = 0x10325476

    s = [
        7,
        12,
        17,
        22,
        7,
        12,
        17,
        22,
        7,
        12,
        17,
        22,
        7,
        12,
        17,
        22,
        5,
        9,
        14,
        20,
        5,
        9,
        14,
        20,
        5,
        9,
        14,
        20,
        5,
        9,
        14,
        20,
        4,
        11,
        16,
        23,
        4,
        11,
        16,
        23,
        4,
        11,
        16,
        23,
        4,
        11,
        16,
        23,
        6,
        10,
        15,
        21,
        6,
        10,
        15,
        21,
        6,
        10,
        15,
        21,
        6,
        10,
        15,
        21,
    ]
    k = [
        0xD76AA478,
        0xE8C7B756,
        0x242070DB,
        0xC1BDCEEE,
        0xF57C0FAF,
        0x4787C62A,
        0xA8304613,
        0xFD469501,
        0x698098D8,
        0x8B44F7AF,
        0xFFFF5BB1,
        0x895CD7BE,
        0x6B901122,
        0xFD987193,
        0xA679438E,
        0x49B40821,
        0xF61E2562,
        0xC040B340,
        0x265E5A51,
        0xE9B6C7AA,
        0xD62F105D,
        0x02441453,
        0xD8A1E681,
        0xE7D3FBC8,
        0x21E1CDE6,
        0xC33707D6,
        0xF4D50D87,
        0x455A14ED,
        0xA9E3E905,
        0xFCEFA3F8,
        0x676F02D9,
        0x8D2A4C8A,
        0xFFFA3942,
        0x8771F681,
        0x6D9D6122,
        0xFDE5380C,
        0xA4BEEA44,
        0x4BDECFA9,
        0xF6BB4B60,
        0xBEBFBC70,
        0x289B7EC6,
        0xEAA127FA,
        0xD4EF3085,
        0x04881D05,
        0xD9D4D039,
        0xE6DB99E5,
        0x1FA27CF8,
        0xC4AC5665,
        0xF4292244,
        0x432AFF97,
        0xAB9423A7,
        0xFC93A039,
        0x655B59C3,
        0x8F0CCC92,
        0xFFEFF47D,
        0x85845DD1,
        0x6FA87E4F,
        0xFE2CE6E0,
        0xA3014314,
        0x4E0811A1,
        0xF7537E82,
        0xBD3AF235,
        0x2AD7D2BB,
        0xEB86D391,
    ]

    for chunk_start in range(0, len(message), 64):
        chunk = message[chunk_start : chunk_start + 64]
        m = [0] * 16
        for i in range(16):
            start = i * 4
            m[i] = int.from_bytes(chunk[start : start + 4], "little")

        a = a0
        b = b0
        c = c0
        d = d0

        for i in range(64):
            if i <= 15:
                f = (b & c) | (~b & d)
                g = i
            elif i <= 31:
                f = (d & b) | (~d & c)
                g = (5 * i + 1) % 16
            elif i <= 47:
                f = b ^ c ^ d
                g = (3 * i + 5) % 16
            else:
                f = c ^ (b | ~d)
                g = (7 * i) % 16

            temp = d
            d = c
            c = b
            rotate = (a + f + k[i] + m[g]) & 0xFFFFFFFF
            b = (b + _left_rotate(rotate, s[i])) & 0xFFFFFFFF
            a = temp

        a0 = (a0 + a) & 0xFFFFFFFF
        b0 = (b0 + b) & 0xFFFFFFFF
        c0 = (c0 + c) & 0xFFFFFFFF
        d0 = (d0 + d) & 0xFFFFFFFF

    return (
        a0.to_bytes(4, "little")
        + b0.to_bytes(4, "little")
        + c0.to_bytes(4, "little")
        + d0.to_bytes(4, "little")
    )


NAMESPACE_DNS = UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_URL = UUID("6ba7b811-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_OID = UUID("6ba7b812-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_X500 = UUID("6ba7b814-9dad-11d1-80b4-00c04fd430c8")
