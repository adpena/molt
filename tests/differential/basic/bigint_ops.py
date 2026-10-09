"""Purpose: differential coverage for bigint ops."""

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
    (-1, 0, True, b""),
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
