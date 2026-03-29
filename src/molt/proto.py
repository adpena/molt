"""Protobuf message mapping for Molt-compiled Python.

Provides the @message decorator and field() function for mapping
Python dataclasses to protobuf messages via buffa.

Usage:
    from molt.proto import message, field

    @message("mypackage.UserProfile")
    class UserProfile:
        name: str = field(1)
        age: int = field(2)
        email: str = field(3)
        scores: list[float] = field(4, repeated=True)

    # Encode
    user = UserProfile(name="Alice", age=30, email="alice@example.com")
    wire_bytes = user.encode()

    # Decode
    decoded = UserProfile.decode(wire_bytes)
"""
from __future__ import annotations

import dataclasses
from dataclasses import dataclass
from typing import Any, Optional, get_type_hints


# Wire type mapping
WIRE_TYPE_VARINT = 0
WIRE_TYPE_FIXED64 = 1
WIRE_TYPE_LENGTH_DELIMITED = 2
WIRE_TYPE_FIXED32 = 5

# Python type to wire type mapping
_TYPE_TO_WIRE: dict[type, int] = {
    int: WIRE_TYPE_VARINT,
    bool: WIRE_TYPE_VARINT,
    float: WIRE_TYPE_FIXED64,
    str: WIRE_TYPE_LENGTH_DELIMITED,
    bytes: WIRE_TYPE_LENGTH_DELIMITED,
}


class FieldDef:
    """Protobuf field definition."""

    def __init__(
        self,
        number: int,
        *,
        repeated: bool = False,
        optional: bool = False,
        wire_type: Optional[int] = None,
    ):
        if number < 1 or number > 536870911:  # 2^29 - 1
            raise ValueError(f"field number must be 1-536870911, got {number}")
        self.number = number
        self.repeated = repeated
        self.optional = optional
        self._wire_type = wire_type

    def wire_type_for(self, python_type: type) -> int:
        if self._wire_type is not None:
            return self._wire_type
        origin = getattr(python_type, "__origin__", None)
        if origin is list:
            # Repeated field — wire type of element
            args = getattr(python_type, "__args__", (Any,))
            return _TYPE_TO_WIRE.get(args[0], WIRE_TYPE_LENGTH_DELIMITED)
        return _TYPE_TO_WIRE.get(python_type, WIRE_TYPE_LENGTH_DELIMITED)


def field(number: int, *, repeated: bool = False, optional: bool = False) -> Any:
    """Define a protobuf field with a field number.

    Args:
        number: Protobuf field number (1-based, must be unique within message)
        repeated: Whether this is a repeated (list) field
        optional: Whether this field has presence tracking
    """
    return dataclasses.field(
        default=None if optional else dataclasses.MISSING,
        metadata={"proto_field": FieldDef(number, repeated=repeated, optional=optional)},
    )


class MessageMeta:
    """Metadata attached to @message-decorated classes."""

    def __init__(self, proto_name: str, fields: dict[str, FieldDef]):
        self.proto_name = proto_name
        self.fields = fields
        # Validate no duplicate field numbers
        numbers = [f.number for f in fields.values()]
        if len(numbers) != len(set(numbers)):
            dupes = [n for n in numbers if numbers.count(n) > 1]
            raise ValueError(f"duplicate field numbers: {set(dupes)}")


def message(proto_name: str):
    """Decorator that maps a Python class to a protobuf message.

    The class is converted to a dataclass and annotated with protobuf
    field metadata. At compile time, Molt's frontend recognizes this
    decorator and generates typed IR for encode/decode.

    Args:
        proto_name: Fully qualified protobuf message name
                    (e.g., "mypackage.MyMessage")
    """
    def decorator(cls):
        # Convert to dataclass if not already
        if not dataclasses.is_dataclass(cls):
            cls = dataclass(cls)

        # Extract field definitions from metadata
        proto_fields: dict[str, FieldDef] = {}
        for f in dataclasses.fields(cls):
            proto_def = f.metadata.get("proto_field")
            if isinstance(proto_def, FieldDef):
                proto_fields[f.name] = proto_def

        if not proto_fields:
            raise ValueError(
                f"@message class {cls.__name__} has no proto fields. "
                f"Use field(N) to define protobuf field numbers."
            )

        # Attach metadata
        cls.__proto_meta__ = MessageMeta(proto_name, proto_fields)

        # Add encode/decode stubs
        def encode(self) -> bytes:
            """Encode this message to protobuf binary format."""
            # In compiled Molt, this is replaced by buffa-backed codegen.
            # In CPython, we provide a basic implementation.
            return _encode_message(self)

        def decode(cls_inner, data: bytes):
            """Decode protobuf binary data into a message instance."""
            return _decode_message(cls_inner, data)

        cls.encode = encode
        cls.decode = classmethod(decode)

        return cls

    return decorator


def _encode_varint(value: int) -> bytes:
    """Encode an unsigned integer as a protobuf varint."""
    result = bytearray()
    while value > 0x7F:
        result.append((value & 0x7F) | 0x80)
        value >>= 7
    result.append(value & 0x7F)
    return bytes(result)


def _encode_message(obj) -> bytes:
    """Basic protobuf encoding (CPython fallback, not used in compiled Molt)."""
    meta: MessageMeta = obj.__proto_meta__
    buf = bytearray()

    for attr_name, field_def in meta.fields.items():
        value = getattr(obj, attr_name, None)
        if value is None:
            continue

        if isinstance(value, bool):
            buf.extend(_encode_varint((field_def.number << 3) | WIRE_TYPE_VARINT))
            buf.extend(_encode_varint(1 if value else 0))
        elif isinstance(value, int):
            buf.extend(_encode_varint((field_def.number << 3) | WIRE_TYPE_VARINT))
            # ZigZag encode for signed ints
            encoded = (value << 1) ^ (value >> 63) if value < 0 else value
            buf.extend(_encode_varint(encoded))
        elif isinstance(value, str):
            encoded = value.encode("utf-8")
            buf.extend(_encode_varint((field_def.number << 3) | WIRE_TYPE_LENGTH_DELIMITED))
            buf.extend(_encode_varint(len(encoded)))
            buf.extend(encoded)
        elif isinstance(value, bytes):
            buf.extend(_encode_varint((field_def.number << 3) | WIRE_TYPE_LENGTH_DELIMITED))
            buf.extend(_encode_varint(len(value)))
            buf.extend(value)
        elif isinstance(value, float):
            import struct
            buf.extend(_encode_varint((field_def.number << 3) | WIRE_TYPE_FIXED64))
            buf.extend(struct.pack("<d", value))

    return bytes(buf)


def _decode_message(cls, data: bytes):
    """Basic protobuf decoding (CPython fallback)."""
    # Minimal implementation — just enough to roundtrip simple messages
    meta: MessageMeta = cls.__proto_meta__
    field_by_number = {fd.number: (name, fd) for name, fd in meta.fields.items()}
    kwargs: dict[str, Any] = {}
    pos = 0

    while pos < len(data):
        # Read tag
        tag, consumed = _decode_varint(data, pos)
        pos += consumed
        field_number = tag >> 3
        wire_type = tag & 0x07

        if field_number not in field_by_number:
            # Skip unknown field
            pos = _skip_field(data, pos, wire_type)
            continue

        name, field_def = field_by_number[field_number]

        if wire_type == WIRE_TYPE_VARINT:
            value, consumed = _decode_varint(data, pos)
            pos += consumed
            # Check if bool
            hints = get_type_hints(cls)
            if hints.get(name) is bool:
                kwargs[name] = bool(value)
            else:
                kwargs[name] = value
        elif wire_type == WIRE_TYPE_LENGTH_DELIMITED:
            length, consumed = _decode_varint(data, pos)
            pos += consumed
            payload = data[pos:pos + length]
            pos += length
            hints = get_type_hints(cls)
            if hints.get(name) is str:
                kwargs[name] = payload.decode("utf-8")
            else:
                kwargs[name] = bytes(payload)
        elif wire_type == WIRE_TYPE_FIXED64:
            import struct
            value = struct.unpack("<d", data[pos:pos + 8])[0]
            pos += 8
            kwargs[name] = value

    return cls(**kwargs)


def _decode_varint(data: bytes, pos: int) -> tuple[int, int]:
    """Decode a varint at position. Returns (value, bytes_consumed)."""
    value = 0
    shift = 0
    consumed = 0
    while pos < len(data):
        byte = data[pos]
        pos += 1
        consumed += 1
        value |= (byte & 0x7F) << shift
        if not (byte & 0x80):
            return value, consumed
        shift += 7
    raise ValueError("truncated varint")


def _skip_field(data: bytes, pos: int, wire_type: int) -> int:
    """Skip an unknown field."""
    if wire_type == WIRE_TYPE_VARINT:
        while pos < len(data) and data[pos] & 0x80:
            pos += 1
        return pos + 1
    elif wire_type == WIRE_TYPE_FIXED64:
        return pos + 8
    elif wire_type == WIRE_TYPE_FIXED32:
        return pos + 4
    elif wire_type == WIRE_TYPE_LENGTH_DELIMITED:
        length, consumed = _decode_varint(data, pos)
        return pos + consumed + length
    raise ValueError(f"unknown wire type {wire_type}")
