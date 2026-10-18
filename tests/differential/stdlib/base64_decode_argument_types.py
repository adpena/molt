"""Purpose: base64 decoders coerce their argument like CPython (HF-322).

CPython's `_bytes_from_decode_data` accepts an ASCII `str` or any buffer and
turns the TypeError from `memoryview(s)` into "argument should be a
bytes-like object or ASCII string, not '<type>'". A non-ASCII `str` raises
ValueError. Only a TypeError, subclasses included, is replaced: another
error from `__buffer__` propagates unchanged. The legacy `decodebytes` and
`encodebytes` check their input with `_input_type_check` instead: a 1-D
buffer of single bytes, never `str`.
"""

import array
import base64


class TypeErrorBuffer:
    def __buffer__(self, flags):
        raise TypeError("buffer export refused")


class RefusedExport(TypeError):
    pass


class TypeErrorSubclassBuffer:
    def __buffer__(self, flags):
        raise RefusedExport("buffer export refused")


class ValueErrorBuffer:
    def __buffer__(self, flags):
        raise ValueError("buffer export failed")


class Buffer:
    def __init__(self, data):
        self.data = bytearray(data)

    def __buffer__(self, flags):
        return memoryview(self.data)


def show(label, operation, argument):
    try:
        result = operation(argument)
    except Exception as exc:  # noqa: BLE001 - the error is the observation
        print(label, type(exc).__name__, exc)
    else:
        print(label, "ok", result)


DECODERS = (
    ("b64decode", base64.b64decode, "aGk="),
    ("standard_b64decode", base64.standard_b64decode, "aGk="),
    ("urlsafe_b64decode", base64.urlsafe_b64decode, "aGk="),
    ("b32decode", base64.b32decode, "NBUQ===="),
    ("b32hexdecode", base64.b32hexdecode, "D1KG===="),
    ("b16decode", base64.b16decode, "6869"),
    ("a85decode", base64.a85decode, "BP@"),
    ("b85decode", base64.b85decode, "XlV"),
)

for name, decode, text in DECODERS:
    show(name + " list", decode, [1, 2])
    show(name + " int", decode, 5)
    show(name + " bigint", decode, 2**100)
    show(name + " float", decode, 1.5)
    show(name + " None", decode, None)
    show(name + " dict", decode, {})
    show(name + " ascii_str", decode, text)
    show(name + " non_ascii_str", decode, "hé")
    show(name + " bytes", decode, text.encode())
    show(name + " bytearray", decode, bytearray(text.encode()))
    show(name + " memoryview", decode, memoryview(text.encode()))
    show(name + " array", decode, array.array("B", text.encode()))
    show(name + " buffer", decode, Buffer(text.encode()))
    show(name + " buffer_type_error", decode, TypeErrorBuffer())
    show(name + " buffer_type_error_subclass", decode, TypeErrorSubclassBuffer())
    show(name + " buffer_value_error", decode, ValueErrorBuffer())

# The optional alphabet arguments go through the same coercion.
show("b64decode altchars list", lambda s: base64.b64decode(b"aGk=", altchars=s), [1, 2])
show("b64decode altchars None", lambda s: base64.b64decode(b"aGk=", altchars=s), None)
show("b64decode altchars str", lambda s: base64.b64decode(b"-_8=", altchars=s), "-_")
show("b32decode map01 list", lambda s: base64.b32decode(b"NBUQ====", map01=s), [1])
show("b32decode map01 int", lambda s: base64.b32decode(b"NBUQ====", map01=s), 1)
show("b32decode map01 str", lambda s: base64.b32decode(b"NBUQ====", map01=s), "L")

LEGACY = (
    ("decodebytes", base64.decodebytes, b"aGk="),
    ("encodebytes", base64.encodebytes, b"hi"),
)

for name, operation, data in LEGACY:
    show(name + " list", operation, [1, 2])
    show(name + " int", operation, 5)
    show(name + " None", operation, None)
    show(name + " str", operation, data.decode())
    show(name + " bytes", operation, data)
    show(name + " bytearray", operation, bytearray(data))
    show(name + " memoryview", operation, memoryview(data))
    show(name + " signed_bytes", operation, array.array("b", data))
    show(name + " wide_items", operation, array.array("i", [1, 2]))
    show(name + " two_dims", operation, memoryview(b"abcd").cast("B", (2, 2)))
    show(name + " buffer_type_error", operation, TypeErrorBuffer())

show("decodebytes buffer", base64.decodebytes, Buffer(b"aGk="))
