"""Integer conversions raise CPython's TypeError text only when they fail.

The success path builds no message; the failure path keeps each message:
the __index__ protocol, bytearray and bytes items, sequence repetition, and
the bin/oct/hex builtins, for an inline int, a big int, a bool, an object
with __index__, and a value without it.
"""


class Index:
    def __index__(self):
        return 7


def attempt(label, fn):
    try:
        print(label, fn())
    except (TypeError, ValueError, OverflowError) as exc:
        print(label, type(exc).__name__, exc)


data = bytearray(3)
for value in (65, True, Index(), 300, -1, 2.5, "x", None):
    attempt("store", lambda v=value: (data.__setitem__(0, v), bytes(data))[1])
    attempt("bytes", lambda v=value: bytes([v]))
    attempt("bin", lambda v=value: bin(v))
    attempt("oct", lambda v=value: oct(v))
    attempt("hex", lambda v=value: hex(v))
    attempt("repeat", lambda v=value: [0] * v)
attempt("big", lambda: hex(1 << 70))
attempt("index-key", lambda: [1, 2, 3][Index() if False else 1.0])
