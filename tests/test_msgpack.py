import pytest

import molt_msgpack
from molt import intrinsics as _intrinsics

msgpack = pytest.importorskip("msgpack")
cbor2 = pytest.importorskip("cbor2")

if not _intrinsics.runtime_active():
    pytest.skip("Molt runtime intrinsics not active", allow_module_level=True)


def test_msgpack_scalars():
    assert molt_msgpack.parse(msgpack.packb(42)) == 42
    assert molt_msgpack.parse_msgpack(msgpack.packb(42)) == 42
    assert molt_msgpack.parse_msgpack(msgpack.packb(True)) is True
    assert molt_msgpack.parse_msgpack(msgpack.packb(None)) is None
    assert molt_msgpack.parse_msgpack(msgpack.packb(3.5)) == 3.5
    assert molt_msgpack.parse_msgpack(msgpack.packb("hi")) == "hi"
    assert molt_msgpack.parse_msgpack(msgpack.packb(b"\x00\x01")) == b"\x00\x01"
    assert molt_msgpack.parse_msgpack(msgpack.packb([1, 2])) == [1, 2]
    assert molt_msgpack.parse_msgpack(msgpack.packb({"a": 1})) == {"a": 1}


def test_cbor_scalars():
    assert molt_msgpack.parse_cbor(cbor2.dumps(42)) == 42
    assert molt_msgpack.parse_cbor(cbor2.dumps(True)) is True
    assert molt_msgpack.parse_cbor(cbor2.dumps(None)) is None
    assert molt_msgpack.parse_cbor(cbor2.dumps(3.5)) == 3.5
    assert molt_msgpack.parse_cbor(cbor2.dumps("hi")) == "hi"
    assert molt_msgpack.parse_cbor(cbor2.dumps(b"hi")) == b"hi"
    assert molt_msgpack.parse_cbor(cbor2.dumps([1, 2])) == [1, 2]
    assert molt_msgpack.parse_cbor(cbor2.dumps({"a": 1})) == {"a": 1}
