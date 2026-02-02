"""Minimal UUID support for Molt."""

from __future__ import annotations

__all__ = ["UUID"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement
# full UUID surface (uuid1/uuid3/uuid4/uuid5, fields, bytes_le, and namespace constants).


class UUID:
    def __init__(
        self,
        hex: str | None = None,
        bytes: bytes | None = None,
        int_value: int | None = None,
        **kwargs: object,
    ) -> None:
        if "int" in kwargs:
            if int_value is not None:
                raise TypeError("got multiple values for argument 'int'")
            int_value = kwargs.pop("int")  # type: ignore[assignment]
        if kwargs:
            raise TypeError("got unexpected keyword arguments")
        if hex is None and bytes is None and int_value is None:
            raise TypeError("one of hex, bytes, or int must be specified")
        if hex is not None:
            self._int = _int_from_hex(hex)
        elif bytes is not None:
            if len(bytes) != 16:
                raise ValueError("bytes is not a 16-char string")
            self._int = _int_from_bytes(bytes)
        else:
            value = int(int_value or 0)
            if value < 0 or value >= 1 << 128:
                raise ValueError("int is out of range (need 0 <= int < 2**128)")
            self._int = value
        self._bytes = _int_to_bytes(int(self._int), 16)

    @property
    def bytes(self) -> bytes:
        return self._bytes

    @property
    def hex(self) -> str:
        return _bytes_to_hex(self._bytes)

    @property
    def int(self) -> int:
        return int(self._int)

    @property
    def version(self) -> int | None:
        if not self._is_rfc4122():
            return None
        return (self._bytes[6] >> 4) & 0x0F

    def _is_rfc4122(self) -> bool:
        return (self._bytes[8] & 0xC0) == 0x80

    def __repr__(self) -> str:
        return f"UUID('{str(self)}')"

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

    def __hash__(self) -> int:
        return hash(self._int)


def _int_from_hex(text: str) -> int:
    value = text.strip().lower()
    if value.startswith("{") and value.endswith("}"):
        value = value[1:-1]
    value = value.replace("-", "")
    if len(value) != 32:
        raise ValueError("badly formed hexadecimal UUID string")
    try:
        return int(value, 16)
    except ValueError as exc:
        raise ValueError("badly formed hexadecimal UUID string") from exc


def _int_from_bytes(data: bytes) -> int:
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


def _bytes_to_hex(data: bytes) -> str:
    chars = []
    for byte in data:
        chars.append(format(int(byte), "02x"))
    return "".join(chars)
