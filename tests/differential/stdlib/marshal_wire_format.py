"""Purpose: CPython marshal wire, heap sharing, historical forms and file cursor parity.

The scalar identity deviation is explicit in docs/design/marshal.md. Non-cached
numeric objects are unshared in the exact-byte corpus; value round trips also
cover CPython's shared numeric records.
"""

import io
import marshal
import math
import sys


def equivalent(left, right, sharing=False):
    """Graph isomorphism, including unordered NaN keys and buffer conversion."""
    def match(a, b, forward, backward):
        kind = type(a)
        if kind in (bytearray, memoryview):
            expected = bytes(a)
            if type(b) is not bytes or expected != b:
                return None
        elif kind is not type(b):
            return None
        if kind is float:
            if math.isnan(a):
                return (forward, backward) if math.isnan(b) else None
            if a != b or (a == 0.0 and math.copysign(1.0, a) != math.copysign(1.0, b)):
                return None
            return forward, backward
        if kind is complex:
            state = match(a.real, b.real, forward, backward)
            return None if state is None else match(a.imag, b.imag, *state)
        if kind in (int, bool, type(None), type(Ellipsis), type):
            return (forward, backward) if a == b else None
        if sharing:
            ai, bi = id(a), id(b)
            if ai in forward or bi in backward:
                return (forward, backward) if forward.get(ai) == bi and backward.get(bi) == ai else None
            forward, backward = dict(forward), dict(backward)
            forward[ai], backward[bi] = bi, ai
        if kind in (tuple, list):
            if len(a) != len(b):
                return None
            state = forward, backward
            for av, bv in zip(a, b):
                state = match(av, bv, *state)
                if state is None:
                    return None
            return state
        if kind is slice:
            state = forward, backward
            for av, bv in zip((a.start, a.stop, a.step), (b.start, b.stop, b.step)):
                state = match(av, bv, *state)
                if state is None:
                    return None
            return state
        if kind in (dict, set, frozenset):
            if len(a) != len(b):
                return None
            left_items = list(a.items()) if kind is dict else [(v, None) for v in a]
            right_items = list(b.items()) if kind is dict else [(v, None) for v in b]
            def pair(index, remaining, state):
                if index == len(left_items):
                    return state
                ak, av = left_items[index]
                for j in remaining:
                    bk, bv = right_items[j]
                    candidate = match(ak, bk, *state)
                    if candidate is not None:
                        candidate = match(av, bv, *candidate)
                    if candidate is not None:
                        result = pair(index + 1, [k for k in remaining if k != j], candidate)
                        if result is not None:
                            return result
                return None
            return pair(0, list(range(len(right_items))), (forward, backward))
        return (forward, backward) if a == b else None
    return match(left, right, {}, {}) is not None


values = [
    None, False, True, StopIteration, Ellipsis,
    -5, 0, 256, 257, -(2**31), 2**31, -(2**130), 2**129 + 32769,
    0.0, -0.0, 1.25, 1e-20, 1e20, float("inf"), float("-inf"), float("nan"),
    complex(1.25, -0.0), complex(float("nan"), float("inf")),
    "", "ascii text", "x" * 255, "x" * 256, "é漢", "\ud800\udfff",
    b"", b"\0\xffdata", (), [], {"answer": 42}, {1, 2, 3}, frozenset({1, 2}),
    (1, [2, 3], {"nested": b"bytes"}),
    bytearray(b"buffer"), memoryview(b"view"),
    {float("nan"): [1], float("nan"): [2]},
    {float("nan"), float("nan")},
]
heap_values = [
    bytes(bytearray(b"shared bytes")), "".join(["shared", " text"]),
    tuple(range(3)), [], {}, {1, 2}, frozenset({1, 2}), bytearray(b"shared buffer"),
]
values.extend([[value, value] for value in heap_values])
slice_index = -1
if marshal.version >= 5:
    shared_slice = slice([1], None, None)
    slice_index = len(values)
    values.append([shared_slice, shared_slice])
supported_cells = [
    (version, index)
    for version in range(marshal.version + 1)
    for index in range(len(values))
    if version >= 5 or index != slice_index
]
checks = 0
for version, index in supported_cells:
    value = values[index]
    decoded = marshal.loads(marshal.dumps(value, version))
    assert equivalent(value, decoded, version >= 3), (version, type(value))
    if isinstance(value, float) and value == 0.0:
        assert math.copysign(1.0, value) == math.copysign(1.0, decoded)
    checks += 1

# Versions <=2 do not encode refcounts or identities; their complete bytes are
# deterministic, including numeric records and every string/tuple form.
for version in range(3):
    payload = (None, True, -7, 32768, 2**70, -0.0, 1.25, 1j, "é\ud800", b"a\xff", [1, 2], {1: 2}, {1, 2})
    print("historical", version, marshal.dumps(payload, version).hex())

# All non-cached scalar occurrences are fresh on CPython. Every string's
# interning state is chosen explicitly at runtime on both sides.
word = sys.intern("marshal" + str(len(values)))
child = [1, word, b"data"]
payload = [child, child, float("1.25"), int("1234567"), complex("1+2j")]
for version in range(3, marshal.version + 1):
    blob = marshal.dumps(payload, version)
    decoded = marshal.loads(blob)
    assert decoded[0] is decoded[1]
    assert decoded[0][1] == word
    print("wire", version, blob.hex())
    checks += 1

# The oracle must reject plausible defects: collapsed NaN keys, lost aliases,
# spurious aliases, and a changed sign of zero, even nested in unordered values.
assert not equivalent({float("nan"), float("nan")}, {float("nan")})
assert not equivalent({float("nan"): -0.0}, {float("nan"): 0.0})
shared = [123]
assert not equivalent([shared, shared], [[123], [123]], True)
assert not equivalent([[123], [123]], [shared, shared], True)

recursive = []
recursive.append(recursive)
tuple_cycle = ([],)
tuple_cycle[0].append(tuple_cycle)
for version in range(3, marshal.version + 1):
    decoded = marshal.loads(marshal.dumps(recursive, version))
    assert decoded[0] is decoded
    decoded = marshal.loads(marshal.dumps(tuple_cycle, version))
    assert decoded[0][0] is decoded
    checks += 2

historical = [
    (b"I\xff\xff\xff\xff\xff\xff\xff\x7f", 2**63 - 1),
    (b"f\x041.25", 1.25),
    (b"a\x01\0\0\0\xff", "ÿ"),
    (b"t\x03\0\0\0abc", "abc"),
]
for blob, expected in historical:
    assert marshal.loads(blob) == expected
    checks += 1

stream = io.BytesIO()
positions = []
for value in (1, "abc", b"def", 1.0, (2, "a", ["b", b"c"])):
    marshal.dump(value, stream)
    stream.write(b"0123")
    positions.append(stream.tell())
stream.seek(0)
for value, position in zip((1, "abc", b"def", 1.0, (2, "a", ["b", b"c"])), positions):
    assert marshal.load(stream) == value
    assert stream.read(4) == b"0123"
    assert stream.tell() == position
    checks += 1


class BadReader(io.BytesIO):
    def readinto(self, buffer):
        count = super().readinto(buffer)
        return count + 1000000 if count > 4 else count


for value in (1.0, 1j, b"0123456789", "0123456789"):
    try:
        marshal.load(BadReader(marshal.dumps(value)))
    except ValueError as error:
        assert str(error).startswith("read() returned too much data:")
        checks += 1
    else:
        raise AssertionError("over-report accepted")

for blob in (b"", b"i\0", b"s\x05\0\0\0xx", b")\x01"):
    try:
        marshal.loads(blob)
    except EOFError:
        checks += 1
    else:
        raise AssertionError("truncation accepted")

for version in (4, 5):
    value = slice([1, 2], "stop", None)
    try:
        decoded = marshal.loads(marshal.dumps(value, version))
    except ValueError as error:
        assert marshal.version < 5 or version < 5
        assert str(error) == "unmarshallable object"
    else:
        assert marshal.version >= 5 and version >= 5
        assert equivalent(value, decoded, True)
    checks += 1

# Equal standalone dumps do not imply orderable Python values. CPython's
# pre-3.14 pair-sort failure becomes MemoryError; 3.14's index disambiguates.
for constructor in (set, frozenset):
    sample = constructor([complex(float("nan"), 0), complex(float("nan"), 0)])
    for version in range(marshal.version + 1):
        try:
            blob = marshal.dumps(sample, version)
        except MemoryError:
            assert sys.version_info < (3, 14)
        else:
            assert equivalent(sample, marshal.loads(blob), version >= 3)
        checks += 1
print("checks", checks)

# This named AOT refusal is a declared compatibility boundary, not a decoder
# success. CPython consumes its own code record; Molt must refuse that tag.
if sys.implementation.name == "molt":
    try:
        marshal.loads(b"c")
    except ValueError as error:
        assert str(error) == "cannot unmarshal code objects across Molt's AOT boundary"
    else:
        raise AssertionError("code record escaped the AOT boundary")

# Optional evidence mode exercises both directions without asking a same-runtime
# round trip to prove interoperability. The external proof driver writes these
# rows with the exact target CPython and validates emitted rows with CPython.
if len(sys.argv) == 2:
    cross_checks = 0
    with open(sys.argv[1], "r") as fixtures:
        for row in fixtures:
            version_text, index_text, payload_hex = row.split(" ")
            version, index = int(version_text), int(index_text)
            result = marshal.loads(bytes.fromhex(payload_hex))
            assert equivalent(values[index], result, version >= 3), (version, index)
            print("cross", version, index, marshal.dumps(values[index], version).hex())
            cross_checks += 1
    # CPython's shared scalar record is deliberately outside exact-byte scope.
    result = marshal.loads(bytes.fromhex("db02000000e7000000000000f43f7201000000"))
    assert equivalent([1.25, 1.25], result, True)
    scalar = float("1.25")
    print("shared-scalar", marshal.dumps([scalar, scalar]).hex())
    print("cross-checks", cross_checks)
