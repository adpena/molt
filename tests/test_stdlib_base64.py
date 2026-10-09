from __future__ import annotations

import base64 as py_base64
import builtins
import codecs

import pytest

# The stub binds every intrinsic when it is imported, so the fakes live in
# the process registry only for that import. A registry left installed
# would answer molt_capabilities_has for every later test on the worker.
_FAKE_INTRINSICS: dict[str, object] = {}
_FAKE_INTRINSICS.setdefault("molt_stdlib_probe", lambda: True)
_FAKE_INTRINSICS.setdefault("molt_capabilities_has", lambda _name=None: True)
_FAKE_INTRINSICS.setdefault(
    "molt_codecs_encode",
    lambda data, encoding, errors="strict": codecs.encode(data, encoding, errors),
)
_FAKE_INTRINSICS.setdefault(
    "molt_codecs_decode",
    lambda data, encoding, errors="strict": codecs.decode(data, encoding, errors),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b64encode",
    lambda data, altchars=None: py_base64.b64encode(data, altchars=altchars),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b64decode",
    lambda data, altchars=None, validate=False: py_base64.b64decode(
        data, altchars=altchars, validate=validate
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_standard_b64encode",
    lambda data: py_base64.standard_b64encode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_standard_b64decode",
    lambda data: py_base64.standard_b64decode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_urlsafe_b64encode",
    lambda data: py_base64.urlsafe_b64encode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_urlsafe_b64decode",
    lambda data: py_base64.urlsafe_b64decode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32encode", lambda data: py_base64.b32encode(data)
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32decode",
    lambda data, casefold=False, map01=None: py_base64.b32decode(
        data, casefold=casefold, map01=map01
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32hexencode",
    lambda data: py_base64.b32hexencode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32hexdecode",
    lambda data, casefold=False: py_base64.b32hexdecode(data, casefold=casefold),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b16encode", lambda data: py_base64.b16encode(data)
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b16decode",
    lambda data, casefold=False: py_base64.b16decode(data, casefold=casefold),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_a85encode",
    lambda data, foldspaces=False, wrapcol=0, pad=False, adobe=False: (
        py_base64.a85encode(
            data, foldspaces=foldspaces, wrapcol=wrapcol, pad=pad, adobe=adobe
        )
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_a85decode",
    lambda data, foldspaces=False, adobe=False: py_base64.a85decode(
        data, foldspaces=foldspaces, adobe=adobe
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b85encode",
    lambda data, pad=False: py_base64.b85encode(data, pad=pad),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b85decode", lambda data: py_base64.b85decode(data)
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_encodebytes",
    lambda data: py_base64.encodebytes(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_decodebytes",
    lambda data: py_base64.decodebytes(data),
)


def _import_molt_base64():
    missing = object()
    previous = getattr(builtins, "_molt_intrinsics", missing)
    registry = dict(previous) if isinstance(previous, dict) else {}
    for name, fake in _FAKE_INTRINSICS.items():
        registry.setdefault(name, fake)
    builtins._molt_intrinsics = registry
    try:
        from molt.stdlib import base64 as module
    finally:
        if previous is missing:
            del builtins._molt_intrinsics
        else:
            builtins._molt_intrinsics = previous
    return module


molt_base64 = _import_molt_base64()


@pytest.mark.parametrize(
    "data",
    [
        b"",
        b"f",
        b"fo",
        b"foo",
        b"foobar",
        bytes(range(0, 64)),
    ],
)
def test_b64_roundtrip(data: bytes) -> None:
    assert molt_base64.b64encode(data) == py_base64.b64encode(data)
    encoded = py_base64.b64encode(data)
    assert molt_base64.b64decode(encoded) == data
    assert molt_base64.urlsafe_b64encode(data) == py_base64.urlsafe_b64encode(data)
    assert molt_base64.urlsafe_b64decode(encoded) == py_base64.urlsafe_b64decode(
        encoded
    )


@pytest.mark.parametrize("data", [b"", b"f", b"foo", bytes(range(0, 32))])
def test_b16_roundtrip(data: bytes) -> None:
    encoded = py_base64.b16encode(data)
    assert molt_base64.b16encode(data) == encoded
    assert molt_base64.b16decode(encoded) == data
    assert molt_base64.b16decode(encoded.lower(), casefold=True) == data


@pytest.mark.parametrize("data", [b"", b"f", b"foo", bytes(range(0, 32))])
def test_b32_roundtrip(data: bytes) -> None:
    encoded = py_base64.b32encode(data)
    assert molt_base64.b32encode(data) == encoded
    assert molt_base64.b32decode(encoded) == data
    assert molt_base64.b32decode(encoded.lower(), casefold=True) == data


def test_b32_map01() -> None:
    encoded = py_base64.b32encode(b"test")
    mapped = encoded.replace(b"O", b"0")
    assert molt_base64.b32decode(mapped, map01="L") == b"test"


def test_b32hex_roundtrip() -> None:
    data = b"hello world"
    encoded = py_base64.b32hexencode(data)
    assert molt_base64.b32hexencode(data) == encoded
    assert molt_base64.b32hexdecode(encoded) == data


@pytest.mark.parametrize("data", [b"", b"hello", b"    ", b"\0\0\0\0"])
def test_a85_roundtrip(data: bytes) -> None:
    encoded = py_base64.a85encode(data)
    assert molt_base64.a85encode(data) == encoded
    assert molt_base64.a85decode(encoded) == data


def test_a85_options() -> None:
    data = b"hello world"
    assert molt_base64.a85encode(data, wrapcol=5) == py_base64.a85encode(
        data, wrapcol=5
    )
    assert molt_base64.a85encode(data, adobe=True) == py_base64.a85encode(
        data, adobe=True
    )
    adobe_encoded = py_base64.a85encode(data, adobe=True)
    assert molt_base64.a85decode(adobe_encoded, adobe=True) == data
    assert molt_base64.a85encode(b"    ", foldspaces=True) == py_base64.a85encode(
        b"    ", foldspaces=True
    )


@pytest.mark.parametrize("data", [b"", b"hello", b"molt", bytes(range(0, 64))])
def test_b85_roundtrip(data: bytes) -> None:
    encoded = py_base64.b85encode(data)
    assert molt_base64.b85encode(data) == encoded
    assert molt_base64.b85decode(encoded) == data


def test_z85_roundtrip() -> None:
    if not hasattr(py_base64, "z85encode"):
        pytest.skip("z85 not available in this Python")
    data = b"hello"
    encoded = py_base64.z85encode(data)
    assert molt_base64.z85encode(data) == encoded
    assert molt_base64.z85decode(encoded) == data


def test_encodebytes_decodebytes() -> None:
    data = b"hello world" * 10
    encoded = py_base64.encodebytes(data)
    assert molt_base64.encodebytes(data) == encoded
    assert molt_base64.decodebytes(encoded) == data
