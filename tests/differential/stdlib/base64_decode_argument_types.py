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
import binascii


# Memoryview distinguishes a missing buffer slot from a real exporter failure.
# The same diagnostic must hold for inline and heap-represented non-exporters.
for value in ([], {}, 5, 2**100, "text", 1.5, None):
    try:
        memoryview(value)
    except TypeError as exc:
        print("memoryview admission", type(value).__name__, str(exc), exc.__context__)


class TypeErrorBuffer:
    def __init__(self):
        self.error = TypeError("buffer export refused")

    def __buffer__(self, flags):
        raise self.error


class RefusedExport(TypeError):
    pass


class TypeErrorSubclassBuffer:
    def __init__(self):
        self.error = RefusedExport("buffer export refused")

    def __buffer__(self, flags):
        raise self.error


class ValueErrorBuffer:
    def __init__(self):
        self.error = ValueError("buffer export failed")

    def __buffer__(self, flags):
        raise self.error


class Buffer:
    def __init__(self, data):
        self.data = bytearray(data)

    def __buffer__(self, flags):
        return memoryview(self.data)


# All a2b consumers share Clinic's ascii_buffer policy. Failed exports are
# replaced, while an enclosing handled exception remains the implicit context.
for decode in (
    binascii.a2b_base64,
    binascii.a2b_hex,
    binascii.unhexlify,
    binascii.a2b_qp,
    binascii.a2b_uu,
):
    for value in ([], "hé", TypeErrorBuffer(), ValueErrorBuffer(), memoryview(b"aXbX")[::2]):
        try:
            decode(value)
        except Exception as exc:
            print("ascii buffer", decode.__name__, type(value).__name__, type(exc).__name__, str(exc), exc.__context__)
    outer = LookupError("outer handler")
    try:
        raise outer
    except LookupError:
        refused = ValueErrorBuffer()
        try:
            decode(refused)
        except TypeError as exc:
            print("ascii buffer context", decode.__name__, exc is refused.error, exc.__context__ is outer, exc.__cause__, exc.__suppress_context__)


def show(label, operation, argument):
    try:
        result = operation(argument)
    except Exception as exc:  # noqa: BLE001 - the error is the observation
        print(label, type(exc).__name__, exc)
        context = exc.__context__
        print(
            label,
            "chain",
            type(context).__name__,
            str(context),
            type(exc.__cause__).__name__,
            exc.__suppress_context__,
        )
        if isinstance(argument, (TypeErrorBuffer, TypeErrorSubclassBuffer)):
            print(label, "original-context", context is argument.error)
        if isinstance(argument, ValueErrorBuffer):
            print(label, "original-error", exc is argument.error)
        if isinstance(context, UnicodeEncodeError):
            print(label, "unicode-context", context.args)
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


class EncodedText(str):
    def encode(self, encoding="utf-8", errors="strict"):
        print("encode override", encoding, errors)
        return self.result


class EncodedErrorText(str):
    def encode(self, encoding="utf-8", errors="strict"):
        raise self.error


class ObservedBytes(bytes):
    def __len__(self):
        print("bytes len")
        return super().__len__()

    def __getitem__(self, index):
        print("bytes index", index)
        return super().__getitem__(index)

    def translate(self, *args):
        print("bytes translate")
        return super().translate(*args)

    def rstrip(self, *args):
        print("bytes rstrip")
        return super().rstrip(*args)


class ObservedBytearray(bytearray):
    def __len__(self):
        print("bytearray len")
        return super().__len__()

    def __getitem__(self, index):
        print("bytearray index", index)
        return super().__getitem__(index)

    def translate(self, *args):
        print("bytearray translate")
        return super().translate(*args)

    def rstrip(self, *args):
        print("bytearray rstrip")
        return super().rstrip(*args)


for name, decode, text in DECODERS:
    data = text.encode()
    for result in (data, None, 7, text, list(data), memoryview(data)):
        argument = EncodedText("ignored")
        argument.result = result
        show(name + " encode result " + type(result).__name__, decode, argument)
    for error in (
        TypeError("encode override failed"),
        UnicodeEncodeError("ascii", "hé", 1, 2, "ordinal not in range(128)"),
    ):
        argument = EncodedErrorText("ignored")
        argument.error = error
        try:
            decode(argument)
        except Exception as exc:  # noqa: BLE001 - observe owner identity
            print(name, "encode error", type(exc).__name__, exc)
            print(name, "encode error identity", exc is error, exc.__context__ is error)
            print(name, "encode error cause", exc.__cause__, exc.__suppress_context__)
    show(name + " bytes subtype", decode, ObservedBytes(data))
    show(name + " bytearray subtype", decode, ObservedBytearray(data))

# The optional alphabet arguments go through the same coercion.
show("b64decode altchars list", lambda s: base64.b64decode(b"aGk=", altchars=s), [1, 2])
show("b64decode altchars None", lambda s: base64.b64decode(b"aGk=", altchars=s), None)
show("b64decode altchars str", lambda s: base64.b64decode(b"-_8=", altchars=s), "-_")
show("b32decode map01 list", lambda s: base64.b32decode(b"NBUQ====", map01=s), [1])
show("b32decode map01 int", lambda s: base64.b32decode(b"NBUQ====", map01=s), 1)
show("b32decode map01 str", lambda s: base64.b32decode(b"NBUQ====", map01=s), "L")
show("b32decode padding before map01", lambda s: base64.b32decode(b"bad", map01=s), [1])
show(
    "b64decode duplicate alphabet",
    lambda s: base64.b64decode(b"AAAA", altchars=s),
    b"AA",
)
show(
    "a85decode custom ignored byte",
    lambda s: base64.a85decode(s, ignorechars=b"~"),
    b"B~P@",
)

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

# Legacy decode consumes a contiguous PyBUF_SIMPLE buffer after checking its
# format/dimensions; modern decode snapshots a strided memoryview instead.
strided = memoryview(b"aXGXkX=X")[::2]
show("decodebytes strided", base64.decodebytes, strided)
show("b64decode strided", base64.b64decode, strided)


class SecondExportError:
    def __init__(self):
        self.exports = 0
        self.error = TypeError("second export refused")

    def __buffer__(self, flags):
        self.exports += 1
        if self.exports == 2:
            raise self.error
        return memoryview(b"aGk=")


second = SecondExportError()
try:
    base64.decodebytes(second)
except TypeError as exc:
    print("decodebytes second export", str(exc), exc is second.error, second.exports)

# Encoding never applies decode coercion to its alphabet. Data export happens
# before length, repr, and buffer conversion of a mutable or user alphabet.
for alphabet in (b"-_", bytearray(b"-_"), memoryview(b"-_"), "-_", [45, 95], 1, b"-"):
    show(
        "b64encode alphabet " + type(alphabet).__name__,
        lambda alt: base64.b64encode(b"\xfb\xff", alt),
        alphabet,
    )


class MutatingData:
    def __init__(self, alphabet):
        self.alphabet = alphabet

    def __buffer__(self, flags):
        self.alphabet[:] = b"!?"
        return memoryview(b"\xfb\xff")


mutable_alphabet = bytearray(b"-_")
print(
    "b64encode mutation",
    base64.b64encode(MutatingData(mutable_alphabet), mutable_alphabet),
)
show("b64encode data error first", lambda s: base64.b64encode(s, 1), TypeErrorBuffer())

try:
    raise LookupError("outer handled error")
except LookupError as outer:
    argument = TypeErrorBuffer()
    try:
        base64.b64decode(argument)
    except TypeError as exc:
        print(
            "outer preserved",
            exc.__context__ is argument.error,
            argument.error.__context__ is outer,
        )

for strict in (False, True):
    for encoded in (
        b"",
        b"aGk=",
        b"aGk",
        b"A",
        b"AAAAA",
        b"=AAA",
        b"A!A",
        b" aGk=\n",
        b"A===",
        b"AAAA====",
        b"AA==junk",
        b"AA=A",
        b"====",
        b"=",
        b"AA==\n",
    ):
        label = "base64 state " + repr((strict, encoded))
        show(label, lambda data: binascii.a2b_base64(data, strict_mode=strict), encoded)
        show(
            label + " facade",
            lambda data: base64.b64decode(data, validate=strict),
            encoded,
        )

# Cross both bulk transitions while a partial quantum or padding is live.
# Retained bytes inside an otherwise ignored run must still reach the parser.
for prefix in (b"", b"A", b"AA", b"AAA", b"AA=", b"AAAA"):
    for ignored in (
        b"!" * 17,
        b"\xff" * 33,
        b" " * 64,
        b"!" * 16 + b"=" + b"!" * 16,
        b"!" * 16 + b"Q" + b"!" * 16,
        b"!" * 16 + b"/" + b"!" * 16,
    ):
        for strict in (False, True):
            encoded = prefix + ignored + b"AA=="
            label = "base64 bulk state " + repr((strict, encoded))
            show(
                label,
                lambda data: binascii.a2b_base64(data, strict_mode=strict),
                encoded,
            )

print("binascii Error bases", binascii.Error.__bases__ == (ValueError,))
print("binascii Incomplete bases", binascii.Incomplete.__bases__ == (Exception,))
print(
    "binascii class names",
    binascii.Error.__module__,
    binascii.Error.__name__,
    binascii.Incomplete.__module__,
    binascii.Incomplete.__name__,
)
original_error = binascii.Error
binascii.Error = LookupError
try:
    binascii.a2b_base64(b"A")
except Exception as exc:  # noqa: BLE001 - native state is independent of the binding
    print("binascii original class", type(exc) is original_error)
finally:
    binascii.Error = original_error


class StrictFlag:
    def __bool__(self):
        print("strict truth")
        return True


show(
    "binascii strict protocol",
    lambda data: binascii.a2b_base64(data, strict_mode=StrictFlag()),
    b"aGk=",
)
show(
    "binascii input before strict",
    lambda data: binascii.a2b_base64(data, strict_mode=StrictFlag()),
    ValueErrorBuffer(),
)


class EqualityTrap(type):
    def __eq__(self, other):
        raise RuntimeError("type comparison must be identity-only")


class BufferWithMetaclass(metaclass=EqualityTrap):
    def __buffer__(self, flags):
        return memoryview(b"aGk=")


show("pure native guard", base64.b64decode, BufferWithMetaclass())
show("binascii no positional", lambda _: binascii.a2b_base64(), None)
show("binascii data keyword", lambda _: binascii.a2b_base64(data=b"AA=="), None)
show("binascii extra positional", lambda _: binascii.a2b_base64(b"AA==", True), None)
show(
    "binascii unknown keyword", lambda _: binascii.a2b_base64(b"AA==", other=True), None
)

# Python suffixes read the public Error binding; C primitives keep their native
# module-state class. Invalid base32 digits also retain the real KeyError context.
for replacement_kind in ("class", "callable", "noncallable"):
    events = []
    if replacement_kind == "class":
        replacement_error = LookupError
    elif replacement_kind == "callable":

        def replacement_error(*args):
            events.append(args)
            return LookupError(*args)

    else:
        replacement_error = 17
    binascii.Error = replacement_error
    try:
        for operation, encoded in (
            (base64.b64decode, b"A"),
            (base64.b32decode, b"!======="),
            (base64.b32hexdecode, b"!======="),
            (base64.b16decode, b"!0"),
            (base64.b16decode, b"aa"),
            (base64.b16decode, b"A"),
        ):
            show(
                "Error binding "
                + replacement_kind
                + " "
                + operation.__name__
                + repr(encoded),
                operation,
                encoded,
            )
        print("Error callable events", replacement_kind, events)
    finally:
        binascii.Error = original_error
