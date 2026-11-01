"""Purpose: pickle writes and reads ints of any width as CPython does.

CPython's save_long (Modules/_pickle.c) writes BININT1/BININT2/BININT for an
int32, LONG1 or LONG4 with a minimal little-endian two's-complement payload
for anything wider from protocol 2, and INT or LONG text below protocol 2;
load_counted_long reads a payload of any width. The values cross the int32,
inline-int, 64-bit and LONG1/LONG4 boundaries.
"""

import pickle

VALUES = [
    0, 1, 255, 256, 65535, 65536,
    2**31 - 1, 2**31, -(2**31), -(2**31) - 1,
    2**40, 2**47, -(2**47), 2**63 - 1, 2**63, 2**64, -(2**63), -(2**63) - 1,
    2**80 + 3, -(2**80 + 3), 2**2048, -(2**2048) + 1,
]

for proto in (0, 2, 4, 5):
    for value in VALUES:
        data = pickle.dumps(value, proto)
        back = pickle.loads(data)
        print(proto, value.bit_length(), value < 0, data.hex()[:96], len(data), back == value, type(back).__name__)

for value in VALUES:
    print("proto1 round trip", value.bit_length(), pickle.loads(pickle.dumps(value, 1)) == value)

nested = {"big": [2**80 + 3, -(2**70)], "small": (7, -8)}
for proto in (0, 1, 2, 5):
    print("nested", proto, pickle.loads(pickle.dumps(nested, proto)) == nested)

# Below protocol 2 save_long writes repr(obj), so the int max-str-digits limit
# applies and nothing is written; the binary payloads have no such limit.
import io
import sys

old_digits = sys.get_int_max_str_digits()
sys.set_int_max_str_digits(640)
try:
    for proto in (0, 1, 2, 5):
        output = io.BytesIO()
        try:
            pickle.dump(10**640, output, proto)
            print("digit limit", proto, "accepted", pickle.loads(output.getvalue()) == 10**640)
        except ValueError as exc:
            print("digit limit", proto, type(exc).__name__, str(exc), output.getvalue() == b"")
finally:
    sys.set_int_max_str_digits(old_digits)

# Protocol 4 frames (PEP 3154): a frame commits at 64 KiB, a payload of 64 KiB
# or more is written outside any frame, and a frame shorter than 4 bytes has no
# header (Modules/_pickle.c: _Pickler_CommitFrame, _Pickler_write_bytes).
for proto in (4, 5):
    for value in ([1, 2, 3], list(range(70000)), [b"x" * 70000, "y" * 10], "z" * 65536, b""):
        data = pickle.dumps(value, proto)
        print("frames", proto, len(data), data[:24].hex(), data[-24:].hex(), pickle.loads(data) == value)

# Batching (Modules/_pickle.c: batch_list_exact, batch_dict_exact, save_set,
# batch_list, batch_dict for __reduce__ iterators): one item takes APPEND or SETITEM, larger containers
# take MARK ... APPENDS/SETITEMS/ADDITEMS per 1000 items, and an exact dict or
# set whose size is a multiple of 1000 ends with an empty batch.
import hashlib


class Bag:
    """A __reduce__ that feeds listitems and dictitems iterators."""

    def __init__(self, tag=0):
        self.tag = tag
        self.items = []
        self.pairs = {}

    def append(self, item):
        self.items.append(item)

    def __setitem__(self, key, value):
        self.pairs[key] = value

    def __reduce__(self):
        return (type(self), (self.tag,), None, iter(self.items), iter(self.pairs.items()))

    def __eq__(self, other):
        return isinstance(other, Bag) and (self.items, self.pairs) == (other.items, other.pairs)


class ExtendingBag(Bag):
    """Loading calls extend() once per APPEND or APPENDS when it exists."""

    extend_calls = 0

    def extend(self, items):
        ExtendingBag.extend_calls += 1
        self.items.extend(items)


def make_bag(size, kind=Bag):
    bag = kind(7)
    for index in range(size):
        bag.append(index)
        bag[index] = index
    return bag


for proto in (2, 4, 5):
    for size in (0, 1, 2, 999, 1000, 1001, 2000):
        listed = list(range(size))
        mapped = {index: index for index in range(size)}
        for label, value in (
            ("list", listed),
            ("dict", mapped),
            ("reduce-iterators", make_bag(size)),
            ("reduce-extend", make_bag(size, ExtendingBag)),
        ):
            data = pickle.dumps(value, proto)
            print("batch", proto, label, size, len(data), hashlib.sha256(data).hexdigest()[:16], pickle.loads(data) == value)
        if proto >= 4:
            data = pickle.dumps(set(listed), proto)
            print("batch", proto, "set", size, len(data), pickle.loads(data) == set(listed))
print("extend calls", ExtendingBag.extend_calls)
