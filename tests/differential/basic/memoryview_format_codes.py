"""Purpose: differential coverage for memoryview format codes."""

import array


arr = array.array("h", [1, 2, 3])
mv = memoryview(arr)
print(mv.format, mv.tolist())

mvb = mv.cast("B")
print(mvb.format, mvb.tolist())


import math
import struct
import sys


def outcome(action):
    try:
        value = action()
    except Exception as error:
        return type(error).__name__, str(error)
    return value


def store(view, key, value):
    view[key] = value


for shape in ([2 ** 30] * 5, [sys.maxsize, 2, "later"], [1 << 100]):
    print("cast shape overflow", outcome(lambda: memoryview(b"x").cast("B", shape=shape)))
print("cast itemsize overflow", outcome(lambda: memoryview(b"abcdefgh").cast("h", shape=[sys.maxsize])))


half = memoryview(bytearray(8)).cast("e")
for index, value in enumerate((0.0, -0.0, 2 ** -24, 65504.0)):
    half[index] = value
print("half values", [x.hex() for x in half])
print("half bytes", half.tobytes() == struct.pack("@4e", 0.0, -0.0, 2 ** -24, 65504.0))
print("half list", [x.hex() for x in half.tolist()])
for value in (1.0 + 2 ** -11, 1.0 + 3 * 2 ** -11, 2 ** -25, 65520.0, float("inf"), float("nan")):
    result = outcome(lambda: store(half, 0, value))
    print("half store", result, math.isnan(half[0]) if math.isnan(value) else half[0].hex())

# Unicode arrays export a valid native code that scalar memoryview operations
# intentionally reject. Slicing preserves the descriptor and empty iteration
# never demands a scalar. This covers the deferred scalar/syntax distinction.
unicode_view = memoryview(array.array("u", "xy"))
print("scalar format", unicode_view.format)
print("scalar read", outcome(lambda: unicode_view[0]))
print("scalar write", outcome(lambda: store(unicode_view, 0, "z")))
print("scalar list", outcome(lambda: unicode_view.tolist()))
print("scalar next", outcome(lambda: next(iter(unicode_view))))
print("scalar slice", unicode_view[:0].shape, list(unicode_view[:0]), unicode_view[:0].tolist())



def typed_format_loop(view: memoryview):
    values = []
    for value in view:
        values.append(value)
    return values


print("typed scalar loop", outcome(lambda: typed_format_loop(unicode_view)))
print("typed empty scalar loop", typed_format_loop(unicode_view[:0]))
unsupported_iterator = iter(unicode_view)
for _ in range(4):
    print("unsupported iterator", outcome(lambda: next(unsupported_iterator)))


class ScalarCallbackFailure:
    def __init__(self, error):
        self.error = error

    def __index__(self):
        raise self.error("scalar callback")

    def __float__(self):
        raise self.error("scalar callback")

    def __bool__(self):
        raise self.error("scalar callback")


class ScalarValueError(ValueError):
    pass


for code in ("B", "h", "f", "d", "e", "?"):
    view = memoryview(bytearray(struct.calcsize("@" + code))).cast(code)
    for error in (TypeError, OverflowError, ValueError, ScalarValueError, LookupError):
        print(
            "scalar callback", code, error.__name__,
            outcome(lambda: store(view, 0, ScalarCallbackFailure(error))),
        )


class ReleasingScalarConversion:
    def __init__(self, view, value, calls, error=None, resize_owner=None):
        self.view = view
        self.value = value
        self.calls = calls
        self.error = error
        self.resize_owner = resize_owner

    def convert(self, protocol):
        self.calls.append(protocol)
        self.view.release()
        if self.resize_owner is not None:
            self.resize_owner.append(99)
        if self.error is not None:
            raise self.error("released scalar callback")
        return self.value

    def __index__(self):
        return self.convert("index")

    def __float__(self):
        return self.convert("float")

    def __bool__(self):
        return self.convert("bool")


# C-width conversion failure comes first, release second, final destination
# range/half packing third. Every row prints the exact exception and no-write
# result; same-class ValueErrors cannot conceal the wrong phase.
for code, value in (
    ("B", 120), ("B", 300), ("B", -1), ("B", 1 << 100),
    ("b", 128), ("h", 32768), ("I", 1 << 100), ("q", 1 << 100),
    ("Q", -1), ("n", 1 << 100), ("N", -1),
    ("?", True), ("f", 1.25), ("d", 1.25), ("e", 1.25), ("e", 65520.0),
):
    owner = bytearray(struct.calcsize("@" + code))
    view = memoryview(owner).cast(code)
    calls = []
    scalar = ReleasingScalarConversion(view, value, calls)
    print("released scalar", code, value, outcome(lambda: store(view, 0, scalar)), calls, not any(owner))
    owner.append(99)
    print("released scalar owner", len(owner) == struct.calcsize("@" + code) + 1)

# A callback error wins over the release it performed. Numeric conversion
# translates type/value/range errors; truth conversion keeps its exact error.
for code, value in (("B", 1), ("?", True), ("f", 1.0), ("d", 1.0), ("e", 1.0)):
    for error in (TypeError, OverflowError, ValueError, ScalarValueError, LookupError):
        owner = bytearray(struct.calcsize("@" + code))
        view = memoryview(owner).cast(code)
        calls = []
        scalar = ReleasingScalarConversion(view, value, calls, error=error)
        print("release and failure", code, error.__name__, outcome(lambda: store(view, 0, scalar)), calls, not any(owner))
        owner.append(99)

# Scalar conversion retains no pointer into the destination across callbacks.
for code, value in (("B", 300), ("?", True), ("f", 1.0), ("d", 1.0), ("e", 65520.0)):
    owner = bytearray(struct.calcsize("@" + code))
    view = memoryview(owner).cast(code)
    calls = []
    scalar = ReleasingScalarConversion(view, value, calls, resize_owner=owner)
    print("scalar resize", code, outcome(lambda: store(view, 0, scalar)), calls, owner[-1], not any(owner[:-1]))

# Pointer packing admits actual integers, including signed negative values,
# but must never invoke an object's __index__ callback.
pointer = memoryview(bytearray(struct.calcsize("@P"))).cast("P")
print("pointer negative", outcome(lambda: store(pointer, 0, -1)), pointer[0] == (1 << (8 * struct.calcsize("@P"))) - 1)
calls = []
scalar = ReleasingScalarConversion(pointer, 0, calls)
print("pointer protocol", outcome(lambda: store(pointer, 0, scalar)), calls, len(pointer))
pointer.release()


class ReleasingStoreKey:
    def __init__(self, view, index, calls, owner=None):
        self.view = view
        self.index = index
        self.calls = calls
        self.owner = owner

    def __index__(self):
        self.calls.append("key")
        self.view.release()
        if self.owner is not None:
            self.owner.append(99)
        return self.index


class FollowingStoreValue:
    def __init__(self, calls, error=None):
        self.calls = calls
        self.error = error

    def __index__(self):
        self.calls.append("value")
        if self.error is not None:
            raise self.error("value after key release")
        return 1


for tuple_key in (False, True):
    for index, value_kind in ((9, "valid"), (0, "type"), (0, "range"), (0, "wide"), (0, "callback"), (0, "callback error")):
        owner = bytearray(4)
        view = memoryview(owner)
        calls = []
        key = ReleasingStoreKey(view, index, calls, owner)
        value = {
            "valid": 1, "type": "x", "range": 300, "wide": 1 << 100,
            "callback": FollowingStoreValue(calls),
            "callback error": FollowingStoreValue(calls, LookupError),
        }[value_kind]
        print("store key precedence", tuple_key, index, value_kind,
              outcome(lambda: store(view, (key,) if tuple_key else key, value)), calls, owner)

# Unsupported native scalar formats beat release after key conversion without
# running the value protocol; syntax/readonly still remain operation admission.
view = memoryview(array.array("u", "xy"))
calls = []
key = ReleasingStoreKey(view, 0, calls)
print("unsupported store key", outcome(lambda: store(view, key, FollowingStoreValue(calls))), calls)

for first in (0, 9):
    view = memoryview(bytearray(4)).cast("B", shape=[2, 2])
    calls = []
    key = ReleasingStoreKey(view, first, calls)
    later = FollowingStoreValue(calls, LookupError)
    print("tuple store later callback", first,
          outcome(lambda: store(view, (key, later), "x")), calls)

# All exporter types use the same source acquisition and copy authority.
destination = memoryview(array.array("h", [0, 0, 0]))
destination[:] = array.array("h", [7, 8, 9])
print("array source assignment", destination.tolist())
destination.release()
