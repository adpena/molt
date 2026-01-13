import pytest

import molt_msgpack
from molt import shims

msgpack = pytest.importorskip("msgpack")
cbor2 = pytest.importorskip("cbor2")


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


def test_runtime_msgpack_bytes_roundtrip():
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, "molt_msgpack_parse_scalar"):
        pytest.skip("runtime msgpack parser not available")
    data = msgpack.packb(b"\x00\x01", use_bin_type=True)
    assert molt_msgpack._parse_runtime(data, "molt_msgpack_parse_scalar") == b"\x00\x01"


def test_runtime_cbor_bytes_roundtrip():
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, "molt_cbor_parse_scalar"):
        pytest.skip("runtime cbor parser not available")
    data = cbor2.dumps(b"hi")
    assert molt_msgpack._parse_runtime(data, "molt_cbor_parse_scalar") == b"hi"


def test_runtime_msgpack_bytes_methods():
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, "molt_msgpack_parse_scalar"):
        pytest.skip("runtime msgpack parser not available")
    data = msgpack.packb(b"one,two", use_bin_type=True)
    val = molt_msgpack._parse_runtime(data, "molt_msgpack_parse_scalar")
    assert val.split(b",") == [b"one", b"two"]
    assert val.replace(b"one", b"uno") == b"uno,two"


def test_runtime_cbor_bytes_methods():
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, "molt_cbor_parse_scalar"):
        pytest.skip("runtime cbor parser not available")
    data = cbor2.dumps(b"one,two")
    val = molt_msgpack._parse_runtime(data, "molt_cbor_parse_scalar")
    assert val.split(b",") == [b"one", b"two"]
    assert val.replace(b"two", b"dos") == b"one,dos"
