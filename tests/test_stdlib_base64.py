from __future__ import annotations

import base64 as py_base64
import binascii as py_binascii
import builtins
import codecs

import pytest

_CANONICAL_BINASCII_ERROR = py_binascii.Error


def _native_decode_outcome(operation, *args, **kwargs):
    try:
        return operation(*args, **kwargs)
    except _CANONICAL_BINASCII_ERROR:
        return None


# The stub binds every intrinsic when it is imported, so the fakes live in
# the process registry only for that import. A registry left installed
# would answer molt_capabilities_has for every later test on the worker.
_FAKE_INTRINSICS: dict[str, object] = {}
_FAKE_INTRINSICS.setdefault("molt_capabilities_has", lambda _name=None: True)
_FAKE_INTRINSICS.setdefault(
    "molt_codecs_encode",
    lambda data, encoding, errors, with_consumed: (
        codecs.lookup(encoding).encode(data, errors)
        if with_consumed
        else codecs.encode(data, encoding, errors)
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_codecs_decode",
    lambda data, encoding, errors, with_consumed: (
        codecs.lookup(encoding).decode(data, errors)
        if with_consumed
        else codecs.decode(data, encoding, errors)
    ),
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
    lambda data, casefold=False: _native_decode_outcome(
        py_base64.b32decode, data, casefold=casefold
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32hexencode",
    lambda data: py_base64.b32hexencode(data),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b32hexdecode",
    lambda data, casefold=False: _native_decode_outcome(
        py_base64.b32hexdecode, data, casefold=casefold
    ),
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b16encode", lambda data: py_base64.b16encode(data)
)
_FAKE_INTRINSICS.setdefault(
    "molt_base64_b16decode",
    lambda data, casefold=False: _native_decode_outcome(
        py_base64.b16decode, data, casefold=casefold
    ),
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


_DECODER_INPUTS = (
    ("b64decode", b"aGk="),
    ("standard_b64decode", b"aGk="),
    ("urlsafe_b64decode", b"aGk="),
    ("b32decode", b"NBUQ===="),
    ("b32hexdecode", b"D1KG===="),
    ("b16decode", b"6869"),
    ("a85decode", b"BP@"),
    ("b85decode", b"XlV"),
)


def _exception_observation(exc):
    if exc is None:
        return None
    return (
        type(exc).__name__,
        str(exc),
        _exception_observation(exc.__cause__),
        _exception_observation(exc.__context__),
        exc.__suppress_context__,
    )


def _observe(operation, argument):
    try:
        return ("ok", operation(argument))
    except Exception as exc:  # noqa: BLE001 - compare the public error contract
        return ("error", _exception_observation(exc))


@pytest.mark.parametrize("name,encoded", _DECODER_INPUTS)
@pytest.mark.parametrize("returned", ["bytes", "none", "int", "str", "list", "view"])
def test_decoder_string_encode_results_reach_source_operations(name, encoded, returned):
    def run(module):
        events = []
        result = {
            "bytes": encoded,
            "none": None,
            "int": 7,
            "str": encoded.decode(),
            "list": list(encoded),
            "view": memoryview(encoded),
        }[returned]

        class Text(str):
            def encode(self, *args):
                events.append(("encode", args))
                return result

        return _observe(getattr(module, name), Text("ignored")), events

    assert run(molt_base64) == run(py_base64)


@pytest.mark.parametrize("name,encoded", _DECODER_INPUTS)
@pytest.mark.parametrize("base", [bytes, bytearray])
def test_decoder_byte_subtypes_keep_observable_protocols(name, encoded, base):
    def run(module):
        events = []

        class Data(base):
            def __len__(self):
                events.append("len")
                return super().__len__()

            def __getitem__(self, index):
                events.append(("getitem", repr(index)))
                return super().__getitem__(index)

            def translate(self, *args):
                events.append("translate")
                return super().translate(*args)

            def rstrip(self, *args):
                events.append("rstrip")
                return super().rstrip(*args)

        return _observe(getattr(module, name), Data(encoded)), events

    assert run(molt_base64) == run(py_base64)


@pytest.mark.parametrize("name,encoded", _DECODER_INPUTS)
def test_exact_decode_inputs_keep_native_kernels(monkeypatch, name, encoded):
    def no_generic(_argument):
        raise AssertionError("exact input entered the generic operand suffix")

    monkeypatch.setattr(molt_base64, "_bytes_from_decode_data", no_generic)
    assert getattr(molt_base64, name)(encoded) == getattr(py_base64, name)(encoded)


def test_encodebytes_byte_subtype_keeps_length_and_slice_protocol():
    def run(module):
        events = []

        class Data(bytes):
            def __len__(self):
                events.append("len")
                return super().__len__()

            def __getitem__(self, index):
                events.append(("getitem", repr(index)))
                return b"different"

        return module.encodebytes(Data(b"input")), events

    assert run(molt_base64) == run(py_base64)


@pytest.mark.parametrize("alphabet", ["-_", [45, 95], 1, b"-", b"++", bytearray(b"-_")])
def test_encoder_alphabet_uses_encoding_protocol(alphabet):
    assert _observe(
        lambda alt: molt_base64.b64encode(b"\xfb\xff", alt), alphabet
    ) == _observe(lambda alt: py_base64.b64encode(b"\xfb\xff", alt), alphabet)


def test_duplicate_decode_alphabet_uses_translation_last_binding():
    assert molt_base64.b64decode(b"AAAA", b"AA") == py_base64.b64decode(b"AAAA", b"AA")


def test_native_guard_does_not_compare_user_metaclasses():
    class Meta(type):
        def __eq__(self, other):
            raise AssertionError("native guard invoked metaclass equality")

    class Buffer(metaclass=Meta):
        def __buffer__(self, flags):
            return memoryview(b"aGk=")

    assert molt_base64.b64decode(Buffer()) == b"hi"


def test_immutable_encoder_alphabet_keeps_single_native_dispatch(monkeypatch):
    calls = []

    def native(data, alphabet):
        calls.append((data, alphabet))
        return b"native encoded"

    def no_generic(_data):
        raise AssertionError("immutable alphabet entered generic encoding")

    monkeypatch.setattr(molt_base64, "_molt_base64_b64encode", native)
    monkeypatch.setattr(molt_base64, "_molt_base64_standard_b64encode", no_generic)
    source = object()
    assert molt_base64.b64encode(source, b"-_") == b"native encoded"
    assert calls == [(source, b"-_")]


@pytest.mark.parametrize(
    "name,encoded",
    [
        ("b32decode", b"!======="),
        ("b32hexdecode", b"!======="),
        ("b16decode", b"!0"),
        ("b16decode", b"aa"),
        ("b16decode", b"A"),
    ],
)
@pytest.mark.parametrize("binding", ["class", "callable", "noncallable"])
def test_declined_native_decode_uses_actual_source_error_authority(
    monkeypatch, name, encoded, binding
):
    # The native outcome is independent of the mutable Python exception name.
    monkeypatch.setattr(molt_base64, "_molt_base64_" + name, lambda *_args: None)

    def run(module):
        events = []
        replacement = LookupError if binding == "class" else 17
        if binding == "callable":

            def replacement(*args):
                events.append(args)
                return LookupError(*args)

        monkeypatch.setattr(py_binascii, "Error", replacement)
        return _observe(getattr(module, name), encoded), events

    assert run(molt_base64) == run(py_base64)
