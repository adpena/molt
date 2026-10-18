"""Purpose: differential coverage for bigint ops."""

import sys

a = 1 << 60
b = a + 123
literal = 123456789012345678901234567890

print(a)
print(b)
print(literal)
print(a + b)
print(a * 3)
print(a // 7)
print(a % 7)
print(a << 5)
print(a >> 3)
print(a | b)
print(int("123456789012345678901234567890"))
print(int(1e20))
print(round(1e20))



# A heap integer literal remains one Python object when a list repeats it.
def heap_literal_fill(count):
    values = [4611686018427387904] * count
    return len(values), values[0], values[0] is values[-1]


# Each evaluation of this code constant publishes the same object reference.
def heap_constant_comprehension():
    values = [4611686018427387904 for unused in range(3)]
    return len(values), values[0], values[0] is values[-1]


print("heap literal fill", heap_literal_fill(3))
print("heap constant comprehension", heap_constant_comprehension())


# Literal fixed-width byte oracles; conversion overrides must not run for int storage.
class ByteInt(int):
    def __int__(self):
        raise AssertionError("to_bytes must read integer payload")

    def __index__(self):
        raise AssertionError("to_bytes must read integer payload")


for value, width, signed, expected in [
    (0, 0, False, b""),
    (True, 2, False, b"\x01\x00"),
    (-129, 2, True, b"\x7f\xff"),
    (128, 2, True, b"\x80\x00"),
    ((1 << 64) + 0x1234, 9, False, b"\x34\x12\x00\x00\x00\x00\x00\x00\x01"),
    (-(1 << 128), 17, True, b"\x00" * 16 + b"\xff"),
]:
    for constructor in (int, ByteInt):
        number = constructor(value)
        little = int.to_bytes(number, width, "little", signed=signed)
        big = int.to_bytes(number, width, "big", signed=signed)
        assert little == expected
        assert big == expected[::-1]
        print("fixed bytes", constructor.__name__, value, width, little.hex(), big.hex())

# CPython 3.13 tightened empty signed exports to require a sign bit for -1.
for constructor in (int, ByteInt):
    if sys.version_info >= (3, 13):
        try:
            constructor(-1).to_bytes(0, signed=True)
        except OverflowError:
            print("empty negative bytes refused", constructor.__name__)
        else:
            raise AssertionError("empty signed export lost its required sign bit")
    else:
        assert constructor(-1).to_bytes(0, signed=True) == b""
        print("empty negative bytes", constructor.__name__)

for value, width, signed in [(-1, 0, False), (-2, 0, True), (1, 0, True),
                             (128, 1, True), (-129, 1, True)]:
    try:
        int.to_bytes(value, width, "little", signed=signed)
    except OverflowError:
        print("byte overflow", value, width, signed)
    else:
        raise AssertionError("fixed-width overflow accepted")

try:
    int.to_bytes(3.0, 1, "little")
except TypeError:
    print("float byte receiver refused")
else:
    raise AssertionError("integral float is not an int byte receiver")


class BrokenSigned:
    def __bool__(self):
        print("signed truth")
        raise ValueError("signed sentinel")


try:
    (123).to_bytes(2, "little", signed=BrokenSigned())
except ValueError as error:
    print("signed error", str(error))
else:
    raise AssertionError("signed truth error lost")

for order in ("LITTLE", "Big", "unknown", "\ud800"):
    try:
        (1).to_bytes(1, order)
    except ValueError:
        print("byteorder rejected", repr(order))
    else:
        raise AssertionError("noncanonical byteorder admitted")


# Clinic coercion order is independent of value overflow and byteorder content.
byte_trace = []


class ByteLength:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        byte_trace.append("index")
        assert (513).to_bytes(2, "little") == b"\x01\x02"
        return self.value


class ByteSigned:
    def __bool__(self):
        byte_trace.append("signed")
        assert (-129).to_bytes(2, "little", signed=True) == b"\x7f\xff"
        return True


for receiver, length, order, error_type, expected_trace, message in [
    (1, -1, "bad", ValueError, ["index", "signed"], "byteorder"),
    (1, -1, "little", ValueError, ["index", "signed"], "length"),
    (1, -1, 3, TypeError, ["index"], "byteorder"),
    (1, 1 << 100, 3, OverflowError, ["index"], ""),
    (1.0, 1, "little", TypeError, [], "descriptor"),
]:
    byte_trace.clear()
    try:
        int.to_bytes(receiver, ByteLength(length), order, signed=ByteSigned())
    except error_type as error:
        assert byte_trace == expected_trace
        assert message in str(error)
        print("byte argument order", error_type.__name__, byte_trace)
    else:
        raise AssertionError("invalid byte arguments accepted")

byte_trace.clear()
assert (1).to_bytes(ByteLength(2), "little", signed=ByteSigned()) == b"\x01\x00"
assert byte_trace == ["index", "signed"]
print("byte coercion success", byte_trace)


# CPython 3.12-3.14 Clinic defaults and keyword-only boundary. Literal byte
# results distinguish byteorder, width, sign extension and integer subclasses.
for number, expected in [(0, b"\x00"), (255, b"\xff"), (True, b"\x01"),
                         (ByteInt(17), b"\x11")]:
    assert number.to_bytes() == expected
    assert int.to_bytes(number) == expected
    print("default byte", type(number).__name__, expected.hex())
assert (513).to_bytes(2) == b"\x02\x01"
assert (513).to_bytes(length=2) == b"\x02\x01"
assert (1).to_bytes(byteorder="little") == b"\x01"
assert (-1).to_bytes(signed=True) == b"\xff"
assert (128).to_bytes(length=2, signed=True) == b"\x00\x80"
assert int.from_bytes(b"\x02\x01") == 513
assert int.from_bytes(bytes=b"\x02\x01") == 513
assert int.from_bytes(b"\x02\x01", byteorder="little") == 258
assert int.from_bytes(b"\xff", signed=True) == -1
assert bool.from_bytes(b"\x02") is True
assert bool.from_bytes(b"\x00") is False
subclass_bytes = ByteInt.from_bytes(b"\x02\x01")
assert type(subclass_bytes) is ByteInt and subclass_bytes == 513
print("codec defaults and subclass construction")

for name, call, error_type in [
    ("default overflow", lambda: (256).to_bytes(), OverflowError),
    ("missing bytes", lambda: int.from_bytes(), TypeError),
    ("to signed positional", lambda: (1).to_bytes(1, "big", False), TypeError),
    ("from signed positional", lambda: int.from_bytes(b"x", "big", False), TypeError),
    ("to duplicate length", lambda: (1).to_bytes(1, length=2), TypeError),
    ("from duplicate bytes", lambda: int.from_bytes(b"x", bytes=b"y"), TypeError),
    ("to invalid keyword", lambda: (1).to_bytes(unknown=1), TypeError),
    ("from invalid keyword", lambda: int.from_bytes(b"x", unknown=1), TypeError),
    ("from no bytes None", lambda: int.from_bytes(None), TypeError),
    ("from no integer count", lambda: int.from_bytes(3), TypeError),
    ("from invalid byte", lambda: int.from_bytes([256]), ValueError),
]:
    try:
        call()
    except error_type:
        print("codec call rejected", name)
    else:
        raise AssertionError(name)


# from_bytes uses PyObject_Bytes: buffer, iterable and __bytes__ dispatch,
# including subclass overrides. An index-only object is never a byte count.
byte_trace.clear()


class ByteSource:
    def __bytes__(self):
        byte_trace.append("bytes")
        return b"\x02\x01"

    def __iter__(self):
        raise AssertionError("__bytes__ must win over iteration")


class OverriddenBytes(bytes):
    def __bytes__(self):
        byte_trace.append("bytes subclass")
        return b"\x02\x01"


for source in [bytearray(b"\x02\x01"), memoryview(b"\x02\x01"),
               memoryview(b"\x02x\x01y")[::2], [2, 1], (2, 1), iter([2, 1]),
               ByteSource(), OverriddenBytes(b"wrong")]:
    assert int.from_bytes(source) == 513
    print("from byte source", type(source).__name__)
assert byte_trace == ["bytes", "bytes subclass"]


class InvalidByteSource:
    def __bytes__(self):
        byte_trace.append("invalid bytes")
        return "not bytes"  # noqa: PLE0308 - adversarial protocol result


class FromSigned:
    def __init__(self, fail=False):
        self.fail = fail

    def __bool__(self):
        byte_trace.append("signed")
        if self.fail:
            raise LookupError("signed from sentinel")
        return False


for order, fail, error_type, expected_trace in [
    ("BIG", False, ValueError, ["signed"]),
    ("little", True, LookupError, ["signed"]),
    ("wrong", True, LookupError, ["signed"]),
    (None, False, TypeError, []),
    (3, True, TypeError, []),
]:
    byte_trace.clear()
    try:
        int.from_bytes(ByteSource(), order, signed=FromSigned(fail))
    except error_type:
        assert byte_trace == expected_trace
        print("from byte argument order", error_type.__name__, byte_trace)
    else:
        raise AssertionError("from_bytes argument conversion order lost")
byte_trace.clear()
assert int.from_bytes(ByteSource(), signed=FromSigned()) == 513
assert byte_trace == ["signed", "bytes"]
print("from byte coercion success", byte_trace)
try:
    int.from_bytes(InvalidByteSource())
except TypeError as error:
    assert "__bytes__ returned non-bytes" in str(error)
    print("invalid byte conversion result rejected")
else:
    raise AssertionError("from_bytes accepted non-bytes __bytes__ result")


class ByteOrder(str):
    def __eq__(self, other):
        raise AssertionError("byteorder must compare Unicode storage")


class ByteKeyword(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        raise AssertionError("Clinic must compare Unicode keyword storage")


assert (513).to_bytes(2, ByteOrder("little")) == b"\x01\x02"
assert int.from_bytes(b"\x01\x02", ByteOrder("little")) == 513
assert (513).to_bytes(**{ByteKeyword("length"): 2}) == b"\x02\x01"
assert int.from_bytes(**{ByteKeyword("bytes"): b"\x02\x01"}) == 513
print("byte codecs compare Unicode storage")

for codec, first in [((1).to_bytes, 1), (int.from_bytes, b"x")]:
    try:
        codec(first, None)
    except TypeError as error:
        assert str(error) == codec.__name__ + "() argument 'byteorder' must be str, not None"
        print("None byteorder diagnostic", codec.__name__)
    else:
        raise AssertionError("None byteorder accepted")
