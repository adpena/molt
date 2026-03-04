from __future__ import annotations

import base64 as py_base64
import builtins

import pytest

_INTRINSICS = {
    "molt_stdlib_probe": lambda: True,
    "molt_capabilities_has": lambda _name=None: True,
    "molt_base64_b64encode": py_base64.b64encode,
    "molt_base64_b64decode": py_base64.b64decode,
    "molt_base64_standard_b64encode": py_base64.standard_b64encode,
    "molt_base64_standard_b64decode": py_base64.standard_b64decode,
    "molt_base64_urlsafe_b64encode": py_base64.urlsafe_b64encode,
    "molt_base64_urlsafe_b64decode": py_base64.urlsafe_b64decode,
    "molt_base64_b32encode": py_base64.b32encode,
    "molt_base64_b32decode": py_base64.b32decode,
    "molt_base64_b32hexencode": py_base64.b32hexencode,
    "molt_base64_b32hexdecode": py_base64.b32hexdecode,
    "molt_base64_b16encode": py_base64.b16encode,
    "molt_base64_b16decode": py_base64.b16decode,
    "molt_base64_a85encode": lambda data,
    foldspaces=False,
    wrapcol=0,
    pad=False,
    adobe=False: py_base64.a85encode(
        data,
        foldspaces=foldspaces,
        wrapcol=wrapcol,
        pad=pad,
        adobe=adobe,
    ),
    "molt_base64_a85decode": lambda data,
    foldspaces=False,
    adobe=False: py_base64.a85decode(
        data,
        foldspaces=foldspaces,
        adobe=adobe,
    ),
    "molt_base64_b85encode": py_base64.b85encode,
    "molt_base64_b85decode": py_base64.b85decode,
    "molt_base64_encodebytes": py_base64.encodebytes,
    "molt_base64_decodebytes": py_base64.decodebytes,
}


def _install_intrinsic_lookup() -> None:
    setattr(builtins, "_molt_runtime", True)
    setattr(builtins, "_molt_intrinsics_strict", True)

    def _lookup(name: str):
        value = _INTRINSICS.get(name)
        if callable(value):
            return value
        return None

    setattr(builtins, "_molt_intrinsic_lookup", _lookup)


_install_intrinsic_lookup()

from molt.stdlib import base64 as molt_base64  # noqa: E402


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
