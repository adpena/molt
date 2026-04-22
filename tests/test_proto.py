"""Tests for the @molt.proto decorator."""

import sys

sys.path.insert(0, "src")

from molt.proto import message, field, FieldDef


@message("test.Simple")
class Simple:
    name: str = field(1)
    value: int = field(2)


@message("test.WithOptional")
class WithOptional:
    name: str = field(1)
    age: int = field(2, optional=True)


def test_message_creates_dataclass():
    s = Simple(name="hello", value=42)
    assert s.name == "hello"
    assert s.value == 42


def test_message_has_proto_meta():
    assert hasattr(Simple, "__proto_meta__")
    meta = Simple.__proto_meta__
    assert meta.proto_name == "test.Simple"
    assert "name" in meta.fields
    assert meta.fields["name"].number == 1
    assert meta.fields["value"].number == 2


def test_encode_decode_roundtrip():
    original = Simple(name="hello", value=42)
    wire = original.encode()
    assert isinstance(wire, bytes)
    assert len(wire) > 0

    decoded = Simple.decode(wire)
    assert decoded.name == "hello"
    assert decoded.value == 42


def test_optional_field_absent():
    obj = WithOptional(name="test")
    wire = obj.encode()
    decoded = WithOptional.decode(wire)
    assert decoded.name == "test"


def test_field_number_validation():
    try:
        FieldDef(0)
        assert False, "should have raised"
    except ValueError:
        pass


def test_duplicate_field_numbers():
    try:

        @message("test.Bad")
        class Bad:
            a: int = field(1)
            b: int = field(1)  # duplicate!

        assert False, "should have raised"
    except ValueError as e:
        assert "duplicate" in str(e)
