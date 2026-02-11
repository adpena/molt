"""UUID support for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic
import builtins as _builtins
import enum as _enum

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

_NODE: int | None = None

_MOLT_UUID_GETNODE = _require_intrinsic("molt_uuid_getnode", globals())
_MOLT_UUID_UUID1_BYTES = _require_intrinsic("molt_uuid_uuid1_bytes", globals())
_MOLT_UUID_UUID3_BYTES = _require_intrinsic("molt_uuid_uuid3_bytes", globals())
_MOLT_UUID_UUID4_BYTES = _require_intrinsic("molt_uuid_uuid4_bytes", globals())
_MOLT_UUID_UUID5_BYTES = _require_intrinsic("molt_uuid_uuid5_bytes", globals())


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
        return _builtins.int(self._int)

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
    node_override = _validate_node(node) if node is not None else None
    clock_seq_override = (
        _normalize_clock_seq(clock_seq) if clock_seq is not None else None
    )
    payload = _to_bytes(_MOLT_UUID_UUID1_BYTES(node_override, clock_seq_override))
    if len(payload) != 16:
        raise RuntimeError("invalid uuid1 intrinsic payload")
    return UUID(bytes=payload)


def uuid3(namespace: UUID, name: str | bytes) -> UUID:
    name_bytes = _coerce_name(namespace, name)
    payload = _to_bytes(_MOLT_UUID_UUID3_BYTES(namespace.bytes, name_bytes))
    if len(payload) != 16:
        raise RuntimeError("invalid uuid3 intrinsic payload")
    return UUID(bytes=payload)


def uuid4() -> UUID:
    payload = _to_bytes(_MOLT_UUID_UUID4_BYTES())
    if len(payload) != 16:
        raise RuntimeError("invalid uuid4 intrinsic payload")
    return UUID(bytes=payload)


def uuid5(namespace: UUID, name: str | bytes) -> UUID:
    name_bytes = _coerce_name(namespace, name)
    payload = _to_bytes(_MOLT_UUID_UUID5_BYTES(namespace.bytes, name_bytes))
    if len(payload) != 16:
        raise RuntimeError("invalid uuid5 intrinsic payload")
    return UUID(bytes=payload)


def getnode() -> int:
    global _NODE
    if _NODE is None:
        node = _MOLT_UUID_GETNODE()
        if not isinstance(node, int):
            raise RuntimeError("invalid uuid getnode intrinsic payload")
        _NODE = _validate_node(node)
    return _NODE


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
    data = _builtins.bytearray(_int_to_bytes(value, 16))
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
        return _builtins.int(value, 16)
    except ValueError as exc:
        raise ValueError("badly formed hexadecimal UUID string") from exc


def _int_from_bytes(data: _builtins.bytes | bytearray) -> Int:
    return _builtins.int.from_bytes(data, "big")


def _int_to_bytes(value: int, length: int) -> bytes:
    if value < 0:
        raise ValueError("value must be non-negative")
    width = length * 2
    hex_text = _builtins.format(value, "x")
    if len(hex_text) > width:
        hex_text = hex_text[-width:]
    if len(hex_text) < width:
        hex_text = ("0" * (width - len(hex_text))) + hex_text
    out = _builtins.bytearray(length)
    for idx in _builtins.range(length):
        start = idx * 2
        out[idx] = _builtins.int(hex_text[start : start + 2], 16)
    return _builtins.bytes(out)


def _bytes_le_to_bytes(data: bytes) -> bytes:
    return data[0:4][::-1] + data[4:6][::-1] + data[6:8][::-1] + data[8:16]


def _bytes_to_bytes_le(data: bytes) -> bytes:
    return data[0:4][::-1] + data[4:6][::-1] + data[6:8][::-1] + data[8:16]


def _bytes_to_hex(data: bytes) -> str:
    chars = []
    for byte in data:
        chars.append(_builtins.format(_builtins.int(byte), "02x"))
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


NAMESPACE_DNS = UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_URL = UUID("6ba7b811-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_OID = UUID("6ba7b812-9dad-11d1-80b4-00c04fd430c8")
NAMESPACE_X500 = UUID("6ba7b814-9dad-11d1-80b4-00c04fd430c8")
